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
    pub confirmation: Option<String>,
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
        confirmation: query_value(&uri, "done"),
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

    /// Which actions this workspace is in a state to accept.
    ///
    /// Decided here rather than in the template, because "may I retry this"
    /// is a fact about the workspace and the template would have to restate
    /// the status vocabulary to work it out. A hidden button is cosmetic
    /// either way - the service refuses regardless, which is the rule the
    /// product's grid already follows for permission gating.
    pub can_retry: bool,
    pub can_migrate: bool,
    pub can_suspend: bool,
    pub can_resume: bool,
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

        can_retry: Act::Retry.applies_to(&tenant, &latest),
        can_migrate: Act::Migrate.applies_to(&tenant, &latest),
        can_suspend: Act::Suspend.applies_to(&tenant, &latest),
        can_resume: Act::Resume.applies_to(&tenant, &latest),
    })
}

// ---------------------------------------------------------------------------
// The three safe writes
// ---------------------------------------------------------------------------

/// What Desk can do to one workspace.
///
/// An enum with the words on it rather than four pairs of near-identical
/// handlers: the confirm page and the POST differ only in which of these they
/// are given, and a sentence written twice is a sentence that ends up saying
/// two things.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Retry,
    Migrate,
    Suspend,
    Resume,
}

impl Act {
    fn path(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Migrate => "migrate",
            Self::Suspend => "suspend",
            Self::Resume => "resume",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Retry => "Retry this provisioning?",
            Self::Migrate => "Migrate this workspace?",
            Self::Suspend => "Suspend this workspace?",
            Self::Resume => "Resume this workspace?",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Retry => {
                "This workspace stopped part-way through being created. Retrying finishes the \
                 steps that did not complete: the database, its migrations, and the row going \
                 active."
            }
            Self::Migrate => {
                "This workspace's database is on an older schema than this build. Migrating \
                 brings it forward, app by app, in the same pass the server runs on boot."
            }
            Self::Suspend => {
                "The workspace stops serving traffic immediately. Everyone signed in to it is \
                 refused on their next request."
            }
            Self::Resume => {
                "The workspace starts serving traffic again, if its licence is current."
            }
        }
    }

    /// The things somebody would want to have known afterwards.
    fn consequences(self) -> Vec<&'static str> {
        match self {
            Self::Retry => vec![
                "Every step is skip-if-present, so this is safe to run more than once.",
                "No owner account is created and no licence is issued. Repairing a workspace \
                 is not the moment to decide what it is authorized for.",
            ],
            Self::Migrate => vec![
                "Forward-only. There is no migration that goes back.",
                "The status is not touched, so a suspended workspace stays suspended.",
                "It runs while you wait, and a large database can take a while.",
            ],
            Self::Suspend => vec![
                "Requests answer 403, which is distinguishable from the 404 an unknown \
                 address gets - so this is tellable apart from a DNS mistake.",
                "Its background work stops too: no upload verification, no outbox relay.",
                "The database is untouched and nothing is deleted. Resuming puts it back.",
                "The licence is not changed. This is a decision with your name on it, which \
                 is a different fact from a licence running out.",
            ],
            Self::Resume => vec![
                "If the licence has lapsed or been withdrawn, the workspace stays refused - \
                 that is a second thing to fix, and this page says which.",
            ],
        }
    }

    fn button(self) -> &'static str {
        match self {
            Self::Retry => "Retry provisioning",
            Self::Migrate => "Migrate now",
            Self::Suspend => "Suspend the workspace",
            Self::Resume => "Resume the workspace",
        }
    }

    /// Only one of these stops a customer.
    fn is_dangerous(self) -> bool {
        self == Self::Suspend
    }

    /// Whether this workspace is in a state to accept the action.
    fn applies_to(self, tenant: &TenantRecord, latest: &str) -> bool {
        match self {
            Self::Retry => tenant.status == TenantStatus::Provisioning,
            Self::Migrate => is_outdated(tenant, latest),
            Self::Suspend => tenant.status == TenantStatus::Active,
            Self::Resume => tenant.status == TenantStatus::Suspended,
        }
    }

    fn done(self) -> &'static str {
        match self {
            Self::Retry => "Provisioning finished. Check that it has a licence.",
            Self::Migrate => "Migrated.",
            Self::Suspend => "Suspended. The workspace has stopped serving.",
            Self::Resume => "Resumed.",
        }
    }
}

#[derive(Template)]
#[template(path = "confirm.html")]
pub struct ConfirmPage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub heading: String,
    pub detail: String,
    pub consequences: Vec<&'static str>,
    pub action: String,
    pub button: String,
    pub danger: bool,
    pub back: String,
}

pub async fn confirm_retry(a: SignedIn, s: State<DeskState>, p: Path<String>) -> Response {
    confirm(Act::Retry, a, s, p).await
}
pub async fn confirm_migrate(a: SignedIn, s: State<DeskState>, p: Path<String>) -> Response {
    confirm(Act::Migrate, a, s, p).await
}
pub async fn confirm_suspend(a: SignedIn, s: State<DeskState>, p: Path<String>) -> Response {
    confirm(Act::Suspend, a, s, p).await
}
pub async fn confirm_resume(a: SignedIn, s: State<DeskState>, p: Path<String>) -> Response {
    confirm(Act::Resume, a, s, p).await
}

pub async fn do_retry(a: SignedIn, s: State<DeskState>, p: Path<String>, h: HeaderMap) -> Response {
    perform(Act::Retry, a, s, p, h).await
}
pub async fn do_migrate(
    a: SignedIn,
    s: State<DeskState>,
    p: Path<String>,
    h: HeaderMap,
) -> Response {
    perform(Act::Migrate, a, s, p, h).await
}
pub async fn do_suspend(
    a: SignedIn,
    s: State<DeskState>,
    p: Path<String>,
    h: HeaderMap,
) -> Response {
    perform(Act::Suspend, a, s, p, h).await
}
pub async fn do_resume(
    a: SignedIn,
    s: State<DeskState>,
    p: Path<String>,
    h: HeaderMap,
) -> Response {
    perform(Act::Resume, a, s, p, h).await
}

