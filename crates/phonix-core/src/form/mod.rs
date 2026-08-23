//! Submitting one entity, and what comes back.
//!
//! # The counterpart to [`query`](crate::query)
//!
//! `query` phrases "which rows do I want"; this phrases "here is one, store
//! it". Both live here for the same reason: the browser, the server function
//! and the service all have to agree on the wording, and three layers each
//! inventing their own is three chances to disagree.
//!
//! # A rejected form is an outcome, not an error
//!
//! This is the rule the rest of the codebase already follows - see the note at
//! the top of `phonix_services::error` about a wrong password being an outcome
//! rather than a failure. A form that fails validation is the *expected* path
//! through a form; it happens all day, and modelling it as `Err` has two
//! specific costs:
//!
//! * every caller unwraps something ordinary, and
//! * the field-by-field detail does not survive. `ServerFnError` carries a
//!   string, so `ServiceError::Rejected` collapses to
//!   `"first_name: required, email: already in use"` on the way across the
//!   wire - which a form can print at the top and cannot attach to the two
//!   inputs it is actually about.
//!
//! So a submission returns `Ok(Submission::Rejected(errors))` with the
//! [`FieldError`]s intact, and `Err` stays for the things that really are
//! failures: the caller is not permitted, the database is down.
//!
//! # Saved returns the stored value
//!
//! [`Submission::Saved`] carries the entity as it now stands rather than `()`,
//! so the screen re-renders from what was actually written - including
//! whatever the server normalised, defaulted or declined to change. The
//! permissions editor already works this way.

use serde::{Deserialize, Serialize};

use crate::i18n::Message;
use crate::identity::validation::FieldError;

/// What happened to a submitted form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Submission<T> {
    /// Stored. Carries the entity as it now stands.
    Saved(T),
    /// Not stored, and here is why, field by field.
    Rejected(Vec<FieldError>),
}

impl<T> Submission<T> {
    /// One rejected field, for the common single-problem case.
    pub fn rejected(field: impl Into<String>, message: Message) -> Self {
        Self::Rejected(vec![FieldError::new(field, message)])
    }

    pub const fn is_saved(&self) -> bool {
        matches!(self, Self::Saved(_))
    }

    /// The stored entity, if it was stored.
    pub fn saved(self) -> Option<T> {
        match self {
            Self::Saved(value) => Some(value),
            Self::Rejected(_) => None,
        }
    }

    /// The problems, or an empty slice when there were none.
    pub fn errors(&self) -> &[FieldError] {
        match self {
            Self::Saved(_) => &[],
            Self::Rejected(errors) => errors,
        }
    }

    /// The message for one field, if that field has one.
    ///
    /// The [`Message`], not its words: the caller is the one that knows which
    /// catalog to resolve against.
    pub fn error_for(&self, field: &str) -> Option<&Message> {
        self.errors()
            .iter()
            .find(|error| error.field == field)
            .map(|error| &error.message)
    }

    /// Rejections that name no field of the form - a message about the whole
    /// submission, or one naming a field this screen does not show.
    ///
    /// A form that only rendered per-field messages would swallow those
    /// silently, which looks like a save button that does nothing.
    pub fn unattached<'a>(&'a self, fields: &[&str]) -> Vec<&'a FieldError> {
        self.errors()
            .iter()
            .filter(|error| !fields.contains(&error.field.as_str()))
            .collect()
    }

    /// The same outcome with the stored entity converted.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Submission<U> {
        match self {
            Self::Saved(value) => Submission::Saved(f(value)),
            Self::Rejected(errors) => Submission::Rejected(errors),
        }
    }
}

/// Collapse a validation result into a submission.
///
/// Lets a service write `reject_if(errors)?` style flow as one line:
///
/// ```ignore
/// if let Some(rejected) = rejected(draft.validate()) {
///     return Ok(rejected);
/// }
/// ```
pub fn rejected<T>(errors: Vec<FieldError>) -> Option<Submission<T>> {
    (!errors.is_empty()).then_some(Submission::Rejected(errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejection() -> Submission<u8> {
        Submission::Rejected(vec![
            FieldError::new("email", crate::msg!("validation.email.required")),
            FieldError::new(
                "secret_handshake",
                crate::msg!("validation.form.uncheckable"),
            ),
        ])
    }

    #[test]
    fn a_saved_submission_carries_what_was_stored() {
        let saved = Submission::Saved(7u8);

        assert!(saved.is_saved());
        assert!(saved.errors().is_empty());
        assert_eq!(saved.saved(), Some(7));
    }

    #[test]
    fn a_rejection_can_be_asked_about_one_field() {
        assert_eq!(
            rejection().error_for("email").map(|m| m.key.as_str()),
            Some("validation.email.required")
        );
        assert_eq!(rejection().error_for("first_name"), None);
    }

    #[test]
    fn a_rejection_naming_a_field_the_screen_does_not_show_is_not_lost() {
        // Otherwise the form prints nothing and looks like a dead button.
        let rejection = rejection();
        let unattached = rejection.unattached(&["email"]);

        assert_eq!(unattached.len(), 1);
        assert_eq!(unattached[0].field, "secret_handshake");
    }

    #[test]
    fn nothing_is_unattached_when_the_form_shows_every_named_field() {
        let submission =
            Submission::<u8>::rejected("email", crate::msg!("validation.email.required"));

        assert!(submission.unattached(&["email"]).is_empty());
    }

    #[test]
    fn a_saved_value_can_be_converted_without_losing_the_outcome() {
        assert_eq!(
            Submission::Saved(2u8).map(u32::from),
            Submission::Saved(2u32)
        );
        assert!(!rejection().map(u32::from).is_saved());
    }

    #[test]
    fn no_errors_is_not_a_rejection() {
        assert!(rejected::<u8>(Vec::new()).is_none());
        assert!(
            rejected::<u8>(vec![FieldError::new(
                "a",
                crate::msg!("validation.email.required")
            )])
            .is_some()
        );
    }
}
