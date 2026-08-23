//! This workspace's own relay: reading it, changing it, and turning it into
//! something that can send.
//!
//! # The password goes in and does not come back
//!
//! [`load`] returns [`MailSettings`], which has no password field. [`save`]
//! takes a [`MailSettingsInput`], whose password is an `Option` where `None`
//! means "leave the stored one alone" - so the settings screen can change a
//! host without ever having been given the secret it is not changing.
//!
//! The only path that reads the password back is [`as_relay`], and it hands it
//! straight to the SMTP client. Nothing between here and the socket sees it as
//! a `String`.

use std::time::Duration;

use phonix_core::form::{Submission, rejected};
use phonix_core::mail::{MailSettings, MailSettingsInput};
use phonix_core::permissions;
use phonix_db::mail as store;
use phonix_db::sqlx::PgPool;
use secrecy::{ExposeSecret, SecretString};

use super::{Relay, mailbox};
use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::crypto::vault::{SecretVault, mail_context};
use crate::error::ServiceResult;

/// What this workspace has configured, if anything.
///
/// Gated on `Settings`, like the security policy: a relay's host and username
/// are not a secret from an administrator, and the password is not returned by
/// this or any other read.
pub async fn load(pool: &PgPool, caller: &Caller) -> ServiceResult<MailSettings> {
    caller.require(permissions::SETTINGS)?;
    Ok(store::load(pool).await?.to_settings())
}

/// Store a relay an administrator submitted.
///
/// Returns the settings as they now stand, so the screen re-renders from what
/// was written - including the normalisation, and including `has_password`
/// after a password was set or cleared.
///
/// A [`Submission`] rather than a bare value, for the reason the rest of the
/// codebase gives: a form that fails validation is the expected path through a
/// form, and modelling it as `Err` collapses the per-field detail into one
/// string on the way across the wire.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    vault: &SecretVault,
    input: MailSettingsInput,
) -> ServiceResult<Submission<MailSettings>> {
    caller.require(permissions::SETTINGS)?;
    let changed_by = acting_user(caller)?;

    let input = input.normalised();

    // Validated here rather than left to the CHECK constraint: a constraint can
    // only refuse the whole row, and it arrives as a constraint name that
    // nobody outside this codebase can read - where the form needs to know
    // *which* field is wrong.
    if let Some(rejection) = rejected(input.validate()) {
        return Ok(rejection);
    }

    let before = store::load(pool).await?;

    // Three states, and they are not the same:
    //   None      leave the stored password alone
    //   Some("")  remove it - some relays authenticate on the username alone
    //   Some(pw)  replace it
    let sealed = match input.password.as_deref().map(str::trim) {
        None => None,
        Some("") => {
            store::clear_password(pool).await?;
            None
        }
        Some(password) => Some(vault.seal(password.as_bytes(), &mail_context())?),
    };

    store::save(
        pool,
        store::MailUpdate {
            enabled: input.enabled,
            host: &input.host,
            port: input.port,
            username: &input.username,
            password_sealed: sealed.as_deref(),
            from_address: &input.from_address,
            from_name: &input.from_name,
            reply_to: input.reply_to.as_deref(),
            encryption: input.encryption,
            updated_by: Some(changed_by),
        },
    )
    .await?;

    let after = store::load(pool).await?.to_settings();

    // Neither side carries a password - `MailSettings` has no field for one -
    // so what is recorded is that a relay changed and to what, which is the
    // question an audit asks. Whether the *password* changed is visible as
    // `has_password` going false to true, and that is as much as should be
    // recorded.
    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::MAIL_SETTINGS).named(&after.host),
        &before.to_settings(),
        &after,
    )
    .await;

    tracing::info!(
        host = %after.host,
        enabled = after.enabled,
        %changed_by,
        "mail settings changed",
    );

    Ok(Submission::Saved(after))
}

/// This workspace's relay, ready to send, if it has a usable one.
///
/// `None` means "fall back to the system default" - which is why an override
/// whose password cannot be opened returns `None` rather than an error. A key
/// rotated out from under a stored password should cost this workspace its
/// override, not its ability to send at all.
pub async fn as_relay(
    pool: &PgPool,
    vault: &SecretVault,
    default_timeout_secs: u64,
) -> ServiceResult<Option<Relay>> {
    let row = store::load(pool).await?;
    let settings = row.to_settings();

    if !settings.is_active() {
        return Ok(None);
    }

    let Some(from) = mailbox(&settings.from_name, &settings.from_address) else {
        return Ok(None);
    };

    let password = match &row.password_sealed {
        None => SecretString::from(String::new()),
        Some(sealed) => match vault.open(sealed, &mail_context()) {
            Ok(opened) => match String::from_utf8(opened.expose_secret().clone()) {
                Ok(password) => SecretString::from(password),
                Err(_) => {
                    tracing::warn!(
                        "the stored relay password is not text; using the system default"
                    );
                    return Ok(None);
                }
            },
            Err(_) => {
                // Deliberately not an error: see the note on this function.
                tracing::warn!(
                    "the stored relay password could not be opened - the encryption key may have \
                     changed; using the system default"
                );
                return Ok(None);
            }
        },
    };

    Ok(Some(Relay {
        host: settings.host,
        port: settings.port,
        username: settings.username,
        password,
        from,
        reply_to: settings
            .reply_to
            .as_deref()
            .and_then(|address| mailbox("", address)),
        encryption: settings.encryption,
        timeout: Duration::from_secs(default_timeout_secs.max(1)),
        tenant_override: true,
    }))
}

/// Which relay a workspace would use.
///
/// Re-exported from `phonix_core` rather than defined here: it is the return
/// type of a server function, so the browser has to be able to name it too.
pub use phonix_core::mail::RelayInUse;

/// What [`super::resolve`] would pick, without building a transport for it.
pub async fn in_use(
    pool: &PgPool,
    caller: &Caller,
    config: &phonix_config::SmtpConfig,
    vault: Option<&SecretVault>,
) -> ServiceResult<RelayInUse> {
    caller.require(permissions::SETTINGS)?;

    match super::resolve(pool, config, vault).await? {
        Some(relay) if relay.tenant_override => Ok(RelayInUse::Workspace { host: relay.host }),
        Some(relay) => Ok(RelayInUse::SystemDefault { host: relay.host }),
        None => Ok(RelayInUse::None),
    }
}
