//! Following a setup link: choosing a password and enrolling an authenticator.
//!
//! One page and one submit. The password and the authenticator are set
//! together, and the account only becomes usable if both succeed - an account
//! that got a password on Tuesday and a second factor on Thursday can sign in
//! on Wednesday with one factor, which is the thing Desk exists to refuse.
//!
//! # There is no QR code, and that is deliberate
//!
//! Drawing one means either a JavaScript library or an image endpoint that
//! renders whatever it is handed. The page shows the secret as text and as an
//! `otpauth://` link instead: every authenticator app accepts a typed secret,
//! the link opens one directly on a phone, and neither needs a script.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::Response;
use http::HeaderMap;
use phonix_services::desk::account::{self, SetupOutcome};
use secrecy::SecretString;
use serde::Deserialize;

use crate::html::{MessagePage, message, render};
use crate::routes::{Client, internal_error, see_other};
use crate::state::DeskState;

#[derive(Template)]
#[template(path = "setup.html")]
pub struct SetupPage {
    title: String,
    banner: Option<String>,
    display_name: String,
    email: String,
    secret_base32: String,
    provisioning_uri: String,
    token: String,
}

#[derive(Deserialize)]
pub struct SetupForm {
    password: String,
    code: String,
}

pub async fn form(
    State(state): State<DeskState>,
    Path(token): Path<String>,
    uri: axum::http::Uri,
) -> Response {
    let secret = SecretString::from(token.clone());

    let page = match account::begin_setup(state.pool(), &state.security(), &secret).await {
        Ok(Some(page)) => page,
        Ok(None) => return unusable(),
        Err(err) => return internal_error(err, "starting desk account setup"),
    };

    let banner = match uri.query() {
        Some(q) if q.contains("wrong_code") => Some(
            "That code did not match. Check the clock on your phone and use the current code."
                .to_owned(),
        ),
        Some(q) if q.contains("weak") => Some(
            "That password was refused. Use a longer one that you do not use elsewhere.".to_owned(),
        ),
        _ => None,
    };

    render(&SetupPage {
        title: "Set up your account".to_owned(),
        banner,
        display_name: page.display_name,
        email: page.email,
        secret_base32: page.secret_base32,
        provisioning_uri: page.provisioning_uri,
        token,
    })
}

pub async fn submit(
    State(state): State<DeskState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Form(form): Form<SetupForm>,
) -> Response {
    let client = Client::read(&headers, &state);
    let secret = SecretString::from(token.clone());
    let password = SecretString::from(form.password);

    let outcome = account::complete_setup(
        state.pool(),
        &state.security(),
        &secret,
        &password,
        &form.code,
        client.facts(),
    )
    .await;

    match outcome {
        Ok(SetupOutcome::Completed) => see_other("/sign-in"),
        // Back to the same page, which issues a fresh secret. That is correct
        // rather than wasteful: nothing was confirmed, so nothing is lost, and
        // somebody who scanned the last one badly needs exactly this.
        Ok(SetupOutcome::WrongCode) => see_other(&format!("/setup/{token}?wrong_code")),
        Ok(SetupOutcome::Invalid(problems)) => {
            // The policy's own sentence, rendered from its key against the
            // built-in English catalog - Desk does not restate password rules
            // in words of its own, because then there would be two.
            let detail = problems
                .first()
                .map(|problem| message(&problem.message))
                .unwrap_or_else(|| "That password was refused.".to_owned());

            tracing::info!(detail, "a desk password was refused by the policy");
            see_other(&format!("/setup/{token}?weak"))
        }
        Ok(SetupOutcome::LinkNotUsable) => unusable(),
        Err(err) => internal_error(err, "finishing desk account setup"),
    }
}

/// One answer for unknown, spent and expired alike.
///
/// Three sentences and no clue which of the three it was. A page that
/// distinguishes a spent link from an unknown one tells whoever is holding it
/// whether it was ever real.
fn unusable() -> Response {
    render(
        &MessagePage::new(
            "That link does not work",
            "It may have expired, or already been used.",
        )
        .extra("Ask whoever created your account to issue another one."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The escaping case a hand-written escaper gets wrong: a value
    /// interpolated into an *attribute* rather than into text. This page has
    /// the only one Desk has - the `otpauth://` link's `href` - and it carries
    /// a display name and an email address, neither of which Desk wrote.
    ///
    /// A single quote is in there because an attribute written with single
    /// quotes is a mistake somebody eventually makes, and an escaper that only
    /// handles the double is then wrong in a file nobody re-reads.
    #[test]
    fn the_authenticator_link_cannot_break_out_of_its_attribute() {
        let page = SetupPage {
            title: "Set up your account".to_owned(),
            banner: None,
            display_name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            secret_base32: "JBSWY3DPEHPK3PXP".to_owned(),
            provisioning_uri: "otpauth://totp/x?issuer=\" onmouseover='alert(1)".to_owned(),
            token: "tok".to_owned(),
        };

        let rendered = page.render().expect("the page renders");

        assert!(!rendered.contains("onmouseover='alert(1)"));
        assert!(rendered.contains("&#34;"));
        assert!(rendered.contains("&#39;"));
    }
}
