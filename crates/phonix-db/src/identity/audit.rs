//! The security audit trail (`identity_events`).
//!
//! This is where the truth about a failed sign-in goes. The login form itself
//! must stay vague - saying "no such account" turns it into an enumeration
//! oracle - so the specific reason is recorded here instead, where only people
//! with `Pages.Administration.AuditLogs` can read it.
//!
//! Recording is best-effort: a failure to write an audit row is logged and
//! swallowed rather than failing the request it describes. The alternative is
//! that a full disk locks everyone out of the application.

use chrono::{DateTime, Utc};
use phonix_core::identity::{NOTABLE_EVENTS, UserId};
use phonix_core::query::{Page, PageRequest};
use serde_json::Value as Json;
use sqlx::{AssertSqlSafe, FromRow, PgExecutor, PgPool, Row};

use crate::error::DbError;

/// What happened. Matches the `identity_events_event_valid` constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityEvent {
    Signup,
    Login,
    Logout,
    PasswordChange,
    PasswordResetRequested,
    PasswordResetCompleted,
    EmailVerificationSent,
    EmailVerified,
    MfaEnrolled,
    MfaChallenge,
    MfaRemoved,
    /// A recovery code was spent - the user has lost their authenticator.
    MfaRecoveryUsed,
    MfaRecoveryGenerated,
    AccountLocked,
    AccountUnlocked,
    RoleChanged,
    SessionRevoked,
    InvitationSent,
    InvitationAccepted,
    /// An administrator changed what this workspace requires. Recorded because
    /// "who relaxed the password rules, and when" is exactly what an audit asks
    /// after the fact.
    PasswordPolicyChanged,
    MfaPolicyChanged,
    /// An individual account's permission overrides were edited.
    UserPermissionsChanged,
    /// A role's grants were edited, which reaches everybody holding it.
    RolePermissionsChanged,
    /// Where this workspace's mail goes was changed.
    ///
    /// The one setting that can redirect every invitation and every reset link
    /// to a relay somebody else controls, which is why it is recorded.
    MailSettingsChanged,
    /// An administrator edited an account: its name, its status, its roles.
    ///
    /// One event for the whole save rather than one per field, because it was
    /// one decision. Recorded as `{from, to}`, which is what earns it a diff
    /// on the detail page instead of a sentence.
    UserUpdated,
    /// A role was defined. It grants nothing until somebody edits the tree.
    RoleCreated,
    /// A role's name, label, description or default flag changed.
    ///
    /// Distinct from [`Self::RolePermissionsChanged`], which is what the role
    /// *grants*, and from [`Self::RoleChanged`], which is one account being
    /// given or losing one. Three different questions, three different events,
    /// because an audit is read by asking one of them at a time.
    RoleUpdated,
    /// A role was removed, and everybody holding it lost what it granted.
    RoleDeleted,
    /// The organization's own details changed - its legal name, its
    /// registration number, its address, the currency it counts in.
    ///
    /// These are what appear on anything the workspace issues, so "who changed
    /// the entity name, and to what" is the question asked after a document
    /// goes out wrong. Recorded as `{from, to}`, which earns it a diff on the
    /// detail page rather than a sentence saying something changed.
    OrganizationProfileChanged,
    /// The logo that goes on this workspace's documents was replaced or
    /// removed.
    ///
    /// Its own event rather than part of the profile diff: the profile is a
    /// form somebody fills in, and this is an upload that happens beside it.
    /// Recorded by file name, because a diff of two UUIDs tells a reader
    /// nothing they can act on.
    OrganizationLogoChanged,
}

