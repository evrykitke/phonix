//! Desk accounts: who may sign in, and creating the next one.
//!
//! There are no roles. Everybody who can reach this page can do everything Desk
//! does, which is right for a small internal team and is written down as a
//! decision with a trigger for reversing it - see ADR 0005 section 4. The audit
//! trail is what distinguishes people until then.

use axum::Form;
use axum::extract::{Path, State};
use axum::response::Response;
use http::HeaderMap;
use phonix_services::desk::account;
use phonix_services::error::ServiceError;
use serde::Deserialize;
use uuid::Uuid;

use crate::html::{Page, esc, message, notice};
use crate::routes::{Client, SignedIn, html_response, internal_error, see_other};
use crate::state::DeskState;

#[derive(Deserialize)]
pub struct CreateForm {
    email: String,
    display_name: String,
}

#[derive(Deserialize)]
pub struct DisabledForm {
    disabled: String,
}

pub async fn index(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    uri: axum::http::Uri,
) -> Response {
    let accounts = match account::list(state.pool()).await {
        Ok(accounts) => accounts,
        Err(err) => return internal_error(err, "listing desk accounts"),
    };

    // A freshly minted setup link arrives back as a query parameter and is
    // shown once. It is not stored anywhere it could be read again, which is
    // the same rule an issued API key follows.
    let banner = match query_value(&uri, "link") {
        Some(link) => format!(
            r#"<div class="notice good">
  <p><strong>Account created.</strong> Send this link to them out of band. It is
     shown once and cannot be recovered.</p>
  <code class="secret">{link}</code>
</div>"#,
            link = esc(&link)
        ),
        None => match query_value(&uri, "refused") {
            Some(reason) => notice("bad", &reason),
            None => String::new(),
        },
    };

    let rows = accounts
        .iter()
        .map(|user| {
            let is_me = user.id == caller.user.id;
            let action = if user.status == phonix_db::desk::DeskUserStatus::Disabled {
                format!(
                    r#"<form method="post" action="/accounts/{id}/disabled">
                         <input type="hidden" name="disabled" value="false">
                         <button class="quiet">Reinstate</button>
                       </form>"#,
                    id = user.id
                )
            } else if is_me {
                // Not a permission check - the service refuses the last usable
                // account regardless. This is only the button being honest
                // about the ordinary case.
                r#"<span class="hint">This is you</span>"#.to_owned()
            } else {
                format!(
                    r#"<form method="post" action="/accounts/{id}/disabled">
                         <input type="hidden" name="disabled" value="true">
                         <button class="quiet">Disable</button>
                       </form>"#,
                    id = user.id
                )
            };

            format!(
                r#"<tr>
  <td>{name}</td>
  <td class="mono">{email}</td>
  <td><span class="pill">{status}</span></td>
  <td>{seen}</td>
  <td>{action}</td>
</tr>"#,
                name = esc(&user.display_name),
                email = esc(&user.email),
                status = esc(user.status.as_str()),
                seen = user
                    .last_signed_in_at
                    .map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "never".to_owned()),
                action = action,
            )
        })
        .collect::<String>();

    let body = format!(
        r#"{banner}
<div class="panel">
  <h1>Desk accounts</h1>
  <p class="lede">Everyone who can run the platform. There are no roles: each of
     these can do everything Desk does.</p>
  <table>
    <thead><tr><th>Name</th><th>Email</th><th>Status</th><th>Last signed in</th><th></th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</div>
<div class="panel">
  <h2>Add someone</h2>
  <p class="lede">They choose their own password and enrol an authenticator through
     a single-use link. Nobody here sets a password for somebody else.</p>
  <form method="post" action="/accounts">
    <div class="field">
      <label for="display_name">Name</label>
      <input id="display_name" name="display_name" type="text" required>
    </div>
    <div class="field">
      <label for="email">Email</label>
      <input id="email" name="email" type="email" required>
    </div>
    <button type="submit">Create account</button>
  </form>
</div>"#
    );

    html_response(
        Page::new("Desk accounts", body)
            .signed_in_as(&caller.user.display_name)
            .environment(state.environment())
            .render(),
    )
}

pub async fn create(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    let client = Client::read(&headers, &state);

    let created = account::create(
        state.pool(),
        state.desk(),
        &form.email,
        &form.display_name,
        Some(&caller),
        client.facts(),
    )
    .await;

    match created {
        Ok(created) => {
            use secrecy::ExposeSecret;
            // The link is built from the host the request arrived on, so it is
            // right whether Desk is reached through nginx or a tunnel, without
            // a second place to configure its own address.
            let host = headers
                .get(http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("localhost");
            let scheme = if state.config.security.session.secure {
                "https"
            } else {
                "http"
            };
            let link = format!(
                "{scheme}://{host}/setup/{}",
                created.setup_token.expose_secret()
            );

            see_other(&format!("/accounts?link={}", urlencode(&link)))
        }
        Err(ServiceError::Rejected(problems)) => {
            let detail = problems
                .first()
                .map(|problem| message(&problem.message))
                .unwrap_or_else(|| "That could not be created.".to_owned());
            see_other(&format!("/accounts?refused={}", urlencode(&detail)))
        }
        Err(ServiceError::Db(phonix_db::DbError::UserExists(email))) => see_other(&format!(
            "/accounts?refused={}",
            urlencode(&format!("{email} already has a Desk account."))
        )),
        Err(err) => internal_error(err, "creating a desk account"),
    }
}

pub async fn set_disabled(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Form(form): Form<DisabledForm>,
) -> Response {
    let client = Client::read(&headers, &state);
    let disabled = form.disabled == "true";

    match account::set_disabled(state.pool(), id, disabled, &caller, client.facts()).await {
        Ok(()) => see_other("/accounts"),
        Err(ServiceError::Rejected(problems)) => {
            let detail = problems
                .first()
                .map(|problem| message(&problem.message))
                .unwrap_or_else(|| "That is not allowed.".to_owned());
            see_other(&format!("/accounts?refused={}", urlencode(&detail)))
        }
        Err(ServiceError::NotFound(_)) => see_other(&format!(
            "/accounts?refused={}",
            urlencode("That account no longer exists.")
        )),
        Err(err) => internal_error(err, "changing a desk account"),
    }
}

/// Percent-encode a value for a query string.
///
/// Small and local rather than a dependency: Desk puts exactly two things in a
/// query string, and both are its own text.
fn urlencode(raw: &str) -> String {
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
fn query_value(uri: &axum::http::Uri, name: &str) -> Option<String> {
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
