//! What Desk may do to a workspace.
//!
//! Every use case here writes a `desk_audit` row, in the catalog, where the
//! workspace it was done to cannot read, edit or lose it. That is not a
//! nicety: "who suspended this" must not be a fact its own administrators
//! hold. See `docs/adr/0005-phonix-desk.md` section 8.
//!
//! # None of this reads business data
//!
//! Desk answers questions about a workspace as an *object* - is it running, is
//! it current, is it authorized, is it wedged. Every question about what is
//! *inside* one is answered by signing in to it as somebody with the right to
//! see it. That line is what keeps this surface auditable: a tool that can
//! read tenant data is a tool that has to justify every read, and no small team
//! sustains that. ADR 0005 section 6.

use chrono::{DateTime, Utc};
use phonix_config::DatabaseConfig;
use phonix_core::identity::validation::FieldError;
use phonix_core::msg;
use phonix_core::{Licence, LicenceState, TenantSlug, TenantStatus};
use phonix_db::desk::audit::{DeskAction, DeskAuditEntry, Outcome};
use phonix_db::desk::session::ClientFacts;
use phonix_db::tenancy::catalog::{Catalog, TenantRecord};
use phonix_db::tenancy::licence::{self, LicenceInput};
use phonix_db::tenancy::{MigrationSweep, apps, provision};
use serde_json::json;

use crate::desk::auth::DeskCaller;
use crate::error::{ServiceError, ServiceResult};

/// The longest note the column accepts. Checked here so the refusal reaches a
/// form field rather than arriving as a constraint violation.
const NOTE_LIMIT: usize = 500;

/// One workspace, or nothing.
///
/// A thin pass-through to the catalog, named here so a screen does not reach
/// past this module into the repository - and so the day this grows a
/// dependency-health read, there is one place it goes.
pub async fn find(catalog: &Catalog, slug: &TenantSlug) -> ServiceResult<Option<TenantRecord>> {
    Ok(catalog.find_by_slug(slug).await?)
}

/// Every workspace, slug order.
pub async fn list(catalog: &Catalog) -> ServiceResult<Vec<TenantRecord>> {
    Ok(catalog.list().await?)
}

/// What a desk user decided about a workspace's authorization.
///
/// Deliberately not a `Licence`: `valid_from` and `updated_by` are not the
/// person's to choose - the first stays where it was so extending a licence
/// does not rewrite when it began, and the second is whoever is signed in.
#[derive(Debug, Clone)]
pub struct LicenceDecision {
    pub state: LicenceState,
    /// `None` means no end date, which is a deliberate act and not an
    /// omission - the audit row is what makes it one.
    pub valid_until: Option<DateTime<Utc>>,
    pub note: Option<String>,
}

/// Issue, extend, shorten or withdraw a workspace's licence.
///
/// All four are one statement, because they are one act with different
/// arguments. What tells them apart afterwards is the `before`/`after` pair on
/// the audit row - the shape the tenant entity trail already uses, and the one
/// that earns a diff on a detail page rather than a sentence that narrates.
pub async fn set_licence(
    catalog: &Catalog,
    slug: &TenantSlug,
    decision: LicenceDecision,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
) -> ServiceResult<Licence> {
    let tenant = catalog
        .find_by_slug(slug)
        .await?
        .ok_or_else(|| phonix_db::DbError::UnknownTenant(slug.to_string()))?;

    let note = decision
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.chars().count() > NOTE_LIMIT) {
        return Err(ServiceError::Rejected(vec![FieldError::new(
            "note",
            msg!("desk.licence.note_too_long"),
        )]));
    }

    // Extending does not move the start. A licence that began in March and was
    // extended in June still began in March, and rewriting that would quietly
    // erase how long a workspace has been authorized.
    let valid_from = tenant
        .licence
        .as_ref()
        .map(|existing| existing.valid_from)
        .unwrap_or_else(Utc::now);

    if decision
        .valid_until
        .is_some_and(|until| until <= valid_from)
    {
        return Err(ServiceError::Rejected(vec![FieldError::new(
            "valid_until",
            msg!("desk.licence.end_before_start"),
        )]));
    }

    let before = snapshot(tenant.licence.as_ref());

    let written = licence::set(
        catalog.pool(),
        tenant.id,
        LicenceInput {
            state: decision.state,
            valid_from,
            valid_until: decision.valid_until,
            note,
            updated_by: actor.email(),
        },
    )
    .await?;

    let action = match decision.state {
        LicenceState::Revoked => DeskAction::LicenceWithdrawn,
        _ => DeskAction::LicenceSet,
    };

    // Not best-effort. A licence change nobody can attribute is worse than a
    // licence change that failed and has to be repeated - this is the row that
    // answers "who authorized this workspace", and it is half of what Desk is
    // for.
    phonix_db::desk::audit::record(
        catalog.pool(),
        DeskAuditEntry::new(action, Outcome::Ok)
            .actor(Some(actor.id()), Some(actor.email()))
            .about(slug.as_str())
            .from_to(before, snapshot(Some(&written)))
            .from_client(facts.ip),
    )
    .await?;

    tracing::info!(
        tenant = %slug,
        state = written.state.as_str(),
        by = actor.email(),
        "licence set"
    );

    Ok(written)
}

