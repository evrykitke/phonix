//! "I forgot my password": a code by email, then a new password.
//!
//! Two calls, [`request`] and [`redeem`], and almost everything below is about
//! what they refuse to tell the caller.
//!
//! # The whole feature is an account oracle unless it is built not to be
//!
//! Anybody can type any address into this form. If the answer differs at all
//! between an address that has an account here and one that does not - a
//! different message, a different field, a *noticeably different response
//! time* - then the form is a free membership check against the workspace, and
//! the first thing it will be used for is confirming which of a leaked address
//! list belongs to this customer.
//!
//! So [`request`] returns [`ResetRequest::Accepted`] for every well-formed
//! address, and the three ways it can differ underneath are each closed
//! deliberately:
//!
//! * **What it says.** One answer, on both paths.
//! * **What it does.** No work at all happens for an unknown address, but the
//!   work that happens for a known one is a token insert - microseconds, and
//!   under the noise of the round trip.
//! * **How long it takes.** This is the one that would have leaked. Handing a
//!   message to an SMTP relay takes between a hundred milliseconds and several
//!   seconds, and a response that is reliably slower for real accounts is a
//!   perfectly good oracle measured over a few requests. The send is therefore
//!   moved off the request entirely - see [`send_in_background`].
//!
//! [`redeem`] has the same problem in a different shape: a wrong code and an
//! address with no account must be the same answer, which is why the outcome
//! for both is [`ResetOutcome::NotUsable`].
//!
//! # Why the code is spent before the password is judged
//!
//! `redeem` consumes the code first and only then looks at the new password, so
//! a password the workspace policy refuses costs the user their code. That is
//! the unfriendly order and it is the necessary one.
//!
//! The alternative - judge the password first, so a typo is recoverable - means
//! answering "that password is too short" for an address that has an account
//! and "that code is not usable" for one that does not, before any code has
//! been checked. The friendlier order reintroduces the oracle the whole module
//! exists to close.
//!
//! The cost is paid where it does not hurt: the form applies the same policy in
//! the browser before it submits, so a password that would be refused here is
//! refused there, with the code still unspent.
//!
//! [`super::invitation::accept`] reaches the same order from the other
//! direction - there, a link that survives a failed attempt is a link that can
//! be probed.

use phonix_config::AppConfig;
use phonix_core::identity::{FieldError, UserStatus};
use phonix_db::identity::one_time_token::TokenPurpose;
use phonix_db::identity::user::UserRecord;
use phonix_db::identity::{AuditEntry, IdentityEvent, audit, one_time_token, user as user_store};
use phonix_db::sqlx::PgPool;
use secrecy::{ExposeSecret, SecretString};

use crate::caller::Caller;
use crate::crypto::code;
use crate::error::ServiceResult;
use crate::mail;
use crate::{SecretVault, Security};

/// What a workspace needs to send one of these.
///
/// Grouped rather than passed loose for the same reason
/// [`super::invitation::Inviting`] is: five borrowed things at two call sites
/// is an argument list nobody can read, and the workspace's own name is in the
/// message.
pub struct Resetting<'a> {
    pub config: &'a AppConfig,
    pub vault: &'a SecretVault,
    /// Appears in the message, so somebody with three workspaces knows which
    /// one this is about.
    pub workspace_name: &'a str,
}

/// What [`request`] did, as far as anybody outside is allowed to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetRequest {
    /// The request was well-formed and has been dealt with.
    ///
    /// Says nothing about whether an account exists, whether a code was
    /// issued, or whether any mail was sent - by design. See the module note.
    Accepted,
    /// Self-service reset is switched off for this deployment.
    Disabled,
}

/// What [`redeem`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetOutcome {
    /// The password is changed and every session is gone.
    Reset,
    /// The code was wrong, expired, exhausted, already used, or belonged to an
    /// address with no account here. Five cases, one answer.
    NotUsable,
    /// The code was good and the new password is not acceptable. Reaching this
    /// means the code is spent - see the module note on ordering.
    Rejected(Vec<FieldError>),
    /// Self-service reset is switched off for this deployment.
    Disabled,
}

