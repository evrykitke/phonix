//! The audit trail's own screen.
//!
//! Desk's read of `catalog.desk_audit` is the only place in this product that
//! shows it, and that placement is the point: "who suspended this workspace"
//! must not be a row that workspace's own administrators can read, edit, or
//! lose when the database is archived. See ADR 0005 section 8.
//!
//! Opening this page writes nothing. There is no "read audit" event - a list
//! of what happened is not something that happened.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use phonix_db::desk::audit::Outcome;
use phonix_services::desk::trail;

use crate::html::{Chrome, render};
use crate::routes::{SignedIn, internal_error, query_value};
use crate::state::DeskState;

/// One row, already in the words the page uses.
pub struct EntryRow {
    pub occurred_at: String,
    pub action: String,
    pub actor: String,
    pub workspace: String,
    pub outcome: String,
    /// Whether the outcome is worth colouring. `refused` and `failed` are; the
    /// ordinary case is not, and a trail where every row is coloured is a trail
    /// where nothing stands out.
    pub went_wrong: bool,
    pub detail: String,
    pub ip: String,
    pub changes: Vec<ChangeRow>,
}

pub struct ChangeRow {
    pub field: String,
    pub before: String,
    pub after: String,
}

#[derive(Template)]
#[template(path = "trail.html")]
pub struct TrailPage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub rows: Vec<EntryRow>,
    pub total: i64,
    /// One-based on the page, zero-based in the query string. The reader counts
    /// from one and the arithmetic counts from zero, and putting the conversion
    /// here keeps it out of the template.
    pub showing: i64,
    pub pages: i64,
    pub previous: Option<i64>,
    pub next: Option<i64>,
}

pub async fn index(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    uri: axum::http::Uri,
) -> Response {
    let wanted = query_value(&uri, "page")
        .and_then(|raw| raw.parse::<i64>().ok())
        // The query string counts from zero; nobody types that, so the address
        // bar counts from one and this is where the two meet.
        .map(|page| page - 1)
        .unwrap_or(0)
        .max(0);

    let page = match trail::page(&state.catalog, wanted).await {
        Ok(page) => page,
        Err(err) => return internal_error(err, "reading the audit trail"),
    };

    render(&TrailPage {
        title: "Audit trail".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "audit"),
        banner: None,
        rows: page.entries.iter().map(row).collect(),
        total: page.total,
        showing: page.page + 1,
        pages: page.pages,
        previous: (page.page > 0).then_some(page.page),
        next: (page.page + 1 < page.pages).then_some(page.page + 2),
    })
}

/// Shared with the workspace page, which shows the same rows filtered to one
/// slug - so an entry cannot come to read differently in the two places.
pub fn row(entry: &trail::Entry) -> EntryRow {
    EntryRow {
        occurred_at: entry
            .occurred_at
            .format("%Y-%m-%d %H:%M:%S UTC")
            .to_string(),
        action: entry.action.clone(),
        actor: entry.actor.clone(),
        workspace: entry.tenant_slug.clone().unwrap_or_else(|| "-".to_owned()),
        outcome: entry.outcome.as_str().to_owned(),
        went_wrong: entry.outcome != Outcome::Ok,
        detail: entry.detail.clone().unwrap_or_default(),
        ip: entry.ip.clone().unwrap_or_default(),
        changes: entry
            .changes
            .iter()
            .map(|change| ChangeRow {
                field: change.field.clone(),
                // "not set" rather than an empty cell: a licence that had no
                // end date and now has one is a change *from* something, and a
                // blank reads as a rendering fault.
                before: change
                    .before
                    .clone()
                    .unwrap_or_else(|| "not set".to_owned()),
                after: change.after.clone().unwrap_or_else(|| "not set".to_owned()),
            })
            .collect(),
    }
}
