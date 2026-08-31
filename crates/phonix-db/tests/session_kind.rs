//! Mobile sessions against a live PostgreSQL server.
//!
//! What is worth a live test here is not the Rust - `windows` is unit-tested in
//! the repository itself - but **the two things only a server can answer**:
//!
//! * that migration 0021 applies to a workspace built from nothing, and that
//!   its `sessions_kind_known` constraint is really there;
//! * that `touch` slides a phone's deadline by the *mobile* idle window. That
//!   choice is made in SQL, from the row's own kind, because the caller holds
//!   only a token and does not yet know what kind it is. A `CASE` in a
//!   statement is exactly the sort of thing that type-checks while being
//!   wrong, and nothing but a server will say so.
//!
//! See `docs/adr/0003-mobile-authentication.md`.
//!
//! Ignored by default: these need a reachable server and the credentials in
//! `.env`. Run them deliberately.
//!
//! ```text
//! cargo test -p phonix-db --test session_kind -- --ignored --test-threads=1
//! ```

use phonix_config::{DatabaseConfig, MobileSessionConfig, SameSitePolicy, SessionConfig};
use phonix_core::identity::{SessionKind, UserId, UserStatus};
use phonix_db::identity::session::{self, ClientFacts};
use phonix_db::identity::user;
use phonix_db::sqlx::{self, PgPool};
use phonix_db::tenancy::provision;

const DATABASE: &str = "phonix_test_session_kind";

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
}

/// Deliberately unlike the shipped defaults, and far apart from each other, so
/// a deadline computed from the wrong block is off by weeks rather than by an
/// amount a clock skew could explain.
fn config() -> SessionConfig {
    SessionConfig {
        cookie_name: "phonix_session".into(),
        idle_timeout_mins: 60,
        absolute_timeout_hours: 24,
        remember_me_days: 30,
        secure: false,
        same_site: SameSitePolicy::Lax,
        handoff_ttl_secs: 120,
        purge_interval_mins: 60,
        mobile: MobileSessionConfig {
            idle_timeout_mins: 60 * 24 * 30,
            absolute_timeout_days: 90,
        },
    }
}

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

async fn account(pool: &PgPool) -> UserId {
    user::create(
        pool,
        phonix_db::identity::NewUser {
            email: "ada@example.com",
            first_name: "Ada",
            last_name: "Lovelace",
            password_hash: Some("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$notarealhash"),
            status: UserStatus::Active,
            is_owner: false,
            invited_by: None,
        },
    )
    .await
    .expect("create the account the sessions belong to")
    .id
}

/// A digest of the shape the column's CHECK constraint insists on. This layer
/// never sees a token, so there is nothing here to hash.
fn digest(seed: u8) -> Vec<u8> {
    vec![seed; 32]
}

async fn clean_up(pool: PgPool, cfg: &DatabaseConfig) {
    pool.close().await;
    provision::drop_tenant_database(cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_phone_and_a_browser_get_different_deadlines_and_keep_them() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = account(&pool).await;
    let session_cfg = config();

    let browser = session::create(
        &pool,
        user_id,
        &digest(1),
        &session_cfg,
        SessionKind::Browser,
        false,
        true,
        ClientFacts::default(),
    )
    .await
    .expect("open a browser session");

    let mobile = session::create(
        &pool,
        user_id,
        &digest(2),
        &session_cfg,
        SessionKind::Mobile,
        false,
        true,
        ClientFacts::default(),
    )
    .await
    .expect("open a mobile session");

    assert_eq!(browser.kind, SessionKind::Browser);
    assert_eq!(mobile.kind, SessionKind::Mobile);

    // A day against ninety. If these ever come out equal, the kind is not
    // reaching the deadline arithmetic.
    assert!(
        mobile.absolute_expires_at > browser.absolute_expires_at,
        "a phone's ceiling should be far beyond a browser's"
    );
    assert!((mobile.absolute_expires_at - browser.absolute_expires_at).num_days() >= 88);

    // The kind survives a round trip through the reader that every request
    // uses - which is the one that has to get it right.
    let read = session::find(&pool, &digest(2))
        .await
        .expect("read the mobile session")
        .expect("it is live");
    assert_eq!(read.kind, SessionKind::Mobile);

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn touch_slides_each_kind_by_its_own_idle_window() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = account(&pool).await;
    let session_cfg = config();

    for (seed, kind) in [(1u8, SessionKind::Browser), (2u8, SessionKind::Mobile)] {
        session::create(
            &pool,
            user_id,
            &digest(seed),
            &session_cfg,
            kind,
            false,
            true,
            ClientFacts::default(),
        )
        .await
        .expect("open a session");
    }

    let browser = session::touch(&pool, &digest(1), &session_cfg)
        .await
        .expect("touch the browser session")
        .expect("it is live");
    let mobile = session::touch(&pool, &digest(2), &session_cfg)
        .await
        .expect("touch the mobile session")
        .expect("it is live");

    // An hour against thirty days - and this is the assertion the `CASE` in
    // that statement exists for. A `touch` that read the browser's window for
    // both would sign a phone out on a browser's schedule, and every unit test
    // in the repository would still pass.
    assert!(
        (mobile.expires_at - browser.expires_at).num_days() >= 28,
        "the mobile session should have slid by its own window, not the browser's"
    );

    // Neither may pass its own ceiling: activity extends a session, it does not
    // make one immortal.
    assert!(browser.expires_at <= browser.absolute_expires_at);
    assert!(mobile.expires_at <= mobile.absolute_expires_at);

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn the_database_refuses_a_kind_the_application_does_not_know() {
    // The Rust side cannot write one - `SessionKind` is a closed enum - so this
    // is about the constraint being genuinely present rather than about a path
    // through the application. A missing CHECK is invisible until somebody
    // writes to this table by hand, and then it is a row nothing can read.
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = account(&pool).await;

    let refused = sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, kind, expires_at, absolute_expires_at)
         VALUES ($1, $2, 'desktop', now() + interval '1 hour', now() + interval '1 day')",
    )
    .bind(user_id)
    .bind(digest(3))
    .execute(&pool)
    .await;

    assert!(
        refused.is_err(),
        "sessions_kind_known should have refused 'desktop'"
    );

    // And the two the application does know are accepted, so the constraint is
    // not simply refusing everything.
    for kind in [SessionKind::Browser, SessionKind::Mobile] {
        sqlx::query(
            "INSERT INTO sessions (user_id, token_hash, kind, expires_at, absolute_expires_at)
             VALUES ($1, $2, $3, now() + interval '1 hour', now() + interval '1 day')",
        )
        .bind(user_id)
        .bind(digest(if kind == SessionKind::Browser { 4 } else { 5 }))
        .bind(kind.as_str())
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("{kind} should be accepted: {err}"));
    }

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_session_that_predates_the_migration_reads_as_a_browser() {
    // The column has a default rather than a backfill, and this is what that
    // default means: a row written without naming a kind - which is every row
    // that existed before 0021 - is a browser's.
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = account(&pool).await;

    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, expires_at, absolute_expires_at)
         VALUES ($1, $2, now() + interval '1 hour', now() + interval '1 day')",
    )
    .bind(user_id)
    .bind(digest(6))
    .execute(&pool)
    .await
    .expect("insert a session the way 0002 would have");

    let found = session::find(&pool, &digest(6))
        .await
        .expect("read it back")
        .expect("it is live");

    assert_eq!(found.kind, SessionKind::Browser);

    clean_up(pool, &cfg).await;
}
