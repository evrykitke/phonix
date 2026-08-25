//! The `core` schema, end to end, against a live PostgreSQL server.
//!
//! Two paths have to arrive at the same place, and only one of them is the one
//! a developer sees day to day:
//!
//! * a database provisioned **today**, which builds straight into `core`;
//! * a database provisioned **before** migration 0014, whose seventeen tables
//!   are sitting in `public` and have to be relocated underneath a migrator
//!   that is mid-run.
//!
//! The second is the one worth a test. Migration 0014 moves sqlx's own
//! `_sqlx_migrations` bookkeeping table, in the same transaction in which sqlx
//! is about to record that 0014 succeeded.
//!
//! Ignored by default: these need a reachable server and the credentials in
//! `.env`. Run them deliberately.
//!
//! ```text
//! cargo test -p phonix-db --test tenant_schema -- --ignored --test-threads=1
//! ```

use phonix_config::DatabaseConfig;
use phonix_db::sqlx::{self, PgPool, Row};
use phonix_db::tenancy::{apps, provision};

/// Provisioned the way the runner does it today.
const FRESH: &str = "phonix_test_schema_fresh";
/// Built the way a database created before 0014 was: everything in `public`.
const LEGACY: &str = "phonix_test_schema_legacy";

/// Every table core's stream creates, all of which must live in `core`.
const CORE_TABLES: &[&str] = &[
    "users",
    "sessions",
    "user_tokens",
    "user_mfa_factors",
    "password_history",
    "identity_events",
    "roles",
    "user_roles",
    "role_permissions",
    "user_permissions",
    "entity_events",
    "file_uploads",
    "workspace_settings",
    "organization_profile",
    "mail_settings",
    "outbox_events",
    "processed_events",
    "currencies",
    "exchange_rates",
    "number_sequences",
];

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
}

/// Drop and recreate, so a failed run never poisons the next one.
async fn recreate(cfg: &DatabaseConfig, database: &str) {
    provision::drop_tenant_database(cfg, database)
        .await
        .expect("drop scratch database");

    let mut conn = phonix_db::maintenance_connection(cfg)
        .await
        .expect("maintenance connection");

    let sql = format!(r#"CREATE DATABASE "{database}" ENCODING 'UTF8'"#);
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&mut conn)
        .await
        .expect("create scratch database");
}

fn open(cfg: &DatabaseConfig, database: &str) -> PgPool {
    phonix_db::tenant_pool(cfg, database)
}

async fn tables_in(pool: &PgPool, schema: &str) -> Vec<String> {
    sqlx::query("SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename")
        .bind(schema)
        .fetch_all(pool)
        .await
        .expect("list tables")
        .into_iter()
        .map(|row| row.get::<String, _>("tablename"))
        .collect()
}

