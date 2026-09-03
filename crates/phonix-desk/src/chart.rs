//! The geometry behind Desk's charts, in numbers a template can only place.
//!
//! # Why this is arithmetic and not a library
//!
//! Desk's charts are drawn as inline SVG whose fills are the same CSS custom
//! properties every other Desk surface uses - `var(--brand)`, `var(--danger)`.
//! That is what makes a chart follow the light and dark themes, and follow the
//! accent the deployment chose, without rendering twice. A plotting crate bakes
//! resolved colours into its output and brings its own typography and axis
//! chrome, so the chart would stop matching the page it sits on the moment
//! either changed. The shapes here are a column and a stacked bar; the cost of
//! owning that arithmetic is this file, and what it buys is a chart that is
//! part of the design system rather than a picture pasted into it.
//!
//! # Every decision is made here, none in the template
//!
//! Bar positions, the rounded path, which columns are worth labelling, where
//! the label sits, the axis ticks: all computed, all handed over as numbers and
//! strings. The template's whole job is to put them in attributes. A template
//! that computed a bar height would be a second place the chart is defined, and
//! the one place nothing can test.
//!
//! # What the marks follow
//!
//! Thin marks and recessive chrome: a column is capped at [`MAX_BAR`] wide with
//! a 4px rounded data-end and a square foot on the baseline, neighbours are
//! separated by [`BAR_GAP`] of plain surface rather than by a stroke, gridlines
//! are solid hairlines one step off the surface, and values are labelled
//! *selectively* - the tallest column and the most recent one - because a
//! number over every bar is read as noise and skipped.

/// The drawing's coordinate space. The SVG scales to its container; these are
/// the units everything below is expressed in.
pub const WIDTH: f64 = 640.0;
pub const HEIGHT: f64 = 180.0;

/// Room on the left for the axis tick labels.
const PAD_LEFT: f64 = 32.0;
const PAD_RIGHT: f64 = 6.0;
/// Room above the tallest column for its value label.
const PAD_TOP: f64 = 16.0;
/// Where every column stands.
pub const BASELINE: f64 = 146.0;
/// Where the period labels sit, below the baseline.
pub const AXIS_LABEL_Y: f64 = 162.0;

/// Columns never grow past this, however much room the band has. A bar that
/// fills its slot leaves no air, and the chart reads as a solid block.
const MAX_BAR: f64 = 24.0;
/// Plain surface between neighbours, never a stroke around them.
const BAR_GAP: f64 = 2.0;
/// The rounded data-end. The foot stays square on the baseline.
const CORNER: f64 = 4.0;

/// How many period labels the axis can hold before they start to touch.
const MAX_AXIS_LABELS: usize = 13;

/// One period, before it is given a position.
pub struct Point {
    /// The short label under the column - `Mar`, `12`.
    pub axis_label: String,
    /// The period spelled out - `September 2026`. Used by the table under the
    /// chart, where there is room for it and where a screen reader needs it.
    pub period: String,
    /// The whole sentence, shown as the browser's own tooltip. Native
    /// `<title>`, so hovering works with no script at all.
    pub title: String,
    pub value: i64,
}

/// One column, positioned.
pub struct Column {
    /// The rounded-top path. Empty when the value is zero - there is no shape
    /// to draw, and a zero-height rectangle renders as a stray line.
    pub path: String,
    pub value: i64,
    pub axis_label: String,
    pub period: String,
    /// Whether this column's label is one of the ones the axis has room for.
    pub show_axis_label: bool,
    /// The centre, for the axis label and the value label.
    pub center: f64,
    /// The value written above the column, on the few columns that earn one.
    pub value_label: Option<String>,
    /// Where that label's baseline sits.
    pub value_label_y: f64,
    pub title: String,
}

/// One horizontal gridline, and the number against it.
pub struct Tick {
    pub y: f64,
    pub label: String,
    /// Where the number sits. Offsets computed here rather than as arithmetic
    /// in the template, which has no arithmetic to do them with.
    pub label_x: f64,
    pub label_y: f64,
}

/// A column chart over one series.
pub struct ColumnChart {
    pub columns: Vec<Column>,
    pub ticks: Vec<Tick>,
    pub plot_left: f64,
    pub plot_right: f64,
    pub total: i64,
    /// Every value is zero. The page says so in words rather than drawing a
    /// flat axis and leaving the reader to work out whether it is broken.
    pub empty: bool,
}

