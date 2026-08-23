//! The change trail: recording what happened to a record, and reading it back.
//!
//! # One call, at the end of a save
//!
//! ```ignore
//! audit::updated(
//!     pool,
//!     caller,
//!     Target::new(kinds::ROLE, role.id).named(&role.display_name),
//!     &before,
//!     &after,
//! )
//! .await;
//! ```
//!
//! That is the whole contract for auditing an entity. There is no migration to
//! write, no event name to invent and no CHECK constraint to restate across
//! every tenant database - those were the costs that made
//! [`crate::identity::audit_view`]'s trail expensive to extend, and removing
//! them is the point of this module. Declaring the kind in
//! [`phonix_core::audit::kinds`] is the only other step.
//!
//! # Nothing is recorded when nothing moved
//!
//! [`updated`] compares the two values and writes nothing when they are equal.
//! Opening a screen, pressing save and changing nothing is a thing people do,
//! and a trail full of it is noise between the entries somebody is looking for.
//! That check lives here rather than at each call site, because the call site
//! that forgets it is the one nobody notices.
//!
//! # Recording never fails a save
//!
//! Every function here is best-effort and returns nothing. Losing a trail row
//! is bad; refusing an administrator's save because the trail is unwritable is
//! worse, and a `?` on an audit write is how a full disk locks a workspace out
//! of its own settings.
//!
//! # A workspace decides how much of this it wants
//!
//! Every write here first asks [`phonix_core::audit::AuditPolicy`] whether this
//! kind of record is being recorded at all. The trail is the one table nothing
//! deletes from, and in a workspace of ten thousand accounts it can outgrow the
//! records it describes - so which kinds are kept is the organization's to
//! decide, on the settings screen.
//!
//! The one change that is *never* gated is a change to that policy. "Who
//! stopped the recording, and when" is the single entry that has to survive the
//! recording being stopped, and it is marked with [`Target::always`]. Without
//! it, switching auditing off would be the one act with no trace - which is
//! precisely the act somebody covering their tracks would reach for first.
//!
//! # What is *not* recorded here
//!
//! Sign-ins, lockouts, second factors, sessions. Those did not change a record;
//! they happened to an account, and they stay on the security trail in
//! [`crate::identity`]. See `phonix_core::audit` for where the line is drawn.

pub mod diff;

/// The declared entity kinds, re-exported so a call site needs one import.
///
/// `use crate::audit::{self, Target, kinds};` is the whole preamble for
/// auditing something, and a second import from a second crate is the kind of
/// friction that gets a call site skipped.
pub use phonix_core::audit::kinds;

use phonix_core::audit::{EntityAction, EntityChange, EntityChangeDetail, EntityKind};
use phonix_core::permissions;
use phonix_core::query::{Page, PageRequest};
use phonix_db::audit as store;
use phonix_db::audit::EntityRecord;
use phonix_db::settings as settings_store;
use phonix_db::sqlx::PgPool;
use serde::Serialize;
use serde_json::Value as Json;

use crate::caller::Caller;
use crate::error::ServiceResult;

/// How many entries a record's own history section shows.
///
/// A section on a detail page, not a screen of its own: it is there to answer
/// "what happened to this recently", and somebody who needs the twentieth entry
/// is asking a question the full trail answers better.
pub const HISTORY_LIMIT: i64 = 20;

/// Which record a change is about, and anything else worth writing beside it.
///
/// Carried as one value rather than four arguments so that a call site cannot
/// pass a role's id under an account's kind - the two would compile, and the
/// result is a history that quietly belongs to the wrong record.
#[derive(Debug, Clone)]
pub struct Target {
    kind: EntityKind,
    id: String,
    label: Option<String>,
    /// Context that is not part of the diff: how many people a deleted role
    /// reached, which file replaced which. Rendered as labelled facts beside
    /// the change - see `diff::facts`.
    facts: serde_json::Map<String, Json>,
    /// Recorded whatever the workspace's audit policy says. See
    /// [`Target::always`].
    always: bool,
}

