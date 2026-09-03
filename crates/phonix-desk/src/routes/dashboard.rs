//! The estate at a glance: the page Desk opens on.
//!
//! Six numbers, the shape of the estate, and three series - how many
//! workspaces arrived, when the licences run out, and what desk users have
//! been doing. It is the landing page because it is the screen that says
//! whether anything needs attention at all; the workspace list, which says
//! *which* thing, is one click away at `/workspaces`.
//!
//! # It is cheap on purpose
//!
//! One catalog query with the licence joined on, and one aggregate over
//! `desk_audit`. No tenant pool is opened and no dependency is dialled - those
//! are `/queues` and `/dependencies`, each its own page for its own cost. A
//! landing page that reloads slowly is a landing page people stop reloading.
//!
//! # The charts are SVG this crate draws itself
//!
//! See [`crate::chart`] for why there is no plotting library under this. The
//! short version: the fills are the same CSS custom properties as every other
//! Desk surface, so a chart follows the theme and the accent without being
//! rendered twice, and a crate that baked its own palette in could not.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use chrono::NaiveDate;
use phonix_services::desk::dashboard::{
    self, ACTIVITY_DAYS, Bucket, CREATED_MONTHS, RUNWAY_MONTHS, Tally,
};

use crate::chart::{self, ColumnChart, Point, StackedBar};
use crate::html::{Chrome, render};
use crate::routes::{SignedIn, internal_error};
use crate::state::DeskState;

/// One headline number.
pub struct Tile {
    pub label: &'static str,
    pub value: String,
    /// What the number means, in the words somebody would use out loud. Always
    /// present: a bare number with a two-word label is a number people invent
    /// their own definition for.
    pub note: &'static str,
    /// Whether this number is asking for something to be done. Only ever true
    /// when it is non-zero - a red nought is an alarm about nothing.
    pub alarm: bool,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardPage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub tiles: Vec<Tile>,
    pub composition: StackedBar,
    pub created: ColumnChart,
    pub runway: ColumnChart,
    pub activity: ColumnChart,
    /// The sentence under each chart's heading. Written here because it names
    /// the window's length, and the window's length is a constant in the
    /// service - a template that spelled "12" would be a second copy of it.
    pub created_blurb: String,
    pub runway_blurb: String,
    pub activity_blurb: String,
}

pub async fn index(SignedIn(caller): SignedIn, State(state): State<DeskState>) -> Response {
    let estate = match dashboard::compose(&state.catalog).await {
        Ok(estate) => estate,
        Err(err) => return internal_error(err, "reading the estate"),
    };

    render(&DashboardPage {
        title: "Overview".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "overview"),
        banner: None,
        tiles: tiles(&estate.tally),
        composition: composition(&estate.tally),
        created: chart::columns(monthly(&estate.created, "created")),
        runway: chart::columns(monthly(&estate.runway, "ending")),
        activity: chart::columns(daily(&estate.activity)),
        created_blurb: format!("The last {CREATED_MONTHS} months."),
        runway_blurb: format!(
            "The next {RUNWAY_MONTHS} months. A licence with no end date is in none of these \
             bars - it is not expiring, it is perpetual."
        ),
        activity_blurb: format!(
            "Audit rows written in the last {ACTIVITY_DAYS} days, sign-ins and refusals included."
        ),
    })
}

/// The six numbers, in the order somebody scans them: what there is, what is
/// working, then what is wrong.
fn tiles(tally: &Tally) -> Vec<Tile> {
    vec![
        Tile {
            label: "Workspaces",
            value: tally.total.to_string(),
            note: "on this box, every status",
            alarm: false,
        },
        Tile {
            label: "Serving",
            value: tally.serving.to_string(),
            note: "active and authorized, both halves agreeing",
            alarm: false,
        },
        Tile {
            label: "Stuck provisioning",
            value: tally.provisioning.to_string(),
            note: "never finished being created; retryable",
            alarm: tally.provisioning > 0,
        },
        Tile {
            label: "Behind the build",
            value: tally.outdated.to_string(),
            note: "schema older than this binary's",
            alarm: tally.outdated > 0,
        },
        Tile {
            label: "Unlicensed",
            value: tally.unlicensed.to_string(),
            note: "active, but nothing authorizes it",
            alarm: tally.unlicensed > 0,
        },
        Tile {
            label: "Expiring soon",
            value: tally.expiring_soon.to_string(),
            note: "licence ends within the month",
            alarm: tally.expiring_soon > 0,
        },
    ]
}

/// The estate split by status.
///
/// Status colours rather than a categorical palette, because these *are*
/// states: green is running, amber is mid-flight, red is stopped, grey is put
/// away. Every segment is labelled as well as coloured, so the reading never
/// depends on telling two hues apart.
fn composition(tally: &Tally) -> StackedBar {
    chart::stacked(vec![
        ("Active", tally.active, "--success"),
        ("Provisioning", tally.provisioning, "--warning"),
        ("Suspended", tally.suspended, "--danger"),
        ("Archived", tally.archived, "--content-subtle"),
    ])
}

/// Month buckets, as columns.
fn monthly(buckets: &[Bucket], verb: &'static str) -> Vec<Point> {
    buckets
        .iter()
        .map(|bucket| Point {
            axis_label: month_short(bucket.start),
            period: month_long(bucket.start),
            title: format!(
                "{}: {} workspace{} {verb}",
                month_long(bucket.start),
                bucket.value,
                plural(bucket.value),
            ),
            value: bucket.value,
        })
        .collect()
}

