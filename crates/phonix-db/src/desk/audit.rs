//! The `desk_audit` table.
//!
//! What a desk user did, in the catalog, where the workspace it was done to
//! cannot read, edit or lose it. That placement is the whole point: "who
//! suspended this workspace" is not a fact its own administrators get to hold.
//!
//! # Two rules this table follows
//!
//! **A change is recorded from → to**, the shape the tenant entity trail
//! already uses, because that shape is what earns a diff on a detail page.
//! An action with no before-state - a migration sweep, a retry - leaves
//! `before` null and says what happened in `after`.
//!
//! **A refusal is a row.** A failed sign-in, a rejected code, an action a
//! disabled account attempted: all recorded. An audit trail that holds only
//! successes answers "what was done" and not "what was tried", and the second
//! question is the one asked after something goes wrong.

use chrono::{DateTime, Utc};
use serde_json::Value as Json;
use sqlx::{FromRow, PgExecutor};
use uuid::Uuid;

use crate::error::DbError;

/// How an action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It happened.
    Ok,
    /// It was refused - a wrong password, a locked account, a missing licence.
    Refused,
    /// It was allowed and then broke. Distinct from `Refused` because these are
    /// the rows worth waking somebody for.
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "ok" => Some(Self::Ok),
            "refused" => Some(Self::Refused),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// The vocabulary of recordable acts.
///
/// A closed set rather than free text, so the screen can group them and a typo
/// cannot invent a category nobody ever looks at. Adding one is a deliberate
/// edit here and in the display name below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeskAction {
    SignIn,
    SignOut,
    MfaChallenge,
    DeskUserCreated,
    DeskUserSetupCompleted,
    DeskUserDisabled,
    DeskUserReinstated,
}

impl DeskAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SignIn => "desk.sign_in",
            Self::SignOut => "desk.sign_out",
            Self::MfaChallenge => "desk.mfa",
            Self::DeskUserCreated => "desk.user.created",
            Self::DeskUserSetupCompleted => "desk.user.setup_completed",
            Self::DeskUserDisabled => "desk.user.disabled",
            Self::DeskUserReinstated => "desk.user.reinstated",
        }
    }
}

/// A row to write.
///
/// Built with [`DeskAuditEntry::new`] and narrowed by the builders, so the
/// common case - an action, an outcome, an actor - is three words and the rest
/// is opt-in.
#[derive(Debug, Clone)]
pub struct DeskAuditEntry<'a> {
    pub action: DeskAction,
    pub outcome: Outcome,
    /// Null when the sign-in named nobody. The address is kept regardless: a
    /// failed attempt against an unknown address is exactly the row worth
    /// having.
    pub desk_user_id: Option<Uuid>,
    pub actor_email: Option<&'a str>,
    pub tenant_slug: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub before: Option<Json>,
    pub after: Option<Json>,
    pub ip: Option<&'a str>,
}

impl<'a> DeskAuditEntry<'a> {
    pub fn new(action: DeskAction, outcome: Outcome) -> Self {
        Self {
            action,
            outcome,
            desk_user_id: None,
            actor_email: None,
            tenant_slug: None,
            detail: None,
            before: None,
            after: None,
            ip: None,
        }
    }

    pub fn actor(mut self, id: Option<Uuid>, email: Option<&'a str>) -> Self {
        self.desk_user_id = id;
        self.actor_email = email;
        self
    }

    pub fn about(mut self, tenant_slug: &'a str) -> Self {
        self.tenant_slug = Some(tenant_slug);
        self
    }

    pub fn detail(mut self, detail: &'a str) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn from_to(mut self, before: Json, after: Json) -> Self {
        self.before = Some(before);
        self.after = Some(after);
        self
    }

    pub fn from_client(mut self, ip: Option<&'a str>) -> Self {
        self.ip = ip;
        self
    }
}

