//! `/api/v1/apps` - what this release contains, and what this workspace has
//! switched on.
//!
//! # Installing does not install anything
//!
//! Every app compiled into the build has already had its migrations run in
//! every tenant database, on boot. Switching one on writes a timestamp and
//! re-syncs the static roles, and it is that second half that makes the app
//! appear: menus, grids and every `Caller::require` already answer to
//! permissions, so granting the subtree beneath the app's root turns all of
//! them on at once. Which is also why an app owns a whole permission subtree,
//! and why a key scoped to that subtree is a key scoped to that app.
//!
//! # A refusal to uninstall is a 409, not a 200
//!
//! The service answers `AlwaysOn` and `NeededBy` as *values* rather than
//! errors, and that is right where it is: a screen renders both beside the
//! button, and neither is a fault. It is wrong on a wire. A script that reads
//! `200` and moves on has been told the app is off when it is still on, and it
//! will find out at some later point that is harder to debug. So the two
//! refusals become `409` with a code naming which one, and `SwitchedOff` -
//! including "it was already off", which is a true statement about the end
//! state - is the `200`.

use axum::Json;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use phonix_core::apps::{self, AppDescriptor, AppState, UninstallOutcome};
use phonix_core::query::{Page, PageRequest};
use phonix_services::ServiceError;
use phonix_services::workspace::apps as service;
use serde::Serialize;
use utoipa::ToSchema;

use super::auth::ApiCaller;
use super::paging::{ListParams, ListRequest, PageEnvelope, cut};
use super::path::ApiPath;
use super::problem::Problem;

/// One app in this release, and what this workspace has done about it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = App)]
pub struct AppResource {
    /// Stable, lowercase, and the string every other endpoint here takes.
    /// Never renamed: it is a primary key in every tenant database and the
    /// name of the schema holding that app's data.
    #[schema(example = "books")]
    pub id: String,
    /// The **message key** for the app's name, not the name. Resolved by
    /// whatever is rendering, because the words differ per language and this
    /// surface has no language. Stable, so a client with its own catalog can
    /// key off it.
    #[schema(example = "app.books.name")]
    pub name_key: String,
    /// The message key for the one line under the name.
    #[schema(example = "app.books.summary")]
    pub summary_key: String,
    /// A Lucide icon name, kebab-cased.
    #[schema(example = "file-text")]
    pub icon: String,
    /// The app's own version in this build - not its schema version, and not
    /// necessarily [`Self::installed_version`].
    #[schema(example = "1.0.0")]
    pub version: String,
    /// The permission every one of this app's pages hangs beneath. Switching
    /// the app on grants this subtree to the static roles; switching it off
    /// revokes it everywhere, including from roles the workspace defined.
    ///
    /// It is also the scope to give a key that should reach this app and
    /// nothing else.
    #[schema(example = "Pages.Books")]
    pub permission: String,
    /// App ids that have to be on for this one to be useful. Installing pulls
    /// them in; uninstalling one that something else needs is refused.
    #[schema(example = json!(["master"]))]
    pub requires: Vec<String>,
    /// Core. Every workspace has it, no workspace can be without it, and
    /// `enabled` is true for it whatever the table says.
    pub always_on: bool,
    pub enabled: bool,
    /// The app's version at the moment it was switched on, which is not
    /// necessarily the version running now. The difference between the two is
    /// what a "what's new" list is a list of.
    pub installed_version: Option<String>,
    pub enabled_on: Option<DateTime<Utc>>,
}

/// A descriptor and a workspace's answer to it, as one row.
fn resource_of(app: &'static AppDescriptor, state: &AppState) -> AppResource {
    AppResource {
        id: app.id.to_owned(),
        name_key: app.name.to_owned(),
        summary_key: app.summary.to_owned(),
        icon: app.icon.to_owned(),
        version: app.version.to_owned(),
        permission: app.permission.to_owned(),
        requires: app.requires.iter().map(|id| (*id).to_owned()).collect(),
        always_on: app.always_on,
        enabled: state.enabled,
        installed_version: state.installed_version.clone(),
        enabled_on: state.enabled_on,
    }
}

/// What an install did.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AppInstalled)]
pub struct InstalledResource {
    /// The app as it now stands.
    pub app: AppResource,
    /// Everything this call switched on, dependencies first, in install order.
    /// **Empty when all of it was already on**, which is a success: installing
    /// something installed is a statement about the end state, not an error.
    ///
    /// A list rather than a flag because installing one app can switch on
    /// another, and answering "done" without saying so would hide a change to
    /// somebody's menu.
    #[schema(example = json!(["master", "books"]))]
    pub switched_on: Vec<String>,
}

