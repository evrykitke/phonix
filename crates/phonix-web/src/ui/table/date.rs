//! Narrowing a list to a span of time.
//!
//! # Why this is not a [`Filter`](super::Filter)
//!
//! A filter is one of a fixed set of choices, and its value is a word the
//! reader recognises. A range is neither: its choices are every pair of
//! instants, and its value has to mean the same thing to a browser, a server
//! function and a `WHERE` clause without any of them owning a calendar. So it
//! is declared separately, carried as two ordinary filter keys - see
//! [`DateRange`] - and drawn by a control of its own.
//!
//! # A name is resolved before it is sent
//!
//! "This week" is a question about a calendar, and the calendar that matters is
//! the one the viewer is looking at. [`DatePreset`] therefore turns a name into
//! two instants in the browser, and only those instants travel. The server is
//! never told a name and so can never disagree about when a week starts, and a
//! request stays reproducible - the same one asked again next month returns the
//! same rows, which "this month" would not.
//!
//! # Everything here is UTC
//!
//! The application stores instants in UTC and every grid renders them in UTC -
//! the `When` column of the audit trail says `2026-08-21 09:15` and means it.
//! So "today" is today in UTC too. Resolving it in the browser's local zone
//! would be more flattering and visibly wrong: a viewer three hours east would
//! press *Today* and get rows stamped with yesterday's date, under a control
//! that says otherwise. If the application ever renders local time, this is one
//! of the two places that changes; [`Cell::to_text`](super::Cell::to_text) is
//! the other.
//!
//! # A row with no timestamp is not in any range
//!
//! Once a span is chosen, a row whose instant is absent is excluded rather than
//! kept. "Between Monday and Friday" cannot include "never", and keeping such
//! rows would make the narrowest range still show them.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use chrono::{Datelike, Days, Months, NaiveDate};
use phonix_core::query::{DateRange, PageRequest};
use phonix_core::{Message, msg};

/// A span with a name, resolved against whatever day it is.
///
/// The variants are the whole vocabulary. A configuration chooses which of them
/// to offer - a list of things that happened cannot usefully be narrowed to
/// "last year" in its first week of life - and [`DatePreset::COMMON`] is the
/// set most lists want.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DatePreset {
    Today,
    Yesterday,
    /// Monday of this week up to next Monday. See the module docs on Mondays.
    ThisWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
    ThisYear,
    LastYear,
}

