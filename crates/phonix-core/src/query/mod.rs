//! Asking for one page of a list, and what comes back.
//!
//! # Why this is domain vocabulary and not a table widget's private business
//!
//! "Page three of the users, twenty-five at a time, sorted by last sign-in,
//! matching `smith`" is a question about data, not about presentation. Three
//! layers need to agree on how it is phrased:
//!
//! * the **browser**, which turns a click on a column header into a request
//! * the **server function**, which carries it across the wire
//! * the **DBAL**, which turns it into `ORDER BY ... LIMIT ... OFFSET ...`
//!
//! If each invented its own shape, every listing endpoint would grow a slightly
//! different set of `page`/`size`/`offset`/`skip` parameters and the off-by-one
//! would be rewritten once per module. So the phrasing lives here, in the layer
//! all three already depend on, and the module that runs the query - in SQL or
//! over a `Vec` - decides only *how* to answer it.
//!
//! # The two conventions worth knowing
//!
//! **Pages are 1-based.** `page = 1` is the first page, because that is what
//! the pager says and what people put in URLs. [`PageRequest::offset`] does the
//! subtraction exactly once, here.
//!
//! **A request is a wish, not a promise.** Anything can arrive over the wire:
//! page zero, ten million rows per page, a sort naming a column that does not
//! exist. [`PageRequest::sanitised`] is what every reader must call first, so
//! the clamping is one function rather than a habit.

pub mod range;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use range::DateRange;

/// The largest page anything will serve, however large a page is asked for.
///
/// A ceiling rather than a preference: it exists so that a hand-written request
/// cannot ask Postgres for a million rows, not to express a view about how many
/// rows read well.
pub const MAX_PER_PAGE: u32 = 500;

/// The page size used when nothing says otherwise.
pub const DEFAULT_PER_PAGE: u32 = 25;

/// Which way a sort runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Ascending)
    }

    /// The other one. What a second click on the same column means.
    pub const fn flipped(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// `ASC` or `DESC`.
    ///
    /// Safe to interpolate into SQL because it is one of two literals - unlike
    /// [`Sort::field`], which never may be.
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }
}

/// A column to order by, and which way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    /// The field's stable identifier - not its heading, which is a label and
    /// changes with the wording.
    ///
    /// Arrives from the browser, so a reader that puts this anywhere near SQL
    /// must match it against a list of columns it already knows. Never
    /// interpolate it.
    pub field: String,
    pub direction: SortDirection,
}

impl Sort {
    pub fn ascending(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            direction: SortDirection::Ascending,
        }
    }

    pub fn descending(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            direction: SortDirection::Descending,
        }
    }

    /// The same column, the other way.
    pub fn flipped(&self) -> Self {
        Self {
            field: self.field.clone(),
            direction: self.direction.flipped(),
        }
    }

    pub fn is(&self, field: &str) -> bool {
        self.field == field
    }
}

/// One page of a list, as asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    /// 1-based.
    pub page: u32,
    pub per_page: u32,
    /// What was typed into the search box. Empty means "everything".
    pub search: String,
    pub sort: Option<Sort>,
    /// Narrowings the viewer chose from a fixed set - "only failures", "only
    /// low stock". Keyed by a stable name the reader knows.
    ///
    /// Separate from `search` because the two are different questions. Search
    /// is free text and every reader answers it the same way, by looking
    /// inside the row. A filter is a named predicate that only the reader
    /// knows how to apply, and a reader that is handed a key it does not
    /// recognise ignores it - see [`PageRequest::filter`].
    ///
    /// `default` because a request crosses the wire as form encoding, where an
    /// empty map has no representation: nothing is written for it, so the
    /// field is simply absent on the way back in. An unfiltered request is the
    /// common case, not the edge one, so without this every list that nobody
    /// has narrowed answers "missing field `filters`". It also makes the field
    /// additive - a browser running a build from before it existed still
    /// deserialises against a server that has it.
    #[serde(default)]
    pub filters: BTreeMap<String, String>,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::first(DEFAULT_PER_PAGE)
    }
}

impl PageRequest {
    /// The first page, unsorted and unfiltered.
    pub fn first(per_page: u32) -> Self {
        Self {
            page: 1,
            per_page,
            search: String::new(),
            sort: None,
            filters: BTreeMap::new(),
        }
    }

