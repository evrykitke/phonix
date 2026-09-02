//! Signing in, the code box, and signing out.
//!
//! Two pages, always in that order. The password page opens a session that can
//! reach exactly one other page; the code page turns it into a sign-in. Nothing
//! here can produce a session that skipped the second step, because
//! `desk::auth` has no way to ask for one.

use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::response::Response;
use http::HeaderMap;
use phonix_services::desk::auth::{self, ChallengeOutcome, SignInOutcome};
use secrecy::SecretString;
use serde::Deserialize;

use crate::html::render;
use crate::routes::{Client, HalfSignedIn, internal_error, see_other, see_other_with_cookie};
use crate::state::DeskState;

#[derive(Template)]
#[template(path = "sign_in.html")]
pub struct SignInPage {
    title: String,
    banner: Option<String>,
}

#[derive(Template)]
#[template(path = "mfa.html")]
pub struct MfaPage {
    title: String,
    banner: Option<String>,
    who: String,
}

#[derive(Deserialize)]
pub struct SignInForm {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct CodeForm {
    code: String,
}

/// The password page.
///
/// `?refused` rather than a flash message in a cookie or a session: this page
/// is reached before there is anywhere to keep one, and a query parameter that
/// only ever says "that did not work" leaks nothing a person at the keyboard
/// does not already know.
pub async fn sign_in_form(uri: axum::http::Uri) -> Response {
    let refused = uri.query().is_some_and(|q| q.contains("refused"));

    render(&SignInPage {
        title: "Sign in".to_owned(),
        // One sentence for every refusal: wrong password, unknown address,
        // locked, disabled, never set up. The audit trail says which; the page
        // does not, because a page that distinguishes them is a page that
        // confirms an address exists.
        banner: refused.then(|| String::from("That did not work. Check the address and password.")),
    })
}

pub async fn sign_in(
    State(state): State<DeskState>,
    headers: HeaderMap,
    Form(form): Form<SignInForm>,
) -> Response {
    let client = Client::read(&headers, &state);
    let password = SecretString::from(form.password);

    let outcome = auth::sign_in(
        state.pool(),
        &state.security(),
        state.desk(),
        &form.email,
        &password,
        client.facts(),
    )
    .await;

    match outcome {
        Ok(SignInOutcome::CodeRequired { token, .. }) => see_other_with_cookie(
            "/mfa",
            crate::cookie::set(
                &token,
                // The cookie's lifetime is the session's ceiling. The idle
                // deadline is the database's business and moves on every
                // request; a cookie cannot follow it and should not try.
                state.desk().session_absolute_hours as i64 * 3600,
                state.config.security.session.secure,
            ),
        ),
        Ok(SignInOutcome::Rejected) => see_other("/sign-in?refused"),
        Err(err) => internal_error(err, "signing in"),
    }
}

/// The code box.
///
/// Takes [`HalfSignedIn`], the one guard that accepts a session which has not
/// cleared the second factor - this is the page it exists for.
pub async fn code_form(HalfSignedIn(caller): HalfSignedIn, uri: axum::http::Uri) -> Response {
    if caller.is_signed_in() {
        return see_other("/");
    }

    let refused = uri.query().is_some_and(|q| q.contains("refused"));

    render(&MfaPage {
        title: "Enter your code".to_owned(),
        banner: refused.then(|| String::from("That code was not right. Try the current one.")),
        who: caller.user.display_name.clone(),
    })
}

pub async fn answer_code(
    State(state): State<DeskState>,
    headers: HeaderMap,
    Form(form): Form<CodeForm>,
) -> Response {
    let client = Client::read(&headers, &state);

    let Some(token) = headers
        .get(http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::cookie::read)
    else {
        return see_other("/sign-in");
    };

    let outcome = auth::answer_challenge(
        state.pool(),
        &state.security(),
        state.desk(),
        &token,
        &form.code,
        client.facts(),
    )
    .await;

    match outcome {
        Ok(ChallengeOutcome::Accepted) => see_other("/"),
        Ok(ChallengeOutcome::Rejected) => see_other("/mfa?refused"),
        // The session is gone, so the cookie has to go with it - otherwise the
        // next page is a redirect loop between a guard that finds no session
        // and a browser that keeps presenting one.
        Ok(ChallengeOutcome::Abandoned) => see_other_with_cookie(
            "/sign-in?refused",
            crate::cookie::clear(state.config.security.session.secure),
        ),
        Err(err) => internal_error(err, "answering the code"),
    }
}

/// Sign out.
///
/// A `POST` because it changes something. A `GET` would let any page anywhere
/// sign somebody out by embedding an image.
pub async fn sign_out(State(state): State<DeskState>, headers: HeaderMap) -> Response {
    let client = Client::read(&headers, &state);

    if let Some(token) = headers
        .get(http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::cookie::read)
    {
        // Resolved here rather than taken as a guard: signing out must work
        // from a session that is half-authenticated, expired, or already
        // revoked. A guard would redirect those to the sign-in page with the
        // cookie still set, which is a sign-out button that leaves you signed
        // in.
        let actor = auth::authenticate(state.pool(), state.desk(), &token)
            .await
            .unwrap_or_default();

        if let Err(err) = auth::sign_out(state.pool(), &token, actor.as_ref(), client.facts()).await
        {
            // Not fatal: the cookie is cleared regardless, so the browser stops
            // presenting a token even if the row could not be marked.
            tracing::error!(error = %err, "could not revoke a desk session");
        }
    }

    see_other_with_cookie(
        "/sign-in",
        crate::cookie::clear(state.config.security.session.secure),
    )
}
