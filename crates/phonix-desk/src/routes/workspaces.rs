//! The home page: the workspaces this deployment holds.
//!
//! Step 3 of the build order fills this in properly - schema version against
//! `schema_fingerprint()`, a detail page, dependency health. What is here now
//! is the honest half: the list the catalog can already answer, and a plain
//! statement of what Desk cannot do yet.
//!
//! Saying so on the page rather than leaving a convincing-looking screen is the
//! point. A console that shows a workspace and offers no way to act on it is
//! only misleading if it does not admit that is where the build stopped.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use phonix_core::TenantStatus;

use crate::html::{Chrome, render};
use crate::routes::{SignedIn, internal_error};
use crate::state::DeskState;

/// One row, already in the words the page uses.
///
/// The template is handed strings rather than a `Tenant`: formatting a date and
/// naming a status are decisions, and a template that makes them is a second
/// place where the answer lives.
pub struct WorkspaceRow {
    pub slug: String,
    pub name: String,
    pub status: String,
    pub schema_version: String,
    pub created: String,
}

#[derive(Template)]
#[template(path = "workspaces.html")]
pub struct WorkspacesPage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub rows: Vec<WorkspaceRow>,
    pub total: usize,
    pub serving: usize,
    pub stuck: usize,
}

pub async fn index(SignedIn(caller): SignedIn, State(state): State<DeskState>) -> Response {
    let tenants = match state.catalog.list().await {
        Ok(tenants) => tenants,
        Err(err) => return internal_error(err, "listing workspaces"),
    };

    let serving = tenants
        .iter()
        .filter(|tenant| tenant.status.serves_traffic())
        .count();
    // Counted rather than merely listed, because this is the reason a workspace
    // list is worth having at all: until Desk existed, a workspace stuck
    // part-way through provisioning was invisible.
    let stuck = tenants
        .iter()
        .filter(|tenant| tenant.status == TenantStatus::Provisioning)
        .count();

    let rows = tenants
        .iter()
        .map(|tenant| WorkspaceRow {
            slug: tenant.slug.as_str().to_owned(),
            name: tenant.display_name.clone(),
            status: tenant.status.as_str().to_owned(),
            schema_version: tenant.schema_version.as_deref().unwrap_or("-").to_owned(),
            created: tenant.created_at.format("%Y-%m-%d").to_string(),
        })
        .collect::<Vec<_>>();

    render(&WorkspacesPage {
        title: "Workspaces".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "workspaces"),
        banner: None,
        total: tenants.len(),
        serving,
        stuck,
        rows,
    })
}
