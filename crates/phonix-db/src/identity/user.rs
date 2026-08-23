//! The `users` table.
//!
//! Every function takes a `&PgPool` for one *tenant's* database. There is no
//! tenant filter anywhere below and there must not be: isolation here is the
//! database boundary, so a query that reaches the wrong tenant is a routing
//! bug, not a missing `WHERE`.

use chrono::{DateTime, Utc};
use phonix_config::LockoutConfig;
use phonix_core::PermissionSet;
use phonix_core::identity::{AuthUser, UserId, UserListing, UserStatus};
use sqlx::{FromRow, PgExecutor, Row};

use crate::error::DbError;

/// One row of `users`, in full.
///
/// Distinct from [`AuthUser`], which is the narrow projection sent to the
/// browser. Anything sensitive - the hash, the lockout counters, the MFA state
/// - lives here and stops here.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: UserId,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub display_name: String,

    /// Argon2id PHC string. `None` only for an invited account that has not
    /// chosen a password yet; the schema forbids it for an active one.
    pub password_hash: Option<String>,
    pub password_updated_at: Option<DateTime<Utc>>,
    pub must_change_password: bool,

    pub status: UserStatus,
    pub is_owner: bool,
    pub email_verified_at: Option<DateTime<Utc>>,

    pub mfa_enabled: bool,
    pub mfa_required: bool,

    pub failed_login_count: i32,
    pub locked_until: Option<DateTime<Utc>>,

    pub last_login_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,

    pub locale: String,
    pub timezone: String,
    pub avatar_url: Option<String>,

    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl UserRecord {
    /// Whether a lockout is currently in force.
    pub fn is_locked(&self, now: DateTime<Utc>) -> bool {
        self.locked_until.is_some_and(|until| until > now)
    }

    /// Seconds left on a lockout, for the message under the login form.
    pub fn lockout_remaining_secs(&self, now: DateTime<Utc>) -> u64 {
        self.locked_until
            .map(|until| (until - now).num_seconds().max(0) as u64)
            .unwrap_or(0)
    }

    /// Whether this account may hold a session right now.
    pub fn can_sign_in(&self, now: DateTime<Utc>) -> bool {
        self.deleted_at.is_none() && self.status.can_sign_in() && !self.is_locked(now)
    }

    /// Project into the shape the browser receives.
    ///
    /// Takes the resolved permissions rather than loading them, so the caller
    /// decides when that query happens and cannot accidentally issue it once
    /// per user while rendering a list.
    pub fn to_auth_user(
        &self,
        roles: Vec<String>,
        permissions: PermissionSet,
        mfa_satisfied: bool,
    ) -> AuthUser {
        AuthUser {
            id: self.id,
            email: self.email.clone(),
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            display_name: self.display_name.clone(),
            roles,
            permissions,
            is_owner: self.is_owner,
            status: self.status,
            mfa_enabled: self.mfa_enabled,
            mfa_satisfied,
            email_verified: self.email_verified_at.is_some(),
        }
    }
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for UserRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let raw_status: String = row.try_get("status")?;
        let status = UserStatus::parse(&raw_status).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "status".to_owned(),
            source: format!("unrecognised user status '{raw_status}'").into(),
        })?;

        Ok(Self {
            id: row.try_get("id")?,
            email: row.try_get("email")?,
            first_name: row.try_get("first_name")?,
            last_name: row.try_get("last_name")?,
            display_name: row.try_get("display_name")?,
            password_hash: row.try_get("password_hash")?,
            password_updated_at: row.try_get("password_updated_at")?,
            must_change_password: row.try_get("must_change_password")?,
            status,
            is_owner: row.try_get("is_owner")?,
            email_verified_at: row.try_get("email_verified_at")?,
            mfa_enabled: row.try_get("mfa_enabled")?,
            mfa_required: row.try_get("mfa_required")?,
            failed_login_count: row.try_get("failed_login_count")?,
            locked_until: row.try_get("locked_until")?,
            last_login_at: row.try_get("last_login_at")?,
            last_seen_at: row.try_get("last_seen_at")?,
            locale: row.try_get("locale")?,
            timezone: row.try_get("timezone")?,
            avatar_url: row.try_get("avatar_url")?,
            created_at: row.try_get("created_at")?,
            deleted_at: row.try_get("deleted_at")?,
        })
    }
}

