//! The `password_history` table.
//!
//! Only used by workspaces that set `password_history_depth > 0`. Rows are
//! Argon2 hashes of *previous* passwords, kept solely so the reuse check has
//! something to compare against, and pruned to the configured depth on every
//! write - an unbounded history is a growing pile of hashes of passwords the
//! user may still be using elsewhere.

use phonix_core::identity::UserId;
use sqlx::{PgExecutor, PgPool};

use crate::error::DbError;

/// The most recent hashes for a user, newest first.
pub async fn recent<'e, E>(executor: E, user_id: UserId, limit: i64) -> Result<Vec<String>, DbError>
where
    E: PgExecutor<'e>,
{
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT password_hash FROM password_history
          WHERE user_id = $1
          ORDER BY created_at DESC
          LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(hashes)
}

/// Record a superseded hash and drop anything past `depth`.
///
/// Both statements in one transaction: a history that grew but was not pruned
/// would keep hashes the policy says to forget.
pub async fn remember(
    pool: &PgPool,
    user_id: UserId,
    superseded_hash: &str,
    depth: i32,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    sqlx::query("INSERT INTO password_history (user_id, password_hash) VALUES ($1, $2)")
        .bind(user_id)
        .bind(superseded_hash)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    sqlx::query(
        "DELETE FROM password_history
          WHERE user_id = $1
            AND id NOT IN (
                SELECT id FROM password_history
                 WHERE user_id = $1
                 ORDER BY created_at DESC
                 LIMIT $2
            )",
    )
    .bind(user_id)
    .bind(i64::from(depth))
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

/// Drop a user's whole history.
///
/// Called when a workspace turns the reuse check off: nothing checks these any
/// more, so there is no reason to keep hashes of passwords that may still be in
/// use somewhere else.
pub async fn forget_all<'e, E>(executor: E, user_id: UserId) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query("DELETE FROM password_history WHERE user_id = $1")
        .bind(user_id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}
