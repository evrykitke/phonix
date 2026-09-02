//! Reading `catalog.desk_audit`.
//!
//! Desk's own read of that table is the only screen in this product that shows
//! it, which is the point: "who suspended this workspace" must not be a row
//! that workspace's own administrators can read, edit, or lose when the
//! database is archived. See ADR 0005 section 8.
//!
//! # There is no write here
//!
//! Every row is written by the use case it belongs to, beside the change it
//! describes. A module that could both write and read the trail would be a
//! place where a row could be written without the thing it records.
//!
//! # And no "read audit" event
//!
//! Opening this page writes nothing. A list of what happened is not something
//! that happened.

use chrono::{DateTime, Utc};
use phonix_db::desk::audit::{self, DeskAction, DeskAuditRecord, Outcome};
use phonix_db::tenancy::catalog::Catalog;
use serde_json::Value as Json;

use crate::error::ServiceResult;

/// How many rows a page of the trail holds.
///
/// Enough that an afternoon's work is one screen, small enough that the page
/// is not a megabyte. Not configurable: a number nobody will ever change is a
/// setting nobody will ever read.
pub const PAGE_SIZE: i64 = 50;

/// One entry, with the parts a screen needs already decided.
pub struct Entry {
    pub occurred_at: DateTime<Utc>,
    /// The action's own words, or the raw stored string for a row this build
    /// does not recognise - which means a row written by a newer one. Shown
    /// rather than hidden: a trail that silently drops what it cannot name is
    /// worse than one that shows an unfamiliar word.
    pub action: String,
    pub actor: String,
    pub tenant_slug: Option<String>,
    pub outcome: Outcome,
    pub detail: Option<String>,
    pub ip: Option<String>,
    /// The change, from → to. Empty for an action that had no before-state - a
    /// sign-in, a migration - which is not a gap but the honest answer.
    pub changes: Vec<Change>,
}

/// One field of a from → to record.
pub struct Change {
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// A page of the trail, and enough to draw a pager.
pub struct TrailPage {
    pub entries: Vec<Entry>,
    pub total: i64,
    /// Zero-based, because it is an offset divided by a page size and calling
    /// it "page 1" here would mean subtracting one in two places.
    pub page: i64,
    pub pages: i64,
}

/// Read one page, newest first.
pub async fn page(catalog: &Catalog, page: i64) -> ServiceResult<TrailPage> {
    let total = audit::count(catalog.pool()).await?;
    let pages = total.div_euclid(PAGE_SIZE) + i64::from(total.rem_euclid(PAGE_SIZE) != 0);
    let page = page.clamp(0, pages.max(1) - 1);

    let rows = audit::recent(catalog.pool(), PAGE_SIZE, page * PAGE_SIZE).await?;

    Ok(TrailPage {
        entries: rows.iter().map(entry).collect(),
        total,
        page,
        pages: pages.max(1),
    })
}

/// One workspace's own history.
pub async fn for_workspace(catalog: &Catalog, slug: &str, limit: i64) -> ServiceResult<Vec<Entry>> {
    let rows = audit::for_tenant(catalog.pool(), slug, limit).await?;

    Ok(rows.iter().map(entry).collect())
}

fn entry(row: &DeskAuditRecord) -> Entry {
    Entry {
        occurred_at: row.occurred_at,
        action: DeskAction::parse(&row.action)
            .map(|action| action.label().to_owned())
            .unwrap_or_else(|| row.action.clone()),
        actor: row
            .actor_email
            .clone()
            // The bootstrap subcommand has no session behind it, and a failed
            // sign-in may name nobody at all. Said plainly rather than left
            // blank, because a blank column reads as a bug.
            .unwrap_or_else(|| "the system".to_owned()),
        tenant_slug: row.tenant_slug.clone(),
        outcome: Outcome::parse(&row.outcome).unwrap_or(Outcome::Ok),
        detail: row.detail.clone(),
        ip: row.ip.clone(),
        changes: diff(row.before_state.as_ref(), row.after_state.as_ref()),
    }
}

/// Turn a before/after pair into a list of changed fields.
///
/// The shape the tenant entity trail already established, and the reason it was
/// established: a diff is what a person reads, and narration is what they have
/// to decode. Unchanged fields are dropped - a row where one date moved should
/// show one date moving.
fn diff(before: Option<&Json>, after: Option<&Json>) -> Vec<Change> {
    let mut fields: Vec<String> = Vec::new();
    for side in [before, after] {
        if let Some(Json::Object(map)) = side {
            for key in map.keys() {
                if !fields.iter().any(|seen| seen == key) {
                    fields.push(key.clone());
                }
            }
        }
    }

    fields
        .into_iter()
        .filter_map(|field| {
            let was = read(before, &field);
            let now = read(after, &field);

            (was != now).then_some(Change {
                field,
                before: was,
                after: now,
            })
        })
        .collect()
}

/// One field of one side, as a sentence fragment.
fn read(side: Option<&Json>, field: &str) -> Option<String> {
    match side?.get(field)? {
        Json::Null => None,
        Json::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_fields_that_moved_are_reported() {
        let before = json!({ "state": "trial", "valid_until": "2026-01-01", "note": "same" });
        let after = json!({ "state": "licensed", "valid_until": "2027-01-01", "note": "same" });

        let changes = diff(Some(&before), Some(&after));

        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|change| change.field != "note"));
    }

    /// The case the whole from → to shape exists to make legible: something
    /// that had no previous value at all. An issue and an extension have to
    /// read differently.
    #[test]
    fn a_field_that_had_no_value_reads_as_having_none() {
        let after = json!({ "state": "trial" });

        let changes = diff(None, Some(&after));

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before, None);
        assert_eq!(changes[0].after.as_deref(), Some("trial"));
    }

    /// An action with no before-state - a sign-in, a sweep - is not a row with
    /// a broken diff. It has nothing to show, and shows nothing.
    #[test]
    fn an_action_with_no_change_produces_no_diff() {
        assert!(diff(None, None).is_empty());
    }

    /// A field cleared is a change, and one worth seeing: a licence that lost
    /// its end date became open-ended, which is a decision.
    #[test]
    fn clearing_a_field_is_a_change() {
        let before = json!({ "valid_until": "2026-01-01" });
        let after = json!({ "valid_until": null });

        let changes = diff(Some(&before), Some(&after));

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].after, None);
    }
}
