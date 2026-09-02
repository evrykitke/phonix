//! The `desk_sessions` table.
//!
//! Shaped like `identity::session` and for the same reasons: the cookie holds
//! the token, this table holds its SHA-256 digest, and **every function here
//! takes the digest** so no repository ever holds a credential a client could
//! present.
//!
//! Two deadlines, both enforced in SQL rather than in Rust - `expires_at`
//! slides forward with activity, `absolute_expires_at` never moves - so an
//! expired session cannot be resurrected by a code path that forgets to check.
//!
//! # What is deliberately simpler than the tenant version
//!
//! There is one kind of desk session and no "remember me". A browser that
//! suspends workspaces does not get a ninety-day cookie because somebody ticked
//! a box, and a second window to configure is a second thing to get wrong.
//!
//! `mfa_satisfied` is false between the password and the code. A session in
//! that state exists only so the challenge page has something to attach to.

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// A desk session row, without the token.
#[derive(Debug, Clone, FromRow)]
pub struct DeskSessionRecord {
    pub id: Uuid,
    pub desk_user_id: Uuid,
    /// Whether this session has cleared the second factor.
    pub mfa_satisfied: bool,
    pub mfa_attempts: i32,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

impl DeskSessionRecord {
    /// Seconds until the deadline activity cannot extend.
    ///
    /// The absolute one, not the sliding one: the idle deadline moves on every
    /// request, so a number derived from it is stale before the response is
    /// written.
    pub fn remaining_secs(&self) -> i64 {
        (self.absolute_expires_at - Utc::now()).num_seconds().max(0)
    }
}

/// What a client sent along with the request.
#[derive(Debug, Clone, Default)]
pub struct ClientFacts<'a> {
    pub ip: Option<&'a str>,
    /// Truncated by the caller. The column refuses more than 256 characters,
    /// because a header is whatever the client felt like sending.
    pub user_agent: Option<&'a str>,
}

const INSERT: &str = "INSERT INTO desk_sessions \
     (desk_user_id, token_hash, mfa_satisfied, ip, user_agent, expires_at, absolute_expires_at) \
     VALUES ($1, $2, false, $3, $4, $5, $6) \
     RETURNING id, desk_user_id, mfa_satisfied, mfa_attempts, ip, user_agent, created_at, \
     last_seen_at, expires_at, absolute_expires_at";

/// Open a session for a desk user, with the second factor still outstanding.
///
/// Always `mfa_satisfied = false`: TOTP is mandatory for Desk, so there is no
/// caller that could legitimately ask for a session that skipped it, and a
/// parameter for it would be a way to introduce one.
pub async fn create<'e, E>(
    executor: E,
    desk_user_id: Uuid,
    token_hash: &[u8],
    idle_minutes: i64,
    absolute_hours: i64,
    facts: ClientFacts<'_>,
) -> Result<DeskSessionRecord, DbError>
where
    E: PgExecutor<'e>,
{
    let now = Utc::now();
    let absolute_expires_at = now + Duration::hours(absolute_hours);
    // Clamped: an idle window configured longer than the absolute one would
    // otherwise produce a session whose sliding deadline outlives its ceiling.
    let expires_at = (now + Duration::minutes(idle_minutes)).min(absolute_expires_at);

    sqlx::query_as::<_, DeskSessionRecord>(INSERT)
        .bind(desk_user_id)
        .bind(token_hash)
        .bind(facts.ip)
        .bind(facts.user_agent)
        .bind(expires_at)
        .bind(absolute_expires_at)
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)
}

