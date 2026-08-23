//! Signing in and out.
//!
//! # Two hosts, two shapes
//!
//! On a workspace host (`acme.localhost:3000`) the tenant is known, so signing
//! in sets the session cookie directly on the response.
//!
//! On the bare domain (`localhost:3000`) there is no tenant, so the form also
//! asks which workspace. Authentication happens against that workspace's
//! database, and the browser is then sent to the workspace's own host carrying
//! a single-use handoff token - because the cookie is host-only and this host
//! cannot set one for a subdomain it does not control.

use leptos::prelude::*;
use phonix_core::identity::{AuthUser, LoginResult};
use serde::{Deserialize, Serialize};

/// What the sign-in form submits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignInInput {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub remember_me: bool,
    /// The workspace address, when signing in from the bare domain.
    ///
    /// Ignored on a workspace host, where the tenant comes from the request:
    /// a form field must not be able to point authentication at a different
    /// workspace than the one whose page is being viewed.
    #[serde(default)]
    pub workspace: String,
}

/// The result, plus where the browser goes next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInResponse {
    pub result: LoginResult,
    /// Where to send the browser. Absolute when the session lives on another
    /// host, relative when the cookie was just set on this one.
    pub redirect_to: Option<String>,
}

/// Verify credentials and open a session.
#[server(name = SignIn, prefix = "/api", endpoint = "sign-in")]
pub async fn sign_in(input: SignInInput) -> Result<SignInResponse, ServerFnError> {
    use phonix_core::TenantSlug;
    use phonix_core::identity::Credentials;
    use phonix_db::identity::session::ClientFacts;
    use phonix_services::Delivery;
    use secrecy::ExposeSecret;

    use crate::server::cookie;
    use crate::state::{app_state, optional_tenant, set_response_cookie};

    let state = app_state()?;
    let on_workspace_host = optional_tenant().await;

    // Which workspace to authenticate against, and how the answer gets home.
    let (slug, delivery) = match &on_workspace_host {
        Some(tenant) => (tenant.slug.clone(), Delivery::Cookie),
        None => {
            let Ok(slug) = TenantSlug::parse(input.workspace.trim()) else {
                // An unparseable or empty workspace is the same answer as a
                // wrong password: this form must not report which workspaces
                // exist.
                return Ok(SignInResponse {
                    result: LoginResult::Rejected,
                    redirect_to: None,
                });
            };
            (slug, Delivery::Handoff)
        }
    };

    // An unknown workspace is also just `Rejected`, and deliberately so.
    let Ok(handle) = state.tenants.resolve(&slug).await else {
        return Ok(SignInResponse {
            result: LoginResult::Rejected,
            redirect_to: None,
        });
    };

    let facts = client_facts().await;
    let signed_in = phonix_services::sign_in(
        &handle.pool,
        &state.security(),
        &Credentials {
            email: input.email,
            password: input.password,
            remember_me: input.remember_me,
        },
        ClientFacts {
            ip: facts.0.as_deref(),
            user_agent: facts.1.as_deref(),
        },
        delivery,
    )
    .await
    .map_err(|err| ServerFnError::new(phonix_core::Error::from(err)))?;

    // A rejection or a lockout: nothing to set, nowhere to go.
    if !signed_in.result.password_accepted() {
        return Ok(SignInResponse {
            result: signed_in.result,
            redirect_to: None,
        });
    }

    if let Some(token) = &signed_in.token {
        set_response_cookie(cookie::set_session(
            &state.config.security.session,
            slug.as_str(),
            token,
            signed_in.max_age_secs,
        ))?;

        return Ok(SignInResponse {
            redirect_to: Some(signed_in.result.next_path().to_owned()),
            result: signed_in.result,
        });
    }

    let handoff = signed_in
        .handoff
        .as_ref()
        .ok_or_else(|| ServerFnError::new("sign-in produced neither a session nor a handoff"))?;

    Ok(SignInResponse {
        redirect_to: Some(format!(
            "{}/auth/handoff?token={}",
            state.config.server.tenant_origin(slug.as_str()),
            urlencode(handoff.expose_secret())
        )),
        result: signed_in.result,
    })
}

/// End the current session.
/// What happened when somebody followed an invitation link.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AcceptInvitationResult {
    /// The account is set up. They can sign in now.
    Accepted { email: String },
    /// Unknown, expired, or already used - deliberately one answer, because
    /// distinguishing them tells whoever intercepted the link that it was real.
    LinkNotUsable,
    /// The link was fine; the password was not.
    Rejected(Vec<phonix_core::identity::FieldError>),
}

/// Set the password on an invited account.
///
/// Reachable with no session, like signing in - the person following the link
/// does not have one yet, which is the point of an invitation.
#[server(name = AcceptInvitation, prefix = "/api", endpoint = "invitations/accept")]
pub async fn accept_invitation(
    token: String,
    password: String,
) -> Result<AcceptInvitationResult, ServerFnError> {
    use phonix_services::identity::invitation::{self, Acceptance};
    use secrecy::SecretString;

    use crate::state::{app_state, service_error, tenant_pool};

    let pool = tenant_pool().await?;
    let state = app_state()?;

    let outcome = invitation::accept(
        &pool,
        &state.hasher,
        &SecretString::from(token),
        &SecretString::from(password),
    )
    .await
    .map_err(service_error)?;

    Ok(match outcome {
        Acceptance::Accepted { email, .. } => AcceptInvitationResult::Accepted { email },
        Acceptance::NotUsable => AcceptInvitationResult::LinkNotUsable,
        Acceptance::Rejected(errors) => AcceptInvitationResult::Rejected(errors),
    })
}

