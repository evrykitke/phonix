//! Turning a request into somebody `/api/v1` will listen to.
//!
//! Four questions, in this order, and the order is the point:
//!
//! 1. **Which workspace?** From the host, by the same middleware a page goes
//!    through. There is no tenant parameter and never will be: a caller that
//!    could name its own workspace would be a caller that could try somebody
//!    else's.
//! 2. **Which credential is this?** Two are accepted, and the `phx_` prefix
//!    says which without a second header or a `?type=`.
//! 3. **Is it live?** One indexed lookup on a digest, either way.
//! 4. **May it do this?** Not here. That is `Caller::require`, inside the use
//!    case, where every other adapter's answer comes from.
//!
//! The extractor stops at 3 and hands step 4 the caller it built.
//!
//! # Two credentials, one door
//!
//! ```text
//! Authorization: Bearer phx_…   →  API key   →  owner ∩ scopes   →  Caller
//! Authorization: Bearer <other> →  session   →  the person       →  Caller
//! ```
//!
//! An **API key** is a machine acting for the person who issued it, narrowed by
//! scopes and gated by the `api_enabled` licence. A **session** is a person
//! signed in on their own phone, holding exactly what they hold in a browser.
//! Everything downstream is indifferent to which: both arms end at a
//! [`Caller`], and `Caller::require` inside each use case is the only gate.
//!
//! Keeping the two arms converging is the whole safety argument here. A check
//! that exists on one path and not the other is the failure this shape is
//! arranged to make obvious - which is why they are a single `match` in a
//! single function rather than two extractors that happen to look alike.
//!
//! See `docs/adr/0002-public-api.md` and
//! `docs/adr/0003-mobile-authentication.md`.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use phonix_core::{Error as CoreError, TenantSummary};
use phonix_db::PgPool;
use phonix_services::identity::{api_key, authentication};
use phonix_services::Caller;
use phonix_web::state::AppState;
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use super::problem::Problem;

/// The prefix that marks one of our API keys.
///
/// Introduced by ADR 0002 so a secret scanner could recognise the credential in
/// a public repository; it now earns a second job as the discriminator between
/// the two lookups. Session tokens are bare url-safe base64 and never begin
/// with it, so the split is total.
const KEY_PREFIX: &str = "phx_";

/// The workspace a request was routed to, and its pool.
///
/// For the handful of endpoints that run *before* anybody is authenticated -
/// signing in, above all. It resolves the tenant and nothing else, so a handler
/// taking this has no caller to require anything of and cannot pretend it does.
pub struct ApiWorkspace {
    pub pool: PgPool,
    /// The request's headers, so a handler that has to read the bearer itself -
    /// answering a second factor, signing out - can, without a second
    /// extractor whose only job is to fail differently.
    pub headers: axum::http::HeaderMap,
    /// Cloned rather than borrowed: an extractor cannot hand out a reference
    /// into state, and `AppState` is a handful of `Arc`s by design.
    pub state: AppState,
}

impl FromRequestParts<AppState> for ApiWorkspace {
    type Rejection = Problem;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The workspace itself is not kept: it is already on the request's
        // tracing span, and a field nothing reads is a field somebody later
        // trusts to mean something.
        let (pool, _tenant) = workspace(parts, state).await?;

        Ok(Self {
            pool,
            headers: parts.headers.clone(),
            state: state.clone(),
        })
    }
}

/// An authenticated API caller, and everything a handler needs to serve it.
pub struct ApiCaller {
    /// Who this acts as. For a key: its owner, narrowed to the key's scopes.
    /// For a session: the signed-in person, with everything they hold.
    pub caller: Caller,
    /// This workspace's pool.
    pub pool: PgPool,
    /// Which key this was, so a handler that changes something can say which
    /// credential did it. `None` for a session, where the acting user is on the
    /// caller already. The workspace itself is on the request's tracing span,
    /// put there by the middleware that resolved it.
    pub key_id: Option<Uuid>,
}

impl FromRequestParts<AppState> for ApiCaller {
    type Rejection = Problem;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let (pool, tenant) = workspace(parts, state).await?;

        let Some(presented) = bearer_of(&parts.headers) else {
            return Err(Problem::unauthenticated(
                "Send an API key as `Authorization: Bearer phx_...`, or a session \
                 token from `POST /api/v1/auth/token`.",
            ));
        };