/// A user about to be created.
#[derive(Debug, Clone)]
pub struct NewUser<'a> {
    /// Already lowercased and trimmed by `phonix_core::identity`.
    pub email: &'a str,
    pub first_name: &'a str,
    pub last_name: &'a str,
    /// Argon2id PHC string. `None` for an invitation awaiting acceptance.
    pub password_hash: Option<&'a str>,
    pub status: UserStatus,
    /// Created the workspace. At most one per tenant; the schema enforces it.
    pub is_owner: bool,
    pub invited_by: Option<UserId>,
}

// sqlx 0.9 accepts only `&'static str` as SQL unless the string is explicitly
// asserted safe, so the column list is repeated in each query rather than
// shared through a runtime `format!`. `FromRow` above is what keeps them
// honest: a column dropped from one of these fails that query's decode.
const INSERT_USER: &str = "INSERT INTO users \
     (email, first_name, last_name, display_name, password_hash, password_updated_at, \
      status, is_owner, invited_by, invited_at) \
     VALUES ($1, $2, $3, $4, $5, CASE WHEN $5 IS NULL THEN NULL ELSE now() END, \
             $6, $7, $8, CASE WHEN $8 IS NULL THEN NULL ELSE now() END) \
     RETURNING id, email, first_name, last_name, display_name, password_hash, \
     password_updated_at, must_change_password, status, is_owner, email_verified_at, \
     mfa_enabled, mfa_required, failed_login_count, locked_until, last_login_at, \
     last_seen_at, locale, timezone, avatar_url, created_at, deleted_at";

const SELECT_BY_EMAIL: &str = "SELECT id, email, first_name, last_name, display_name, \
     password_hash, password_updated_at, must_change_password, status, is_owner, \
     email_verified_at, mfa_enabled, mfa_required, failed_login_count, locked_until, \
     last_login_at, last_seen_at, locale, timezone, avatar_url, created_at, deleted_at \
     FROM users WHERE lower(email) = lower($1) AND deleted_at IS NULL";

const SELECT_BY_ID: &str = "SELECT id, email, first_name, last_name, display_name, \
     password_hash, password_updated_at, must_change_password, status, is_owner, \
     email_verified_at, mfa_enabled, mfa_required, failed_login_count, locked_until, \
     last_login_at, last_seen_at, locale, timezone, avatar_url, created_at, deleted_at \
     FROM users WHERE id = $1 AND deleted_at IS NULL";

const SELECT_ALL: &str = "SELECT id, email, first_name, last_name, display_name, \
     password_hash, password_updated_at, must_change_password, status, is_owner, \
     email_verified_at, mfa_enabled, mfa_required, failed_login_count, locked_until, \
     last_login_at, last_seen_at, locale, timezone, avatar_url, created_at, deleted_at \
     FROM users WHERE deleted_at IS NULL ORDER BY created_at";

/// Insert a user.
///
/// `display_name` is derived here rather than taken, so it cannot drift from
/// the names it is built out of.
pub async fn create<'e, E>(executor: E, new: NewUser<'_>) -> Result<UserRecord, DbError>
where
    E: PgExecutor<'e>,
{
    let display_name = format!("{} {}", new.first_name.trim(), new.last_name.trim())
        .trim()
        .to_owned();

    sqlx::query_as::<_, UserRecord>(INSERT_USER)
        .bind(new.email)
        .bind(new.first_name)
        .bind(new.last_name)
        .bind(&display_name)
        .bind(new.password_hash)
        .bind(new.status.as_str())
        .bind(new.is_owner)
        .bind(new.invited_by)
        .fetch_one(executor)
        .await
        .map_err(|err| match &err {
            // 23505 = unique_violation, i.e. users_active_email_key.
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
                DbError::UserExists(new.email.to_owned())
            }
            _ => DbError::Query(err),
        })
}

