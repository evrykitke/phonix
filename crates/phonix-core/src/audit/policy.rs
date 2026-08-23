//! Whether a change is recorded at all, and for how long it is kept.
//!
//! # Why this is a setting and not a constant
//!
//! The change trail grows without bound and nothing ever deletes from it. That
//! is right for roles and for the security policy, which change a handful of
//! times a year and whose history is the reason the table exists. It is a
//! different proposition for accounts in a workspace of ten thousand people,
//! where the trail can outgrow the records it describes.
//!
//! So an organization decides. Not the operator - this is about how much
//! history the customer wants to keep about themselves, which is exactly the
//! kind of decision `workspace_settings` holds.
//!
//! # Kinds are excluded, never included
//!
//! [`AuditPolicy::excluded`] names the kinds that are *off*. Storing it the
//! other way round would mean that a kind added to [`ENTITY_KINDS`] in a later
//! release arrives switched off in every existing workspace, silently and with
//! nothing on screen to say so - and an audit trail that quietly stops covering
//! new things is worse than one that was never switched on.
//!
//! The cost is that turning a kind off is a positive act that survives
//! upgrades, which is the behaviour worth having on both sides.
//!
//! # Turning it off is itself recorded
//!
//! [`records`](Self::records) is consulted by `phonix_services::audit`, and the
//! one thing it does not gate is a change to this policy. "Who stopped the
//! recording, and when" is the single entry that must survive the recording
//! being stopped; see `Target::always`.

use serde::{Deserialize, Serialize};

use super::entity::{ENTITY_KINDS, EntityKind};
use crate::identity::validation::FieldError;
use crate::{Message, msg, pmsg};

/// The longest retention an administrator may ask for, in days.
///
/// Ten years. Past that the number stops meaning "keep it for a while" and
/// starts meaning "forever" - which is what leaving it unset already says, more
/// clearly and without a prune job walking the table every night.
pub const MAX_RETENTION_DAYS: i32 = 3650;

/// The shortest retention that is not simply "throw it away".
///
/// A week. Anything under it makes the trail useless for the thing a trail is
/// for - somebody noticing on Monday that something went wrong on Friday.
pub const MIN_RETENTION_DAYS: i32 = 7;

/// What this workspace records, and what it keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditPolicy {
    /// The master switch. When false nothing is written, whatever
    /// [`Self::excluded`] says.
    ///
    /// Existing entries are left alone. Switching the trail off is a decision
    /// about the future; deleting what is already there is what
    /// [`Self::retention_days`] is for, and conflating the two would mean a
    /// checkbox that destroys history.
    pub enabled: bool,
    /// The stored names of the kinds this workspace does *not* record.
    ///
    /// Unknown names are kept rather than dropped: a workspace that switched a
    /// kind off, then rolled back to a build that has never heard of it, must
    /// not silently switch it back on when it rolls forward again.
    pub excluded: Vec<String>,
    /// Delete entries older than this many days. `None` keeps them forever.
    pub retention_days: Option<i32>,
}

impl Default for AuditPolicy {
    fn default() -> Self {
        Self::system_default()
    }
}

impl AuditPolicy {
    /// What a workspace starts with: everything recorded, nothing deleted.
    ///
    /// On rather than off, and forever rather than a year. A trail somebody has
    /// to go and switch on is one that is off on the day it was needed, and a
    /// default that deletes is a default that loses evidence nobody agreed to
    /// lose.
    pub const fn system_default() -> Self {
        Self {
            enabled: true,
            excluded: Vec::new(),
            retention_days: None,
        }
    }

    /// Whether changes to this kind of record are written.
    pub fn records(&self, kind: EntityKind) -> bool {
        self.records_named(kind.name)
    }

    /// The same question, for a kind that arrived as a stored name.
    pub fn records_named(&self, name: &str) -> bool {
        self.enabled && !self.excluded.iter().any(|excluded| excluded == name)
    }

    /// The declared kinds this workspace records, in declaration order.
    pub fn included_kinds(&self) -> Vec<EntityKind> {
        if !self.enabled {
            return Vec::new();
        }

        ENTITY_KINDS
            .iter()
            .copied()
            .filter(|kind| self.records(*kind))
            .collect()
    }

    /// Switch one kind on or off, leaving the rest alone.
    #[must_use]
    pub fn with_kind(mut self, kind: EntityKind, recorded: bool) -> Self {
        self.excluded.retain(|excluded| excluded != kind.name);

        if !recorded {
            self.excluded.push(kind.name.to_owned());
        }

        self.excluded.sort_unstable();
        self
    }

    /// Check a policy an administrator submitted.
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();

