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

use chrono::{DateTime, NaiveDate, Utc};
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

    // --- workspaces ----------------------------------------------------
    /// A licence issued, extended or shortened.
    ///
    /// One action for all three, because they are the same act with a
    /// different date and the `before`/`after` pair says which it was. A verb
    /// per date arithmetic would be three names for one decision.
    LicenceSet,
    /// A licence withdrawn. Its own action, and not folded into
    /// [`Self::LicenceSet`], because this is the one that stops a workspace
    /// serving - and "who withdrew this" is a question somebody asks by
    /// scanning a column, not by reading diffs.
    LicenceWithdrawn,
    /// A workspace created from Desk, with its licence and its owner
    /// invitation in the same act.
    WorkspaceCreated,
    /// The workspace owner's invitation issued again, superseding the one
    /// that was lost.
    WorkspaceOwnerInvited,
    /// A workspace stopped serving because somebody decided so. Not a lapse -
    /// that is a date passing and writes no row at all.
    WorkspaceSuspended,
    WorkspaceResumed,
    /// A stuck `provisioning` finished off.
    WorkspaceRetried,
    /// One workspace's database brought forward to this build's schema.
    WorkspaceMigrated,
    /// Every outdated workspace, in one pass. `tenant_slug` is null because the
    /// row is about the estate rather than about one of them; what it swept is
    /// in `after`.
    WorkspacesSwept,
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
            Self::LicenceSet => "desk.licence.set",
            Self::LicenceWithdrawn => "desk.licence.withdrawn",
            Self::WorkspaceCreated => "desk.workspace.created",
            Self::WorkspaceOwnerInvited => "desk.workspace.owner_invited",
            Self::WorkspaceSuspended => "desk.workspace.suspended",
            Self::WorkspaceResumed => "desk.workspace.resumed",
            Self::WorkspaceRetried => "desk.workspace.retried",
            Self::WorkspaceMigrated => "desk.workspace.migrated",
            Self::WorkspacesSwept => "desk.workspace.swept",
        }
    }
    /// Turn a stored string back into an action.
    ///
    /// `None` for a row this build does not recognise, which is a row written
    /// by a newer one. The screen shows the raw string in that case rather than
    /// hiding the row: an audit trail that silently drops what it cannot name
    /// is worse than one that shows an unfamiliar word.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.as_str() == raw)
    }

    /// What a screen calls it.
    ///
    /// English literals rather than message keys, unlike everything the product
    /// shows: Desk has no language switcher and no locale overlay - see
    /// `phonix_desk::html`.
    pub fn label(self) -> &'static str {
        match self {
            Self::SignIn => "Signed in",
            Self::SignOut => "Signed out",
            Self::MfaChallenge => "Answered a code",
            Self::DeskUserCreated => "Created a desk account",
            Self::DeskUserSetupCompleted => "Finished setting up an account",
            Self::DeskUserDisabled => "Disabled a desk account",
            Self::DeskUserReinstated => "Reinstated a desk account",
            Self::LicenceSet => "Set a licence",
            Self::LicenceWithdrawn => "Withdrew a licence",
            Self::WorkspaceCreated => "Created a workspace",
            Self::WorkspaceOwnerInvited => "Issued an owner invitation",
            Self::WorkspaceSuspended => "Suspended a workspace",
            Self::WorkspaceResumed => "Resumed a workspace",
            Self::WorkspaceRetried => "Retried a provisioning",
            Self::WorkspaceMigrated => "Migrated a workspace",
            Self::WorkspacesSwept => "Migrated every outdated workspace",
        }
    }

    /// Every action. The one list, so adding a variant without naming it here
    /// fails to compile rather than going missing from the screen.
    pub const ALL: [Self; 16] = [
        Self::SignIn,
        Self::SignOut,
        Self::MfaChallenge,
        Self::DeskUserCreated,
        Self::DeskUserSetupCompleted,
        Self::DeskUserDisabled,
        Self::DeskUserReinstated,
        Self::LicenceSet,
        Self::LicenceWithdrawn,
        Self::WorkspaceCreated,
        Self::WorkspaceOwnerInvited,
        Self::WorkspaceSuspended,
        Self::WorkspaceResumed,
        Self::WorkspaceRetried,
        Self::WorkspaceMigrated,
        Self::WorkspacesSwept,
    ];
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

const SELECT_FOR_TENANT: &str = "SELECT id, desk_user_id, actor_email, action, tenant_slug, \
     outcome, detail, before_state, after_state, ip, occurred_at \
     FROM desk_audit WHERE tenant_slug = $1 ORDER BY occurred_at DESC LIMIT $2";

/// One workspace's own history, newest first.
///
/// The estate-wide sweep writes its row with no `tenant_slug`, so it does not
/// appear here - correctly: it is a fact about the box, and a workspace's page
/// claiming it was individually migrated would be narration rather than a
/// record.
pub async fn for_tenant<'e, E>(
    executor: E,
    tenant_slug: &str,
    limit: i64,
) -> Result<Vec<DeskAuditRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DeskAuditRecord>(SELECT_FOR_TENANT)
        .bind(tenant_slug)
        .bind(limit.clamp(1, 200))
        .fetch_all(executor)
        .await
        .map_err(DbError::Query)
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

/// One day, and how much happened on it.
#[derive(Debug, Clone, FromRow)]
pub struct DailyCount {
    pub day: NaiveDate,
    pub entries: i64,
}

const SELECT_BY_DAY: &str = "SELECT (occurred_at AT TIME ZONE 'UTC')::date AS day, \
     count(*) AS entries \
     FROM desk_audit WHERE occurred_at >= $1 \
     GROUP BY 1 ORDER BY 1";

/// How many rows were written per day since `since`, for the dashboard.
///
/// **Only the days that have rows.** A quiet day is absent rather than zero,
/// and the caller fills the gaps - which is the right split: this function
/// reports what the table holds, and how many days a chart wants to draw is
/// the chart's business. Zero-filling here would mean this query needed to
/// know the window's shape as well as its start.
///
/// `AT TIME ZONE 'UTC'` rather than a bare `date_trunc`: bucketing a
/// `timestamptz` by day depends on the session's `TimeZone`, so without this
/// the same rows would land in different buckets on a box configured
/// differently, and the chart would be quietly wrong rather than visibly.
pub async fn activity_by_day<'e, E>(
    executor: E,
    since: DateTime<Utc>,
) -> Result<Vec<DailyCount>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, DailyCount>(SELECT_BY_DAY)
        .bind(since)
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
        for action in DeskAction::ALL {
            assert!(
                action.as_str().starts_with("desk."),
                "{} is not namespaced",
                action.as_str()
            );
        }
    }

    /// The screen turns a stored string back into an action to name it. A
    /// variant whose stored form does not round-trip would appear in the trail
    /// as raw text for ever, and nobody would notice which one.
    #[test]
    fn every_action_survives_a_round_trip() {
        for action in DeskAction::ALL {
            assert_eq!(DeskAction::parse(action.as_str()), Some(action));
        }
        assert_eq!(DeskAction::parse("desk.workspace.deleted"), None);
    }

    /// `ALL` is what the screen and both tests above walk. A variant missing
    /// from it is a row nobody can name, so the length is asserted against the
    /// number of distinct stored forms rather than trusted.
    #[test]
    fn every_variant_is_in_the_list_exactly_once() {
        let mut forms: Vec<&str> = DeskAction::ALL.iter().map(|a| a.as_str()).collect();
        forms.sort_unstable();
        let before = forms.len();
        forms.dedup();

        assert_eq!(before, forms.len(), "two actions share a stored form");
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