/// Every app in this release, with this workspace's answer to each.
///
/// Driven by the compiled catalog rather than by the table, so an app added in
/// this release appears with `enabled: false` before anything has written a row
/// for it.
///
/// Searches the id and the permission - not the name, which is a message key
/// rather than words. Sorts by `id` or `version`; the default is catalog order,
/// which is the display order. Narrows on `filter[enabled]`.
///
/// Requires `Pages.Administration.Apps`.
#[utoipa::path(
    get,
    path = "/apps",
    tag = "apps",
    operation_id = "listApps",
    params(
        ListParams,
        ("filter[enabled]" = Option<String>, Query,
            description = "`true` for what this workspace has switched on.",
            example = "true"),
    ),
    responses(
        (status = 200, description = "One page of the catalog", body = PageEnvelope<AppResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Apps", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<AppResource>>, Problem> {
    let states = service::catalog(&caller.pool, &caller.caller).await?;

    Ok(Json(PageEnvelope::new(paginate(&states, &request))))
}

/// Switch an app on, with whatever it depends on.
///
/// Idempotent: installing something already installed changes nothing, records
/// nothing, and succeeds with an empty `switched_on`. A slow button pressed
/// twice is not two subscriptions.
///
/// Requires `Pages.Administration.Apps.Install`. Note what that permission
/// really is: **installing an app is granting its permission subtree**, to
/// every static role at once. There is no second gate afterwards, by design -
/// see the service.
#[utoipa::path(
    post,
    path = "/apps/{id}/install",
    tag = "apps",
    operation_id = "installApp",
    params(("id" = String, Path, description = "The app's id", example = "books")),
    responses(
        (status = 200, description = "The app, and everything this switched on", body = InstalledResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Apps.Install", body = Problem),
        (status = 404, description = "No such app in this release", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn install(
    caller: ApiCaller,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<InstalledResource>, Problem> {
    let installed = service::install(&caller.pool, &caller.caller, &id)
        .await
        .map_err(|err| unknown_app(err, &id))?;

    if !installed.switched_on.is_empty() {
        tracing::info!(
            key = ?caller.key_id,
            app = %installed.app_id,
            switched_on = installed.switched_on.len(),
            "app installed through the api"
        );
    }

    // Read back, so the caller sees the row rather than an assumption about
    // it - including `enabled_on`, which only the database knows.
    let app = one(&caller, &installed.app_id).await?;

    Ok(Json(InstalledResource {
        app,
        switched_on: installed.switched_on,
    }))
}

/// Switch an app off.
///
/// **Its schema and every row in it stay exactly where they are.** What goes is
/// the permission, everywhere - including from roles the workspace defined
/// itself, because a role is not a subscription. Switching the app back on
/// restores the static roles' access and leaves the custom roles for somebody
/// to decide about again.
///
/// Answers `200` when the app is off at the end of the call, including when it
/// was already off. Answers `409` when it cannot be: `app_always_on` for core,
/// and `app_required_by` when something else switched on depends on it - which
/// names the dependant, because "no" without a reason is a dead end and this
/// one has an obvious next step.
///
/// Requires `Pages.Administration.Apps.Install` - the same permission, because
/// it is the same power in the other direction.
#[utoipa::path(
    post,
    path = "/apps/{id}/uninstall",
    tag = "apps",
    operation_id = "uninstallApp",
    params(("id" = String, Path, description = "The app's id", example = "books")),
    responses(
        (status = 200, description = "The app, now off", body = AppResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Apps.Install", body = Problem),
        (status = 404, description = "No such app in this release", body = Problem),
        (status = 409, description = "Core, or something switched on depends on it", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn uninstall(
    caller: ApiCaller,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<AppResource>, Problem> {
    let outcome = service::uninstall(&caller.pool, &caller.caller, &id)
        .await
        .map_err(|err| unknown_app(err, &id))?;

    match outcome {
        UninstallOutcome::SwitchedOff => {
            tracing::info!(key = ?caller.key_id, app = %id, "app switched off through the api");

            Ok(Json(one(&caller, &id).await?))
        }
        UninstallOutcome::AlwaysOn => Err(Problem::new(
            StatusCode::CONFLICT,
            "app_always_on",
            format!("{id} is core to every workspace and cannot be switched off."),
        )),
        UninstallOutcome::NeededBy { app_id } => Err(Problem::new(
            StatusCode::CONFLICT,
            "app_required_by",
            format!("{app_id} is switched on and depends on {id}. Switch {app_id} off first."),
        )),
    }
}

/// An app id this release does not contain, as a sentence somebody can read.
///
/// The service reports it as `ServiceError::NotFound("app")`, which converts
/// on its own to a 404 - with `not found: app` as the detail, because
/// `Error::NotFound`'s `Display` is written for a log line rather than for a
/// caller. The status was already right; only the sentence was not, and a
/// published surface should not answer with a fragment of our own vocabulary.
fn unknown_app(err: ServiceError, app_id: &str) -> Problem {
    if matches!(err, ServiceError::NotFound(_)) {
        return Problem::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("There is no app called {app_id} in this release."),
        );
    }

    Problem::from(err)
}

/// One app's row, re-read after a change.
///
/// The catalog call is the same one the list makes and applies the same
/// `Caller::require`, so this costs one read and gives every write the same
/// answer a subsequent `GET /apps` would.
async fn one(caller: &ApiCaller, app_id: &str) -> Result<AppResource, Problem> {
    let states = service::catalog(&caller.pool, &caller.caller).await?;

    states
        .iter()
        .find(|state| state.app_id == app_id)
        .and_then(|state| apps::find(&state.app_id).map(|app| resource_of(app, state)))
        .ok_or_else(|| {
            Problem::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("There is no app called {app_id} in this release."),
            )
        })
}

/// Search, narrow, sort and cut one page - in memory, and deliberately.
///
/// The catalog is a compiled `const` table with a handful of entries. Paging it
/// in SQL would be two statements to save nothing, and the rows do not come
/// from SQL in the first place.
fn paginate(states: &[AppState], request: &PageRequest) -> Page<AppResource> {
    let needle = request.needle();

    let mut matching: Vec<(usize, &'static AppDescriptor, &AppState)> = states
        .iter()
        // Catalog order is display order, and the index is what preserves it
        // through a sort that does not name a column.
        .enumerate()
        // A state whose descriptor this build does not have cannot happen -
        // `catalog` is driven *by* the descriptors - and dropping it rather
        // than unwrapping is what keeps that true of the code as well as of
        // today's data.
        .filter_map(|(index, state)| apps::find(&state.app_id).map(|app| (index, app, state)))
        .filter(|(_, app, _)| match &needle {
            Some(needle) => {
                app.id.to_lowercase().contains(needle)
                    || app.permission.to_lowercase().contains(needle)
            }
            None => true,
        })
        .filter(|(_, _, state)| match request.filter("enabled") {
            Some(flag) => state.enabled == flag.eq_ignore_ascii_case("true"),
            None => true,
        })
        .collect();

    let descending = request
        .sort
        .as_ref()
        .is_some_and(|sort| !sort.direction.is_ascending());

    // The id is the tie-break, and it is unique by construction - a test in
    // `phonix_core::apps` refuses a repeat - so no two rows compare equal.
    match request.sort.as_ref().map(|sort| sort.field.as_str()) {
        Some("id") => matching.sort_by_key(|(_, app, _)| app.id),
        Some("version") => matching
            .sort_by(|(_, a, _), (_, b, _)| a.version.cmp(b.version).then_with(|| a.id.cmp(b.id))),
        _ => matching.sort_by_key(|(index, _, _)| *index),
    }
    if descending {
        matching.reverse();
    }

    cut(matching, request, |(_, app, state)| resource_of(app, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states() -> Vec<AppState> {
        apps::CATALOG
            .iter()
            .map(|app| AppState {
                app_id: app.id.to_owned(),
                enabled: app.always_on,
                installed_version: app.always_on.then(|| app.version.to_owned()),
                enabled_on: None,
            })
            .collect()
    }

    #[test]
    fn the_default_page_is_the_catalog_in_its_own_order() {
        let page = paginate(&states(), &PageRequest::first(100));

        assert_eq!(page.total as usize, apps::CATALOG.len());
        let ids: Vec<&str> = page.rows.iter().map(|row| row.id.as_str()).collect();
        let catalog: Vec<&str> = apps::CATALOG.iter().map(|app| app.id).collect();
        assert_eq!(ids, catalog);
    }

    #[test]
    fn an_always_on_app_reads_as_enabled() {
        // Whatever the table says. A row claiming core was off would be a
        // workspace nobody can sign in to, and the service refuses to believe
        // one - so this surface must not report it either.
        let page = paginate(&states(), &PageRequest::first(100));

        for row in &page.rows {
            if row.always_on {
                assert!(row.enabled, "{} is core and must read as on", row.id);
            }
        }
    }

    #[test]
    fn the_enabled_filter_narrows_both_ways() {
        let on = paginate(
            &states(),
            &PageRequest::first(100).filtered_by("enabled", "true"),
        );
        let off = paginate(
            &states(),
            &PageRequest::first(100).filtered_by("enabled", "false"),
        );

        assert!(on.rows.iter().all(|row| row.enabled));
        assert!(off.rows.iter().all(|row| !row.enabled));
        assert_eq!(on.total + off.total, apps::CATALOG.len() as u64);
    }

    #[test]
    fn the_permission_subtree_is_on_the_row() {
        // It is what a client scopes a key to when it wants a key that reaches
        // one app, so leaving it off would mean reading it out of
        // `/permissions` by guessing at the naming convention.
        let page = paginate(&states(), &PageRequest::first(100));

        for row in &page.rows {
            // A node of the real tree, which is the claim that matters: it is
            // the string a client puts in a key's `scopes`, and one that is
            // not in `DEFINITIONS` would be silently refused at issue time.
            // Not "starts with `Pages.`" - core owns the root itself.
            assert!(
                phonix_core::authorization::is_defined(&row.permission),
                "{} names {}, which is not in the permission tree",
                row.id,
                row.permission
            );
        }
    }

    #[test]
    fn a_search_looks_at_the_id_and_the_permission_rather_than_a_message_key() {
        // Searching `name_key` would match "app." on every row, which is a
        // search that always answers everything.
        let page = paginate(
            &states(),
            &PageRequest {
                search: "app.".to_owned(),
                ..PageRequest::first(100)
            }
            .sanitised(),
        );

        assert_eq!(page.total, 0);
    }
}
