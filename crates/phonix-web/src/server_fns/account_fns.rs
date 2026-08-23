//! Your own account: the profile, and the second factor on it.
//!
//! Every function here is about the *caller's* account. None takes a user id,
//! which is the simplest possible way of guaranteeing that a request cannot
//! enrol a factor on somebody else's account by editing a form field. An
//! administrator acting on another account uses the user-administration
//! endpoints, which state `Users.Edit` and are audited as such.
//!
//! # The secret is shown once
//!
//! [`start_totp_enrolment`] returns the shared secret, its QR code, and a
//! factor id. Nothing returns it again. If the page is closed before
//! [`confirm_totp`] succeeds, the unconfirmed row is dead weight and enrolment
//! starts over - which is the correct outcome, because a secret half-typed into
//! an authenticator app is a lockout waiting to happen.

use leptos::prelude::*;
use phonix_core::identity::{MfaStatus, RecoveryCodes};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The state of the caller's second factor, plus what the workspace requires.
#[server(name = MyMfaStatus, prefix = "/api", endpoint = "account/mfa")]
pub async fn my_mfa_status() -> Result<MfaStatus, ServerFnError> {
    use crate::state::{pool_and_caller, service_error, workspace_settings};

    let (pool, caller) = pool_and_caller().await?;
    let settings = workspace_settings().await?;
    let user_id = caller
        .user_id()
        .ok_or_else(|| ServerFnError::new(phonix_core::Error::Unauthenticated))?;

    phonix_services::identity::mfa::status(
        &pool,
        &settings.mfa,
        user_id,
        account_age_days(&pool, user_id).await?,
    )
    .await
    .map_err(service_error)
}

/// A started enrolment: the secret, once, in the two forms a person can use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartedEnrolment {
    pub factor_id: Uuid,
    /// RFC 4648 base32, for typing into an app that will not scan.
    pub secret_base32: String,
    /// An inline `<svg>`, rendered server-side from the `otpauth://` URI.
    ///
    /// Rendered here rather than in the browser so the QR library - and the
    /// secret it encodes - never enters the wasm bundle.
    pub qr_svg: String,
    pub digits: u8,
    pub period_secs: u64,
}

/// Begin enrolling an authenticator app.
#[server(name = StartTotpEnrolment, prefix = "/api", endpoint = "account/mfa/start")]
pub async fn start_totp_enrolment() -> Result<StartedEnrolment, ServerFnError> {
    use crate::state::workspace_settings;
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;
    let (pool, caller) = pool_and_caller().await?;
    let settings = workspace_settings().await?;

    let user = caller
        .auth_user()
        .ok_or_else(|| ServerFnError::new(phonix_core::Error::Unauthenticated))?;

    let enrolment = phonix_services::identity::mfa::begin_totp_enrolment(
        &pool,
        &state.vault,
        &state.config.security.mfa,
        &settings.mfa,
        &caller,
        user.id,
        // What the authenticator app shows under the workspace name. The email
        // rather than the display name: somebody with two accounts in the same
        // workspace has to be able to tell the two entries apart.
        &user.email,
        tenant.display_name.as_str(),
    )
    .await
    .map_err(service_error)?;

    Ok(StartedEnrolment {
        qr_svg: qr_svg(&enrolment.provisioning_uri),
        factor_id: enrolment.factor_id,
        secret_base32: enrolment.secret_base32,
        digits: enrolment.digits,
        period_secs: enrolment.period_secs,
    })
}

/// Finish enrolment by proving a code can be produced from the new secret.
///
/// `Ok(false)` for a wrong code, not an error: mistyping six digits is an
/// ordinary thing to do and the screen offers another go.
#[server(name = ConfirmTotp, prefix = "/api", endpoint = "account/mfa/confirm")]
pub async fn confirm_totp(factor_id: Uuid, code: String) -> Result<bool, ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error};

    let state = app_state()?;
    let (pool, caller) = pool_and_caller().await?;
    let user_id = caller
        .user_id()
        .ok_or_else(|| ServerFnError::new(phonix_core::Error::Unauthenticated))?;

    phonix_services::identity::mfa::confirm_totp(
        &pool,
        &state.vault,
        &state.config.security.mfa,
        user_id,
        factor_id,
        code.trim(),
    )
    .await
    .map_err(service_error)
}

/// Remove one of the caller's own factors.
#[server(name = RemoveMyFactor, prefix = "/api", endpoint = "account/mfa/remove")]
pub async fn remove_my_factor(factor_id: Uuid) -> Result<bool, ServerFnError> {
    use crate::state::{pool_and_caller, service_error, workspace_settings};

    let (pool, caller) = pool_and_caller().await?;
    let settings = workspace_settings().await?;
    let user_id = caller
        .user_id()
        .ok_or_else(|| ServerFnError::new(phonix_core::Error::Unauthenticated))?;

    phonix_services::identity::mfa::remove_factor(
        &pool,
        &settings.mfa,
        &caller,
        user_id,
        factor_id,
        account_age_days(&pool, user_id).await?,
    )
    .await
    .map_err(service_error)
}

