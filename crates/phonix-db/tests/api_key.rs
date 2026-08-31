//! API keys against a live PostgreSQL server.
//!
//! What is worth a live test here is not the Rust - the narrowing of a key's
//! permissions is unit-tested in `phonix-services` - but **the `WHERE` clause
//! that decides whether a key is alive**. Revocation and expiry are enforced in
//! SQL precisely so a caller cannot forget them, and the only way to prove that
//! is against a server that runs the statement.
//!
//! It also proves migration 0020 applies to a workspace built from nothing.
//!
//! Ignored by default: these need a reachable server and the credentials in
//! `.env`. Run them deliberately.
//!
//! ```text
//! cargo test -p phonix-db --test api_key -- --ignored --test-threads=1
//! ```

use chrono::{Duration, Utc};
use phonix_config::DatabaseConfig;
use phonix_core::identity::{UserId, UserStatus};
use phonix_core::query::PageRequest;
use phonix_db::identity::{NewApiKey, api_key, user};
use phonix_db::sqlx::{self, PgPool};
use phonix_db::tenancy::provision;

const DATABASE: &str = "phonix_test_api_key";

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
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

async fn owner(pool: &PgPool, email: &str) -> UserId {
    user::create(
        pool,
        phonix_db::identity::NewUser {
            email,
            first_name: "Ada",
            last_name: "Lovelace",
            // A password of some shape, because `users_password_or_pending`
            // refuses an active account without one. Nothing here verifies it;
            // this layer only ever stores what `phonix-services` hashed.
            password_hash: Some("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$notarealhash"),
            status: UserStatus::Active,
            is_owner: false,
            invited_by: None,
        },
    )
    .await
    .expect("create the owner of the key")
    .id
}

/// A digest, of the shape the column's CHECK constraint insists on.
///
/// Not a real SHA-256 - this layer never sees a token, so a test at this layer
/// has nothing to hash. Thirty-two bytes is what matters.
fn digest(seed: u8) -> Vec<u8> {
    vec![seed; 32]
}

fn key_named<'a>(
    user_id: UserId,
    name: &'a str,
    hash: &'a [u8],
    scopes: &'a [String],
) -> NewApiKey<'a> {
    NewApiKey {
        user_id,
        name,
        token_hash: hash,
        token_hint: "wxyz",
        scopes,
        expires_at: None,
        created_by: Some(user_id),
    }
}

