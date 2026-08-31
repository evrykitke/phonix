//! The people in one workspace, as an administrator sees them.
//!
//! [`AuthUser`](super::user::AuthUser) answers "who is signed in, and what may
//! they do" - it carries a resolved permission set because every screen asks
//! that question. [`UserListing`] answers "who is in this workspace", which is
//! a different question with a different cost: resolving permissions for two
//! hundred rows to render a table nobody reads them in would be two hundred
//! queries for nothing.

use serde::{Deserialize, Serialize};

use super::user::{UserId, UserStatus};

/// One row of the users screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserListing {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub status: UserStatus,
    /// Created the workspace. Shown because the row cannot be suspended or
    /// stripped of its roles, and a table that offers actions it will then
    /// refuse is worse than one that explains itself.
    pub is_owner: bool,
    pub email_verified: bool,
    /// Holds at least one confirmed second factor.
    pub mfa_enabled: bool,
    /// Role names, in the order the database returned them.
    pub roles: Vec<String>,
    /// Set when the account is locked out after failed sign-ins. Nothing
    /// clears it when it passes, so an instant in the past is an ordinary
    /// value meaning "was locked, is not now".
    pub locked_until: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether that lockout still held **when this row was read**, decided by
    /// the server.
    ///
    /// Carried rather than worked out from `locked_until` by whoever is
    /// drawing, for two reasons. The browser's clock is not authoritative
    /// about whether an account is locked - the server's is, and it is the one
    /// enforcing it. And a view that compares `locked_until` against
    /// `Utc::now()` is read at two different moments on the two sides of
    /// hydration: a lockout expiring in the gap makes the server draw a badge
    /// the browser does not, the two disagree about the node count, and that
    /// is an unrecoverable hydration error rather than a stale badge. See
    /// [`lockout_holds`].
    pub locked: bool,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Whether a lockout ending at `locked_until` still holds at `now`.
///
/// A free function taking the instant, rather than a method reading the clock,
/// so that asking the question requires saying *when* - and so the only place
/// that can answer it is one that has a trustworthy answer to that. The DBAL
/// calls it once per read and stores the result in [`UserListing::locked`].
pub fn lockout_holds(
    locked_until: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    locked_until.is_some_and(|until| until > now)
}

impl UserListing {
    /// Initials for the avatar, matching [`AuthUser::initials`].
    ///
    /// [`AuthUser::initials`]: super::user::AuthUser::initials
    pub fn initials(&self) -> String {
        let mut words = self
            .display_name
            .split_whitespace()
            .filter_map(|word| word.chars().next());

        match (words.next(), words.next()) {
            (Some(first), Some(second)) => format!("{first}{second}").to_uppercase(),
            (Some(first), None) => first.to_uppercase().to_string(),
            _ => self
                .email
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned()),
        }
    }


    /// Whether the text matches this row, for the search box.
    ///
    /// Case-insensitive across the three fields somebody would actually type:
    /// a name, an address, or a role.
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }

        self.display_name.to_lowercase().contains(&needle)
            || self.email.to_lowercase().contains(&needle)
            || self
                .roles
                .iter()
                .any(|role| role.to_lowercase().contains(&needle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> UserListing {
        UserListing {
            id: UserId::nil(),
            email: "ada.lovelace@example.com".into(),
            display_name: "Ada Lovelace".into(),
            status: UserStatus::Active,
            is_owner: false,
            email_verified: true,
            mfa_enabled: false,
            roles: vec!["Admin".into()],
            locked_until: None,
            locked: false,
            last_login_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn initials_come_from_the_display_name() {
        assert_eq!(listing().initials(), "AL");

        let single = UserListing {
            display_name: "Ada".into(),
            ..listing()
        };
        assert_eq!(single.initials(), "A");

        // A display name nobody filled in still produces something to draw.
        let nameless = UserListing {
            display_name: "   ".into(),
            ..listing()
        };
        assert_eq!(nameless.initials(), "A");
    }

    #[test]
    fn search_looks_at_the_three_fields_people_type() {
        let user = listing();

        assert!(user.matches("ada"));
        assert!(user.matches("LOVELACE"));
        assert!(user.matches("example.com"));
        assert!(user.matches("admin"));
        assert!(!user.matches("babbage"));

        // An empty box is not a filter.
        assert!(user.matches(""));
        assert!(user.matches("   "));
    }

    #[test]
    fn a_lockout_expires_on_its_own() {
        let now = chrono::Utc::now();

        assert!(lockout_holds(Some(now + chrono::Duration::minutes(5)), now));
        // Nothing clears the column, so this is the ordinary resting state of
        // any account that was ever locked out.
        assert!(!lockout_holds(Some(now - chrono::Duration::minutes(5)), now));
        assert!(!lockout_holds(None, now));
    }

    #[test]
    fn the_boundary_is_the_instant_it_expires() {
        let now = chrono::Utc::now();

        // Exactly `now` is over. The two sides of a comparison this narrow are
        // why the answer is decided once by the server and carried, rather
        // than recomputed wherever a row is drawn.
        assert!(!lockout_holds(Some(now), now));
    }
}
