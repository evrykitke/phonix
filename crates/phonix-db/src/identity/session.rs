//! The `sessions` table.
//!
//! The cookie holds 32 random bytes; this table holds their SHA-256 digest
//! and everything else. **Every function here takes the digest**, never the
//! token: minting one and digesting it are the application layer's job
//! (`phonix_services::identity::session`), so no repository in this crate ever
//! holds a credential a client could present. The cost is one indexed lookup per request. What it
//! buys is revocation that actually works - sign out everywhere, suspend an
//! account, respond to a lost laptop - which a self-contained signed token
//! cannot do without exactly this table anyway.
//!
//! Two deadlines, both enforced in SQL rather than in Rust:
//!
//! * `expires_at` slides forward with activity;
//! * `absolute_expires_at` never moves.
//!
//! Putting both in the `WHERE` clause means an expired session cannot be
//! resurrected by a code path that forgets to check, and the sweeper is only
//! housekeeping rather than a correctness requirement.

use chrono::{DateTime, Duration, Utc};
use phonix_config::SessionConfig;
use phonix_core::identity::{SessionKind, UserId};
use sqlx::{FromRow, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// A session row, without the token.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user_id: UserId,
    /// Whether this session was opened by a browser or by a phone.
    ///
    /// Read on every request rather than only at sign-in, because it decides
    /// which idle window `touch` slides the deadline by.
    pub kind: SessionKind,
    /// Whether this session cleared the second factor.
    pub mfa_satisfied: bool,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

impl SessionRecord {
    /// Seconds until the deadline activity cannot extend.
    ///
    /// The absolute one, not the sliding one: the idle deadline moves on every
    /// request, so a number derived from it is stale before the response is
    /// written. This is the moment a real sign-in becomes necessary, which is
    /// the only expiry worth telling a client about. Floored at zero, because
    /// a negative "expires in" is a number no client handles well.
    pub fn remaining_secs(&self) -> i64 {
        (self.absolute_expires_at - Utc::now()).num_seconds().max(0)
    }
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for SessionRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            // A stored kind this build does not know is a decode error, not a
            // default: guessing 'browser' would sign a phone out on a
            // browser's schedule, which reads as an application that randomly
            // forgets people.
            kind: row
                .try_get::<String, _>("kind")?
                .parse()
                .map_err(|err: phonix_core::identity::UnknownSessionKind| {
                    sqlx::Error::ColumnDecode {
                        index: "kind".to_owned(),
                        source: Box::new(std::io::Error::other(err.to_string())),
                    }
                })?,
            mfa_satisfied: row.try_get("mfa_satisfied")?,
            ip: row.try_get("ip")?,
            user_agent: row.try_get("user_agent")?,
            created_at: row.try_get("created_at")?,
            last_seen_at: row.try_get("last_seen_at")?,
            expires_at: row.try_get("expires_at")?,
            absolute_expires_at: row.try_get("absolute_expires_at")?,
        })
    }
}

/// What a client sent along with the request.
#[derive(Debug, Clone, Default)]
pub struct ClientFacts<'a> {
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
}

const INSERT_SESSION: &str = "INSERT INTO sessions \
     (user_id, token_hash, kind, mfa_satisfied, ip, user_agent, expires_at, absolute_expires_at) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
     RETURNING id, user_id, kind, mfa_satisfied, ip, user_agent, created_at, last_seen_at, \
     expires_at, absolute_expires_at";

/// The two deadlines a session of this kind lives by, in minutes and hours.
///
/// One place, because `create` picks them once and `touch` has to keep picking
/// the same idle window on every request afterwards. Two copies of this choice
/// is how a phone ends up opened on a 90-day ceiling and slid on a browser's
/// 12-hour window.
///
/// `remember_me` is a checkbox on a sign-in form and a mobile application has
/// no such form, so it is ignored for [`SessionKind::Mobile`] rather than
/// making its ceiling depend on something no client can send.
fn windows(cfg: &SessionConfig, kind: SessionKind, remember_me: bool) -> (u64, u64) {
    match kind {
        SessionKind::Browser if remember_me => (cfg.idle_timeout_mins, cfg.remember_me_days * 24),
        SessionKind::Browser => (cfg.idle_timeout_mins, cfg.absolute_timeout_hours),
        SessionKind::Mobile => (
            cfg.mobile.idle_timeout_mins,
            cfg.mobile.absolute_timeout_hours(),
        ),
    }
}