impl IdentityEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signup => "signup",
            Self::Login => "login",
            Self::Logout => "logout",
            Self::PasswordChange => "password_change",
            Self::PasswordResetRequested => "password_reset_requested",
            Self::PasswordResetCompleted => "password_reset_completed",
            Self::EmailVerificationSent => "email_verification_sent",
            Self::EmailVerified => "email_verified",
            Self::MfaEnrolled => "mfa_enrolled",
            Self::MfaChallenge => "mfa_challenge",
            Self::MfaRemoved => "mfa_removed",
            Self::MfaRecoveryUsed => "mfa_recovery_used",
            Self::MfaRecoveryGenerated => "mfa_recovery_generated",
            Self::AccountLocked => "account_locked",
            Self::AccountUnlocked => "account_unlocked",
            Self::RoleChanged => "role_changed",
            Self::SessionRevoked => "session_revoked",
            Self::InvitationSent => "invitation_sent",
            Self::InvitationAccepted => "invitation_accepted",
            Self::PasswordPolicyChanged => "password_policy_changed",
            Self::MfaPolicyChanged => "mfa_policy_changed",
            Self::UserPermissionsChanged => "user_permissions_changed",
            Self::RolePermissionsChanged => "role_permissions_changed",
            Self::MailSettingsChanged => "mail_settings_changed",
            Self::UserUpdated => "user_updated",
            Self::RoleCreated => "role_created",
            Self::RoleUpdated => "role_updated",
            Self::RoleDeleted => "role_deleted",
            Self::OrganizationProfileChanged => "organization_profile_changed",
            Self::OrganizationLogoChanged => "organization_logo_changed",
        }
    }
}

/// One entry to record.
#[derive(Debug, Clone)]
pub struct AuditEntry<'a> {
    pub event: IdentityEvent,
    pub succeeded: bool,
    /// `None` when no account matched - which is itself worth recording.
    pub user_id: Option<UserId>,
    /// Kept alongside `user_id` so the trail survives the account, and so a
    /// failed sign-in for an address that does not exist still names it.
    pub email: Option<&'a str>,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    /// Free-form context: the failure reason, the factor kind, the old role.
    pub detail: Json,
}

impl<'a> AuditEntry<'a> {
    pub fn new(event: IdentityEvent, succeeded: bool) -> Self {
        Self {
            event,
            succeeded,
            user_id: None,
            email: None,
            ip: None,
            user_agent: None,
            detail: Json::Object(Default::default()),
        }
    }

    pub fn user(mut self, id: UserId) -> Self {
        self.user_id = Some(id);
        self
    }

    pub fn email(mut self, email: &'a str) -> Self {
        self.email = Some(email);
        self
    }

    pub fn client(mut self, ip: Option<&'a str>, user_agent: Option<&'a str>) -> Self {
        self.ip = ip;
        self.user_agent = user_agent;
        self
    }

    /// Attach the reason. Only ever read by someone with audit-log access, so
    /// this is the right place for the detail the user must not be told.
    pub fn reason(mut self, reason: &str) -> Self {
        self.detail = serde_json::json!({ "reason": reason });
        self
    }

    pub fn detail(mut self, detail: Json) -> Self {
        self.detail = detail;
        self
    }
}

/// One row of `identity_events`.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub id: i64,
    pub user_id: Option<UserId>,
    pub email: Option<String>,
    pub event: String,
    pub succeeded: bool,
    pub detail: Json,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for AuditRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            email: row.try_get("email")?,
            event: row.try_get("event")?,
            succeeded: row.try_get("succeeded")?,
            detail: row.try_get("detail")?,
            ip: row.try_get("ip")?,
            user_agent: row.try_get("user_agent")?,
            occurred_at: row.try_get("occurred_at")?,
        })
    }
}

/// Append an entry.
pub async fn record<'e, E>(executor: E, entry: AuditEntry<'_>) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO identity_events
             (user_id, email, event, succeeded, detail, ip, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(entry.user_id)
    .bind(entry.email)
    .bind(entry.event.as_str())
    .bind(entry.succeeded)
    .bind(&entry.detail)
    .bind(entry.ip)
    .bind(entry.user_agent)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Append an entry, logging rather than propagating a failure.
///
/// Use this on the sign-in and signup paths. Losing an audit row is bad;
/// refusing to sign anybody in because the audit table is unwritable is worse.
pub async fn record_best_effort<'e, E>(executor: E, entry: AuditEntry<'_>)
where
    E: PgExecutor<'e>,
{
    let event = entry.event.as_str();
    let succeeded = entry.succeeded;

    if let Err(err) = record(executor, entry).await {
        tracing::error!(
            error = %err,
            event,
            succeeded,
            "could not write the identity audit entry"
        );
    }
}

/// The most recent entries, newest first. Backs the audit-log screen.
pub async fn recent<'e, E>(executor: E, limit: i64) -> Result<Vec<AuditRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, AuditRecord>(
        "SELECT id, user_id, email, event, succeeded, detail, ip, user_agent, occurred_at
           FROM identity_events
          ORDER BY occurred_at DESC
          LIMIT $1",
    )
    .bind(limit.clamp(1, 500))
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)
}