/// One row of `catalog.desk_audit`, as read back.
#[derive(Debug, Clone, FromRow)]
pub struct DeskAuditRecord {
    pub id: Uuid,
    pub desk_user_id: Option<Uuid>,
    pub actor_email: Option<String>,
    pub action: String,
    pub tenant_slug: Option<String>,
    pub outcome: String,
    pub detail: Option<String>,
    pub before_state: Option<Json>,
    pub after_state: Option<Json>,
    pub ip: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

const INSERT: &str = "INSERT INTO desk_audit \
     (desk_user_id, actor_email, action, tenant_slug, outcome, detail, before_state, \
      after_state, ip) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)";

const SELECT_RECENT: &str = "SELECT id, desk_user_id, actor_email, action, tenant_slug, outcome, \
     detail, before_state, after_state, ip, occurred_at \
     FROM desk_audit ORDER BY occurred_at DESC LIMIT $1 OFFSET $2";

/// Write one row.
///
/// Returns `Result` rather than swallowing the error, unlike the tenant audit
/// helper: there, a failed audit write must not fail the business action the
/// user asked for. Here the audit trail *is* half of what Desk is for, and a
/// suspension nobody can attribute is worse than a suspension that did not
/// happen. Callers that genuinely cannot fail - recording a failed sign-in, for
/// instance - log and continue at their own call site, visibly.
pub async fn record<'e, E>(executor: E, entry: DeskAuditEntry<'_>) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(INSERT)
        .bind(entry.desk_user_id)
        .bind(entry.actor_email)
        .bind(entry.action.as_str())
        .bind(entry.tenant_slug)
        .bind(entry.outcome.as_str())
        .bind(entry.detail)
        .bind(entry.before)
        .bind(entry.after)
        .bind(entry.ip)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    Ok(())
}

/// The newest rows first, which is the only order this table is ever read in.
pub async fn recent<'e, E>(
    executor: E,
    limit: i64,
    offset: i64,
) -> Result<Vec<DeskAuditRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskAuditRecord>(SELECT_RECENT)
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(executor)
        .await
        .map_err(DbError::Query)
}

/// How many rows there are, for the pager.
pub async fn count<'e, E>(executor: E) -> Result<i64, DbError>
where
    E: PgExecutor<'e>,
{
    use sqlx::Row;

    let row = sqlx::query("SELECT count(*) AS total FROM desk_audit")
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)?;

    row.try_get("total").map_err(DbError::Query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_survives_a_round_trip() {
        for outcome in [Outcome::Ok, Outcome::Refused, Outcome::Failed] {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
        }
        assert_eq!(Outcome::parse("partial"), None);
    }

    /// The check constraint in migration 0004 lists three outcomes. If a fourth
    /// is added here without touching the migration, every write of it fails at
    /// runtime - so the two lists are asserted equal in the only way a unit test
    /// can: by naming what the constraint allows.
    #[test]
    fn outcomes_match_the_check_constraint() {
        let allowed = ["ok", "refused", "failed"];
        for outcome in [Outcome::Ok, Outcome::Refused, Outcome::Failed] {
            assert!(
                allowed.contains(&outcome.as_str()),
                "{} is not allowed by desk_audit_outcome_valid",
                outcome.as_str()
            );
        }
    }

    /// Every action is namespaced, so a glance at the trail says which surface
    /// wrote the row - and so a future tenant-side action cannot collide.
    #[test]
    fn every_action_is_namespaced() {
        for action in [
            DeskAction::SignIn,
            DeskAction::SignOut,
            DeskAction::MfaChallenge,
            DeskAction::DeskUserCreated,
            DeskAction::DeskUserSetupCompleted,
            DeskAction::DeskUserDisabled,
            DeskAction::DeskUserReinstated,
        ] {
            assert!(
                action.as_str().starts_with("desk."),
                "{} is not namespaced",
                action.as_str()
            );
        }
    }

    #[test]
    fn the_builder_leaves_everything_it_was_not_given_empty() {
        let entry = DeskAuditEntry::new(DeskAction::SignIn, Outcome::Refused)
            .actor(None, Some("nobody@example.com"));

        assert_eq!(entry.actor_email, Some("nobody@example.com"));
        assert!(entry.desk_user_id.is_none());
        assert!(entry.tenant_slug.is_none());
        assert!(entry.before.is_none());
        assert!(entry.after.is_none());
    }
}
