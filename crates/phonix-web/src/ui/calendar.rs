//! A month, as a grid of days.
//!
//! # What it is and is not
//!
//! [`MonthCalendar`] draws one month and reports which day was pressed. It
//! holds no selection, no range and no idea what a range is for: the caller
//! says which month to show, which days to mark, and what to do with a press.
//! That is what lets the same component serve a filter's range picker, and
//! later a single-date field on a form, without either of them growing a
//! second calendar.
//!
//! # It is never told what day it is
//!
//! `today` is a prop. Reading the clock inside a component would make its
//! markup depend on when it rendered, and this application renders every screen
//! twice - once on the server, once again in the browser to hydrate it. A
//! calendar that highlighted a different day in each render is a hydration
//! mismatch, and in a wasm bundle that is not a warning but a dead page.
//!
//! The caller reads the clock, in a browser, from an event handler - which is
//! also why nothing here appears in the server's markup: the panel this sits in
//! does not exist until somebody opens it.
//!
//! # Six rows, always
//!
//! A month occupies four to six weeks depending on where its first day falls.
//! Sized to fit, the panel would change height as the arrows are pressed, and
//! the arrows would move out from under the pointer. So the grid is always six
//! rows of seven, with the days either side of the month drawn faintly rather
//! than left blank - which also puts the last days of the previous month one
//! click away when the range starts in them.
//!
//! # Weeks begin on Monday
//!
//! One convention, chosen once, matching [`DatePreset::ThisWeek`]. A calendar
//! whose weeks start on Sunday beside a "this week" that means Monday to Sunday
//! is two answers to the same question on one panel.
//!
//! [`DatePreset::ThisWeek`]: crate::ui::table::DatePreset::ThisWeek

use chrono::{Datelike, Days, Months, NaiveDate};
use leptos::prelude::*;

use phonix_core::i18n::datetime;

use crate::i18n::{Locale, t};
use crate::icons::{Icon, IconSize};
use crate::l;

/// How many days one panel draws. Six weeks - see the module docs.
const CELLS: u64 = 42;

/// The column headings, Monday first.
///
/// The order is a design decision, not a locale one: this panel picks spans,
/// and a week that starts on Sunday would put the weekend at both ends of it.
/// Only the two letters come from the catalog.
const WEEKDAYS: [chrono::Weekday; 7] = [
    chrono::Weekday::Mon,
    chrono::Weekday::Tue,
    chrono::Weekday::Wed,
    chrono::Weekday::Thu,
    chrono::Weekday::Fri,
    chrono::Weekday::Sat,
    chrono::Weekday::Sun,
];

/// The forty-two days a panel showing `month` draws, starting on a Monday.
///
/// Empty only if the calendar has run off the end of what a date can express,
/// which is a few hundred thousand years away and renders as a blank grid
/// rather than as a panic.
pub fn cells(month: NaiveDate) -> Vec<NaiveDate> {
    let Some(first) = month.with_day(1) else {
        return Vec::new();
    };

    let back = u64::from(first.weekday().num_days_from_monday());
    let Some(start) = first.checked_sub_days(Days::new(back)) else {
        return Vec::new();
    };

    (0..CELLS)
        .filter_map(|n| start.checked_add_days(Days::new(n)))
        .collect()
}

/// The first of the month `date` falls in - what the calendar is positioned by.
pub fn month_of(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}

/// The month `count` before or after this one, or this one if the calendar has
/// run out of years.
fn shifted(month: NaiveDate, forward: bool) -> NaiveDate {
    let step = Months::new(1);
    let moved = if forward {
        month.checked_add_months(step)
    } else {
        month.checked_sub_months(step)
    };

    moved.unwrap_or(month)
}

