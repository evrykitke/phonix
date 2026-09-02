//! Whether a workspace is authorized to be here, and until when.
//!
//! A licence answers exactly one question and is not a plan, a bundle of
//! entitlements or a feature switch. See `docs/adr/0005-phonix-desk.md`
//! section 7 for what was deliberately left out of it - seats, editions,
//! anything priced - and why that belongs beside this row rather than in it.
//!
//! # Why it lives here and not in the workspace
//!
//! A licence a tenant's own administrators can reach is not a licence. The row
//! is in the catalog, and this module is the vocabulary both sides read it
//! with. There is already a cautionary example in this codebase:
//! `workspace_settings.api_enabled` sits in the tenant's own database and can
//! be flipped by anyone holding `Settings`, which is a feature switch
//! pretending to be a commercial one.
//!
//! # Three states, not two dates
//!
//! "It ran out" and "we withdrew it" are the same date arithmetic and
//! completely different events - the first is answered by extending, the second
//! by a conversation. And a trial that expires is the expected case where a
//! paid licence that expires is somebody's problem to chase, which is why
//! [`LicenceState::Trial`] is separate from [`LicenceState::Licensed`] rather
//! than being inferred from how long the term is.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What kind of permission this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenceState {
    /// Time-limited, issued by self-service signup and by Desk. A trial is a
    /// licence with an end date, not a separate concept - which means the
    /// expiry path is exercised constantly rather than for the first time on a
    /// real customer.
    Trial,
    /// Paid, or internal, or a demonstration. May have no end date.
    Licensed,
    /// Withdrawn by a person. Never set by a date passing - see
    /// [`Licence::is_current_at`].
    Revoked,
}

impl LicenceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trial => "trial",
            Self::Licensed => "licensed",
            Self::Revoked => "revoked",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "trial" => Some(Self::Trial),
            "licensed" => Some(Self::Licensed),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }

    /// Every state, in the order a form should offer them.
    pub const ALL: [Self; 3] = [Self::Trial, Self::Licensed, Self::Revoked];
}

/// One workspace's licence, as stored in `catalog.tenant_licences`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Licence {
    pub state: LicenceState,
    pub valid_from: DateTime<Utc>,
    /// `None` means no end: an internal workspace, a demonstration tenant. A
    /// licence with no end is a deliberate act by a named desk user, and the
    /// audit row saying so is the point.
    pub valid_until: Option<DateTime<Utc>>,
    /// Free text, for the human reason. Never parsed.
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
    /// The address of the desk user who last decided this, or `migration` for
    /// the backfill that licensed the workspaces predating the idea.
    pub updated_by: Option<String>,
}

impl Licence {
    /// Whether this licence authorizes the workspace right now.
    pub fn is_current(&self) -> bool {
        self.is_current_at(Utc::now())
    }

    /// The same question at a stated instant.
    ///
    /// Separated so the answer can be tested without waiting for a date, and so
    /// a caller deciding several workspaces at once decides them all against
    /// one clock reading rather than a slightly different one each time.
    pub fn is_current_at(&self, now: DateTime<Utc>) -> bool {
        matches!(self.standing_at(now), LicenceStanding::Current)
    }

    /// Why the licence is, or is not, current.
    ///
    /// The screen shows this and the refusal quotes it: "your licence ended"
    /// and "we stopped you" are different sentences to receive, and a caller
    /// that only has a boolean cannot tell them apart.
    pub fn standing_at(&self, now: DateTime<Utc>) -> LicenceStanding {
        if self.state == LicenceState::Revoked {
            return LicenceStanding::Revoked;
        }
        if now < self.valid_from {
            return LicenceStanding::NotYetStarted;
        }
        match self.valid_until {
            Some(until) if now >= until => LicenceStanding::Expired,
            _ => LicenceStanding::Current,
        }
    }

    pub fn standing(&self) -> LicenceStanding {
        self.standing_at(Utc::now())
    }
}

/// Where a workspace stands, including having no licence at all.
///
/// [`Self::Missing`] is not a state a licence can be in - it is the answer for
/// a workspace with no row, which after the backfill in catalog migration 0005
/// means one created between that migration and the code that issues one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenceStanding {
    Current,
    /// Dated to begin later. Rare and deliberate.
    NotYetStarted,
    /// `valid_until` has passed. A lapse, not a suspension.
    Expired,
    Revoked,
    Missing,
}

