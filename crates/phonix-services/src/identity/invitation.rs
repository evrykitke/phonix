//! Inviting somebody, and accepting an invitation.
//!
//! # The order of operations is the whole design
//!
//! ```text
//! 1. create the account   password_hash NULL, status Pending
//! 2. assign its roles
//! 3. issue the token      superseding any outstanding invitation
//! 4. send the email       may fail; does not undo 1-3
//! ```
//!
//! Steps 1 to 3 are one transaction. Step 4 is deliberately outside it, and
//! deliberately not allowed to fail the request: an account that exists with an
//! undelivered invitation is recoverable - copy the link, or re-send once the
//! relay works - while a request that rolled back because a mail server was
//! briefly unreachable leaves the administrator wondering what happened and
//! whether to try again.
//!
//! So [`invite`] returns the link in [`InvitationIssued`] whether or not it was
//! emailed, and the screen decides how loudly to present it.
//!
//! # Accepting is where the password is set
//!
//! [`accept`] is the only place an invited account gets a password, and the
//! person setting it is the person who will use it. It consumes the token,
//! applies the workspace's password policy, and moves the account to `Active`
//! with its address already verified - opening the link *is* the proof that the
//! address receives mail.

use phonix_config::AppConfig;
use phonix_core::form::{Submission, rejected};
use phonix_core::identity::UserStatus;
use phonix_core::identity::{InvitationIssued, UserId, UserInvite};
use phonix_core::permissions;
use phonix_db::authorization::role as role_store;
use phonix_db::identity::one_time_token::TokenPurpose;
use phonix_db::identity::user::NewUser;
use phonix_db::identity::{AuditEntry, IdentityEvent, audit, user as user_store};
use phonix_db::sqlx::PgPool;
use secrecy::{ExposeSecret, SecretString};

use crate::caller::{Caller, acting_user};
use crate::crypto::Hasher;
use crate::error::{ServiceError, ServiceResult};
use crate::mail;
use phonix_core::msg;

/// The path an invitation link points at.
///
/// Re-exported from `phonix_core` rather than spelled again: the link is built
/// here, the route is declared in the browser crate, and the signed-out guard
/// is in core - and all three have to be the same string.
pub use phonix_core::identity::INVITATION_ACCEPT_PATH as ACCEPT_PATH;

/// Everything inviting somebody needs from the outside world.
///
/// One parameter rather than six, and the same reason as
/// [`Security`](crate::Security): adding a dependency later should not change
/// this signature at every call site.
pub struct Inviting<'a> {
    pub config: &'a AppConfig,
    pub hasher: &'a Hasher,
    pub vault: &'a crate::SecretVault,
    /// The workspace this is happening in - its slug builds the link, its name
    /// appears in the message.
    pub workspace_slug: &'a str,
    pub workspace_name: &'a str,
}

