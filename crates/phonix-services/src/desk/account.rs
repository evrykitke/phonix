//! Creating desk accounts, and the setup link that gives one a password.
//!
//! There is no signup here and no "reset this person's password". A desk
//! account is created by another desk user - or, for the very first one, by a
//! CLI subcommand run on the box - and arrives holding nothing but a single-use
//! link. Whoever will use the account follows that link, chooses a password
//! only they know, and enrols an authenticator in the same act.
//!
//! That is the same rule the tenant side already follows for invitations, and
//! it matters more here: the person creating the account can suspend every
//! workspace on the box, and "I set it up for you, the password is..." would
//! mean two people can act as one name in the audit trail.
//!
//! # The setup link is one act, not two
//!
//! The password and the authenticator are set in the same submit, and the
//! account only becomes `active` if both succeed. An account that got a
//! password on Tuesday and an authenticator on Thursday can sign in on
//! Wednesday with one factor, which is the whole thing Desk refuses to allow.

use chrono::{Duration, Utc};
use phonix_config::DeskConfig;
use phonix_core::identity::password::validate_password;
use phonix_core::identity::validation::{FieldError, validate_email, validate_person_name};
use phonix_core::msg;
use phonix_db::desk::audit::{DeskAction, DeskAuditEntry, Outcome};
use phonix_db::desk::session::ClientFacts;
use phonix_db::desk::{DeskUserRecord, NewDeskUser, audit, session, user};
use phonix_db::sqlx::PgPool;
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use crate::Security;
use crate::crypto::{token, totp};
use crate::desk::auth::{DeskCaller, TOTP_CONTEXT};
use crate::error::{ServiceError, ServiceResult};

/// A desk account that has just been created, and the link that finishes it.
pub struct CreatedDeskUser {
    pub user: DeskUserRecord,
    /// Handed over out of band, once. There is no way to read it back.
    pub setup_token: SecretString,
}

/// What the enrolment page needs to draw itself.
pub struct SetupPage {
    pub email: String,
    pub display_name: String,
    /// The secret in the form an authenticator app accepts by typing.
    pub secret_base32: String,
    /// The same secret as the `otpauth://` URI behind a QR code.
    pub provisioning_uri: String,
}

/// What finishing setup produced.
#[derive(Debug)]
pub enum SetupOutcome {
    /// The account is `active` and can sign in.
    Completed,
    /// The code did not match the secret this page issued.
    WrongCode,
    /// The password was refused by the policy.
    Invalid(Vec<FieldError>),
    /// The link is unknown, spent or out of date. One variant for all three:
    /// there is nothing useful to tell apart, and a link that says which it was
    /// is a link that confirms it once existed.
    LinkNotUsable,
}

/// Create a `pending` account and mint its setup link.
///
/// `actor` is `None` only for the bootstrap subcommand, which runs on the box
/// with no session behind it - and the audit row says so rather than leaving
/// the actor blank and unexplained.
pub async fn create(
    catalog: &PgPool,
    desk: &DeskConfig,
    email: &str,
    display_name: &str,
    actor: Option<&DeskCaller>,
    facts: ClientFacts<'_>,
) -> ServiceResult<CreatedDeskUser> {
    let mut problems = Vec::new();

    let email = match validate_email(email) {
        Ok(email) => email,
        Err(err) => {
            problems.push(err);
            String::new()
        }
    };
    let display_name = match validate_person_name("display_name", display_name) {
        Ok(name) => name,
        Err(err) => {
            problems.push(err);
            String::new()
        }
    };

    if !problems.is_empty() {
        return Err(ServiceError::Rejected(problems));
    }

    let issued = token::IssuedToken::generate();
    let expires_at = Utc::now() + Duration::hours(desk.setup_link_hours as i64);

    let created = user::insert(
        catalog,
        NewDeskUser {
            email: &email,
            display_name: &display_name,
            setup_token_hash: &issued.digest,
            setup_expires_at: expires_at,
        },
    )
    .await?;

    // Not best-effort: this is somebody gaining the power to stop every
    // workspace on the box, and a row nobody can attribute is worse than a
    // creation that failed and has to be repeated.
    audit::record(
        catalog,
        DeskAuditEntry::new(DeskAction::DeskUserCreated, Outcome::Ok)
            .actor(actor.map(DeskCaller::id), actor.map(DeskCaller::email))
            .detail(if actor.is_some() {
                "created from Desk"
            } else {
                "created by the bootstrap subcommand on the box"
            })
            .from_to(
                serde_json::Value::Null,
                serde_json::json!({
                    "email": created.email,
                    "display_name": created.display_name,
                    "status": created.status.as_str(),
                }),
            )
            .from_client(facts.ip),
    )
    .await?;

    Ok(CreatedDeskUser {
        user: created,
        setup_token: issued.secret,
    })
}

/// Draw the enrolment page, issuing a fresh authenticator secret.
///
/// The secret is written to the row **unconfirmed** rather than carried through
/// the form: a hidden field would let a client choose the secret its own
/// account is checked against, and there is no reason to accept that. Drawing
/// the page again issues a new one and discards the last, which is what
/// somebody who scanned it wrong needs.
pub async fn begin_setup(
    catalog: &PgPool,
    security: &Security<'_>,
    token: &SecretString,
) -> ServiceResult<Option<SetupPage>> {
    if !token::looks_like_a_token(token.expose_secret()) {
        return Ok(None);
    }

    let digest = token::digest_of_secret(token);
    let Some(account) = user::find_by_setup_token(catalog, &digest).await? else {
        return Ok(None);
    };

    let params = totp::TotpParams::from_config(&security.config.mfa);
    let secret = totp::generate_secret(security.config.mfa.secret_bytes);
    let sealed = security.vault.seal(&secret, TOTP_CONTEXT)?;

    if !user::stage_totp_secret(catalog, &digest, &sealed).await? {
        // The link went stale between the two statements. Rare, and the honest
        // answer is the same as never having found it.
        return Ok(None);
    }

    Ok(Some(SetupPage {
        secret_base32: totp::encode_secret(&secret),
        provisioning_uri: totp::provisioning_uri(
            &security.config.mfa.issuer,
            &account.email,
            &secret,
            params,
        ),
        email: account.email,
        display_name: account.display_name,
    }))
}