/// Find a user by address, case-insensitively. Soft-deleted rows are excluded.
pub async fn find_by_email<'e, E>(executor: E, email: &str) -> Result<Option<UserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, UserRecord>(SELECT_BY_EMAIL)
        .bind(email)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

pub async fn find_by_id<'e, E>(executor: E, id: UserId) -> Result<Option<UserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, UserRecord>(SELECT_BY_ID)
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

pub async fn list<'e, E>(executor: E) -> Result<Vec<UserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, UserRecord>(SELECT_ALL)
        .fetch_all(executor)
        .await
        .map_err(DbError::Query)
}

/// Every user, with their role names, for the administration screen.
///
/// One query rather than a list followed by a role lookup per row: the users
/// screen is the first place a workspace of any size feels an N+1, and the
/// aggregate costs nothing here.
///
/// Returns [`UserListing`] rather than [`UserRecord`] deliberately. The screen
/// has no business seeing a password hash, and a type that cannot carry one
/// cannot leak it into a template.
pub async fn listings<'e, E>(executor: E) -> Result<Vec<UserListing>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query(
        "SELECT u.id, u.email, u.display_name, u.status, u.is_owner,
                u.email_verified_at, u.mfa_enabled, u.locked_until,
                u.last_login_at, u.created_at,
                coalesce(
                    array_agg(r.name ORDER BY r.name) FILTER (WHERE r.name IS NOT NULL),
                    '{}'
                ) AS roles
           FROM users u
           LEFT JOIN user_roles ur ON ur.user_id = u.id
           LEFT JOIN roles r ON r.id = ur.role_id
          WHERE u.deleted_at IS NULL
          GROUP BY u.id
          ORDER BY u.display_name, u.email",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter()
        .map(|row| {
            let raw_status: String = row.try_get("status")?;
            let status =
                UserStatus::parse(&raw_status).ok_or_else(|| sqlx::Error::ColumnDecode {
                    index: "status".to_owned(),
                    source: format!("unrecognised user status '{raw_status}'").into(),
                })?;

            let email_verified_at: Option<DateTime<Utc>> = row.try_get("email_verified_at")?;

            Ok(UserListing {
                id: row.try_get("id")?,
                email: row.try_get("email")?,
                display_name: row.try_get("display_name")?,
                status,
                is_owner: row.try_get("is_owner")?,
                email_verified: email_verified_at.is_some(),
                mfa_enabled: row.try_get("mfa_enabled")?,
                roles: row.try_get("roles")?,
                locked_until: row.try_get("locked_until")?,
                last_login_at: row.try_get("last_login_at")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(DbError::Query)
}

pub async fn count<'e, E>(executor: E) -> Result<i64, DbError>
where
    E: PgExecutor<'e>,
{
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE deleted_at IS NULL")
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(count)
}

/// Clear the failure counters and stamp the sign-in.
pub async fn record_successful_login<'e, E>(
    executor: E,
    id: UserId,
    ip: Option<&str>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE users
            SET failed_login_count = 0,
                locked_until       = NULL,
                last_login_at      = now(),
                last_seen_at       = now(),
                last_login_ip      = $2
          WHERE id = $1",
    )
    .bind(id)
    .bind(ip)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Increment the failure counter, locking the account once it crosses the
/// configured threshold.
///
/// Counter and lock are set in one statement so two simultaneous wrong-password
/// attempts cannot read the same count and both decide not to lock.
///
/// Returns the lockout deadline, if the account is now locked.
pub async fn record_failed_login<'e, E>(
    executor: E,
    id: UserId,
    lockout: &LockoutConfig,
) -> Result<Option<DateTime<Utc>>, DbError>
where
    E: PgExecutor<'e>,
{
    // 0 disables lockout. A threshold of 0 with the SQL below would lock the
    // account on the very first typo.
    if lockout.max_failed_attempts <= 0 {
        sqlx::query(
            "UPDATE users
                SET failed_login_count   = failed_login_count + 1,
                    last_failed_login_at = now()
              WHERE id = $1",
        )
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
        return Ok(None);
    }

    let row = sqlx::query(
        "UPDATE users
            SET failed_login_count   = failed_login_count + 1,
                last_failed_login_at = now(),
                locked_until = CASE
                    WHEN failed_login_count + 1 >= $2
                    THEN now() + ($3::int * interval '1 minute')
                    ELSE locked_until
                END
          WHERE id = $1
      RETURNING locked_until",
    )
    .bind(id)
    .bind(lockout.max_failed_attempts)
    .bind(lockout.lockout_mins as i32)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    let locked_until: Option<DateTime<Utc>> = match row {
        Some(row) => row.try_get("locked_until").map_err(DbError::Query)?,
        None => None,
    };

    // Only report a lock that is actually in force; the column may still hold
    // an expired deadline from an earlier round.
    Ok(locked_until.filter(|until| *until > Utc::now()))
}

/// Lift a lock, e.g. after an administrator intervenes.
pub async fn clear_lockout<'e, E>(executor: E, id: UserId) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("UPDATE users SET failed_login_count = 0, locked_until = NULL WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

/// Replace the stored hash.
///
/// Also clears `must_change_password`, because the change has now happened.
pub async fn set_password_hash<'e, E>(executor: E, id: UserId, hash: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE users
            SET password_hash        = $2,
                password_algorithm   = 'argon2id',
                password_updated_at  = now(),
                must_change_password = FALSE
          WHERE id = $1",
    )
    .bind(id)
    .bind(hash)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Move `last_seen_at` forward.
