//! Changing a password, under the workspace's own policy.
//!
//! This is where an organization's [`PasswordPolicy`] actually bites. Sign-up
//! cannot use it - there is no organization yet at that point, so the sign-up
//! form applies the system default - but every change afterwards goes through
//! here and through whatever the administrators have configured.
//!
//! # The reuse check, and why it is bounded
//!
//! `password_history_depth` is capped at 24 because each remembered password
//! costs one Argon2 verification on every change, at ~50 ms each. A workspace
//! that set it to 200 would make changing a password a ten-second operation and
//! would be storing 200 hashes of passwords the user may still be using
//! somewhere else.

use phonix_core::identity::password::PasswordPolicy;
use phonix_core::identity::{FieldError, UserId};
use phonix_core::permissions;
use phonix_db::identity::password_history;
use phonix_db::identity::user::UserRecord;
use phonix_db::identity::{AuditEntry, IdentityEvent, audit, session, user};
use phonix_db::settings as settings_store;
use phonix_db::sqlx::PgPool;
use secrecy::SecretString;

use crate::Security;
use crate::caller::Caller;
use crate::error::{ServiceError, ServiceResult};
use phonix_core::msg;

/// What a change attempt produced.
///
/// A wrong current password is `Rejected`, not an error: it is the expected
/// path through a form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordChange {
    Changed,
    /// Per-field problems to show on the form.
    Rejected(Vec<FieldError>),
}

impl PasswordChange {
    fn rejected(field: &str, message: phonix_core::Message) -> Self {
        Self::Rejected(vec![FieldError::new(field, message)])
    }
}

/// Change a user's own password.
///
/// The current password is required even though the caller is already signed
/// in: a session is a bearer credential, and somebody who found an unlocked
/// laptop should not be able to take the account with it.
pub async fn change_own_password(
    pool: &PgPool,
    security: &Security<'_>,
    caller: &Caller,
    current: &SecretString,
    new_password: &SecretString,
    keep_session: Option<&SecretString>,
) -> ServiceResult<PasswordChange> {
    use secrecy::ExposeSecret;

    // No permission required to change your own password, but you do have to be
    // somebody: the system caller has no password to change.
    let user_id = crate::caller::acting_user(caller)?;

    let Some(account) = user::find_by_id(pool, user_id).await? else {
        return Err(ServiceError::Unauthenticated);
    };

    let Some(stored_hash) = account.password_hash.clone() else {
        return Err(ServiceError::rejected(
            "current_password",
            msg!("error.password.none_set"),
        ));
    };

    let matched = security
        .hasher
        .verify(current, &stored_hash)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    if !matched {
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::PasswordChange, false)
                .user(user_id)
                .reason("wrong current password"),
        )
        .await;

        return Ok(PasswordChange::rejected(
            "current_password",
            msg!("error.password.wrong_current"),
        ));
    }

    let policy = settings_store::load(pool).await?.password;

    if let Some(problem) = check_new_password(&policy, &account, new_password.expose_secret()) {
        return Ok(PasswordChange::Rejected(vec![problem]));
    }

    if let Some(problem) = check_not_reused(pool, security, &policy, &account, new_password).await?
    {
        return Ok(PasswordChange::Rejected(vec![problem]));
    }

    let hash = security
        .hasher
        .hash(new_password)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    apply(pool, &policy, user_id, &hash, &stored_hash).await?;

    // Every other session goes. A password change is what somebody does after
    // "I think someone has my password", and leaving the other sessions live
    // makes it useless for that.
    let revoked = match keep_session {
        Some(token) => {
            let digest = crate::crypto::token::digest_of_secret(token);
            session::revoke_all_for_user_except(pool, user_id, &digest, "password changed").await?
        }
        None => session::revoke_all_for_user(pool, user_id, "password changed").await?,
    };

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::PasswordChange, true)
            .user(user_id)
            .detail(serde_json::json!({ "sessions_revoked": revoked })),
    )
    .await;

    Ok(PasswordChange::Changed)
}

/// Set a password on behalf of a user, without their current one.
///
/// For an administrator resetting an account, and for a redeemed reset link.
/// The account is flagged `must_change_password` so the person whose account it
/// is picks their own on next sign-in - an administrator who knows a user's
/// working password can act as them, and this is what closes that.
pub async fn set_password_administratively(
    pool: &PgPool,
    security: &Security<'_>,
    caller: &Caller,
    user_id: UserId,
    new_password: &SecretString,
    force_change: bool,
) -> ServiceResult<PasswordChange> {
    use secrecy::ExposeSecret;

    // Setting somebody else's password is `Users.Edit`. A redeemed reset link
    // arrives as `Caller::system`, which is how a user with no session at all
    // reaches this without holding an administrator's permission.
    caller.require_self_or(user_id, permissions::USERS_EDIT)?;

    let Some(account) = user::find_by_id(pool, user_id).await? else {
        return Err(ServiceError::rejected("user", msg!("error.user.not_found")));
    };

    let policy = settings_store::load(pool).await?.password;

    if let Some(problem) = check_new_password(&policy, &account, new_password.expose_secret()) {
        return Ok(PasswordChange::Rejected(vec![problem]));
    }

    let hash = security
        .hasher
        .hash(new_password)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    let previous = account.password_hash.clone().unwrap_or_default();
    apply(pool, &policy, user_id, &hash, &previous).await?;

    if force_change {
        user::set_must_change_password(pool, user_id, true).await?;
    }

    let revoked = session::revoke_all_for_user(pool, user_id, "password reset").await?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::PasswordResetCompleted, true)
            .user(user_id)
            .detail(serde_json::json!({
                "sessions_revoked": revoked,
                "must_change": force_change,
            })),
    )
    .await;

    Ok(PasswordChange::Changed)
}

