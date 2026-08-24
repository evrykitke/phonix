//! Recovering an account nobody can sign in to.
//!
//! Both calls are **public** - the whole point is that the caller has no
//! session and cannot get one. Neither takes a `Caller` and neither checks a
//! permission, so the guards are the ones inside
//! [`phonix_services::identity::password_reset`]: a uniform answer, a spent
//! attempt per guess, and a workspace that must actually exist.
//!
//! # These run on a workspace host, and only there
//!
//! A reset needs a specific account in a specific workspace's database, and on
//! the bare domain there is no such thing. `tenant_pool()` is what enforces it:
//! with no tenant on the request it fails, and both calls fail with it.
//!
//! The sign-in screen already asks for a workspace address on the bare domain,
//! so somebody who arrived there reaches their own host before they ever see
//! the "forgot your password" link.

use leptos::prelude::*;
use phonix_core::identity::FieldError;
use serde::{Deserialize, Serialize};

/// What the first screen is told, which is deliberately almost nothing.
///
/// The service's own `ResetRequest` lives in `phonix-services` and stops there,
/// like `invitation::Acceptance` does: the wire type is the web crate's, so the
/// browser bundle never links a server-only crate. Here the two happen to be
/// the same shape, and that is a coincidence worth keeping - if the service
/// ever grows a variant, this is the wall that stops it reaching the client by
/// accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetRequested {
    /// The request was well-formed and has been dealt with.
    ///
    /// **Not** "an email was sent." No account, a suspended account, a relay
    /// that refused - all of them arrive here, because any variant the caller
    /// could distinguish is a membership check against this workspace.
    Accepted,
    /// Self-service reset is switched off for this deployment.
    Disabled,
}

/// What happened when somebody submitted a code and a new password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PasswordResetResult {
    /// Done. Every session for that account is gone, including any the person
    /// still has open elsewhere.
    Reset,
    /// Wrong, expired, out of attempts, already used, or an address with no
    /// account here. One answer for all five - see `AcceptInvitationResult`,
    /// which makes the same trade for the same reason.
    CodeNotUsable,
    /// The code was good and the new password is not acceptable. The code is
    /// spent: reaching this means asking for another one.
    Rejected(Vec<FieldError>),
    /// Self-service reset is switched off for this deployment.
    Disabled,
}

/// Ask for a code.
///
/// **Always succeeds.** An unknown address, a suspended account and a
/// successful send are one answer, because any difference between them is a
/// free membership check against this workspace. See the module note on
/// `password_reset`.
#[server(name = RequestPasswordReset, prefix = "/api", endpoint = "password-reset/request")]
pub async fn request_password_reset(email: String) -> Result<ResetRequested, ServerFnError> {
    use phonix_services::identity::password_reset::ResetRequest;

    use crate::state::{app_state, inviting_context, service_error, tenant_pool};

    let pool = tenant_pool().await?;
    let (_, tenant) = inviting_context().await?;
    let state = app_state()?;

    // Bounded before it reaches a query. Nothing here is a validation message -
    // an address too long to be one is answered exactly like an address that
    // simply has no account.
    if email.len() > 320 {
        return Ok(ResetRequested::Accepted);
    }

    let outcome = phonix_services::identity::password_reset::request(
        &pool,
        &phonix_services::identity::password_reset::Resetting {
            config: &state.config,
            vault: &state.vault,
            workspace_name: &tenant.display_name,
        },
        &email,
        client_ip().await.as_deref(),
    )
    .await
    .map_err(service_error)?;

    Ok(match outcome {
        ResetRequest::Accepted => ResetRequested::Accepted,
        ResetRequest::Disabled => ResetRequested::Disabled,
    })
}

/// What the second screen sends: the code, and the password to set.
///
/// The email travels with it rather than being remembered in a cookie or a
/// server-side stash. It is not a secret - the person typed it on the previous
/// screen - and the server holding no per-browser state between the two calls
/// is what lets a workspace with several server processes answer either one.
/// The screen carries it across the two steps in memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordResetInput {
    pub email: String,
    pub code: String,
    pub password: String,
    pub password_confirmation: String,
}

/// Redeem a code and set a new password.
///
/// The code is spent by the attempt whether or not it was right, so this is not
/// safe to retry blindly - which is what the five-attempt limit is counting.
#[server(name = CompletePasswordReset, prefix = "/api", endpoint = "password-reset/complete")]
pub async fn complete_password_reset(
    input: PasswordResetInput,
) -> Result<PasswordResetResult, ServerFnError> {
    use phonix_services::identity::password_reset::ResetOutcome;
    use secrecy::SecretString;

    use crate::state::{app_state, service_error, tenant_pool};

    // Checked here as well as in the browser, because this endpoint is
    // reachable without one. Cheap, and it happens before the code is spent -
    // the two halves of a mistyped password should not cost an attempt.
    if input.password != input.password_confirmation {
        return Ok(PasswordResetResult::Rejected(vec![FieldError::new(
            "password_confirmation",
            phonix_core::msg!("validation.password.mismatch"),
        )]));
    }

    let pool = tenant_pool().await?;
    let state = app_state()?;

    let outcome = phonix_services::identity::password_reset::redeem(
        &pool,
        &state.security(),
        &state.config,
        &input.email,
        &input.code,
        &SecretString::from(input.password),
    )
    .await
    .map_err(service_error)?;

    Ok(match outcome {
        ResetOutcome::Reset => PasswordResetResult::Reset,
        ResetOutcome::NotUsable => PasswordResetResult::CodeNotUsable,
        ResetOutcome::Rejected(problems) => PasswordResetResult::Rejected(problems),
        ResetOutcome::Disabled => PasswordResetResult::Disabled,
    })
}

/// The caller's address, for the audit trail on the issued token.
///
/// Duplicated from `onboarding_fns` rather than shared: three lines, and the
/// alternative is a `pub` helper in the server-fn layer that exists only to be
/// called twice.
#[cfg(feature = "ssr")]
async fn client_ip() -> Option<String> {
    let headers: http::HeaderMap = leptos_axum::extract().await.ok()?;

    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