async fn clean_up(pool: PgPool, cfg: &DatabaseConfig) {
    pool.close().await;
    provision::drop_tenant_database(cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_key_is_found_by_its_digest_until_it_is_revoked() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = owner(&pool, "ada@example.com").await;

    let scopes = vec!["Pages.Administration.Settings".to_owned()];
    let hash = digest(1);

    let created = api_key::create(&pool, key_named(user_id, "nightly export", &hash, &scopes))
        .await
        .expect("create the key");

    assert_eq!(created.scopes, scopes);
    assert!(created.last_used_at.is_none());
    assert!(created.is_live(Utc::now()));

    let found = api_key::find_live_by_hash(&pool, &hash)
        .await
        .expect("look the key up")
        .expect("a live key");
    assert_eq!(found.id, created.id);

    assert!(
        api_key::revoke(&pool, created.id, "no longer needed", Some(user_id))
            .await
            .expect("revoke")
    );

    // The point of the whole test: liveness is in the statement, so a caller
    // that never checks `revoked_at` still cannot use a stopped key.
    assert!(
        api_key::find_live_by_hash(&pool, &hash)
            .await
            .expect("look the revoked key up")
            .is_none()
    );

    // The row survives, so "who issued this and who stopped it" outlives the
    // credential.
    let kept = api_key::find_by_id(&pool, created.id)
        .await
        .expect("read by id")
        .expect("the row is kept");
    assert!(kept.revoked_at.is_some());
    assert_eq!(kept.revoked_reason.as_deref(), Some("no longer needed"));

    // Revoking twice is not an error worth raising, but it is not a second
    // revocation either.
    assert!(
        !api_key::revoke(&pool, created.id, "again", Some(user_id))
            .await
            .expect("revoke again")
    );

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn an_expired_key_is_already_gone_without_anybody_sweeping() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = owner(&pool, "ada@example.com").await;

    let hash = digest(2);
    let mut key = key_named(user_id, "expired yesterday", &hash, &[]);
    key.expires_at = Some(Utc::now() - Duration::hours(1));

    let created = api_key::create(&pool, key).await.expect("create the key");
    assert!(!created.is_live(Utc::now()));

    assert!(
        api_key::find_live_by_hash(&pool, &hash)
            .await
            .expect("look the expired key up")
            .is_none(),
        "an expired key must not be resurrectable by a lookup that forgot to check"
    );

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn use_is_recorded_once_and_then_left_alone() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = owner(&pool, "ada@example.com").await;

    let hash = digest(3);
    let created = api_key::create(&pool, key_named(user_id, "phone", &hash, &[]))
        .await
        .expect("create the key");

    api_key::touch_last_used(&pool, created.id)
        .await
        .expect("first use");

    let first = api_key::find_by_id(&pool, created.id)
        .await
        .expect("read")
        .expect("the key")
        .last_used_at
        .expect("first use is recorded");

    // The second call inside the resolution window is deliberately a no-op:
    // this column answers "is anything still using this key", and paying for a
    // row update per request to answer it precisely is not worth it.
    api_key::touch_last_used(&pool, created.id)
        .await
        .expect("second use");

    let second = api_key::find_by_id(&pool, created.id)
        .await
        .expect("read again")
        .expect("the key")
        .last_used_at
        .expect("still recorded");

    assert_eq!(first, second);

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn the_list_narrows_to_live_or_to_stopped() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = owner(&pool, "ada@example.com").await;

    let live_hash = digest(4);
    api_key::create(&pool, key_named(user_id, "live one", &live_hash, &[]))
        .await
        .expect("create the live key");

    let revoked_hash = digest(5);
    let revoked = api_key::create(&pool, key_named(user_id, "stopped one", &revoked_hash, &[]))
        .await
        .expect("create the second key");
    api_key::revoke(&pool, revoked.id, "test", Some(user_id))
        .await
        .expect("revoke");

    let expired_hash = digest(6);
    let mut expired = key_named(user_id, "expired one", &expired_hash, &[]);
    expired.expires_at = Some(Utc::now() - Duration::minutes(1));
    api_key::create(&pool, expired).await.expect("create third");

    let all = api_key::page(&pool, &PageRequest::default())
        .await
        .expect("list them all");
    assert_eq!(all.total, 3);
    // The owner's name comes back with the row, so the screen does not have to
    // ask a second time per row.
    assert!(all.rows.iter().all(|row| row.owner_name == "Ada Lovelace"));

    let live = api_key::page(&pool, &PageRequest::default().filtered_by("revoked", "live"))
        .await
        .expect("list the live ones");
    assert_eq!(live.total, 1);
    assert_eq!(live.rows[0].key.name, "live one");

    // Revoked and expired are one answer to somebody tidying up: both are keys
    // that no longer work.
    let stopped = api_key::page(
        &pool,
        &PageRequest::default().filtered_by("revoked", "revoked"),
    )
    .await
    .expect("list the stopped ones");
    assert_eq!(stopped.total, 2);

    let searched = api_key::page(
        &pool,
        &PageRequest {
            search: "stopped".to_owned(),
            ..PageRequest::default()
        },
    )
    .await
    .expect("search by name");
    assert_eq!(searched.total, 1);

    clean_up(pool, &cfg).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn deleting_the_owner_takes_their_keys_with_them() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let user_id = owner(&pool, "ada@example.com").await;

    let hash = digest(7);
    api_key::create(&pool, key_named(user_id, "hers", &hash, &[]))
        .await
        .expect("create the key");

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("hard-delete the account");

    // Not tidiness: a key whose owner is gone has no permissions to intersect
    // against, and leaving one that still authenticates would be a credential
    // nobody can reason about.
    assert!(
        api_key::find_live_by_hash(&pool, &hash)
            .await
            .expect("look it up")
            .is_none()
    );

    clean_up(pool, &cfg).await;
}
