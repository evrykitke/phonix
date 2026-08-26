//! What a control holds while it is being edited.
//!
//! # Why an editing value is not a [`Cell`]
//!
//! [`Cell`](crate::ui::table::Cell) is what a *column* reads: one-way, lossy,
//! and shaped for comparing and displaying. `Cell::Text("Active")` cannot be
//! turned back into a `UserStatus`, and it was never meant to be - a grid never
//! writes.
//!
//! A form does. So a field declares two closures instead of one: read the draft
//! into a [`FieldValue`], and write a `FieldValue` back into the draft. The
//! conversion in both directions is the field's own business, which is what
//! keeps a typed entity typed while the control in the middle only ever handles
//! strings and booleans.
//!
//! # Empty is not zero, and not "the first choice"
//!
//! [`FieldValue::Number`] and [`FieldValue::Choice`] hold an `Option`, because
//! an empty number box and a zero are different answers and a select that has
//! not been touched is not the same as one set to its first option. Collapsing
//! either would make a form quietly store something nobody typed.
//!
//! # A record is not a choice
//!
//! [`FieldValue::Choice`] holds a string because an option's label is a
//! constant the control already has. A *record* chosen from another entity is
//! not like that: its label lives in the other entity's row, and the control
//! that found it - a table picker - has a row rather than an id-to-label map.
//! A draft keeping only the id would redraw the field blank the next time the
//! form rendered, which is why [`FieldValue::Records`] carries both halves and
//! the draft is expected to store both.

use std::collections::BTreeSet;

use super::field::Choice;

/// One control's current value.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Bool(bool),
    /// `None` when the box is empty, which is not the same as `Some(0.0)`.
    Number(Option<f64>),
    /// One choice from a fixed set. `None` when nothing is chosen.
    Choice(Option<String>),
    /// Several choices. Ordered and de-duplicated so that two forms holding the
    /// same set compare equal however they were clicked into it.
    Choices(BTreeSet<String>),
    /// Records chosen from another entity, each carrying the label needed to
    /// draw it. Empty is nothing chosen; a single-valued lookup holds at most
    /// one.
    ///
    /// A `Vec` rather than a set, because the order records were chosen in is
    /// visible - they are drawn as chips in that order - and sorting them
    /// would rearrange the field under the person filling it in.
    Records(Vec<Choice>),
}

impl FieldValue {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// A choice, from something that names itself.
    pub fn choice(value: impl Into<String>) -> Self {
        Self::Choice(Some(value.into()))
    }

    pub fn choices(values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Choices(values.into_iter().map(Into::into).collect())
    }

    /// The records a lookup holds.
    pub fn records(records: impl IntoIterator<Item = Choice>) -> Self {
        Self::Records(records.into_iter().collect())
    }

    /// One record, or nothing chosen.
    ///
    /// Takes an `Option` rather than offering a separate empty constructor,
    /// because the caller almost always has one: a draft's `Option<PartyId>`,
    /// or an id that is only meaningful next to the name stored beside it.
    pub fn record(record: Option<Choice>) -> Self {
        Self::Records(record.into_iter().collect())
    }