impl DatePreset {
    /// Every name, in the order a panel lists them: shortest span first, so
    /// the one most lists are opened for is the one nearest the pointer.
    pub const ALL: &'static [Self] = &[
        Self::Today,
        Self::Yesterday,
        Self::ThisWeek,
        Self::LastWeek,
        Self::ThisMonth,
        Self::LastMonth,
        Self::ThisYear,
        Self::LastYear,
    ];

    /// What a list offers unless it says otherwise.
    ///
    /// `LastMonth` is left out: between "last week" and "this month" it is
    /// rarely the question being asked, and a panel of eight buttons is one
    /// nobody reads to the end of.
    pub const COMMON: &'static [Self] = &[
        Self::Today,
        Self::Yesterday,
        Self::ThisWeek,
        Self::LastWeek,
        Self::ThisMonth,
        Self::ThisYear,
        Self::LastYear,
    ];

    /// What a panel calls this span.
    pub fn label(self) -> Message {
        match self {
            Self::Today => msg!("date.preset.today"),
            Self::Yesterday => msg!("date.preset.yesterday"),
            Self::ThisWeek => msg!("date.preset.this_week"),
            Self::LastWeek => msg!("date.preset.last_week"),
            Self::ThisMonth => msg!("date.preset.this_month"),
            Self::LastMonth => msg!("date.preset.last_month"),
            Self::ThisYear => msg!("date.preset.this_year"),
            Self::LastYear => msg!("date.preset.last_year"),
        }
    }

    /// The span this name means, given what day it is.
    ///
    /// `today` is a parameter rather than a call to the clock so that this is a
    /// pure function - which is what lets the awkward cases (the week that
    /// starts in the previous month, the year that ends the day after
    /// tomorrow) be tested instead of clicked at.
    ///
    /// A calendar arithmetic that overflows - a hundred thousand years from
    /// now - leaves that end unbounded rather than failing, which narrows less
    /// than asked and never panics.
    pub fn resolve(self, today: NaiveDate) -> DateRange {
        let monday =
            || today.checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())));
        let first_of_month = || today.with_day(1);
        let first_of_year = || today.with_month(1).and_then(|date| date.with_day(1));

        let week = Days::new(7);
        let month = Months::new(1);
        let year = Months::new(12);

        match self {
            Self::Today => span(Some(today), today.checked_add_days(Days::new(1))),
            Self::Yesterday => span(today.checked_sub_days(Days::new(1)), Some(today)),

            Self::ThisWeek => span(
                monday(),
                monday().and_then(|day| day.checked_add_days(week)),
            ),
            Self::LastWeek => span(
                monday().and_then(|day| day.checked_sub_days(week)),
                monday(),
            ),

            Self::ThisMonth => span(
                first_of_month(),
                first_of_month().and_then(|day| day.checked_add_months(month)),
            ),
            Self::LastMonth => span(
                first_of_month().and_then(|day| day.checked_sub_months(month)),
                first_of_month(),
            ),

            Self::ThisYear => span(
                first_of_year(),
                first_of_year().and_then(|day| day.checked_add_months(year)),
            ),
            Self::LastYear => span(
                first_of_year().and_then(|day| day.checked_sub_months(year)),
                first_of_year(),
            ),
        }
    }

    /// The name of `range`, if any of `presets` means exactly it.
    ///
    /// This is how a control labels itself. Keeping the name out of the state
    /// and deriving it back is what stops the two disagreeing: there is no
    /// pressed button to leave lit after the range underneath it has been
    /// edited by hand, and a panel left open past midnight relabels itself from
    /// "Today" to a pair of dates, which is the truth.
    pub fn naming(presets: &[Self], range: DateRange, today: NaiveDate) -> Option<Self> {
        (!range.is_any())
            .then(|| {
                presets
                    .iter()
                    .copied()
                    .find(|preset| preset.resolve(today) == range)
            })
            .flatten()
    }
}

/// A span of days as a span of instants, either end of which may be missing.
fn span(from: Option<NaiveDate>, to: Option<NaiveDate>) -> DateRange {
    DateRange::new(from.map(midnight), to.map(midnight))
}

/// The instant a day begins.
pub fn midnight(day: NaiveDate) -> DateTime<Utc> {
    day.and_time(chrono::NaiveTime::MIN).and_utc()
}

/// Where a row's instant is, for a grid that answers its own questions.
type At<T> = Arc<dyn Fn(&T) -> Option<DateTime<Utc>> + Send + Sync>;

/// A span of time offered above the table.
///
/// Declared once in a configuration and answered wherever the rows are, exactly
/// as a [`Filter`](super::Filter) is:
///
/// * [`Source::in_memory`](super::Source::in_memory) - say which instant to
///   compare with [`DateFilter::at`].
/// * [`Source::paged`](super::Source::paged) - say nothing, and the reader
///   turns the two keys into a `WHERE`. A closure here could only narrow the
///   page already fetched, which is not what a date range means.
pub struct DateFilter<T: 'static> {
    /// The name the reader knows it by. It occupies `{key}_from` and
    /// `{key}_to` in [`PageRequest::filters`].
    pub(crate) key: &'static str,
    pub(crate) label: String,
    pub(crate) presets: &'static [DatePreset],
    /// Whether the viewer may pick a time of day as well as a date.
    pub(crate) with_time: bool,
    pub(crate) at: Option<At<T>>,
}

