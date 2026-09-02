//! The workspace list, one workspace's page, and its licence.
//!
//! # Two questions, kept apart on the screen
//!
//! Whether a workspace is *running* and whether it is *authorized* are two
//! facts, stored separately and shown separately. A lapse is a date passing; a
//! suspension is somebody's decision with their name against it. If the page
//! folded them into one badge, reinstating a workspace would mean guessing
//! which of the two had stopped it. See ADR 0005 section 7.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::Response;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use http::HeaderMap;
use phonix_core::{LicenceState, TenantSlug, TenantStatus};
use phonix_db::tenancy::catalog::TenantRecord;
use phonix_services::desk::workspace::{self, LicenceDecision};
use phonix_services::error::ServiceError;
use serde::Deserialize;

use crate::html::{Chrome, message, render};
use crate::routes::{
    Client, SignedIn, internal_error, not_found, query_value, see_other, urlencode,
};
use crate::state::DeskState;

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

/// One row, already in the words the page uses.
///
/// The template is handed strings rather than a `TenantRecord`: formatting a
/// date and naming a standing are decisions, and a template that makes them is
/// a second place where the answer lives.
pub struct WorkspaceRow {
    pub slug: String,
    pub name: String,
    pub status: String,
    pub licence: String,
    /// Whether the licence half is what stops this workspace. Drives the
    /// emphasis on the row, and is not the same question as `status`.
    pub licence_refuses: bool,
    pub schema_version: String,
    /// Whether the schema is behind the build. Computed here against
    /// `schema_fingerprint()` rather than compared in the template, where the
    /// current value would have to be passed in and could go stale.
    pub outdated: bool,
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
    pub unlicensed: usize,
    pub outdated: usize,
}

pub async fn index(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    uri: axum::http::Uri,
) -> Response {
    let tenants = match workspace::list(&state.catalog).await {
        Ok(tenants) => tenants,
        Err(err) => return internal_error(err, "listing workspaces"),
    };

    let latest = phonix_db::tenancy::schema_fingerprint();

    let serving = tenants.iter().filter(|t| t.serves_traffic()).count();
    // Counted rather than merely listed, because this is the reason a workspace
    // list is worth having at all: until Desk existed, a workspace stuck
    // part-way through provisioning was invisible.
    let stuck = tenants
        .iter()
        .filter(|t| t.status == TenantStatus::Provisioning)
        .count();
    // An active workspace that nothing authorizes. After catalog migration
    // 0005's backfill this can only be one created since, which is exactly the
    // thing somebody has to look at.
    let unlicensed = tenants
        .iter()
        .filter(|t| t.status == TenantStatus::Active && t.licence_problem().is_some())
        .count();
    let outdated = tenants.iter().filter(|t| is_outdated(t, &latest)).count();

    let rows = tenants.iter().map(|t| row_for(t, &latest)).collect();

    render(&WorkspacesPage {
        title: "Workspaces".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "workspaces"),
        banner: query_value(&uri, "refused"),
        total: tenants.len(),
        serving,
        stuck,
        unlicensed,
        outdated,
        rows,
    })
}

fn row_for(tenant: &TenantRecord, latest: &str) -> WorkspaceRow {
    WorkspaceRow {
        slug: tenant.slug.as_str().to_owned(),
        name: tenant.display_name.clone(),
        status: tenant.status.as_str().to_owned(),
        licence: licence_standing(tenant),
        licence_refuses: tenant.licence_problem().is_some(),
        schema_version: tenant.schema_version.as_deref().unwrap_or("-").to_owned(),
        outdated: is_outdated(tenant, latest),
        created: tenant.created_at.format("%Y-%m-%d").to_string(),
    }
}

/// Whether this workspace's database is behind the build.
///
/// A workspace still provisioning has no schema version and is not "outdated";
/// it is unfinished, which is a different problem with a different fix.
fn is_outdated(tenant: &TenantRecord, latest: &str) -> bool {
    tenant.status != TenantStatus::Provisioning && tenant.schema_version.as_deref() != Some(latest)
}

/// The licence in one word, including the word for having none.
fn licence_standing(tenant: &TenantRecord) -> String {
    phonix_core::tenant::licence::standing_of(tenant.licence.as_ref(), Utc::now())
        .as_str()
        .to_owned()
}

// ---------------------------------------------------------------------------
// One workspace
// ---------------------------------------------------------------------------

/// The licence, as the page shows it and as the form starts out.
pub struct LicenceView {
    pub standing: String,
    pub authorizes: bool,
    pub state: String,
    pub valid_from: String,
    pub valid_until: String,
    /// `valid_until` as `YYYY-MM-DD`, or empty. What the date input needs, and
    /// deliberately a second field: the displayed form carries a time and the
    /// input must not.
    pub valid_until_date: String,
    pub note: String,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Template)]
#[template(path = "workspace.html")]
pub struct WorkspacePage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub confirmation: Option<String>,

    pub slug: String,
    pub name: String,
    pub status: String,
    pub serving: bool,
    pub database_name: String,
    pub schema_version: String,
    pub current_schema: String,
    pub outdated: bool,
    pub owner_email: String,
    pub created: String,
    pub onboarded: String,

    /// `None` means the workspace has no licence at all, which is a refusal to
    /// serve and not a blank field.
    pub licence: Option<LicenceView>,
    /// Which radio the form starts on. `trial` when there is nothing yet,
    /// because that is the ordinary first answer.
    pub chosen_state: String,
}

