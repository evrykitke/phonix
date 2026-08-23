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

use super::field::Choice;

/// The control a field is drawn as.
#[derive(Debug, Clone, PartialEq)]
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
            | Self::MultiSelect { .. } => None,
        }
    }

    /// The options, for the kinds that have them.
    pub fn choices(&self) -> &[Choice] {
        match self {
            Self::Select { choices } | Self::MultiSelect { choices } => choices,
            _ => &[],
        }
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
