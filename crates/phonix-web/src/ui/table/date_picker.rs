//! The control a [`DateFilter`] is drawn as: a button, and a panel behind it.
//!
//! ```text
//!  [ 17 Aug - 23 Aug 2026 v ]
//!  +--------------------------------------+
//!  | Today      |  <   August 2026    >   |
//!  | Yesterday  |  Mo Tu We Th Fr Sa Su   |
//!  | This week  |                27 28 29 |
//!  | Last week  |  30 31  1  2  3  4  5   |
//!  | This month |   6  7  8  9 10 11 12   |
//!  | This year  |  13 14 15 16 [17 18 19] |
//!  | Last year  | [20 21 22 23] 24 25 26  |
//!  +--------------------------------------+
//!  | From [2026-08-17]  To [2026-08-23]   |
//!  | Clear                           Done |
//!  +--------------------------------------+
//! ```
//!
//! # Three ways in, one thing chosen
//!
//! A name, a pair of clicks on the calendar, or two typed dates. All three
//! write the same thing - two instants, in [`GridState::set_range`] - and the
//! panel reads that back to decide what to show. There is no "which control was
//! used last" to keep straight, and no pressed button left lit over a range
//! that has since been edited underneath it: the name is *derived*, by asking
//! which preset resolves to exactly the span in force.
//!
//! # The calendar takes two clicks and sends one request
//!
//! The first click is an anchor and goes nowhere near the state. Committing it
//! would put a one-day range on the wire that nobody asked for, and the second
//! click would then have to be drawn over its answer. So the anchor lives in a
//! signal here, the panel marks it, and the request is made when the span has
//! both ends. Clicking the earlier day second is the same span - a calendar is
//! read in whichever direction the eye goes.
//!
//! # It never renders on the server
//!
//! The panel exists only while it is open, and it opens on a click, so its
//! markup is only ever built in a browser. That is what makes it safe for it to
//! know what day it is: a component that reads the clock while rendering
//! produces different markup on the server and in the browser, and a hydration
//! mismatch in a wasm bundle is a dead page rather than a warning.
//!
//! The button outside it is rendered on both sides, and says "Any time" on
//! both: a grid opens unnarrowed, so there is no range to format and no clock
//! to read until somebody has already pressed something.
//!
//! # Read the clock, do not remember it
//!
//! `today` is fetched where it is needed rather than captured when the control
//! is built. A list screen is left open across a lunch break and across
//! midnight; a "Today" resolved when the page loaded would quietly mean
//! yesterday by the time it is pressed.

use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys;
use phonix_core::i18n::datetime;
use phonix_core::query::DateRange;
use phonix_core::{Message, msg};

use super::date::{DateControl, DatePreset, midnight};
use super::state::GridState;
use crate::i18n::{Locale, t};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::calendar::{MonthCalendar, month_of};

/// What the button says, and what the clearing control offers, when no span is
/// in force. One key because the two must read as the same thing.
const ANY: &str = "date.any";

/// What the two fields at the foot of the panel accept.
const DATE_FORMAT: &str = "%Y-%m-%d";
/// The same field once it carries a time of day.
const MOMENT_FORMAT: &str = "%Y-%m-%dT%H:%M";
/// What `<input type="datetime-local">` produces, and the seconds some browsers
/// add to it.
const MOMENT_FORMATS: [&str; 2] = ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S"];

/// What day it is, in UTC - which is the zone every grid renders in. See the
/// note on zones in [`super::date`].
fn today() -> NaiveDate {
    Utc::now().date_naive()
}

/// Where the panel sits, in viewport coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct At {
    left: f64,
    top: Option<f64>,
    bottom: Option<f64>,
}

impl At {
    fn style(self) -> String {
        let vertical = match (self.top, self.bottom) {
            (Some(top), _) => format!("top:{top}px"),
            (_, Some(bottom)) => format!("bottom:{bottom}px"),
            _ => "top:0".to_owned(),
        };

        format!("position:fixed;left:{}px;{vertical}", self.left)
    }
}