pub async fn show(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    Path(slug): Path<String>,
    uri: axum::http::Uri,
) -> Response {
    let Ok(parsed) = TenantSlug::parse(&slug) else {
        return not_found().await;
    };

    let tenant = match workspace::find(&state.catalog, &parsed).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return not_found().await,
        Err(err) => return internal_error(err, "reading a workspace"),
    };

    let latest = phonix_db::tenancy::schema_fingerprint();
    let licence = tenant.licence.as_ref().map(|licence| LicenceView {
        standing: licence.standing().as_str().to_owned(),
        authorizes: licence.standing().authorizes(),
        state: licence.state.as_str().to_owned(),
        valid_from: stamp(licence.valid_from),
        valid_until: licence
            .valid_until
            .map(stamp)
            .unwrap_or_else(|| "no end date".to_owned()),
        valid_until_date: licence
            .valid_until
            .map(|until| until.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        note: licence.note.clone().unwrap_or_default(),
        updated_by: licence.updated_by.clone().unwrap_or_else(|| "-".to_owned()),
        updated_at: stamp(licence.updated_at),
    });

    render(&WorkspacePage {
        title: tenant.display_name.clone(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "workspaces"),
        banner: query_value(&uri, "refused"),
        confirmation: query_value(&uri, "done"),

        slug: tenant.slug.as_str().to_owned(),
        name: tenant.display_name.clone(),
        status: tenant.status.as_str().to_owned(),
        serving: tenant.serves_traffic(),
        database_name: tenant.database_name.clone(),
        schema_version: tenant
            .schema_version
            .clone()
            .unwrap_or_else(|| "-".to_owned()),
        current_schema: latest.clone(),
        outdated: is_outdated(&tenant, &latest),
        owner_email: tenant.owner_email.clone().unwrap_or_else(|| "-".to_owned()),
        created: stamp(tenant.created_at),
        onboarded: tenant
            .onboarded_at
            .map(stamp)
            .unwrap_or_else(|| "not through signup".to_owned()),

        chosen_state: licence
            .as_ref()
            .map(|view| view.state.clone())
            .unwrap_or_else(|| LicenceState::Trial.as_str().to_owned()),
        licence,
    })
}

// ---------------------------------------------------------------------------
// The licence form
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LicenceForm {
    state: String,
    /// `YYYY-MM-DD` from a native date input, or empty for no end date. Empty
    /// is a decision here, not a missing value - see the hint on the form.
    valid_until: String,
    note: String,
}

pub async fn set_licence(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Form(form): Form<LicenceForm>,
) -> Response {
    let Ok(parsed) = TenantSlug::parse(&slug) else {
        return not_found().await;
    };
    let client = Client::read(&headers, &state);

    let Some(chosen) = LicenceState::parse(&form.state) else {
        // Not a form error to report on a field: the only way here is a hand
        // written request, because the page offers three radios.
        return refused(&slug, "That is not a licence state.");
    };

    let valid_until = match parse_end_date(&form.valid_until) {
        Ok(until) => until,
        Err(()) => {
            return refused(
                &slug,
                &message(&phonix_core::msg!("desk.licence.unreadable_date")),
            );
        }
    };

    let decision = LicenceDecision {
        state: chosen,
        valid_until,
        note: Some(form.note),
    };

    match workspace::set_licence(&state.catalog, &parsed, decision, &caller, client.facts()).await {
        Ok(licence) => {
            let done = match licence.state {
                LicenceState::Revoked => "Licence withdrawn. The workspace has stopped serving.",
                _ => "Licence saved.",
            };
            see_other(&format!("/workspaces/{slug}?done={}", urlencode(done)))
        }
        Err(ServiceError::Rejected(problems)) => {
            let detail = problems
                .first()
                .map(|problem| message(&problem.message))
                .unwrap_or_else(|| "That was refused.".to_owned());
            refused(&slug, &detail)
        }
        Err(ServiceError::Db(phonix_db::DbError::UnknownTenant(_))) => not_found().await,
        Err(err) => internal_error(err, "setting a workspace licence"),
    }
}

fn refused(slug: &str, detail: &str) -> Response {
    see_other(&format!("/workspaces/{slug}?refused={}", urlencode(detail)))
}

/// Read the date input into the instant the licence stops covering.
///
/// Half-open, like every other interval in this codebase: the day typed here is
/// the first one **not** covered, and the form says so. Midnight UTC rather
/// than the operator's midnight - Desk is one screen for a box that serves
/// workspaces in several places, and a licence that ended at a different moment
/// depending on who set it would be unexplainable afterwards.
///
/// An empty string is `Ok(None)`: no end date, which is a deliberate act.
fn parse_end_date(raw: &str) -> Result<Option<DateTime<Utc>>, ()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| ())?;

    Utc.from_local_datetime(&date.and_hms_opt(0, 0, 0).ok_or(())?)
        .single()
        .ok_or(())
        .map(Some)
}

/// One way of writing an instant, everywhere on these pages.
fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_end_date_means_no_end_date() {
        assert_eq!(parse_end_date(""), Ok(None));
        assert_eq!(parse_end_date("   "), Ok(None));
    }

    /// The half-open reading, stated as a test because it is the one thing
    /// about this field somebody could reasonably assume the other way.
    #[test]
    fn the_date_typed_is_the_first_day_not_covered() {
        let end = parse_end_date("2026-12-31").unwrap().unwrap();

        assert_eq!(
            end.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-12-31 00:00:00"
        );
    }

    #[test]
    fn a_date_the_calendar_does_not_have_is_refused() {
        assert!(parse_end_date("2026-02-30").is_err());
        assert!(parse_end_date("31/12/2026").is_err());
        assert!(parse_end_date("tomorrow").is_err());
    }
}