/// Open a session for a user.
///
/// `token_hash` is the digest of a token the caller has already minted, and
/// `mfa_satisfied` is false when a second factor is still outstanding - the
/// session exists so the challenge page has something to attach to, but until
/// the flag is set the caller is not really signed in.
#[allow(clippy::too_many_arguments)]
pub async fn create<'e, E>(
    executor: E,
    user_id: UserId,
    token_hash: &[u8],
    cfg: &SessionConfig,
    kind: SessionKind,
    remember_me: bool,
    mfa_satisfied: bool,
    facts: ClientFacts<'_>,
) -> Result<SessionRecord, DbError>
where
    E: PgExecutor<'e>,
{
    let now = Utc::now();
    let (idle_mins, absolute_hours) = windows(cfg, kind, remember_me);

    let absolute_expires_at = now + Duration::hours(absolute_hours as i64);

    // Clamped, because the idle window may be configured longer than the
    // absolute one for a non-remembered session, and the schema constraint
    // `absolute_expires_at >= expires_at` would reject the insert.
    let expires_at = (now + Duration::minutes(idle_mins as i64)).min(absolute_expires_at);

    sqlx::query_as::<_, SessionRecord>(INSERT_SESSION)
        .bind(user_id)
        .bind(token_hash)
        .bind(kind.as_str())
        .bind(mfa_satisfied)
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
/// arriving just as a session expires could read it as valid and then extend a
/// session that another request had already seen as dead.
///
/// Returns `None` for a token that is unknown, expired or revoked - the three
/// are indistinguishable to the caller on purpose.
pub async fn touch<'e, E>(
    executor: E,
    token_hash: &[u8],
    cfg: &SessionConfig,
) -> Result<Option<SessionRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, SessionRecord>(
        "UPDATE sessions
            SET last_seen_at = now(),
                -- Never past the absolute ceiling: activity extends a session,
                -- it does not make one immortal.
                --
                -- The window is chosen from the row's own kind rather than
                -- passed in, because this statement is reached from a request
                -- that has only a token: whoever is calling does not yet know
                -- whether it came from a phone or a browser, and asking first
                -- would be a second round trip to answer a question this row
                -- already holds.
                expires_at = least(
                    now() + (
                        CASE kind
                            WHEN 'mobile' THEN $3::bigint
                            ELSE $2::bigint
                        END * interval '1 minute'
                    ),
                    absolute_expires_at
                )
          WHERE token_hash = $1
            AND revoked_at IS NULL
            AND expires_at > now()
            AND absolute_expires_at > now()
      RETURNING id, user_id, kind, mfa_satisfied, ip, user_agent, created_at, last_seen_at,
                expires_at, absolute_expires_at",
    )
    .bind(token_hash)
    .bind(cfg.idle_timeout_mins as i64)
    .bind(cfg.mobile.idle_timeout_mins as i64)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

/// Read a session without extending it. For listing active sessions.
pub async fn find<'e, E>(executor: E, token_hash: &[u8]) -> Result<Option<SessionRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, SessionRecord>(
        "SELECT id, user_id, kind, mfa_satisfied, ip, user_agent, created_at, last_seen_at,
                expires_at, absolute_expires_at
           FROM sessions
          WHERE token_hash = $1
            AND revoked_at IS NULL
            AND expires_at > now()
            AND absolute_expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

/// Start the MFA challenge clock on a half-authenticated session.
///
/// A separate deadline from the session's own, and much shorter: a session
/// lives for hours, but a proven password waiting at a code box must not.
pub async fn start_mfa_challenge<'e, E>(executor: E, id: Uuid, ttl_mins: i64) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE sessions
            SET mfa_challenge_expires_at = now() + ($2::bigint * interval '1 minute')
          WHERE id = $1",
    )
    .bind(id)
    .bind(ttl_mins)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

/// How the outstanding challenge on a session stands.
#[derive(Debug, Clone, Copy)]
pub struct ChallengeState {
    pub attempts: i32,
    /// True when the challenge deadline has passed, or was never set.
    pub expired: bool,
}

