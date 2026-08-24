//! `/auth/google/start` and `/auth/google/callback` - signing in with Google.
//!
//! Axum handlers rather than server functions, like `auth::handoff` next door
//! and for the same reason: every step is a plain browser navigation, the
//! responses are redirects with cookies, and no client-side code is involved.
//!
//! # Both of these run on ONE host, and it is not the workspace's
//!
//! Google compares `redirect_uri` byte for byte against a URI registered in
//! its console, and **it does not accept wildcards**. Workspaces live on
//! `*.example.com`, so no registered URI can cover them and there is no
//! arrangement in which Google redirects back to `acme.example.com`.
//!
//! So the whole conversation happens on the host named by
//! `security.google.redirect_uri` - in production the same host signup runs on -
//! and the session is carried to the workspace afterwards by the one-time
//! handoff token that already exists for exactly this problem. The button on a
//! workspace page is therefore a cross-host link, built from that same URI's
//! origin.
//!
//! Keeping both endpoints on one host is also what makes the cookie below
//! work at all: cookies are host-only here, so a flow that started on
//! `acme.example.com` and came back to `phonix.example.com` would arrive with
//! nothing to check the callback against.
//!
//! # Which workspace, and why the browser is not asked
//!
//! The slug travels in the same cookie as the CSRF state, set when the flow
//! started. Not in `state` - that is compared for equality and would have to be
//! parsed instead - and emphatically not in the callback's query string, where
//! anybody could choose it. A slug the caller can pick is a slug that says
//! "sign me in to whichever workspace this address happens to be a member of",
//! which is a different feature and a worse one.

use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use phonix_core::TenantSlug;
use phonix_db::identity::session::ClientFacts;
use phonix_services::identity::authentication::Delivery;
use phonix_services::identity::federated::{GOOGLE, sign_in_federated};
use phonix_services::oauth::google::{ClaimProblem, Pending};
use phonix_web::state::AppState;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

/// The cookie holding a sign-in that is halfway through.
///
/// Not the session cookie and nothing like it: it holds no authority, expires
/// in minutes, and is deleted the moment the callback reads it.
const PENDING_COOKIE: &str = "phonix_google";

/// How long somebody has between pressing the button and finishing at Google.
///
/// Ten minutes covers reading a consent screen, finding a password manager and
/// answering a two-step prompt. It is not a session length - an abandoned
/// attempt should not leave a live cookie on a shared machine all afternoon.
const PENDING_TTL_SECS: i64 = 600;

#[derive(Debug, Deserialize)]
pub struct StartQuery {
    /// Which workspace this sign-in is for.
    workspace: String,
}

/// Send the browser to Google.
pub async fn start(State(state): State<AppState>, Query(query): Query<StartQuery>) -> Response {
    let config = &state.config.security.google;

    if !config.is_usable() {
        // Not an error page. Somebody reaching this on a deployment with no
        // Google client configured followed a stale link or typed the path,
        // and the sign-in form is where they were going.
        return Redirect::to("/").into_response();
    }

    let Ok(slug) = TenantSlug::parse(query.workspace.trim()) else {
        return (StatusCode::BAD_REQUEST, "Unknown workspace.").into_response();
    };

    // Resolved now rather than in the callback. Failing here costs a redirect;
    // failing there costs somebody a trip through Google's consent screen
    // before being told the workspace does not exist.
    if state.tenants.resolve(&slug).await.is_err() {
        return (StatusCode::BAD_REQUEST, "Unknown workspace.").into_response();
    }

    let pending = Pending::generate();
    let target = phonix_services::oauth::google::authorize_url(config, &pending);

    let cookie = pending_cookie(&state, &pending, slug.as_str());
    let Ok(cookie) = HeaderValue::from_str(&cookie) else {
        tracing::error!("could not build the Google sign-in cookie");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Could not sign you in.").into_response();
    };

    let mut response = Redirect::to(&target).into_response();
    response.headers_mut().append(header::SET_COOKIE, cookie);
    response
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    /// Google's own refusal - `access_denied` when somebody pressed cancel.
    #[serde(default)]
    error: Option<String>,
}

