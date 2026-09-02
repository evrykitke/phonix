//! Desk accounts: who may sign in, and creating the next one.
//!
//! There are no roles. Everybody who can reach this page can do everything Desk
//! does, which is right for a small internal team and is written down as a
//! decision with a trigger for reversing it - see ADR 0005 section 4. The audit
//! trail is what distinguishes people until then.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::Response;
use http::HeaderMap;
use phonix_db::desk::DeskUserStatus;
use phonix_services::desk::account;
use phonix_services::error::ServiceError;
use serde::Deserialize;
use uuid::Uuid;

use crate::html::{Chrome, message, render};
use crate::routes::{Client, SignedIn, internal_error, query_value, see_other, urlencode};
use crate::state::DeskState;

/// One account, already in the words the page uses.
pub struct AccountRow {
    pub id: Uuid,
    pub display_name: String,
    pub email: String,
    pub status: String,
    pub last_signed_in: String,
    /// Which of the two buttons the row offers. Decided here rather than in the
    /// template, because it is a fact about the account and not about the
    /// markup.
    pub disabled: bool,
    /// Not a permission check - the service refuses the last usable account
    /// regardless. This is only the button being honest about the ordinary
    /// case.
    pub is_me: bool,
}

#[derive(Template)]
#[template(path = "accounts.html")]
pub struct AccountsPage {
    title: String,
    chrome: Chrome,
    banner: Option<String>,
    /// A freshly minted setup link, shown once. It is not stored anywhere it
    /// could be read again, which is the rule an issued API key follows too.
    setup_link: Option<String>,
    rows: Vec<AccountRow>,
}

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

    let rows = accounts
        .iter()
        .map(|user| AccountRow {
            id: user.id,
            display_name: user.display_name.clone(),
            email: user.email.clone(),
            status: user.status.as_str().to_owned(),
            last_signed_in: user
                .last_signed_in_at
                .map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "never".to_owned()),
            disabled: user.status == DeskUserStatus::Disabled,
            is_me: user.id == caller.user.id,
        })
        .collect::<Vec<_>>();

    render(&AccountsPage {
        title: "Desk accounts".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "accounts"),
        setup_link: query_value(&uri, "link"),
        banner: query_value(&uri, "refused"),
        rows,
    })
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
