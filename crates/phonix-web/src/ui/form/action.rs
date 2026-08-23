//! What a form's buttons do, and what happens after.
//!
//! # Saving is not one of the actions
//!
//! [`FormConfig::submits`](super::FormConfig::submits) owns submission, and the
//! actions here are the *extras* - "Save and add another", "Deactivate",
//! "Send again". That split is deliberate. A primary save has five obligations
//! that a generic callback cannot discharge:
//!
//! * validate before it goes,
//! * know whether anything actually changed,
//! * disable itself while in flight,
//! * put the server's field errors back on the fields they name, and
//! * refuse to submit twice.
//!
//! Modelled as one more entry in a list, each of the five gets re-implemented
//! per form, and the fifth one gets forgotten.
//!
//! # What happens next is data, not a pipeline
//!
//! An action declares what should follow as a list of [`Then`]s, run in order
//! and stopping at the first failure:
//!
//! ```ignore
//! FormAction::submit("Save and add another")
//!     .then(Then::Say("Saved."))
//!     .then(Then::Reset)
//! ```
//!
//! The alternative considered was an observable pipeline in the style of rxjs,
//! composed at runtime. It was rejected for reasons worth writing down, because
//! the idea will come back:
//!
//! * What a form needs is "do this, then that, stop on failure" - which is
//!   `async` and `Result`, already in the language. Observables solve
//!   cancellation, backpressure and multicast, and a form has none of those.
//! * It would be a second scheduler running beside leptos's own reactive graph,
//!   with the two observing each other.
//! * A chain of heterogeneous steps has to erase its types at every join, so
//!   the compiler stops being able to check the thing most likely to be wrong.
//! * Every operator closure would need `Arc` and `'static`, in every
//!   configuration file, forever.
//!
//! A closed enum keeps the declarative shape and none of that: every chain is
//! inspectable data, there is exactly one runner, and the compiler knows the
//! cases. It is also the reversible choice - a variant can be added without
//! touching a call site, while unwinding a runtime pipeline back into data
//! would be a rewrite of every configuration.

use std::sync::Arc;

use leptos::prelude::*;
use phonix_core::identity::AuthUser;

use crate::components::page::Tone;
use crate::icons::Icon;
use crate::ui::alert::Alert;

/// Where to go, built from what was saved.
type Destination<T> = Arc<dyn Fn(&T) -> String + Send + Sync>;

/// Something that happens once an action has succeeded.
///
/// Kept small on purpose. A variant is added when a screen needs it, not when
/// one can be imagined - every entry is a case the runner has to honour and a
/// reader has to know.
pub enum Then<T: 'static> {
    /// Say it worked, in these words, wherever this form reports - which is a
    /// toast unless the configuration said otherwise. See
    /// [`FormConfig::reports`](super::FormConfig::reports).
    Say(&'static str),
    /// Say exactly this, exactly there, whatever the form's own channel is.
    ///
    /// For the action that is louder than the form around it: a delete that
    /// wants acknowledging on a screen whose saves are content to toast.
    Alert(Alert),
    /// Re-read whatever list this form was opened from.
    Refresh,
    /// Put the form back to the values it opened with. For "save and add
    /// another", where staying on a saved draft would invite saving it twice.
    Reset,
    /// Close the modal, or leave the page if this form is not in one.
    Close,
    /// Go somewhere, built from what was stored.
    Navigate(Destination<T>),
}

impl<T: 'static> Clone for Then<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Say(message) => Self::Say(message),
            Self::Alert(alert) => Self::Alert(alert.clone()),
            Self::Refresh => Self::Refresh,
            Self::Reset => Self::Reset,
            Self::Close => Self::Close,
            Self::Navigate(to) => Self::Navigate(Arc::clone(to)),
        }
    }
}

impl<T: 'static> Then<T> {
    /// Report through a channel of this action's choosing.
    pub fn alert(alert: Alert) -> Self {
        Self::Alert(alert)
    }

    /// Go to a destination built from the stored entity.
    pub fn navigate(to: impl Fn(&T) -> String + Send + Sync + 'static) -> Self {
        Self::Navigate(Arc::new(to))
    }
}

/// What pressing an action's button means.
pub enum ActionKind<T: 'static> {
    /// Submit the form, then run the chain. The usual one.
    Submit,
    /// Do something to the draft without submitting it - a "generate" button, a
    /// "copy from billing address".
    Run(Callback<T>),
    /// Leave without submitting.
    Cancel,
}

impl<T: 'static> Clone for ActionKind<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Submit => Self::Submit,
            Self::Run(run) => Self::Run(*run),
            Self::Cancel => Self::Cancel,
        }
    }
}

