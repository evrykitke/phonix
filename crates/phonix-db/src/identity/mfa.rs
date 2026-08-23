//! The `user_mfa_factors` table.
//!
//! Three kinds of row, stored differently for a reason:
//!
//! | kind            | `secret_encrypted` holds | why                          |
//! | --------------- | ------------------------ | ---------------------------- |
//! | `totp`          | a sealed shared secret   | the server recomputes codes  |
//! | `recovery_code` | a SHA-256 digest         | one comparison, then deleted |
//! | `webauthn`      | nothing (a public key)   | reserved, nothing issues it  |
//!
//! Nothing here seals, opens, digests or verifies anything. The bytes arrive
//! ready to store and leave exactly as stored; deciding what they mean is
//! `phonix_services::identity::mfa`'s job. That keeps the one rule this layer
//! has: **a repository never holds a credential in a form it could use.**
//!
//! Two invariants are enforced in SQL rather than in Rust:
//!
//! * an unconfirmed factor (`confirmed_at IS NULL`) is never returned by
//!   [`confirmed_totp`], so it cannot satisfy a challenge;
//! * a spent recovery code is `DELETE`d rather than flagged, so there is
//!   nothing left for a second attempt to match.

use chrono::{DateTime, Utc};
use phonix_core::identity::UserId;
use phonix_core::identity::mfa::{MfaFactorKind, MfaFactorSummary};
use sqlx::{PgExecutor, PgPool, Row};
use uuid::Uuid;

use crate::error::DbError;

/// A stored factor with its material, for the layer that can interpret it.
pub struct StoredFactor {
    pub id: Uuid,
    /// Sealed TOTP secret, or a recovery-code digest.
    pub material: Vec<u8>,
    pub key_version: i16,
}

/// Insert an unconfirmed authenticator app, replacing any earlier attempt.
///
/// Unconfirmed rows are cleared first so a user who abandoned the enrolment
/// screen and came back does not accumulate dead rows - and so the partial
/// unique index on confirmed TOTP factors stays satisfiable.
pub async fn insert_unconfirmed_totp(
    pool: &PgPool,
    user_id: UserId,
    label: &str,
    sealed_secret: &[u8],
    key_version: i16,
) -> Result<Uuid, DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    sqlx::query(
        "DELETE FROM user_mfa_factors
          WHERE user_id = $1 AND kind = 'totp' AND confirmed_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    let id: Uuid = sqlx::query(
        "INSERT INTO user_mfa_factors (user_id, kind, label, secret_encrypted, key_version)
         VALUES ($1, 'totp', $2, $3, $4)
         RETURNING id",
    )
    .bind(user_id)
    .bind(label)
    .bind(sealed_secret)
    .bind(key_version)
    .fetch_one(&mut *tx)
    .await
    .map_err(DbError::Query)?
    .try_get("id")
    .map_err(DbError::Query)?;

    tx.commit().await.map_err(DbError::Query)?;
    Ok(id)
}

/// The pending enrolment a user is part-way through.
pub async fn pending_totp<'e, E>(
    executor: E,
    user_id: UserId,
    factor_id: Uuid,
) -> Result<Option<StoredFactor>, DbError>
where
    E: PgExecutor<'e>,
{
    fetch_factor(
        executor,
        "SELECT id, secret_encrypted, key_version FROM user_mfa_factors
          WHERE id = $2 AND user_id = $1 AND kind = 'totp' AND confirmed_at IS NULL",
        user_id,
        Some(factor_id),
    )
    .await
}

/// The authenticator app a user has actually confirmed, if any.
pub async fn confirmed_totp<'e, E>(
    executor: E,
    user_id: UserId,
) -> Result<Option<StoredFactor>, DbError>
where
    E: PgExecutor<'e>,
{
    fetch_factor(
        executor,
        "SELECT id, secret_encrypted, key_version FROM user_mfa_factors
          WHERE user_id = $1 AND kind = 'totp' AND confirmed_at IS NOT NULL",
        user_id,
        None,
    )
    .await
}

async fn fetch_factor<'e, E>(
    executor: E,
    sql: &'static str,
    user_id: UserId,
    factor_id: Option<Uuid>,
) -> Result<Option<StoredFactor>, DbError>
where
    E: PgExecutor<'e>,
{
    let mut query = sqlx::query(sql).bind(user_id);
    if let Some(factor_id) = factor_id {
        query = query.bind(factor_id);
    }

    let row = query
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)?;

    let Some(row) = row else {
        return Ok(None);
    };

    Ok(Some(StoredFactor {
        id: row.try_get("id").map_err(DbError::Query)?,
        material: row.try_get("secret_encrypted").map_err(DbError::Query)?,
        key_version: row.try_get("key_version").map_err(DbError::Query)?,
    }))
}

