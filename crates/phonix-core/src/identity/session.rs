//! What kind of client is holding a session.
//!
//! A session is a session whether it reached us in a cookie or in an
//! `Authorization` header - same row, same two deadlines, same `revoked_at`,
//! same `mfa_satisfied`. See `docs/adr/0003-mobile-authentication.md` for why
//! a phone was not given a credential subsystem of its own.
//!
//! What the kind decides is narrow and worth stating exactly:
//!
//! * **which deadlines apply.** `[security.session]` is tuned for a browser -
//!   12 hours idle, 7 days absolute - and `[security.session.mobile]` for an
//!   application somebody expects to stay signed in to. The kind is read again
//!   on every request, because sliding the idle deadline is not a sign-in-time
//!   decision.
//! * **what a device list calls it.** "the phone app, last seen an hour ago"
//!   is a different fact from a user-agent string, and it is the fact somebody
//!   reviewing their own account is actually reading.
//!
//! What it does **not** decide is what the session may do. Permissions come
//! from the account, resolved per request; a phone and a browser held by the
//! same person reach exactly the same things.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// How a session's token travels, and therefore which lifetimes it lives by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// A cookie set on the workspace's own host.
    #[default]
    Browser,
    /// A bearer token held by an application on somebody's phone.
    Mobile,
}

impl SessionKind {
    /// The stable value stored in `sessions.kind`.
    ///
    /// Matched against the `sessions_kind_known` check constraint, so a variant
    /// added here without a migration fails on the first insert rather than
    /// quietly writing a value nothing reads.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Mobile => "mobile",
        }
    }

    /// Whether this session is presented as an `Authorization: Bearer` token
    /// rather than as a cookie.
    pub const fn is_bearer(self) -> bool {
        matches!(self, Self::Mobile)
    }
}

impl fmt::Display for SessionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stored value this build does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSessionKind(pub String);

impl fmt::Display for UnknownSessionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}' is not a session kind this build knows", self.0)
    }
}

impl FromStr for SessionKind {
    type Err = UnknownSessionKind;

    /// Deliberately strict. A row whose kind cannot be read is a row whose
    /// deadlines cannot be chosen, and guessing "browser" would sign a phone
    /// out on a browser's schedule - which reads as an application that
    /// randomly forgets people.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "browser" => Ok(Self::Browser),
            "mobile" => Ok(Self::Mobile),
            other => Err(UnknownSessionKind(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_stored_value() {
        for kind in [SessionKind::Browser, SessionKind::Mobile] {
            assert_eq!(kind.as_str().parse::<SessionKind>(), Ok(kind));
        }
    }

    #[test]
    fn an_unknown_stored_value_is_refused_rather_than_guessed() {
        assert!("desktop".parse::<SessionKind>().is_err());
        // Case matters: the check constraint stores lower case, and a match
        // that accepted 'Browser' would hide a writer that is not ours.
        assert!("Browser".parse::<SessionKind>().is_err());
    }

    #[test]
    fn a_session_with_no_kind_stated_is_a_browser() {
        // The column defaults to 'browser' for every row that predates the
        // migration, and this is the Rust side of that same statement.
        assert_eq!(SessionKind::default(), SessionKind::Browser);
    }

    #[test]
    fn only_the_mobile_kind_travels_as_a_bearer_token() {
        assert!(!SessionKind::Browser.is_bearer());
        assert!(SessionKind::Mobile.is_bearer());
    }
}