        if presented.expose_secret().starts_with(KEY_PREFIX) {
            api_key_caller(&pool, &tenant, presented).await
        } else {
            session_caller(&pool, state, presented).await
        }
    }
}

/// The API-key arm: a licence, then a credential.
async fn api_key_caller(
    pool: &PgPool,
    tenant: &TenantSummary,
    presented: SecretString,
) -> Result<ApiCaller, Problem> {
    // The licence, before the credential. A workspace that has not been sold
    // the API answers the same way to a valid key and an invented one, which is
    // what stops this being a place to test tokens.
    if !api_key::api_enabled(pool).await.map_err(Problem::from)? {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "api_disabled",
            "This workspace does not have API key access. An administrator can turn \
             it on. Signing in with `POST /api/v1/auth/token` is not affected.",
        ));
    }

    let authenticated = api_key::authenticate(pool, &presented)
        .await
        .map_err(Problem::from)?;

    let Some(authenticated) = authenticated else {
        // Deliberately one answer for unknown, revoked, expired and
        // owned-by-a-suspended-account. See the service.
        return Err(Problem::unauthenticated("That key cannot be used."));
    };

    tracing::debug!(
        workspace = %tenant.slug,
        key = %authenticated.key_id,
        "api request authenticated by key"
    );

    Ok(ApiCaller {
        caller: authenticated.caller,
        pool: pool.clone(),
        key_id: Some(authenticated.key_id),
    })
}

/// The session arm: a person signed in on their own device.
///
/// **No `api_enabled` check, deliberately.** That flag is the licence for the
/// API-key surface - a customer's script integrating with the product. Somebody
/// using our own mobile application is using the product, and folding the two
/// together would mean anybody who wants the phone app has to buy "API access".
/// See ADR 0003 §3, which also states the residual this leaves open.
async fn session_caller(
    pool: &PgPool,
    state: &AppState,
    presented: SecretString,
) -> Result<ApiCaller, Problem> {
    let user = authentication::authenticate_session(pool, &presented, &state.config.security)
        .await
        .map_err(Problem::from)?;

    let Some(user) = user else {
        // One answer again, for unknown, expired, revoked and
        // account-since-suspended.
        return Err(Problem::unauthenticated("That session is not valid."));
    };

    tracing::debug!(user = %user.id, "api request authenticated by session");

    Ok(ApiCaller {
        caller: Caller::user(user),
        pool: pool.clone(),
        key_id: None,
    })
}

/// Resolve the workspace this request was routed to.
async fn workspace(
    parts: &mut Parts,
    state: &AppState,
) -> Result<(PgPool, TenantSummary), Problem> {
    // Set by `middleware::resolve_tenant`, which runs below everything.
    let Some(tenant) = parts.extensions.get::<TenantSummary>().cloned() else {
        return Err(CoreError::UnknownTenant(host_of(parts)).into());
    };

    let handle = state.tenants.resolve(&tenant.slug).await.map_err(|err| {
        tracing::warn!(error = %err, tenant = %tenant.slug, "could not resolve a tenant for the api");
        Problem::from(CoreError::Unavailable("workspace".to_owned()))
    })?;

    Ok((handle.pool.clone(), tenant))
}

/// The bearer token, if one was presented in a shape worth looking up.
///
/// Case-insensitive on the scheme because RFC 7235 says so, and clients get it
/// wrong in both directions.
pub fn bearer_of(headers: &axum::http::HeaderMap) -> Option<SecretString> {
    let raw = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim();

    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }

    let token = token.trim();
    (!token.is_empty()).then(|| SecretString::from(token.to_owned()))
}

/// The host as the request wrote it, for the "no such workspace" answer.
///
/// Only ever put in an error message that names the address the caller already
/// used, so it tells them nothing they did not send.
fn host_of(parts: &Parts) -> String {
    parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("this address")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prefix is what routes a token to one lookup or the other, so it has
    /// to be exactly what the issuer mints. A change on one side without the
    /// other sends every key down the session path, where it fails as an
    /// unknown session rather than as a bad key.
    #[test]
    fn the_prefix_is_the_one_keys_are_issued_with() {
        assert_eq!(KEY_PREFIX, phonix_services::identity::api_key::TOKEN_PREFIX);
    }
}