        if let Some(days) = self.retention_days
            && !(MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&days)
        {
            errors.push(FieldError::new(
                "audit_retention_days",
                msg!(
                    "validation.audit.retention_range",
                    min = MIN_RETENTION_DAYS,
                    max = MAX_RETENTION_DAYS
                ),
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// What the settings screen says under the heading, in one line.
    /// One key per shape, not a sentence assembled from clauses.
    ///
    /// The old version glued a scope onto a retention phrase with a comma. That
    /// works in English and nowhere reliably: the clause order, the comma, and
    /// whether "kept" agrees with anything are all decisions the catalog has to
    /// be allowed to make, and it can only make them if it owns the whole
    /// sentence.
    pub fn summary(&self) -> Message {
        if !self.enabled {
            return msg!("audit.summary.off");
        }

        let recorded = self.included_kinds().len();
        let total = ENTITY_KINDS.len();
        let everything = recorded == total;

        match (everything, self.retention_days) {
            (true, None) => msg!("audit.summary.all.forever"),
            (true, Some(days)) => pmsg!("audit.summary.all.kept", i64::from(days)),
            (false, None) => msg!(
                "audit.summary.some.forever",
                recorded = recorded.to_string(),
                total = total.to_string()
            ),
            (false, Some(days)) => pmsg!(
                "audit.summary.some.kept",
                i64::from(days),
                recorded = recorded.to_string(),
                total = total.to_string()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::kinds;

    #[test]
    fn a_new_workspace_records_everything_and_deletes_nothing() {
        // A trail somebody has to switch on is one that was off on the day it
        // was needed.
        let policy = AuditPolicy::system_default();

        assert!(policy.validate().is_ok());
        assert_eq!(policy.included_kinds().len(), ENTITY_KINDS.len());
        assert_eq!(policy.retention_days, None);
    }

    #[test]
    fn a_kind_this_build_has_never_heard_of_is_recorded() {
        // Exclusions are stored, not inclusions. A kind added in a later
        // release must arrive switched *on* in every existing workspace.
        let policy = AuditPolicy {
            excluded: vec!["user".to_owned()],
            ..AuditPolicy::system_default()
        };

        assert!(policy.records_named("purchase_order"));
        assert!(!policy.records_named("user"));
    }

    #[test]
    fn the_master_switch_beats_every_inclusion() {
        let policy = AuditPolicy {
            enabled: false,
            ..AuditPolicy::system_default()
        };

        assert!(!policy.records(kinds::ROLE));
        assert!(policy.included_kinds().is_empty());
    }

    #[test]
    fn switching_one_kind_leaves_the_others_alone() {
        let policy = AuditPolicy::system_default()
            .with_kind(kinds::USER, false)
            .with_kind(kinds::ROLE, false)
            .with_kind(kinds::USER, true);

        assert!(policy.records(kinds::USER));
        assert!(!policy.records(kinds::ROLE));
        assert_eq!(policy.excluded, ["role"]);
    }

    #[test]
    fn switching_a_kind_off_twice_excludes_it_once() {
        // The list is stored, and a duplicate would survive a round trip and
        // then have to be de-duplicated by everything that reads it.
        let policy = AuditPolicy::system_default()
            .with_kind(kinds::USER, false)
            .with_kind(kinds::USER, false);

        assert_eq!(policy.excluded, ["user"]);
    }

    #[test]
    fn a_retention_nobody_could_have_meant_is_refused() {
        for days in [1, MIN_RETENTION_DAYS - 1, MAX_RETENTION_DAYS + 1] {
            let policy = AuditPolicy {
                retention_days: Some(days),
                ..AuditPolicy::system_default()
            };

            assert!(policy.validate().is_err(), "{days} days was accepted");
        }
    }

    #[test]
    fn keeping_entries_forever_is_an_ordinary_answer() {
        // `None` is not a missing value here - it is the default, and it is
        // what most workspaces should stay on.
        let policy = AuditPolicy {
            retention_days: None,
            ..AuditPolicy::system_default()
        };

        assert!(policy.validate().is_ok());
        assert_eq!(policy.summary().key, "audit.summary.all.forever");
    }

    #[test]
    fn the_summary_says_which_of_the_three_states_it_is_in() {
        // On the key rather than on the words: the words are the catalog's to
        // change, and a test that pins them is a test that fails on a comma.
        let off = AuditPolicy {
            enabled: false,
            ..AuditPolicy::system_default()
        };
        assert_eq!(off.summary().key, "audit.summary.off");

        let partial = AuditPolicy::system_default().with_kind(kinds::USER, false);
        let summary = partial.summary();
        assert_eq!(summary.key, "audit.summary.some.forever");
        assert_eq!(
            summary
                .args
                .iter()
                .find(|arg| arg.name == "total")
                .map(|arg| arg.value.as_str()),
            Some(ENTITY_KINDS.len().to_string().as_str()),
        );

        let kept = AuditPolicy {
            retention_days: Some(90),
            ..AuditPolicy::system_default()
        };
        let summary = kept.summary();
        assert_eq!(summary.key, "audit.summary.all.kept");
        assert_eq!(summary.count, Some(90));

        // And it still comes out as a sentence, which is the thing a key alone
        // does not prove.
        let catalog = crate::i18n::Catalog::builtin(crate::Language::ENGLISH);
        assert!(catalog.render(&summary).contains("90 days"));
    }
}
