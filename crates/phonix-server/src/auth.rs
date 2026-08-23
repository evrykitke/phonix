//! `/auth/handoff` - trading a single-use token for a session cookie.
//!
//! An axum handler rather than a server function, because it is reached by a
//! plain browser navigation: the response is a redirect with a `Set-Cookie`,
//! and there is no client-side code involved at all.
//!
//! # Why this endpoint exists
//!
//! Session cookies are host-only - no `Domain` attribute, ever - so one
//! workspace's server never receives another's token. The cost is that a form
//! running anywhere but the workspace's own host cannot set the cookie itself.
//! Two flows hit that wall:
//!
//! * the end of signup, on the bare domain, for a workspace that has just been
//!   created on a subdomain;
//! * a sign-in submitted on the bare domain with a workspace address typed in.
//!
//! Both issue a token that lives for seconds and can be redeemed once. That is
//! deliberately not a session token: this URL lands in browser history, proxy
//! logs and `Referer` headers, and none of those should ever hold a credential
//! that stays valid.

use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use phonix_core::TenantSummary;
use phonix_db::identity::session::ClientFacts;
use phonix_web::state::AppState;
use secrecy::SecretString;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct HandoffQuery {
    token: String,
}

/// Redeem a handoff token and land on the workspace.
pub async fn handoff(
    State(state): State<AppState>,
    tenant: Option<axum::Extension<TenantSummary>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<HandoffQuery>,
) -> Response {
    // The tenant comes from the host, never from the token. A token redeemed on
    // the wrong host must fail rather than open a session in whichever workspace
    // it happens to name.
    let Some(axum::Extension(tenant)) = tenant else {
        return (
            StatusCode::BAD_REQUEST,
            "This link must be opened on a workspace address.",
        )
            .into_response();
    };

    let Ok(handle) = state.tenants.resolve(&tenant.slug).await else {
        return (StatusCode::SERVICE_UNAVAILABLE, "Workspace unavailable.").into_response();
    };

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok());

    let redeemed = phonix_services::redeem_handoff(
        &handle.pool,
        &state.security(),
        &SecretString::from(query.token),
        ClientFacts { ip, user_agent },
    )
    .await;

    let signed_in = match redeemed {
        Ok(Some(signed_in)) => signed_in,
        // Unknown, expired, already spent, or an account that has since lost
        // the right to sign in. All the same answer: go and sign in properly.
        Ok(None) => return Redirect::to("/?expired=1").into_response(),
        Err(err) => {
            tracing::error!(error = %err, tenant = %tenant.slug, "handoff redemption failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "Could not sign you in.").into_response();
        }
    };

    let Some(token) = &signed_in.token else {
        tracing::error!("handoff redemption produced no session");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Could not sign you in.").into_response();
    };

    let cookie = phonix_web::server::cookie::set_session(
        &state.config.security.session,
        tenant.slug.as_str(),
        token,
        signed_in.max_age_secs,
    );

    let Ok(cookie) = HeaderValue::from_str(&cookie) else {
        tracing::error!("could not build the session cookie");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Could not sign you in.").into_response();
    };

    // Where the outcome says, not always the dashboard: a workspace that
    // requires a second factor sends the browser to the challenge instead.
    let mut response = Redirect::to(signed_in.result.next_path()).into_response();
    response.headers_mut().append(header::SET_COOKIE, cookie);

    tracing::info!(tenant = %tenant.slug, "handoff redeemed");
    response
}
