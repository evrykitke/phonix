//! Tenant lifecycle and the summary attached to every request.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    pub fn serves_traffic(self) -> bool {
        matches!(self, Self::Active)
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

    #[test]
    fn only_active_tenants_serve_traffic() {
        assert!(TenantStatus::Active.serves_traffic());
        for blocked in [
            TenantStatus::Provisioning,
            TenantStatus::Suspended,
            TenantStatus::Archived,
        ] {
            assert!(!blocked.serves_traffic(), "{blocked:?} must not serve");
        }
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