/// Everything true of a tenant database once the runner has finished with it,
/// whichever path it took to get there.
async fn assert_core_is_whole(pool: &PgPool, context: &str) {
    let core = tables_in(pool, "core").await;

    for table in CORE_TABLES {
        assert!(
            core.iter().any(|found| found == table),
            "{context}: core.{table} is missing; core holds {core:?}"
        );
    }

    assert!(
        core.iter().any(|found| found == "_sqlx_migrations"),
        "{context}: the migrator's bookkeeping stayed behind in public"
    );
    assert!(
        core.iter().any(|found| found == "installed_apps"),
        "{context}: core.installed_apps was not created"
    );

    let public = tables_in(pool, "public").await;
    assert!(
        public.is_empty(),
        "{context}: public should be empty, holds {public:?}"
    );

    // No triggers, anywhere.
    //
    // `core` used to carry five `BEFORE UPDATE` triggers that set `updated_at`,
    // and migration 0017 removed them along with the plpgsql function behind
    // them. The rule is that application behaviour lives in the application:
    // the database refuses bad writes (`CHECK`, `REFERENCES`, `NOT NULL`) and
    // supplies defaults on insert, but it does not perform writes of its own.
    //
    // Asserted as an absolute rather than as a count, because this is the kind
    // of thing that comes back one table at a time. A migration that adds a
    // trigger fails here, and the failure names it.
    let triggers: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname || '.' || t.tgname
           FROM pg_trigger t
           JOIN pg_class c ON c.oid = t.tgrelid
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE NOT t.tgisinternal AND n.nspname NOT IN ('pg_catalog', 'information_schema')
          ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .expect("list triggers");
    assert!(
        triggers.is_empty(),
        "{context}: the database is doing work of its own; triggers: {triggers:?}"
    );

    // And nothing left behind to hang one off. A stored procedure is the same
    // problem wearing a different hat.
    //
    // Functions belonging to an extension are excluded by `pg_depend`, not by
    // name: `pgcrypto` installs a few dozen into `public`, and they are not
    // ours to have an opinion about.
    let routines: Vec<String> = sqlx::query_scalar(
        "SELECT n.nspname || '.' || p.proname
           FROM pg_proc p
           JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
            AND NOT EXISTS (
                SELECT 1 FROM pg_depend d
                 WHERE d.objid = p.oid AND d.deptype = 'e'
            )
          ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .expect("list routines");
    assert!(
        routines.is_empty(),
        "{context}: stored routines survive: {routines:?}"
    );

    // Sequences owned by relocated columns follow their table.
    let stranded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_sequences WHERE schemaname <> 'core'")
            .fetch_one(pool)
            .await
            .expect("count sequences");
    assert_eq!(
        stranded, 0,
        "{context}: a sequence did not follow its table"
    );
}

/// The tables the `master` app owns.
///
/// Listed here rather than counted, for the reason [`CORE_TABLES`] is: a
/// migration that half-ran leaves a schema with most of its tables, and a count
/// would pass.
const MASTER_TABLES: &[&str] = &[
    "parties",
    "party_roles",
    "party_addresses",
    "party_contacts",
    "tax_codes",
    "tax_rates",
    "tax_groups",
    "tax_group_members",
];

/// The second app exists, in its own schema, with its own bookkeeping.
///
/// This is what proves the mechanism rather than the app: if `master` cannot be
/// built out of the same registry entry, search path and stream that `core`
/// uses, then apps are not really installable and the next one will need a
/// special case.
async fn assert_master_is_whole(pool: &PgPool, context: &str) {
    let master = tables_in(pool, "master").await;

    for table in MASTER_TABLES {
        assert!(
            master.iter().any(|found| found == table),
            "{context}: master.{table} is missing; master holds {master:?}"
        );
    }

    // The streams are independent because each app's migrations run on a search
    // path rooted at its own schema, which is what puts sqlx's own bookkeeping
    // beside the tables it created rather than in core's.
    assert!(
        master.iter().any(|found| found == "_sqlx_migrations"),
        "{context}: master's migration bookkeeping did not land in master"
    );

    // The one constraint the tax design leans on: two rates for one code can
    // never be live at the same time. Checked by name, because losing it would
    // not fail any other assertion here - and a quarter would be filed before
    // anybody noticed.
    let overlap_guard: i64 = sqlx::query_scalar(
        "SELECT count(*)
           FROM pg_constraint c
           JOIN pg_namespace n ON n.oid = c.connamespace
          WHERE n.nspname = 'master' AND c.conname = 'tax_rates_no_overlap'",
    )
    .fetch_one(pool)
    .await
    .expect("look for the exclusion constraint");
    assert_eq!(
        overlap_guard, 1,
        "{context}: master.tax_rates lost its no-overlap exclusion constraint"
    );
}

/// One app is present, migrated, and recorded at the version this build embeds.
async fn assert_app_is_recorded(pool: &PgPool, app_id: &str, context: &str) {
    let row =
        sqlx::query("SELECT schema_version, state FROM core.installed_apps WHERE app_id = $1")
            .bind(app_id)
            .fetch_one(pool)
            .await
            .unwrap_or_else(|err| panic!("{context}: {app_id} is not in installed_apps: {err}"));

    let expected = apps::APPS
        .iter()
        .find(|app| app.app_id == app_id)
        .unwrap_or_else(|| panic!("{app_id} is registered"))
        .latest_version();

    assert_eq!(
        row.get::<String, _>("schema_version"),
        expected,
        "{context}: installed_apps disagrees with the embedded migrator for {app_id}"
    );
    assert_eq!(row.get::<String, _>("state"), "active", "{context}");
}

/// Every app this build carries is installed and recorded.
async fn assert_core_is_recorded(pool: &PgPool, context: &str) {
    for app in apps::APPS {
        assert_app_is_recorded(pool, app.app_id, context).await;
    }
    // Named explicitly as well, so the reason core comes first stays visible:
    // it is the app that creates the table the others record themselves in.
    assert_app_is_recorded(pool, apps::CORE_APP_ID, context).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_fresh_database_is_built_straight_into_core() {
    let cfg = database_config();
    recreate(&cfg, FRESH).await;

    provision::migrate_tenant(&cfg, FRESH)
        .await
        .expect("first migration pass");

    let pool = open(&cfg, FRESH);
    assert_core_is_whole(&pool, "fresh").await;
    assert_master_is_whole(&pool, "fresh").await;
    assert_core_is_recorded(&pool, "fresh").await;
    pool.close().await;

    // The sweep runs on every boot. A second pass has to be a no-op rather than
    // an error, which is what makes 0014's relocation loop conditional.
    provision::migrate_tenant(&cfg, FRESH)
        .await
        .expect("second migration pass is idempotent");

    let pool = open(&cfg, FRESH);
    assert_core_is_whole(&pool, "fresh, twice").await;
    assert_master_is_whole(&pool, "fresh, twice").await;
    pool.close().await;

    provision::drop_tenant_database(&cfg, FRESH)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_database_left_in_public_is_relocated_under_the_migrator() {
    let cfg = database_config();
    recreate(&cfg, LEGACY).await;

    // Reproduce the pre-0014 layout the honest way: run the real stream on the
    // real search path, but *without* creating the `core` schema first. With
    // core absent, `core,public` resolves to public, so 0001-0013 build exactly
    // where they used to - and then 0014 has something to move.
    let legacy = phonix_db::connect::schema_migration_pool(
        &cfg,
        LEGACY,
        phonix_db::connect::TENANT_SEARCH_PATH,
    );
    phonix_db::CORE_MIGRATIONS
        .run(&legacy)
        .await
        .expect("migrating a database that had no core schema");
    legacy.close().await;

    let pool = open(&cfg, LEGACY);
    assert_core_is_whole(&pool, "relocated").await;
    // Not `assert_master_is_whole` yet, deliberately: only core's stream has
    // been run by hand at this point, which is exactly the pre-0014 state this
    // test is reproducing. `master` arrives with the runner, below.

    // The migrator recorded 0014 itself, in a table 0014 moved mid-transaction.
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM core._sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("read the relocated migration log");
    assert_eq!(
        applied as usize,
        phonix_db::CORE_MIGRATIONS.iter().count(),
        "the migrator lost track of itself across the relocation"
    );
    pool.close().await;

    // And the runner proper is happy to take it from here.
    provision::migrate_tenant(&cfg, LEGACY)
        .await
        .expect("the runner is idempotent over a relocated database");

    let pool = open(&cfg, LEGACY);
    assert_core_is_whole(&pool, "relocated, then swept").await;
    assert_master_is_whole(&pool, "relocated, then swept").await;
    assert_core_is_recorded(&pool, "relocated, then swept").await;
    pool.close().await;

    provision::drop_tenant_database(&cfg, LEGACY)
        .await
        .expect("clean up");
}