/// Lay a series out.
pub fn columns(points: Vec<Point>) -> ColumnChart {
    let max = points.iter().map(|p| p.value).max().unwrap_or(0);
    let total = points.iter().map(|p| p.value).sum();
    let scale_top = nice_ceiling(max);

    let plot_left = PAD_LEFT;
    let plot_right = WIDTH - PAD_RIGHT;
    let plot_width = plot_right - plot_left;
    let plot_height = BASELINE - PAD_TOP;

    let count = points.len().max(1);
    let band = plot_width / count as f64;
    let bar_width = MAX_BAR.min(band - BAR_GAP).max(1.0);

    // Which column earns a written value: the tallest, and the most recent -
    // unless they are neighbours, where two labels would sit on top of each
    // other and the tallest is the one worth keeping.
    let tallest = points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.value > 0)
        .max_by_key(|(_, p)| p.value)
        .map(|(index, _)| index);
    let newest = points
        .iter()
        .rposition(|p| p.value > 0)
        .filter(|index| match tallest {
            Some(other) => index.abs_diff(other) >= 2,
            None => true,
        });

    let label_stride = points.len().div_ceil(MAX_AXIS_LABELS).max(1);
    // Counted from the newest end, so the most recent period is always
    // labelled: it is the one being read.
    let last = points.len().saturating_sub(1);

    let columns = points
        .into_iter()
        .enumerate()
        .map(|(index, point)| {
            let height = if scale_top == 0 {
                0.0
            } else {
                (point.value as f64 / scale_top as f64) * plot_height
            };
            let x = plot_left + index as f64 * band + (band - bar_width) / 2.0;
            let y = BASELINE - height;

            Column {
                path: bar_path(x, y, bar_width, height),
                value: point.value,
                axis_label: point.axis_label,
                period: point.period,
                show_axis_label: last.saturating_sub(index) % label_stride == 0,
                center: x + bar_width / 2.0,
                value_label: (Some(index) == tallest || Some(index) == newest)
                    .then(|| point.value.to_string()),
                value_label_y: y - 5.0,
                title: point.title,
            }
        })
        .collect();

    ColumnChart {
        columns,
        ticks: ticks(scale_top, plot_height),
        plot_left,
        plot_right,
        total,
        empty: max == 0,
    }
}

/// A column with a rounded data-end and a square foot.
///
/// Drawn as a path rather than a `<rect rx>` because `rx` rounds all four
/// corners, which lifts the bar off its own baseline and makes short bars look
/// like pills floating above the axis.
fn bar_path(x: f64, y: f64, width: f64, height: f64) -> String {
    if height <= 0.0 {
        return String::new();
    }

    let radius = CORNER.min(width / 2.0).min(height);
    let right = x + width;
    let bottom = y + height;

    format!(
        "M{x:.1} {bottom:.1}V{:.1}A{radius:.1} {radius:.1} 0 0 1 {:.1} {y:.1}H{:.1}A{radius:.1} {radius:.1} 0 0 1 {right:.1} {:.1}V{bottom:.1}Z",
        y + radius,
        x + radius,
        right - radius,
        y + radius,
    )
}

/// Three gridlines: the baseline, the middle, and the top of the scale.
///
/// Two numbers plus zero is enough to read a height off, and every extra line
/// is ink competing with the data. The values are whole because the scale's top
/// is chosen to make them whole.
fn ticks(scale_top: i64, plot_height: f64) -> Vec<Tick> {
    let mut ticks = vec![tick(BASELINE, "0".to_owned())];

    if scale_top == 0 {
        return ticks;
    }

    for value in [scale_top / 2, scale_top] {
        if value == 0 {
            continue;
        }
        ticks.push(tick(
            BASELINE - (value as f64 / scale_top as f64) * plot_height,
            value.to_string(),
        ));
    }

    ticks
}

/// A gridline with its label already placed.
///
/// The number sits clear of the plot on the left and is nudged down by a third
/// of its own size so it reads as centred on the line rather than sitting on
/// top of it.
fn tick(y: f64, label: String) -> Tick {
    Tick {
        y,
        label,
        label_x: PAD_LEFT - 8.0,
        label_y: y + 4.0,
    }
}

