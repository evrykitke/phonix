//! Switching an app on for a workspace, and off again.
//!
//! # Installing does not install anything
//!
//! The schema is already there. Every app compiled into this build has had its
//! migrations run in every tenant database, on boot, whether the workspace uses
//! it or not - see `phonix_db::tenancy::provision`. So [`install`] writes one
//! timestamp and then re-syncs the static roles, and that second part is what
//! makes the app appear: the menu, the command palette, every grid and every
//! `Caller::require` in every service already answer to permissions, so
//! granting the subtree beneath the app's root turns all of them on at once.
//!
//! Doing it the other way round - a schema created on subscription - would put
//! DDL under a live request and leave a half-migrated database as an ordinary
//! outcome. Doing it with a second gate beside permissions would be two things
//! to keep in step, and one of them would eventually be forgotten in a service
//! nobody thought about.
//!
//! # The eight seconds are the browser's, not this file's
//!
//! [`install`] returns as soon as the write commits, which is fast. The install
//! dialog holds a progress animation for its own reasons - a subscription that
//! completes between two frames reads as a button that did nothing - and that
//! is a decision about perception, made where perception happens. Putting a
//! sleep in here would make every future caller wait too: the API, a script,
//! a bulk provisioning run.
//!
//! # Switching off is not deleting
//!
//! The rows stay. What goes is the permission, everywhere - including from
//! roles the organization defined themselves, because a role is not a
//! subscription. Switching the app back on restores `Admin`'s access and leaves
//! the custom roles for somebody to decide about again, which is the honest
//! default: re-granting a role months later would be re-granting access
//! somebody may have meant to remove in between.

use phonix_core::apps::{self, AppDescriptor};
use phonix_core::permissions;
use phonix_db::authorization::role;
use phonix_db::sqlx::PgPool;
use phonix_db::tenancy::installs;

use crate::audit::{self, Target, kinds};
use crate::caller::Caller;
use crate::error::{ServiceError, ServiceResult};

pub use phonix_core::apps::{AppState, Installed, UninstallOutcome};

/// Every app in this release, and what this workspace has done about each.
///
/// Driven by the compiled catalog rather than by the table, so an app added in
/// this release appears with `enabled: false` before anything has written a row
/// for it - which is exactly the state the store needs to offer it.
pub async fn catalog(pool: &PgPool, caller: &Caller) -> ServiceResult<Vec<AppState>> {
    caller.require(permissions::APPS)?;

    let rows = installs::list(pool).await?;

    Ok(apps::CATALOG
        .iter()
        .map(|app| {
            let row = rows.iter().find(|row| row.app_id == app.id);
            AppState {
                app_id: app.id.to_owned(),
                // An always-on app is on whatever the table says. A row that
                // claimed otherwise would be a workspace nobody can sign in to,
                // and this is the cheapest place to refuse to believe it.
                enabled: app.always_on || row.is_some_and(|row| row.is_enabled()),
                installed_version: row.and_then(|row| row.app_version.clone()),
                enabled_on: row.and_then(|row| row.enabled_at),
            }
        })
        .collect())
}

/// The ids this workspace has switched on, for anything that needs to gate.
///
/// Ungated, like the currency list and for the same reason: it is what a shell
/// renders its launcher from, and requiring a permission to read it would mean
/// only administrators could see their own menu.
pub async fn enabled_ids(pool: &PgPool) -> ServiceResult<Vec<String>> {
    installs::enabled_ids(pool)
        .await
        .map_err(ServiceError::from)
}

/// Switch an app on for this workspace, with whatever it depends on.
///
/// Idempotent: installing something already installed changes nothing, records
/// nothing, and succeeds. A slow button clicked twice is not two
/// subscriptions.
pub async fn install(pool: &PgPool, caller: &Caller, app_id: &str) -> ServiceResult<Installed> {
    caller.require(permissions::APPS_INSTALL)?;

    let app = known(app_id)?;
    let mut switched_on = Vec::new();

    // Dependencies first, and the catalog guarantees they come earlier in it -
    // so this is one pass, not a graph walk. `requires` is one level deep by
    // construction; a test in `phonix_core::apps` refuses anything else.
    for needed in app.requires {
        let dependency = known(needed)?;
        if enable(pool, caller, dependency).await? {
            switched_on.push(dependency.id.to_owned());
        }
    }

    if enable(pool, caller, app).await? {
        switched_on.push(app.id.to_owned());
    }

    if !switched_on.is_empty() {
        // Once, after the whole set. Syncing per app would leave a window in
        // which Books' pages were reachable and master data's were not.
        role::sync_static_roles(pool).await?;
    }

    Ok(Installed {
        app_id: app.id.to_owned(),
        switched_on,
    })
}

/// Switch an app off. Its schema and every row in it stay exactly where they
/// are.
pub async fn uninstall(
    pool: &PgPool,
    caller: &Caller,
    app_id: &str,
) -> ServiceResult<UninstallOutcome> {
    caller.require(permissions::APPS_INSTALL)?;

    let app = known(app_id)?;

    if app.always_on {
        return Ok(UninstallOutcome::AlwaysOn);
    }

    let enabled = installs::enabled_ids(pool).await?;

    if let Some(dependant) = apps::enabled_in(&enabled)
        .find(|other| other.id != app.id && other.requires.contains(&app.id))
    {
        return Ok(UninstallOutcome::NeededBy {
            app_id: dependant.id.to_owned(),
        });
    }

    if !installs::disable(pool, app.id).await? {
        // Already off. Nothing to record and nothing to revoke.
        return Ok(UninstallOutcome::SwitchedOff);
    }

    // Both halves, in this order. `sync_static_roles` rewrites Admin and User
    // from the compiled tree filtered by what is now enabled; `revoke_everywhere`
    // deals with the roles an organization defined for itself and with per-user
    // overrides, which the sync deliberately does not touch.
    role::sync_static_roles(pool).await?;
    let revoked = role::revoke_everywhere(pool, app.permission).await?;

    audit::changed_json(
        pool,
        caller,
        Target::new(kinds::APP, app.id).named(app.id),
        serde_json::json!({ "enabled": true }),
        serde_json::json!({ "enabled": false, "grants_revoked": revoked }),
    )
    .await;

    tracing::info!(app = app.id, revoked, "app switched off");
    Ok(UninstallOutcome::SwitchedOff)
}

/// One app on, audited. Answers whether this call is what switched it.
async fn enable(pool: &PgPool, caller: &Caller, app: &AppDescriptor) -> ServiceResult<bool> {
    if !installs::enable(pool, app.id, app.version, caller.user_id()).await? {
        return Ok(false);
    }

    audit::changed_json(
        pool,
        caller,
        Target::new(kinds::APP, app.id).named(app.id),
        serde_json::json!({ "enabled": false }),
        serde_json::json!({ "enabled": true, "version": app.version }),
    )
    .await;

    tracing::info!(app = app.id, version = app.version, "app switched on");
    Ok(true)
}

/// An app id this build actually contains.
///
/// A workspace's table can name an app a later release dropped, and a request
/// can name anything at all. Neither is an internal error - both are somebody
/// asking for something that is not here.
fn known(app_id: &str) -> ServiceResult<&'static AppDescriptor> {
    apps::find(app_id).ok_or(ServiceError::NotFound("app"))
}