/// One button at the foot of a form.
pub struct FormAction<T: 'static> {
    pub(crate) label: String,
    pub(crate) icon: Option<Icon>,
    pub(crate) tone: Tone,
    pub(crate) primary: bool,
    pub(crate) permission: Option<&'static str>,
    pub(crate) kind: ActionKind<T>,
    pub(crate) confirm: Option<String>,
    pub(crate) chain: Vec<Then<T>>,
}

impl<T: 'static> Clone for FormAction<T> {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            icon: self.icon,
            tone: self.tone,
            primary: self.primary,
            permission: self.permission,
            kind: self.kind.clone(),
            confirm: self.confirm.clone(),
            chain: self.chain.clone(),
        }
    }
}

impl<T: 'static> FormAction<T> {
    /// The button that saves. A form usually has exactly one.
    pub fn submit(label: impl Into<String>) -> Self {
        Self::of(label, ActionKind::Submit).as_primary()
    }

    /// A button that changes the draft in place without saving it.
    pub fn run(label: impl Into<String>, on_run: impl Fn(T) + Send + Sync + 'static) -> Self {
        Self::of(label, ActionKind::Run(Callback::new(on_run)))
    }

    /// A button that leaves without saving.
    pub fn cancel(label: impl Into<String>) -> Self {
        Self::of(label, ActionKind::Cancel).then(Then::Close)
    }

    fn of(label: impl Into<String>, kind: ActionKind<T>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            tone: Tone::Neutral,
            primary: false,
            permission: None,
            kind,
            confirm: None,
            chain: Vec::new(),
        }
    }

    /// What happens next, in order. Each one runs only if the last succeeded.
    #[must_use]
    pub fn then(mut self, then: Then<T>) -> Self {
        self.chain.push(then);
        self
    }

    /// The permission needed to see this button.
    ///
    /// Cosmetic, exactly as it is on a grid action: this hides a button, and
    /// `Caller::require` inside the service is what refuses the request.
    #[must_use]
    pub const fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    #[must_use]
    pub const fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Draw it as the form's main button.
    #[must_use]
    pub const fn as_primary(mut self) -> Self {
        self.primary = true;
        self
    }

    /// Ask before running. Only for what repeating cannot undo.
    #[must_use]
    pub fn confirm(mut self, question: impl Into<String>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn submits(&self) -> bool {
        matches!(self.kind, ActionKind::Submit)
    }

    /// Whether this viewer may see the button.
    pub fn permitted(&self, user: Option<&AuthUser>) -> bool {
        match self.permission {
            None => true,
            Some(permission) => user.is_some_and(|user| user.can(permission)),
        }
    }

    pub fn chain(&self) -> &[Then<T>] {
        &self.chain
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::PermissionSet;
    use phonix_core::identity::{UserId, UserStatus};

    use super::*;

    fn owned<T>(build: impl FnOnce() -> T) -> T {
        Owner::new().with(build)
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
    fn a_chain_keeps_the_order_it_was_declared_in() {
        // The whole point of the enum over a pipeline: the chain is data, so a
        // test can read it without running anything.
        let action = FormAction::<u8>::submit("Save")
            .then(Then::Say("Saved."))
            .then(Then::Refresh)
            .then(Then::navigate(|id| format!("/thing/{id}")));

        assert_eq!(action.chain().len(), 3);
        assert!(matches!(action.chain()[0], Then::Say("Saved.")));
        assert!(matches!(action.chain()[1], Then::Refresh));
        assert!(matches!(action.chain()[2], Then::Navigate(_)));
    }

    #[test]
    fn a_navigate_builds_its_destination_from_what_was_saved() {
        let action =
            FormAction::<u8>::submit("Save").then(Then::navigate(|id| format!("/thing/{id}")));

        let Then::Navigate(to) = &action.chain()[0] else {
            panic!("expected a navigate");
        };

        assert_eq!(to(&7), "/thing/7");
    }

    #[test]
    fn the_save_button_is_the_primary_one_without_being_told() {
        assert!(FormAction::<u8>::submit("Save").primary);
        assert!(FormAction::<u8>::submit("Save").submits());
    }

    #[test]
    fn cancelling_closes_without_being_told() {
        // A cancel that left the form open would be a button that does nothing,
        // and every form would have to remember to add the Close itself.
        let cancel = FormAction::<u8>::cancel("Cancel");

        assert!(!cancel.submits());
        assert!(matches!(cancel.chain()[0], Then::Close));
    }

    #[test]
    fn a_gated_button_is_hidden_from_a_viewer_without_the_permission() {
        let action = FormAction::<u8>::submit("Save").require(phonix_core::permissions::USERS_EDIT);

        assert!(!action.permitted(Some(&viewer(PermissionSet::new()))));
        assert!(action.permitted(Some(&viewer(PermissionSet::all()))));
        // And while nobody is known yet.
        assert!(!action.permitted(None));
    }

    #[test]
    fn a_run_action_does_not_submit() {
        let action = owned(|| FormAction::<u8>::run("Generate", |_| {}));

        assert!(!action.submits());
    }
}