/// The smallest round number at or above `max`.
///
/// So the axis reads 0 / 5 / 10 rather than 0 / 3.5 / 7, and the middle tick is
/// always a whole number - which is what lets the tick labels be integers
/// without rounding a line into the wrong place.
///
/// The series is 1-2-4-5-6-8-10 rather than the usual 1-2-5, and the extra
/// rungs are there to keep short bars tall. With 1-2-5 alone the middle-tick
/// rule below rejects 5, so a tallest bar of 3 would be drawn against a scale
/// of 10 - a third of the height it deserves, on a chart whose whole job is
/// relative height. A workspace count is usually a small number, so this is the
/// common case rather than an edge one.
fn nice_ceiling(max: i64) -> i64 {
    if max <= 0 {
        return 0;
    }

    let mut step = 1;
    loop {
        for multiple in [1, 2, 4, 5, 6, 8, 10] {
            let candidate = step * multiple;
            // Halved for the middle tick, so an odd top would put that line
            // between two integers and label it with the wrong one. One is the
            // exception: a lone bar has no middle tick to mislabel.
            if candidate >= max && (candidate % 2 == 0 || candidate == 1) {
                return candidate;
            }
        }
        step *= 10;
    }
}

// ---------------------------------------------------------------------------
// The composition bar
// ---------------------------------------------------------------------------

/// How tall the estate-composition bar is drawn.
pub const BAR_HEIGHT: f64 = 28.0;

/// One class in a part-to-whole bar.
pub struct Segment {
    pub x: f64,
    pub width: f64,
    pub label: &'static str,
    pub value: usize,
    /// The CSS custom property this segment is filled with. A status word, not
    /// a series index: these are states, and the colour has to mean the state.
    pub fill: &'static str,
    pub title: String,
}

/// A single horizontal bar split into classes, with the whole as its width.
pub struct StackedBar {
    pub segments: Vec<Segment>,
    pub total: usize,
    pub empty: bool,
}

