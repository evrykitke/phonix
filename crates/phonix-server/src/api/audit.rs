//! `/api/v1/audit` - the two trails, which answer two different questions.
//!
//! ```text
//! /audit/changes   what was changed, to what, and by whom   entity_events
//! /audit/events    what happened to an account              identity_events
//! ```
//!
//! They are not one resource with a filter, and that is not an accident of how
//! they are stored. A **change** names a record and carries a diff: it exists
//! so that "what has ever happened to this role" is one index away. An
//! **event** names an account and something that happened to it - a sign-in, a
//! lockout, a factor enrolled - most of which changed no record at all.
//! Merging them would produce rows where half the fields are always null and a
//! `type` field to tell you which half.
//!
//! Both are gated on `Pages.Administration.AuditLogs`, and deliberately not on
//! the permission for the thing they are about: reading who signed in and when
//! somebody's permissions changed is a different kind of access from seeing a
//! list of names.
//!
//! # Nothing here writes
//!
//! There is no `POST`. A trail an API can append to is a trail whose entries
//! are only as trustworthy as the widest key in the workspace, and every entry
//! that exists is written by the use case that did the thing. Retention is a
//! policy on `PUT /settings/security`, not a delete endpoint.
//!
//! # The `detail` object is not on the wire
//!
//! The stored rows carry a free-form JSON `detail`, and shipping it verbatim
//! would be shipping an unversioned internal structure to clients that would
//! then depend on it. What crosses is [`FieldChangeResource`] and
//! [`FactResource`] - the same rendering the screens read, through the same
//! `audit::diff`, so a client and a person looking at the trail see the same
//! change described the same way.

use axum::Json;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use phonix_core::audit::change::{Change, Fact, FieldChange};
use phonix_core::audit::{EntityAction, EntityChange, EntityChangeDetail};
use phonix_core::identity::{AuditEvent, AuditEventDetail};
use phonix_services::ServiceError;
use phonix_services::audit;
use phonix_services::identity::directory;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::ApiCaller;
use super::paging::{ListParams, ListRequest, PageEnvelope};
use super::path::ApiPath;
use super::problem::Problem;

// ---------------------------------------------------------------------------
// The diff, shared by both trails
// ---------------------------------------------------------------------------

/// What happened to one field.
///
/// A tagged union rather than four nullable fields, because the two cases are
/// genuinely different questions. `value` is one thing replacing another;
/// `members` is a collection gaining and losing entries, which is how a
/// permission set that changed one name out of forty stays readable.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schema(as = Change)]
pub enum ChangeResource {
    /// One value replaced another. `null` on either side means **not set**,
    /// which is not the same as an empty string and must not be rendered as
    /// one.
    Value {
        before: Option<String>,
        after: Option<String>,
    },
    /// A collection gained and lost members.
    Members {
        added: Vec<String>,
        removed: Vec<String>,
    },
}

impl From<&Change> for ChangeResource {
    fn from(change: &Change) -> Self {
        match change {
            Change::Value { before, after } => Self::Value {
                before: before.clone(),
                after: after.clone(),
            },
            Change::Members { added, removed } => Self::Members {
                added: added.clone(),
                removed: removed.clone(),
            },
        }
    }
}

/// One line of a diff.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = FieldChange)]
pub struct FieldChangeResource {
    /// A dotted path into the record: `password.min_length`. Stable, and what
    /// a client groups or filters on.
    #[schema(example = "password.min_length")]
    pub field: String,
    pub change: ChangeResource,
}

impl From<&FieldChange> for FieldChangeResource {
    fn from(change: &FieldChange) -> Self {
        Self {
            field: change.field.clone(),
            change: ChangeResource::from(&change.change),
        }
    }
}

/// Something recorded beside an entry that is not part of the diff.
///
/// Who it was done to, which kind of factor, why it was refused. Present
/// exactly when the use case that wrote the entry had something to add that
/// was not a before and an after.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AuditFact)]
pub struct FactResource {
    #[schema(example = "email")]
    pub label: String,
    #[schema(example = "ada@example.com")]
    pub value: String,
}

