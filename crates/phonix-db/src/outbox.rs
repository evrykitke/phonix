//! The transactional outbox (`outbox_events`).
//!
//! The table has existed since migration 0001 and this is what finally writes
//! to it. The problem it solves is small to state and impossible to fix any
//! other way:
//!
//! > Something changed, and something else needs to hear about it. Publishing
//! > to the broker and committing the change are two operations against two
//! > systems, and there is no order in which both are safe.
//!
//! Publish first and the transaction may roll back: an event announcing
//! something that never happened. Commit first and the process may die: a
//! change nobody was told about. Retrying does not help - it moves the window,
//! it does not close it.
//!
//! The outbox closes it by making the announcement part of the change. The
//! event is inserted **in the same transaction** as the rows it describes, so
//! it commits if and only if they do. A relay then reads unpublished rows and
//! sends them, which can fail as often as it likes: the row is still there.
//!
//! ```text
//!   BEGIN
//!     UPDATE file_uploads SET status = 'stored' ...
//!     INSERT INTO outbox_events (routing_key, payload) ...
//!   COMMIT                                    <- both, or neither
//!
//!   ... later, and possibly much later ...
//!
//!   relay: claim unpublished -> publish to rabbitmq -> mark published
//! ```
//!
//! # Delivery is at least once, and that is a promise to consumers
//!
//! A relay that publishes and then dies before marking the row will publish
//! again on the next pass. That is the correct trade - the alternative loses
//! events - and it is why every message carries a `message_id` and why the
//! tenant databases have a `processed_events` table. A handler that is not
//! idempotent is a handler with a bug.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value as Json;
use sqlx::{FromRow, PgExecutor, PgPool, Row};
use uuid::Uuid;

use crate::error::DbError;

/// An event waiting to be published, or one that already has been.
#[derive(Debug, Clone)]
pub struct OutboxEvent {
    /// The row's own identity, used to mark it published.
    pub id: i64,
    /// What the broker sees as `message_id`, and what a consumer deduplicates
    /// on. Generated at insert, so a republished row carries the same id.
    pub event_id: Uuid,
    /// Appended to `tenant.<slug>.` to form the routing key.
    pub routing_key: String,
    pub payload: Json,
    pub occurred_at: DateTime<Utc>,
    pub attempts: u32,
    pub last_error: Option<String>,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for OutboxEvent {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let attempts: i32 = row.try_get("attempts")?;

        Ok(Self {
            id: row.try_get("id")?,
            event_id: row.try_get("event_id")?,
            routing_key: row.try_get("routing_key")?,
            payload: row.try_get("payload")?,
            occurred_at: row.try_get("occurred_at")?,
            attempts: u32::try_from(attempts).unwrap_or(0),
            last_error: row.try_get("last_error")?,
        })
    }
}

const COLUMNS: &str = "id, event_id, routing_key, payload, occurred_at, attempts, last_error";

/// Record an event to be published.
///
/// Takes an executor rather than a pool, and that is the whole point: pass the
/// **transaction** that is making the change this event describes. Passing a
/// pool here compiles perfectly and quietly reintroduces the problem the table
/// exists to solve.
///
/// Returns the `event_id` the message will carry, so a caller can log it
/// alongside whatever it just changed.
pub async fn enqueue<'e, E, T>(executor: E, routing_key: &str, payload: &T) -> Result<Uuid, DbError>
where
    E: PgExecutor<'e>,
    T: Serialize + ?Sized,
{
    let payload = serde_json::to_value(payload).map_err(|err| {
        // Serialising a payload cannot depend on the database, so this is a
        // programming error rather than a storage failure - but it must not
        // panic in a transaction that is about to commit real work.
        DbError::Serialization(err.to_string())
    })?;

    let event_id: Uuid = sqlx::query_scalar(
        "INSERT INTO outbox_events (routing_key, payload)
         VALUES ($1, $2)
         RETURNING event_id",
    )
    .bind(routing_key)
    .bind(payload)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(event_id)
}

/// Claim a batch of unpublished events.
///
/// `FOR UPDATE SKIP LOCKED` for the same reason the upload queue uses it: two
/// relays - two processes, or one process restarted before the old one has
/// finished draining - must take different rows rather than block on each
/// other.
///
/// Note what this does *not* do: mark anything. A row is published first and
/// marked afterwards, because the other order loses events whenever the publish
/// fails. Claiming holds the row only for the length of the transaction, which
/// ends when this returns.
pub async fn claim_unpublished(pool: &PgPool, limit: usize) -> Result<Vec<OutboxEvent>, DbError> {
    let statement = format!(
        "SELECT {COLUMNS}
           FROM outbox_events
          WHERE published_at IS NULL
          ORDER BY occurred_at
          LIMIT $1
            FOR UPDATE SKIP LOCKED"
    );

    sqlx::query_as::<_, OutboxEvent>(sqlx::AssertSqlSafe(statement))
        .bind(limit as i64)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)
}