impl<T: 'static> Clone for DateFilter<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            label: self.label.clone(),
            presets: self.presets,
            with_time: self.with_time,
            at: self.at.clone(),
        }
    }
}

impl<T: 'static> DateFilter<T> {
    /// A range offering [`DatePreset::COMMON`].
    ///
    /// ```ignore
    /// DateFilter::new("occurred", "When")
    /// ```
    ///
    /// `key` should name the thing that happened rather than the column - the
    /// wire carries `occurred_from`, and a reader asked for `occurred_at_from`
    /// reads as though the column had grown a suffix.
    pub fn new(key: &'static str, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
            presets: DatePreset::COMMON,
            with_time: false,
            at: None,
        }
    }

    /// Offer these names instead of the usual ones. An empty slice offers none,
    /// leaving the calendar alone.
    #[must_use]
    pub const fn presets(mut self, presets: &'static [DatePreset]) -> Self {
        self.presets = presets;
        self
    }

    /// Let the viewer pick a time of day, not only a date.
    ///
    /// Worth it for a list where a day is too coarse to be a useful answer - a
    /// trail of what happened during one incident. Most lists are read by the
    /// day and the extra precision is two more fields to tab past.
    #[must_use]
    pub const fn with_time(mut self) -> Self {
        self.with_time = true;
        self
    }

    /// Which instant on the row this range is about. Required for an in-memory
    /// grid, meaningless on a paged one.
    ///
    /// `None` from the closure means the row has no such instant - an account
    /// that has never signed in - and such a row is outside every range.
    #[must_use]
    pub fn at(mut self, at: impl Fn(&T) -> Option<DateTime<Utc>> + Send + Sync + 'static) -> Self {
        self.at = Some(Arc::new(at));
        self
    }

    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// Whether this range can be answered without asking the server.
    pub const fn is_local(&self) -> bool {
        self.at.is_some()
    }

    /// Whether this row falls inside whatever span `request` carries.
    ///
    /// True when no span was chosen, and true when there is no closure to ask -
    /// a paged grid was narrowed by the server, and narrowing it again here
    /// would drop rows it was right to send.
    pub fn accepts(&self, row: &T, request: &PageRequest) -> bool {
        let Some(at) = &self.at else {
            return true;
        };

        let range = request.range(self.key);

        range.is_any() || at(row).is_some_and(|when| range.contains(when))
    }
}

/// One range, as the bar needs to know it.
///
/// The same split [`FilterControl`](super::toolbar::FilterControl) makes, for
/// the same reason: the closure that answers the question cannot travel to a
/// toolbar that has never heard of the row type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateControl {
    pub key: &'static str,
    pub label: String,
    pub presets: &'static [DatePreset],
    pub with_time: bool,
}