/// How many codes a fresh set contains.
///
/// Ten is the number every authenticator flow settles on: enough that losing a
/// couple does not matter, few enough to print on one line each.
pub const RECOVERY_CODE_COUNT: usize = 10;

/// Issue a new set of recovery codes, invalidating any outstanding ones.
#[server(name = NewRecoveryCodes, prefix = "/api", endpoint = "account/mfa/recovery-codes")]
pub async fn new_recovery_codes() -> Result<RecoveryCodes, ServerFnError> {
    use crate::state::{pool_and_caller, service_error, workspace_settings};

    let (pool, caller) = pool_and_caller().await?;
    let settings = workspace_settings().await?;
    let user_id = caller
        .user_id()
        .ok_or_else(|| ServerFnError::new(phonix_core::Error::Unauthenticated))?;

    phonix_services::identity::mfa::generate_recovery_codes(
        &pool,
        &settings.mfa,
        &caller,
        user_id,
        RECOVERY_CODE_COUNT,
    )
    .await
    .map_err(service_error)
}

/// What the change-password form needs to check as you type.
///
/// Ungated on purpose: it is this workspace's own rule, applied to the person
/// it applies to, and the form cannot show a green field the server will reject
/// without it.
#[server(name = MyPasswordPolicy, prefix = "/api", endpoint = "account/password-policy")]
pub async fn my_password_policy() -> Result<phonix_core::identity::PasswordPolicy, ServerFnError> {
    Ok(crate::state::workspace_settings().await?.password)
}

/// How the change-password form came back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PasswordChangeResult {
    Changed,
    /// The current password was wrong, or the new one failed the policy.
    Rejected(Vec<phonix_core::identity::FieldError>),
}

/// Change the caller's own password.
#[server(name = ChangeMyPassword, prefix = "/api", endpoint = "account/password")]
pub async fn change_my_password(
    current: String,
    new_password: String,
) -> Result<PasswordChangeResult, ServerFnError> {
    use phonix_services::identity::password::PasswordChange;
    use secrecy::SecretString;

    use crate::state::{app_state, pool_and_caller, service_error, session_token};

    let state = app_state()?;
    let (pool, caller) = pool_and_caller().await?;

    // The session this request arrived on survives; every other one is closed.
    // Changing a password is what somebody does after "I think someone got
    // in", and signing the browser they are using back out would punish them
    // for it.
    let keep = session_token().await;

    let outcome = phonix_services::identity::password::change_own_password(
        &pool,
        &state.security(),
        &caller,
        &SecretString::from(current),
        &SecretString::from(new_password),
        keep.as_ref(),
    )
    .await
    .map_err(service_error)?;

    Ok(match outcome {
        PasswordChange::Changed => PasswordChangeResult::Changed,
        // A rejection is an answer, not a failure: the form shows it per field.
        PasswordChange::Rejected(errors) => PasswordChangeResult::Rejected(errors),
    })
}

/// How old the account is, which is what the grace period is measured against.
#[cfg(feature = "ssr")]
async fn account_age_days(
    pool: &phonix_db::PgPool,
    user_id: phonix_core::identity::UserId,
) -> Result<i64, ServerFnError> {
    use crate::state::service_error;

    let account = phonix_db::identity::user::find_by_id(pool, user_id)
        .await
        .map_err(|err| service_error(err.into()))?;

    Ok(account
        .map(|account| (chrono::Utc::now() - account.created_at).num_days())
        .unwrap_or(0))
}

/// Render an `otpauth://` URI as an inline SVG.
///
/// Falls back to an empty string rather than failing the whole enrolment: the
/// secret is also shown in base32 beside it, so a missing QR is an
/// inconvenience and not a dead end.
#[cfg(feature = "ssr")]
fn qr_svg(uri: &str) -> String {
    use qrcode::render::svg;
    use qrcode::{EcLevel, QrCode};

    // `M` rather than the default `L`: this is scanned off a screen at whatever
    // brightness the user has, and the extra redundancy costs a few modules.
    match QrCode::with_error_correction_level(uri.as_bytes(), EcLevel::M) {
        Ok(code) => code
            .render()
            .min_dimensions(180, 180)
            // Fixed colours rather than theme tokens: a QR code needs contrast
            // to scan, and a dark-mode inversion is exactly what breaks it.
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build()
            // The renderer prepends an XML declaration, which is what you want
            // for a standalone `.svg` file and invalid inside an HTML document.
            // This string is injected with `inner_html`, so it has to be a
            // fragment.
            .trim_start_matches("<?xml version=\"1.0\" standalone=\"yes\"?>")
            .to_owned(),
        Err(err) => {
            tracing::error!(error = %err, "could not render the enrolment QR code");
            String::new()
        }
    }
}
