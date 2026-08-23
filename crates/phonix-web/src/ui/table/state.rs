//! What the viewer has done to the table since it opened.
//!
//! Search text, page, page size, sort, and which columns are turned off. All of
//! it is per-view and none of it is stored: reopening a screen gives everyone
//! the same table, which is the version that can be described over a shoulder.
//!
//! # Every change but one resets to page one
//!
//! Searching, sorting or resizing while on page four almost always means the
//! viewer wants to see the top of the new result, not row 76 of it - and on a
//! shorter result there may be no page four to stay on. [`GridState`] therefore
//! resets the page itself rather than leaving each control to remember, which
//! is the kind of thing exactly one control always forgets.

use std::collections::{BTreeMap, BTreeSet};

use leptos::prelude::*;
use phonix_core::query::{DateRange, PageRequest, Sort, SortDirection};

use super::config::GridConfig;

/// The live state of one grid on screen.
#[derive(Clone, Copy)]
pub struct GridState {
    pub search: RwSignal<String>,
    pub page: RwSignal<u32>,
    pub per_page: RwSignal<u32>,
    pub sort: RwSignal<Option<Sort>>,
    /// What each declared filter is set to. A key that is absent, or set to
    /// the empty string, is a filter nobody has narrowed.
    pub filters: RwSignal<BTreeMap<String, String>>,
    /// Fields of the columns that are currently turned off.
    pub hidden: RwSignal<BTreeSet<&'static str>>,
    /// Whether the column menu is up.
    pub columns_open: RwSignal<bool>,
}

impl GridState {
    /// Opened as the configuration describes it.
    pub fn new<T: 'static>(config: &GridConfig<T>) -> Self {
        Self {
            search: RwSignal::new(String::new()),
            page: RwSignal::new(1),
            per_page: RwSignal::new(config.pagination.default),
            sort: RwSignal::new(config.initial_sort.clone()),
            filters: RwSignal::new(BTreeMap::new()),
            hidden: RwSignal::new(config.hidden_by_default().into_iter().collect()),
            columns_open: RwSignal::new(false),
        }
    }

    /// The request as it now stands. Tracks every signal it reads, so a
    /// resource keyed on this re-runs when anything changes.
    pub fn request(&self) -> PageRequest {
        PageRequest {
            page: self.page.get(),
            per_page: self.per_page.get(),
            search: self.search.get(),
            sort: self.sort.get(),
            filters: self.filters.get(),
        }
    }

    /// What `key` is narrowed to right now, as the `<select>` needs it.
    pub fn filter(&self, key: &'static str) -> String {
        self.filters
            .with(|filters| filters.get(key).cloned().unwrap_or_default())
    }

    /// Narrow, or stop narrowing when `value` is empty.
    ///
    /// Back to page one, for the same reason searching is: the page being
    /// looked at usually does not survive the narrowing.
    pub fn set_filter(&self, key: &'static str, value: String) {
        self.filters.update(|filters| {
            if value.is_empty() {
                filters.remove(key);
            } else {
                filters.insert(key.to_owned(), value);
            }
        });

        self.page.set(1);
    }

    /// The span of time `key` is narrowed to, as its two halves carry it.
    pub fn range(&self, key: &'static str) -> DateRange {
        let (from, to) = DateRange::keys(key);

        self.filters.with(|filters| {
            let end = |key: &str| {
                filters
                    .get(key)
                    .map(String::as_str)
                    .and_then(DateRange::decode)
            };

            DateRange::new(end(&from), end(&to))
        })
    }

    /// Narrow to a span of time, or stop narrowing when it is
    /// [`DateRange::ANY`].
    ///
    /// Both halves are written in one update rather than through two calls to
    /// [`set_filter`](Self::set_filter). A range is one decision, and writing
    /// it as two would put a request on the wire in between - the one asking
    /// for everything from Monday with no end yet, whose rows nobody wants and
    /// whose round trip the next request has to be drawn over.
    pub fn set_range(&self, key: &'static str, range: DateRange) {
        let (from_key, to_key) = DateRange::keys(key);

        self.filters.update(|filters| {
            for (key, end) in [(from_key, range.from), (to_key, range.to)] {
                match end {
                    Some(at) => filters.insert(key, DateRange::encode(at)),
                    None => filters.remove(&key),
                };
            }
        });

        self.page.set(1);
    }

    pub fn set_search(&self, text: String) {
        self.search.set(text);
        self.page.set(1);
    }

    pub fn set_per_page(&self, per_page: u32) {
        self.per_page.set(per_page);
        self.page.set(1);
    }

    pub fn go_to(&self, page: u32) {
        self.page.set(page.max(1));
    }

    /// Sort by `field`, or turn the existing sort around if it is already the
    /// one in force.
    ///
    /// There is no way back to unsorted. A third click that restored "whatever
    /// order the server happened to return" would be a state nobody can name,
    /// and every click after it would be a guess about which of the three the
    /// column is in.
    pub fn toggle_sort(&self, field: &'static str) {
        self.sort.update(|sort| {
            *sort = Some(match sort {
                Some(current) if current.is(field) => current.flipped(),
                _ => Sort::ascending(field),
            });
        });

        self.page.set(1);
    }

    /// Which way this column is sorted, if it is the one in force.
    pub fn sort_of(&self, field: &str) -> Option<SortDirection> {
        self.sort.with(|sort| {
            sort.as_ref()
                .filter(|sort| sort.is(field))
                .map(|sort| sort.direction)
        })
    }

    pub fn is_hidden(&self, field: &'static str) -> bool {
        self.hidden.with(|hidden| hidden.contains(field))
    }

    pub fn toggle_column(&self, field: &'static str) {
        self.hidden.update(|hidden| {
            if !hidden.remove(field) {
                hidden.insert(field);
            }
        });
    }

    /// Turn every column back on.
    pub fn show_all_columns(&self) {
        self.hidden.update(BTreeSet::clear);
    }

    /// Whether anything has been typed into the search box.
    pub fn is_searching(&self) -> bool {
        self.search.with(|search| !search.trim().is_empty())
    }

    pub fn clear_search(&self) {
        self.set_search(String::new());
    }
}