impl LicenceStanding {
    /// Whether a workspace in this standing may be served.
    pub fn authorizes(self) -> bool {
        matches!(self, Self::Current)
    }

    /// One short phrase, for a pill on a screen and for a log line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::NotYetStarted => "not yet started",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::Missing => "unlicensed",
        }
    }

    /// The sentence a refused request is answered with.
    ///
    /// Deliberately different per standing. A customer whose trial ran out and
    /// a customer whose licence was withdrawn are looking at the same 403 and
    /// need to know which of the two conversations to start.
    pub fn refusal(self) -> &'static str {
        match self {
            Self::Current => "this workspace is licensed",
            Self::NotYetStarted => "this workspace's licence has not started yet",
            Self::Expired => "this workspace's licence has ended",
            Self::Revoked => "this workspace's licence was withdrawn",
            Self::Missing => "this workspace has no licence",
        }
    }
}

/// The standing of a licence that may not be there.
///
/// The one place `Option<&Licence>` is turned into an answer, so "no row" and
/// "a row that has lapsed" cannot come to mean different things in two callers.
pub fn standing_of(licence: Option<&Licence>, now: DateTime<Utc>) -> LicenceStanding {
    match licence {
        Some(licence) => licence.standing_at(now),
        None => LicenceStanding::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn at(offset_days: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_760_000_000, 0).unwrap() + Duration::days(offset_days)
    }

    fn licence(state: LicenceState, until: Option<DateTime<Utc>>) -> Licence {
        Licence {
            state,
            valid_from: at(0),
            valid_until: until,
            note: None,
            updated_at: at(0),
            updated_by: None,
        }
    }

    #[test]
    fn a_licence_with_no_end_never_lapses() {
        let open = licence(LicenceState::Licensed, None);

        assert!(open.is_current_at(at(0)));
        assert!(open.is_current_at(at(10_000)));
    }

    /// Half-open, like every other interval in this codebase: the instant named
    /// by `valid_until` is the first one *not* covered.
    #[test]
    fn the_end_date_is_the_first_instant_not_covered() {
        let trial = licence(LicenceState::Trial, Some(at(30)));

        assert!(trial.is_current_at(at(30) - Duration::seconds(1)));
        assert!(!trial.is_current_at(at(30)));
    }

    /// The distinction the whole three-state design exists for. Both of these
    /// refuse the request; they do not refuse it with the same sentence.
    #[test]
    fn a_lapse_and_a_withdrawal_are_told_apart() {
        let lapsed = licence(LicenceState::Trial, Some(at(1)));
        let withdrawn = licence(LicenceState::Revoked, None);

        assert_eq!(lapsed.standing_at(at(2)), LicenceStanding::Expired);
        assert_eq!(withdrawn.standing_at(at(2)), LicenceStanding::Revoked);
        assert_ne!(
            LicenceStanding::Expired.refusal(),
            LicenceStanding::Revoked.refusal()
        );
    }

    /// Revocation is a person's decision and outranks the dates. A licence
    /// withdrawn today with a `valid_until` next year is withdrawn.
    #[test]
    fn a_withdrawal_outranks_a_date_that_has_not_passed() {
        let withdrawn = licence(LicenceState::Revoked, Some(at(365)));

        assert!(!withdrawn.is_current_at(at(1)));
    }

    #[test]
    fn no_licence_at_all_is_its_own_answer() {
        assert_eq!(standing_of(None, at(0)), LicenceStanding::Missing);
        assert!(!LicenceStanding::Missing.authorizes());
    }

    #[test]
    fn every_state_survives_a_round_trip() {
        for state in LicenceState::ALL {
            assert_eq!(LicenceState::parse(state.as_str()), Some(state));
        }
        assert_eq!(LicenceState::parse("expired"), None);
    }

    /// `expired` is a standing, never a stored state. If it ever became one,
    /// a date passing would start overwriting the reason a licence ended - and
    /// the difference between a lapse and a withdrawal is the thing this file
    /// exists to keep.
    #[test]
    fn expiry_is_not_a_state_a_row_can_hold() {
        assert!(
            LicenceState::ALL
                .iter()
                .all(|state| state.as_str() != "expired")
        );
    }
}
