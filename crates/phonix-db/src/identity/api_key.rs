//! The `api_keys` table: bearer credentials for `/api/v1`.
//!
//! The same rule as [`session`](super::session) and
//! [`one_time_token`](super::one_time_token): **every function here takes a
//! digest**, never a token. Minting one, prefixing it and digesting it are the
//! application layer's job, so no repository in this crate ever holds a
//! credential a client could present.
//!
//! Liveness is decided in SQL rather than in Rust. A key is live when it has
//! not been revoked and has not expired, and both halves are in the `WHERE`
//! clause of the lookup - so a caller that forgets to check cannot resurrect a
//! dead key, and a purge is housekeeping rather than a correctness requirement.
//!
//! What this table does *not* hold is what the key may do. The scopes narrow
//! the owner's permissions; the permissions themselves are resolved from
//! `role_permissions` and `user_permissions` on every request. A copy here
//! would be a second answer to a question `core` already answers, and it would
//! be the stale one - see docs/adr/0002-public-api.md.

use chrono::{DateTime, Utc};
use phonix_core::identity::UserId;
use phonix_core::query::{Page, PageRequest};
use sqlx::{AssertSqlSafe, FromRow, PgExecutor, PgPool, Row};
use uuid::Uuid;

use crate::error::DbError;

/// A key row, without anything anyone could present.
#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: UserId,
    pub name: String,
    /// The last four characters of the token, for telling two keys apart.
    pub token_hint: String,
    /// Permission names this key is narrowed to. Empty means it reaches only
    /// what is ungated.
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_reason: Option<String>,
}

impl ApiKeyRecord {
    /// Whether this key would be accepted right now.
    ///
    /// The lookup already filters on both conditions; this is for a screen
    /// deciding what to draw beside a row it has in hand.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_none_or(|expiry| expiry > now)
    }
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for ApiKeyRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            name: row.try_get("name")?,
            token_hint: row.try_get("token_hint")?,
            scopes: row.try_get("scopes")?,
            created_at: row.try_get("created_at")?,
            created_by: row.try_get("created_by")?,
            expires_at: row.try_get("expires_at")?,
            last_used_at: row.try_get("last_used_at")?,
            revoked_at: row.try_get("revoked_at")?,
            revoked_reason: row.try_get("revoked_reason")?,
        })
    }
}

/// Everything a new key needs, so issuing one is a single parameter.
#[derive(Debug, Clone)]
pub struct NewApiKey<'a> {
    /// The account the key acts as.
    pub user_id: UserId,
    pub name: &'a str,
    /// SHA-256 of the token the caller has already minted.
    pub token_hash: &'a [u8],
    pub token_hint: &'a str,
    pub scopes: &'a [String],
    pub expires_at: Option<DateTime<Utc>>,
    /// Who issued it. Usually the same person as `user_id`.
    pub created_by: Option<UserId>,
}

const COLUMNS: &str = "id, user_id, name, token_hint, scopes, created_at, created_by, \
     expires_at, last_used_at, revoked_at, revoked_reason";

/// Record a freshly minted key.
pub async fn create<'e, E>(executor: E, key: NewApiKey<'_>) -> Result<ApiKeyRecord, DbError>
where
    E: PgExecutor<'e>,
{
    let statement = AssertSqlSafe(format!(
        "INSERT INTO api_keys (user_id, name, token_hash, token_hint, scopes, expires_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING {COLUMNS}"
    ));

    sqlx::query_as::<_, ApiKeyRecord>(statement)
        .bind(key.user_id)
        .bind(key.name)
        .bind(key.token_hash)
        .bind(key.token_hint)
        .bind(key.scopes)
        .bind(key.expires_at)
        .bind(key.created_by)
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)
}

/// The key behind a presented token, if it is one this workspace will accept.
///
/// Revocation and expiry are both in the `WHERE` clause, so a dead key is
/// indistinguishable from an unknown one here - which is also the right answer
/// to give a caller: "revoked" and "never existed" tell somebody probing for
/// tokens two different things, and only one of them is their business.
pub async fn find_live_by_hash<'e, E>(
    executor: E,
    token_hash: &[u8],
) -> Result<Option<ApiKeyRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    let statement = AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM api_keys
          WHERE token_hash = $1
            AND revoked_at IS NULL
            AND (expires_at IS NULL OR expires_at > now())"
    ));

    sqlx::query_as::<_, ApiKeyRecord>(statement)
        .bind(token_hash)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

/// One key by id, live or not.
pub async fn find_by_id<'e, E>(executor: E, id: Uuid) -> Result<Option<ApiKeyRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    let statement = AssertSqlSafe(format!("SELECT {COLUMNS} FROM api_keys WHERE id = $1"));

    sqlx::query_as::<_, ApiKeyRecord>(statement)
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

/// How stale `last_used_at` is allowed to get before a request writes it.
///
/// A write per request would put every API call behind a row update, and the
/// question the column answers - "is anything still using this key" - is not a
/// question anybody asks to the minute.
const LAST_USED_RESOLUTION_MINUTES: i64 = 5;