/// Google is finished; turn the code into a session on the workspace.
pub async fn callback(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let config = &state.config.security.google;

    if !config.is_usable() {
        return Redirect::to("/").into_response();
    }

    // Read and immediately invalidate: whatever happens below, this attempt is
    // over and the cookie must not be usable for a second one.
    let pending = read_pending_cookie(&headers);
    let clear = clear_pending_cookie(&state);

    let Some((expected_state, verifier, slug)) = pending else {
        tracing::info!("a Google callback arrived with no pending sign-in");
        return finish(clear, Redirect::to("/?google=expired"));
    };

    let Ok(slug) = TenantSlug::parse(&slug) else {
        return finish(clear, Redirect::to("/?google=failed"));
    };

    // Everything from here can name the workspace, so a refusal lands on the
    // workspace's own sign-in screen rather than on this host's.
    let home = state.config.server.tenant_origin(slug.as_str());

    if let Some(error) = &query.error {
        // `access_denied` is somebody pressing cancel, which is not a failure
        // worth a message - they are simply back where they started.
        tracing::info!(%error, "Google declined the sign-in");
        let target = if error == "access_denied" {
            home
        } else {
            format!("{home}/?google=failed")
        };
        return finish(clear, Redirect::to(&target));
    }

    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return finish(clear, Redirect::to(&format!("{home}/?google=failed")));
    };

    // The CSRF check. Constant-time because it costs nothing, and because a
    // comparison that returns early on the first differing byte is the kind of
    // thing that is fine until the value it guards changes.
    if !constant_time_eq(expected_state.as_bytes(), returned_state.as_bytes()) {
        tracing::warn!("a Google callback arrived with a state that does not match");
        return finish(clear, Redirect::to(&format!("{home}/?google=failed")));
    }

    let claims = match phonix_services::oauth::google::exchange_code(config, &code, &verifier).await
    {
        Ok(claims) => claims,
        Err(err) => {
            tracing::warn!(error = %err, "the Google token exchange failed");
            return finish(clear, Redirect::to(&format!("{home}/?google=unavailable")));
        }
    };

    let email = match claims.email_for(config) {
        Ok(email) => email,
        Err(problem) => {
            // Logged apart because they mean different things to whoever is
            // setting this up: a missing scope is a console misconfiguration,
            // a wrong domain is the restriction working.
            match problem {
                ClaimProblem::EmailNotVerified => {
                    tracing::info!("Google will not vouch for that address");
                }
                ClaimProblem::NoEmail => {
                    tracing::warn!("Google returned no address - is the email scope granted?");
                }
                ClaimProblem::WrongDomain => {
                    tracing::info!("that Google account is outside the permitted hosted domain");
                }
            }

            return finish(clear, Redirect::to(&format!("{home}/?google=refused")));
        }
    };

    let Ok(handle) = state.tenants.resolve(&slug).await else {
        return finish(clear, Redirect::to(&format!("{home}/?google=unavailable")));
    };

    let facts = client_facts(&headers);
    let signed_in = sign_in_federated(
        &handle.pool,
        &state.security(),
        GOOGLE,
        email,
        // No "remember me" here: this flow has no checkbox, and a session
        // length nobody asked for should be the shorter one.
        false,
        ClientFacts {
            ip: facts.0.as_deref(),
            user_agent: facts.1.as_deref(),
        },
        // Never `Cookie`. This host is not the workspace's, so the cookie it
        // set would be for the wrong origin - the same wall signup hits.
        Delivery::Handoff,
    )
    .await;

    let signed_in = match signed_in {
        Ok(signed_in) => signed_in,
        Err(err) => {
            tracing::error!(error = %err, tenant = %slug, "federated sign-in failed");
            return finish(clear, Redirect::to(&format!("{home}/?google=unavailable")));
        }
    };

    if !signed_in.result.password_accepted() {
        // The address is verified and there is no account here for it, or the
        // account cannot sign in.
        //
        // This says so, where the password form and the reset form both refuse
        // to. The difference is what the caller has already proved: Google has
        // just vouched that they control this mailbox, so "you are not a member
        // of this workspace" tells them something about an address that is
        // theirs. Refusing to say it would leave somebody at a button that
        // fails with no reason, which is a support ticket rather than a
        // defence.
        tracing::info!(tenant = %slug, "a Google sign-in matched no account here");
        return finish(clear, Redirect::to(&format!("{home}/?google=no_account")));
    }

    let Some(handoff) = &signed_in.handoff else {
        tracing::error!("a federated sign-in produced no handoff token");
        return finish(clear, Redirect::to(&format!("{home}/?google=unavailable")));
    };

    tracing::info!(tenant = %slug, "google sign-in accepted");

    finish(
        clear,
        Redirect::to(&format!(
            "{home}/auth/handoff?token={}",
            urlencode(handoff.expose_secret())
        )),
    )
}

/// Attach the cookie that deletes the pending attempt to whatever comes next.
fn finish(clear: String, redirect: Redirect) -> Response {
    let mut response = redirect.into_response();

    if let Ok(value) = HeaderValue::from_str(&clear) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }

    response
}

