//! Signing in to Phonix Desk.
//!
//! Two steps, always. A password opens a session that can reach exactly one
//! page - the code box - and a TOTP code turns that session into a sign-in.
//! There is no parameter anywhere here for skipping the second step, because a
//! parameter is how one gets skipped.
//!
//! # What a refusal says
//!
//! Nothing. An unknown address, a wrong password, a locked account, a disabled
//! one and an account that never finished setup all produce the same
//! [`SignInOutcome::Rejected`], and the unknown-address path still spends the
//! time a real Argon2 verification costs. The audit row says which it was; the
//! response does not.
//!
//! # The one thing that is not constant-time, and why that is fine
//!
//! A locked account is refused before the hash is computed, so a lockout is
//! observable by timing. That is deliberate: burning 19 MiB and two iterations
//! per guess is exactly what a lockout exists to stop, and the fact being
//! leaked - "this address is currently locked" - is one a persistent attacker
//! learns anyway by watching it stop responding to correct passwords.

use chrono::{Duration, Utc};
use phonix_config::DeskConfig;
use phonix_db::desk::audit::{DeskAction, DeskAuditEntry, Outcome};
use phonix_db::desk::session::ClientFacts;
use phonix_db::desk::{DeskSessionRecord, DeskUserRecord, audit, session, user};
use phonix_db::sqlx::PgPool;
use secrecy::{ExposeSecret, SecretString};

use crate::Security;
use crate::crypto::{token, totp};
use crate::error::{ServiceError, ServiceResult};

/// The context every desk secret is sealed under.
///
/// Bound into the ciphertext, so a `totp_secret` lifted out of one row and
/// pasted into another fails to open rather than authenticating the wrong
/// person. Distinct from the tenant MFA context for the same reason.
pub const TOTP_CONTEXT: &[u8] = b"desk.totp";

/// What a password attempt produced.
#[derive(Debug)]
pub enum SignInOutcome {
    /// The password was right. The session exists but can only reach the code
    /// box until [`answer_challenge`] succeeds.
    CodeRequired {
        /// The session token, to be set as a cookie. Held once, never stored.
        token: SecretString,
        /// Who it belongs to, for the greeting on the challenge page.
        display_name: String,
    },
    /// Refused. One variant on purpose - see the module docs.
    Rejected,
}

/// What a code attempt produced.
#[derive(Debug, PartialEq, Eq)]
pub enum ChallengeOutcome {
    /// Signed in.
    Accepted,
    /// Wrong code, and there are attempts left.
    Rejected,
    /// Too many wrong codes. The session is gone and the password has to be
    /// entered again.
    Abandoned,
}

/// A desk user with a live session behind them.
///
/// Deliberately *not* called a caller-anything: it has no workspace and no
/// permissions, and nothing in this crate accepts it where a
/// [`crate::caller::Caller`] is expected.
#[derive(Debug, Clone)]
pub struct DeskCaller {
    pub user: DeskUserRecord,
    pub session: DeskSessionRecord,
}

impl DeskCaller {
    /// Whether this session has cleared the second factor.
    ///
    /// A `false` here is not "signed in with a caveat": it is a session that
    /// may reach the code box and nothing else.
    pub fn is_signed_in(&self) -> bool {
        self.session.mfa_satisfied
    }

    pub fn id(&self) -> uuid::Uuid {
        self.user.id
    }

    pub fn email(&self) -> &str {
        &self.user.email
    }
}