/// Finish setup: a password and a code, together, or neither.
pub async fn complete_setup(
    catalog: &PgPool,
    security: &Security<'_>,
    token: &SecretString,
    password: &SecretString,
    code: &str,
    facts: ClientFacts<'_>,
) -> ServiceResult<SetupOutcome> {
    if !token::looks_like_a_token(token.expose_secret()) {
        return Ok(SetupOutcome::LinkNotUsable);
    }

    let digest = token::digest_of_secret(token);
    let Some(account) = user::find_by_setup_token(catalog, &digest).await? else {
        return Ok(SetupOutcome::LinkNotUsable);
    };

    if let Err(problem) = validate_password(password.expose_secret()) {
        return Ok(SetupOutcome::Invalid(vec![problem]));
    }

    let Some(sealed) = account.totp_secret.as_deref() else {
        // The page was never drawn, so there is nothing to check the code
        // against. Reachable only by posting the form directly.
        return Ok(SetupOutcome::LinkNotUsable);
    };

    let secret = security.vault.open(sealed, TOTP_CONTEXT)?;
    let params = totp::TotpParams::from_config(&security.config.mfa);
    let now = Utc::now().timestamp().max(0) as u64;

    if totp::verify(secret.expose_secret(), code, now, params).is_none() {
        return Ok(SetupOutcome::WrongCode);
    }

    let hash = security
        .hasher
        .hash(password)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    if !user::complete_setup(catalog, &digest, &hash).await? {
        return Ok(SetupOutcome::LinkNotUsable);
    }

    // Nothing that was open before a password existed should stay open after
    // one does.
    session::revoke_all_for_user(catalog, account.id, "setup completed").await?;

    audit::record(
        catalog,
        DeskAuditEntry::new(DeskAction::DeskUserSetupCompleted, Outcome::Ok)
            .actor(Some(account.id), Some(&account.email))
            .from_to(
                serde_json::json!({ "status": "pending" }),
                serde_json::json!({ "status": "active" }),
            )
            .from_client(facts.ip),
    )
    .await?;

    Ok(SetupOutcome::Completed)
}

/// Everyone who may sign in to Desk, or once could.
pub async fn list(catalog: &PgPool) -> ServiceResult<Vec<DeskUserRecord>> {
    Ok(user::list(catalog).await?)
}

/// Disable an account, ending its sessions, or bring a disabled one back to
/// `pending` so it can go through setup again.
///
/// # The last account is refused
///
/// Disabling the only usable account leaves a Desk nobody can sign in to, which
/// is recovered by SSHing to the box and running the bootstrap subcommand -
/// possible, but not something anybody should discover by accident at the end
/// of a Friday. The refusal is here rather than in the adapter because it is
/// the rule, not the button.
pub async fn set_disabled(
    catalog: &PgPool,
    id: Uuid,
    disabled: bool,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
) -> ServiceResult<()> {
    let Some(account) = user::find(catalog, id).await? else {
        return Err(ServiceError::NotFound("desk account"));
    };

    if disabled && account.status.may_sign_in() && user::active_count(catalog).await? <= 1 {
        // A `FieldError` carries a `Message`, which is a key `i18n/en.json`
        // has to define - the same rule every other sentence this crate returns
        // follows, and the reason Desk does not get to invent English here.
        return Err(ServiceError::rejected(
            "status",
            msg!("desk.account.last_active"),
        ));
    }

    let before = account.status.as_str().to_owned();
    user::set_disabled(catalog, id, disabled).await?;

    if disabled {
        session::revoke_all_for_user(catalog, id, "account disabled").await?;
    }

    let action = if disabled {
        DeskAction::DeskUserDisabled
    } else {
        DeskAction::DeskUserReinstated
    };

    audit::record(
        catalog,
        DeskAuditEntry::new(action, Outcome::Ok)
            .actor(Some(actor.id()), Some(actor.email()))
            .detail(&account.email)
            .from_to(
                serde_json::json!({ "status": before }),
                serde_json::json!({ "status": if disabled { "disabled" } else { "pending" } }),
            )
            .from_client(facts.ip),
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link that is unknown, spent or expired must be one answer. Three would
    /// mean a probe could tell "never existed" from "already used", which says
    /// somebody was invited.
    #[test]
    fn an_unusable_link_has_exactly_one_outcome() {
        assert!(matches!(
            SetupOutcome::LinkNotUsable,
            SetupOutcome::LinkNotUsable
        ));
    }

    /// A wrong code and a refused password are different answers on purpose:
    /// both are the person's own mistake on a page they are already holding a
    /// valid link for, and telling them which one to fix is the entire job of
    /// that screen.
    #[test]
    fn a_wrong_code_is_not_a_refused_password() {
        let wrong = SetupOutcome::WrongCode;
        assert!(!matches!(wrong, SetupOutcome::Invalid(_)));
    }
}
