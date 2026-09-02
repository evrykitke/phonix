//! The `desk_users` table.
//!
//! One row per person who may sign in to Phonix Desk. There is no signup: a
//! desk user is created by another desk user (or, for the first one, by a CLI
//! subcommand on the box), and collects a password of their own through a
//! single-use setup link.
//!
//! **Nothing here takes a password or a token, only their digests.** Hashing a
//! password and digesting a setup token are the application layer's job, the
//! same rule `identity::session` follows: no repository in this crate ever
//! holds a credential a client could present.
//!
//! # The three statuses, and the constraint behind them
//!
//! `pending` has a row and nothing else - no password, no authenticator.
//! `active` has both, and the check constraint `desk_users_active_is_complete`
//! is what makes that true in the database rather than only in Rust. `disabled`
//! keeps the row so the audit trail still names somebody.
//!
//! TOTP is mandatory for Desk, so "has a confirmed authenticator" is not a
//! preference: a row that cannot produce a code cannot finish a sign-in.

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// Lifecycle of a desk account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeskUserStatus {
    /// Created, but has not yet used its setup link.
    Pending,
    /// Has a password and a confirmed authenticator.
    Active,
    /// Kept for the audit trail; cannot sign in.
    Disabled,
}

impl DeskUserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    /// Whether an account in this state may present a credential at all.
    pub fn may_sign_in(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// One row of `catalog.desk_users`, without any credential.
#[derive(Debug, Clone)]
pub struct DeskUserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    /// Argon2id digest. `None` while the account is still `pending`.
    pub password_hash: Option<String>,
    /// TOTP secret, still sealed - unsealing is `crypto::vault`'s job.
    pub totp_secret: Option<Vec<u8>>,
    pub totp_confirmed_at: Option<DateTime<Utc>>,
    pub status: DeskUserStatus,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_signed_in_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

impl DeskUserRecord {
    /// Whether a lockout is in force right now.
    pub fn is_locked(&self) -> bool {
        self.locked_until.is_some_and(|until| until > Utc::now())
    }

    /// Whether this account has everything a sign-in needs.
    ///
    /// The same condition the check constraint enforces, asked in Rust so a
    /// caller can answer "why not" rather than only failing a write.
    pub fn is_complete(&self) -> bool {
        self.password_hash.is_some()
            && self.totp_secret.is_some()
            && self.totp_confirmed_at.is_some()
    }
}

/// `status` is TEXT in Postgres and an enum here, so the conversion happens in
/// one place rather than by deriving `FromRow` on the raw column.
impl<'r> FromRow<'r, sqlx::postgres::PgRow> for DeskUserRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let raw_status: String = row.try_get("status")?;
        let status =
            DeskUserStatus::parse(&raw_status).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "status".to_owned(),
                source: format!("unrecognised desk user status '{raw_status}'").into(),
            })?;

        Ok(Self {
            id: row.try_get("id")?,
            email: row.try_get("email")?,
            display_name: row.try_get("display_name")?,
            password_hash: row.try_get("password_hash")?,
            totp_secret: row.try_get("totp_secret")?,
            totp_confirmed_at: row.try_get("totp_confirmed_at")?,
            status,
            failed_attempts: row.try_get("failed_attempts")?,
            locked_until: row.try_get("locked_until")?,
            last_signed_in_at: row.try_get("last_signed_in_at")?,
            created_at: row.try_get("created_at")?,
            disabled_at: row.try_get("disabled_at")?,
        })
    }
}

/// A desk account to create. Carries no credential: the password arrives later,
/// from the person who will use it.
pub struct NewDeskUser<'a> {
    pub email: &'a str,
    pub display_name: &'a str,
    /// SHA-256 of the single-use setup token.
    pub setup_token_hash: &'a [u8],
    pub setup_expires_at: DateTime<Utc>,
}

// sqlx 0.9 only accepts `&'static str` as SQL unless the string is explicitly
// asserted safe, so these are literals rather than assembled at runtime - and
// the column list is repeated in each rather than shared, which is the price of
// that rule.
const SELECT_BY_EMAIL: &str = "SELECT id, email, display_name, password_hash, totp_secret, \
     totp_confirmed_at, status, failed_attempts, locked_until, last_signed_in_at, created_at, \
     disabled_at FROM desk_users WHERE email = lower($1)";

const SELECT_BY_ID: &str = "SELECT id, email, display_name, password_hash, totp_secret, \
     totp_confirmed_at, status, failed_attempts, locked_until, last_signed_in_at, created_at, \
     disabled_at FROM desk_users WHERE id = $1";

