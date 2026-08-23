//! What the person has done to the form since it opened.
//!
//! # The five obligations of a save button
//!
//! All of them live here rather than in each form, because a form that gets
//! four of them right looks exactly like one that gets five right, until the
//! day it does not:
//!
//! * **validate** before sending - [`FormState::check`]
//! * **dirty** - [`FormState::is_dirty`], so "Save" on an untouched form is
//!   not a write, and leaving one is not a warning
//! * **in flight** - [`FormState::is_sending`], which disables the button
//! * **field errors** back on the fields that were named -
//!   [`FormState::error_for`]
//! * **never twice** - [`FormState::begin`] refuses to start a second send
//!
//! # The draft is one signal, not one per field
//!
//! A signal per field would mean the draft is assembled at submit time out of
//! however many controls happen to exist, and a field added without being
//! wired in would submit its default silently. One draft signal, written
//! through each field's own writer, means the thing being edited is always the
//! thing that will be sent.

use leptos::prelude::*;
use phonix_core::identity::AuthUser;
use phonix_core::identity::validation::FieldError;

use super::config::FormConfig;
use super::value::FieldValue;

/// The live state of one form on screen.
pub struct FormState<T: Send + Sync + 'static> {
    /// What will be sent.
    pub draft: RwSignal<T>,
    /// What it looked like when the form opened, or when it was last saved.
    /// Only ever compared against - never sent.
    pub original: RwSignal<T>,
    pub errors: RwSignal<Vec<FieldError>>,
    /// A sentence about the whole form: what the server said when the request
    /// itself failed, or what a successful save reported.
    pub notice: RwSignal<Option<(String, bool)>>,
    sending: RwSignal<bool>,
}

// Copy by hand rather than derived: `#[derive(Copy)]` would demand `T: Copy`,
// which no draft is. Every field here is a copyable arena handle whatever `T`
// happens to be.
impl<T: Send + Sync + 'static> Clone for FormState<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for FormState<T> {}

