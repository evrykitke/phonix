//! Document numbering, against a live PostgreSQL server.
//!
//! The pattern renderer is proved by unit tests in `phonix-core`. What cannot be
//! proved there is the only thing that actually makes a sequence gap-free:
//! **the row lock, and the fact that Postgres holds it until the transaction
//! ends.** So the two tests that matter here are the one that rolls a
//! transaction back and finds the number still available, and the one that runs
//! two allocations at once and watches the second wait.
//!
//! Ignored by default: these need a reachable server and the credentials in
//! `.env`. Run them deliberately.
//!
//! ```text
//! cargo test -p phonix-db --test numbering -- --ignored --test-threads=1
//! ```

use std::time::Duration;

use chrono::NaiveDate;
use phonix_config::DatabaseConfig;
use phonix_core::numbering::{Pattern, ResetPeriod};
use phonix_db::error::DbError;
use phonix_db::numbering::{self, SequenceDefinition, SequenceKey, SequenceUpdate};
use phonix_db::sqlx::{self, PgPool};
use phonix_db::tenancy::provision;

const DATABASE: &str = "phonix_test_numbering";
/// A made-up app, and it has to stay made up.
///
/// This was `"books"` until `books` became a real app with a real series in
/// `config/numbering/books.toml` - at which point the runner installed that
/// series into every scratch database and this file's "how many sequences does
/// this app own" assertion started counting three where it expected two.
///
/// `app_id` is deliberately not a foreign key to `installed_apps` (see
/// migration 0016), so a fictional one works perfectly here. It just must not
/// collide with an app that exists.
const APP: &str = "fixtures";
const DOC: &str = "invoice";

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
}

fn day(year: i32, month: u32, of: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
}

fn pattern(raw: &str) -> Pattern {
    Pattern::parse(raw).expect("a valid pattern")
}

/// A freshly created tenant database, migrated to the current schema.
async fn fresh(cfg: &DatabaseConfig) -> PgPool {
    provision::drop_tenant_database(cfg, DATABASE)
        .await
        .expect("drop scratch database");

    let mut conn = phonix_db::maintenance_connection(cfg)
        .await
        .expect("maintenance connection");
    let sql = format!(r#"CREATE DATABASE "{DATABASE}" ENCODING 'UTF8'"#);
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&mut conn)
        .await
        .expect("create scratch database");

    provision::migrate_tenant(cfg, DATABASE)
        .await
        .expect("migrate scratch database");

    phonix_db::tenant_pool(cfg, DATABASE)
}

/// Install one sequence, the way an app's installer would.
async fn install(pool: &PgPool, raw: &str, reset: ResetPeriod, start_at: i64) {
    let format = pattern(raw);
    numbering::install_defaults(
        pool,
        &[SequenceDefinition {
            key: SequenceKey::new(APP, DOC),
            pattern: &format,
            reset_period: reset,
            start_at,
        }],
    )
    .await
    .expect("install the sequence");
}

/// Allocate and commit, the way a document save would.
async fn take(pool: &PgPool, on: NaiveDate) -> String {
    let mut tx = pool.begin().await.expect("begin");
    let allocated = numbering::allocate(&mut tx, SequenceKey::new(APP, DOC), on, 1)
        .await
        .expect("allocate");
    tx.commit().await.expect("commit");
    allocated.number
}

