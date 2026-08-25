//! What a form is told, and what each entity tells it.
//!
//! # The shape of a configuration
//!
//! [`FormConfig`] is the whole contract between a module and the form, and it
//! is built the same way a [`GridConfig`](crate::ui::table::GridConfig) is - a
//! chain of `with`-style methods that reads down the page as a description of
//! the screen:
//!
//! ```ignore
//! FormConfig::new("user", update_user)
//!     .field(Field::text("first_name", "First name", ..).writing(..).required())
//!     .field(Field::select("status", "Status", statuses(), ..).writing(..))
//!     .action(FormAction::submit("Save").then(Then::Say("Saved.")).then(Then::Refresh))
//! ```
//!
//! # Entity configurations live under this module
//!
//! One file per entity, named for it, beside its grid:
//!
//! ```text
//! ui/table/config/users.rs  ->  pub fn users_grid()      -> GridConfig<UserListing>
//! ui/form/config/users.rs   ->  pub fn user_form(..)     -> FormConfig<UserEdit>
//! ```
//!
//! # A field is not a column, and this is why they are not derived
//!
//! It is tempting to build a form out of the grid's columns - the entity is
//! already described there, after all. It does not work, and the reason is
//! worth stating once:
//!
//! A column reads a row into a [`Cell`](crate::ui::table::Cell), which is
//! **one-way and lossy**. `Cell::Text("Active")` cannot become a `UserStatus`
//! again; a status column that renders a badge would have to be un-rendered; a
//! column has no notion of required, of a placeholder, of which permission may
//! change it, or of what the set of legal values even is.
//!
//! So the two are siblings that **share the entity and the field identifiers**.
//! That is the part worth sharing: a `FieldError` from the service names a
//! field, the form places the message by that name, and the column uses the
//! same one - so a test can assert that every field of a form names a real
//! column, which is the cheap way to catch a rename that only went halfway.

pub mod invitations;
pub mod mail;
pub mod organization;
pub mod parties;
pub mod roles;
pub mod taxes;
pub mod users;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use phonix_core::form::Submission;
use phonix_core::identity::AuthUser;
use phonix_core::identity::validation::FieldError;

use super::action::FormAction;
use super::field::Field;
use crate::ui::alert::Channel;

/// A submission in flight. `String` is the error, for the same reason the grid
/// flattens its errors: a form can show a sentence, and anything richer would
/// tie this module to one error type.
pub type Submitting<T> = Pin<Box<dyn Future<Output = Result<Submission<T>, String>> + Send>>;

type Submit<T> = Arc<dyn Fn(T) -> Submitting<T> + Send + Sync>;

/// Everything one form needs to know.
///
/// Cheap to clone - every closure is behind an `Arc` - so a screen can build
/// one per render without thinking about it.
pub struct FormConfig<T: 'static> {
    /// A stable name, used for the ids that tie labels, controls and their
    /// error messages together for a screen reader.
    pub(crate) id: &'static str,
    pub(crate) fields: Vec<Field<T>>,
    pub(crate) actions: Vec<FormAction<T>>,
    pub(crate) submit: Submit<T>,
    /// Shown above the fields. For the rule that applies to the whole form
    /// rather than to one control.
    /// A line above the fields. A sentence, so a `String` from the catalog.
    pub(crate) note: Option<String>,
    /// Two columns above `sm`, or one all the way up. One column suits a short
    /// form and a modal; two suit an edit page with a dozen fields.
    pub(crate) columns: u8,
    /// Where this form's outcomes are shown - both what a save reports and
    /// what the server said when one failed.
    pub(crate) reports: Channel,
}

impl<T: 'static> Clone for FormConfig<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            fields: self.fields.clone(),
            actions: self.actions.clone(),
            submit: Arc::clone(&self.submit),
            note: self.note.clone(),
            columns: self.columns,
            reports: self.reports,
        }
    }
}

