//! Asking before doing.
//!
//! # Why this replaced `window.confirm`
//!
//! The browser's own dialog was doing this job, and it was one call rather than
//! a component, which is a real argument in its favour. What it could not do:
//!
//! * **It looks like the browser, not like the application.** Chrome's grey
//!   sheet at the top of the window in the middle of a dark-themed page is the
//!   one piece of the interface that says "this is a web page" out loud.
//! * **It cannot be styled at all** - no tone, no icon, no name for the button.
//!   Every confirmation reads "OK / Cancel", so the button that deletes a role
//!   and the button that resends an invitation are the same button.
//! * **Some browsers suppress it**, and a suppressed `confirm` returns `false`
//!   silently: the action does not happen and nothing says why.
//!
//! # The shape it forces
//!
//! `window.confirm` blocks and returns a `bool`, so a call site could be
//! written as `if confirmed(question) { do_it() }`. A dialog cannot block, so
//! the deed moves into a callback and the call site inverts:
//!
//! ```ignore
//! match confirm {
//!     None => go(),
//!     Some(question) => alerts.ask(Confirm::new(question, go).titled(label)),
//! }
//! ```
//!
//! That is the whole cost of the change, and it is paid once in each of the two
//! places that had a confirmation: a form's action button and a grid's row
//! action.

use leptos::prelude::*;

use crate::components::page::Tone;

/// A question, and what to do if the answer is yes.
///
/// There is no "if no" - declining is not an event anything needs to hear
/// about, and a callback for it would be an empty closure at every call site.
#[derive(Clone)]
pub struct Confirm {
    /// The heading. `None` reads "Are you sure?".
    pub title: Option<String>,
    pub question: String,
    /// The word on the button that goes ahead. Worth setting: "Delete" tells
    /// somebody what they are about to do in the place they are looking.
    pub confirm_label: String,
    pub tone: Tone,
    pub on_confirm: Callback<()>,
}

impl Confirm {
    /// Ask `question`, and run `on_confirm` if the answer is yes.
    ///
    /// Danger by default. A confirmation is asked for when repeating the action
    /// cannot undo it, and anything gentler than that did not need asking.
    pub fn new(question: impl Into<String>, on_confirm: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            title: None,
            question: question.into(),
            confirm_label: "Confirm".to_owned(),
            tone: Tone::Danger,
            on_confirm: Callback::new(move |()| on_confirm()),
        }
    }

    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The word on the button that goes ahead.
    #[must_use]
    pub fn confirm_label(mut self, label: impl Into<String>) -> Self {
        self.confirm_label = label.into();
        self
    }

    #[must_use]
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// The heading to draw, which is the one set or the house question.
    pub fn heading(&self) -> &str {
        self.title.as_deref().unwrap_or("Are you sure?")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned<T>(build: impl FnOnce() -> T) -> T {
        Owner::new().with(build)
    }

    #[test]
    fn a_confirmation_is_dangerous_unless_told_otherwise() {
        let confirm = owned(|| Confirm::new("Delete it?", || {}));

        assert_eq!(confirm.tone, Tone::Danger);
    }

    #[test]
    fn an_unnamed_question_still_has_a_heading() {
        // A dialog with a blank heading reads as a rendering fault.
        let confirm = owned(|| Confirm::new("Delete it?", || {}));

        assert_eq!(confirm.heading(), "Are you sure?");
        assert_eq!(
            owned(|| Confirm::new("q", || {}).titled("Delete role")).heading(),
            "Delete role"
        );
    }

    #[test]
    fn the_button_can_say_what_it_does() {
        let confirm = owned(|| Confirm::new("Delete it?", || {}).confirm_label("Delete"));

        assert_eq!(confirm.confirm_label, "Delete");
    }
}