/// Day buckets, as columns.
fn daily(buckets: &[Bucket]) -> Vec<Point> {
    buckets
        .iter()
        .map(|bucket| Point {
            axis_label: bucket.start.format("%-d").to_string(),
            period: bucket.start.format("%-d %B %Y").to_string(),
            title: format!(
                "{}: {} entr{}",
                bucket.start.format("%-d %B %Y"),
                bucket.value,
                if bucket.value == 1 { "y" } else { "ies" },
            ),
            value: bucket.value,
        })
        .collect()
}

fn month_short(date: NaiveDate) -> String {
    date.format("%b").to_string()
}

fn month_long(date: NaiveDate) -> String {
    date.format("%B %Y").to_string()
}

fn plural(value: i64) -> &'static str {
    if value == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// A red nought is an alarm about nothing, and a dashboard that cries wolf
    /// on an empty estate is one nobody reads the second week.
    #[test]
    fn a_zero_never_raises_an_alarm() {
        let quiet = tiles(&Tally::default());

        assert!(quiet.iter().all(|tile| !tile.alarm));
        assert!(quiet.iter().all(|tile| tile.value == "0"));
    }

    #[test]
    fn a_problem_that_is_present_raises_its_own_tile_and_no_other() {
        let tally = Tally {
            total: 4,
            provisioning: 1,
            ..Tally::default()
        };

        let alarmed: Vec<_> = tiles(&tally)
            .into_iter()
            .filter(|tile| tile.alarm)
            .map(|tile| tile.label)
            .collect();

        assert_eq!(alarmed, vec!["Stuck provisioning"]);
    }

    #[test]
    fn every_tile_says_what_its_number_means() {
        assert!(tiles(&Tally::default()).iter().all(|t| !t.note.is_empty()));
    }

    /// The composition bar only ever shows classes that exist, so an estate of
    /// four active workspaces is one full-width green bar and not four.
    #[test]
    fn the_composition_bar_shows_only_the_states_present() {
        let bar = composition(&Tally {
            total: 4,
            active: 3,
            suspended: 1,
            ..Tally::default()
        });

        let labels: Vec<_> = bar.segments.iter().map(|s| s.label).collect();
        assert_eq!(labels, vec!["Active", "Suspended"]);
        assert_eq!(bar.total, 4);
    }

    #[test]
    fn a_month_column_is_short_on_the_axis_and_spelled_out_in_the_table() {
        let points = monthly(&[Bucket { start: on(2026, 9, 1), value: 2 }], "created");

        assert_eq!(points[0].axis_label, "Sep");
        assert_eq!(points[0].period, "September 2026");
        assert_eq!(points[0].title, "September 2026: 2 workspaces created");
    }

    #[test]
    fn one_of_something_is_not_written_as_a_plural() {
        let points = monthly(&[Bucket { start: on(2026, 9, 1), value: 1 }], "created");
        assert_eq!(points[0].title, "September 2026: 1 workspace created");

        let days = daily(&[Bucket { start: on(2026, 9, 1), value: 1 }]);
        assert_eq!(days[0].title, "1 September 2026: 1 entry");
    }

    fn page(tally: Tally, created: Vec<Bucket>, activity: Vec<Bucket>) -> DashboardPage {
        DashboardPage {
            title: "Overview".to_owned(),
            chrome: Chrome::new("Ada", "development", "overview"),
            banner: None,
            tiles: tiles(&tally),
            composition: composition(&tally),
            created: chart::columns(monthly(&created, "created")),
            runway: chart::columns(monthly(&[], "ending")),
            activity: chart::columns(daily(&activity)),
            created_blurb: "The last 12 months.".to_owned(),
            runway_blurb: "The next 6 months.".to_owned(),
            activity_blurb: "The last 30 days.".to_owned(),
        }
    }

    /// The page renders, which is the half a unit test over the numbers cannot
    /// reach: a template that reaches for a field that moved fails here rather
    /// than as a 500 in front of somebody.
    #[test]
    fn the_page_renders_with_an_estate_in_it() {
        let rendered = page(
            Tally { total: 3, active: 2, suspended: 1, serving: 2, ..Tally::default() },
            vec![Bucket { start: on(2026, 9, 1), value: 3 }],
            vec![Bucket { start: on(2026, 9, 3), value: 5 }],
        )
        .render()
        .expect("renders");

        assert!(rendered.contains("Workspaces"));
        assert!(rendered.contains("By status"));
        assert!(rendered.contains("Desk activity"));
        // The marks the hover layer listens for, and the tooltip that works
        // without it.
        assert!(rendered.contains("data-mark"));
        assert!(rendered.contains("<title>"));
    }

    /// A box with no workspaces in it is a real state - it is every box on its
    /// first day - and the page has to say so rather than draw an empty axis.
    #[test]
    fn an_empty_estate_renders_words_rather_than_empty_axes() {
        let rendered = page(Tally::default(), Vec::new(), Vec::new())
            .render()
            .expect("renders");

        assert!(rendered.contains("There are no workspaces on this box yet."));
        assert!(rendered.contains("No workspace has been created in this window."));
        assert!(!rendered.contains("data-mark"), "there is nothing to draw");
    }

    #[test]
    fn a_day_column_is_a_bare_number_on_the_axis() {
        let points = daily(&[Bucket { start: on(2026, 9, 7), value: 0 }]);

        assert_eq!(points[0].axis_label, "7");
        assert_eq!(points[0].period, "7 September 2026");
    }
}
