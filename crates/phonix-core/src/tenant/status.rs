//! Tenant lifecycle and the summary attached to every request.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::licence::{Licence, LicenceStanding, standing_of};
use super::slug::TenantSlug;

/// Stable primary key of a tenant in the catalog database.
pub type TenantId = Uuid;

/// Lifecycle state of a tenant, mirrored from `catalog.tenants.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    /// Database exists and migrations are current; serve traffic.
    Active,
    /// Row exists but the database has not been created yet.
    Provisioning,
    /// Deliberately disabled (non-payment, admin action). Reject with 403.
    Suspended,
    /// Scheduled for deletion. Reject like an unknown tenant.
    Archived,
}

impl TenantStatus {
    /// Whether requests for this tenant should be served.
    ///
    /// # Why this takes a licence
    ///
    /// It used to be `matches!(self, Self::Active)` and nothing else. Two
    /// separate facts decide whether a workspace is served - whether we stopped
    /// it, and whether it is authorized to be here - and they are stored
    /// separately on purpose: a lapse is a date passing and a suspension is
    /// somebody's decision with their name against it. If a job flipped
    /// `status` to `suspended` on expiry, reinstating a workspace would mean
    /// guessing what its status had been before. See ADR 0005 section 7.
    ///
    /// Taking the licence as an argument rather than reading it somewhere is
    /// what makes every call site say which licence it decided against. There
    /// were four when this changed, and the compiler found all of them.
    ///
    /// This runs on every request through `Catalog::find_active`, against a row
    /// that is already loaded - so it is one comparison, not a second lookup.
    pub fn serves_traffic(self, licence: Option<&Licence>) -> bool {
        matches!(self, Self::Active) && standing_of(licence, Utc::now()).authorizes()
    }

    /// Why a workspace is not served, or that it is.
    ///
    /// `None` means it is. Otherwise the caller gets the standing to refuse
    /// with, which is what lets "your licence ended" and "we stopped you" be
    /// different sentences.
    pub fn licence_problem(self, licence: Option<&Licence>) -> Option<LicenceStanding> {
        match standing_of(licence, Utc::now()) {
            LicenceStanding::Current => None,
            problem => Some(problem),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Provisioning => "provisioning",
            Self::Suspended => "suspended",
            Self::Archived => "archived",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "provisioning" => Some(Self::Provisioning),
            "suspended" => Some(Self::Suspended),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

/// The tenant facts a request handler actually needs. Attached to every request
/// by the tenant-resolution middleware and readable from server functions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantSummary {
    pub id: TenantId,
    pub slug: TenantSlug,
    pub display_name: String,
    pub status: TenantStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::licence::{Licence, LicenceState};

    fn current_licence() -> Licence {
        Licence {
            state: LicenceState::Licensed,
            valid_from: Utc::now() - chrono::Duration::days(1),
            valid_until: None,
            note: None,
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    #[test]
    fn only_active_tenants_serve_traffic() {
        let licence = current_licence();

        assert!(TenantStatus::Active.serves_traffic(Some(&licence)));
        for blocked in [
            TenantStatus::Provisioning,
            TenantStatus::Suspended,
            TenantStatus::Archived,
        ] {
            assert!(
                !blocked.serves_traffic(Some(&licence)),
                "{blocked:?} must not serve"
            );
        }
    }

    /// The half of the answer that is new. An active workspace with nothing
    /// authorizing it is not served, and that is deliberately not the same
    /// thing as having been suspended.
    #[test]
    fn an_active_workspace_with_no_licence_is_not_served() {
        assert!(!TenantStatus::Active.serves_traffic(None));
        assert_eq!(
            TenantStatus::Active.licence_problem(None),
            Some(super::super::licence::LicenceStanding::Missing)
        );
    }

    /// A suspended workspace holding a perfectly good licence stays suspended.
    /// The two facts are ANDed, and neither can widen the other.
    #[test]
    fn a_licence_cannot_un_suspend_a_workspace() {
        let licence = current_licence();

        assert!(!TenantStatus::Suspended.serves_traffic(Some(&licence)));
        // ...and the licence is not the reason, which is what the detail page
        // has to be able to say.
        assert_eq!(
            TenantStatus::Suspended.licence_problem(Some(&licence)),
            None
        );
    }

    #[test]
    fn statuses_round_trip_through_their_stored_form() {
        for status in [
            TenantStatus::Active,
            TenantStatus::Provisioning,
            TenantStatus::Suspended,
            TenantStatus::Archived,
        ] {
            assert_eq!(TenantStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(TenantStatus::parse("deleted"), None);
    }
}