/// The page in front of an action. A `GET`, and it changes nothing.
async fn confirm(
    act: Act,
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    Path(slug): Path<String>,
) -> Response {
    let Ok(parsed) = TenantSlug::parse(&slug) else {
        return not_found().await;
    };

    let tenant = match workspace::find(&state.catalog, &parsed).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return not_found().await,
        Err(err) => return internal_error(err, "reading a workspace"),
    };

    // A confirm page for an action the workspace cannot accept is a page whose
    // button would only produce a refusal. The service refuses regardless; this
    // is so a stale link or a typed address does not read as an offer.
    if !act.applies_to(&tenant, &phonix_db::tenancy::schema_fingerprint()) {
        return not_found().await;
    }

    render(&ConfirmPage {
        title: tenant.display_name.clone(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "workspaces"),
        banner: None,
        heading: act.heading().to_owned(),
        detail: act.detail().to_owned(),
        consequences: act.consequences(),
        action: format!("/workspaces/{slug}/{}", act.path()),
        button: act.button().to_owned(),
        danger: act.is_dangerous(),
        back: format!("/workspaces/{slug}"),
    })
}

async fn perform(
    act: Act,
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Ok(parsed) = TenantSlug::parse(&slug) else {
        return not_found().await;
    };
    let client = Client::read(&headers, &state);
    let database = &state.config.database;

    let outcome = match act {
        Act::Retry => workspace::retry_provisioning(
            &state.catalog,
            database,
            &parsed,
            &caller,
            client.facts(),
        )
        .await
        .map(|_| ()),
        Act::Migrate => {
            workspace::migrate_one(&state.catalog, database, &parsed, &caller, client.facts())
                .await
                .map(|_| ())
        }
        Act::Suspend => workspace::set_status(
            &state.catalog,
            &parsed,
            TenantStatus::Suspended,
            &caller,
            client.facts(),
        )
        .await
        .map(|_| ()),
        Act::Resume => workspace::set_status(
            &state.catalog,
            &parsed,
            TenantStatus::Active,
            &caller,
            client.facts(),
        )
        .await
        .map(|_| ()),
    };

    match outcome {
        Ok(()) => see_other(&format!(
            "/workspaces/{slug}?done={}",
            urlencode(act.done())
        )),
        Err(ServiceError::Rejected(problems)) => {
            let detail = problems
                .first()
                .map(|problem| message(&problem.message))
                .unwrap_or_else(|| "That was refused.".to_owned());
            refused(&slug, &detail)
        }
        Err(ServiceError::Db(phonix_db::DbError::UnknownTenant(_))) => not_found().await,
        // A migration or a repair that broke has already written a `failed`
        // audit row. The person is told where to look rather than shown a
        // Postgres sentence they cannot act on.
        Err(err) => internal_error(err, act.path()),
    }
}

// ---------------------------------------------------------------------------
// The whole estate
// ---------------------------------------------------------------------------

/// Not `/workspaces/migrate-outdated`.
///
/// That address is also a valid workspace slug, and a router that resolves it
/// as a static segment would make a workspace named that unreachable - a trap
/// nobody would find until somebody hit it. `estate` is the word the ADR uses
/// for the whole box, and it cannot collide.
pub async fn confirm_estate_migrate(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
) -> Response {
    let latest = phonix_db::tenancy::schema_fingerprint();

    let behind = match workspace::list(&state.catalog).await {
        Ok(tenants) => tenants.iter().filter(|t| is_outdated(t, &latest)).count(),
        Err(err) => return internal_error(err, "listing workspaces"),
    };

    render(&ConfirmPage {
        title: "Migrate outdated workspaces".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "workspaces"),
        banner: None,
        heading: format!("Migrate {behind} outdated workspace(s)?"),
        detail: format!(
            "Every workspace whose schema is not {latest} is brought forward, one at a time,              in the same pass the server runs on boot."
        ),
        consequences: vec![
            "Forward-only. There is no migration that goes back.",
            "One workspace failing does not stop the rest - refusing to continue would take              out every other one with it. The failures are named afterwards.",
            "No status is changed, so a suspended workspace stays suspended.",
            "It runs while you wait, and this is the whole estate.",
        ],
        action: "/estate/migrate".to_owned(),
        button: "Migrate them".to_owned(),
        danger: false,
        back: "/".to_owned(),
    })
}

pub async fn do_estate_migrate(
    SignedIn(caller): SignedIn,
    State(state): State<DeskState>,
    headers: HeaderMap,
) -> Response {
    let client = Client::read(&headers, &state);

    match workspace::migrate_outdated(
        &state.catalog,
        &state.config.database,
        &caller,
        client.facts(),
    )
    .await
    {
        Ok(sweep) if sweep.failed.is_empty() => see_other(&format!(
            "/?done={}",
            urlencode(&format!(
                "Migrated {}. {} were already current.",
                sweep.migrated, sweep.current
            ))
        )),
        // A partial sweep is reported as a refusal rather than as a success
        // with a footnote: some workspaces are still behind, and that is the
        // thing to act on.
        Ok(sweep) => see_other(&format!(
            "/?refused={}",
            urlencode(&format!(
                "Migrated {}, and {} failed: {}. The detail is in the log.",
                sweep.migrated,
                sweep.failed.len(),
                sweep.failed.join(", ")
            ))
        )),
        Err(err) => internal_error(err, "migrating outdated workspaces"),
    }
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