    /// The same request with impossible values replaced by possible ones.
    ///
    /// Call this before acting on a request from anywhere: page zero becomes
    /// page one, a page size of zero becomes the default, and an enormous one
    /// becomes [`MAX_PER_PAGE`].
    #[must_use]
    pub fn sanitised(&self) -> Self {
        Self {
            page: self.page.max(1),
            per_page: match self.per_page {
                0 => DEFAULT_PER_PAGE,
                n => n.min(MAX_PER_PAGE),
            },
            search: self.search.trim().to_owned(),
            sort: self.sort.clone(),
            // Filters whose value is empty are the "all of them" choice, and
            // carrying them any further would make every reader check for the
            // empty string as well as for absence.
            filters: self
                .filters
                .iter()
                .filter(|(_, value)| !value.trim().is_empty())
                .map(|(key, value)| (key.clone(), value.trim().to_owned()))
                .collect(),
        }
    }

    /// How many rows to skip. Sanitise first.
    pub fn offset(&self) -> u64 {
        u64::from(self.page.saturating_sub(1)) * u64::from(self.per_page)
    }

    /// How many rows to take. Sanitise first.
    pub fn limit(&self) -> u64 {
        u64::from(self.per_page)
    }

    /// The search text folded for matching, or `None` when nothing was typed.
    ///
    /// Lowercased here so that every implementation of "does this row match"
    /// folds case the same way, rather than one of them forgetting.
    pub fn needle(&self) -> Option<String> {
        let trimmed = self.search.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_lowercase())
    }

    /// What was chosen for `key`, or `None` when nothing was.
    ///
    /// A reader answers the keys it knows and ignores the rest. That is not
    /// laziness: a filter arrives from a browser that may be running a newer
    /// build of the screen than the server, and refusing the whole request over
    /// an unknown narrowing would turn a cosmetic mismatch into an error page.
    pub fn filter(&self, key: &str) -> Option<&str> {
        self.filters
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// Whether `key` was set to `value`.
    pub fn filter_is(&self, key: &str, value: &str) -> bool {
        self.filter(key) == Some(value)
    }

    /// The same request, narrowed. Mostly for tests and for callers that build
    /// a request by hand.
    #[must_use]
    pub fn filtered_by(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.filters.insert(key.into(), value.into());
        self
    }

    /// The span of time chosen for `key`, as its two halves carry it.
    ///
    /// Unparseable and absent are the same answer - unbounded in that
    /// direction - for the reason [`filter`](Self::filter) ignores a key it
    /// does not know: a stale value from an older build of the screen should
    /// narrow nothing, not turn a list into an error page.
    pub fn range(&self, key: &str) -> DateRange {
        let (from, to) = DateRange::keys(key);

        DateRange::new(
            self.filter(&from).and_then(DateRange::decode),
            self.filter(&to).and_then(DateRange::decode),
        )
    }

    /// The same request, narrowed to a span of time.
    ///
    /// An absent end removes its key rather than writing an empty one, so that
    /// "unbounded" has one spelling in the map instead of two.
    #[must_use]
    pub fn in_range(mut self, key: &str, range: DateRange) -> Self {
        let (from_key, to_key) = DateRange::keys(key);

        for (key, end) in [(from_key, range.from), (to_key, range.to)] {
            match end {
                Some(at) => self.filters.insert(key, DateRange::encode(at)),
                None => self.filters.remove(&key),
            };
        }

        self
    }

    /// The same request, pulled back to a page that exists.
    ///
    /// Filtering a list can strip away the page being looked at - typing into
    /// the search box on page nine usually does. Showing an empty table with a
    /// pager that says "page 9 of 1" is the failure this avoids.
    #[must_use]
    pub fn clamped_to(&self, total: u64) -> Self {
        let sane = self.sanitised();
        let last = page_count(total, sane.per_page);

        Self {
            page: sane.page.min(last),
            ..sane
        }
    }
}

/// How many pages `total` rows make at `per_page` each. Never zero: an empty
/// list is one empty page, which is what a pager has to render.
pub fn page_count(total: u64, per_page: u32) -> u32 {
    let per_page = u64::from(per_page.max(1));
    let pages = total.div_ceil(per_page).max(1);

    u32::try_from(pages).unwrap_or(u32::MAX)
}

/// The answer: some rows, and how many there were altogether.
///
/// `total` counts what matched the search, not what exists in the table. A
/// pager needs the first to size itself; nothing needs the second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub total: u64,
    /// The page these rows actually are - which may not be the page asked for,
    /// if the request was clamped.
    pub page: u32,
    pub per_page: u32,
}