// ---------------------------------------------------------------------------
// The three safe writes
//
// Safe in a specific sense: each is either idempotent or reversible, and none
// of them can lose anything. Retrying a stuck provisioning re-runs steps that
// are all skip-if-present. Migrating is forward-only but is the same pass the
// server already runs on boot. Suspending is one column, and resuming puts it
// back. Creating a workspace and archiving one are not here, and deleting a
// database is not exposed at all - ADR 0005 section 6.
//
// Every one of them writes a `desk_audit` row before returning, including when
// it fails: an audit trail that holds only successes answers "what was done"
// and not "what was tried", and the second question is the one asked after
// something has gone wrong.
// ---------------------------------------------------------------------------

/// Finish a workspace whose provisioning stopped part-way through.
///
/// Refused for a workspace that is not actually stuck. There is nothing to
/// repair on a workspace that is running, and a "retry" that quietly re-ran
/// migrations on a live tenant would be a different act wearing this one's
/// name.
pub async fn retry_provisioning(
    catalog: &Catalog,
    database: &DatabaseConfig,
    slug: &TenantSlug,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
) -> ServiceResult<TenantRecord> {
    let tenant = require(catalog, slug).await?;

    if tenant.status != TenantStatus::Provisioning {
        return Err(ServiceError::Rejected(vec![FieldError::new(
            "slug",
            msg!("desk.workspace.not_stuck"),
        )]));
    }

    let before = json!({ "status": tenant.status.as_str() });

    match provision::repair_tenant(catalog, database, &tenant).await {
        Ok(repaired) => {
            record(
                catalog,
                DeskAction::WorkspaceRetried,
                Outcome::Ok,
                slug,
                actor,
                facts,
                Some(before),
                json!({
                    "status": repaired.status.as_str(),
                    "schema_version": repaired.schema_version,
                }),
            )
            .await?;

            Ok(repaired)
        }
        Err(err) => {
            // `Failed`, not `Refused`. This was allowed and then broke, which
            // is the distinction that decides whether a row is worth waking
            // somebody for.
            let detail = err.to_string();
            record_failure(
                catalog,
                DeskAction::WorkspaceRetried,
                Some(slug),
                actor,
                facts,
                &detail,
            )
            .await;
            Err(err.into())
        }
    }
}