/// Entries for one account, newest first.
pub async fn for_user<'e, E>(
    executor: E,
    user_id: UserId,
    limit: i64,
) -> Result<Vec<AuditRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, AuditRecord>(
        "SELECT id, user_id, email, event, succeeded, detail, ip, user_agent, occurred_at
           FROM identity_events
          WHERE user_id = $1
          ORDER BY occurred_at DESC
          LIMIT $2",
    )
    .bind(user_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)
}

/// The range key the audit grid declares, and so the pair of filter keys -
/// `occurred_from` and `occurred_to` - that arrive with a request.
///
/// A constant because it is written in two places that must agree and are in
/// two different crates: here, and `ui::table::config::audit`.
pub const OCCURRED: &str = "occurred";

/// The columns of `identity_events` a grid may order by.
///
/// A whitelist, not a convenience: `sort.field` arrives from a browser and the
/// only safe way to put it in an `ORDER BY` is to not put it there at all - to
/// match it against a list of literals this file wrote itself. A field that is
/// not on the list is ignored rather than refused, because it usually means a
/// stale sort from a column that has since been renamed.
const SORTABLE: &[(&str, &str)] = &[
    ("occurred_at", "occurred_at"),
    ("event", "event"),
    ("email", "email"),
    ("succeeded", "succeeded"),
    ("ip", "ip"),
];

/// Which entries a reader wants, beyond the search text.
///
/// The names match the values [`PageRequest::filter`] carries for the `kind`
/// key, so the screen and the query cannot drift apart in spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuditScope {
    #[default]
    Everything,
    /// Failures, plus anything that changed what somebody may do. The list
    /// lives in `phonix_core` so that this clause and `AuditEvent::is_notable`
    /// cannot answer differently.
    Notable,
    /// Only the entries that did not succeed.
    Failures,
}

impl AuditScope {
    /// The scope named by a request, defaulting to everything.
    pub fn of(request: &PageRequest) -> Self {
        match request.filter("kind") {
            Some("notable") => Self::Notable,
            Some("failures") => Self::Failures,
            _ => Self::Everything,
        }
    }
}

/// One page of the trail, matching a search and a scope.
///
/// Paged in SQL rather than in the browser, unlike the user list: the trail
/// grows for as long as the workspace is used, and a screen that fetched all of
/// it would get slower every day it was not looked at.
///
/// Two statements - a count and a select - so that the page can be pulled back
/// to one that exists before the rows are fetched. Asking for page nine of a
/// two-page result and getting an empty table with a pager that says "page 9 of
/// 2" is the failure that avoids.
pub async fn page(pool: &PgPool, request: &PageRequest) -> Result<Page<AuditRecord>, DbError> {
    let request = request.sanitised();
    let scope = AuditScope::of(&request);
    let needle = request
        .needle()
        .map(|needle| format!("%{}%", escape_like(&needle)));

    // One clause, six bound parameters, no interpolation: a scope that is not
    // in force compiles to `NOT false OR ...`, which Postgres discards, and an
    // end of the range that nobody set is a NULL that discards its own line.
    //
    // The range is bound as two instants rather than as a name. The browser
    // resolved "this week" before it sent anything - see
    // `phonix_core::query::range` - so this file owns no calendar and cannot
    // disagree with the panel about when a week starts.
    const WHERE: &str = "WHERE ($1::text IS NULL
                             OR email ILIKE $1
                             OR event ILIKE $1
                             OR ip ILIKE $1)
                           AND (NOT $2::bool OR (succeeded = FALSE OR event = ANY($3)))
                           AND (NOT $4::bool OR succeeded = FALSE)
                           AND ($5::timestamptz IS NULL OR occurred_at >= $5)
                           AND ($6::timestamptz IS NULL OR occurred_at < $6)";

    let only_notable = matches!(scope, AuditScope::Notable);
    let only_failures = matches!(scope, AuditScope::Failures);
    // Half open: `from` is included, `to` is not, which is what makes a span of
    // one day exactly one day. See `DateRange`.
    let occurred = request.range(OCCURRED);

    // `AssertSqlSafe` because these two statements are composed rather than
    // written: `WHERE` is a constant, and `order` can only be a string this
    // file put in `SORTABLE`. Nothing that arrives from a browser reaches the
    // text of the query - the search term, the scope and the page are all
    // bound parameters.
    let counting = AssertSqlSafe(format!("SELECT count(*) FROM identity_events {WHERE}"));

    let total: i64 = sqlx::query_scalar(counting)
        .bind(needle.as_deref())
        .bind(only_notable)
        .bind(NOTABLE_EVENTS)
        .bind(only_failures)
        .bind(occurred.from)
        .bind(occurred.to)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

    let total = u64::try_from(total).unwrap_or(0);
    let request = request.clamped_to(total);

    let order = match &request.sort {
        Some(sort) => SORTABLE
            .iter()
            .find(|(field, _)| *field == sort.field)
            .map(|(_, column)| format!("{column} {}", sort.direction.sql())),
        None => None,
    }
    // Newest first, and `id` after it whatever the sort: two entries written in
    // the same millisecond would otherwise swap places between one page and the
    // next, which shows up as a row that appears twice.
    .unwrap_or_else(|| "occurred_at DESC".to_owned());

    let selecting = AssertSqlSafe(format!(
        "SELECT id, user_id, email, event, succeeded, detail, ip, user_agent, occurred_at
           FROM identity_events
           {WHERE}
          ORDER BY {order}, id DESC
          LIMIT $7 OFFSET $8"
    ));

    let rows = sqlx::query_as::<_, AuditRecord>(selecting)
        .bind(needle.as_deref())
        .bind(only_notable)
        .bind(NOTABLE_EVENTS)
        .bind(only_failures)
        .bind(occurred.from)
        .bind(occurred.to)
        .bind(request.limit() as i64)
        .bind(request.offset() as i64)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;

    Ok(Page::new(rows, total, &request))
}