const SELECT_BY_SETUP_TOKEN: &str = "SELECT id, email, display_name, password_hash, totp_secret, \
     totp_confirmed_at, status, failed_attempts, locked_until, last_signed_in_at, created_at, \
     disabled_at FROM desk_users \
     WHERE setup_token_hash = $1 AND setup_expires_at > now() AND status <> 'disabled'";

const SELECT_ALL: &str = "SELECT id, email, display_name, password_hash, totp_secret, \
     totp_confirmed_at, status, failed_attempts, locked_until, last_signed_in_at, created_at, \
     disabled_at FROM desk_users ORDER BY lower(display_name), email";

const INSERT: &str = "INSERT INTO desk_users \
     (email, display_name, setup_token_hash, setup_expires_at) \
     VALUES (lower($1), $2, $3, $4) \
     RETURNING id, email, display_name, password_hash, totp_secret, totp_confirmed_at, \
     status, failed_attempts, locked_until, last_signed_in_at, created_at, disabled_at";

/// Find an account by address. Case-insensitive: addresses are stored
/// lowercased, and a sign-in form is not.
pub async fn find_by_email<'e, E>(
    executor: E,
    email: &str,
) -> Result<Option<DeskUserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskUserRecord>(SELECT_BY_EMAIL)
        .bind(email)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

pub async fn find<'e, E>(executor: E, id: Uuid) -> Result<Option<DeskUserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskUserRecord>(SELECT_BY_ID)
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

/// Find the account a setup link belongs to.
///
/// Expiry is in the `WHERE` clause rather than checked afterwards, so a link
/// that has run out cannot be honoured by a caller that forgot to look.
pub async fn find_by_setup_token<'e, E>(
    executor: E,
    token_hash: &[u8],
) -> Result<Option<DeskUserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskUserRecord>(SELECT_BY_SETUP_TOKEN)
        .bind(token_hash)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

pub async fn list<'e, E>(executor: E) -> Result<Vec<DeskUserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskUserRecord>(SELECT_ALL)
        .fetch_all(executor)
        .await
        .map_err(DbError::Query)
}

/// Create a `pending` account holding a setup link and nothing else.
pub async fn insert<'e, E>(executor: E, new: NewDeskUser<'_>) -> Result<DeskUserRecord, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskUserRecord>(INSERT)
        .bind(new.email)
        .bind(new.display_name)
        .bind(new.setup_token_hash)
        .bind(new.setup_expires_at)
        .fetch_one(executor)
        .await
        .map_err(|err| match err {
            sqlx::Error::Database(ref db) if db.is_unique_violation() => {
                DbError::UserExists(new.email.to_owned())
            }
            other => DbError::Query(other),
        })
}

/// Put an unconfirmed TOTP secret on a pending account.
///
/// Written when the enrolment page is *drawn*, not when it is submitted, so the
/// secret never travels back through the form. A hidden field would work - the
/// value is one the person is being shown anyway - but it would also let a
/// client choose the secret its own account is verified against, and there is
/// no reason to accept that.
///
/// Unconfirmed on purpose: `totp_confirmed_at` stays null until a code proves
/// the authenticator actually holds it, and an unconfirmed factor can never
/// satisfy a challenge. Re-drawing the page issues a fresh secret and discards
/// the last, which is what somebody who scanned it wrong needs.
pub async fn stage_totp_secret<'e, E>(
    executor: E,
    token_hash: &[u8],
    sealed_totp_secret: &[u8],
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query(
        "UPDATE desk_users \
            SET totp_secret = $2, totp_confirmed_at = NULL, updated_at = now() \
          WHERE setup_token_hash = $1 AND setup_expires_at > now() AND status <> 'disabled'",
    )
    .bind(token_hash)
    .bind(sealed_totp_secret)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(done.rows_affected() == 1)
}

/// Finish setup: a password of their own, the staged authenticator confirmed,
/// and the link spent.
///
/// One statement, because these facts becoming true separately is what leaves
/// an account that can sign in with no second factor. The `WHERE` clause
/// re-checks the token, so a link cannot be used twice even if two browsers
/// post the form at the same moment, and requires the secret to be there - an
/// account cannot go `active` without one, which is the same thing the check
/// constraint says.
pub async fn complete_setup<'e, E>(
    executor: E,
    token_hash: &[u8],
    password_hash: &str,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query(
        "UPDATE desk_users \
            SET password_hash = $2, \
                totp_confirmed_at = now(), \
                status = 'active', \
                setup_token_hash = NULL, \
                setup_expires_at = NULL, \
                failed_attempts = 0, \
                locked_until = NULL, \
                updated_at = now() \
          WHERE setup_token_hash = $1 \
            AND setup_expires_at > now() \
            AND status <> 'disabled' \
            AND totp_secret IS NOT NULL",
    )
    .bind(token_hash)
    .bind(password_hash)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(done.rows_affected() == 1)
}

