//! Which control a field draws.
//!
//! Deliberately a small, closed set. Every entry here is one more thing the
//! renderer has to draw, label, disable and report errors on consistently, so
//! a kind earns its place by being needed by a screen rather than by being
//! conceivable.
//!
//! Notice what is *not* a kind: "required", "read-only", "full width" and
//! "which permission" are all properties of the [`Field`](super::Field), not of
//! the control. A required email and an optional one are the same control asked
//! a different question, and folding that into the kind would double the set
//! every time a new question was asked of a field.
//!
//! # Why this is not `PartialEq`
//!
//! It was, until [`FieldKind::Lookup`] arrived. A lookup's picker is an erased
//! closure - see [`Choices`] - and the only equality a closure can offer is
//! pointer identity, which reports two identically-built configurations as
//! different every time a screen re-renders. Rather than answer dishonestly,
//! the derive was dropped: nothing outside this file ever compared two kinds,
//! and what the form actually compares for dirtiness is the *draft*, which is
//! `PartialEq` for real.
//!
//! `Debug` survived, because it can be answered honestly - a closure prints as
//! a closure and everything around it prints as itself.

use super::field::Choice;
use crate::ui::lookup::{Choices, QuickAdd};

/// The control a field is drawn as.
#[derive(Debug, Clone)]
pub enum FieldKind {
    Text,
    /// `type=email`, which gets the right keyboard on a phone and the
    /// browser's own shape check. The address is still validated by the
    /// service - `a@b` satisfies a browser and is not an address.
    Email,
    /// Never rendered holding a value, and never read back out of the DOM by
    /// anything but its own control.
    Password,
    Multiline {
        rows: u8,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
    },
    Toggle,
    Select {
        choices: Vec<Choice>,
    },
    MultiSelect {
        choices: Vec<Choice>,
    },
    /// A lookup: a field whose choices are *records* of another entity.
    ///
    /// The kind a `<select>` cannot be. See [`crate::ui::lookup`] for the two
    /// presentations and for why the picker is an erased closure rather than a
    /// type parameter that would spread to every form holding one.
    ///
    /// This is the only kind that reads and writes through
    /// [`FieldValue::Records`], because it is the only one whose value needs a
    /// label the control cannot derive from it.
    ///
    /// [`FieldValue::Records`]: super::FieldValue::Records
    Lookup {
        choices: Choices,
        quick_add: Option<QuickAdd>,
        /// Choosing toggles rather than replaces, and the panel stays open.
        multiple: bool,
    },
}

impl FieldKind {
    /// The `type` attribute for the kinds that are an `<input>`.
    ///
    /// `None` for the kinds that are not - a textarea, a select, a set of
    /// checkboxes - which is what the renderer branches on.
    pub const fn input_type(&self) -> Option<&'static str> {
        match self {
            Self::Text => Some("text"),
            Self::Email => Some("email"),
            Self::Password => Some("password"),
            Self::Number { .. } => Some("number"),
            Self::Multiline { .. }
            | Self::Toggle
            | Self::Select { .. }
            | Self::MultiSelect { .. }
            | Self::Lookup { .. } => None,
        }
    }

    /// The options, for the kinds that have them.
    ///
    /// A lookup answers only when its options are a list that was sent with
    /// the page. A table picker's rows are on the server, so the honest answer
    /// there is that this function cannot say - and empty is that answer.
    pub fn choices(&self) -> &[Choice] {
        match self {
            Self::Select { choices } | Self::MultiSelect { choices } => choices,
            Self::Lookup {
                choices: Choices::List(choices),
                ..
            } => choices,
            _ => &[],
        }
    }

    /// Whether this control holds records rather than strings.
    ///
    /// What the renderer branches on to decide it needs `FieldValue::Records`
    /// on both sides of the wire, rather than a string from the DOM.
    pub const fn is_lookup(&self) -> bool {
        matches!(self, Self::Lookup { .. })
    }

    /// Whether this control holds something that must never be echoed back.
    ///
    /// A form re-renders from what the server stored; a password field must be
    /// the one thing that does not, or the value ends up in the DOM of a page
    /// that may be screenshotted, cached or restored from history.
    pub const fn is_secret(&self) -> bool {
        matches!(self, Self::Password)
    }
}

#[cfg(test)]
mod tests {
    use leptos::prelude::*;

    use super::*;

    #[test]
    fn the_kinds_that_are_inputs_say_which_type() {
        assert_eq!(FieldKind::Email.input_type(), Some("email"));
        assert_eq!(
            FieldKind::Number {
                min: None,
                max: None,
                step: None
            }
            .input_type(),
            Some("number")
        );
    }

    #[test]
    fn the_kinds_that_are_not_inputs_say_so() {
        assert_eq!(FieldKind::Toggle.input_type(), None);
        assert_eq!(FieldKind::Multiline { rows: 3 }.input_type(), None);
        assert_eq!(
            FieldKind::Select {
                choices: Vec::new()
            }
            .input_type(),
            None
        );
    }

    #[test]
    fn a_lookup_over_a_list_still_reports_its_options() {
        let kind = FieldKind::Lookup {
            choices: Choices::List(vec![Choice::new("a", "A")]),
            quick_add: None,
            multiple: false,
        };

        assert_eq!(kind.choices().len(), 1);
        assert!(kind.is_lookup());
        assert_eq!(kind.input_type(), None);
    }

    #[test]
    fn a_table_picker_does_not_pretend_to_know_its_rows() {
        // They are on the server. Answering with anything but "I cannot say"
        // would be a form validating against a list it never had.
        let kind = FieldKind::Lookup {
            choices: Choices::table(|_| ().into_any()),
            quick_add: None,
            multiple: false,
        };

        assert!(kind.choices().is_empty());
        assert!(kind.is_lookup());
    }

    #[test]
    fn only_a_password_is_a_secret() {
        assert!(FieldKind::Password.is_secret());
        assert!(!FieldKind::Text.is_secret());
    }

    #[test]
    fn both_choosing_kinds_expose_their_options() {
        let choices = vec![Choice::new("a", "A")];

        assert_eq!(
            FieldKind::Select {
                choices: choices.clone()
            }
            .choices()
            .len(),
            1
        );
        assert_eq!(FieldKind::MultiSelect { choices }.choices().len(), 1);
        assert!(FieldKind::Text.choices().is_empty());
    }
}
