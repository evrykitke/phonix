//! The apps compiled into this build, and what each tenant has installed.
//!
//! An app owns one Postgres schema inside every tenant database and one
//! migration stream of its own. Installing it is `CREATE SCHEMA` plus that
//! stream; uninstalling it is an export and `DROP SCHEMA`, which is only ever
//! safe because no app schema may hold a foreign key into another one.
//!
//! # Why the registry is compiled in
//!
//! Apps are crates, not plugins. The hydrate bundle is built once and shipped
//! to every browser, so there is nowhere for a route, a form config or a
//! permission to arrive from at runtime. What varies per tenant is which of the
//! compiled-in apps is *switched on* - and that is a subscription question,
//! answered in the catalog database, not here.
//!
//! # Adding an app
//!
//! One entry in [`APPS`], and a directory of migrations beside core's:
//!
//! ```ignore
//! // crates/phonix-db/src/lib.rs
//! pub static BOOKS_MIGRATIONS: sqlx::migrate::Migrator =
//!     sqlx::migrate!("../../migrations/apps/books");
//!
//! // here
//! AppMigrations { app_id: "books", migrator: &crate::BOOKS_MIGRATIONS },
//! ```
//!
//! Two rules the migrations themselves have to keep, neither of which the
//! compiler can check:
//!
//! * **Qualify every reference to core.** `core.users`, never `users`. An app's
//!   search path is its own schema and `public`; core is absent on purpose, so
//!   an unqualified reference fails loudly instead of resolving by luck.
//! * **No foreign key into another app's schema.** Reference by id and resolve
//!   through a capability port. An FK is what makes an app impossible to
//!   uninstall.
//!
//! See `docs/adr/0001-core-boundary.md`.

use sqlx::PgPool;
use sqlx::migrate::Migrator;

use crate::error::DbError;

/// The app that owns the infrastructure every other app is allowed to depend
/// on. Always installed; never uninstallable.
pub const CORE_APP_ID: &str = "core";

/// Commercial master data: the parties a workspace trades with, and the tax
/// codes, rates and groups it applies to them.
///
/// An ordinary app, installed unconditionally for now because every commercial
/// product needs it. A pure clinical build would leave it out, and nothing in
/// `core` would notice.
pub const MASTER_APP_ID: &str = "master";

/// One installable app's migration stream.
#[derive(Debug)]
pub struct AppMigrations {
    /// Stable, lowercase, and *identical to the Postgres schema it owns*.
    /// Deriving the schema name rather than storing it separately is what stops
    /// the two disagreeing.
    pub app_id: &'static str,
    pub migrator: &'static Migrator,
}

impl AppMigrations {
    /// The search path this app's migrations run under.
    ///
    /// Its own schema first, so `CREATE TABLE invoices` lands in `books` and
    /// sqlx's own `_sqlx_migrations` bookkeeping lands there beside it - which
    /// is what makes the streams independent without sqlx needing to know that
    /// apps exist. `public` behind it for pgcrypto.
    ///
    /// For core this evaluates to [`crate::connect::TENANT_SEARCH_PATH`], which
    /// is not a coincidence: core is an app like any other, it just happens to
    /// be the one every request already runs on.
    pub fn search_path(&self) -> String {
        format!("{},public", self.app_id)
    }

    /// The highest version in this app's stream, zero-padded.
    ///
    /// Read from the embedded migrator rather than written down, so adding a
    /// migration cannot leave a version marker claiming an older schema than
    /// the databases actually have.
    pub fn latest_version(&self) -> String {
        let version = self
            .migrator
            .iter()
            .map(|migration| migration.version)
            .max()
            .unwrap_or(0);
        format!("{version:04}")
    }
}

/// Every app compiled into this build.
///
/// Core first, and not merely by convention: [`record_installed`] writes to
/// `core.installed_apps`, which core's own stream is what creates.
pub static APPS: &[AppMigrations] = &[
    AppMigrations {
        app_id: CORE_APP_ID,
        migrator: &crate::CORE_MIGRATIONS,
    },
    AppMigrations {
        app_id: MASTER_APP_ID,
        migrator: &crate::MASTER_MIGRATIONS,
    },
];

/// The schema version this build expects of a tenant database, across all apps.
///
/// Covers every app rather than core alone. The boot sweep skips any tenant
/// whose marker already matches, so a fingerprint that ignored apps would let
/// an app gain a migration and never reach the tenants that need it - the first
/// query wanting the new column would then fail at runtime, per tenant, in
/// production.
///
/// Renders as `core:0014`, or `core:0014,books:0003` once there is more than
/// one. The format is opaque: it is compared, never parsed.
pub fn schema_fingerprint() -> String {
    APPS.iter()
        .map(|app| format!("{}:{}", app.app_id, app.latest_version()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Record that an app's schema is present and migrated.
///
/// Fully qualified, because this runs on a connection whose search path is the
/// *app's* schema - `installed_apps` would not resolve.
///
/// `state` is left alone unless it is still `installing`. A tenant whose
/// subscription lapsed sits at `read_only`, and a migration is not a payment:
/// flipping it back to `active` here would hand back an app nobody is paying
/// for, silently, on the next deploy.
pub async fn record_installed(pool: &PgPool, app_id: &str, version: &str) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO core.installed_apps (app_id, schema_version, state, migrated_at)
         VALUES ($1, $2, 'active', now())
         ON CONFLICT (app_id) DO UPDATE
            SET schema_version = EXCLUDED.schema_version,
                migrated_at    = now(),
                state          = CASE
                                     WHEN core.installed_apps.state = 'installing'
                                     THEN 'active'
                                     ELSE core.installed_apps.state
                                 END",
    )
    .bind(app_id)
    .bind(version)
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_first_because_it_creates_the_table_the_others_record_in() {
        assert_eq!(APPS.first().map(|app| app.app_id), Some(CORE_APP_ID));
    }

    #[test]
    fn app_ids_are_unique_and_safe_as_schema_names() {
        let mut seen = Vec::new();
        for app in APPS {
            assert!(
                !seen.contains(&app.app_id),
                "duplicate app id {:?} - app ids are schema names",
                app.app_id
            );
            seen.push(app.app_id);

            assert!(
                app.app_id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                    && app.app_id.starts_with(|c: char| c.is_ascii_lowercase()),
                "app id {:?} reaches DDL as a schema name and must be a bare \
                 lowercase identifier",
                app.app_id
            );
        }
    }

    #[test]
    fn core_migrations_search_path_matches_the_one_requests_run_on() {
        let core = APPS.first().expect("core is registered");
        assert_eq!(core.search_path(), crate::connect::TENANT_SEARCH_PATH);
    }

    #[test]
    fn fingerprint_names_every_app() {
        let fingerprint = schema_fingerprint();
        for app in APPS {
            assert!(
                fingerprint.contains(app.app_id),
                "{fingerprint} omits {}",
                app.app_id
            );
        }
    }
}
