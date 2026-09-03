//! The routes, and the guard in front of them.
//!
//! Every page is a `GET` that renders and every action is a `POST` that
//! redirects, which is not nostalgia: it is what makes the "complete without
//! JavaScript" rule in [`crate::html`] true rather than aspirational, and it
//! gives the back button and the reload button their ordinary meanings.

pub mod accounts;
pub mod dashboard;
pub mod dependencies;
pub mod queues;
pub mod session;
pub mod setup;
pub mod trail;
pub mod workspaces;

use askama::Template;
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
        // Signed in. The dashboard is the landing page because it is the
        // screen that says whether anything needs attention at all; the
        // workspace list, which says *which* thing, is one click from it.
        .route("/", get(dashboard::index))
        .route("/workspaces", get(workspaces::index))
        .route("/workspaces/{slug}", get(workspaces::show))
        .route("/workspaces/{slug}/licence", post(workspaces::set_licence))
        // Each action is a confirm page and a POST. Without a script a confirm
        // is a page rather than a dialog, which turns out to be better than
        // what it replaces: there is room to say what will happen, the back
        // button means "no", and the address bar says which workspace is about
        // to be acted on.
        .route(
            "/workspaces/{slug}/retry",
            get(workspaces::confirm_retry).post(workspaces::do_retry),
        )
        .route(
            "/workspaces/{slug}/migrate",
            get(workspaces::confirm_migrate).post(workspaces::do_migrate),
        )
        .route(
            "/workspaces/{slug}/suspend",
            get(workspaces::confirm_suspend).post(workspaces::do_suspend),
        )
        .route(
            "/workspaces/{slug}/resume",
            get(workspaces::confirm_resume).post(workspaces::do_resume),
        )
        .route(
            "/workspaces/{slug}/reinvite",
            get(workspaces::confirm_reinvite).post(workspaces::do_reinvite),
        )
        // Not `/workspaces/migrate-outdated`: that address is also a valid
        // workspace slug, and claiming it as a static segment would make a
        // workspace named that unreachable.
        .route(
            "/estate/migrate",
            get(workspaces::confirm_estate_migrate).post(workspaces::do_estate_migrate),
        )
        // `/estate/new` and not `/workspaces/new`, for the same reason: `new`
        // is a perfectly good workspace slug.
        .route(
            "/estate/new",
            get(workspaces::new_form).post(workspaces::create),
        )
        .route("/audit", get(trail::index))
        .route("/queues", get(queues::index))
        .route("/dependencies", get(dependencies::index))
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
        // The two assets Desk serves. Each path carries a content hash, which
        // is what lets them be cached forever - see `crate::assets`.
        .route(crate::assets::STYLESHEET, get(stylesheet))
        .route(crate::assets::SCRIPT, get(script))
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

/// The 404, which is also what a bad slug in a path gets.
///
/// `async` because it is the router's fallback; the handlers that call it
/// directly await it, which costs nothing and keeps one definition.
pub async fn not_found() -> Response {
    let page = crate::html::MessagePage::new("Not found", "There is no page at that address.")
        .back("/", "Back to workspaces");

    (StatusCode::NOT_FOUND, crate::html::render(&page)).into_response()
}

/// The stylesheet.
///
/// Compiled into the binary rather than read from a directory beside it, so
/// Desk stays one artefact: copy the binary, run it. `immutable` is safe to the
/// point of being the reason the name has a hash in it - a changed stylesheet
/// is a different address, so there is nothing here a browser could hold that
/// is wrong.
async fn stylesheet() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        crate::assets::STYLESHEET_CSS,
    )
        .into_response()
}

/// The one script, served the same way and cached the same way.
///
/// Everything it does is an enhancement - see `script/desk.js`. It is a
/// separate file rather than a `<script>` block so that the content security
/// policy can stay `script-src 'self'` with no `unsafe-inline` and no nonce
/// machinery: the only script that can run on a Desk page is one Desk served.
async fn script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        crate::assets::SCRIPT_JS,
    )
        .into_response()
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
                // Desk serves one stylesheet and one script, both from its
                // own binary, and nothing from anywhere else. Saying so means
                // a future page that reaches for a CDN fails visibly here
                // rather than quietly depending on one.
                //
                // `'self'` and never `'unsafe-inline'` for either: both assets
                // are hashed files under `/assets`, so the only code that can
                // run on a Desk page is code Desk served. Nothing here carries
                // a `style=` attribute or an inline handler, and the script is
                // an enhancement the pages do not need - see `script/desk.js`.
                //
                // `style-src 'self'` blocks `style=` attributes too, which is
                // why a coloured swatch anywhere in Desk is an SVG `fill`
                // rather than an inline style: `fill` is a presentation
                // attribute and is not policed, so the tokens keep working
                // without loosening this line.
                "default-src 'none'; style-src 'self'; script-src 'self'; \
                 form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
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

    // Rendered here rather than through `crate::html::render`, which reports a
    // failed render by calling this function: one of the two has to end the
    // recursion, and it is the one that already has nothing better to say.
    let body = crate::html::MessagePage::new(
        "Something went wrong",
        "The details are in the log. Try again; if it keeps happening, look at \
         journalctl -u phonix-desk.",
    )
    .render()
    .unwrap_or_else(|_| "Something went wrong.".to_owned());

    (StatusCode::INTERNAL_SERVER_ERROR, html_response(body)).into_response()
}

/// Redirect after a `POST`, so a reload does not repeat the action.
pub fn see_other(location: &str) -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

// ---------------------------------------------------------------------------
// Query strings
//
// Desk carries three things between a POST and the page it redirects to: a
// setup link, a refusal, and a confirmation. They travel in the query string
// rather than in a flash cookie because the first page that needs one is
// reached before there is a session to hang a cookie on, and because a
// parameter is visible in the address bar - which is the right property for a
// message that is only ever about the request just made.
// ---------------------------------------------------------------------------

/// Percent-encode a value for a query string.
///
/// Small and local rather than a dependency: everything Desk puts in a query
/// string is its own text - a setup link, a refusal, a confirmation - and a
/// crate for that is a crate to keep current for the life of the tool.
pub fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Read one query parameter, percent-decoded.
pub fn query_value(uri: &axum::http::Uri, name: &str) -> Option<String> {
    let query = uri.query()?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == name {
            return Some(urldecode(value));
        }
    }
    None
}

fn urldecode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_survives_a_round_trip() {
        let original = "https://console-desk.example.com/setup/aB3-_x?y&z=1 2";
        assert_eq!(urldecode(&urlencode(original)), original);
    }

    /// The encoder has to escape everything a query string treats specially, or
    /// a setup link containing `&` arrives truncated and does not work.
    #[test]
    fn the_encoder_escapes_query_separators() {
        let encoded = urlencode("a&b=c d");

        assert!(!encoded.contains('&'));
        assert!(!encoded.contains('='));
        assert!(!encoded.contains(' '));
    }

    #[test]
    fn reading_a_parameter_finds_it_among_others() {
        let uri: axum::http::Uri = "/accounts?other=1&link=http%3A%2F%2Fx%2Fy".parse().unwrap();

        assert_eq!(query_value(&uri, "link").as_deref(), Some("http://x/y"));
        assert_eq!(query_value(&uri, "missing"), None);
    }
}