/// Mark an enrolment confirmed and set the account's `mfa_enabled` mirror.
///
/// One transaction: an account flagged as enrolled with no confirmed factor
/// would be challenged for a code nothing can produce.
pub async fn confirm_factor(
    pool: &PgPool,
    user_id: UserId,
    factor_id: Uuid,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    sqlx::query(
        "UPDATE user_mfa_factors SET confirmed_at = now(), last_used_at = now()
          WHERE id = $1 AND user_id = $2",
    )
    .bind(factor_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    sqlx::query(
        "UPDATE users
            SET mfa_enabled = TRUE,
                mfa_enrolled_at = coalesce(mfa_enrolled_at, now())
          WHERE id = $1",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

/// Record that a factor was used.
pub async fn touch_factor<'e, E>(executor: E, factor_id: Uuid) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("UPDATE user_mfa_factors SET last_used_at = now() WHERE id = $1")
        .bind(factor_id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

/// Replace a user's recovery codes with a new set of digests.
///
/// Replacing rather than adding: a user who generates a new set expects the old
/// printout to stop working, and a system that keeps both leaves live codes on
/// a piece of paper somebody threw away.
pub async fn replace_recovery_codes(
    pool: &PgPool,
    user_id: UserId,
    digests: &[Vec<u8>],
    batch_id: Uuid,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    sqlx::query("DELETE FROM user_mfa_factors WHERE user_id = $1 AND kind = 'recovery_code'")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    if !digests.is_empty() {
        // One statement with an array parameter: ten round trips to insert ten
        // rows is latency for nothing.
        sqlx::query(
            "INSERT INTO user_mfa_factors
                 (user_id, kind, label, secret_encrypted, batch_id, confirmed_at)
             SELECT $1, 'recovery_code', 'Recovery code', digest, $3, now()
               FROM unnest($2::bytea[]) AS digest",
        )
        .bind(user_id)
        .bind(digests)
        .bind(batch_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

/// Spend a recovery code by its digest, returning whether one matched.
///
/// `DELETE ... RETURNING` is what makes it exactly once under concurrency: two
/// simultaneous submissions of the same code, and only one deletes a row.
pub async fn consume_recovery_code<'e, E>(
    executor: E,
    user_id: UserId,
    digest: &[u8],
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let deleted = sqlx::query(
        "DELETE FROM user_mfa_factors
          WHERE user_id = $1 AND kind = 'recovery_code' AND secret_encrypted = $2
      RETURNING id",
    )
    .bind(user_id)
    .bind(digest)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(deleted.is_some())
}

/// How many unspent recovery codes a user holds.
pub async fn count_recovery_codes<'e, E>(executor: E, user_id: UserId) -> Result<usize, DbError>
where
    E: PgExecutor<'e>,
{
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_mfa_factors
          WHERE user_id = $1 AND kind = 'recovery_code'",
    )
    .bind(user_id)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(count.max(0) as usize)
}

/// A user's factors, for the security screen.
///
/// Recovery codes are counted rather than listed - ten identical rows tell
/// nobody anything - and no material is returned at all.
pub async fn list_factors<'e, E>(
    executor: E,
    user_id: UserId,
) -> Result<Vec<MfaFactorSummary>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query(
        "SELECT id, kind, label, confirmed_at, created_at, last_used_at
           FROM user_mfa_factors
          WHERE user_id = $1 AND kind <> 'recovery_code'
          ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    let mut factors = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.try_get("kind").map_err(DbError::Query)?;
        let confirmed_at: Option<DateTime<Utc>> =
            row.try_get("confirmed_at").map_err(DbError::Query)?;

        factors.push(MfaFactorSummary {
            id: row.try_get("id").map_err(DbError::Query)?,
            kind: MfaFactorKind::parse(&kind).ok_or_else(|| {
                DbError::CorruptRow(format!("user_mfa_factors.kind holds '{kind}'"))
            })?,
            label: row.try_get("label").map_err(DbError::Query)?,
            confirmed: confirmed_at.is_some(),
            created_at: row.try_get("created_at").map_err(DbError::Query)?,
            last_used_at: row.try_get("last_used_at").map_err(DbError::Query)?,
        });
    }

    Ok(factors)
}

/// Whether a user holds a factor that can answer a challenge.
pub async fn has_confirmed_factor<'e, E>(executor: E, user_id: UserId) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM user_mfa_factors
              WHERE user_id = $1 AND kind = 'totp' AND confirmed_at IS NOT NULL
         )",
    )
    .bind(user_id)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(exists)
}

/// Remove a factor and, if it was the last one, turn MFA off for the account.
///
/// Recovery codes go with it: an account whose only remaining second factor is
/// a printout has a fallback, not a factor.
pub async fn remove_factor(
    pool: &PgPool,
    user_id: UserId,
    factor_id: Uuid,
) -> Result<bool, DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    let removed = sqlx::query(
        "DELETE FROM user_mfa_factors
          WHERE id = $1 AND user_id = $2 AND kind <> 'recovery_code'
      RETURNING id",
    )
    .bind(factor_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    if removed.is_none() {
        tx.rollback().await.map_err(DbError::Query)?;
        return Ok(false);
    }

    if !has_confirmed_factor(&mut *tx, user_id).await? {
        clear_enrolment(&mut tx, user_id).await?;
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(true)
}

/// Remove every factor a user holds.
///
/// The administrator's answer to "I lost my phone and my recovery codes". It
/// leaves the account signing in with a password alone, which is why the caller
/// is expected to have checked `Pages.Administration.Users.Edit` first and to
/// write an audit entry.
pub async fn reset_all_factors(pool: &PgPool, user_id: UserId) -> Result<u64, DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    let removed = sqlx::query("DELETE FROM user_mfa_factors WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    clear_enrolment(&mut tx, user_id).await?;

    tx.commit().await.map_err(DbError::Query)?;
    Ok(removed)
}

async fn clear_enrolment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM user_mfa_factors WHERE user_id = $1 AND kind = 'recovery_code'")
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(DbError::Query)?;

    sqlx::query("UPDATE users SET mfa_enabled = FALSE, mfa_enrolled_at = NULL WHERE id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(DbError::Query)?;

    Ok(())
}
