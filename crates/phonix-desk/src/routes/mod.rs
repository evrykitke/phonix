//! The routes, and the guard in front of them.
//!
//! Every page is a `GET` that renders and every action is a `POST` that
//! redirects, which is not nostalgia: it is what makes the "complete without
//! JavaScript" rule in [`crate::html`] true rather than aspirational, and it
//! gives the back button and the reload button their ordinary meanings.

pub mod accounts;
pub mod session;
pub mod setup;
pub mod workspaces;

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Router, http::request::Parts};
use http::{HeaderMap, StatusCode, header};
use phonix_db::desk::session::ClientFacts;
use phonix_services::desk::DeskCaller;

use crate::state::DeskState;

/// Everything Desk answers.
pub fn router(state: DeskState) -> Router {
    Router::new()
        // Signed in.
        .route("/", get(workspaces::index))
        .route("/accounts", get(accounts::index).post(accounts::create))
        .route("/accounts/{id}/disabled", post(accounts::set_disabled))
        // Signing in.
        .route(
            "/sign-in",
            get(session::sign_in_form).post(session::sign_in),
        )
        .route("/mfa", get(session::code_form).post(session::answer_code))
        .route("/sign-out", post(session::sign_out))
        // Setting up an account somebody else created.
        .route("/setup/{token}", get(setup::form).post(setup::submit))
        // For the systemd unit and nothing else.
        .route("/health", get(health))
        .fallback(not_found)
        .with_state(state)
}

/// Liveness, in the shape `phonix-server` already uses.
///
/// No dependency is touched: a check that fails when Postgres is down would
/// have systemd restart a process whose only problem is that it has nothing to
/// talk to yet.
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn not_found() -> Response {
    let body = crate::html::Page::new(
        "Not found",
        r#"<div class="panel"><h1>Not found</h1>
           <p class="lede">There is no page at that address.</p>
           <p><a href="/">Back to workspaces</a></p></div>"#,
    )
    .render();

    (StatusCode::NOT_FOUND, crate::routes::html_response(body)).into_response()
}

/// An HTML response with the headers every Desk page carries.
///
/// `no-store` because these pages hold workspace names, addresses and audit
/// detail, and a shared machine's back button should not reproduce them after a
/// sign-out. The rest are the ordinary hardening headers; nginx may set them
/// too, and both saying the same thing is fine.
pub fn html_response(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "same-origin"),
            (
                header::CONTENT_SECURITY_POLICY,
                // Desk serves no script and no external asset. Saying so means
                // a future page that reaches for one fails visibly here rather
                // than quietly depending on a CDN.
                "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; \
                 base-uri 'none'; frame-ancestors 'none'",
            ),
        ],
        body,
    )
        .into_response()
}

/// What the client sent, as far as it can be trusted.
///
/// Owned, and turned into borrowed [`ClientFacts`] at the call site: the
/// services layer takes references so a record can be written without copying,
/// and a helper that returned those directly would have to leak the strings
/// they point at.
pub struct Client {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

impl Client {
    /// Read what a request says about who sent it.
    ///
    /// The address comes from the header the proxy sets, named once in
    /// `[security.rate_limit] client_ip_header`. With nothing configured there
    /// is **no** address rather than a wrong one: behind nginx the socket
    /// address is nginx, and putting that on every audit row is worse than
    /// leaving it empty, because it looks like an answer.
    pub fn read(headers: &HeaderMap, state: &DeskState) -> Self {
        let ip = state
            .config
            .security
            .rate_limit
            .ip_header()
            .and_then(|name| headers.get(&name))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            // The column refuses more than 256 characters, and a header is
            // whatever the client felt like sending.
            .map(|value| value.chars().take(256).collect::<String>());

        Self { ip, user_agent }
    }

    pub fn facts(&self) -> ClientFacts<'_> {
        ClientFacts {
            ip: self.ip.as_deref(),
            user_agent: self.user_agent.as_deref(),
        }
    }
}

/// A signed-in desk user.
///
/// The guard. A request that reaches a handler taking this has a live session
/// that has cleared the second factor - there is no variant of it that has not,
/// which is what stops a half-authenticated session from being *nearly* signed
/// in somewhere nobody checked.
pub struct SignedIn(pub DeskCaller);

impl FromRequestParts<DeskState> for SignedIn {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &DeskState,
    ) -> Result<Self, Self::Rejection> {
        let caller = caller_from(parts, state).await;

        match caller {
            Some(caller) if caller.is_signed_in() => Ok(Self(caller)),
            // A password was accepted and the code was not: the only place to
            // go is the code box.
            Some(_) => Err(Redirect::to("/mfa").into_response()),
            None => Err(Redirect::to("/sign-in").into_response()),
        }
    }
}

/// A session that exists, whether or not it has cleared the second factor.
///
/// Only the code box and sign-out take this.
pub struct HalfSignedIn(pub DeskCaller);

impl FromRequestParts<DeskState> for HalfSignedIn {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &DeskState,
    ) -> Result<Self, Self::Rejection> {
        match caller_from(parts, state).await {
            Some(caller) => Ok(Self(caller)),
            None => Err(Redirect::to("/sign-in").into_response()),
        }
    }
}

async fn caller_from(parts: &Parts, state: &DeskState) -> Option<DeskCaller> {
    let header = parts.headers.get(header::COOKIE)?.to_str().ok()?;
    let token = crate::cookie::read(header)?;

    match phonix_services::desk::auth::authenticate(state.pool(), state.desk(), &token).await {
        Ok(caller) => caller,
        Err(err) => {
            // A database that cannot answer is not an invalid session, and
            // treating it as one would silently sign everybody out during an
            // outage rather than saying what happened.
            tracing::error!(error = %err, "could not resolve a desk session");
            None
        }
    }
}

/// What to show when a use case failed for a reason nobody can act on.
///
/// Deliberately vague on the page and loud in the log: the person in front of
/// Desk cannot fix a broken catalog connection, and the detail belongs where
/// somebody debugging will look.
pub fn internal_error(err: impl std::fmt::Display, doing: &str) -> Response {
    tracing::error!(error = %err, doing, "desk request failed");

    let body = crate::html::Page::new(
        "Something went wrong",
        r#"<div class="panel"><h1>Something went wrong</h1>
           <p class="lede">The details are in the log. Try again; if it keeps
           happening, look at <code>journalctl -u phonix-desk</code>.</p></div>"#,
    )
    .render();

    (StatusCode::INTERNAL_SERVER_ERROR, html_response(body)).into_response()
}

/// Redirect after a `POST`, so a reload does not repeat the action.
pub fn see_other(location: &str) -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

/// Redirect and set a cookie in the same response.
pub fn see_other_with_cookie(location: &str, cookie: String) -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, location.to_owned()),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}