///
/// Called on session activity, so it is deliberately a single unindexed write
/// with no read first.
pub async fn touch_last_seen<'e, E>(executor: E, id: UserId) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("UPDATE users SET last_seen_at = now() WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

/// Flag (or clear) a forced password change.
///
/// Set after an administrative reset: an administrator who knows a working
/// password can act as that user, and this is what closes it.
pub async fn set_must_change_password<'e, E>(
    executor: E,
    id: UserId,
    must_change: bool,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("UPDATE users SET must_change_password = $2 WHERE id = $1")
        .bind(id)
        .bind(must_change)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

/// Mark the address confirmed.
pub async fn mark_email_verified<'e, E>(executor: E, id: UserId) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE users
            SET email_verified_at = coalesce(email_verified_at, now()),
                status = CASE WHEN status = 'pending' THEN 'active' ELSE status END
          WHERE id = $1",
    )
    .bind(id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

/// Store a changed name.
///
/// `display_name` is written rather than derived in SQL: how a name reads is a
/// domain decision - `UserEdit::display_name` makes it - and a `CONCAT` here
/// would be a second, silently different answer.
pub async fn set_names<'e, E>(
    executor: E,
    id: UserId,
    first_name: &str,
    last_name: &str,
    display_name: &str,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE users
            SET first_name = $2, last_name = $3, display_name = $4
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(first_name)
    .bind(last_name)
    .bind(display_name)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Change a user's lifecycle state.
///
/// Refuses to touch the workspace owner: suspending them would leave nobody
/// able to administer the workspace, which is the one state this system must
/// not be able to reach.
pub async fn set_status<'e, E>(executor: E, id: UserId, status: UserStatus) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query("UPDATE users SET status = $2 WHERE id = $1 AND NOT is_owner")
        .bind(id)
        .bind(status.as_str())
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    if result.rows_affected() == 0 {
        return Err(DbError::OwnerProtected);
    }
    Ok(())
}

/// Soft-delete. The row stays for referential integrity and the audit trail;
/// the partial unique index frees the address for reuse.
pub async fn soft_delete<'e, E>(executor: E, id: UserId) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE users
            SET deleted_at = now(), status = 'deactivated'
          WHERE id = $1 AND deleted_at IS NULL AND NOT is_owner",
    )
    .bind(id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    if result.rows_affected() == 0 {
        return Err(DbError::OwnerProtected);
    }
    Ok(())
}