impl Target {
    pub fn new(kind: EntityKind, id: impl ToString) -> Self {
        Self {
            kind,
            id: id.to_string(),
            label: None,
            facts: serde_json::Map::new(),
            always: false,
        }
    }

    /// The one record of a kind there is only one of.
    ///
    /// The id comes from the kind itself, so no two call sites can spell it
    /// differently - which would be one record with two histories, and nothing
    /// would fail to say so.
    pub fn singleton(kind: EntityKind) -> Self {
        Self {
            kind,
            id: kind.singleton_id().to_owned(),
            label: None,
            facts: serde_json::Map::new(),
            always: false,
        }
    }

    /// Record this whatever the workspace's audit policy says.
    ///
    /// For exactly one thing: a change to the audit policy itself. Somebody who
    /// switches the trail off must leave a row saying they did, or the one act
    /// worth catching is the one act with no trace.
    ///
    /// Not for "this is important". Everything on this trail is important; the
    /// point of the policy is that an organization gets to disagree, and a call
    /// site that opts out of that decision is a call site overruling its own
    /// customer.
    #[must_use]
    pub const fn always(mut self) -> Self {
        self.always = true;
        self
    }

    /// What to call this record on the trail.
    ///
    /// Worth setting on anything a person named. It is stored rather than
    /// joined, so a deletion still says which role went - after the row is gone
    /// there is nothing left to look the name up in.
    #[must_use]
    pub fn named(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Something worth recording that is not a field that moved.
    ///
    /// How many people a deleted role reached, which relay a test message went
    /// through. It becomes a labelled line under the diff rather than part of
    /// it, because it is not a before and an after and drawing it as one would
    /// claim a field changed that does not exist.
    ///
    /// A value that will not serialise is dropped rather than recorded as
    /// null: an absent fact reads as "nothing to say", and a null one reads as
    /// a fact whose value is missing.
    #[must_use]
    pub fn fact(mut self, label: &str, value: impl Serialize) -> Self {
        if let Ok(value) = serde_json::to_value(value) {
            self.facts.insert(label.to_owned(), value);
        }

        self
    }
}

/// A record came into existence.
///
/// `after` is stored as the `to` half of a diff with an empty `from`, so the
/// detail page shows what it was created with rather than an empty table.
pub async fn created<T>(pool: &PgPool, caller: &Caller, target: Target, after: &T)
where
    T: Serialize,
{
    write(
        pool,
        caller,
        target,
        EntityAction::Created,
        Json::Null,
        json_of(after),
    )
    .await;
}

/// A record was edited.
///
/// Writes nothing when the two values are equal - see the module docs. The
/// bound is `PartialEq` rather than a comparison of the serialised forms
/// because the type knows what it means to be unchanged and JSON does not.
pub async fn updated<T>(pool: &PgPool, caller: &Caller, target: Target, before: &T, after: &T)
where
    T: Serialize + PartialEq,
{
    if before == after {
        return;
    }

    write(
        pool,
        caller,
        target,
        EntityAction::Updated,
        json_of(before),
        json_of(after),
    )
    .await;
}

/// A record is gone.
///
/// `before` is stored as the `from` half, so the trail keeps what was deleted
/// and not merely that something was.
pub async fn deleted<T>(pool: &PgPool, caller: &Caller, target: Target, before: &T)
where
    T: Serialize,
{
    write(
        pool,
        caller,
        target,
        EntityAction::Deleted,
        json_of(before),
        Json::Null,
    )
    .await;
}

/// A change whose before and after are already JSON.
///
/// For the cases where the two sides are not one serialisable value: the
/// permission editor holds two lists, and a logo change is two file names
/// rather than the two UUIDs the column stores. Everything else should use
/// [`created`], [`updated`] or [`deleted`], which cannot get the shape wrong.
pub async fn changed_json(
    pool: &PgPool,
    caller: &Caller,
    target: Target,
    before: Json,
    after: Json,
) {
    if before == after {
        return;
    }

    write(pool, caller, target, EntityAction::Updated, before, after).await;
}

/// The one place a row is actually built.
///
/// `from` and `to` are always both present, even when one is null: that is the
/// shape [`diff`] recognises, and a row carrying only one side is read as a
/// fact rather than as a diff - which would silently lose the detail page for
/// every creation.
async fn write(
    pool: &PgPool,
    caller: &Caller,
    target: Target,
    action: EntityAction,
    from: Json,
    to: Json,
) {
    let Target {
        kind,
        id,
        label,
        facts,
        always,
    } = target;

    if !always && !records(pool, kind).await {
        return;
    }

    let mut detail = serde_json::Map::new();
    detail.insert("from".to_owned(), from);
    detail.insert("to".to_owned(), to);
    // Facts last, and they cannot displace the diff: `from` and `to` are the
    // two keys `diff` reads, and a fact allowed to overwrite one would turn a
    // change into a sentence saying nothing changed.
    for (label, value) in facts {
        detail.entry(label).or_insert(value);
    }

    let detail = Json::Object(detail);

    let mut entry = store::EntityEntry::new(kind, id, action).detail(detail);

    if let Some(label) = label {
        entry = entry.label(label);
    }

    // The system caller has no account behind it - onboarding runs before an
    // owner exists, and a sweep is nobody. The row is still written: an
    // unattributed change is worth more on a trail than no change at all.
    if let Some(user) = caller.auth_user() {
        entry = entry.actor(user.id, Some(&user.email));
    }

    store::record_best_effort(pool, entry).await;
}

/// Whether this workspace is recording changes to this kind of record.
///
/// One indexed single-row lookup per audited write, not cached. An
/// administrator who switches the trail off means *now*, and a cache would make
/// that take effect at some unpredictable later moment - which on this
/// particular setting is the difference between a decision and a suggestion.
/// `phonix_db::settings` makes the same argument for the password policy.
///
/// A policy that cannot be read is treated as "record it". The alternative is
/// that a transient database problem silently turns auditing off, and a trail
/// with a hole in it that nobody was told about is worse than one extra row.
async fn records(pool: &PgPool, kind: EntityKind) -> bool {
    match settings_store::load(pool).await {
        Ok(settings) => settings.audit.records(kind),
        Err(err) => {
            tracing::warn!(
                error = %err,
                kind = kind.name,
                "could not read the audit policy; recording the change anyway",
            );
            true
        }
    }
}

/// Serialise, or record nothing rather than panic.
///
/// A type that will not serialise is a bug in that type, and it must not take
/// the save down with it. The row is written with a null on that side, which
/// reads on screen as "not recorded" rather than as a wrong diff.
fn json_of<T: Serialize>(value: &T) -> Json {
    match serde_json::to_value(value) {
        Ok(json) => json,
        Err(err) => {
            tracing::error!(error = %err, "an audited value could not be serialised");
            Json::Null
        }
    }
}

/// One page of the trail.
///
/// Gated on `AuditLogs`, like the security trail: the two screens answer the
/// same kind of question and are read by the same person.
pub async fn trail(
    pool: &PgPool,
    caller: &Caller,
    request: &PageRequest,
) -> ServiceResult<Page<EntityChange>> {
    caller.require(permissions::AUDIT_LOGS)?;

    Ok(store::page(pool, request).await?.map(listing))
}

/// One change, opened.
pub async fn change(pool: &PgPool, caller: &Caller, id: i64) -> ServiceResult<EntityChangeDetail> {
    caller.require(permissions::AUDIT_LOGS)?;

    let record = store::find(pool, id)
        .await?
        .ok_or_else(|| crate::error::ServiceError::NotFound("audit entry"))?;

    Ok(described(record))
}

/// Everything that has ever happened to one record.
///
/// The history section on a detail page. Gated on `AuditLogs` like the rest of
/// the trail: a history is the trail, filtered, and filtering something is not
/// a reason to need less permission to read it.
pub async fn history(
    pool: &PgPool,
    caller: &Caller,
    kind: EntityKind,
    id: &str,
) -> ServiceResult<Vec<EntityChange>> {
    caller.require(permissions::AUDIT_LOGS)?;

    let rows = store::for_entity(pool, kind.name, id, HISTORY_LIMIT).await?;

    Ok(rows.into_iter().map(listing).collect())
}

/// The history of the workspace's one organization profile.
///
/// A named function rather than a `Target` at the call site, because a
/// singleton's id is the one thing a caller has no reason to know.
pub async fn organization_history(
    pool: &PgPool,
    caller: &Caller,
) -> ServiceResult<Vec<EntityChange>> {
    history(
        pool,
        caller,
        kinds::ORGANIZATION,
        kinds::ORGANIZATION.singleton_id(),
    )
    .await
}

/// One stored row as a line of the trail.
fn listing(record: EntityRecord) -> EntityChange {
    EntityChange {
        id: record.id,
        entity_type: record.entity_type,
        entity_id: record.entity_id,
        action: record.action,
        label: record.label,
        actor_id: record.actor_id,
        actor_email: record.actor_email,
        ip: record.ip,
        summary: diff::summarise(&record.detail),
        occurred_at: record.occurred_at,
    }
}

/// One stored row, opened.
fn described(record: EntityRecord) -> EntityChangeDetail {
    let changes = diff::changes(&record.detail);
    let facts = diff::facts(&record.detail);
    let user_agent = record.user_agent.clone();

    EntityChangeDetail {
        change: listing(record),
        user_agent,
        changes,
        facts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_gated_by_the_workspace_policy_unless_it_says_otherwise() {
        // The default matters: a call site that forgets the question is one
        // that respects the customer's decision, not one that overrules it.
        assert!(!Target::new(kinds::ROLE, "8f2c").always);
        assert!(!Target::singleton(kinds::ORGANIZATION).always);
        assert!(Target::singleton(kinds::SECURITY_POLICY).always().always);
    }

    #[test]
    fn a_singleton_target_takes_its_id_from_its_kind() {
        // Nobody has to know what a singleton's id is, so nobody can get it
        // wrong - two spellings would be one record with two histories.
        let target = Target::singleton(kinds::ORGANIZATION);

        assert_eq!(target.id, kinds::ORGANIZATION.singleton_id());
        assert_eq!(target.id, "organization");
    }

    #[test]
    fn a_target_carries_the_name_the_record_had() {
        let target = Target::new(kinds::ROLE, "8f2c").named("Auditor");

        assert_eq!(target.id, "8f2c");
        assert_eq!(target.label.as_deref(), Some("Auditor"));
    }

    #[test]
    fn a_value_that_will_not_serialise_records_nothing_rather_than_panicking() {
        // `f64::NAN` has no JSON representation. The point is that the save it
        // was recorded beside is not taken down with it.
        assert_eq!(json_of(&f64::NAN), Json::Null);
    }

    #[test]
    fn a_creation_and_a_deletion_are_still_diffable_shapes() {
        // Both sides are always written, even when one is null: a row carrying
        // only `to` is read as a fact rather than as a diff, which would lose
        // the detail page for every creation.
        let created = serde_json::json!({ "from": Json::Null, "to": { "name": "Auditor" } });
        let deleted = serde_json::json!({ "from": { "name": "Auditor" }, "to": Json::Null });

        assert_eq!(
            diff::changes(&created)
                .first()
                .map(|change| change.field.clone()),
            Some("name".to_owned()),
        );
        assert_eq!(
            diff::changes(&deleted)
                .first()
                .map(|change| change.field.clone()),
            Some("name".to_owned()),
        );
    }
}