/// Issue (or reissue) a setup link for an account that has not finished one.
pub async fn set_setup_token<'e, E>(
    executor: E,
    id: Uuid,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE desk_users \
            SET setup_token_hash = $2, setup_expires_at = $3, updated_at = now() \
          WHERE id = $1 AND status <> 'disabled'",
    )
    .bind(id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Count a failed sign-in, and lock the account when the count reaches
/// `lock_after`.
///
/// Returns the new count. The lock deadline is computed in SQL so two
/// simultaneous failures cannot both read the old count and write the same new
/// one.
pub async fn record_failed_attempt<'e, E>(
    executor: E,
    id: Uuid,
    lock_after: i32,
    lock_for: Duration,
) -> Result<i32, DbError>
where
    E: PgExecutor<'e>,
{
    let seconds = lock_for.num_seconds().max(0);

    let row = sqlx::query(
        "UPDATE desk_users \
            SET failed_attempts = failed_attempts + 1, \
                locked_until = CASE \
                    WHEN failed_attempts + 1 >= $2 THEN now() + make_interval(secs => $3) \
                    ELSE locked_until \
                END, \
                updated_at = now() \
          WHERE id = $1 \
      RETURNING failed_attempts",
    )
    .bind(id)
    .bind(lock_after)
    .bind(seconds as f64)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    row.try_get("failed_attempts").map_err(DbError::Query)
}

/// A successful sign-in: the counter goes back to zero and the lock lifts.
pub async fn record_sign_in<'e, E>(executor: E, id: Uuid) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE desk_users \
            SET failed_attempts = 0, locked_until = NULL, \
                last_signed_in_at = now(), updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Disable an account, or bring a disabled one back to `pending`.
///
/// There is no path back to `active` here: an account that was disabled has to
/// go through setup again, because the reason it was disabled is usually that
/// somebody left and their password is a thing they still remember.
pub async fn set_disabled<'e, E>(executor: E, id: Uuid, disabled: bool) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    if disabled {
        sqlx::query(
            "UPDATE desk_users \
                SET status = 'disabled', disabled_at = now(), \
                    setup_token_hash = NULL, setup_expires_at = NULL, updated_at = now() \
              WHERE id = $1",
        )
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    } else {
        sqlx::query(
            "UPDATE desk_users \
                SET status = 'pending', disabled_at = NULL, \
                    password_hash = NULL, totp_secret = NULL, totp_confirmed_at = NULL, \
                    updated_at = now() \
              WHERE id = $1 AND status = 'disabled'",
        )
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    }

    Ok(())
}

/// How many accounts can still sign in.
///
/// Asked before disabling one: a Desk with no usable account is a box somebody
/// has to SSH into to recover, and the last account is exactly the one nobody
/// notices they are removing.
pub async fn active_count<'e, E>(executor: E) -> Result<i64, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query("SELECT count(*) AS live FROM desk_users WHERE status = 'active'")
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)?;

    row.try_get("live").map_err(DbError::Query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_survives_a_round_trip() {
        for status in [
            DeskUserStatus::Pending,
            DeskUserStatus::Active,
            DeskUserStatus::Disabled,
        ] {
            assert_eq!(DeskUserStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(DeskUserStatus::parse("retired"), None);
    }

    #[test]
    fn only_an_active_account_may_sign_in() {
        assert!(DeskUserStatus::Active.may_sign_in());
        assert!(!DeskUserStatus::Pending.may_sign_in());
        assert!(!DeskUserStatus::Disabled.may_sign_in());
    }

    /// Every statement that feeds `FromRow` has to select every column it
    /// reads, and a miss is a decode error at runtime rather than a compile
    /// error. The lists are written out per statement, so this is the only
    /// thing that holds them together.
    #[test]
    fn every_select_covers_what_from_row_reads() {
        for column in [
            "id",
            "email",
            "display_name",
            "password_hash",
            "totp_secret",
            "totp_confirmed_at",
            "status",
            "failed_attempts",
            "locked_until",
            "last_signed_in_at",
            "created_at",
            "disabled_at",
        ] {
            for (name, statement) in [
                ("the sign-in lookup", SELECT_BY_EMAIL),
                ("the lookup by id", SELECT_BY_ID),
                ("the setup-token lookup", SELECT_BY_SETUP_TOKEN),
                ("the listing", SELECT_ALL),
                ("the insert", INSERT),
            ] {
                assert!(
                    statement.contains(column),
                    "{column} is missing from {name}"
                );
            }
        }
    }
}
