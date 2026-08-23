//! The tenant slug: a routing key that is also part of a database name.
//!
//! Phonix is database-per-tenant. A slug is resolved from the request host,
//! looked up in the shared catalog, and mapped to a dedicated Postgres
//! database - which is why [`TenantSlug`] validates on construction instead of
//! being a bare `String`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A validated tenant slug, e.g. `acme` in `acme.localhost`.
///
/// Validation is enforced at construction so that a slug can be interpolated
/// into a database name without risking injection. Postgres identifiers are
/// also capped at 63 bytes, which bounds the length here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TenantSlug(String);

/// Longest slug that still leaves room for the `phonix_tenant_` prefix inside
/// Postgres' 63-byte identifier limit.
const MAX_SLUG_LEN: usize = 40;
const MIN_SLUG_LEN: usize = 2;

impl TenantSlug {
    /// Accepts `[a-z0-9]` separated by single hyphens, 2..=40 characters, not
    /// starting or ending with a hyphen.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, InvalidTenantSlug> {
        let raw = raw.as_ref().trim().to_ascii_lowercase();

        if raw.len() < MIN_SLUG_LEN || raw.len() > MAX_SLUG_LEN {
            return Err(InvalidTenantSlug::Length {
                found: raw.len(),
                min: MIN_SLUG_LEN,
                max: MAX_SLUG_LEN,
            });
        }
        if !raw.is_ascii() {
            return Err(InvalidTenantSlug::Charset);
        }
        if raw.starts_with('-') || raw.ends_with('-') {
            return Err(InvalidTenantSlug::Boundary);
        }
        if raw.contains("--") {
            return Err(InvalidTenantSlug::DoubleHyphen);
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(InvalidTenantSlug::Charset);
        }
        // A leading digit is legal in DNS but would force quoting in Postgres.
        // `first` rather than `[0]`: the length check above already rules out
        // an empty slug, but this runs in the browser too, and there an index
        // that is wrong once is a page that has stopped responding.
        if raw.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            return Err(InvalidTenantSlug::LeadingDigit);
        }

        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Physical database name for this tenant, e.g. `phonix_tenant_acme`.
    ///
    /// Safe to interpolate into DDL because [`TenantSlug::parse`] has already
    /// restricted the character set to `[a-z0-9-]`; the hyphens are mapped to
    /// underscores here so the identifier needs no quoting.
    pub fn database_name(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.0.replace('-', "_"))
    }
}

impl fmt::Display for TenantSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TenantSlug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TenantSlug {
    type Error = InvalidTenantSlug;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

// Deserialize goes through `parse` so a slug arriving from JSON, a cookie or a
// config file is validated exactly like one arriving from a Host header.
impl<'de> Deserialize<'de> for TenantSlug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidTenantSlug {
    #[error("tenant slug must be {min}-{max} characters, got {found}")]
    Length {
        found: usize,
        min: usize,
        max: usize,
    },
    #[error("tenant slug may only contain a-z, 0-9 and hyphens")]
    Charset,
    #[error("tenant slug may not start or end with a hyphen")]
    Boundary,
    #[error("tenant slug may not contain consecutive hyphens")]
    DoubleHyphen,
    #[error("tenant slug may not start with a digit")]
    LeadingDigit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_slugs() {
        for good in ["acme", "globex", "initech-2", "a1", "north-wind-co"] {
            assert!(TenantSlug::parse(good).is_ok(), "{good} should parse");
        }
    }

    #[test]
    fn normalises_case_and_whitespace() {
        let slug = TenantSlug::parse("  AcMe  ").unwrap();
        assert_eq!(slug.as_str(), "acme");
    }

    #[test]
    fn rejects_slugs_that_would_be_unsafe_in_ddl() {
        // Each of these would otherwise end up inside a CREATE DATABASE statement.
        for bad in [
            "a",                  // too short
            "-acme",              // leading hyphen
            "acme-",              // trailing hyphen
            "ac--me",             // consecutive hyphens
            "acme;drop database", // injection attempt
            "acme db",            // whitespace
            "acme_db",            // underscore is not in the DNS charset
            "ACME!",              // punctuation
            "1acme",              // leading digit
            "tenant.sub",         // dot would split the host wrongly
            "acmé",               // non-ascii
        ] {
            assert!(TenantSlug::parse(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn rejects_overlong_slugs() {
        assert!(TenantSlug::parse("a".repeat(MAX_SLUG_LEN + 1)).is_err());
        assert!(TenantSlug::parse("a".repeat(MAX_SLUG_LEN)).is_ok());
    }

    #[test]
    fn database_name_is_a_bare_postgres_identifier() {
        let slug = TenantSlug::parse("north-wind").unwrap();
        let name = slug.database_name("phonix_tenant_");
        assert_eq!(name, "phonix_tenant_north_wind");
        // Must fit Postgres' 63-byte identifier limit without quoting.
        assert!(name.len() <= 63);
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        );
    }

    #[test]
    fn deserialize_validates() {
        assert!(serde_json::from_str::<TenantSlug>("\"acme\"").is_ok());
        assert!(serde_json::from_str::<TenantSlug>("\"ac--me\"").is_err());
    }
}