    /// The value as a control's string, which is what an `<input>` binds to.
    pub fn as_input(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Bool(true) => "true".to_owned(),
            Self::Bool(false) => "false".to_owned(),
            Self::Number(None) | Self::Choice(None) => String::new(),
            Self::Number(Some(number)) => format_number(*number),
            Self::Choice(Some(choice)) => choice.clone(),
            Self::Choices(choices) => choices.iter().cloned().collect::<Vec<_>>().join(", "),
            Self::Records(records) => records
                .iter()
                .map(|record| record.value.clone())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Text(text) => !text.is_empty(),
            Self::Number(number) => number.is_some_and(|n| n != 0.0),
            Self::Choice(choice) => choice.is_some(),
            Self::Choices(choices) => !choices.is_empty(),
            Self::Records(records) => !records.is_empty(),
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(number) => *number,
            Self::Text(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// The chosen value, for a single-choice control.
    pub fn as_choice(&self) -> Option<&str> {
        match self {
            Self::Choice(choice) => choice.as_deref(),
            Self::Records(records) => records.first().map(|record| record.value.as_str()),
            Self::Text(text) if !text.is_empty() => Some(text),
            _ => None,
        }
    }

    /// The chosen values, for a multiple-choice control.
    pub fn as_set(&self) -> BTreeSet<String> {
        match self {
            Self::Choices(choices) => choices.clone(),
            Self::Choice(Some(choice)) => BTreeSet::from([choice.clone()]),
            Self::Records(records) => records
                .iter()
                .map(|record| record.value.clone())
                .collect(),
            _ => BTreeSet::new(),
        }
    }

    /// The records chosen, for a lookup.
    ///
    /// Empty for every other kind. This is the only accessor that keeps the
    /// labels, and it is what the control seeds itself from.
    pub fn as_records(&self) -> Vec<Choice> {
        match self {
            Self::Records(records) => records.clone(),
            _ => Vec::new(),
        }
    }

    /// The same kind of value, taken from a control's string.
    ///
    /// The *kind* comes from what is already there rather than from the event,
    /// because the DOM only ever hands back a string and a number field that
    /// silently became text would submit `"12"` where the draft wants `12`.
    #[must_use]
    pub fn with_input(&self, input: String) -> Self {
        match self {
            Self::Text(_) => Self::Text(input),
            Self::Bool(_) => Self::Bool(matches!(input.as_str(), "true" | "on" | "1")),
            Self::Number(_) => {
                let trimmed = input.trim();

                // An unparseable number keeps the box empty rather than
                // reverting to the last good value, which would fight the
                // person typing "-" before a digit.
                Self::Number(
                    (!trimmed.is_empty())
                        .then(|| trimmed.parse().ok())
                        .flatten(),
                )
            }
            Self::Choice(_) => Self::Choice((!input.is_empty()).then_some(input)),
            Self::Choices(choices) => {
                let mut choices = choices.clone();

                // A multiple-choice control reports one toggled member at a
                // time, so this is a flip rather than a replacement.
                if !choices.remove(&input) {
                    choices.insert(input);
                }

                Self::Choices(choices)
            }
            Self::Records(records) => {
                // Removal only, and deliberately. A bare string can name a
                // record but cannot describe one - there is no label to be had
                // from the DOM - so adding here would store a chip with nothing
                // written on it. A lookup answers with whole records through
                // the field's writer instead; this is what a chip's own remove
                // button goes through.
                let mut records = records.clone();
                records.retain(|record| record.value != input);

                Self::Records(records)
            }
        }
    }

    /// Whether this counts as filled in, for a required field.
    ///
    /// Whitespace is not an answer: a required name of `"   "` is a blank one,
    /// and accepting it stores a row nobody can identify.
    pub fn is_present(&self) -> bool {
        match self {
            Self::Text(text) => !text.trim().is_empty(),
            // A false toggle is an answer. A required checkbox that has to be
            // ticked is a *consent* control, and it says so with its own rule.
            Self::Bool(_) => true,
            Self::Number(number) => number.is_some(),
            Self::Choice(choice) => choice.is_some(),
            Self::Choices(choices) => !choices.is_empty(),
            Self::Records(records) => !records.is_empty(),
        }
    }
}

/// Trailing zeroes dropped: `4`, not `4.0000000`.
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.4}");

        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_number_box_is_not_a_zero() {
        let empty = FieldValue::Number(None);

        assert_eq!(empty.as_input(), "");
        assert!(!empty.is_present());
        assert!(FieldValue::Number(Some(0.0)).is_present());
    }