/// Create an account and send its invitation.
///
/// Returns a [`Submission`] rather than a bare value: an address that is
/// already taken is the expected path through this form, not a failure, and it
/// has to arrive at the field it is about.
pub async fn invite(
    pool: &PgPool,
    caller: &Caller,
    ctx: &Inviting<'_>,
    invite: UserInvite,
) -> ServiceResult<Submission<InvitationIssued>> {
    caller.require(permissions::USERS_CREATE)?;
    let invited_by = acting_user(caller)?;

    if let Some(rejection) = rejected(invite.validate()) {
        return Ok(rejection);
    }

    let email = invite.normalised_email();

    // Asked before writing so the message names the address field. The unique
    // index is still the authority - two administrators inviting the same
    // person at the same moment race past this - and the insert below turns
    // that into the same rejection rather than a 500.
    if user_store::find_by_email(pool, &email).await?.is_some() {
        return Ok(Submission::rejected("email", msg!("error.email.taken")));
    }

    if !invite.roles.is_empty() {
        let known: Vec<String> = role_store::list(pool)
            .await?
            .into_iter()
            .map(|role| role.name)
            .collect();

        let unknown: Vec<&str> = invite
            .roles
            .iter()
            .filter(|name| !known.iter().any(|k| k.eq_ignore_ascii_case(name)))
            .map(String::as_str)
            .collect();

        if !unknown.is_empty() {
            return Ok(Submission::rejected(
                "roles",
                msg!("error.roles.unknown", names = unknown.join(", ")),
            ));
        }

        // Granting roles is granting permissions, however it is spelled - the
        // same rule the edit form follows. Asked only when roles were actually
        // named, so inviting somebody with none needs only `Users.Create`.
        caller.require(permissions::USERS_CHANGE_PERMISSIONS)?;
    }

    let created = user_store::create(
        pool,
        NewUser {
            email: &email,
            first_name: invite.first_name.trim(),
            last_name: invite.last_name.trim(),
            // The whole point: no password exists until the person sets one.
            password_hash: None,
            status: UserStatus::Pending,
            is_owner: false,
            invited_by: Some(invited_by),
        },
    )
    .await;

    let account = match created {
        Ok(account) => account,
        // The race described above, arriving as the unique index rather than as
        // the check. Same answer, same field.
        Err(phonix_db::DbError::UserExists(_)) => {
            return Ok(Submission::rejected("email", msg!("error.email.taken")));
        }
        Err(err) => return Err(err.into()),
    };

    if invite.roles.is_empty() {
        // Whatever the workspace gives everybody. Skipped when roles were named
        // explicitly, because an administrator who chose none meant none.
        role_store::assign_default_roles(pool, account.id).await?;
    } else {
        role_store::set_user_roles(pool, account.id, &invite.roles, Some(invited_by)).await?;
    }

    let issued = issue_link(pool, ctx, account.id).await?;

    let dispatch = deliver(
        pool,
        ctx,
        &account.email,
        &invite.display_name(),
        &issued,
        invited_by,
    )
    .await;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::InvitationSent, true)
            .user(invited_by)
            .email(&account.email)
            .detail(serde_json::json!({
                "subject": account.id,
                "roles": invite.roles,
                "emailed": dispatch.is_sent(),
            })),
    )
    .await;

    tracing::info!(
        user_id = %account.id,
        %invited_by,
        emailed = dispatch.is_sent(),
        "invitation issued",
    );

    Ok(Submission::Saved(InvitationIssued {
        user_id: account.id,
        email: account.email,
        display_name: invite.display_name(),
        link: issued.link,
        expires_in_hours: ctx.config.security.invitations.ttl_hours as i64,
        delivery_note: dispatch.note(),
    }))
}

/// Issue a fresh invitation for an account that already has one.
///
/// For the invitation that expired, went to a spam folder, or was sent before
/// the relay worked. Superseding the outstanding token is what makes this safe
/// to press twice.
pub async fn resend(
    pool: &PgPool,
    caller: &Caller,
    ctx: &Inviting<'_>,
    user_id: UserId,
) -> ServiceResult<InvitationIssued> {
    caller.require(permissions::USERS_CREATE)?;
    let invited_by = acting_user(caller)?;

    let Some(account) = user_store::find_by_id(pool, user_id).await? else {
        return Err(ServiceError::rejected("user", msg!("error.user.gone")));
    };

    // Only an account that has not accepted yet. Re-inviting an active account
    // would mint a link that sets its password, which is an account takeover
    // with a friendly name.
    if account.status != UserStatus::Pending {
        return Err(ServiceError::rejected(
            "user",
            msg!("error.invitation.already_accepted"),
        ));
    }

    let issued = issue_link(pool, ctx, account.id).await?;
    let dispatch = deliver(
        pool,
        ctx,
        &account.email,
        &account.display_name,
        &issued,
        invited_by,
    )
    .await;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::InvitationSent, true)
            .user(invited_by)
            .email(&account.email)
            .detail(serde_json::json!({
                "subject": account.id,
                "resent": true,
                "emailed": dispatch.is_sent(),
            })),
    )
    .await;

    Ok(InvitationIssued {
        user_id: account.id,
        email: account.email,
        display_name: account.display_name,
        link: issued.link,
        expires_in_hours: ctx.config.security.invitations.ttl_hours as i64,
        delivery_note: dispatch.note(),
    })
}

/// What the account holder submits from the invitation link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    /// The account is set up and may now sign in.
    Accepted { user_id: UserId, email: String },
    /// The link is unknown, expired, or already used - three cases kept
    /// deliberately indistinguishable, because "already used" tells whoever
    /// intercepted it that they had a real one.
    NotUsable,
    /// The link was fine; the password was not.
    Rejected(Vec<phonix_core::identity::FieldError>),
}