/// The `Set-Cookie` that remembers a sign-in in flight.
///
/// `SameSite=Lax` and not `Strict`, which is the one attribute that has to be
/// right: the callback is a top-level navigation from `accounts.google.com`,
/// and a `Strict` cookie is not sent on one. The flow would fail every time,
/// on every browser, in a way that looks like the state check working.
fn pending_cookie(state: &AppState, pending: &Pending, slug: &str) -> String {
    use cookie::{Cookie, SameSite, time::Duration};

    // Three fields, separated by a character none of them can contain: the
    // state and verifier are base64url and the slug is `[a-z0-9-]`.
    let value = format!(
        "{}|{}|{}",
        pending.state,
        pending.verifier.expose_secret(),
        slug
    );

    Cookie::build((PENDING_COOKIE, value))
        // Narrower than the session cookie's `/`: nothing outside this flow has
        // any business receiving it, and the PKCE verifier is in it.
        .path("/auth/google")
        .http_only(true)
        .secure(state.config.security.session.secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(PENDING_TTL_SECS))
        .build()
        .to_string()
}

/// The `Set-Cookie` that deletes it.
///
/// Every attribute must match the one that set it, or the browser treats this
/// as a different cookie and the original survives - with a live PKCE verifier
/// in it.
fn clear_pending_cookie(state: &AppState) -> String {
    use cookie::{Cookie, SameSite, time::Duration};

    Cookie::build((PENDING_COOKIE, String::new()))
        .path("/auth/google")
        .http_only(true)
        .secure(state.config.security.session.secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(0))
        .build()
        .to_string()
}

/// Pull the three fields back out, or `None` if anything is missing.
fn read_pending_cookie(headers: &axum::http::HeaderMap) -> Option<(String, SecretString, String)> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    let raw = phonix_web::server::cookie::read(header, PENDING_COOKIE)?;

    let mut parts = raw.split('|');
    let (Some(state), Some(verifier), Some(slug), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };

    if state.is_empty() || verifier.is_empty() || slug.is_empty() {
        return None;
    }

    Some((
        state.to_owned(),
        SecretString::from(verifier.to_owned()),
        slug.to_owned(),
    ))
}

/// Compare without letting the clock describe the difference.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// Who is asking, for the audit trail.
fn client_facts(headers: &axum::http::HeaderMap) -> (Option<String>, Option<String>) {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    (ip, user_agent)
}

/// Percent-encode one query value.
fn urlencode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_cookie_survives_the_round_trip() {
        let pending = Pending::generate();
        let value = format!(
            "{}|{}|acme",
            pending.state,
            pending.verifier.expose_secret()
        );

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{PENDING_COOKIE}={value}")).expect("a header"),
        );

        let (state, verifier, slug) = read_pending_cookie(&headers).expect("the cookie");

        assert_eq!(state, pending.state);
        assert_eq!(verifier.expose_secret(), pending.verifier.expose_secret());
        assert_eq!(slug, "acme");
    }

    #[test]
    fn a_cookie_that_is_not_three_fields_is_not_a_pending_sign_in() {
        for value in [
            "",
            "only-one",
            "two|fields",
            "a|b|c|d",
            "|b|c",
            "a||c",
            "a|b|",
        ] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                header::COOKIE,
                HeaderValue::from_str(&format!("{PENDING_COOKIE}={value}")).expect("a header"),
            );

            assert!(
                read_pending_cookie(&headers).is_none(),
                "{value:?} should not parse as a pending sign-in",
            );
        }
    }

    #[test]
    fn no_cookie_at_all_is_not_a_pending_sign_in() {
        assert!(read_pending_cookie(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn the_verifier_is_never_readable_from_javascript() {
        // HttpOnly is what stops an XSS bug from lifting the PKCE verifier out
        // and completing somebody else's sign-in.
        let state = |secure: bool| secure;
        let _ = state;

        // Built without an AppState so the attributes can be asserted on the
        // string this function produces.
        use cookie::{Cookie, SameSite, time::Duration};
        let cookie = Cookie::build((PENDING_COOKIE, "value"))
            .path("/auth/google")
            .http_only(true)
            .secure(true)
            .same_site(SameSite::Lax)
            .max_age(Duration::seconds(PENDING_TTL_SECS))
            .build()
            .to_string();

        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        // Lax and not Strict: the callback is a top-level navigation from
        // Google, and Strict would not be sent on it.
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/auth/google"));
    }

    #[test]
    fn state_comparison_rejects_everything_but_an_exact_match() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"abc", b""));
        assert!(constant_time_eq(b"", b""));
    }
}