/// The workspace owner, if one is recorded.
pub async fn find_owner<'e, E>(executor: E) -> Result<Option<UserRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, UserRecord>(
        "SELECT id, email, first_name, last_name, display_name, password_hash, \
         password_updated_at, must_change_password, status, is_owner, email_verified_at, \
         mfa_enabled, mfa_required, failed_login_count, locked_until, last_login_at, \
         last_seen_at, locale, timezone, avatar_url, created_at, deleted_at \
         FROM users WHERE is_owner AND deleted_at IS NULL",
    )
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------

/// One row of the card query: everything `UserCard` needs, in one round trip.
///
/// Its own struct rather than `UserRecord` because it is a different set of
/// columns - it wants the picture and the roles, and it does not want the
/// password hash, the failed-login count or the lockout instant. Reading a
/// whole account row to draw a name and a job title would be pulling a
/// credential out of the database to render a tooltip.
#[derive(Debug, Clone)]
pub struct CardRow {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub status: UserStatus,
    pub is_owner: bool,
    pub avatar_file_id: Option<uuid::Uuid>,
    pub roles: Vec<String>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for CardRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let status: String = row.try_get("status")?;

        Ok(Self {
            id: row.try_get("id")?,
            email: row.try_get("email")?,
            display_name: row.try_get("display_name")?,
            // A status the CHECK constraint forbids cannot be stored, so an
            // unparseable one means the constraint was dropped. `Deactivated`
            // is the reading that claims least about an account nobody can
            // describe.
            status: UserStatus::parse(&status).unwrap_or(UserStatus::Deactivated),
            is_owner: row.try_get("is_owner")?,
            avatar_file_id: row.try_get("avatar_file_id")?,
            roles: row.try_get("roles")?,
            last_login_at: row.try_get("last_login_at")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Who somebody is, for a card beside their name.
///
/// One statement including the roles, because the card is fetched on demand for
/// one person and a second round trip to name their roles would double the
/// latency of the thing somebody just clicked.
///
/// A deleted account returns `None`. The trail still names them by the address
/// it stored, which is the point of storing it - but there is no profile left
/// to show, and inventing one would be worse than saying so.
pub async fn card<'e, E>(executor: E, id: UserId) -> Result<Option<CardRow>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, CardRow>(
        "SELECT u.id, u.email, u.display_name, u.status, u.is_owner, u.avatar_file_id,
                u.last_login_at, u.created_at,
                COALESCE(
                    ARRAY(
                        SELECT r.name
                          FROM roles r
                          JOIN user_roles ur ON ur.role_id = r.id
                         WHERE ur.user_id = u.id
                         ORDER BY r.is_static DESC, lower(r.name)
                    ),
                    '{}'
                ) AS roles
           FROM users u
          WHERE u.id = $1 AND u.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

// ---------------------------------------------------------------------------
// The profile picture
//
// `avatar_file_id` points at a row in `file_uploads` - a picture uploaded here,
// which therefore has a size, a type decided from its bytes and a verification
// state. It is not the same thing as `avatar_url`, which holds an address
// somewhere else entirely (a Gravatar, an identity provider's picture) and is
// left alone by everything below.
// ---------------------------------------------------------------------------

/// Point an account at an uploaded picture, and say which one it replaced.
///
/// The previous id comes back so the caller can delete the file it named:
/// swapping a picture must not leave the old one on disk for ever, and this is
/// the only moment anything knows both ids at once.
///
/// One statement rather than a read and a write, so two browser tabs racing to
/// change the same picture cannot both read the same "previous" id and leave
/// one of the two files stranded.
pub async fn set_avatar<'e, E>(
    executor: E,
    id: UserId,
    file_id: uuid::Uuid,
) -> Result<Option<uuid::Uuid>, DbError>
where
    E: PgExecutor<'e>,
{
    // `RETURNING` on an UPDATE sees the new row, so the old value has to be
    // captured before the assignment - which a CTE reading the row inside the
    // same statement does.
    let previous: Option<Option<uuid::Uuid>> = sqlx::query_scalar(
        "WITH previous AS (
             SELECT avatar_file_id FROM users WHERE id = $1
         )
         UPDATE users
            SET avatar_file_id = $2
           FROM previous
          WHERE users.id = $1
      RETURNING previous.avatar_file_id",
    )
    .bind(id)
    .bind(file_id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(previous.flatten())
}

/// Remove an account's uploaded picture, and say which one it was.
pub async fn clear_avatar<'e, E>(executor: E, id: UserId) -> Result<Option<uuid::Uuid>, DbError>
where
    E: PgExecutor<'e>,
{
    let previous: Option<Option<uuid::Uuid>> = sqlx::query_scalar(
        "WITH previous AS (
             SELECT avatar_file_id FROM users WHERE id = $1
         )
         UPDATE users
            SET avatar_file_id = NULL
           FROM previous
          WHERE users.id = $1
      RETURNING previous.avatar_file_id",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(previous.flatten())
}

/// Which uploaded picture an account is using, if any.
pub async fn avatar_file<'e, E>(executor: E, id: UserId) -> Result<Option<uuid::Uuid>, DbError>
where
    E: PgExecutor<'e>,
{
    let file_id: Option<Option<uuid::Uuid>> =
        sqlx::query_scalar("SELECT avatar_file_id FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(DbError::Query)?;

    Ok(file_id.flatten())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use uuid::Uuid;

    use super::*;

    fn record() -> UserRecord {
        UserRecord {
            id: Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            password_hash: Some("$argon2id$...".into()),
            password_updated_at: None,
            must_change_password: false,
            status: UserStatus::Active,
            is_owner: true,
            email_verified_at: None,
            mfa_enabled: false,
            mfa_required: false,
            failed_login_count: 0,
            locked_until: None,
            last_login_at: None,
            last_seen_at: None,
            locale: "en".into(),
            timezone: "UTC".into(),
            avatar_url: None,
            created_at: Utc::now(),
            deleted_at: None,
        }
    }

    #[test]
    fn a_lockout_expires_on_its_own() {
        let now = Utc::now();
        let mut user = record();

        assert!(!user.is_locked(now));
        assert!(user.can_sign_in(now));

        user.locked_until = Some(now + Duration::minutes(15));
        assert!(user.is_locked(now));
        assert!(!user.can_sign_in(now));
        assert_eq!(user.lockout_remaining_secs(now), 900);

        // Past deadlines are not locks.
        user.locked_until = Some(now - Duration::minutes(1));
        assert!(!user.is_locked(now));
        assert!(user.can_sign_in(now));
        assert_eq!(user.lockout_remaining_secs(now), 0);
    }

    #[test]
    fn every_reason_to_refuse_a_sign_in_is_checked() {
        let now = Utc::now();

        let mut suspended = record();
        suspended.status = UserStatus::Suspended;
        assert!(!suspended.can_sign_in(now));

        let mut pending = record();
        pending.status = UserStatus::Pending;
        assert!(!pending.can_sign_in(now));

        let mut deleted = record();
        deleted.deleted_at = Some(now);
        assert!(!deleted.can_sign_in(now));
    }

    #[test]
    fn the_browser_projection_drops_everything_sensitive() {
        let user = record();
        let auth = user.to_auth_user(vec!["Admin".into()], PermissionSet::all(), true);

        assert_eq!(auth.id, user.id);
        assert_eq!(auth.email, user.email);
        assert!(auth.is_owner);
        assert!(auth.mfa_satisfied);
        assert!(!auth.email_verified);

        // The projection is a struct, so "no hash field" is a compile-time
        // guarantee; this checks the serialised form for good measure.
        let json = serde_json::to_string(&auth).unwrap();
        assert!(
            !json.contains("argon2"),
            "the hash reached the client: {json}"
        );
        assert!(!json.contains("locked_until"));
        assert!(!json.contains("failed_login"));
    }
}