/// Start a reset for whoever owns this address, if anybody does.
///
/// Returns [`ResetRequest::Accepted`] whether or not the address is known. The
/// caller must not branch on anything it can observe here, because there is
/// nothing here to branch on.
pub async fn request(
    pool: &PgPool,
    ctx: &Resetting<'_>,
    email: &str,
    client_ip: Option<&str>,
) -> ServiceResult<ResetRequest> {
    let settings = &ctx.config.security.password_reset;

    if !settings.enabled {
        return Ok(ResetRequest::Disabled);
    }

    let email = email.trim().to_lowercase();

    // Everything from here down is silent. Each `return Accepted` below is a
    // case where nothing was sent, and the caller cannot tell which.
    let Some(account) = user_store::find_by_email(pool, &email).await? else {
        return Ok(ResetRequest::Accepted);
    };

    if !can_reset(&account) {
        // Suspended, deactivated, or invited and never accepted. The first two
        // must not get a way back in, and the third already has an invitation
        // that does the same job - issuing a reset code for an account with no
        // password would turn a pending invitation into a second, weaker route
        // to the same account.
        tracing::info!(
            user_id = %account.id,
            status = account.status.as_str(),
            "password reset not offered for this account",
        );
        return Ok(ResetRequest::Accepted);
    }

    let issued = code::generate(account.id);

    one_time_token::issue(
        pool,
        account.id,
        TokenPurpose::PasswordReset,
        &issued.digest,
        settings.ttl_secs(),
        client_ip,
    )
    .await?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::PasswordResetRequested, true)
            .user(account.id)
            .email(&account.email),
    )
    .await;

    let message = mail::message::password_reset_code(
        &account.email,
        &account.display_name,
        ctx.workspace_name,
        issued.secret.expose_secret(),
        settings.code_ttl_mins as i64,
    );

    send_in_background(pool, ctx, message).await;

    Ok(ResetRequest::Accepted)
}

/// Finish a reset: check the code, then set the password.
///
/// `new_password` is judged against the *workspace's* policy, not the system
/// default - the person is about to be bound by it, and an account whose
/// password was set through this route must meet what every other account
/// meets.
pub async fn redeem(
    pool: &PgPool,
    security: &Security<'_>,
    config: &AppConfig,
    email: &str,
    presented_code: &str,
    new_password: &SecretString,
) -> ServiceResult<ResetOutcome> {
    let settings = &config.security.password_reset;

    if !settings.enabled {
        return Ok(ResetOutcome::Disabled);
    }

    let normalised = code::normalise(presented_code);

    // Shape first, so a scanner or a mangled paste costs neither a query nor
    // one of the five attempts.
    if !code::looks_like_a_code(&normalised) {
        return Ok(ResetOutcome::NotUsable);
    }

    let email = email.trim().to_lowercase();

    let Some(account) = user_store::find_by_email(pool, &email).await? else {
        return Ok(ResetOutcome::NotUsable);
    };

    if !can_reset(&account) {
        return Ok(ResetOutcome::NotUsable);
    }

    // Spends an attempt whether or not the code is right, and burns the row on
    // the attempt that reaches the limit. This is the only thing making six
    // digits safe.
    let redeemed = one_time_token::redeem_code(
        pool,
        account.id,
        TokenPurpose::PasswordReset,
        &code::digest_for(account.id, &normalised),
        settings.max_attempts,
    )
    .await?;

    let Some(user_id) = redeemed else {
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::PasswordResetCompleted, false)
                .user(account.id)
                .email(&account.email)
                .reason("code not usable"),
        )
        .await;

        return Ok(ResetOutcome::NotUsable);
    };

    // `Caller::system`, because the person doing this has no session - that is
    // the entire situation. The code is what stood in for authentication, and
    // it has just been spent.
    //
    // `force_change` is false: unlike an administrator setting a password on
    // somebody's behalf, the person choosing it here is the account's owner and
    // has just proved they read its mailbox. Making them change it again at the
    // next sign-in would be asking them to do this twice.
    let changed = super::password::set_password_administratively(
        pool,
        security,
        &Caller::system("password reset by emailed code"),
        user_id,
        new_password,
        false,
    )
    .await?;

    match changed {
        super::password::PasswordChange::Changed => {
            // Any other outstanding reset dies with it. Two codes in flight is
            // ordinary - somebody presses the button twice - and the one that
            // was not used must not stay usable after the password it would
            // have changed has already changed.
            one_time_token::revoke_all(pool, user_id, TokenPurpose::PasswordReset).await?;

            tracing::info!(%user_id, "password reset completed");
            Ok(ResetOutcome::Reset)
        }
        super::password::PasswordChange::Rejected(problems) => Ok(ResetOutcome::Rejected(problems)),
    }
}

