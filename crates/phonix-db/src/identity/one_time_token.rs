//! Single-use secrets: email verification, password reset, invitations, and
//! the signup handoff.
//!
//! One table and one implementation for all four, because the columns are the
//! same and so is the danger. Four hand-rolled versions of "issue, expire,
//! consume exactly once" is four chances to get the third step wrong.
//!
//! Consumption is a conditional `UPDATE ... RETURNING`, so two simultaneous
//! redemptions of the same token cannot both succeed: the first sets
//! `consumed_at`, the second matches no row.

use chrono::{DateTime, Utc};
use phonix_core::identity::UserId;
use sqlx::{PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// What a token is for. Matches the `user_tokens_purpose_valid` constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenPurpose {
    EmailVerification,
    PasswordReset,
    Invitation,
    /// Trades a just-created account on the signup host for a session cookie on
    /// the workspace's own host. Lives for seconds.
    SessionHandoff,
}

impl TokenPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmailVerification => "email_verification",
            Self::PasswordReset => "password_reset",
            Self::Invitation => "invitation",
            Self::SessionHandoff => "session_handoff",
        }
    }
}

/// The stored side of an issued token.
///
/// No secret: the caller minted it and is the only place it exists.
pub struct TokenRecord {
    pub id: Uuid,
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
}

/// Issue a token, invalidating any outstanding one for the same purpose.
///
/// Superseding matters: if requesting a second password reset left the first
/// live, an email intercepted an hour ago would still work. The old row is
/// marked consumed rather than deleted so a replay of it is visible as a
/// replay rather than as an unknown token.
pub async fn issue<'e, E>(
    executor: E,
    user_id: UserId,
    purpose: TokenPurpose,
    token_hash: &[u8],
    ttl_secs: i64,
    created_ip: Option<&str>,
) -> Result<TokenRecord, DbError>
where
    E: PgExecutor<'e> + Copy,
{
    sqlx::query(
        "UPDATE user_tokens
            SET consumed_at = now()
          WHERE user_id = $1 AND purpose = $2 AND consumed_at IS NULL",
    )
    .bind(user_id)
    .bind(purpose.as_str())
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    let row = sqlx::query(
        "INSERT INTO user_tokens (user_id, purpose, token_hash, expires_at, created_ip)
         VALUES ($1, $2, $3, now() + ($4::bigint * interval '1 second'), $5)
         RETURNING id, expires_at",
    )
    .bind(user_id)
    .bind(purpose.as_str())
    .bind(token_hash)
    .bind(ttl_secs)
    .bind(created_ip)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(TokenRecord {
        id: row.try_get("id").map_err(DbError::Query)?,
        user_id,
        expires_at: row.try_get("expires_at").map_err(DbError::Query)?,
    })
}

/// Redeem a token, exactly once.
///
/// Returns the user it belonged to, or `None` when the token is unknown,
/// expired, already used, or issued for a different purpose. The four are
/// deliberately indistinguishable to the caller: a message that says "already
/// used" instead of "invalid" confirms to whoever intercepted the link that
/// they had a real one.
///
/// The purpose is part of the match so a verification link cannot be redeemed
/// as a password reset.
pub async fn consume<'e, E>(
    executor: E,
    token_hash: &[u8],
    purpose: TokenPurpose,
) -> Result<Option<UserId>, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(
        "UPDATE user_tokens
            SET consumed_at = now()
          WHERE token_hash = $1
            AND purpose = $2
            AND consumed_at IS NULL
            AND expires_at > now()
      RETURNING user_id",
    )
    .bind(token_hash)
    .bind(purpose.as_str())
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    match row {
        Some(row) => Ok(Some(row.try_get("user_id").map_err(DbError::Query)?)),
        None => Ok(None),
    }
}

/// Invalidate every outstanding token of one purpose for a user.
///
/// Called after a password change, so a reset link mailed before the change
/// cannot be used after it.
pub async fn revoke_all<'e, E>(
    executor: E,
    user_id: UserId,
    purpose: TokenPurpose,
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE user_tokens SET consumed_at = now()
          WHERE user_id = $1 AND purpose = $2 AND consumed_at IS NULL",
    )
    .bind(user_id)
    .bind(purpose.as_str())
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}

/// Delete tokens that are long past use. Housekeeping only.
///
/// Consumed rows are kept for a week so a replay attempt still shows up in the
/// audit trail as a replay.
pub async fn purge_expired<'e, E>(executor: E) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "DELETE FROM user_tokens
          WHERE expires_at < now() - interval '7 days'
             OR (consumed_at IS NOT NULL AND consumed_at < now() - interval '7 days')",
    )
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purposes_match_the_check_constraint() {
        // These strings are written into a column with a CHECK on exactly this
        // list; a typo here is an insert failure at runtime.
        for (purpose, expected) in [
            (TokenPurpose::EmailVerification, "email_verification"),
            (TokenPurpose::PasswordReset, "password_reset"),
            (TokenPurpose::Invitation, "invitation"),
            (TokenPurpose::SessionHandoff, "session_handoff"),
        ] {
            assert_eq!(purpose.as_str(), expected);
        }
    }
}