/// Read the challenge attached to a session.
///
/// Returns `None` when the session is unknown, revoked, expired or already past
/// its second factor - all of which mean there is no challenge to answer.
pub async fn challenge_state<'e, E>(
    executor: E,
    id: Uuid,
) -> Result<Option<ChallengeState>, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(
        "SELECT mfa_attempts,
                (mfa_challenge_expires_at IS NULL OR mfa_challenge_expires_at <= now())
                    AS expired
           FROM sessions
          WHERE id = $1
            AND revoked_at IS NULL
            AND NOT mfa_satisfied
            AND expires_at > now()
            AND absolute_expires_at > now()",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    let Some(row) = row else {
        return Ok(None);
    };

    Ok(Some(ChallengeState {
        attempts: row.try_get("mfa_attempts").map_err(DbError::Query)?,
        expired: row.try_get("expired").map_err(DbError::Query)?,
    }))
}

/// Count one wrong code, returning the new total.
///
/// Incremented and read in one statement: two codes submitted at once must not
/// both read the same count and each conclude one attempt was used.
pub async fn record_mfa_attempt<'e, E>(executor: E, id: Uuid) -> Result<i32, DbError>
where
    E: PgExecutor<'e>,
{
    let attempts: i32 = sqlx::query_scalar(
        "UPDATE sessions SET mfa_attempts = mfa_attempts + 1
          WHERE id = $1
      RETURNING mfa_attempts",
    )
    .bind(id)
    .fetch_one(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(attempts)
}

/// Mark a session as having cleared the second factor.
pub async fn mark_mfa_satisfied<'e, E>(executor: E, id: Uuid) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    // The challenge deadline goes with it: a satisfied session has no
    // outstanding challenge, and leaving a stale deadline behind would make
    // `challenge_state` describe one that no longer exists.
    sqlx::query(
        "UPDATE sessions
            SET mfa_satisfied = TRUE, mfa_challenge_expires_at = NULL
          WHERE id = $1",
    )
    .bind(id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

/// End one session. Idempotent: revoking an already-revoked session is a no-op.
pub async fn revoke<'e, E>(executor: E, token_hash: &[u8], reason: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE sessions
            SET revoked_at = now(), revoked_reason = $2
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
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $2
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(reason)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

/// End every session a user holds. Returns how many were live.
///
/// This is what "sign out everywhere" and "suspend this account" both call, and
/// it is the reason sessions are rows rather than self-contained tokens.
pub async fn revoke_all_for_user<'e, E>(
    executor: E,
    user_id: UserId,
    reason: &str,
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE sessions
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

/// End every session a user holds except the one presented.
///
/// What a password change calls: the person doing it keeps the tab they are
/// working in, and everything else - including whoever prompted the change by
/// having their password - is signed out.
pub async fn revoke_all_for_user_except<'e, E>(
    executor: E,
    user_id: UserId,
    keep_token_hash: &[u8],
    reason: &str,
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "UPDATE sessions
            SET revoked_at = now(), revoked_reason = $3
          WHERE user_id = $1 AND revoked_at IS NULL AND token_hash <> $2",
    )
    .bind(user_id)
    .bind(keep_token_hash)
    .bind(reason)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}

/// Every live session for a user, newest first. Backs a "your devices" screen.
pub async fn list_for_user<'e, E>(
    executor: E,
    user_id: UserId,
) -> Result<Vec<SessionRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, SessionRecord>(
        "SELECT id, user_id, mfa_satisfied, ip, user_agent, created_at, last_seen_at,
                expires_at, absolute_expires_at
           FROM sessions
          WHERE user_id = $1
            AND revoked_at IS NULL
            AND expires_at > now()
            AND absolute_expires_at > now()
          ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)
}

