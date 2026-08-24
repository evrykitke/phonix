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

/// Redeem a *guessable* secret, spending one attempt whether or not it is right.
///
/// [`consume`] is the right operation for a 32-byte token: nothing guesses one,
/// so an endpoint that answers "no" forever costs an attacker nothing and gains
/// them nothing. A six-digit code inverts that. A million values is a few
/// minutes of scripted guessing, and the only thing standing between a mailbox
/// nobody read and somebody else's account is that the answering stops.
///
/// So the count lives on the row and moves in the same transaction as the
/// comparison. Two guesses arriving together both take `FOR UPDATE`, so the
/// second waits for the first to commit and sees the incremented count - the
/// alternative is a limit of five that a concurrent client can turn into
/// however many requests it can open at once.
///
/// The row is burned - `consumed_at` set - on the attempt that reaches
/// `max_attempts`, not on the one after it. An exhausted code that is still
/// technically live is a code the next request can keep guessing at.
///
/// Returns the user on a correct code and `None` on every other outcome:
/// wrong, expired, exhausted, already spent, never issued. Indistinguishable
/// for the same reason [`consume`]'s four cases are, and with one more here -
/// the caller has an email address in hand, so "there is no reset in progress
/// for that address" would be an account oracle.
///
/// `max_attempts` of 0 or less means the code cannot be redeemed at all, which
/// is the honest reading of "no attempts are allowed" rather than a licence for
/// unlimited ones.
pub async fn redeem_code(
    pool: &sqlx::PgPool,
    user_id: UserId,
    purpose: TokenPurpose,
    presented_hash: &[u8],
    max_attempts: i16,
) -> Result<Option<UserId>, DbError> {
    if max_attempts <= 0 {
        return Ok(None);
    }

    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    // `FOR UPDATE` is the whole point of the transaction: it serialises
    // concurrent guesses at the same code onto the same count.
    let row = sqlx::query(
        "SELECT id, token_hash, attempts
           FROM user_tokens
          WHERE user_id = $1
            AND purpose = $2
            AND consumed_at IS NULL
            AND expires_at > now()
          FOR UPDATE",
    )
    .bind(user_id)
    .bind(purpose.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    let Some(row) = row else {
        // Nothing outstanding. Commit rather than roll back: there is nothing
        // to undo, and a rollback here would be indistinguishable in the logs
        // from one that mattered.
        tx.commit().await.map_err(DbError::Query)?;
        return Ok(None);
    };

    let id: Uuid = row.try_get("id").map_err(DbError::Query)?;
    let stored: Vec<u8> = row.try_get("token_hash").map_err(DbError::Query)?;
    let attempts: i16 = row.try_get("attempts").map_err(DbError::Query)?;

    // Constant-time. The comparison is against a SHA-256 digest rather than the
    // code itself, so a timing leak would reveal a prefix of the digest and not
    // of the secret - but the cost of doing it properly is nil and reasoning
    // about which comparisons are safe to shortcut is not.
    let matched = constant_time_eq(&stored, presented_hash);
    let spent = attempts.saturating_add(1);

    // Burn it when it is right, and when this attempt was the last one it had.
    let burn = matched || spent >= max_attempts;

    sqlx::query(
        "UPDATE user_tokens
            SET attempts = $2,
                consumed_at = CASE WHEN $3 THEN now() ELSE consumed_at END
          WHERE id = $1",
    )
    .bind(id)
    .bind(spent)
    .bind(burn)
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    tx.commit().await.map_err(DbError::Query)?;

    Ok(matched.then_some(user_id))
}

/// Compare two digests without letting the clock describe them.
///
/// `Vec<u8> == Vec<u8>` returns at the first differing byte. Nothing here is
/// remotely close to exploitable, and writing the loop that does not stop early
/// is cheaper than establishing that every time somebody reads it.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
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