impl From<&Fact> for FactResource {
    fn from(fact: &Fact) -> Self {
        Self {
            label: fact.label.clone(),
            value: fact.value.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// The change trail
// ---------------------------------------------------------------------------

/// What a change did to a record.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = EntityAction)]
pub enum EntityActionResource {
    Created,
    Updated,
    Deleted,
}

impl From<EntityAction> for EntityActionResource {
    fn from(action: EntityAction) -> Self {
        match action {
            EntityAction::Created => Self::Created,
            EntityAction::Updated => Self::Updated,
            EntityAction::Deleted => Self::Deleted,
        }
    }
}

/// One change to one record.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = EntityChange)]
pub struct ChangeSummaryResource {
    pub id: i64,
    /// The stored kind name - `role`, `user`, `organization`. The vocabulary
    /// `filter[kind]` matches, and the same strings the audit policy's
    /// `excluded` list carries.
    ///
    /// A kind a client does not recognise is a kind a later release added, and
    /// is safe to show as itself rather than to refuse.
    #[schema(example = "role")]
    pub entity_type: String,
    /// Which record. A singleton - the organization profile, the security
    /// policy - records its own kind name here, because there is no id.
    pub entity_id: String,
    pub action: EntityActionResource,
    /// What the record was called at the time. Stored rather than joined, so
    /// the row still names the thing after it has been deleted - which is the
    /// row most worth reading.
    pub label: Option<String>,
    pub actor_id: Option<Uuid>,
    /// Who did it, as recorded then. Kept even when that account is later
    /// removed, which is the point of a trail. `null` for something the system
    /// did with no person behind it.
    pub actor_email: Option<String>,
    pub ip: Option<String>,
    /// The diff in one line, already rendered. For a list; the fields are on
    /// the detail.
    pub summary: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

impl From<&EntityChange> for ChangeSummaryResource {
    fn from(change: &EntityChange) -> Self {
        Self {
            id: change.id,
            entity_type: change.entity_type.clone(),
            entity_id: change.entity_id.clone(),
            action: change.action.into(),
            label: change.label.clone(),
            actor_id: change.actor_id,
            actor_email: change.actor_email.clone(),
            ip: change.ip.clone(),
            summary: change.summary.clone(),
            occurred_at: change.occurred_at,
        }
    }
}

/// One change, opened.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = EntityChangeDetail)]
pub struct ChangeDetailResource {
    /// The same object the list returns, nested rather than merged - so a
    /// client holding one from the list reads this field into the same type,
    /// and a field added to either cannot collide with the other.
    pub change: ChangeSummaryResource,
    /// The browser the request came from, verbatim. Only here, never in the
    /// list: it is long, and it matters exactly once - when somebody is working
    /// out whether two changes were the same person.
    pub user_agent: Option<String>,
    /// Empty when the stored entry carried no before and after: a creation
    /// recorded without its values, or a row from a release that wrote less.
    pub changes: Vec<FieldChangeResource>,
    pub facts: Vec<FactResource>,
}

impl From<&EntityChangeDetail> for ChangeDetailResource {
    fn from(detail: &EntityChangeDetail) -> Self {
        Self {
            change: ChangeSummaryResource::from(&detail.change),
            user_agent: detail.user_agent.clone(),
            changes: detail
                .changes
                .iter()
                .map(FieldChangeResource::from)
                .collect(),
            facts: detail.facts.iter().map(FactResource::from).collect(),
        }
    }
}

/// What has been changed in this workspace, newest first.
///
/// Searches the label, the actor's address and the kind name. Sorts by
/// `occurred_at` (the default, **descending** - a trail is read from the top),
/// `entity_type`, `action`, `label` or `actor_email`. A field this build does
/// not have is ignored rather than refused.
///
/// Paged in SQL, because nothing ever deletes from this table by default:
/// there is no number of rows at which fetching all of it stops being wrong,
/// only a date at which it becomes obvious.
///
/// Requires `Pages.Administration.AuditLogs`.
#[utoipa::path(
    get,
    path = "/audit/changes",
    tag = "audit",
    operation_id = "listEntityChanges",
    params(
        ListParams,
        ("filter[kind]" = Option<String>, Query,
            description = "One stored entity type, matched whole: role, user, organization, \
                           security_policy, mail_settings, currencies, party, tax_code, …",
            example = "role"),
        ("filter[action]" = Option<String>, Query,
            description = "`created`, `updated` or `deleted`.",
            example = "deleted"),
        ("filter[occurred_from]" = Option<String>, Query,
            description = "RFC 3339. **Inclusive.** Omit for no lower bound.",
            example = "2026-08-01T00:00:00Z"),
        ("filter[occurred_to]" = Option<String>, Query,
            description = "RFC 3339. **Exclusive**, which is what makes a span of one day \
                           exactly one day. Omit for no upper bound.",
            example = "2026-09-01T00:00:00Z"),
    ),
    responses(
        (status = 200, description = "One page of the change trail", body = PageEnvelope<ChangeSummaryResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry AuditLogs", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list_changes(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<ChangeSummaryResource>>, Problem> {
    let page = audit::trail(&caller.pool, &caller.caller, &request).await?;

    Ok(Json(PageEnvelope::new(
        page.map(|change| ChangeSummaryResource::from(&change)),
    )))
}

/// One change, with its diff.
#[utoipa::path(
    get,
    path = "/audit/changes/{id}",
    tag = "audit",
    operation_id = "getEntityChange",
    params(("id" = i64, Path, description = "The entry's id")),
    responses(
        (status = 200, description = "The change and what it changed", body = ChangeDetailResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry AuditLogs", body = Problem),
        (status = 404, description = "No such entry", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_change(
    caller: ApiCaller,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<ChangeDetailResource>, Problem> {
    // `audit::change` already answers a missing row with `NotFound`, which
    // converts to a 404 on its own. Its sibling below does not, which is why
    // only one of these two handlers has to say anything.
    let detail = audit::change(&caller.pool, &caller.caller, id).await?;

    Ok(Json(ChangeDetailResource::from(&detail)))
}

// ---------------------------------------------------------------------------
// The security trail
// ---------------------------------------------------------------------------

/// One thing that happened to an account.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AuditEvent)]
pub struct EventSummaryResource {
    pub id: i64,
    /// The stored event name - `signed_in`, `mfa_enrolled`, `password_changed`.
    /// Stable, and what a client branches on. A name this client does not know
    /// is one a later release added.
    #[schema(example = "signed_in")]
    pub event: String,
    /// Whether it worked. A failed sign-in is an entry too, and is the entry
    /// most often being looked for.
    pub succeeded: bool,
    pub user_id: Option<Uuid>,
    /// Who it happened to, as recorded at the time - kept even when the account
    /// is later deleted. Also the address somebody *tried*, for an attempt
    /// against an account that does not exist.
    pub email: Option<String>,
    pub ip: Option<String>,
    /// Why it failed, or what changed - a short line, already rendered.
    pub summary: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

impl From<&AuditEvent> for EventSummaryResource {
    fn from(event: &AuditEvent) -> Self {
        Self {
            id: event.id,
            event: event.event.clone(),
            succeeded: event.succeeded,
            user_id: event.user_id,
            email: event.email.clone(),
            ip: event.ip.clone(),
            summary: event.summary.clone(),
            occurred_at: event.occurred_at,
        }
    }
}

/// One event, opened.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AuditEventDetail)]
pub struct EventDetailResource {
    pub event: EventSummaryResource,
    /// The browser the request came from, verbatim. Only here, for the reason
    /// the change detail gives.
    pub user_agent: Option<String>,
    /// Empty for the events that changed nothing, which is what tells a diff
    /// from a narration.
    pub changes: Vec<FieldChangeResource>,
    pub facts: Vec<FactResource>,
}

impl From<&AuditEventDetail> for EventDetailResource {
    fn from(detail: &AuditEventDetail) -> Self {
        Self {
            event: EventSummaryResource::from(&detail.event),
            user_agent: detail.user_agent.clone(),
            changes: detail
                .changes
                .iter()
                .map(FieldChangeResource::from)
                .collect(),
            facts: detail.facts.iter().map(FactResource::from).collect(),
        }
    }
}

/// What has happened to the accounts in this workspace, newest first.
///
/// Searches the address, the event name and the IP. Sorts by `occurred_at`
/// (the default, descending), `event`, `email`, `succeeded` or `ip`.
///
/// Requires `Pages.Administration.AuditLogs`.
#[utoipa::path(
    get,
    path = "/audit/events",
    tag = "audit",
    operation_id = "listAuditEvents",
    params(
        ListParams,
        ("filter[kind]" = Option<String>, Query,
            description = "`notable` for failures plus anything that changed what somebody \
                           may do, or `failures` for the refusals alone. Anything else is \
                           everything.",
            example = "notable"),
        ("filter[occurred_from]" = Option<String>, Query,
            description = "RFC 3339. Inclusive.",
            example = "2026-08-01T00:00:00Z"),
        ("filter[occurred_to]" = Option<String>, Query,
            description = "RFC 3339. Exclusive.",
            example = "2026-09-01T00:00:00Z"),
    ),
    responses(
        (status = 200, description = "One page of the security trail", body = PageEnvelope<EventSummaryResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry AuditLogs", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list_events(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<EventSummaryResource>>, Problem> {
    let page = directory::audit_trail(&caller.pool, &caller.caller, &request).await?;

    Ok(Json(PageEnvelope::new(
        page.map(|event| EventSummaryResource::from(&event)),
    )))
}

/// One event, with everything recorded about it.
#[utoipa::path(
    get,
    path = "/audit/events/{id}",
    tag = "audit",
    operation_id = "getAuditEvent",
    params(("id" = i64, Path, description = "The entry's id")),
    responses(
        (status = 200, description = "The event and its detail", body = EventDetailResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry AuditLogs", body = Problem),
        (status = 404, description = "No such entry", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_event(
    caller: ApiCaller,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<EventDetailResource>, Problem> {
    match directory::audit_event(&caller.pool, &caller.caller, id).await {
        Ok(detail) => Ok(Json(EventDetailResource::from(&detail))),
        // The trap ADR 0002 records under users, in the one shape where the
        // handler cannot work around it: `audit_event` answers a missing row
        // with a *rejection*, which renders as a 422 with a field in it. That
        // is right for a screen, where the id came from a row somebody
        // clicked; it is wrong for an address with nothing at it.
        //
        // `users::get` avoids this by doing its own lookup over the list.
        // There is no list here - the trail is paged in SQL and grows forever -
        // so the rejection is recognised instead, by the field the service
        // names. It is the only rejection this call can produce: every other
        // failure is a permission or the database.
        Err(err) if rejects_field(&err, "event") => Err(Problem::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "There is no audit event with that id on this workspace.",
        )),
        Err(err) => Err(Problem::from(err)),
    }
}

/// Whether a service refused one named field, rather than failing.
fn rejects_field(err: &ServiceError, field: &str) -> bool {
    match err {
        ServiceError::Rejected(errors) => errors.iter().any(|error| error.field == field),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::identity::FieldError;
    use phonix_core::msg;

    use super::*;

    #[test]
    fn a_value_change_keeps_not_set_distinct_from_empty() {
        // `null` and `""` mean different things and must not render alike: one
        // is a field nobody has filled in, the other is one somebody cleared.
        let added = FieldChangeResource::from(&FieldChange {
            field: "trading_name".to_owned(),
            change: Change::Value {
                before: None,
                after: Some(String::new()),
            },
        });

        let json = serde_json::to_string(&added).expect("it serialises");

        assert!(json.contains("\"before\":null"));
        assert!(json.contains("\"after\":\"\""));
        assert!(json.contains("\"type\":\"value\""));
    }

    #[test]
    fn a_membership_change_is_two_lists_rather_than_two_sets() {
        // The reason the union is tagged: a permission set that gained one
        // name out of forty is unreadable as two lists of forty and obvious as
        // one line of one.
        let change = FieldChangeResource::from(&FieldChange {
            field: "permissions".to_owned(),
            change: Change::Members {
                added: vec!["Pages.Administration.Users".to_owned()],
                removed: Vec::new(),
            },
        });

        let json = serde_json::to_string(&change).expect("it serialises");

        assert!(json.contains("\"type\":\"members\""));
        assert!(json.contains("Pages.Administration.Users"));
        assert!(json.contains("\"removed\":[]"));
    }

    #[test]
    fn a_missing_event_is_recognised_by_the_field_the_service_names() {
        // If the service ever renames that field this stops matching, and the
        // endpoint goes back to answering 422 - so the assertion is on the
        // real message the real call site produces.
        let gone = ServiceError::Rejected(vec![FieldError::new(
            "event",
            msg!("error.audit_event.gone"),
        )]);
        let other = ServiceError::Rejected(vec![FieldError::new("user", msg!("error.user.gone"))]);

        assert!(rejects_field(&gone, "event"));
        assert!(!rejects_field(&other, "event"));
        assert!(!rejects_field(
            &ServiceError::NotFound("audit entry"),
            "event"
        ));
    }

    #[test]
    fn every_action_maps_across() {
        for action in [
            EntityAction::Created,
            EntityAction::Updated,
            EntityAction::Deleted,
        ] {
            let resource = EntityActionResource::from(action);
            let json = serde_json::to_string(&resource).expect("it serialises");
            assert_eq!(json, format!("\"{}\"", action.as_str()));
        }
    }
}
