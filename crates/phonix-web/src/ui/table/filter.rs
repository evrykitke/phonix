//! Named narrowings: "only failures", "only low stock".
//!
//! # Why this is not the search box
//!
//! Search is free text, and every source answers it the same way - look inside
//! the row and see whether the words are there. A filter is a *named
//! predicate*: "notable" is not a word in any column, it is a question only the
//! module that defined it can answer.
//!
//! Modelling one as the other is the failure this avoids. A checkbox wired to
//! the search box would filter on whatever the word happened to match, and a
//! filter dropped into a column would have to be sortable, exportable and
//! searchable to fit through [`Column`](super::Column), which it is not.
//!
//! # A filter is answered where the rows are
//!
//! The chosen value travels in [`PageRequest::filters`], so both sources can
//! answer it, and each answers it in the only place it can:
//!
//! * [`Source::in_memory`](super::Source::in_memory) - the browser holds every
//!   row, so the filter is a closure over one row. Supply it with
//!   [`Filter::matching`].
//! * [`Source::paged`](super::Source::paged) - the browser holds one page and
//!   cannot know what it did not fetch, so the key crosses the wire and the
//!   reader turns it into a `WHERE` clause. No closure, and one would be a lie:
//!   it could only narrow the page already on screen.
//!
//! Getting that backwards is caught at build time in debug: see
//! [`GridConfig::filter`](super::GridConfig::filter).
//!
//! # The first choice is "all of them"
//!
//! Every filter opens showing everything, and the choice that means that
//! carries an empty value - which [`PageRequest::sanitised`] then drops, so a
//! reader never has to know both spellings of "not filtered".

use std::sync::Arc;

use phonix_core::query::PageRequest;

/// One option of a filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterChoice {
    /// What crosses the wire. Empty means "everything"; see the module docs.
    /// Machinery, and stays one.
    pub value: &'static str,
    /// What the control reads. A sentence, and comes out of the catalog.
    pub label: String,
}

impl FilterChoice {
    pub fn new(value: &'static str, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }

    /// The opening choice: everything, unnarrowed.
    pub fn all(label: impl Into<String>) -> Self {
        Self {
            value: "",
            label: label.into(),
        }
    }
}

/// Whether a row survives a chosen value.
type Matches<T> = Arc<dyn Fn(&T, &str) -> bool + Send + Sync>;

/// One narrowing offered above the table.
pub struct Filter<T: 'static> {
    /// The name the reader knows it by. Stable, and it is what a paged source
    /// looks for in [`PageRequest::filter`].
    pub(crate) key: &'static str,
    pub(crate) label: String,
    /// Owned rather than `&'static`: the labels come out of the catalog, and
    /// a `const` slice cannot hold a `String`.
    pub(crate) choices: Vec<FilterChoice>,
    /// How to answer it over rows already in the browser. `None` on a paged
    /// grid, where the server answers instead.
    pub(crate) matches: Option<Matches<T>>,
}

impl<T: 'static> Clone for Filter<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            label: self.label.clone(),
            choices: self.choices.clone(),
            matches: self.matches.clone(),
        }
    }
}

impl<T: 'static> Filter<T> {
    /// A filter whose choices are fixed.
    ///
    /// ```ignore
    /// const KINDS: &[FilterChoice] = &[
    ///     FilterChoice::all("All events"),
    ///     FilterChoice::new("notable", l!("audit.outcome.notable")),
    /// ];
    ///
    /// Filter::new("kind", "Show", KINDS)
    /// ```
    pub fn new(key: &'static str, label: impl Into<String>, choices: Vec<FilterChoice>) -> Self {
        Self {
            key,
            label: label.into(),
            choices,
            matches: None,
        }
    }

    /// How to answer this filter in the browser. Required for an in-memory
    /// grid, meaningless on a paged one.
    #[must_use]
    pub fn matching(mut self, matches: impl Fn(&T, &str) -> bool + Send + Sync + 'static) -> Self {
        self.matches = Some(Arc::new(matches));
        self
    }

    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// Whether this row survives whatever `request` chose.
    ///
    /// True when nothing was chosen, and true when there is no closure to ask -
    /// a paged grid has already been narrowed by the server, and narrowing the
    /// page a second time here would drop rows it was right to send.
    pub fn accepts(&self, row: &T, request: &PageRequest) -> bool {
        match (request.filter(self.key), &self.matches) {
            (None, _) | (Some(_), None) => true,
            (Some(value), Some(matches)) => matches(row, value),
        }
    }

    /// Whether this filter can be answered without asking the server.
    pub const fn is_local(&self) -> bool {
        self.matches.is_some()
    }

    /// The value this filter opens on: the first choice.
    pub fn default_value(&self) -> &'static str {
        self.choices.first().map_or("", |choice| choice.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Vec<FilterChoice> {
        vec![
            FilterChoice::all("Everything"),
            FilterChoice::new("even", "Only even"),
        ]
    }

    fn evens() -> Filter<u8> {
        Filter::new("kind", "Show", kinds()).matching(|row: &u8, value| match value {
            "even" => row.is_multiple_of(2),
            _ => true,
        })
    }

    #[test]
    fn a_filter_nobody_chose_keeps_every_row() {
        let filter = evens();
        let request = PageRequest::first(10);

        assert!(filter.accepts(&1, &request));
        assert!(filter.accepts(&2, &request));
    }

    #[test]
    fn a_chosen_filter_narrows() {
        let filter = evens();
        let request = PageRequest::first(10).filtered_by("kind", "even");

        assert!(!filter.accepts(&1, &request));
        assert!(filter.accepts(&2, &request));
    }

    #[test]
    fn a_filter_with_no_closure_keeps_every_row_it_is_handed() {
        // The paged case: the server already narrowed, and narrowing the page
        // again here would throw away rows it was right to send.
        let filter: Filter<u8> = Filter::new("kind", "Show", kinds());
        let request = PageRequest::first(10).filtered_by("kind", "even");

        assert!(filter.accepts(&1, &request));
        assert!(!filter.is_local());
    }

    #[test]
    fn a_filter_opens_on_its_first_choice_and_that_choice_is_everything() {
        assert_eq!(evens().default_value(), "");
    }
}