#[server(name = SignOut, prefix = "/api", endpoint = "sign-out")]
pub async fn sign_out() -> Result<(), ServerFnError> {
    use crate::server::cookie;
    use crate::state::{
        app_state, current_caller, session_token, set_response_cookie, tenant_from_request,
        tenant_pool,
    };

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;

    if let Some(token) = session_token().await {
        let pool = tenant_pool().await?;
        let caller = current_caller().await?;

        phonix_services::sign_out(&pool, &token, caller.and_then(|caller| caller.user_id()))
            .await
            .map_err(|err| ServerFnError::new(phonix_core::Error::from(err)))?;
    }

    // Cleared even when there was no session: a cookie for a session the server
    // has already forgotten should not survive a sign-out.
    set_response_cookie(cookie::clear_session(
        &state.config.security.session,
        tenant.slug.as_str(),
    ))?;

    Ok(())
}

/// Answer the second-factor challenge on a half-authenticated session.
///
/// The session id comes from the cookie, never from the caller. A challenge
/// endpoint that took one as a parameter would let anybody who guessed a code
/// satisfy somebody else's pending sign-in.
///
/// Rejections come back as `Ok(..)`: a mistyped code is an ordinary thing, and
/// the answer carries how many attempts are left, which the person entering it
/// has already earned by proving the password.
#[server(name = AnswerMfaChallenge, prefix = "/api", endpoint = "mfa-challenge")]
pub async fn answer_mfa_challenge(
    code: String,
) -> Result<phonix_core::identity::MfaChallengeResult, ServerFnError> {
    use phonix_core::identity::MfaChallengeResult;

    use crate::state::{app_state, service_error, session_token, tenant_pool};

    let state = app_state()?;
    let pool = tenant_pool().await?;

    // Not `current_caller`: that resolves an `AuthUser`, and this session has
    // deliberately not finished authenticating. What is needed here is the
    // session row itself.
    let Some(token) = session_token().await else {
        return Ok(MfaChallengeResult::NoChallenge);
    };

    let Some(session) =
        phonix_services::identity::session::resume(&pool, &token, &state.config.security.session)
            .await
            .map_err(service_error)?
    else {
        return Ok(MfaChallengeResult::NoChallenge);
    };

    phonix_services::identity::mfa::answer_challenge(
        &pool,
        &state.vault,
        &state.config.security.mfa,
        session.id,
        session.user_id,
        code.trim(),
    )
    .await
    .map_err(service_error)
}

/// The half-authenticated account waiting at the challenge screen.
///
/// [`current_user`] returns `None` for it - the session has not finished
/// authenticating - so the challenge page has no other way to greet somebody by
/// name or to know it should not be showing at all.
#[server(name = PendingChallenge, prefix = "/api", endpoint = "mfa-challenge/pending")]
pub async fn pending_challenge() -> Result<Option<PendingChallengeInfo>, ServerFnError> {
    use crate::state::{app_state, service_error, session_token, tenant_pool};

    let state = app_state()?;
    let pool = tenant_pool().await?;

    let Some(token) = session_token().await else {
        return Ok(None);
    };

    let Some(session) =
        phonix_services::identity::session::resume(&pool, &token, &state.config.security.session)
            .await
            .map_err(service_error)?
    else {
        return Ok(None);
    };

    if session.mfa_satisfied {
        // Already through. The page redirects rather than asking again.
        return Ok(None);
    }

    let account = phonix_db::identity::user::find_by_id(&pool, session.user_id)
        .await
        .map_err(|err| service_error(err.into()))?;

    let policy = crate::state::workspace_settings().await?.mfa;

    Ok(account.map(|account| PendingChallengeInfo {
        display_name: account.display_name,
        email: account.email,
        recovery_codes_allowed: policy.allow_recovery_codes,
    }))
}

/// Who the challenge screen is asking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingChallengeInfo {
    pub display_name: String,
    pub email: String,
    /// Whether to offer "use a recovery code" as a way out.
    pub recovery_codes_allowed: bool,
}

/// Who is signed in, if anyone.
///
/// `None` rather than an error for an anonymous request: "nobody" is a perfectly
/// good answer, and the layout renders differently rather than failing.
#[server(name = CurrentUser, prefix = "/api", endpoint = "current-user")]
pub async fn current_user() -> Result<Option<AuthUser>, ServerFnError> {
    use crate::state::current_caller;

    Ok(current_caller()
        .await?
        .and_then(|caller| caller.auth_user().cloned()))
}

/// The caller's address and user agent, for `sessions` and the audit trail.
#[cfg(feature = "ssr")]
async fn client_facts() -> (Option<String>, Option<String>) {
    let Ok(headers) = leptos_axum::extract::<http::HeaderMap>().await else {
        return (None, None);
    };

    // `x-forwarded-for` first: behind a proxy the socket address is the proxy.
    // Only the first entry is read - the rest are appended by intermediaries,
    // and the client controls whatever it sent.
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_owned());

    let user_agent = headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        // Bounded: this is stored, and an unbounded header should not decide
        // how wide a column has to be.
        .map(|value| value.chars().take(256).collect());

    (ip, user_agent)
}

/// Percent-encode a token for a query string.
#[cfg(feature = "ssr")]
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