/// A span of time, as a button and a panel.
#[component]
pub fn date_range_picker(state: GridState, control: DateControl) -> impl IntoView {
    let DateControl {
        key,
        label,
        presets,
        with_time,
    } = control;

    let open = RwSignal::new(false);
    let at = RwSignal::new(At::default());
    // Read on the server too, and deliberately never observable there: the
    // panel is behind a `Show` that is closed on both sides, and opening it
    // sets this before it can be rendered. Nothing in the server's markup
    // depends on it, which is the only reason a clock may be touched here.
    let month = RwSignal::new(month_of(today()));
    // The first of two clicks on the calendar. See the module docs.
    let anchor = RwSignal::new(None::<NaiveDate>);

    let trigger = NodeRef::<leptos::html::Button>::new();
    let panel = NodeRef::<leptos::html::Div>::new();

    let range = Signal::derive(move || state.range(key));
    let narrowed = Signal::derive(move || !range.get().is_any());

    // Everything that closes it, exactly as the row menu does it: a pointer
    // down that is neither the button nor the panel, Escape, and anything that
    // would move a fixed panel out from under its trigger.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }

        let outside = window_event_listener(leptos::ev::pointerdown, move |event| {
            let Some(target) = event.target() else {
                return;
            };
            let node = target.dyn_ref::<web_sys::Node>();

            let within = panel
                .get_untracked()
                .is_some_and(|panel| panel.contains(node))
                || trigger
                    .get_untracked()
                    .is_some_and(|trigger| trigger.contains(node));

            if !within {
                open.set(false);
            }
        });

        let escape = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                open.set(false);
            }
        });

        let scrolled = window_event_listener(leptos::ev::wheel, move |_| open.set(false));
        let resized = window_event_listener(leptos::ev::resize, move |_| open.set(false));

        on_cleanup(move || {
            outside.remove();
            escape.remove();
            scrolled.remove();
            resized.remove();
        });
    });

    // Committing anything closes the half-finished calendar pick with it: a
    // typed date or a pressed name replaces the span, and an anchor left over
    // from before would join the next click to a span that is no longer there.
    let commit = Callback::new(move |chosen: DateRange| {
        anchor.set(None);
        state.set_range(key, chosen.normalised());
    });

    let choose_day = Callback::new(move |day: NaiveDate| match anchor.get_untracked() {
        None => anchor.set(Some(day)),
        Some(first) => {
            let (from, to) = if day < first {
                (day, first)
            } else {
                (first, day)
            };

            commit.run(days(from, to));
        }
    });

    // While a pick is half made the anchor is the only day marked, so the
    // calendar shows what the next click will extend from rather than the span
    // that is about to be replaced.
    let start = Signal::derive(move || match anchor.get() {
        Some(day) => Some(day),
        None => range.get().from.map(|at| at.date_naive()),
    });
    let end = Signal::derive(move || match anchor.get() {
        Some(_) => None,
        None => last_day(range.get()),
    });

    view! {
        <div class="inline-flex">
            <button
                type="button"
                node_ref=trigger
                class=move || {
                    let base = "inline-flex h-8 max-w-[13rem] shrink-0 items-center gap-1.5 \
                                rounded-control border px-2.5 text-sm";
                    if narrowed.get() {
                        format!("{base} border-brand bg-brand-subtle text-content")
                    } else {
                        format!("{base} border-edge text-content-muted hover:bg-surface-hover hover:text-content")
                    }
                }
                aria-haspopup="dialog"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-label=label.clone()
                title=label.clone()
                on:click=move |_| {
                    if open.get_untracked() {
                        open.set(false);
                        return;
                    }
                    // Opened on the month the span starts in, so a range chosen
                    // in March is not reviewed against August.
                    month
                        .set(
                            month_of(
                                range.get_untracked().from.map_or_else(today, |at| at.date_naive()),
                            ),
                        );
                    anchor.set(None);
                    at.set(place(trigger));
                    open.set(true);
                }
            >
                <Icon icon=Icon::Calendar size=IconSize::Xs class="shrink-0" />
                <span class="truncate-fade">
                    // The clock is read only once there is a span to describe,
                    // and a grid opens with none. That ordering is the whole
                    // reason this button is safe to render on the server:
                    // `Utc::now()` during a render is how the two renders come
                    // out different, and a hydration mismatch here is a frozen
                    // page rather than a wrong date. `summarise` answers "Any
                    // time" without consulting `today` either, so this is
                    // belt and braces - deliberately.
                    {
                        let words = Locale::get().shared();

                        move || {
                            let range = range.get();

                            if range.is_any() {
                                t(&Message::new(ANY))
                            } else {
                                summarise(&words, range, presets, today())
                            }
                        }
                    }
                </span>
            </button>

            // A `Show` rather than a hidden panel, which is the opposite of
            // what the row menu had to do. The difference is where each one
            // sits: a row is inside the grid's `Suspend` and hydrates
            // asynchronously, so a node appearing on a click there is a node
            // leptos tries to hydrate against a comment marker. The toolbar is
            // outside it and hydrates eagerly, so this is an ordinary
            // client-side create. `ColumnMenu`, three components along in this
            // same bar, has been doing it this way all along.
            <Show when=move || open.get() fallback=|| ()>
                <div
                    node_ref=panel
                    role="dialog"
                    aria-label=label.clone()
                    class="alert-enter z-[55] w-[min(20rem,calc(100vw-1rem))] rounded-card border border-edge bg-surface-raised p-2 shadow-pop sm:w-auto"
                    style=move || at.get().style()
                >
                    <div class="flex flex-col gap-2 sm:flex-row sm:gap-3">
                        {(!presets.is_empty())
                            .then(|| {
                                view! {
                                    <Presets
                                        presets=presets
                                        range=range
                                        month=month
                                        commit=commit
                                    />
                                }
                            })}

                        <MonthCalendar
                            month=month
                            start=start
                            end=end
                            today=today()
                            on_pick=choose_day
                        />
                    </div>

                    <Ends key=key range=range with_time=with_time commit=commit />

                    <div class="flex items-center justify-between gap-2 pt-2">
                        <button
                            type="button"
                            class="text-xs font-medium text-brand hover:underline disabled:text-content-subtle disabled:no-underline"
                            disabled=move || !narrowed.get()
                            on:click=move |_| commit.run(DateRange::ANY)
                        >
                            {l!("date.any")}
                        </button>
                        <button
                            type="button"
                            class="inline-flex h-7 items-center rounded-control border border-edge px-2.5 text-xs text-content-muted hover:bg-surface-hover hover:text-content"
                            on:click=move |_| open.set(false)
                        >
                            {l!("common.done")}
                        </button>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// The named spans, down the side of the panel.
#[component]
fn presets(
    presets: &'static [DatePreset],
    range: Signal<DateRange>,
    month: RwSignal<NaiveDate>,
    commit: Callback<DateRange>,
) -> impl IntoView {
    // Wrapped chips on a phone, a column beside the calendar from `sm` up: a
    // seven-row column next to a six-row calendar is taller than the calendar,
    // and on a narrow screen it would be all anyone saw.
    view! {
        <div class="flex flex-wrap gap-1 sm:w-[7.5rem] sm:flex-col sm:flex-nowrap">
            {presets
                .iter()
                .copied()
                .map(|preset| {
                    let chosen = move || {
                        DatePreset::naming(presets, range.get(), today()) == Some(preset)
                    };

                    view! {
                        <button
                            type="button"
                            class=move || {
                                let base = "rounded-control px-2 py-1 text-start text-xs";
                                if chosen() {
                                    format!("{base} bg-brand font-medium text-on-brand")
                                } else {
                                    format!("{base} text-content-muted hover:bg-surface-hover hover:text-content")
                                }
                            }
                            aria-pressed=move || if chosen() { "true" } else { "false" }
                            on:click=move |_| {
                                let chosen = preset.resolve(today());

                                if let Some(from) = chosen.from {
                                    month.set(month_of(from.date_naive()));
                                }

                                commit.run(chosen);
                            }
                        >
                            {t(&preset.label())}
                        </button>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// The two ends, as fields.
///
/// Typed rather than pressed, which is the only way to reach a span the
/// calendar would take a lot of scrolling to select - "since we went live",
/// three years back.
///
/// The browser's own control, and deliberately so. A hand-built segmented
/// field would look more like the rest of the panel and would be worse at the
/// three things that matter here: it would replace the OS wheel picker on a
/// phone, it would have to choose one segment order for every locale, and it
/// would be several hundred lines of keyboard handling in a bundle where a
/// panic freezes the tab.
///
/// It keeps its calendar button too. Suppressing that leaves a field that can
/// only be typed into, which is a worse control than the one it replaced - and
/// a second calendar here is not a duplicate: this one sets one end exactly,
/// where the panel's sets a span.
#[component]
fn ends(
    key: &'static str,
    range: Signal<DateRange>,
    with_time: bool,
    commit: Callback<DateRange>,
) -> impl IntoView {
    let kind = if with_time { "datetime-local" } else { "date" };

    // The stored `to` is exclusive, and a field showing the 24th for a span
    // that ends on the 23rd is a field that argues with the button above it.
    // So the date-only field shows and accepts the last day *included*, and
    // the exclusive edge stays where it belongs - on the wire.
    let shown = move |end: bool| {
        let range = range.get();

        match (end, with_time) {
            (false, _) => range.from.map(|at| stamp(at, with_time)),
            (true, true) => range.to.map(|at| stamp(at, true)),
            (true, false) => last_day(range).map(|day| day.format(DATE_FORMAT).to_string()),
        }
    };

    let typed = move |end: bool, text: String| {
        let range = range.get_untracked();
        let parsed = parse(&text, with_time);

        // An unparseable field is an empty one - a half-typed year is not a
        // reason to narrow to something nobody asked for.
        let chosen = match (end, with_time) {
            (false, _) => DateRange {
                from: parsed,
                ..range
            },
            (true, true) => DateRange {
                to: parsed,
                ..range
            },
            // The inclusive day the viewer typed, turned back into the
            // midnight after it.
            (true, false) => DateRange {
                to: parsed
                    .and_then(|at| at.date_naive().checked_add_days(Days::new(1)))
                    .map(midnight),
                ..range
            },
        };

        commit.run(chosen);
    };

    view! {
        <div class="mt-2 grid grid-cols-2 gap-2 border-t border-edge pt-2">
            {[(false, l!("date.from")), (true, l!("date.to"))]
                .into_iter()
                .map(|(end, label)| {
                    let id = format!("{key}-{}", if end { "to" } else { "from" });

                    view! {
                        <label class="flex flex-col gap-1">
                            <span class="text-2xs font-medium uppercase tracking-wide text-content-subtle">
                                {label}
                            </span>
                            // The browser's own control, whole - its calendar
                            // button included. The two calendars are not in
                            // competition: the panel's picks a span, this one
                            // picks one precise end of it.
                            <input
                                type=kind
                                id=id.clone()
                                class="h-7 w-full text-xs"
                                prop:value=move || shown(end).unwrap_or_default()
                                on:change=move |event| typed(end, event_target_value(&event))
                            />
                        </label>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// The span covering `from` to `to`, both days included.
fn days(from: NaiveDate, to: NaiveDate) -> DateRange {
    DateRange::new(
        Some(midnight(from)),
        to.checked_add_days(Days::new(1)).map(midnight),
    )
}

/// The last day a span includes, given that its end is exclusive.
///
/// A span ending at midnight ends on the day before; one ending at nine in the
/// morning ends on that day. One subtraction covers both, and it is the reason
/// the exclusive edge never has to be explained to anybody looking at the
/// panel.
fn last_day(range: DateRange) -> Option<NaiveDate> {
    range
        .to
        .and_then(|at| at.checked_sub_signed(TimeDelta::nanoseconds(1)))
        .map(|at| at.date_naive())
}

/// One end as a field shows it.
fn stamp(at: DateTime<Utc>, with_time: bool) -> String {
    let format = if with_time {
        MOMENT_FORMAT
    } else {
        DATE_FORMAT
    };

    at.format(format).to_string()
}

/// What a field was typed into, as an instant.
///
/// A `datetime-local` field has no zone in it, and the value is read as UTC -
/// the zone this application stores and renders everything in, so that a viewer
/// who types 14:30 and a row that displays 14:30 mean the same moment.
fn parse(text: &str, with_time: bool) -> Option<DateTime<Utc>> {
    let text = text.trim();

    if text.is_empty() {
        return None;
    }

    if !with_time {
        return NaiveDate::parse_from_str(text, DATE_FORMAT)
            .ok()
            .map(midnight);
    }

    MOMENT_FORMATS
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .map(|at| at.and_utc())
}

/// What the button says.
///
/// A name when one fits exactly, the dates otherwise. Deriving the name rather
/// than remembering which button was pressed is what keeps the two from
/// disagreeing - including across midnight, when a span that was "Today" when
/// it was chosen correctly starts calling itself by its date.
fn summarise(
    catalog: &phonix_core::i18n::Catalog,
    range: DateRange,
    presets: &[DatePreset],
    today: NaiveDate,
) -> String {
    if range.is_any() {
        return catalog.render(&Message::new(ANY));
    }

    if let Some(preset) = DatePreset::naming(presets, range, today) {
        return catalog.render(&preset.label());
    }

    // This year's dates lose the year: it is the same on almost every row on
    // the screen, and the button has one line to say everything in.
    let day = |date: NaiveDate| {
        if date.year() == today.year() {
            datetime::day_short_no_year(catalog, date)
        } else {
            datetime::day_short(catalog, date)
        }
    };

    match (range.from.map(|at| at.date_naive()), last_day(range)) {
        (Some(from), Some(to)) if from == to => day(from),
        (Some(from), Some(to)) => {
            catalog.render(&msg!("date.span.between", from = day(from), to = day(to)))
        }
        (Some(from), None) => catalog.render(&msg!("date.span.from", day = day(from))),
        (None, Some(to)) => catalog.render(&msg!("date.span.until", day = day(to))),
        // Unreachable: a range with neither end is `ANY`, answered above.
        (None, None) => catalog.render(&Message::new(ANY)),
    }
}

/// Where the panel should sit, measured from its button.
///
/// Browser only, like the row menu's: measuring needs interfaces this crate
/// asks for in the hydrate build alone, and the server never opens a panel.
#[cfg(feature = "hydrate")]
fn place(trigger: NodeRef<leptos::html::Button>) -> At {
    // Roughly how large the panel will be. An estimate, not a measurement -
    // measuring means rendering it off-screen first, and being a few pixels out
    // only changes which side of the button it opens on.
    const HEIGHT: f64 = 380.0;
    const WIDE: f64 = 440.0;
    const NARROW: f64 = 320.0;
    const GAP: f64 = 4.0;

    let Some(button) = trigger.get_untracked() else {
        return At::default();
    };

    let rect = button.get_bounding_client_rect();
    let viewport = window()
        .inner_height()
        .ok()
        .and_then(|height| height.as_f64())
        .unwrap_or(0.0);
    let width = window()
        .inner_width()
        .ok()
        .and_then(|width| width.as_f64())
        .unwrap_or(0.0);

    // The `sm` breakpoint, where the names move from above the calendar to
    // beside it and the panel becomes half as tall and half again as wide.
    let panel = if width >= 640.0 {
        WIDE
    } else {
        NARROW.min(width - GAP * 2.0)
    };
    let below = viewport - rect.bottom();
    let upwards = below < HEIGHT && rect.top() >= HEIGHT;

    At {
        // Aligned with the button, pulled back onto the screen when that would
        // hang it off the right edge - which is where a filter bar puts it on
        // a phone.
        left: rect.left().min(width - panel - GAP).max(GAP),
        top: (!upwards).then(|| rect.bottom() + GAP),
        bottom: upwards.then(|| viewport - rect.top() + GAP),
    }
}

#[cfg(not(feature = "hydrate"))]
fn place(_trigger: NodeRef<leptos::html::Button>) -> At {
    At::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built-in English, which is what these tests assert the words of.
    fn words() -> phonix_core::i18n::Catalog {
        phonix_core::i18n::Catalog::builtin(phonix_core::i18n::Language::ENGLISH)
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    fn friday() -> NaiveDate {
        date(2026, 8, 21)
    }

    #[test]
    fn a_span_of_days_includes_the_day_it_ends_on() {
        let span = days(date(2026, 8, 17), date(2026, 8, 23));

        assert!(span.contains(midnight(date(2026, 8, 23))));
        assert!(span.contains(midnight(date(2026, 8, 23)) + TimeDelta::hours(23)));
        assert!(!span.contains(midnight(date(2026, 8, 24))));
    }

    #[test]
    fn one_day_picked_twice_is_that_day() {
        let span = days(friday(), friday());

        assert_eq!(
            span,
            DateRange::between(midnight(friday()), midnight(date(2026, 8, 22)))
        );
    }

    #[test]
    fn the_last_day_of_a_span_is_the_one_before_the_edge() {
        // What the `To` field shows. Off by one here would put every span a day
        // longer on the screen than it is on the wire.
        assert_eq!(
            last_day(days(friday(), date(2026, 8, 23))),
            Some(date(2026, 8, 23))
        );
        assert_eq!(last_day(DateRange::ANY), None);
    }

    #[test]
    fn a_span_that_ends_mid_morning_ends_on_that_day() {
        let range = DateRange::until(midnight(friday()) + TimeDelta::hours(9));

        assert_eq!(last_day(range), Some(friday()));
    }

    #[test]
    fn a_typed_date_is_that_midnight() {
        assert_eq!(parse("2026-08-21", false), Some(midnight(friday())));
        assert_eq!(parse("  2026-08-21 ", false), Some(midnight(friday())));
    }

    #[test]
    fn a_typed_moment_is_read_as_utc() {
        // Not the browser's zone: the grid renders UTC, so a viewer typing
        // 14:30 has to get the rows the `When` column shows as 14:30.
        let expected = midnight(friday()) + TimeDelta::hours(14) + TimeDelta::minutes(30);

        assert_eq!(parse("2026-08-21T14:30", true), Some(expected));
        // Some browsers add seconds once a step is set.
        assert_eq!(parse("2026-08-21T14:30:00", true), Some(expected));
    }

    #[test]
    fn a_field_that_is_empty_or_half_typed_chooses_nothing() {
        assert_eq!(parse("", false), None);
        assert_eq!(parse("2026-0", false), None);
        assert_eq!(parse("2026-08-21", true), None);
    }

    #[test]
    fn a_field_shows_the_value_it_would_accept_back() {
        let at = midnight(friday()) + TimeDelta::hours(9);

        assert_eq!(stamp(at, false), "2026-08-21");
        assert_eq!(stamp(at, true), "2026-08-21T09:00");
        assert_eq!(parse(&stamp(at, true), true), Some(at));
    }

    #[test]
    fn every_format_string_this_control_uses_actually_formats() {
        // The compiler cannot check these; a bad one panics where it renders.
        let at = midnight(friday()) + TimeDelta::hours(9);

        assert_eq!(at.format(DATE_FORMAT).to_string(), "2026-08-21");
        assert_eq!(at.format(MOMENT_FORMAT).to_string(), "2026-08-21T09:00");
    }

    #[test]
    fn a_span_nobody_chose_says_so() {
        assert_eq!(
            summarise(&words(), DateRange::ANY, DatePreset::COMMON, friday()),
            "Any time"
        );
    }

    #[test]
    fn a_span_that_is_exactly_a_name_is_shown_by_its_name() {
        let range = DatePreset::ThisWeek.resolve(friday());

        assert_eq!(
            summarise(&words(), range, DatePreset::COMMON, friday()),
            "This week"
        );
    }

    #[test]
    fn a_span_of_this_year_does_not_repeat_the_year() {
        let range = days(date(2026, 8, 17), date(2026, 8, 23));

        assert_eq!(
            summarise(&words(), range, &[], friday()),
            "17 Aug \u{2013} 23 Aug"
        );
    }

    #[test]
    fn a_span_reaching_back_into_another_year_says_which() {
        let range = days(date(2025, 12, 30), date(2026, 1, 2));

        assert_eq!(
            summarise(&words(), range, &[], friday()),
            "30 Dec 2025 \u{2013} 2 Jan"
        );
    }

    #[test]
    fn a_single_day_is_shown_once_rather_than_twice() {
        assert_eq!(
            summarise(&words(), days(friday(), friday()), &[], friday()),
            "21 Aug"
        );
    }

    #[test]
    fn a_span_with_one_end_says_which_end_it_has() {
        assert_eq!(
            summarise(
                &words(),
                DateRange::since(midnight(friday())),
                &[],
                friday()
            ),
            "From 21 Aug",
        );
        assert_eq!(
            summarise(
                &words(),
                DateRange::until(midnight(date(2026, 8, 22))),
                &[],
                friday()
            ),
            "Until 21 Aug",
        );
    }

    #[test]
    fn a_panel_hangs_below_its_button_when_there_is_room() {
        let at = At {
            left: 96.0,
            top: Some(120.0),
            bottom: None,
        };

        assert_eq!(at.style(), "position:fixed;left:96px;top:120px");
    }

    #[test]
    fn a_panel_with_no_room_below_opens_upwards() {
        let at = At {
            left: 96.0,
            top: None,
            bottom: Some(48.0),
        };

        assert_eq!(at.style(), "position:fixed;left:96px;bottom:48px");
    }
}
