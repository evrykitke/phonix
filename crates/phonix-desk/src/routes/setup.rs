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

use axum::Form;
use axum::extract::{Path, State};
use axum::response::Response;
use http::HeaderMap;
use phonix_services::desk::account::{self, SetupOutcome};
use secrecy::SecretString;
use serde::Deserialize;

use crate::html::{Page, esc, message, notice};
use crate::routes::{Client, html_response, internal_error, see_other};
use crate::state::DeskState;

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
        Some(q) if q.contains("wrong_code") => notice(
            "bad",
            "That code did not match. Check the clock on your phone and use the current code.",
        ),
        Some(q) if q.contains("weak") => notice(
            "bad",
            "That password was refused. Use a longer one that you do not use elsewhere.",
        ),
        _ => String::new(),
    };

    let body = format!(
        r#"<div class="panel">
  <h1>Set up your Desk account</h1>
  <p class="lede">{name}, this link sets the password for {email}.</p>
  {banner}
  <p>Add this secret to your authenticator app, then enter the code it shows.
     TOTP is not optional here.</p>
  <code class="secret">{secret}</code>
  <p class="hint"><a href="{uri}">Open in an authenticator app</a> if you are on the phone
     that holds it.</p>
  <form method="post" action="/setup/{token}">
    <div class="field">
      <label for="password">Choose a password</label>
      <input id="password" name="password" type="password" autocomplete="new-password" required>
      <p class="hint">Only you will know it. Nobody at Desk can set or read it.</p>
    </div>
    <div class="field">
      <label for="code">Code from your authenticator</label>
      <input id="code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
             pattern="[0-9 ]*" required>
    </div>
    <button type="submit">Finish setup</button>
  </form>
</div>"#,
        name = esc(&page.display_name),
        email = esc(&page.email),
        banner = banner,
        secret = esc(&page.secret_base32),
        uri = esc(&page.provisioning_uri),
        token = esc(&token),
    );

    html_response(Page::new("Set up your account", body).render())
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
fn unusable() -> Response {
    let body = r#"<div class="panel">
  <h1>That link does not work</h1>
  <p class="lede">It may have expired, or already been used.</p>
  <p>Ask whoever created your account to issue another one.</p>
</div>"#;

    html_response(Page::new("That link does not work", body).render())
}