impl<T: 'static> From<&DateFilter<T>> for DateControl {
    fn from(filter: &DateFilter<T>) -> Self {
        Self {
            key: filter.key,
            label: filter.label.clone(),
            presets: filter.presets,
            with_time: filter.with_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    /// A Friday.
    fn friday() -> NaiveDate {
        date(2026, 8, 21)
    }

    fn resolved(preset: DatePreset) -> (Option<NaiveDate>, Option<NaiveDate>) {
        let range = preset.resolve(friday());

        (
            range.from.map(|at| at.date_naive()),
            range.to.map(|at| at.date_naive()),
        )
    }

    #[test]
    fn today_is_one_day_and_the_day_after_it_is_not_in_it() {
        let range = DatePreset::Today.resolve(friday());

        assert_eq!(
            range,
            DateRange::between(midnight(friday()), midnight(date(2026, 8, 22)))
        );
        assert!(range.contains(midnight(friday())));
        assert!(!range.contains(midnight(date(2026, 8, 22))));
    }

    #[test]
    fn yesterday_ends_where_today_begins() {
        assert_eq!(
            DatePreset::Yesterday.resolve(friday()).to,
            DatePreset::Today.resolve(friday()).from,
        );
        assert_eq!(
            resolved(DatePreset::Yesterday),
            (Some(date(2026, 8, 20)), Some(friday()))
        );
    }

    #[test]
    fn a_week_runs_monday_to_monday_and_contains_the_day_it_was_resolved_on() {
        // 21 August 2026 is a Friday; its Monday is the 17th.
        assert_eq!(
            resolved(DatePreset::ThisWeek),
            (Some(date(2026, 8, 17)), Some(date(2026, 8, 24))),
        );
        assert!(
            DatePreset::ThisWeek
                .resolve(friday())
                .contains(midnight(friday()))
        );
    }

    #[test]
    fn a_week_resolved_on_its_own_monday_starts_that_day_rather_than_a_week_early() {
        let monday = date(2026, 8, 17);
        let range = DatePreset::ThisWeek.resolve(monday);

        assert_eq!(range.from, Some(midnight(monday)));
        assert!(range.contains(midnight(monday)));
    }

    #[test]
    fn a_week_resolved_on_a_sunday_is_the_week_that_sunday_ends() {
        // The Monday-first convention's one counterintuitive case: on Sunday
        // the 23rd, "this week" is the 17th to the 24th, not the 24th onwards.
        let range = DatePreset::ThisWeek.resolve(date(2026, 8, 23));

        assert_eq!(range.from, Some(midnight(date(2026, 8, 17))));
        assert!(range.contains(midnight(date(2026, 8, 23))));
    }

    #[test]
    fn last_week_ends_where_this_week_begins() {
        assert_eq!(
            resolved(DatePreset::LastWeek),
            (Some(date(2026, 8, 10)), Some(date(2026, 8, 17))),
        );
        assert_eq!(
            DatePreset::LastWeek.resolve(friday()).to,
            DatePreset::ThisWeek.resolve(friday()).from,
        );
    }

    #[test]
    fn a_month_is_the_first_to_the_first() {
        assert_eq!(
            resolved(DatePreset::ThisMonth),
            (Some(date(2026, 8, 1)), Some(date(2026, 9, 1))),
        );
        assert_eq!(
            resolved(DatePreset::LastMonth),
            (Some(date(2026, 7, 1)), Some(date(2026, 8, 1))),
        );
    }

    #[test]
    fn a_month_that_follows_a_shorter_one_still_ends_on_a_first() {
        // Resolved on 31 March: adding a month to the 31st would be 31 April.
        // The arithmetic runs from the 1st, so it cannot arise - and this is
        // the test that says so.
        let range = DatePreset::ThisMonth.resolve(date(2026, 3, 31));

        assert_eq!(range.from, Some(midnight(date(2026, 3, 1))));
        assert_eq!(range.to, Some(midnight(date(2026, 4, 1))));
    }

    #[test]
    fn last_month_across_a_year_boundary_is_december() {
        let range = DatePreset::LastMonth.resolve(date(2026, 1, 9));

        assert_eq!(range.from, Some(midnight(date(2025, 12, 1))));
        assert_eq!(range.to, Some(midnight(date(2026, 1, 1))));
    }

    #[test]
    fn a_year_is_january_to_january() {
        assert_eq!(
            resolved(DatePreset::ThisYear),
            (Some(date(2026, 1, 1)), Some(date(2027, 1, 1))),
        );
        assert_eq!(
            resolved(DatePreset::LastYear),
            (Some(date(2025, 1, 1)), Some(date(2026, 1, 1))),
        );
    }

    #[test]
    fn a_leap_day_is_inside_the_year_that_has_one() {
        let range = DatePreset::ThisYear.resolve(date(2028, 6, 1));

        assert!(range.contains(midnight(date(2028, 2, 29))));
    }

    #[test]
    fn every_preset_is_a_span_with_both_ends() {
        // A name that resolved to something half-bounded would narrow far more
        // than it says, and the control has no way to show that it had.
        for preset in DatePreset::ALL {
            let range = preset.resolve(friday());

            assert!(
                range.from.is_some() && range.to.is_some(),
                "{}",
                preset.label()
            );
            assert!(!range.is_impossible(), "{}", preset.label());
        }
    }

    #[test]
    fn a_range_that_is_exactly_a_preset_is_labelled_by_it() {
        let range = DatePreset::ThisWeek.resolve(friday());

        assert_eq!(
            DatePreset::naming(DatePreset::COMMON, range, friday()),
            Some(DatePreset::ThisWeek),
        );
    }

    #[test]
    fn a_range_a_day_off_a_preset_is_not_labelled_by_it() {
        let range = DateRange::between(midnight(friday()), midnight(date(2026, 8, 23)));

        assert_eq!(
            DatePreset::naming(DatePreset::COMMON, range, friday()),
            None
        );
    }

    #[test]
    fn a_preset_the_configuration_did_not_offer_does_not_name_a_range() {
        let range = DatePreset::LastMonth.resolve(friday());

        // `COMMON` leaves `LastMonth` out, so a range that happens to be last
        // month is shown as its dates rather than under a name nothing offered.
        assert_eq!(
            DatePreset::naming(DatePreset::COMMON, range, friday()),
            None
        );
        assert_eq!(
            DatePreset::naming(DatePreset::ALL, range, friday()),
            Some(DatePreset::LastMonth),
        );
    }

    #[test]
    fn nothing_names_the_whole_of_time() {
        assert_eq!(
            DatePreset::naming(DatePreset::ALL, DateRange::ANY, friday()),
            None
        );
    }

    // --- the filter itself ---------------------------------------------

    #[derive(Clone)]
    struct Row(Option<DateTime<Utc>>);

    fn occurred() -> DateFilter<Row> {
        DateFilter::new("occurred", "When").at(|row: &Row| row.0)
    }

    fn on(day: NaiveDate) -> Row {
        Row(Some(midnight(day)))
    }

    #[test]
    fn a_range_nobody_chose_keeps_every_row() {
        let request = PageRequest::first(10);

        assert!(occurred().accepts(&on(friday()), &request));
        assert!(occurred().accepts(&Row(None), &request));
    }

    #[test]
    fn a_chosen_range_keeps_the_rows_inside_it() {
        let request =
            PageRequest::first(10).in_range("occurred", DatePreset::ThisWeek.resolve(friday()));

        assert!(occurred().accepts(&on(friday()), &request));
        assert!(occurred().accepts(&on(date(2026, 8, 17)), &request));
        assert!(!occurred().accepts(&on(date(2026, 8, 24)), &request));
        assert!(!occurred().accepts(&on(date(2026, 8, 16)), &request));
    }

    #[test]
    fn a_row_with_no_instant_is_outside_every_chosen_range() {
        let request =
            PageRequest::first(10).in_range("occurred", DatePreset::ThisYear.resolve(friday()));

        assert!(!occurred().accepts(&Row(None), &request));
    }

    #[test]
    fn a_range_with_no_closure_keeps_every_row_it_is_handed() {
        // The paged case. Narrowing the page again here would throw away rows
        // the server was right to send.
        let filter: DateFilter<Row> = DateFilter::new("occurred", "When");
        let request =
            PageRequest::first(10).in_range("occurred", DatePreset::Today.resolve(friday()));

        assert!(filter.accepts(&on(date(2020, 1, 1)), &request));
        assert!(!filter.is_local());
    }

    #[test]
    fn a_range_opens_offering_the_usual_names_and_no_clock() {
        let filter: DateFilter<Row> = DateFilter::new("occurred", "When");

        assert_eq!(filter.presets, DatePreset::COMMON);
        assert!(!filter.with_time);
        assert!(filter.with_time().with_time);
    }
}
