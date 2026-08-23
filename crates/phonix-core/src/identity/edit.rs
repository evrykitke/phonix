//! What an administrator may change about somebody else's account.
//!
//! # A draft is not a listing
//!
//! [`UserListing`](super::UserListing) is what a row shows: name, status,
//! whether a second factor is enrolled, when they last signed in. Most of that
//! is *observed* - nobody edits "last sign-in" - and the parts that are
//! editable are shown in a rendered form rather than a storable one.
//!
//! So the thing a form edits is its own type, holding exactly the fields that
//! may be written and nothing else. That is not ceremony: it is what makes
//! "which fields can this screen change" answerable by reading a struct, and it
//! is why a request cannot smuggle in a change to `is_owner` or `mfa_enabled`
//! by putting one in the payload.
//!
//! # What is deliberately absent
//!
//! * **email** - changing the address somebody signs in with is an account
//!   recovery flow, not a text box. It needs the new address verified before
//!   the old one stops working, or a typo locks them out permanently.
//! * **password** - an administrator never sets one; see the invitation flow.
//! * **is_owner** - exactly one per workspace, enforced by the schema.
//! * **permissions** - the permission editor is its own screen, because the
//!   effective set is computed from roles plus overrides and a flat list of
//!   tick boxes would misrepresent it.

use serde::{Deserialize, Serialize};

use super::UserStatus;
use super::user::UserId;
use super::validation::{FieldError, validate_person_name};

/// The editable part of an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserEdit {
    pub id: UserId,
    pub first_name: String,
    pub last_name: String,
    pub status: UserStatus,
    /// Role *names*, not ids - the same strings the listing shows and the
    /// grid searches, so a form and a row cannot disagree about what somebody
    /// holds.
    pub roles: Vec<String>,
}

impl UserEdit {
    /// Check what can be checked without touching the database.
    ///
    /// Returns every problem rather than the first, so somebody fixing a form
    /// is not sent round the loop once per field.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();

        if let Err(err) = validate_person_name("first_name", &self.first_name) {
            errors.push(err);
        }
        if let Err(err) = validate_person_name("last_name", &self.last_name) {
            errors.push(err);
        }

        errors
    }

    /// The names as they should be stored: trimmed, with the display name
    /// derived once rather than at each call site.
    pub fn display_name(&self) -> String {
        format!("{} {}", self.first_name.trim(), self.last_name.trim())
            .trim()
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(first: &str, last: &str) -> UserEdit {
        UserEdit {
            id: UserId::nil(),
            first_name: first.to_owned(),
            last_name: last.to_owned(),
            status: UserStatus::Active,
            roles: Vec::new(),
        }
    }

    #[test]
    fn a_valid_edit_has_nothing_to_say() {
        assert!(edit("Ada", "Lovelace").validate().is_empty());
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        // Not the first one: otherwise fixing a form is one round trip per
        // field, and each one looks like a new failure.
        let errors = edit("", "").validate();

        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].field, "first_name");
        assert_eq!(errors[1].field, "last_name");
    }

    #[test]
    fn a_field_error_names_the_field_the_form_calls_it() {
        // The form places the message by matching this against its own field
        // names, so the spelling is a contract rather than a label.
        let errors = edit(" ", "Lovelace").validate();

        assert_eq!(errors[0].field, "first_name");
    }

    #[test]
    fn the_display_name_is_derived_once_and_trimmed() {
        assert_eq!(edit("  Ada ", " Lovelace ").display_name(), "Ada Lovelace");
    }

    #[test]
    fn a_missing_half_does_not_leave_a_dangling_space() {
        assert_eq!(edit("Ada", "").display_name(), "Ada");
    }
}