/// Delete sessions that are past their absolute deadline, plus revoked ones
/// that have aged out.
///
/// Pure housekeeping - the `WHERE` clauses above already refuse to hand back an
/// expired session, so skipping this loses disk, not safety. Revoked rows are
/// kept for a week first so "when was I signed out?" remains answerable.
pub async fn purge_expired<'e, E>(executor: E) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "DELETE FROM sessions
          WHERE absolute_expires_at < now() - interval '7 days'
             OR (revoked_at IS NOT NULL AND revoked_at < now() - interval '7 days')",
    )
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SessionConfig {
        SessionConfig {
            cookie_name: "phonix_session".into(),
            idle_timeout_mins: 720,
            absolute_timeout_hours: 168,
            remember_me_days: 30,
            secure: false,
            same_site: phonix_config::SameSitePolicy::Lax,
            handoff_ttl_secs: 120,
            purge_interval_mins: 60,
            mobile: phonix_config::MobileSessionConfig {
                idle_timeout_mins: 43_200,
                absolute_timeout_days: 90,
            },
        }
    }

    /// The deadlines `create` would compute, from the same `windows` it uses.
    ///
    /// Deliberately not a second copy of the arithmetic: a helper that mirrored
    /// it would keep passing on the day the real one changed, which is the only
    /// day this test matters.
    fn deadlines(
        cfg: &SessionConfig,
        kind: SessionKind,
        remember_me: bool,
    ) -> (DateTime<Utc>, DateTime<Utc>) {
        let now = Utc::now();
        let (idle_mins, absolute_hours) = windows(cfg, kind, remember_me);

        let absolute = now + Duration::hours(absolute_hours as i64);
        let idle = (now + Duration::minutes(idle_mins as i64)).min(absolute);
        (idle, absolute)
    }

    #[test]
    fn remember_me_extends_only_the_absolute_deadline() {
        let cfg = config();

        let (idle, absolute) = deadlines(&cfg, SessionKind::Browser, false);
        let (remembered_idle, remembered_absolute) = deadlines(&cfg, SessionKind::Browser, true);

        // The idle window is the same either way - "remember me" is about how
        // long you may stay away, not how long a single visit lasts.
        assert!((remembered_idle - idle).num_seconds().abs() <= 1);
        assert!(remembered_absolute > absolute);
        assert!((remembered_absolute - Utc::now()).num_days() >= 29);
    }

    #[test]
    fn the_idle_deadline_is_clamped_to_the_absolute_one() {
        // A deliberately silly configuration: idle longer than absolute.
        let mut cfg = config();
        cfg.idle_timeout_mins = 60 * 24 * 30;
        cfg.absolute_timeout_hours = 1;

        let (idle, absolute) = deadlines(&cfg, SessionKind::Browser, false);

        // Unclamped this would violate `absolute_expires_at >= expires_at` and
        // the insert would be rejected by the schema.
        assert!(
            idle <= absolute,
            "idle {idle} must not exceed absolute {absolute}"
        );
    }

    #[test]
    fn a_phone_lives_by_its_own_deadlines_and_they_are_longer() {
        let cfg = config();

        let (browser_idle, browser_absolute) = deadlines(&cfg, SessionKind::Browser, false);
        let (mobile_idle, mobile_absolute) = deadlines(&cfg, SessionKind::Mobile, false);

        // The whole reason the column exists. A phone signed out on a
        // browser's schedule is an application people stop opening.
        assert!(mobile_idle > browser_idle);
        assert!(mobile_absolute > browser_absolute);
        assert!((mobile_absolute - Utc::now()).num_days() >= 89);
    }

    #[test]
    fn remember_me_is_ignored_for_a_phone() {
        // It is a checkbox on a sign-in form and a mobile application has no
        // such form, so honouring it would make a phone's ceiling depend on
        // something no client can send - and on a *browser's* setting at that.
        let cfg = config();

        assert_eq!(
            windows(&cfg, SessionKind::Mobile, false),
            windows(&cfg, SessionKind::Mobile, true),
        );
    }

    #[test]
    fn the_mobile_deadlines_come_from_the_mobile_block() {
        let mut cfg = config();
        cfg.mobile.idle_timeout_mins = 111;
        cfg.mobile.absolute_timeout_days = 7;

        assert_eq!(windows(&cfg, SessionKind::Mobile, false), (111, 7 * 24));
        // And changing them leaves a browser alone.
        assert_eq!(
            windows(&cfg, SessionKind::Browser, false),
            (cfg.idle_timeout_mins, cfg.absolute_timeout_hours),
        );
    }
}