/// Step one: an address and a password.
///
/// `catalog` is the catalog pool. Desk never opens a tenant connection to sign
/// somebody in, because a desk user does not live in a tenant.
pub async fn sign_in(
    catalog: &PgPool,
    security: &Security<'_>,
    desk: &DeskConfig,
    email: &str,
    password: &SecretString,
    facts: ClientFacts<'_>,
) -> ServiceResult<SignInOutcome> {
    let email = email.trim().to_ascii_lowercase();

    let Some(account) = user::find_by_email(catalog, &email).await? else {
        // Spend what a real verification would, so an address that exists is
        // not visible in the response time.
        security.hasher.verify_dummy(password).await;
        record(
            catalog,
            DeskAuditEntry::new(DeskAction::SignIn, Outcome::Refused)
                .actor(None, Some(&email))
                .detail("no desk account with that address")
                .from_client(facts.ip),
        )
        .await;
        return Ok(SignInOutcome::Rejected);
    };

    // Before the password check: a locked account must not be usable to test
    // passwords, however slowly.
    if account.is_locked() {
        record(
            catalog,
            DeskAuditEntry::new(DeskAction::SignIn, Outcome::Refused)
                .actor(Some(account.id), Some(&account.email))
                .detail("account is locked")
                .from_client(facts.ip),
        )
        .await;
        return Ok(SignInOutcome::Rejected);
    }

    if !account.status.may_sign_in() || !account.is_complete() {
        security.hasher.verify_dummy(password).await;
        record(
            catalog,
            DeskAuditEntry::new(DeskAction::SignIn, Outcome::Refused)
                .actor(Some(account.id), Some(&account.email))
                .detail("account is not active")
                .from_client(facts.ip),
        )
        .await;
        return Ok(SignInOutcome::Rejected);
    }

    let stored = account
        .password_hash
        .as_deref()
        .ok_or_else(|| ServiceError::Crypto("an active desk account has no password".to_owned()))?;

    let matched = security
        .hasher
        .verify(password, stored)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    if !matched {
        let lockout = &security.config.lockout;
        let attempts = user::record_failed_attempt(
            catalog,
            account.id,
            lockout.max_failed_attempts,
            Duration::minutes(lockout.lockout_mins as i64),
        )
        .await?;

        record(
            catalog,
            DeskAuditEntry::new(DeskAction::SignIn, Outcome::Refused)
                .actor(Some(account.id), Some(&account.email))
                .detail("wrong password")
                .from_to(
                    serde_json::json!({ "failed_attempts": attempts - 1 }),
                    serde_json::json!({ "failed_attempts": attempts }),
                )
                .from_client(facts.ip),
        )
        .await;

        return Ok(SignInOutcome::Rejected);
    }

    // The password is right and the code is not in yet, so this session is
    // half of one. `session::create` cannot be asked for any other kind.
    let issued = token::IssuedToken::generate();
    session::create(
        catalog,
        account.id,
        &issued.digest,
        desk.session_idle_minutes as i64,
        desk.session_absolute_hours as i64,
        facts.clone(),
    )
    .await?;

    record(
        catalog,
        DeskAuditEntry::new(DeskAction::SignIn, Outcome::Ok)
            .actor(Some(account.id), Some(&account.email))
            .detail("password accepted, code outstanding")
            .from_client(facts.ip),
    )
    .await;

    Ok(SignInOutcome::CodeRequired {
        token: issued.secret,
        display_name: account.display_name,
    })
}

/// Step two: the six digits.
///
/// The attempt counter lives on the *session* rather than the account. A
/// challenge is short-lived, and the right answer to guessing at one is to end
/// that attempt - not to lock out a person whose password was correct, which
/// would let anybody who knows an address and a password lock its owner out by
/// typing digits badly.
pub async fn answer_challenge(
    catalog: &PgPool,
    security: &Security<'_>,
    desk: &DeskConfig,
    token: &SecretString,
    code: &str,
    facts: ClientFacts<'_>,
) -> ServiceResult<ChallengeOutcome> {
    let Some(caller) = authenticate(catalog, desk, token).await? else {
        return Ok(ChallengeOutcome::Abandoned);
    };

    if caller.is_signed_in() {
        // Already through. Answering twice is not a failure and must not cost
        // an attempt.
        return Ok(ChallengeOutcome::Accepted);
    }

    let sealed =
        caller.user.totp_secret.as_deref().ok_or_else(|| {
            ServiceError::Crypto("a desk account has no authenticator".to_owned())
        })?;

    let secret = security.vault.open(sealed, TOTP_CONTEXT)?;
    let params = totp::TotpParams::from_config(&security.config.mfa);
    let now = Utc::now().timestamp().max(0) as u64;

    if totp::verify(secret.expose_secret(), code, now, params).is_some() {
        session::mark_mfa_satisfied(catalog, caller.session.id).await?;
        user::record_sign_in(catalog, caller.user.id).await?;

        record(
            catalog,
            DeskAuditEntry::new(DeskAction::MfaChallenge, Outcome::Ok)
                .actor(Some(caller.user.id), Some(&caller.user.email))
                .from_client(facts.ip),
        )
        .await;

        return Ok(ChallengeOutcome::Accepted);
    }

    let attempts = session::record_mfa_attempt(catalog, caller.session.id).await?;
    let ceiling = security.config.mfa.max_challenge_attempts as i32;

    if attempts >= ceiling {
        session::revoke_by_id(catalog, caller.session.id, "too many wrong codes").await?;

        record(
            catalog,
            DeskAuditEntry::new(DeskAction::MfaChallenge, Outcome::Refused)
                .actor(Some(caller.user.id), Some(&caller.user.email))
                .detail("too many wrong codes; session ended")
                .from_client(facts.ip),
        )
        .await;

        return Ok(ChallengeOutcome::Abandoned);
    }

    record(
        catalog,
        DeskAuditEntry::new(DeskAction::MfaChallenge, Outcome::Refused)
            .actor(Some(caller.user.id), Some(&caller.user.email))
            .detail("wrong code")
            .from_client(facts.ip),
    )
    .await;

    Ok(ChallengeOutcome::Rejected)
}