impl<T: 'static> FormConfig<T> {
    /// A form with no fields yet, submitting through `submit`.
    ///
    /// The submitter is usually a server function. It returns a
    /// [`Submission`], not a `Result`, because a form that fails validation is
    /// an outcome rather than an error - see [`phonix_core::form`]. `Err` is
    /// for the request failing: not permitted, or the database is down.
    pub fn new<F, Fut, E>(id: &'static str, submit: F) -> Self
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Submission<T>, E>> + Send + 'static,
        E: std::fmt::Display + 'static,
    {
        Self {
            id,
            fields: Vec::new(),
            actions: Vec::new(),
            submit: Arc::new(move |draft| {
                let sending = submit(draft);

                Box::pin(async move { sending.await.map_err(|err| err.to_string()) })
            }),
            note: None,
            columns: 2,
            reports: Channel::default(),
        }
    }

    /// Add a field. Order here is order on screen.
    #[must_use]
    pub fn field(mut self, field: Field<T>) -> Self {
        self.fields.push(field);
        self
    }

    /// Add a button. A form with none gets a plain "Save" - see
    /// [`Self::buttons`].
    #[must_use]
    pub fn action(mut self, action: FormAction<T>) -> Self {
        self.actions.push(action);
        self
    }

    /// A line above the fields, about the whole form.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Lay the fields out in one column rather than two. For a modal, and for
    /// any form short enough that two columns would leave one of them empty.
    #[must_use]
    pub const fn single_column(mut self) -> Self {
        self.columns = 1;
        self
    }

    /// Where this form's outcomes appear.
    ///
    /// A toast unless told otherwise, because by the time a save comes back the
    /// person is usually looking at the next field or the next tab rather than
    /// at the top of the form. `Channel::Inline` is right for a form short
    /// enough to be on screen whole; `Channel::MessageBox` for one whose result
    /// must be acknowledged before anything else. See [`crate::ui::alert`].
    ///
    /// This governs the *whole* form - a failure is reported the same way a
    /// success is, so a screen cannot give good news a home and leave bad news
    /// with nowhere to go. One action can still overrule it with
    /// [`Then::Alert`](crate::ui::form::Then::Alert).
    #[must_use]
    pub const fn reports(mut self, channel: Channel) -> Self {
        self.reports = channel;
        self
    }

    /// Where this form's outcomes appear.
    pub const fn channel(&self) -> Channel {
        self.reports
    }

    pub const fn id(&self) -> &'static str {
        self.id
    }

    pub fn fields(&self) -> &[Field<T>] {
        &self.fields
    }

    /// The buttons to draw, which is the configured ones or a default save.
    ///
    /// A form with no configured action still has to be submittable, and the
    /// alternative - every configuration repeating `FormAction::submit("Save")`
    /// - is a line that is only ever wrong by being missing.
    pub fn buttons(&self) -> Vec<FormAction<T>> {
        if self.actions.is_empty() {
            vec![FormAction::submit("Save")]
        } else {
            self.actions.clone()
        }
    }

    /// The names of every field, for matching a rejection against them.
    pub fn field_names(&self) -> Vec<&'static str> {
        self.fields.iter().map(Field::name).collect()
    }

    /// Send `draft` to wherever this form submits.
    pub fn send(&self, draft: T) -> Submitting<T> {
        (self.submit)(draft)
    }

    /// What is missing, before anything is sent.
    ///
    /// A courtesy, and never the control. The service validates the same rules
    /// and is what actually refuses - this only saves a round trip and puts the
    /// message next to the control while the person is still looking at it.
    ///
    /// Only fields this viewer can actually edit are checked: demanding a value
    /// in a box somebody is not allowed to type into would be a form that
    /// cannot be submitted at all.
    pub fn missing(&self, draft: &T, user: Option<&AuthUser>) -> Vec<FieldError> {
        self.fields
            .iter()
            .filter(|field| field.required && field.applies_to(draft))
            .filter(|field| field.editable_by(user))
            .filter(|field| !field.value(draft).is_present())
            // The label is still English: `Field::label` is a `&'static str`
            // on the form's config, and keying those is the next sweep. Until
            // then this sentence is translated around an untranslated noun,
            // which is the honest halfway house - see `phonix_core::i18n`.
            .map(|field| {
                FieldError::new(
                    field.field,
                    phonix_core::msg!("validation.field.required", label = field.label.clone()),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::PermissionSet;
    use phonix_core::identity::{UserId, UserStatus};

    use super::super::action::Then;
    use super::super::value::FieldValue;
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Draft {
        name: String,
        note: String,
    }

    fn draft(name: &str) -> Draft {
        Draft {
            name: name.to_owned(),
            note: String::new(),
        }
    }

    fn config() -> FormConfig<Draft> {
        FormConfig::new("test", |draft: Draft| async move {
            Ok::<_, String>(Submission::Saved(draft))
        })
        .field(
            Field::text("name", "Name", |d: &Draft| FieldValue::text(&d.name))
                .writing(|d, value| d.name = value.as_input())
                .required(),
        )
        .field(
            Field::text("note", "Note", |d: &Draft| FieldValue::text(&d.note))
                .writing(|d, value| d.note = value.as_input()),
        )
    }

    fn viewer(permissions: PermissionSet) -> AuthUser {
        AuthUser {
            id: UserId::nil(),
            email: "viewer@example.test".to_owned(),
            first_name: "V".to_owned(),
            last_name: "Iewer".to_owned(),
            display_name: "V Iewer".to_owned(),
            roles: Vec::new(),
            permissions,
            is_owner: false,
            status: UserStatus::Active,
            mfa_satisfied: true,
            mfa_enabled: false,
            email_verified: true,
        }
    }

    #[test]
    fn a_required_field_left_empty_is_reported_against_that_field() {
        let missing = config().missing(&draft("  "), None);

        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].field, "name");
        assert_eq!(missing[0].message.to_string(), "Name is required.");
    }

    #[test]
    fn a_filled_in_form_is_missing_nothing() {
        assert!(config().missing(&draft("Ada"), None).is_empty());
    }

    #[test]
    fn a_field_the_viewer_cannot_edit_is_not_demanded_of_them() {
        // Otherwise the form cannot be submitted at all: the message names a
        // box the person is not allowed to type into.
        let config = config().field(
            Field::text("secret", "Secret", |_: &Draft| FieldValue::text(""))
                .writing(|_, _| {})
                .required()
                .require(phonix_core::permissions::USERS_EDIT),
        );

        assert!(
            config
                .missing(&draft("Ada"), Some(&viewer(PermissionSet::new())))
                .is_empty()
        );
        assert_eq!(
            config
                .missing(&draft("Ada"), Some(&viewer(PermissionSet::all())))
                .len(),
            1
        );
    }

    #[test]
    fn a_field_that_does_not_apply_is_not_demanded_either() {
        let config = FormConfig::new("test", |d: Draft| async move {
            Ok::<_, String>(Submission::Saved(d))
        })
        .field(
            Field::text("note", "Note", |d: &Draft| FieldValue::text(&d.note))
                .writing(|d, value| d.note = value.as_input())
                .required()
                .when(|d: &Draft| d.name == "Ada"),
        );

        assert!(config.missing(&draft("Grace"), None).is_empty());
        assert_eq!(config.missing(&draft("Ada"), None).len(), 1);
    }

    #[test]
    fn a_form_with_no_configured_action_is_still_submittable() {
        let buttons = config().buttons();

        assert_eq!(buttons.len(), 1);
        assert!(buttons[0].submits());
    }

    #[test]
    fn configured_actions_replace_the_default_rather_than_joining_it() {
        // Two save buttons, one of them unasked for, would be the alternative.
        let config = config()
            .action(FormAction::submit("Save and close").then(Then::Close))
            .action(FormAction::cancel("Cancel"));

        let buttons = config.buttons();
        let labels: Vec<&str> = buttons.iter().map(FormAction::label).collect();

        assert_eq!(labels, ["Save and close", "Cancel"]);
    }

    #[test]
    fn the_field_names_are_what_a_rejection_is_matched_against() {
        assert_eq!(config().field_names(), ["name", "note"]);
    }
}