/// Bring one workspace's database up to this build's schema.
///
/// Returns the fingerprint it is now on. Runs in the request, which is honest
/// rather than lazy: the person who pressed the button is the person who should
/// see whether it worked, and a background job would report into a log nobody
/// is watching at that moment.
pub async fn migrate_one(
    catalog: &Catalog,
    database: &DatabaseConfig,
    slug: &TenantSlug,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
) -> ServiceResult<String> {
    let tenant = require(catalog, slug).await?;
    let latest = apps::schema_fingerprint();

    let before = json!({ "schema_version": tenant.schema_version });

    match provision::migrate_tenant(database, &tenant.database_name).await {
        Ok(()) => {
            // `mark_migrated`, so migrating a suspended workspace does not
            // resume it. The two are separate decisions and stay separate.
            catalog.mark_migrated(slug, &latest).await?;

            record(
                catalog,
                DeskAction::WorkspaceMigrated,
                Outcome::Ok,
                slug,
                actor,
                facts,
                Some(before),
                json!({ "schema_version": latest }),
            )
            .await?;

            Ok(latest)
        }
        Err(err) => {
            let detail = err.to_string();
            record_failure(
                catalog,
                DeskAction::WorkspaceMigrated,
                Some(slug),
                actor,
                facts,
                &detail,
            )
            .await;
            Err(err.into())
        }
    }
}

/// Bring every outdated workspace forward, in one pass.
///
/// The same function the server runs on boot. One workspace failing does not
/// stop the rest - a workspace that cannot be migrated is a problem for that
/// workspace, and refusing to continue would take out every other one with it.
/// The failures come back by slug, and the audit row names them.
pub async fn migrate_outdated(
    catalog: &Catalog,
    database: &DatabaseConfig,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
) -> ServiceResult<MigrationSweep> {
    let sweep = provision::migrate_outdated_tenants(catalog, database).await?;

    let outcome = if sweep.failed.is_empty() {
        Outcome::Ok
    } else {
        Outcome::Failed
    };

    // No `tenant_slug`: this row is about the estate rather than about one
    // workspace, and putting a slug on it would make it turn up in the wrong
    // workspace's history. What it swept is in `after`.
    phonix_db::desk::audit::record(
        catalog.pool(),
        DeskAuditEntry::new(DeskAction::WorkspacesSwept, outcome)
            .actor(Some(actor.id()), Some(actor.email()))
            .from_to(
                json!({ "schema_version": "various" }),
                json!({
                    "schema_version": apps::schema_fingerprint(),
                    "already_current": sweep.current,
                    "migrated": sweep.migrated,
                    "failed": sweep.failed,
                }),
            )
            .from_client(facts.ip),
    )
    .await?;

    Ok(sweep)
}

/// Suspend a workspace, or resume it.
///
/// One function for both, because they are one column and giving them separate
/// use cases would be two places to keep the guards in step. What differs is
/// the action written to the trail, so "who suspended this" stays a question a
/// person answers by scanning a column.
///
/// The licence is untouched. A suspension is somebody's decision; a lapse is a
/// date passing. Resuming a workspace whose licence has since expired leaves it
/// still refused, and that is correct - it is a second thing to fix, and the
/// page says which.
pub async fn set_status(
    catalog: &Catalog,
    slug: &TenantSlug,
    status: TenantStatus,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
) -> ServiceResult<TenantRecord> {
    let tenant = require(catalog, slug).await?;

    if tenant.status == status {
        return Err(ServiceError::Rejected(vec![FieldError::new(
            "status",
            msg!("desk.workspace.status_unchanged"),
        )]));
    }

    // A workspace that never finished provisioning has no database to serve
    // from, and marking it active would route traffic into nothing. That is the
    // exact failure ADR 0005 section 12 gives as the reason a table editor is
    // not a substitute for a use case.
    if tenant.status == TenantStatus::Provisioning && status == TenantStatus::Active {
        return Err(ServiceError::Rejected(vec![FieldError::new(
            "status",
            msg!("desk.workspace.cannot_resume_unprovisioned"),
        )]));
    }

    catalog.set_status(slug, status).await?;

    let action = match status {
        TenantStatus::Active => DeskAction::WorkspaceResumed,
        _ => DeskAction::WorkspaceSuspended,
    };

    record(
        catalog,
        action,
        Outcome::Ok,
        slug,
        actor,
        facts,
        Some(json!({ "status": tenant.status.as_str() })),
        json!({ "status": status.as_str() }),
    )
    .await?;

    tracing::info!(
        tenant = %slug,
        from = tenant.status.as_str(),
        to = status.as_str(),
        by = actor.email(),
        "workspace status changed"
    );

    require(catalog, slug).await
}