/// Note that a key was used, at most once every few minutes.
///
/// Best-effort by construction: the `WHERE` clause is what makes it cheap, and
/// a caller that ignores the result is behaving correctly. A failure here must
/// never fail the request it is attached to.
pub async fn touch_last_used<'e, E>(executor: E, id: Uuid) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE api_keys
            SET last_used_at = now()
          WHERE id = $1
            AND (last_used_at IS NULL
                 OR last_used_at < now() - ($2::bigint * interval '1 minute'))",
    )
    .bind(id)
    .bind(LAST_USED_RESOLUTION_MINUTES)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Which columns a list may be sorted by, and what they are in SQL.
///
/// The same rule as every other listing: a sort field arrives from a screen, so
/// it is matched against literals this file wrote rather than interpolated.
const SORTABLE: &[(&str, &str)] = &[
    ("name", "k.name"),
    ("created_at", "k.created_at"),
    ("last_used_at", "k.last_used_at"),
    ("expires_at", "k.expires_at"),
];

/// One page of the workspace's keys, with their owners' names.
///
/// `revoked` is a named filter carrying `live` or `revoked`; anything else - a
/// stale value from an older build of the screen - narrows nothing.
pub async fn page(pool: &PgPool, request: &PageRequest) -> Result<Page<ApiKeyListing>, DbError> {
    let request = request.sanitised();
    let needle = request
        .needle()
        .map(|needle| format!("%{}%", escape_like(&needle)));

    let live = match request.filter("revoked") {
        Some("live") => Some(true),
        Some("revoked") => Some(false),
        _ => None,
    };

    const WHERE: &str = "WHERE ($1::text IS NULL OR k.name ILIKE $1 OR u.display_name ILIKE $1)
                           AND ($2::bool IS NULL
                                OR ($2 = TRUE  AND k.revoked_at IS NULL
                                    AND (k.expires_at IS NULL OR k.expires_at > now()))
                                OR ($2 = FALSE AND (k.revoked_at IS NOT NULL
                                    OR (k.expires_at IS NOT NULL AND k.expires_at <= now()))))";

    // Inner join, not left: `user_id` is `NOT NULL` and cascades, so a key
    // without an owner is a row that cannot exist.
    let counting = AssertSqlSafe(format!(
        "SELECT count(*) FROM api_keys AS k JOIN users AS u ON u.id = k.user_id {WHERE}"
    ));

    let total: i64 = sqlx::query_scalar(counting)
        .bind(needle.as_deref())
        .bind(live)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

    let total = u64::try_from(total).unwrap_or(0);
    let request = request.clamped_to(total);

    let order = match &request.sort {
        Some(sort) => SORTABLE
            .iter()
            .find(|(field, _)| *field == sort.field)
            .map(|(_, column)| format!("{column} {}", sort.direction.sql())),
        None => None,
    }
    // Newest first, and `id` after it whatever the sort: two keys issued in the
    // same millisecond would otherwise swap places between one page and the
    // next, which reads as a row appearing twice.
    .unwrap_or_else(|| "k.created_at DESC".to_owned());

    let selecting = AssertSqlSafe(format!(
        "SELECT k.id, k.user_id, k.name, k.token_hint, k.scopes, k.created_at, k.created_by,
                k.expires_at, k.last_used_at, k.revoked_at, k.revoked_reason,
                u.display_name AS owner_name
           FROM api_keys AS k
           JOIN users AS u ON u.id = k.user_id
           {WHERE}
          ORDER BY {order}, k.id DESC
          LIMIT $3 OFFSET $4"
    ));

    let rows = sqlx::query(selecting)
        .bind(needle.as_deref())
        .bind(live)
        .bind(request.limit() as i64)
        .bind(request.offset() as i64)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;

    let listings = rows
        .iter()
        .map(|row| {
            Ok(ApiKeyListing {
                key: ApiKeyRecord::from_row(row)?,
                owner_name: row.try_get("owner_name")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(DbError::Query)?;

    Ok(Page::new(listings, total, &request))
}

/// A key as a list shows it: the row, plus the name of the person it acts as.
#[derive(Debug, Clone)]
pub struct ApiKeyListing {
    pub key: ApiKeyRecord,
    pub owner_name: String,
}

/// Stop a key.
///
/// `false` means no live key had that id - already revoked, or never there.
/// The row is kept rather than deleted so that "who issued this, and who
/// stopped it" survives the key itself.
pub async fn revoke<'e, E>(
    executor: E,
    id: Uuid,
    reason: &str,
    revoked_by: Option<UserId>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE api_keys
            SET revoked_at = now(), revoked_reason = $2, revoked_by = $3
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(reason)
    .bind(revoked_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected() > 0)
}

/// Stop every live key belonging to one account.
///
/// For the moment somebody is suspended or deleted: the intersection with the
/// owner's permissions already makes their keys powerless, but a credential
/// that still authenticates is a credential somebody has to reason about.
pub async fn revoke_all_for_user<'e, E>(
    executor: E,
    user_id: UserId,
    reason: &str,
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE api_keys
            SET revoked_at = now(), revoked_reason = $2
          WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(reason)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}

/// `%` and `_` are wildcards in `LIKE`; a search box is not a pattern language.
fn escape_like(needle: &str) -> String {
    needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