impl<T: Clone + PartialEq + Send + Sync + 'static> FormState<T> {
    /// Opened on `value`.
    pub fn new(value: T) -> Self {
        Self {
            draft: RwSignal::new(value.clone()),
            original: RwSignal::new(value),
            errors: RwSignal::new(Vec::new()),
            notice: RwSignal::new(None),
            sending: RwSignal::new(false),
        }
    }

    /// Whether anything has been changed since the form opened or last saved.
    pub fn is_dirty(&self) -> bool {
        self.draft
            .with(|draft| self.original.with(|original| draft != original))
    }

    pub fn is_sending(&self) -> bool {
        self.sending.get()
    }

    /// Read one field out of the draft.
    pub fn value_of(&self, read: impl Fn(&T) -> FieldValue) -> FieldValue {
        self.draft.with(read)
    }

    /// Write one field into the draft, and take down that field's error.
    ///
    /// Clearing on edit rather than on the next submit is what stops a form
    /// showing "required" under a box somebody has just filled in.
    pub fn edit(&self, field: &'static str, write: impl FnOnce(&mut T)) {
        self.draft.update(write);
        self.errors
            .update(|errors| errors.retain(|error| error.field != field));
    }

    /// The message against one field, if it has one.
    pub fn error_for(&self, field: &'static str) -> Option<String> {
        self.errors.with(|errors| {
            errors
                .iter()
                .find(|error| error.field == field)
                .map(|error| crate::i18n::t(&error.message))
        })
    }

    /// Messages naming fields this form does not show.
    ///
    /// Rendered above the form. Without this they would be dropped, and a save
    /// that was refused for a reason the screen cannot place would look like a
    /// button that does nothing.
    pub fn unplaced(&self, fields: &[&'static str]) -> Vec<String> {
        self.errors.with(|errors| {
            errors
                .iter()
                .filter(|error| !fields.contains(&error.field.as_str()))
                .map(|error| crate::i18n::t(&error.message))
                .collect()
        })
    }

    /// Check what the browser can check, and record it.
    ///
    /// Returns whether it is worth sending. A courtesy only: the service
    /// applies the same rules and is what refuses.
    pub fn check(&self, config: &FormConfig<T>, user: Option<&AuthUser>) -> bool {
        let missing = self.draft.with(|draft| config.missing(draft, user));
        let ok = missing.is_empty();

        self.errors.set(missing);
        ok
    }

    /// Claim the right to send, or report that a send is already under way.
    ///
    /// The double-submit guard. A disabled button is not one: a form can be
    /// submitted with the keyboard between the click and the re-render.
    pub fn begin(&self) -> bool {
        if self.sending.get_untracked() {
            return false;
        }

        self.sending.set(true);
        self.notice.set(None);
        true
    }

    pub fn finish(&self) {
        self.sending.set(false);
    }

    /// Accept what the server stored: the draft becomes it, and so does the
    /// baseline, so the form is no longer dirty.
    pub fn accept(&self, saved: T) {
        self.draft.set(saved.clone());
        self.original.set(saved);
        self.errors.set(Vec::new());
    }

    /// Put the server's rejection on the fields it names.
    pub fn reject(&self, errors: Vec<FieldError>) {
        self.errors.set(errors);
    }

    /// Back to what the form opened with.
    pub fn reset(&self) {
        self.draft.set(self.original.get_untracked());
        self.errors.set(Vec::new());
    }

    pub fn say(&self, message: impl Into<String>) {
        self.notice.set(Some((message.into(), true)));
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.notice.set(Some((message.into(), false)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The *whole* test runs inside an owner, not just the construction.
    ///
    /// A signal is allocated in the arena belonging to the current owner, and
    /// with `sandboxed-arenas` reading one outside that owner panics rather
    /// than returning a stale value. Building the state inside and asserting
    /// outside - which is enough for the grid's tests, because those only
    /// allocate a `Callback` and never read it back - leaves every assert here
    /// reaching into an arena that is no longer active.
    fn owned(body: impl FnOnce()) {
        Owner::new().with(body);
    }

    #[test]
    fn a_form_opens_clean() {
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            assert!(!state.is_dirty());
            assert!(!state.is_sending());
        });
    }

    #[test]
    fn editing_makes_it_dirty_and_saving_makes_it_clean_again() {
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            state.edit("name", |draft| *draft = "Grace".to_owned());
            assert!(state.is_dirty());

            state.accept("Grace".to_owned());
            assert!(!state.is_dirty());
        });
    }

    #[test]
    fn editing_back_to_where_it_started_is_not_dirty() {
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            state.edit("name", |draft| *draft = "Grace".to_owned());
            state.edit("name", |draft| *draft = "Ada".to_owned());

            assert!(!state.is_dirty());
        });
    }

    #[test]
    fn a_second_send_is_refused_while_the_first_is_in_flight() {
        // The guard that a disabled button is not: a keyboard submit can land
        // between the click and the re-render.
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            assert!(state.begin());
            assert!(!state.begin());

            state.finish();
            assert!(state.begin());
        });
    }

    #[test]
    fn editing_a_field_takes_down_that_fields_message_and_no_other() {
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            state.reject(vec![
                FieldError::new(
                    "name",
                    phonix_core::msg!("validation.field.required", label = "Name"),
                ),
                FieldError::new("email", phonix_core::msg!("error.email.taken")),
            ]);

            state.edit("name", |draft| *draft = "Grace".to_owned());

            assert_eq!(state.error_for("name"), None);
            assert_eq!(
                state.error_for("email").as_deref(),
                Some("Somebody already has that address.")
            );
        });
    }

    #[test]
    fn a_rejection_naming_a_field_the_form_does_not_show_is_surfaced_anyway() {
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            state.reject(vec![FieldError::new(
                "tenant",
                phonix_core::msg!("error.email.taken"),
            )]);

            assert_eq!(
                state.unplaced(&["name"]),
                ["Somebody already has that address."]
            );
        });
    }

    #[test]
    fn resetting_returns_to_what_the_form_opened_with() {
        owned(|| {
            let state = FormState::new(String::from("Ada"));

            state.edit("name", |draft| *draft = "Grace".to_owned());
            state.reject(vec![FieldError::new(
                "name",
                phonix_core::msg!("validation.field.required", label = "Name"),
            )]);
            state.reset();

            assert_eq!(state.draft.get_untracked(), "Ada");
            assert!(!state.is_dirty());
            assert_eq!(state.error_for("name"), None);
        });
    }

    #[test]
    fn accepting_what_the_server_stored_rather_than_what_was_typed() {
        // The server may normalise - trim, lowercase an address, drop a role it
        // declined to grant. The form has to show what was actually written.
        owned(|| {
            let state = FormState::new(String::from("ada"));

            state.edit("name", |draft| *draft = "  ADA  ".to_owned());
            state.accept("ada".to_owned());

            assert_eq!(state.draft.get_untracked(), "ada");
            assert!(!state.is_dirty());
        });
    }
}