impl<T> Page<T> {
    /// Rows that have already been narrowed to one page, plus the total.
    pub fn new(rows: Vec<T>, total: u64, request: &PageRequest) -> Self {
        let request = request.clamped_to(total);

        Self {
            rows,
            total,
            page: request.page,
            per_page: request.per_page,
        }
    }

    /// Nothing matched.
    pub fn empty(request: &PageRequest) -> Self {
        Self::new(Vec::new(), 0, request)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn page_count(&self) -> u32 {
        page_count(self.total, self.per_page)
    }

    pub fn has_previous(&self) -> bool {
        self.page > 1
    }

    pub fn has_next(&self) -> bool {
        self.page < self.page_count()
    }

    /// 1-based number of the first row on this page, for "showing 21-40 of 96".
    /// Zero when there are no rows at all.
    pub fn first_row_number(&self) -> u64 {
        if self.rows.is_empty() {
            return 0;
        }

        u64::from(self.page.saturating_sub(1)) * u64::from(self.per_page) + 1
    }

    /// 1-based number of the last row on this page.
    pub fn last_row_number(&self) -> u64 {
        if self.rows.is_empty() {
            return 0;
        }

        self.first_row_number() + self.rows.len() as u64 - 1
    }

    /// The same page with its rows converted - a listing turned into a view
    /// model, without the paging arithmetic being redone.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Page<U> {
        Page {
            rows: self.rows.into_iter().map(f).collect(),
            total: self.total,
            page: self.page,
            per_page: self.per_page,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;

    /// Midnight UTC on a day of August 2026.
    fn midnight(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 0, 0, 0)
            .single()
            .expect("a real instant")
    }

    #[test]
    fn the_first_page_skips_nothing() {
        let request = PageRequest::first(10);

        assert_eq!(request.offset(), 0);
        assert_eq!(request.limit(), 10);
    }

    #[test]
    fn later_pages_skip_the_ones_before_them() {
        let request = PageRequest {
            page: 3,
            ..PageRequest::first(10)
        };

        assert_eq!(request.offset(), 20);
    }

    #[test]
    fn an_impossible_request_is_made_possible() {
        let request = PageRequest {
            page: 0,
            per_page: 0,
            search: "  ".into(),
            ..PageRequest::default()
        };
        let sane = request.sanitised();

        assert_eq!(sane.page, 1);
        assert_eq!(sane.per_page, DEFAULT_PER_PAGE);
        assert_eq!(sane.search, "");
    }

    #[test]
    fn nobody_may_ask_for_a_million_rows() {
        let request = PageRequest {
            per_page: 1_000_000,
            ..PageRequest::default()
        };

        assert_eq!(request.sanitised().per_page, MAX_PER_PAGE);
    }

    #[test]
    fn searching_folds_case_and_trims() {
        let request = PageRequest {
            search: "  SmiTh ".into(),
            ..PageRequest::default()
        };

        assert_eq!(request.needle().as_deref(), Some("smith"));
    }

    #[test]
    fn a_range_is_two_ordinary_filter_keys_and_reads_back_as_one_range() {
        let (from, to) = (midnight(17), midnight(24));

        let request = PageRequest::first(10).in_range("occurred", DateRange::between(from, to));

        // Two plain entries, so a reader that knows nothing about ranges still
        // round-trips them and `sanitised` still drops them when empty.
        assert_eq!(
            request.filter("occurred_from"),
            Some("2026-08-17T00:00:00Z")
        );
        assert_eq!(request.filter("occurred_to"), Some("2026-08-24T00:00:00Z"));
        assert_eq!(request.range("occurred"), DateRange::between(from, to));
    }

    #[test]
    fn clearing_one_end_of_a_range_removes_its_key() {
        let from = midnight(17);
        let request = PageRequest::first(10)
            .in_range("occurred", DateRange::between(from, from))
            .in_range("occurred", DateRange::since(from));

        assert!(!request.filters.contains_key("occurred_to"));
        assert_eq!(request.range("occurred"), DateRange::since(from));
    }

    #[test]
    fn a_range_nobody_set_is_the_whole_of_time() {
        assert!(PageRequest::first(10).range("occurred").is_any());
    }

    #[test]
    fn a_range_half_of_which_is_nonsense_keeps_the_half_that_is_not() {
        let request = PageRequest::first(10)
            .filtered_by("occurred_from", "2026-08-17T00:00:00Z")
            .filtered_by("occurred_to", "whenever");

        assert!(request.range("occurred").from.is_some());
        assert!(request.range("occurred").to.is_none());
    }

    #[test]
    fn a_request_with_no_filters_survives_the_round_trip_it_actually_takes() {
        // Form encoding, which is how a server function carries its arguments -
        // not JSON, which writes `"filters":{}` and would have hidden this.
        // An empty map writes nothing at all, so the field comes back missing,
        // and an unfiltered request is every request until somebody narrows one.
        let request = PageRequest::first(25);
        let encoded = serde_qs::to_string(&request).expect("encodes");

        assert!(
            !encoded.contains("filters"),
            "an empty map wrote something: {encoded}"
        );

        let decoded: PageRequest = serde_qs::from_str(&encoded).expect("decodes");

        assert_eq!(decoded.page, 1);
        assert_eq!(decoded.per_page, 25);
        assert!(decoded.filters.is_empty());
    }

    #[test]
    fn a_chosen_filter_makes_it_across_the_wire() {
        let request = PageRequest::first(25).filtered_by("kind", "failures");
        let encoded = serde_qs::to_string(&request).expect("encodes");
        let decoded: PageRequest = serde_qs::from_str(&encoded).expect("decodes");

        assert!(decoded.filter_is("kind", "failures"));
    }

    #[test]
    fn a_filter_that_was_not_chosen_is_absent_rather_than_empty() {
        let request = PageRequest::first(10).filtered_by("notable", "");

        // Set to the "all of them" choice, which is the same as not set: a
        // reader must not have to know both spellings.
        assert_eq!(request.sanitised().filter("notable"), None);
        assert!(request.sanitised().filters.is_empty());
    }

    #[test]
    fn a_chosen_filter_survives_sanitising_and_clamping() {
        let request = PageRequest {
            page: 9,
            ..PageRequest::first(10)
        }
        .filtered_by("notable", "yes");

        assert!(request.sanitised().filter_is("notable", "yes"));
        assert!(request.clamped_to(12).filter_is("notable", "yes"));
    }

    #[test]
    fn an_unknown_filter_is_simply_not_answered() {
        let request = PageRequest::first(10).filtered_by("invented_by_a_newer_screen", "1");

        assert_eq!(request.filter("notable"), None);
    }

    #[test]
    fn an_empty_search_is_no_search() {
        assert!(PageRequest::default().needle().is_none());
    }

    #[test]
    fn a_partial_last_page_still_counts_as_a_page() {
        assert_eq!(page_count(21, 10), 3);
        assert_eq!(page_count(20, 10), 2);
    }

    #[test]
    fn an_empty_list_is_one_empty_page() {
        assert_eq!(page_count(0, 10), 1);
    }

    #[test]
    fn a_page_past_the_end_is_pulled_back_to_the_last_one() {
        let request = PageRequest {
            page: 9,
            ..PageRequest::first(10)
        };

        assert_eq!(request.clamped_to(12).page, 2);
    }

    #[test]
    fn a_page_within_the_list_is_left_alone() {
        let request = PageRequest {
            page: 2,
            ..PageRequest::first(10)
        };

        assert_eq!(request.clamped_to(100).page, 2);
    }

    #[test]
    fn a_page_reports_the_range_it_is_showing() {
        let request = PageRequest {
            page: 3,
            ..PageRequest::first(10)
        };
        let page = Page::new(vec!['a', 'b', 'c'], 23, &request);

        assert_eq!(page.first_row_number(), 21);
        assert_eq!(page.last_row_number(), 23);
        assert!(page.has_previous());
        assert!(!page.has_next());
    }

    #[test]
    fn an_empty_page_shows_no_range_rather_than_a_backwards_one() {
        let page: Page<char> = Page::empty(&PageRequest::first(10));

        assert_eq!(page.first_row_number(), 0);
        assert_eq!(page.last_row_number(), 0);
        assert_eq!(page.page_count(), 1);
        assert!(!page.has_next());
    }

    #[test]
    fn a_second_click_on_a_column_turns_the_sort_around() {
        let sort = Sort::ascending("email");

        assert_eq!(sort.flipped().direction, SortDirection::Descending);
        assert_eq!(sort.flipped().flipped().direction, SortDirection::Ascending);
        assert!(sort.is("email"));
    }
}