/// Everything about the new password that can be judged without hashing.
///
/// Shared with [`super::invitation`], which sets the first password rather than
/// changing one: an invited account has to meet the same policy as everybody
/// else, and a second copy of this would be the place the two drift apart.
pub(crate) fn check_new_password(
    policy: &PasswordPolicy,
    account: &UserRecord,
    candidate: &str,
) -> Option<FieldError> {
    if let Err(problem) = policy.check(candidate) {
        return Some(problem);
    }

    if policy.forbid_personal_information && echoes_identity(account, candidate) {
        return Some(FieldError::new(
            "password",
            msg!("validation.password.contains_identity"),
        ));
    }

    None
}

/// Whether the new password is one of the last few.
///
/// Runs only when the workspace asked for it: with `history_depth = 0` this is
/// not a query and not a hash.
async fn check_not_reused(
    pool: &PgPool,
    security: &Security<'_>,
    policy: &PasswordPolicy,
    account: &UserRecord,
    candidate: &SecretString,
) -> ServiceResult<Option<FieldError>> {
    if policy.history_depth == 0 {
        return Ok(None);
    }

    // The current password counts as the most recent one, whether or not it
    // reached the history table yet.
    let mut hashes = Vec::new();
    if let Some(current) = account.password_hash.clone() {
        hashes.push(current);
    }
    hashes
        .extend(password_history::recent(pool, account.id, i64::from(policy.history_depth)).await?);

    for hash in hashes.iter().take(usize::from(policy.history_depth)) {
        let matched = security
            .hasher
            .verify(candidate, hash)
            .await
            .map_err(|err| ServiceError::Crypto(err.to_string()))?;

        if matched {
            return Ok(Some(FieldError::new(
                "password",
                msg!("validation.password.reused", depth = policy.history_depth),
            )));
        }
    }

    Ok(None)
}

/// Write the new hash, and remember the old one if the policy says to.
async fn apply(
    pool: &PgPool,
    policy: &PasswordPolicy,
    user_id: UserId,
    new_hash: &str,
    previous_hash: &str,
) -> ServiceResult<()> {
    user::set_password_hash(pool, user_id, new_hash).await?;

    if policy.history_depth > 0 && !previous_hash.is_empty() {
        password_history::remember(
            pool,
            user_id,
            previous_hash,
            i32::from(policy.history_depth),
        )
        .await?;
    } else {
        // A workspace that turned history off keeps nothing: the rows are
        // hashes of passwords the user may still be using elsewhere, and there
        // is no reason to hold them once nothing checks them.
        password_history::forget_all(pool, user_id).await?;
    }

    Ok(())
}

/// Whether the password is built from the user's own details.
fn echoes_identity(account: &UserRecord, candidate: &str) -> bool {
    let lowered = candidate.to_lowercase();
    let local_part = account.email.split('@').next().unwrap_or_default();

    [
        account.first_name.as_str(),
        account.last_name.as_str(),
        local_part,
    ]
    .iter()
    .filter(|part| part.chars().count() >= 3)
    .any(|part| lowered.contains(&part.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use phonix_core::identity::UserStatus;

    fn account() -> UserRecord {
        UserRecord {
            id: uuid::Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            password_hash: None,
            status: UserStatus::Active,
            is_owner: true,
            must_change_password: false,
            email_verified_at: None,
            mfa_enabled: false,
            mfa_required: false,
            failed_login_count: 0,
            locked_until: None,
            password_updated_at: Some(Utc::now()),
            last_login_at: None,
            last_seen_at: None,
            locale: "en".into(),
            timezone: "UTC".into(),
            avatar_url: None,
            created_at: Utc::now(),
            deleted_at: None,
        }
    }

    #[test]
    fn a_password_built_from_the_users_own_name_is_refused() {
        let account = account();

        assert!(echoes_identity(&account, "AdaAdaAda1234"));
        assert!(echoes_identity(&account, "lovelace-forever"));
        assert!(
            echoes_identity(&account, "xxadaxxxxxxxx"),
            "case-insensitive"
        );

        // The email *domain* is not the user's secret and is shared by everyone
        // in the workspace - treating it as forbidden would refuse half of all
        // reasonable passwords.
        assert!(!echoes_identity(&account, "example.com is fine here"));
        assert!(!echoes_identity(&account, "correct horse battery staple"));
    }

    #[test]
    fn the_workspaces_policy_is_what_gets_applied() {
        let account = account();
        let strict = PasswordPolicy {
            min_length: 20,
            require_symbol: true,
            ..PasswordPolicy::system_default()
        };

        // Long enough for the system default, refused by this workspace.
        assert!(
            PasswordPolicy::system_default()
                .check("correct horse")
                .is_ok()
        );
        assert!(check_new_password(&strict, &account, "correct horse battery st").is_some());
        assert!(check_new_password(&strict, &account, "correct horse battery st!").is_none());
    }

    #[test]
    fn a_rejection_names_the_field_the_form_shows_it_under() {
        match PasswordChange::rejected("current_password", msg!("error.password.wrong_current")) {
            PasswordChange::Rejected(errors) => {
                assert_eq!(errors[0].field, "current_password");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }
}