/// Lay out a part-to-whole bar across the full [`WIDTH`].
///
/// Segments are separated by [`BAR_GAP`] of surface, the same gap the columns
/// use, so a boundary between two classes reads the same way everywhere.
pub fn stacked(parts: Vec<(&'static str, usize, &'static str)>) -> StackedBar {
    let total: usize = parts.iter().map(|(_, value, _)| value).sum();
    if total == 0 {
        return StackedBar {
            segments: Vec::new(),
            total: 0,
            empty: true,
        };
    }

    // The gaps come out of the drawable width, so the segments still add up to
    // the whole and the last one ends exactly on the right edge.
    let drawn = parts.iter().filter(|(_, value, _)| *value > 0).count();
    let gaps = BAR_GAP * drawn.saturating_sub(1) as f64;
    let usable = WIDTH - gaps;

    let mut x = 0.0;
    let mut segments = Vec::with_capacity(drawn);

    for (label, value, fill) in parts {
        if value == 0 {
            continue;
        }

        let width = usable * (value as f64 / total as f64);

        segments.push(Segment {
            x,
            width,
            label,
            value,
            fill,
            title: format!("{label}: {value} of {total}"),
        });

        x += width + BAR_GAP;
    }

    StackedBar {
        segments,
        total,
        empty: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(label: &str, value: i64) -> Point {
        Point {
            axis_label: label.to_owned(),
            period: label.to_owned(),
            title: format!("{label}: {value}"),
            value,
        }
    }

    #[test]
    fn a_scale_top_is_a_round_even_number_at_or_above_the_tallest_bar() {
        assert_eq!(nice_ceiling(0), 0);
        assert_eq!(nice_ceiling(1), 1);
        assert_eq!(nice_ceiling(3), 4);
        assert_eq!(nice_ceiling(7), 8);
        assert_eq!(nice_ceiling(11), 20);
        assert_eq!(nice_ceiling(45), 50);
        assert_eq!(nice_ceiling(150), 200);
    }

    /// The reason the series has more rungs than 1-2-5. A workspace count is a
    /// small number, and a scale that overshot it would draw every bar short.
    #[test]
    fn a_small_tallest_bar_still_fills_most_of_the_plot() {
        for max in 1..=40 {
            let top = nice_ceiling(max);
            let filled = max as f64 / top as f64;
            assert!(
                filled >= 0.5,
                "a tallest bar of {max} against a scale of {top} fills only {:.0}%",
                filled * 100.0,
            );
        }
    }

    /// The middle gridline is labelled with a whole number, which only holds if
    /// the top of the scale is even.
    #[test]
    fn every_tick_label_is_a_whole_number() {
        for max in 1..200 {
            let top = nice_ceiling(max);
            assert!(top >= max, "{top} must reach {max}");
            assert!(
                top % 2 == 0 || top == 1,
                "{top} halves into a fraction, so its middle tick would be mislabelled"
            );
        }
    }

    #[test]
    fn the_tallest_and_the_newest_columns_are_the_ones_labelled() {
        let chart = columns(vec![
            point("Jan", 2),
            point("Feb", 9),
            point("Mar", 1),
            point("Apr", 3),
        ]);

        let labelled: Vec<_> = chart
            .columns
            .iter()
            .filter_map(|c| c.value_label.as_deref().map(|v| (c.axis_label.as_str(), v)))
            .collect();

        assert_eq!(labelled, vec![("Feb", "9"), ("Apr", "3")]);
    }

    /// Two labels a band apart would overlap, and the tallest is the one worth
    /// keeping.
    #[test]
    fn a_newest_column_beside_the_tallest_is_not_also_labelled() {
        let chart = columns(vec![point("Jan", 2), point("Feb", 9), point("Mar", 3)]);

        let labelled: Vec<_> = chart
            .columns
            .iter()
            .filter(|c| c.value_label.is_some())
            .map(|c| c.axis_label.as_str())
            .collect();

        assert_eq!(labelled, vec!["Feb"]);
    }

    #[test]
    fn a_zero_column_has_no_shape_to_draw() {
        let chart = columns(vec![point("Jan", 0), point("Feb", 4)]);

        assert!(chart.columns[0].path.is_empty());
        assert!(!chart.columns[1].path.is_empty());
        assert!(!chart.empty, "one bar has a value, so the chart is not empty");
    }

    #[test]
    fn a_series_of_nothing_is_empty_rather_than_flat() {
        let chart = columns(vec![point("Jan", 0), point("Feb", 0)]);

        assert!(chart.empty);
        assert_eq!(chart.total, 0);
        // Only the baseline: there is no scale to draw ticks against.
        assert_eq!(chart.ticks.len(), 1);
    }

    /// Thirty days will not fit thirty labels, and the most recent day is the
    /// one that must keep its label.
    #[test]
    fn a_long_series_thins_its_axis_labels_and_keeps_the_newest() {
        let points = (1..=30).map(|d| point(&d.to_string(), 1)).collect();
        let chart = columns(points);

        let shown: Vec<_> = chart
            .columns
            .iter()
            .filter(|c| c.show_axis_label)
            .map(|c| c.axis_label.as_str())
            .collect();

        assert!(shown.len() <= MAX_AXIS_LABELS, "{} labels", shown.len());
        assert_eq!(shown.last(), Some(&"30"), "the newest period keeps its label");
    }

    #[test]
    fn a_short_series_labels_every_column() {
        let chart = columns(vec![point("Jan", 1), point("Feb", 2), point("Mar", 3)]);

        assert!(chart.columns.iter().all(|c| c.show_axis_label));
    }

    /// Every bar stands on the baseline and none of them escapes the plot.
    #[test]
    fn no_column_is_drawn_outside_the_plot() {
        let chart = columns(vec![point("Jan", 1), point("Feb", 100), point("Mar", 50)]);

        for column in &chart.columns {
            assert!(column.center >= chart.plot_left, "{}", column.axis_label);
            assert!(column.center <= chart.plot_right, "{}", column.axis_label);
            if column.value > 0 {
                assert!(
                    column.value_label_y >= 0.0 || column.value_label.is_none(),
                    "a label above the tallest bar must stay on the canvas"
                );
            }
        }
    }

    #[test]
    fn a_stacked_bar_spans_the_full_width_and_leaves_gaps_between_classes() {
        let bar = stacked(vec![("Active", 3, "--success"), ("Suspended", 1, "--danger")]);

        assert_eq!(bar.total, 4);
        assert_eq!(bar.segments.len(), 2);

        let last = bar.segments.last().unwrap();
        assert!(
            (last.x + last.width - WIDTH).abs() < 0.01,
            "the last segment must end on the right edge, ended at {}",
            last.x + last.width
        );
        // The gap is surface showing through, not a stroke.
        assert!((bar.segments[1].x - (bar.segments[0].width + BAR_GAP)).abs() < 0.01);
    }

    #[test]
    fn a_class_with_nothing_in_it_gets_no_segment() {
        let bar = stacked(vec![
            ("Active", 2, "--success"),
            ("Provisioning", 0, "--warning"),
            ("Suspended", 1, "--danger"),
        ]);

        assert_eq!(bar.segments.len(), 2);
        assert!(bar.segments.iter().all(|s| s.value > 0));
    }

    #[test]
    fn an_empty_estate_draws_no_bar() {
        let bar = stacked(vec![("Active", 0, "--success")]);

        assert!(bar.empty);
        assert!(bar.segments.is_empty());
    }
}