/// The workspace, or `UnknownTenant`.
///
/// Every write here starts with this rather than with an `UPDATE ... WHERE`,
/// because the before-state is half of what the audit row records.
async fn require(catalog: &Catalog, slug: &TenantSlug) -> ServiceResult<TenantRecord> {
    catalog
        .find_by_slug(slug)
        .await?
        .ok_or_else(|| phonix_db::DbError::UnknownTenant(slug.to_string()).into())
}

/// Write one row about one workspace.
///
/// Not best-effort, for the reason `phonix_db::desk::audit::record` gives: an
/// action nobody can attribute is worse than one that failed and has to be
/// repeated.
#[allow(clippy::too_many_arguments)]
async fn record(
    catalog: &Catalog,
    action: DeskAction,
    outcome: Outcome,
    slug: &TenantSlug,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
    before: Option<serde_json::Value>,
    after: serde_json::Value,
) -> ServiceResult<()> {
    let entry = DeskAuditEntry::new(action, outcome)
        .actor(Some(actor.id()), Some(actor.email()))
        .about(slug.as_str())
        .from_to(before.unwrap_or(serde_json::Value::Null), after)
        .from_client(facts.ip);

    phonix_db::desk::audit::record(catalog.pool(), entry).await?;
    Ok(())
}

/// Record an action that was allowed and then broke.
///
/// Best-effort *here specifically*: the caller is already returning the real
/// failure, and turning "the migration failed and so did writing that down"
/// into a different error would hide the one that matters. It is logged
/// loudly instead.
async fn record_failure(
    catalog: &Catalog,
    action: DeskAction,
    slug: Option<&TenantSlug>,
    actor: &DeskCaller,
    facts: ClientFacts<'_>,
    detail: &str,
) {
    let mut entry = DeskAuditEntry::new(action, Outcome::Failed)
        .actor(Some(actor.id()), Some(actor.email()))
        .detail(detail)
        .from_client(facts.ip);

    let slug = slug.map(TenantSlug::as_str);
    if let Some(slug) = slug {
        entry = entry.about(slug);
    }

    if let Err(err) = phonix_db::desk::audit::record(catalog.pool(), entry).await {
        tracing::error!(
            error = %err,
            action = action.as_str(),
            "could not record a failed desk action"
        );
    }
}

/// A licence as the audit trail records it.
///
/// `null` for "there was none", which is what makes an issue distinguishable
/// from an extension when the two rows are read back a year later.
fn snapshot(licence: Option<&Licence>) -> serde_json::Value {
    match licence {
        None => serde_json::Value::Null,
        Some(licence) => json!({
            "state": licence.state.as_str(),
            "valid_from": licence.valid_from,
            "valid_until": licence.valid_until,
            "note": licence.note,
            "updated_by": licence.updated_by,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn licence_at(from: DateTime<Utc>) -> Licence {
        Licence {
            state: LicenceState::Trial,
            valid_from: from,
            valid_until: Some(from + chrono::Duration::days(30)),
            note: Some("Trial issued by self-service signup, 30 days.".to_owned()),
            updated_at: from,
            updated_by: Some("signup".to_owned()),
        }
    }

    /// The from-to shape, and the case that shape exists for: an issue and an
    /// extension have to be tellable apart from the row alone.
    #[test]
    fn an_issue_and_an_extension_are_different_rows() {
        let issued = snapshot(None);
        let extended = snapshot(Some(&licence_at(Utc::now())));

        assert!(issued.is_null());
        assert!(!extended.is_null());
        assert_eq!(extended["state"], "trial");
    }

    /// A snapshot carries the reason and the person, not only the dates.
    /// Reading the trail a year later, "who decided this and why" is the whole
    /// question.
    #[test]
    fn a_snapshot_records_who_decided_and_why() {
        let snap = snapshot(Some(&licence_at(Utc::now())));

        assert_eq!(snap["updated_by"], "signup");
        assert!(snap["note"].as_str().unwrap().contains("30 days"));
    }
}