async fn clean_up(cfg: &DatabaseConfig, pool: PgPool) {
    pool.close().await;
    provision::drop_tenant_database(cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_grouped_hash_mask_survives_the_column_and_comes_back_grouped() {
    // The renderer is proved in `phonix-core`. What is proved here is that the
    // column's CHECK accepts the `#` spelling at all - migration 0016 accepted
    // only `{N...}`, and a mask that parses in Rust and is refused by Postgres
    // fails at install time, on a customer's deployment, for no visible reason.
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    install(&pool, "#-#####-####", ResetPeriod::Never, 1).await;

    assert_eq!(take(&pool, day(2026, 8, 25)).await, "0-00000-0001");
    assert_eq!(take(&pool, day(2026, 8, 25)).await, "0-00000-0002");

    // And the other half of 0018: one spelling or the other, never both.
    let mixed =
        sqlx::query("INSERT INTO number_sequences (app_id, doc_type, pattern) VALUES ($1, $2, $3)")
            .bind(APP)
            .bind("mixed")
            .bind("INV #{NNNNN}")
            .execute(&pool)
            .await;
    assert!(
        mixed.is_err(),
        "a mixed mask reads as one width and renders another; the column has to refuse it"
    );

    pool.close().await;
    provision::drop_tenant_database(&cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn numbers_come_out_in_order_in_the_format_that_was_installed() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "INV-{YYYY}-{NNNNNN}", ResetPeriod::Yearly, 1).await;

    let mut issued = Vec::new();
    for _ in 0..3 {
        issued.push(take(&pool, day(2026, 8, 24)).await);
    }

    assert_eq!(
        issued,
        ["INV-2026-000001", "INV-2026-000002", "INV-2026-000003"]
    );

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_rolled_back_save_returns_the_number_instead_of_burning_it() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "INV-{NNNN}", ResetPeriod::Never, 1).await;

    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-0001");

    // A save that fails after the number was taken. This is the case a
    // Postgres SEQUENCE gets wrong: `nextval()` is non-transactional, so 0002
    // would be gone for ever and the sequence would have a hole in it - which
    // is an audit finding in Italy, Spain, Portugal, Poland and India.
    let mut tx = pool.begin().await.expect("begin");
    let doomed = numbering::allocate(&mut tx, SequenceKey::new(APP, DOC), day(2026, 8, 24), 1)
        .await
        .expect("allocate");
    assert_eq!(doomed.number, "INV-0002");
    tx.rollback().await.expect("roll back");

    // And 0002 is still there to be issued.
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-0002");
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-0003");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_second_allocation_waits_for_the_first_to_commit() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "INV-{NNNN}", ResetPeriod::Never, 1).await;

    // Transaction A takes a number and holds it, as a document save does while
    // it writes its lines.
    let mut first = pool.begin().await.expect("begin A");
    let taken = numbering::allocate(&mut first, SequenceKey::new(APP, DOC), day(2026, 8, 24), 1)
        .await
        .expect("allocate in A");
    assert_eq!(taken.number, "INV-0001");

    // Transaction B asks for one at the same moment.
    let second = pool.clone();
    let waiting = tokio::spawn(async move {
        let mut tx = second.begin().await.expect("begin B");
        let allocated =
            numbering::allocate(&mut tx, SequenceKey::new(APP, DOC), day(2026, 8, 24), 1)
                .await
                .expect("allocate in B");
        tx.commit().await.expect("commit B");
        allocated.number
    });

    // It has to block. If it did not, the two would race for the same counter
    // and one of these documents would be issued a number the other already
    // has - the failure a unique index in the app's own table would catch, but
    // only after somebody had already been shown a duplicate invoice number.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !waiting.is_finished(),
        "the second allocation did not wait for the first"
    );

    first.commit().await.expect("commit A");

    let number = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the second allocation should proceed once the first commits")
        .expect("the waiting task panicked");
    assert_eq!(number, "INV-0002");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_period_boundary_resets_the_counter_without_anything_running_at_midnight() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "{YYYY}{MM}-{NNN}", ResetPeriod::Monthly, 1).await;

    assert_eq!(take(&pool, day(2026, 8, 20)).await, "202608-001");
    assert_eq!(take(&pool, day(2026, 8, 31)).await, "202608-002");

    // First document of September. The reset happens because the period key
    // stopped matching, inside the same statement that issued the number.
    assert_eq!(take(&pool, day(2026, 9, 1)).await, "202609-001");
    assert_eq!(take(&pool, day(2026, 9, 2)).await, "202609-002");

    // A document backdated into August carries August's tokens - and August's
    // counter, which has moved on. The number and the period it prints agree,
    // which is the point of taking the document's date rather than today's.
    assert_eq!(take(&pool, day(2026, 8, 15)).await, "202608-001");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_fiscal_year_reset_follows_the_organizations_own_year() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "FY{FY}/{NNN}", ResetPeriod::FiscalYear, 1).await;

    // April opening, so 31 March 2027 is still FY2026 and 1 April 2027 is not.
    let april = 4u8;
    let number = |on: NaiveDate| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.expect("begin");
            let allocated = numbering::allocate(&mut tx, SequenceKey::new(APP, DOC), on, april)
                .await
                .expect("allocate");
            tx.commit().await.expect("commit");
            allocated.number
        }
    };

    assert_eq!(number(day(2026, 4, 1)).await, "FY2026/001");
    assert_eq!(number(day(2027, 3, 31)).await, "FY2026/002");
    assert_eq!(number(day(2027, 4, 1)).await, "FY2027/001");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_sequence_starts_where_it_was_told_to() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    // A fresh row has counter 0. Incrementing it would issue 1 and quietly
    // ignore the setting - which matters for a workspace migrating off another
    // system, where the whole reason for the setting is not to reuse numbers
    // that are already out.
    install(&pool, "INV-{NNNNN}", ResetPeriod::Never, 5_000).await;

    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-05000");
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-05001");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn raising_start_at_moves_the_sequence_on_and_lowering_it_does_nothing() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "INV-{NNNNN}", ResetPeriod::Never, 1).await;

    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-00001");

    let format = pattern("INV-{NNNNN}");
    let moved = numbering::update(
        &pool,
        SequenceKey::new(APP, DOC),
        SequenceUpdate {
            pattern: &format,
            reset_period: ResetPeriod::Never,
            start_at: 9_000,
            is_active: true,
            updated_by: None,
        },
    )
    .await
    .expect("raise start_at");
    assert!(moved);

    // Raising it past the counter is the supported way to jump a sequence.
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-09000");
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-09001");

    // Lowering it changes nothing while the period runs: the counter is
    // already ahead, so it carries on rather than reissuing numbers that are
    // already out.
    numbering::update(
        &pool,
        SequenceKey::new(APP, DOC),
        SequenceUpdate {
            pattern: &format,
            reset_period: ResetPeriod::Never,
            start_at: 1,
            is_active: true,
            updated_by: None,
        },
    )
    .await
    .expect("lower start_at");

    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-09002");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_missing_or_switched_off_sequence_refuses_rather_than_inventing_a_number() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    // Nothing installed at all.
    let mut tx = pool.begin().await.expect("begin");
    let refused =
        numbering::allocate(&mut tx, SequenceKey::new(APP, DOC), day(2026, 8, 24), 1).await;
    assert!(
        matches!(refused, Err(DbError::UnusableSequence { .. })),
        "got {refused:?}"
    );
    tx.rollback().await.expect("roll back");

    install(&pool, "INV-{NNNN}", ResetPeriod::Never, 1).await;
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-0001");

    let format = pattern("INV-{NNNN}");
    numbering::update(
        &pool,
        SequenceKey::new(APP, DOC),
        SequenceUpdate {
            pattern: &format,
            reset_period: ResetPeriod::Never,
            start_at: 1,
            is_active: false,
            updated_by: None,
        },
    )
    .await
    .expect("switch the sequence off");

    // Switched off means documents stop, not that numbering carries on quietly.
    let mut tx = pool.begin().await.expect("begin");
    let refused =
        numbering::allocate(&mut tx, SequenceKey::new(APP, DOC), day(2026, 8, 24), 1).await;
    assert!(
        matches!(refused, Err(DbError::UnusableSequence { .. })),
        "got {refused:?}"
    );
    tx.rollback().await.expect("roll back");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn reinstalling_an_app_does_not_undo_what_the_tenant_edited() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    install(&pool, "INV-{NNNN}", ResetPeriod::Never, 1).await;

    assert_eq!(take(&pool, day(2026, 8, 24)).await, "INV-0001");

    let tenants_own = pattern("ACME/{YYYY}/{NNNNNN}");
    numbering::update(
        &pool,
        SequenceKey::new(APP, DOC),
        SequenceUpdate {
            pattern: &tenants_own,
            reset_period: ResetPeriod::Yearly,
            start_at: 1,
            is_active: true,
            updated_by: None,
        },
    )
    .await
    .expect("the tenant edits the format");

    // An upgrade re-runs the installer with the shipped default.
    let inserted = {
        let shipped = pattern("INV-{NNNN}");
        numbering::install_defaults(
            &pool,
            &[SequenceDefinition {
                key: SequenceKey::new(APP, DOC),
                pattern: &shipped,
                reset_period: ResetPeriod::Never,
                start_at: 1,
            }],
        )
        .await
        .expect("reinstall")
    };
    assert_eq!(inserted, 0, "reinstalling should insert nothing");

    let row = numbering::find(&pool, SequenceKey::new(APP, DOC))
        .await
        .expect("read the sequence back")
        .expect("it is still there");
    assert_eq!(
        row.pattern, tenants_own,
        "the shipped default overwrote the edit"
    );
    assert_eq!(row.reset_period, ResetPeriod::Yearly);
    assert_eq!(
        row.counter, 1,
        "reinstalling reset a counter that had issued"
    );

    // A default is what a workspace starts with, not what it is held to.
    //
    // Note the counter: 000001, not 000002. Switching from `Never` to `Yearly`
    // changed which period the sequence is running in, and a new period opens
    // at `start_at` - which is the whole point of a reset period and is exactly
    // the risk `numbering::update` warns its caller about. Nothing collides
    // here because the shape changed too, but a tenant editing the period while
    // keeping the pattern would reissue numbers that are already out. That
    // judgement belongs to the service layer, which either raises `start_at`
    // past the highest issued number or refuses the edit.
    assert_eq!(take(&pool, day(2026, 8, 24)).await, "ACME/2026/000001");

    clean_up(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn scoped_sequences_count_independently() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let format = pattern("{SCOPE}-{NNNN}");
    numbering::install_defaults(
        &pool,
        &[
            SequenceDefinition {
                key: SequenceKey::new(APP, DOC).scoped("NBO"),
                pattern: &format,
                reset_period: ResetPeriod::Never,
                start_at: 1,
            },
            SequenceDefinition {
                key: SequenceKey::new(APP, DOC).scoped("MBA"),
                pattern: &format,
                reset_period: ResetPeriod::Never,
                start_at: 1,
            },
        ],
    )
    .await
    .expect("install two branches");

    let branch = |scope: &'static str| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.expect("begin");
            let allocated = numbering::allocate(
                &mut tx,
                SequenceKey::new(APP, DOC).scoped(scope),
                day(2026, 8, 24),
                1,
            )
            .await
            .expect("allocate");
            tx.commit().await.expect("commit");
            allocated.number
        }
    };

    assert_eq!(branch("NBO").await, "NBO-0001");
    assert_eq!(branch("NBO").await, "NBO-0002");
    // A second branch is a second row, so it does not inherit the first's
    // count - and neither of them blocks the other, which is the reason to
    // scope a sequence rather than share one.
    assert_eq!(branch("MBA").await, "MBA-0001");
    assert_eq!(branch("NBO").await, "NBO-0003");

    // Two rows, one per branch - and asked for this app alone, because a
    // scratch database also carries whatever series the real apps installed.
    let all = numbering::list(&pool, Some(APP)).await.expect("list");
    assert_eq!(all.len(), 2, "expected one sequence per branch, got {all:?}");

    clean_up(&cfg, pool).await;
}