/// Look up a live session and slide its idle deadline forward.
///
/// Lookup and refresh are one statement. As two they would race: a request
/// arriving just as a session expires could read it as valid and then extend
/// one another request had already seen as dead.
///
/// `None` covers unknown, expired and revoked alike - indistinguishable to the
/// caller on purpose.
pub async fn touch<'e, E>(
    executor: E,
    token_hash: &[u8],
    idle_minutes: i64,
) -> Result<Option<DeskSessionRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskSessionRecord>(
        "UPDATE desk_sessions
            SET last_seen_at = now(),
                -- Never past the ceiling: activity extends a session, it does
                -- not make one immortal.
                expires_at = least(
                    now() + ($2::bigint * interval '1 minute'),
                    absolute_expires_at
                )
          WHERE token_hash = $1
            AND revoked_at IS NULL
            AND expires_at > now()
            AND absolute_expires_at > now()
      RETURNING id, desk_user_id, mfa_satisfied, mfa_attempts, ip, user_agent, created_at,
                last_seen_at, expires_at, absolute_expires_at",
    )
    .bind(token_hash)
    .bind(idle_minutes)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

/// The second factor has been produced. From here the session is a sign-in.
pub async fn mark_mfa_satisfied<'e, E>(executor: E, id: Uuid) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("UPDATE desk_sessions SET mfa_satisfied = true, mfa_attempts = 0 WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    Ok(())
}

/// Count a wrong code, and hand back the running total.
///
/// Counted on the session rather than the account: a challenge is a short-lived
/// thing and the right response to guessing at it is to end that attempt, not
/// to lock out a person whose password was correct.
pub async fn record_mfa_attempt<'e, E>(executor: E, id: Uuid) -> Result<i32, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(
        "UPDATE desk_sessions SET mfa_attempts = mfa_attempts + 1 \
          WHERE id = $1 RETURNING mfa_attempts",
    )
    .bind(id)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    row.try_get("mfa_attempts").map_err(DbError::Query)
}

/// End a session. Idempotent: signing out twice is not an error.
pub async fn revoke<'e, E>(executor: E, token_hash: &[u8], reason: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE desk_sessions SET revoked_at = now(), revoked_reason = $2 \
          WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .bind(reason)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

pub async fn revoke_by_id<'e, E>(executor: E, id: Uuid, reason: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE desk_sessions SET revoked_at = now(), revoked_reason = $2 \
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(reason)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// End every session a desk user holds.
///
/// What disabling an account has to do, and what finishing setup does too: a
/// password change must not leave the old browser signed in.
pub async fn revoke_all_for_user<'e, E>(
    executor: E,
    desk_user_id: Uuid,
    reason: &str,
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query(
        "UPDATE desk_sessions SET revoked_at = now(), revoked_reason = $2 \
          WHERE desk_user_id = $1 AND revoked_at IS NULL",
    )
    .bind(desk_user_id)
    .bind(reason)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(done.rows_affected())
}

/// Housekeeping, not correctness: the deadlines are in every `WHERE` clause,
/// so a row left here is stale rather than dangerous.
pub async fn purge_expired<'e, E>(executor: E) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query(
        "DELETE FROM desk_sessions \
          WHERE absolute_expires_at < now() - interval '30 days' \
             OR (revoked_at IS NOT NULL AND revoked_at < now() - interval '30 days')",
    )
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_is_never_negative() {
        let past = Utc::now() - Duration::hours(2);
        let session = DeskSessionRecord {
            id: Uuid::nil(),
            desk_user_id: Uuid::nil(),
            mfa_satisfied: true,
            mfa_attempts: 0,
            ip: None,
            user_agent: None,
            created_at: past,
            last_seen_at: past,
            expires_at: past,
            absolute_expires_at: past,
        };

        assert_eq!(session.remaining_secs(), 0);
    }

    /// `DeskSessionRecord` derives `FromRow`, so a column missing from the
    /// statement is a decode error at runtime rather than a compile error.
    #[test]
    fn the_insert_returns_every_field_from_row_reads() {
        for column in [
            "id",
            "desk_user_id",
            "mfa_satisfied",
            "mfa_attempts",
            "ip",
            "user_agent",
            "created_at",
            "last_seen_at",
            "expires_at",
            "absolute_expires_at",
        ] {
            assert!(
                INSERT.contains(column),
                "{column} is not returned by INSERT"
            );
        }
    }
}
