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
use phonix_core::identity::validation::FieldError;
use phonix_core::msg;
use phonix_core::{Licence, LicenceState, TenantSlug};
use phonix_db::desk::audit::{DeskAction, DeskAuditEntry, Outcome};
use phonix_db::desk::session::ClientFacts;
use phonix_db::tenancy::catalog::{Catalog, TenantRecord};
use phonix_db::tenancy::licence::{self, LicenceInput};
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