/// One entry, for the screen that opens from the list.
pub async fn find<'e, E>(executor: E, id: i64) -> Result<Option<AuditRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, AuditRecord>(
        "SELECT id, user_id, email, event, succeeded, detail, ip, user_agent, occurred_at
           FROM identity_events
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

/// Neutralise the wildcards in a search term.
///
/// Without this, typing `%` into the search box matches every row and typing
/// `_` matches every single character - which reads as a search box that
/// sometimes ignores what was typed.
fn escape_like(needle: &str) -> String {
    needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_term_cannot_smuggle_in_a_wildcard() {
        // `%` and `_` are LIKE syntax; somebody searching for "50%" means the
        // characters, not "everything".
        assert_eq!(escape_like("50%"), "50\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
    }

    #[test]
    fn the_scope_comes_from_the_request_and_defaults_to_everything() {
        let request = PageRequest::first(10);

        assert_eq!(AuditScope::of(&request), AuditScope::Everything);
        assert_eq!(
            AuditScope::of(&request.clone().filtered_by("kind", "notable")),
            AuditScope::Notable
        );
        assert_eq!(
            AuditScope::of(&request.clone().filtered_by("kind", "failures")),
            AuditScope::Failures
        );
        // A value this build does not know is not an error; it is everything.
        assert_eq!(
            AuditScope::of(&request.filtered_by("kind", "invented-later")),
            AuditScope::Everything
        );
    }

    #[test]
    fn every_sortable_field_names_a_column_of_the_table() {
        // The pair exists so that the browser never names a column directly.
        // If these ever differ, the browser is choosing SQL identifiers.
        for (field, column) in SORTABLE {
            assert!(!field.is_empty() && !column.is_empty());
        }
    }

    #[test]
    fn event_names_match_the_check_constraint() {
        // Written into a column with a CHECK on exactly this list.
        for (event, expected) in [
            (IdentityEvent::Signup, "signup"),
            (IdentityEvent::Login, "login"),
            (IdentityEvent::AccountLocked, "account_locked"),
            (
                IdentityEvent::PasswordResetRequested,
                "password_reset_requested",
            ),
            (IdentityEvent::InvitationAccepted, "invitation_accepted"),
        ] {
            assert_eq!(event.as_str(), expected);
        }
    }

    #[test]
    fn an_entry_can_name_an_address_with_no_account_behind_it() {
        // The most valuable failed-login row is the one with no user_id.
        let entry = AuditEntry::new(IdentityEvent::Login, false)
            .email("nobody@example.com")
            .client(Some("203.0.113.7"), Some("curl/8"))
            .reason("no such account");

        assert!(entry.user_id.is_none());
        assert_eq!(entry.email, Some("nobody@example.com"));
        assert_eq!(entry.detail["reason"], "no such account");
    }
}