    #[test]
    fn typing_keeps_the_kind_the_field_already_had() {
        let number = FieldValue::Number(Some(8.0));

        assert_eq!(
            number.with_input("12".into()),
            FieldValue::Number(Some(12.0))
        );
        // Not `Text("12")`, which is what taking the kind from the DOM would
        // have produced.
        assert!(matches!(
            number.with_input("12".into()),
            FieldValue::Number(_)
        ));
    }

    #[test]
    fn a_half_typed_number_leaves_the_box_empty_rather_than_snapping_back() {
        // Somebody typing "-12" passes through "-", and a control that reverted
        // to the last good value there cannot be typed into at all.
        assert_eq!(
            FieldValue::Number(Some(8.0)).with_input("-".into()),
            FieldValue::Number(None)
        );
    }

    #[test]
    fn whitespace_does_not_satisfy_a_required_text_field() {
        assert!(!FieldValue::text("   ").is_present());
        assert!(FieldValue::text("Ada").is_present());
    }

    #[test]
    fn an_unticked_toggle_is_still_an_answer() {
        // Otherwise every optional checkbox would fail a required check.
        assert!(FieldValue::Bool(false).is_present());
    }

    #[test]
    fn a_multiple_choice_control_toggles_rather_than_replaces() {
        let roles = FieldValue::choices(["admin"]);
        let added = roles.with_input("auditor".into());

        assert_eq!(
            added.as_set(),
            BTreeSet::from(["admin".to_owned(), "auditor".to_owned()])
        );

        let removed = added.with_input("admin".into());

        assert_eq!(removed.as_set(), BTreeSet::from(["auditor".to_owned()]));
    }

    #[test]
    fn a_record_keeps_the_label_the_value_cannot_be_turned_back_into() {
        // The whole reason this variant exists. A form re-rendered from a draft
        // that kept only "USD" has nowhere to look up "US Dollar".
        let chosen = FieldValue::record(Some(Choice::new("USD", "US Dollar")));

        assert_eq!(chosen.as_choice(), Some("USD"));
        assert_eq!(
            chosen.as_records().first().map(|record| record.label.clone()),
            Some("US Dollar".to_owned())
        );
        assert!(chosen.is_present());
    }

    #[test]
    fn nothing_chosen_is_no_records_rather_than_an_empty_one() {
        let empty = FieldValue::record(None);

        assert_eq!(empty.as_input(), "");
        assert!(!empty.is_present());
        assert!(empty.as_records().is_empty());
    }

    #[test]
    fn a_string_can_take_a_record_away_and_cannot_bring_one_back() {
        // A bare string has no label attached, so adding through this path
        // would store a chip with nothing written on it.
        let held = FieldValue::records([Choice::new("USD", "US Dollar")]);

        assert!(held.with_input("USD".to_owned()).as_records().is_empty());
        assert_eq!(held.with_input("EUR".to_owned()), held);
    }

    #[test]
    fn several_records_report_every_value_they_hold() {
        let held = FieldValue::records([
            Choice::new("USD", "US Dollar"),
            Choice::new("EUR", "Euro"),
        ]);

        assert_eq!(held.as_input(), "USD, EUR");
        assert_eq!(
            held.as_set(),
            BTreeSet::from(["USD".to_owned(), "EUR".to_owned()])
        );
        // The first is what a single-valued lookup would store.
        assert_eq!(held.as_choice(), Some("USD"));
    }

    #[test]
    fn only_a_lookup_answers_with_records() {
        assert!(FieldValue::text("USD").as_records().is_empty());
        assert!(FieldValue::choice("USD").as_records().is_empty());
    }

    #[test]
    fn a_set_compares_equal_however_it_was_clicked_into() {
        assert_eq!(
            FieldValue::choices(["b", "a"]),
            FieldValue::choices(["a", "b"])
        );
    }

    #[test]
    fn clearing_a_select_is_nothing_chosen_rather_than_an_empty_choice() {
        let chosen = FieldValue::choice("active");

        assert_eq!(chosen.with_input(String::new()), FieldValue::Choice(None));
        assert!(!FieldValue::Choice(None).is_present());
    }
}
