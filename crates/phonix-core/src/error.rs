//! The error type shared by the server and the client.
//!
//! Variants are deliberately coarse: anything crossing the server-function
//! boundary is serialised and shown to a browser, so it must never carry a
//! connection string, a SQL fragment or a backtrace. Detailed causes are logged
//! server-side and replaced here by a stable, safe summary.

use serde::{Deserialize, Serialize};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Error {
    /// The request host did not map to a known tenant.
    #[error("unknown tenant: {0}")]
    UnknownTenant(String),

    /// The tenant exists but is suspended or archived.
    #[error("tenant is not active: {0}")]
    TenantInactive(String),

    /// No tenant could be derived from the request at all.
    #[error("no tenant in request context")]
    MissingTenantContext,

    /// Caller-supplied data failed validation.
    #[error("invalid input: {0}")]
    Validation(String),

    /// Caller is not authenticated.
    #[error("not authenticated")]
    Unauthenticated,

    /// Caller is authenticated but not allowed to do this.
    #[error("not authorised")]
    Forbidden,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("rate limited")]
    RateLimited,

    /// A dependency (database, cache, broker) failed. The real cause is logged
    /// server-side; this carries only a short, non-sensitive label.
    #[error("service unavailable: {0}")]
    Unavailable(String),

    /// Catch-all for unexpected server-side faults.
    #[error("internal error")]
    Internal,
}

impl Error {
    /// HTTP status this error should map to.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnknownTenant(_) | Self::NotFound(_) => 404,
            Self::TenantInactive(_) | Self::Forbidden => 403,
            Self::Unauthenticated => 401,
            Self::Validation(_) => 422,
            Self::Conflict(_) => 409,
            Self::RateLimited => 429,
            Self::Unavailable(_) => 503,
            Self::MissingTenantContext | Self::Internal => 500,
        }
    }

    /// Whether the caller could reasonably retry the same request.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::RateLimited)
    }

    /// Short, stable identifier for dashboards and alert rules.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTenant(_) => "unknown_tenant",
            Self::TenantInactive(_) => "tenant_inactive",
            Self::MissingTenantContext => "missing_tenant_context",
            Self::Validation(_) => "validation",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Unavailable(_) => "unavailable",
            Self::Internal => "internal",
        }
    }
}

impl From<crate::tenant::InvalidTenantSlug> for Error {
    fn from(err: crate::tenant::InvalidTenantSlug) -> Self {
        Self::Validation(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_match_semantics() {
        assert_eq!(Error::Unauthenticated.status_code(), 401);
        assert_eq!(Error::Forbidden.status_code(), 403);
        assert_eq!(Error::UnknownTenant("x".into()).status_code(), 404);
        assert_eq!(Error::Internal.status_code(), 500);
        assert_eq!(Error::Unavailable("db".into()).status_code(), 503);
    }

    #[test]
    fn only_transient_failures_are_retryable() {
        assert!(Error::Unavailable("db".into()).is_retryable());
        assert!(Error::RateLimited.is_retryable());
        assert!(!Error::Validation("bad".into()).is_retryable());
        assert!(!Error::Internal.is_retryable());
    }

    #[test]
    fn round_trips_across_the_server_function_boundary() {
        let err = Error::Validation("email is required".into());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(serde_json::from_str::<Error>(&json).unwrap(), err);
    }
}