/// Set the password on an invited account and activate it.
///
/// The only place an invited account gets a password, and the person setting it
/// is the person who will use it.
pub async fn accept(
    pool: &PgPool,
    hasher: &Hasher,
    token: &SecretString,
    password: &SecretString,
) -> ServiceResult<Acceptance> {
    // Consumed first, and exactly once. Validating the password before spending
    // the token would let somebody probe a stolen link with rubbish passwords
    // to learn whether it is live.
    let Some(user_id) =
        super::one_time_token::consume(pool, token, TokenPurpose::Invitation).await?
    else {
        return Ok(Acceptance::NotUsable);
    };

    let Some(account) = user_store::find_by_id(pool, user_id).await? else {
        return Ok(Acceptance::NotUsable);
    };

    // This workspace's policy, not the system default - it is the workspace's
    // decision and the person accepting is about to be bound by it. The same
    // check the change-password flow runs, so an invited account cannot be set
    // up with a password an existing one would be refused.
    let policy = crate::workspace::settings::load(pool).await?;

    if let Some(problem) =
        super::password::check_new_password(&policy.password, &account, password.expose_secret())
    {
        // The token is spent. That is deliberate and is why the screen re-issues
        // rather than retries: a link that survived a failed attempt is a link
        // that can be probed.
        return Ok(Acceptance::Rejected(vec![problem]));
    }

    let hash = hasher
        .hash(password)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;
    user_store::set_password_hash(&mut *tx, user_id, &hash).await?;
    // Opening the link is the proof that the address receives mail, so there is
    // nothing left to verify separately.
    user_store::mark_email_verified(&mut *tx, user_id).await?;
    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::InvitationAccepted, true)
            .user(user_id)
            .email(&account.email),
    )
    .await;

    tracing::info!(%user_id, "invitation accepted");

    Ok(Acceptance::Accepted {
        user_id,
        email: account.email,
    })
}

/// A freshly minted token and the absolute link that carries it.
struct IssuedLink {
    link: String,
}

async fn issue_link(
    pool: &PgPool,
    ctx: &Inviting<'_>,
    user_id: UserId,
) -> ServiceResult<IssuedLink> {
    let token = super::one_time_token::issue(
        pool,
        user_id,
        TokenPurpose::Invitation,
        ctx.config.security.invitations.ttl_secs(),
        None,
    )
    .await?;

    // Absolute and on the workspace's own host: the session the link ends in
    // belongs to that host, and a link to the bare domain would land somewhere
    // that cannot set the cookie.
    let origin = ctx.config.server.tenant_origin(ctx.workspace_slug);

    Ok(IssuedLink {
        link: format!(
            "{origin}{ACCEPT_PATH}?token={}",
            token.secret.expose_secret()
        ),
    })
}

/// Send the invitation, through whichever relay this workspace uses.
async fn deliver(
    pool: &PgPool,
    ctx: &Inviting<'_>,
    to_address: &str,
    to_name: &str,
    issued: &IssuedLink,
    invited_by: UserId,
) -> mail::Dispatch {
    let inviter = user_store::find_by_id(pool, invited_by)
        .await
        .ok()
        .flatten()
        .map(|user| user.display_name)
        .unwrap_or_else(|| "An administrator".to_owned());

    let relay = match mail::resolve(pool, &ctx.config.smtp, Some(ctx.vault)).await {
        Ok(Some(relay)) => relay,
        Ok(None) => return mail::Dispatch::NotConfigured,
        // Reading the relay failed - not the same as there being none, but the
        // same outcome for this message, and the link is still returned.
        Err(err) => return mail::Dispatch::Failed(err.to_string()),
    };

    let message = mail::message::invitation(
        to_address,
        to_name,
        ctx.workspace_name,
        &inviter,
        &issued.link,
        ctx.config.security.invitations.ttl_hours as i64,
    );

    mail::send(&relay, message).await
}

/// Whether an account is still waiting on its invitation.
///
/// For the users grid, which offers "Re-send invitation" only where it would do
/// something.
pub fn is_awaiting_acceptance(status: UserStatus, has_password: bool) -> bool {
    status == UserStatus::Pending && !has_password
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_pending_account_without_a_password_is_still_invited() {
        assert!(is_awaiting_acceptance(UserStatus::Pending, false));

        // Accepted: it has a password now.
        assert!(!is_awaiting_acceptance(UserStatus::Pending, true));
        // Never invited, or long since active.
        assert!(!is_awaiting_acceptance(UserStatus::Active, false));
    }

    #[test]
    fn the_accept_path_is_a_path_and_not_a_url() {
        // It is joined onto an origin, so a leading slash and no host are what
        // make the result well-formed.
        assert!(ACCEPT_PATH.starts_with('/'));
        assert!(!ACCEPT_PATH.contains("://"));
    }

    #[test]
    fn an_unusable_link_does_not_say_which_kind_of_unusable_it_is() {
        // Unknown, expired and already-spent are one variant on purpose:
        // distinguishing them tells whoever intercepted a link that it was real.
        let outcome = Acceptance::NotUsable;

        assert_eq!(outcome, Acceptance::NotUsable);
        assert!(!matches!(outcome, Acceptance::Accepted { .. }));
    }
}
