//! Which apps a workspace has switched on.
//!
//! Reads and writes `core.installed_apps` from the *tenant* connection, on the
//! ordinary search path - as opposed to [`super::apps`], which writes the same
//! table from a migration pool rooted at some other app's schema and therefore
//! has to qualify everything.
//!
//! # Installed and enabled are two different facts
//!
//! Every app compiled into this build has its schema migrated into every tenant
//! database, always. `schema_version` and `state` are about that, and they are
//! the migration runner's business.
//!
//! `enabled_at` is about subscription: this workspace chose Books. It is the
//! only one of the four that a person ever sets, and switching it off leaves
//! every table and every row exactly where they were - which is what makes
//! reinstalling instant rather than a restore.
//!
//! What enablement *does* is in `phonix_services::workspace::apps`: it decides
//! which permissions the static roles hold, and everything downstream already
//! answers to permissions.

use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// One row of `core.installed_apps`, as a workspace sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInstall {
    pub app_id: String,
    /// Highest migration applied to this app's schema. Present on every app in
    /// the build, whether or not the workspace uses it.
    pub schema_version: Option<String>,
    /// The app's own version at the moment it was switched on.
    pub app_version: Option<String>,
    /// `None` when the workspace has not subscribed to it.
    pub enabled_at: Option<DateTime<Utc>>,
    pub enabled_by: Option<Uuid>,
}

impl AppInstall {
    pub const fn is_enabled(&self) -> bool {
        self.enabled_at.is_some()
    }
}

/// Every app this tenant database knows about.
///
/// Rows the build no longer contains are returned too. That is on purpose: an
/// app removed from a release still owns a schema full of somebody's data, and
/// a list that silently omitted it would be the reason nobody noticed.
pub async fn list<'e, E>(executor: E) -> Result<Vec<AppInstall>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query(
        "SELECT app_id, schema_version, app_version, enabled_at, enabled_by
           FROM installed_apps
          ORDER BY app_id",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter()
        .map(|row| {
            Ok(AppInstall {
                app_id: row.try_get("app_id")?,
                schema_version: row.try_get("schema_version")?,
                app_version: row.try_get("app_version")?,
                enabled_at: row.try_get("enabled_at")?,
                enabled_by: row.try_get("enabled_by")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(DbError::Query)
}

/// The ids this workspace has switched on.
///
/// The narrow question, asked on nearly every permission sync, so it is a
/// column rather than a filter over [`list`].
pub async fn enabled_ids<'e, E>(executor: E) -> Result<Vec<String>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query(
        "SELECT app_id FROM installed_apps WHERE enabled_at IS NOT NULL ORDER BY app_id",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter()
        .map(|row| row.try_get::<String, _>("app_id"))
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(DbError::Query)
}

/// Switch an app on, recording who did it and which version they got.
///
/// Idempotent by design rather than by accident: an app already on keeps its
/// original `enabled_at` and `enabled_by`. Re-running an install must not
/// rewrite the date somebody subscribed, which is a billing fact, and a second
/// click on a slow button must not look like a second subscription.
///
/// An app switched on *again* after being switched off does get today's date
/// and today's version, because that is a new subscription and a possibly
/// different app from the one they had.
///
/// Returns whether this call is the one that switched it on, so the caller can
/// tell an install from a no-op - the audit trail should record one and not the
/// other.
pub async fn enable<'e, E>(
    executor: E,
    app_id: &str,
    version: &str,
    by: Option<Uuid>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    // `installing` rather than `active` for a row that does not exist yet: the
    // migration runner owns `state`, and inventing `active` here would claim a
    // schema is ready when nothing has run.
    //
    // The `WHERE` on the conflict clause is what makes the answer honest. A row
    // already switched on is not updated and therefore not returned, so
    // `is_some()` means "this call is the one that switched it on" - which is
    // the difference between an audit entry and silence.
    let switched_on = sqlx::query(
        "INSERT INTO installed_apps (app_id, state, app_version, enabled_at, enabled_by)
         VALUES ($1, 'installing', $2, now(), $3)
         ON CONFLICT (app_id) DO UPDATE
            SET app_version = EXCLUDED.app_version,
                enabled_at  = EXCLUDED.enabled_at,
                enabled_by  = EXCLUDED.enabled_by
          WHERE installed_apps.enabled_at IS NULL
         RETURNING app_id",
    )
    .bind(app_id)
    .bind(version)
    .bind(by)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?
    .is_some();

    Ok(switched_on)
}

/// Switch an app off. Its schema and every row in it stay.
///
/// Returns whether it had been on.
pub async fn disable<'e, E>(executor: E, app_id: &str) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE installed_apps
            SET enabled_at = NULL, enabled_by = NULL
          WHERE app_id = $1 AND enabled_at IS NOT NULL",
    )
    .bind(app_id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected() > 0)
}
