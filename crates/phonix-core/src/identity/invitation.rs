//! Adding somebody to a workspace.
//!
//! # An administrator never types somebody else's password
//!
//! Creating an account issues an invitation and emails a link. The account
//! exists immediately, with no password and status `Pending`, and becomes
//! usable when the person accepts.
//!
//! The alternative - an administrator setting a temporary password - was
//! rejected for two reasons that outlive any convenience it buys:
//!
//! * it is a working credential that somebody other than the account holder
//!   knows, travelling over a channel nobody controls; and
//! * it makes the audit trail unable to tell "the user signed in" from "the
//!   administrator signed in as them", which quietly undercuts the MFA policy
//!   and every question the trail exists to answer.
//!
//! # What an invitation asks for, and what it does not
//!
//! Email, name and roles. Not a password, obviously, and not a status: an
//! invited account is `Pending` by definition and choosing otherwise would mean
//! creating an active account nobody can sign in to.
//!
//! The address is the one field that cannot be corrected later without an
//! account-recovery flow - see [`UserEdit`](super::UserEdit) - which is why it
//! is validated here rather than left to the relay to bounce.

use serde::{Deserialize, Serialize};

use super::user::UserId;
use super::validation::{FieldError, validate_email, validate_person_name};

/// Somebody an administrator is adding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInvite {
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    /// Role *names*, matching [`UserEdit`](super::UserEdit) and the listing, so
    /// one spelling travels the whole way.
    ///
    /// Empty is legitimate: an account with no role can sign in and do nothing,
    /// which is a reasonable place to start somebody whose access is being
    /// decided.
    pub roles: Vec<String>,
}

impl UserInvite {
    /// A blank invitation, for a form to open on.
    pub fn blank() -> Self {
        Self {
            email: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            roles: Vec::new(),
        }
    }

    /// Check what can be checked without touching the database.
    ///
    /// Every problem rather than the first, so somebody filling in a form is
    /// not sent round the loop once per field. Whether the address is already
    /// taken is not answerable here and comes back from the service.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();

        if let Err(err) = validate_email(&self.email) {
            errors.push(err);
        }
        if let Err(err) = validate_person_name("first_name", &self.first_name) {
            errors.push(err);
        }
        if let Err(err) = validate_person_name("last_name", &self.last_name) {
            errors.push(err);
        }

        errors
    }

    /// The address as it should be stored: trimmed and lowercased.
    ///
    /// Addresses are compared case-insensitively everywhere else in this
    /// codebase, so storing the case somebody typed would make the stored value
    /// disagree with every lookup that finds it.
    pub fn normalised_email(&self) -> String {
        self.email.trim().to_lowercase()
    }

    /// The display name, derived once rather than at each call site.
    pub fn display_name(&self) -> String {
        format!("{} {}", self.first_name.trim(), self.last_name.trim())
            .trim()
            .to_owned()
    }
}

/// What came back from issuing an invitation.
///
/// # The link is carried, and shown once
///
/// An invitation link signs somebody in, so it is a credential. It is returned
/// here because the screen has to be able to show it: when no relay is
/// configured, or when the relay refused, the only way the invitation reaches
/// anybody is by hand.
///
/// It is shown **once**, at the moment of creation, and never redisplayed.
/// Re-issuing is the recovery, and superseding the outstanding token is what
/// makes that safe - see `one_time_token::issue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationIssued {
    pub user_id: UserId,
    pub email: String,
    pub display_name: String,
    /// Absolute, single-use, and the one time it is available.
    pub link: String,
    /// Hours from now.
    pub expires_in_hours: i64,
    /// What happened to the email, if there is anything to say. `None` means it
    /// was delivered and there is nothing worth reporting.
    pub delivery_note: Option<String>,
}

impl InvitationIssued {
    /// Whether the person will actually receive this without help.
    ///
    /// Drives whether the screen presents the link as the main thing or as a
    /// fallback worth copying.
    pub const fn was_emailed(&self) -> bool {
        self.delivery_note.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> UserInvite {
        UserInvite {
            email: "ada@example.com".to_owned(),
            first_name: "Ada".to_owned(),
            last_name: "Lovelace".to_owned(),
            roles: vec!["Admin".to_owned()],
        }
    }

    #[test]
    fn a_complete_invitation_has_nothing_to_say() {
        assert!(invite().validate().is_empty());
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let broken = UserInvite {
            email: "not-an-address".to_owned(),
            first_name: String::new(),
            last_name: String::new(),
            roles: Vec::new(),
        };

        let fields: Vec<String> = broken
            .validate()
            .into_iter()
            .map(|error| error.field)
            .collect();

        assert!(fields.contains(&"email".to_owned()));
        assert!(fields.contains(&"first_name".to_owned()));
        assert!(fields.contains(&"last_name".to_owned()));
    }

    #[test]
    fn no_role_is_a_legitimate_invitation() {
        // An account that can sign in and do nothing is a reasonable place to
        // start somebody whose access has not been decided yet.
        let roleless = UserInvite {
            roles: Vec::new(),
            ..invite()
        };

        assert!(roleless.validate().is_empty());
    }

    #[test]
    fn the_address_is_stored_the_way_every_lookup_will_search_for_it() {
        // Addresses are compared case-insensitively everywhere else, so storing
        // what somebody typed would disagree with the query that finds it.
        let shouty = UserInvite {
            email: "  Ada@Example.COM ".to_owned(),
            ..invite()
        };

        assert_eq!(shouty.normalised_email(), "ada@example.com");
    }

    #[test]
    fn the_display_name_is_derived_once_and_trimmed() {
        let padded = UserInvite {
            first_name: "  Ada ".to_owned(),
            last_name: " Lovelace ".to_owned(),
            ..invite()
        };

        assert_eq!(padded.display_name(), "Ada Lovelace");
    }

    #[test]
    fn an_issued_invitation_says_whether_it_actually_reached_anybody() {
        let sent = InvitationIssued {
            user_id: UserId::nil(),
            email: "ada@example.com".to_owned(),
            display_name: "Ada Lovelace".to_owned(),
            link: "https://acme.test/invitations/abc".to_owned(),
            expires_in_hours: 72,
            delivery_note: None,
        };

        assert!(sent.was_emailed());

        let undelivered = InvitationIssued {
            delivery_note: Some("No mail relay is configured.".to_owned()),
            ..sent
        };

        assert!(!undelivered.was_emailed());
    }
}