/// Mark an event as published.
pub async fn mark_published<'e, E>(executor: E, id: i64) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE outbox_events
            SET published_at = now(),
                last_error   = NULL
          WHERE id = $1",
    )
    .bind(id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Record that publishing failed, so the next pass can see how long this has
/// been going on.
///
/// The row stays unpublished, which is the point: a broker that is down means
/// events accumulate here and are delivered when it comes back, rather than
/// being lost while it was away.
pub async fn record_failure<'e, E>(executor: E, id: i64, error: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE outbox_events
            SET attempts   = attempts + 1,
                last_error = $2
          WHERE id = $1",
    )
    .bind(id)
    .bind(clip(error, 1000))
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Discard events published longer ago than `older_than_days`.
///
/// Published rows are kept for a while rather than deleted on the spot: they
/// are the record of what was announced, and the first question after an
/// incident is usually whether an event was sent at all.
pub async fn purge_published<'e, E>(executor: E, older_than_days: u32) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "DELETE FROM outbox_events
          WHERE published_at IS NOT NULL
            AND published_at < now() - make_interval(days => $1)",
    )
    .bind(i32::try_from(older_than_days).unwrap_or(i32::MAX))
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}

/// How many events are waiting.
///
/// For the readiness endpoint and for a dashboard: a backlog that grows without
/// bound is a broker that has been unreachable for a while, and the outbox is
/// the only place that shows it.
pub async fn pending_count<'e, E>(executor: E) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE published_at IS NULL")
            .fetch_one(executor)
            .await
            .map_err(DbError::Query)?;

    Ok(u64::try_from(count).unwrap_or(0))
}

/// The relay's backlog, as an operator sees it.
///
/// Counts and one timestamp. No routing keys and no payloads: Phonix Desk
/// reads this, and an event payload is business data - see ADR 0005 section 6.
/// "How far behind is the relay" is answerable without any of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backlog {
    pub unpublished: u64,
    /// The oldest thing still unpublished. A backlog of ten from a minute ago
    /// is a busy moment; a backlog of ten from Tuesday is a broker that has
    /// been unreachable since Tuesday.
    pub oldest_at: Option<DateTime<Utc>>,
    /// How many have been tried and failed at least once. Distinguishes "not
    /// got to yet" from "keeps not working".
    pub retried: u64,
}

/// Read [`Backlog`] in one statement.
pub async fn backlog<'e, E>(executor: E) -> Result<Backlog, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(
        "SELECT count(*)                             AS unpublished,
                min(occurred_at)                     AS oldest,
                count(*) FILTER (WHERE attempts > 0) AS retried
           FROM outbox_events
          WHERE published_at IS NULL",
    )
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    let count = |name: &str| -> Result<u64, DbError> {
        let value: i64 = row.try_get(name).map_err(DbError::Query)?;
        Ok(u64::try_from(value).unwrap_or(0))
    };

    Ok(Backlog {
        unpublished: count("unpublished")?,
        oldest_at: row.try_get("oldest").map_err(DbError::Query)?,
        retried: count("retried")?,
    })
}

/// Cut a string to a length the column will take, on a character boundary.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }

    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    text.get(..end).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_lands_on_a_character_boundary() {
        let long = "日".repeat(500);
        let cut = clip(&long, 1000);

        assert!(cut.len() <= 1000);
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
        assert_eq!(clip("short", 100), "short");
    }

    #[test]
    fn the_claim_does_not_mark_anything() {
        // Publishing and marking are two steps in that order on purpose: the
        // other order loses an event whenever the publish fails. A `SET
        // published_at` creeping into this statement would be exactly that bug,
        // and it would look like a tidy-up.
        let statement = format!(
            "SELECT {COLUMNS} FROM outbox_events WHERE published_at IS NULL \
             ORDER BY occurred_at LIMIT $1 FOR UPDATE SKIP LOCKED"
        );

        assert!(!statement.contains("UPDATE outbox_events"));
        assert!(statement.contains("SKIP LOCKED"));
    }
}