/// Whether this account may be recovered by email at all.
///
/// `Active` and nothing else. A suspended or deactivated account being able to
/// mail itself a way back in would make the suspension advisory, and a
/// `Pending` one already has an invitation doing exactly this job.
fn can_reset(account: &UserRecord) -> bool {
    account.status == UserStatus::Active && account.password_hash.is_some()
}

/// Hand the message to a relay without the caller waiting for it.
///
/// **This is a security control, not a latency optimisation.** An SMTP
/// conversation takes between a hundred milliseconds and several seconds. If
/// [`request`] waited for it, the response would be reliably slower for an
/// address that has an account than for one that does not, and a handful of
/// requests with a stopwatch would recover exactly the membership list the
/// uniform answer is there to protect.
///
/// The relay is resolved *before* spawning, because resolving it reads the
/// workspace's mail settings out of this pool - and the pool handle is the one
/// thing the spawned task should not have to keep alive. Resolution is a cached
/// settings read, so it costs the same for both paths.
///
/// A failure has nowhere to be reported: the person who asked is looking at a
/// screen that deliberately does not know whether an account exists. It goes to
/// the log, which is where somebody investigating "I never got the email" will
/// be looking anyway.
async fn send_in_background(pool: &PgPool, ctx: &Resetting<'_>, message: mail::Mail) {
    let relay = match mail::resolve(pool, &ctx.config.smtp, Some(ctx.vault)).await {
        Ok(Some(relay)) => relay,
        Ok(None) => {
            // Not an error and not a surprise: SMTP is off by default, and a
            // deployment with no relay simply cannot offer this. Logged at warn
            // because a *user* asked for something that silently did not
            // happen, which is worth seeing.
            tracing::warn!(
                "a password reset code was issued but no mail relay is configured, \
                 so nothing was sent"
            );
            return;
        }
        Err(err) => {
            tracing::warn!(error = %err, "could not resolve a relay for a password reset");
            return;
        }
    };

    tokio::spawn(async move {
        let dispatch = mail::send(&relay, message).await;

        if let mail::Dispatch::Failed(reason) = dispatch {
            tracing::warn!(%reason, "a password reset code could not be delivered");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record with the two fields [`can_reset`] reads set as the test wants
    /// them, and every other field at its least interesting value.
    fn account(status: UserStatus, has_password: bool) -> UserRecord {
        UserRecord {
            id: uuid::Uuid::from_u128(1),
            email: "someone@example.com".to_owned(),
            first_name: "Some".to_owned(),
            last_name: "One".to_owned(),
            display_name: "Some One".to_owned(),
            password_hash: has_password.then(|| "argon2-hash".to_owned()),
            password_updated_at: None,
            must_change_password: false,
            status,
            is_owner: false,
            email_verified_at: None,
            mfa_enabled: false,
            mfa_required: false,
            failed_login_count: 0,
            locked_until: None,
            last_login_at: None,
            last_seen_at: None,
            locale: "en".to_owned(),
            timezone: "UTC".to_owned(),
            avatar_url: None,
            created_at: chrono::Utc::now(),
            deleted_at: None,
        }
    }

    #[test]
    fn only_an_active_account_with_a_password_can_be_reset() {
        assert!(can_reset(&account(UserStatus::Active, true)));

        // Suspended and deactivated: a reset would be a way back into an
        // account somebody deliberately closed.
        assert!(!can_reset(&account(UserStatus::Suspended, true)));
        assert!(!can_reset(&account(UserStatus::Deactivated, true)));

        // Invited and never accepted. The invitation already does this job, and
        // a second route to the same account is a second thing to get right.
        assert!(!can_reset(&account(UserStatus::Pending, false)));
        assert!(!can_reset(&account(UserStatus::Active, false)));
    }
}