/// One month of days.
#[component]
pub fn month_calendar(
    /// The month on show. Owned by the caller so that opening the panel can
    /// jump it to the range already chosen.
    month: RwSignal<NaiveDate>,
    /// The first day of the marked span.
    start: Signal<Option<NaiveDate>>,
    /// The last day of the marked span, included. `None` while only a start has
    /// been pressed.
    end: Signal<Option<NaiveDate>>,
    /// What the caller believes today is. See the module docs.
    today: NaiveDate,
    on_pick: Callback<NaiveDate>,
) -> impl IntoView {
    // Held rather than read inside the closure: the heading re-renders on
    // every arrow press and the days are drawn forty-two at a time.
    let words = Locale::get().shared();
    let heading = words.clone();
    let title = move || datetime::month_year(&heading, month.get());

    view! {
        <div class="w-[17.5rem] max-w-full">
            <div class="flex items-center justify-between gap-1 pb-1">
                <MonthStep month=month forward=false />
                // `tabular-nums` so the title does not shift sideways between
                // a month whose name is long and one whose name is short.
                <span class="text-sm font-medium text-content tabular-nums">{title}</span>
                <MonthStep month=month forward=true />
            </div>

            <div class="grid grid-cols-7 gap-0.5 pb-1">
                {WEEKDAYS
                    .into_iter()
                    .map(|day| {
                        let heading = t(&datetime::weekday_short(day));

                        view! {
                            <span class="grid h-6 place-items-center text-2xs font-medium uppercase tracking-wide text-content-subtle">
                                {heading}
                            </span>
                        }
                    })
                    .collect::<Vec<_>>()}
            </div>

            // Keyed on the month, so stepping to the next one replaces the
            // whole grid rather than re-deciding forty-two classes in place.
            <div class="grid grid-cols-7 gap-0.5" role="grid">
                {move || {
                    let shown = month.get();

                    cells(shown)
                        .into_iter()
                        .map(|day| {
                            view! {
                                <DayCell
                                    day=day
                                    shown=shown
                                    today=today
                                    start=start
                                    end=end
                                    on_pick=on_pick
                                    words=words.clone()
                                />
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </div>
        </div>
    }
}

/// One arrow.
#[component]
fn month_step(month: RwSignal<NaiveDate>, forward: bool) -> impl IntoView {
    let label = if forward {
        l!("calendar.next_month")
    } else {
        l!("calendar.previous_month")
    };
    let icon = if forward {
        Icon::ChevronRight
    } else {
        Icon::ChevronLeft
    };

    view! {
        <button
            type="button"
            class="grid size-7 shrink-0 place-items-center rounded-control text-content-muted hover:bg-surface-hover hover:text-content"
            aria-label=label.clone()
            title=label
            on:click=move |_| month.update(|shown| *shown = shifted(*shown, forward))
        >
            <Icon icon=icon size=IconSize::Xs />
        </button>
    }
}

/// One day.
#[component]
fn day_cell(
    day: NaiveDate,
    /// The month the panel is showing, so the days either side can be quieter.
    shown: NaiveDate,
    today: NaiveDate,
    start: Signal<Option<NaiveDate>>,
    end: Signal<Option<NaiveDate>>,
    on_pick: Callback<NaiveDate>,
    /// Passed down rather than read here: forty-two cells reading the same
    /// context is forty-two lookups to reach one `Arc`.
    words: std::sync::Arc<phonix_core::i18n::Catalog>,
) -> impl IntoView {
    let outside = day.month() != shown.month();

    // An end of the span, either end of it. Both are drawn the same: which one
    // was pressed first is not something the viewer should have to remember.
    let edge = move || start.get() == Some(day) || end.get() == Some(day);
    let within = move || match (start.get(), end.get()) {
        (Some(from), Some(to)) => day > from && day < to,
        _ => false,
    };

    let class = move || {
        let base = "grid h-8 place-items-center rounded-control text-xs tabular-nums";

        if edge() {
            format!("{base} bg-brand font-medium text-on-brand")
        } else if within() {
            format!("{base} bg-brand-subtle text-content")
        } else if outside {
            format!("{base} text-content-subtle hover:bg-surface-hover")
        } else if day == today {
            // Ringed rather than filled: today is a landmark, not a selection,
            // and filling it would read as a day already chosen.
            format!(
                "{base} font-medium text-brand ring-1 ring-inset ring-edge-strong hover:bg-surface-hover"
            )
        } else {
            format!("{base} text-content hover:bg-surface-hover")
        }
    };

    view! {
        <button
            type="button"
            class=class
            role="gridcell"
            aria-label=datetime::day_long(&words, day)
            aria-current=move || if day == today { "date" } else { "false" }
            aria-pressed=move || if edge() || within() { "true" } else { "false" }
            on:click=move |_| on_pick.run(day)
        >
            {day.day()}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    #[test]
    fn a_panel_is_always_six_weeks() {
        // Otherwise the panel changes height as the arrows are pressed, and
        // the arrow moves out from under the pointer that is pressing it.
        for month in 1..=12 {
            assert_eq!(cells(date(2026, month, 1)).len(), 42);
        }
    }

    #[test]
    fn a_panel_starts_on_the_monday_on_or_before_the_first() {
        // 1 August 2026 is a Saturday, so the panel opens on 27 July.
        let cells = cells(date(2026, 8, 1));

        assert_eq!(cells.first(), Some(&date(2026, 7, 27)));
        assert_eq!(cells.last(), Some(&date(2026, 9, 6)));
    }

    #[test]
    fn a_month_that_begins_on_a_monday_begins_the_grid() {
        // 1 June 2026 is a Monday, so there is no earlier week to show and the
        // grid runs a fortnight into July instead. The last days of May are
        // reached with the arrow, as they are in every other calendar.
        let cells = cells(date(2026, 6, 1));

        assert_eq!(cells.first(), Some(&date(2026, 6, 1)));
        assert_eq!(cells.last(), Some(&date(2026, 7, 12)));
    }

    #[test]
    fn the_whole_month_fits_in_the_panel_whatever_day_it_starts_on() {
        // Six rows is enough for every month: 31 days beginning on a Sunday is
        // the worst case, at six days of padding plus 31.
        for (year, month) in [
            (2026, 1),
            (2026, 2),
            (2026, 6),
            (2026, 8),
            (2026, 11),
            (2028, 2),
        ] {
            let first = date(year, month, 1);
            let cells = cells(first);

            assert!(
                cells.contains(&first),
                "{first} is missing from its own panel"
            );

            let last = first
                .checked_add_months(Months::new(1))
                .and_then(|next| next.checked_sub_days(Days::new(1)))
                .expect("a real date");

            assert!(
                cells.contains(&last),
                "{last} is missing from its own panel"
            );
        }
    }

    #[test]
    fn every_cell_is_the_day_after_the_one_before_it() {
        let cells = cells(date(2026, 2, 1));

        for pair in cells.windows(2) {
            let [before, after] = pair else { continue };

            assert_eq!(before.checked_add_days(Days::new(1)), Some(*after));
        }
    }

    #[test]
    fn the_panel_names_its_month_and_its_days_from_the_catalog() {
        // What this replaced was a pair of chrono format strings, which chrono
        // does not check until it renders one - and a bad one panics inside a
        // view, where the only symptom is a page that has stopped responding.
        // Assembling from the catalog moves that failure to a missing key,
        // which the build already refuses.
        let words = phonix_core::i18n::Catalog::builtin(phonix_core::i18n::Language::ENGLISH);

        assert_eq!(
            datetime::month_year(&words, date(2026, 8, 21)),
            "August 2026"
        );
        assert_eq!(
            datetime::day_long(&words, date(2026, 8, 21)),
            "21 August 2026"
        );
    }

    #[test]
    fn a_day_is_spoken_without_the_padding_the_grid_shows() {
        // A screen reader saying "zero four August" is the reason the day is
        // not zero-padded, and dropping that is a silent regression.
        let words = phonix_core::i18n::Catalog::builtin(phonix_core::i18n::Language::ENGLISH);

        assert_eq!(
            datetime::day_long(&words, date(2026, 8, 4)),
            "4 August 2026"
        );
    }

    #[test]
    fn the_month_of_any_day_is_its_first() {
        assert_eq!(month_of(date(2026, 8, 21)), date(2026, 8, 1));
        assert_eq!(month_of(date(2026, 8, 1)), date(2026, 8, 1));
    }

    #[test]
    fn stepping_over_a_year_boundary_lands_in_the_right_year() {
        assert_eq!(shifted(date(2026, 12, 1), true), date(2027, 1, 1));
        assert_eq!(shifted(date(2026, 1, 1), false), date(2025, 12, 1));
    }

    #[test]
    fn stepping_from_the_end_of_a_long_month_does_not_invent_a_day() {
        // The calendar is always positioned on a first, so this cannot arise
        // in the panel - but `shifted` must not produce 31 February for a
        // caller that positions it elsewhere.
        assert_eq!(shifted(date(2026, 1, 31), true), date(2026, 2, 28));
    }
}
