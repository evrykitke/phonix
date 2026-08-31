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
use phonix_db::authorization::role;
use phonix_db::sqlx::{self, PgPool, Row};
use phonix_db::tenancy::{apps, installs, provision};

/// Provisioned the way the runner does it today.
const FRESH: &str = "phonix_test_schema_fresh";
/// Built the way a database created before 0014 was: everything in `public`.
const LEGACY: &str = "phonix_test_schema_legacy";
/// Aged back a release, to see whether the sweep catches the role up.
const PERMISSIONS: &str = "phonix_test_schema_permissions";
/// Switched on and off again, to see what that does to the grants.
const ENABLEMENT: &str = "phonix_test_schema_enablement";

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
    "api_keys",
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

/// The tables the `books` app owns.
const BOOKS_TABLES: &[&str] = &["invoices", "invoice_lines", "invoice_line_taxes"];

/// The first app that issues a numbered document, and the proof that the
/// numbering install path works from end to end.
///
/// `core.number_sequences` shipped empty and stayed empty through two apps.
/// Books declares one in `config/numbering/books.toml`, so after installing it
/// the table has exactly one row - which is the only assertion in this file
/// that reaches across two schemas, and the point of the whole mechanism.
async fn assert_books_is_whole(pool: &PgPool, context: &str) {
    let books = tables_in(pool, "books").await;

    for table in BOOKS_TABLES {
        assert!(
            books.iter().any(|found| found == table),
            "{context}: books.{table} is missing; books holds {books:?}"
        );
    }

    // The belt-and-braces index. The sequence in core is what makes numbering
    // gap-free; this is what stops a duplicate reaching the ledger if somebody
    // edits a series by hand, and it cannot live in core.
    let number_guard: i64 = sqlx::query_scalar(
        "SELECT count(*)
           FROM pg_indexes
          WHERE schemaname = 'books' AND indexname = 'invoices_number_key'",
    )
    .fetch_one(pool)
    .await
    .expect("look for the number index");
    assert_eq!(
        number_guard, 1,
        "{context}: books.invoices lost its unique number index"
    );

    // And the series the app declared is in core, installed by the runner.
    let series: Vec<String> = sqlx::query_scalar(
        "SELECT doc_type || ' ' || pattern
           FROM core.number_sequences
          WHERE app_id = 'books'
          ORDER BY doc_type",
    )
    .fetch_all(pool)
    .await
    .expect("read the installed series");
    assert_eq!(
        series,
        vec!["sales_invoice INV-{YYYY}-#####".to_owned()],
        "{context}: books' number series was not installed from its config file"
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

/// Every permission Admin holds in this database.
async fn admin_grants(pool: &PgPool) -> Vec<String> {
    sqlx::query(
        "SELECT rp.name
           FROM role_permissions rp
           JOIN roles r ON r.id = rp.role_id
          WHERE lower(r.name) = 'admin'
          ORDER BY rp.name",
    )
    .fetch_all(pool)
    .await
    .expect("list admin grants")
    .into_iter()
    .map(|row| row.get::<String, _>("name"))
    .collect()
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn switching_an_app_on_and_off_moves_its_permissions_with_it() {
    // The whole mechanism, end to end against a real database: enablement is
    // stored in one column, and the *only* thing it does is decide which
    // permissions the static roles hold. Everything a workspace sees - the
    // menu, the grids, every `Caller::require` - hangs off that, so if this
    // moves correctly, all of it does.
    //
    // What must not move is the data. Switching an app off is a subscription
    // change, not a delete, and a workspace that lost its invoices by
    // unticking a box would be a workspace nobody unticks a box in again.
    let cfg = database_config();
    recreate(&cfg, ENABLEMENT).await;

    provision::migrate_tenant(&cfg, ENABLEMENT)
        .await
        .expect("migration pass");

    let pool = open(&cfg, ENABLEMENT);

    // A migrated database has every app's schema and no optional app on.
    let enabled = installs::enabled_ids(&pool).await.expect("enabled ids");
    assert_eq!(
        enabled,
        vec![apps::CORE_APP_ID.to_owned()],
        "only core is on before anybody installs anything",
    );
    assert!(
        !admin_grants(&pool)
            .await
            .iter()
            .any(|name| name.starts_with("Pages.Sales")),
        "an app nobody installed must not be granted",
    );

    // An invoice, to prove the data outlives the subscription.
    let party = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO core.currencies (code, is_enabled)
         VALUES ('USD', true) ON CONFLICT (code) DO NOTHING",
    )
    .execute(&pool)
    .await
    .expect("a currency to price it in");

    sqlx::query(
        "INSERT INTO books.invoices
             (party_id, party_code, party_name, issued_on, status, currency_code)
         VALUES ($1, 'ACME', 'Acme', current_date, 'draft', 'USD')",
    )
    .bind(party)
    .execute(&pool)
    .await
    .expect("write an invoice");

    // Switch Books on, the way the install use case does: enable, then sync.
    assert!(
        installs::enable(&pool, "books", "0.1.0", None)
            .await
            .expect("enable"),
        "books was off, so enabling it is a change",
    );
    role::sync_static_roles(&pool).await.expect("sync");

    let on = admin_grants(&pool).await;
    assert!(
        on.iter().any(|name| name == "Pages.Sales.Invoices.Post"),
        "installing Books has to grant its permissions",
    );

    // Enabling something already on is not a change, and must not rewrite the
    // date somebody subscribed.
    assert!(
        !installs::enable(&pool, "books", "0.1.0", None)
            .await
            .expect("enable again"),
        "a second install is a no-op, not a second subscription",
    );

    // Off again.
    assert!(
        installs::disable(&pool, "books").await.expect("disable"),
        "it was on",
    );
    role::sync_static_roles(&pool).await.expect("resync");
    role::revoke_everywhere(&pool, "Pages.Sales")
        .await
        .expect("revoke");

    let off = admin_grants(&pool).await;
    assert!(
        !off.iter().any(|name| name.starts_with("Pages.Sales")),
        "switching Books off has to take its permissions with it",
    );
    assert!(
        off.iter().any(|name| name == "Pages.Master.Parties"),
        "and must not take the neighbour's - this is why an app owns a whole          permission subtree",
    );

    // The point of the whole design.
    let invoices: i64 = sqlx::query("SELECT count(*) AS n FROM books.invoices")
        .fetch_one(&pool)
        .await
        .expect("count invoices")
        .get("n");
    assert_eq!(invoices, 1, "switching an app off must not touch its data");

    pool.close().await;

    provision::drop_tenant_database(&cfg, ENABLEMENT)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_migration_pass_gives_admin_the_permissions_this_build_added() {
    // # The bug this exists to stop
    //
    // `sync_static_roles` is documented as the upgrade path - run it again and
    // a newly defined permission reaches every workspace's Admin role. For a
    // long time the only caller was signup, so a workspace got the tree that
    // existed on the day it was created and never another entry. Sales and
    // Master shipped that way: the routes answered, the pages rendered, and
    // `Pages.Sales.Invoices` was held by nobody, so the navigation entry was
    // simply not drawn. There is no error to go looking for in that - it reads
    // as a feature that was never built.
    let cfg = database_config();
    recreate(&cfg, PERMISSIONS).await;

    provision::migrate_tenant(&cfg, PERMISSIONS)
        .await
        .expect("first migration pass");

    let pool = open(&cfg, PERMISSIONS);
    let whole = admin_grants(&pool).await;
    assert!(
        !whole.is_empty(),
        "a migrated database has an Admin role with grants",
    );

    // Stand in for the release that adds a page: before it, this database's
    // Admin held everything *except* the new entries.
    let removed: Vec<String> = whole
        .iter()
        .filter(|name| name.starts_with("Pages."))
        .take(3)
        .cloned()
        .collect();
    assert!(
        !removed.is_empty(),
        "the permission tree has page entries to drift on",
    );

    sqlx::query(
        "DELETE FROM role_permissions rp
          USING roles r
          WHERE r.id = rp.role_id
            AND lower(r.name) = 'admin'
            AND rp.name = ANY($1::text[])",
    )
    .bind(&removed)
    .execute(&pool)
    .await
    .expect("age the role back to an earlier release");

    let aged = admin_grants(&pool).await;
    for name in &removed {
        assert!(!aged.contains(name), "{name} was supposed to be removed");
    }
    pool.close().await;

    // The boot sweep, on a workspace that predates the feature.
    provision::migrate_tenant(&cfg, PERMISSIONS)
        .await
        .expect("second migration pass");

    let pool = open(&cfg, PERMISSIONS);
    assert_eq!(
        admin_grants(&pool).await,
        whole,
        "a migration pass has to put back every permission this build defines",
    );
    pool.close().await;

    provision::drop_tenant_database(&cfg, PERMISSIONS)
        .await
        .expect("clean up");
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
    assert_books_is_whole(&pool, "fresh").await;
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
    assert_books_is_whole(&pool, "fresh, twice").await;
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
    assert_books_is_whole(&pool, "relocated, then swept").await;
    assert_core_is_recorded(&pool, "relocated, then swept").await;
    pool.close().await;

    provision::drop_tenant_database(&cfg, LEGACY)
        .await
        .expect("clean up");
}

/// A database that stopped at 0013, which is how every tenant provisioned
/// before the schema move actually looks.
const STOPPED: &str = "phonix_test_schema_stopped";

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_database_that_stopped_at_0013_is_adopted_rather_than_rebuilt() {
    // The regression this file exists for now.
    //
    // sqlx names its bookkeeping table unqualified, so the moment the runner
    // creates `core` the table sqlx creates lands *there* - and a database
    // whose history is in `public` looks, to sqlx, like a database with no
    // history at all. It then re-ran 0001 onwards into `core`, built a second
    // empty copy of every table, and failed on 0014 with `relation "users"
    // already exists in schema "core"` on every boot afterwards.
    //
    // The other legacy test does not reach this, because it runs the *whole*
    // stream against a database with no `core` schema - so 0014 relocates the
    // bookkeeping itself and the next pass finds it where it belongs. Stopping
    // at 0013 is what reproduces a real tenant.
    let cfg = database_config();
    recreate(&cfg, STOPPED).await;

    // Migrations 0001-0013 only, on the search path a tenant connection uses.
    // With `core` absent, Postgres skips it and everything lands in `public`.
    let legacy = phonix_db::connect::schema_migration_pool(
        &cfg,
        STOPPED,
        phonix_db::connect::TENANT_SEARCH_PATH,
    );
    let stream = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            phonix_db::CORE_MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 13)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    stream.run(&legacy).await.expect("migrating up to 0013");

    // A row of real data, so the relocation is proved to have *moved* the
    // table rather than left the old one behind full and the new one empty.
    sqlx::query(
        "INSERT INTO public.users
             (email, display_name, first_name, last_name, password_hash, status)
         VALUES ('ada@example.test', 'Ada Lovelace', 'Ada', 'Lovelace', 'x', 'active')",
    )
    .execute(&legacy)
    .await
    .expect("a user to carry across");
    legacy.close().await;

    // This is what a pre-0014 tenant looks like, and the assertion is the
    // premise of the test rather than its conclusion.
    let pool = open(&cfg, STOPPED);
    let where_history_lives: Vec<String> =
        sqlx::query_scalar("SELECT schemaname FROM pg_tables WHERE tablename = '_sqlx_migrations'")
            .fetch_all(&pool)
            .await
            .expect("find the bookkeeping");
    assert_eq!(
        where_history_lives,
        vec!["public".to_owned()],
        "the premise: a stopped database keeps its history in public"
    );
    pool.close().await;

    // And the runner takes it from there.
    provision::migrate_tenant(&cfg, STOPPED)
        .await
        .expect("a database that stopped at 0013 migrates");

    let pool = open(&cfg, STOPPED);
    assert_core_is_whole(&pool, "stopped at 0013").await;
    assert_master_is_whole(&pool, "stopped at 0013").await;
    assert_books_is_whole(&pool, "stopped at 0013").await;
    assert_core_is_recorded(&pool, "stopped at 0013").await;

    // Core's history moved into core, and nothing was left behind in public to
    // be re-adopted on the next boot.
    //
    // Asked as two questions rather than as a list of schemas: every app has a
    // history of its own - that is the point of per-app streams - so an
    // enumeration here would fail every time an app is added, which is not what
    // this test is about.
    let where_history_lives: Vec<String> = sqlx::query_scalar(
        "SELECT schemaname FROM pg_tables
          WHERE tablename = '_sqlx_migrations'
          ORDER BY schemaname",
    )
    .fetch_all(&pool)
    .await
    .expect("find the bookkeeping");

    assert!(
        where_history_lives.iter().any(|schema| schema == "core"),
        "core has no migration history at all: {where_history_lives:?}"
    );
    assert!(
        !where_history_lives.iter().any(|schema| schema == "public"),
        "core's history was rebuilt rather than adopted, leaving one in public:          {where_history_lives:?}"
    );
    assert_eq!(
        where_history_lives.len(),
        apps::APPS.len(),
        "one history per app, and no more: {where_history_lives:?}"
    );

    // The row came with its table.
    let carried: i64 = sqlx::query_scalar("SELECT count(*) FROM core.users")
        .fetch_one(&pool)
        .await
        .expect("count the users");
    assert_eq!(carried, 1, "the relocation lost the row it was moving");

    pool.close().await;
    provision::drop_tenant_database(&cfg, STOPPED)
        .await
        .expect("clean up");
}