/// Resolve a cookie into whoever holds it, sliding the idle deadline forward.
///
/// Returns half-authenticated sessions too - the caller asks
/// [`DeskCaller::is_signed_in`] - because the challenge page needs to find the
/// session it is challenging. Everything else in Desk must check.
pub async fn authenticate(
    catalog: &PgPool,
    desk: &DeskConfig,
    token: &SecretString,
) -> ServiceResult<Option<DeskCaller>> {
    if !token::looks_like_a_token(token.expose_secret()) {
        // Nothing that cannot be one of ours reaches the database. A cookie is
        // whatever a client felt like sending.
        return Ok(None);
    }

    let digest = token::digest_of_secret(token);

    let Some(session) = session::touch(catalog, &digest, desk.session_idle_minutes as i64).await?
    else {
        return Ok(None);
    };

    let Some(user) = user::find(catalog, session.desk_user_id).await? else {
        return Ok(None);
    };

    // Checked on every request rather than only at sign-in: disabling an
    // account has to end what it is already doing, not only stop the next
    // sign-in.
    if !user.status.may_sign_in() {
        session::revoke_by_id(catalog, session.id, "account is no longer active").await?;
        return Ok(None);
    }

    Ok(Some(DeskCaller { user, session }))
}

/// End a session. Idempotent - signing out twice is not an error.
pub async fn sign_out(
    catalog: &PgPool,
    token: &SecretString,
    actor: Option<&DeskCaller>,
    facts: ClientFacts<'_>,
) -> ServiceResult<()> {
    let digest = token::digest_of_secret(token);
    session::revoke(catalog, &digest, "signed out").await?;

    if let Some(actor) = actor {
        record(
            catalog,
            DeskAuditEntry::new(DeskAction::SignOut, Outcome::Ok)
                .actor(Some(actor.user.id), Some(&actor.user.email))
                .from_client(facts.ip),
        )
        .await;
    }

    Ok(())
}

/// Write an audit row, or say loudly that it could not be written.
///
/// The tenant audit helper swallows its error so a failed trail cannot fail the
/// business action. That trade is wrong here for anything an operator does -
/// see `phonix_db::desk::audit::record` - but it is right for these: refusing a
/// sign-in because the audit table is unwritable would lock everybody out of
/// the tool they would use to find out why. So the row is best-effort and its
/// absence is an ERROR in the log, which the health panel reads.
async fn record(catalog: &PgPool, entry: DeskAuditEntry<'_>) {
    let action = entry.action.as_str();
    if let Err(err) = audit::record(catalog, entry).await {
        tracing::error!(error = %err, action, "could not write a desk audit row");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The context string is bound into every sealed secret, so changing it
    /// makes every existing authenticator undecryptable. Pinned so that is a
    /// deliberate act with a failing test in front of it.
    #[test]
    fn the_totp_context_is_pinned() {
        assert_eq!(TOTP_CONTEXT, b"desk.totp");
    }

    /// A refusal must not carry a reason a client could read. This asserts the
    /// shape rather than the wording: one variant, no fields.
    #[test]
    fn a_rejection_carries_nothing() {
        let rejected = SignInOutcome::Rejected;
        assert!(matches!(rejected, SignInOutcome::Rejected));
    }

    #[test]
    fn abandoning_is_distinct_from_rejecting() {
        assert_ne!(ChallengeOutcome::Rejected, ChallengeOutcome::Abandoned);
        assert_ne!(ChallengeOutcome::Accepted, ChallengeOutcome::Rejected);
    }
}
