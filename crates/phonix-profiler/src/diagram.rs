//! The page load drawn as a diagram, in SVG, with no script.
//!
//! `report`'s rule is no JavaScript, for a reason that still holds here: the
//! profiler is wanted exactly when the application is broken, and every
//! dependency is another way to be unavailable. So the interaction is built
//! from things the browser already does.
//!
//! * **Clicking a layer** is an `<a href="#layer-phonix-db">` inside the SVG.
//!   The panel below is shown by `:target`. No handler, no state, and the
//!   selection survives a reload because it is in the URL.
//! * **Clicking a file** is an ordinary link to a page that renders that
//!   file's source. Not a fetch into a panel - which would need script, and
//!   would need an endpoint returning file contents to anything that asks.
//! * **Choosing a phase** is a link back to this page with `?phase=`.
//!
//! The layout is a fixed grid because the spine is fixed - see
//! [`crate::flow`]. Nothing here decides what is drawn; it decides where.

use std::fmt::Write as _;

use crate::flow::{Flow, LayerKind, Node, PageFlow, Phase};
use crate::profile::Token;
use crate::report::{escape, millis};

const BOX_W: usize = 148;
const BOX_H: usize = 40;
/// A row nothing touched, drawn as a band rather than as four empty boxes.
///
/// The spine stays whole - a layer that is missing from the picture is worse
/// than one drawn faintly - but a request that used three layers should not
/// spend half the panel saying so.
const SLIM_H: usize = 20;
const GAP_X: usize = 12;
const GAP_Y: usize = 34;
const SLIM_GAP: usize = 16;
const PAD: usize = 10;
const COLUMNS: usize = 4;
const ROWS: usize = 5;

/// Where each row sits and how tall it is, once the empty ones are squeezed.
#[derive(Debug, Clone, Copy)]
struct Row {
    y: usize,
    height: usize,
    live: bool,
}

fn rows_of(flow: &Flow) -> Vec<Row> {
    let mut rows = Vec::with_capacity(ROWS);
    let mut y = PAD;

    for row in 0..ROWS {
        let live = flow
            .nodes
            .iter()
            .any(|node| node.row == row && node.observed);
        let height = if live { BOX_H } else { SLIM_H };

        rows.push(Row { y, height, live });

        y += height + if live { GAP_Y } else { SLIM_GAP };
    }

    rows
}

fn width() -> usize {
    PAD * 2 + COLUMNS * BOX_W + (COLUMNS - 1) * GAP_X
}

fn height(rows: &[Row]) -> usize {
    rows.last().map(|row| row.y + row.height).unwrap_or(0) + PAD
}

fn x_of(column: usize) -> usize {
    PAD + column * (BOX_W + GAP_X)
}

/// The whole section: phase strip, diagram, layer panels.
///
/// `active` is the phase being shown, or `None` for the whole page load.
pub fn section(page: &str, flow: &PageFlow, active: Option<Token>) -> String {
    let shown = match active {
        Some(token) => flow
            .phases
            .iter()
            .find(|phase| phase.token == token)
            .map(|phase| &phase.flow)
            .unwrap_or(&flow.overall),
        None => &flow.overall,
    };

    let mut html = String::new();

    html.push_str("<section class=\"card\"><h2>How this page load ran</h2>");

    phases(&mut html, page, flow, active);

    // An all-grey diagram is the correct answer often enough that it has to
    // say so. Without this it reads as a broken feature, and whoever is looking
    // goes hunting for the bug rather than accepting the answer.
    if shown.nothing_observed() {
        html.push_str(
            "<p class=\"note warn\">Nothing recorded here - no statements, no log \
             lines above the filter. Widen <code>profiler.filter</code> to see \
             more.</p>",
        );
    } else if shown.without_stacks {
        html.push_str(
            "<p class=\"note warn\">No stacks captured, so no arrows - \
             <code>profiler.backtraces = false</code>.</p>",
        );
    }

    svg(&mut html, shown);
    legend(&mut html, shown);
    panels(&mut html, page, shown);

    html.push_str("</section>");
    html
}

/// The phase strip: the whole load, then each request in it.
fn phases(html: &mut String, page: &str, flow: &PageFlow, active: Option<Token>) {
    if flow.phases.len() < 2 {
        return;
    }

    html.push_str("<div class=\"phases\">");

    let _ = write!(
        html,
        "<a class=\"button{on}\" href=\"/_profiler/page/{page}\">whole load</a>",
        on = if active.is_none() { " on" } else { "" },
        page = escape(page),
    );

    for phase in &flow.phases {
        let _ = write!(
            html,
            "<a class=\"button{on}\" href=\"/_profiler/page/{page}?phase={token}\">\
             <span class=\"method\">{method}</span> {route}</a>",
            on = if active == Some(phase.token) {
                " on"
            } else {
                ""
            },
            page = escape(page),
            token = phase.token,
            method = escape(&phase.method),
            route = escape(short_route(&phase.route)),
        );
    }

    html.push_str("</div>");
}

/// A route trimmed to something that fits on a button.
fn short_route(route: &str) -> &str {
    match route.rsplit_once('/') {
        Some((_, tail)) if !tail.is_empty() => tail,
        _ => route,
    }
}

fn svg(html: &mut String, flow: &Flow) {
    let rows = rows_of(flow);

    let _ = write!(
        html,
        "<div class=\"diagram\"><svg viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" \
         role=\"img\" aria-label=\"Layers this page load passed through\">",
        w = width(),
        h = height(&rows),
    );

    // One marker, referenced by every edge. Two, because an unobserved edge
    // never happens - but a faint arrowhead is still needed for the legend.
    html.push_str(
        "<defs><marker id=\"arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" \
         markerWidth=\"6\" markerHeight=\"6\" orient=\"auto-start-reverse\">\
         <path d=\"M 0 0 L 10 5 L 0 10 z\" class=\"head\"/></marker></defs>",
    );

    // Edges first, so a box is never drawn under a line.
    for edge in &flow.edges {
        let (Some(from), Some(to)) = (flow.node(&edge.from), flow.node(&edge.to)) else {
            continue;
        };

        let x1 = x_of(from.column) + BOX_W / 2;
        let y1 = rows[from.row].y + rows[from.row].height;
        let x2 = x_of(to.column) + BOX_W / 2;
        let y2 = rows[to.row].y;

        // A gentle S rather than a straight line: several edges often share a
        // start or an end, and straight ones overlap into a single thick stem.
        let lift = y2.abs_diff(y1).max(24) / 2;

        let _ = write!(
            html,
            "<path class=\"edge\" d=\"M {x1} {y1} C {x1} {c1} {x2} {c2} {x2} {y2}\" \
             stroke-width=\"{weight:.1}\" marker-end=\"url(#arrow)\"/>",
            c1 = y1 + lift,
            c2 = y2.saturating_sub(lift),
            weight = 1.2 + (edge.hits.min(24) as f64) / 8.0,
        );

        // The count sits at the midpoint. Only the edge into the database
        // carries a time, because it is the only one measured.
        let (label, tone) = match edge.elapsed {
            Some(elapsed) => (
                format!("{} · {}", edge.hits, millis(elapsed)),
                crate::report::tone_of(elapsed),
            ),
            None => (edge.hits.to_string(), ""),
        };

        let _ = write!(
            html,
            "<text class=\"edge-label{tone}\" x=\"{x}\" y=\"{y}\">{label}</text>",
            x = x1.midpoint(x2),
            y = y1.midpoint(y2),
            label = escape(&label),
        );
    }

    for node in &flow.nodes {
        box_of(html, node, rows[node.row]);
    }

    html.push_str("</svg></div>");
}

fn box_of(html: &mut String, node: &Node, row: Row) {
    let x = x_of(node.column);
    let y = row.y;

    let class = match (node.observed, node.kind) {
        (true, LayerKind::External) => "lay on ext",
        (true, LayerKind::Crate) => "lay on",
        (false, LayerKind::External) => "lay ext",
        (false, LayerKind::Crate) => "lay",
    };

    // Only a layer with files to show is a link. An external has none and never
    // will, and a link that opens an empty panel is a broken promise.
    let clickable = node.observed && !node.files.is_empty();

    if clickable {
        let _ = write!(html, "<a href=\"#layer-{}\">", escape(&node.id));
    }

    // The icon is a compile-time constant from `flow::SPINE` - markup this
    // file wrote, never anything a request supplied - so it is emitted as it
    // stands. Escaping it would print the path data instead of drawing it.
    let icon = if row.live { 15 } else { 11 };

    let _ = write!(
        html,
        "<g class=\"{class}\"><rect x=\"{x}\" y=\"{y}\" width=\"{BOX_W}\" height=\"{h}\" \
         rx=\"6\"/>\
         <g class=\"ic\" transform=\"translate({ix} {iy}) scale({scale:.3})\">{glyph}</g>\
         <text class=\"name\" x=\"{tx}\" y=\"{ty}\">{label}</text>",
        h = row.height,
        ix = x + 9,
        iy = y + (row.height - icon) / 2,
        scale = f64::from(icon as u32) / 16.0,
        glyph = node.icon,
        tx = x + 9 + icon + 7,
        ty = y + if row.live { 17 } else { 14 },
        label = escape(&node.label),
    );

    // A squeezed row has no space for a second line, and "not seen" repeated
    // four times across a band says less than the band already does.
    if row.live {
        let detail = if node.observed {
            format!("{} frames", node.hits)
        } else {
            "not seen".to_owned()
        };

        let _ = write!(
            html,
            "<text class=\"sub\" x=\"{tx}\" y=\"{ty}\">{detail}</text>",
            tx = x + 9 + icon + 7,
            ty = y + 31,
            detail = escape(&detail),
        );
    }

    html.push_str("</g>");

    if clickable {
        html.push_str("</a>");
    }
}

/// The one line worth having always visible, and the rest folded away.
///
/// The explanation matters - a reader who mistakes grey for "not used" draws
/// the wrong conclusion - but it is read once and then never again, and three
/// paragraphs above a diagram is most of a panel spent on prose.
fn legend(html: &mut String, flow: &Flow) {
    let unlit_externals: Vec<&str> = flow
        .nodes
        .iter()
        .filter(|node| node.kind == LayerKind::External && !node.observed)
        .map(|node| node.label.as_str())
        .collect();

    html.push_str(
        "<p class=\"note\">Click a lit layer for its files. A number on an arrow \
         is how many stacks crossed there.</p>",
    );

    html.push_str(
        "<details class=\"about\"><summary>How to read this</summary>\
         <p class=\"note\">Boxes are this workspace's layers, always drawn in the \
         same places so the shape is the same every time. A box is lit only when \
         a captured stack put a frame in it, and an arrow exists only where two \
         frames crossed between layers - nothing here assumes a layer ran because \
         the one below it did. A row nothing touched is squeezed to a band.</p>",
    );

    html.push_str(
        "<p class=\"note\">Only the arrow into the database carries a time. sqlx \
         measures its own round trip; nothing measures how long a request spent \
         inside a layer, and a number there would be invented.</p>",
    );

    if !unlit_externals.is_empty() {
        let _ = write!(
            html,
            "<p class=\"note\">{} stay grey whatever the traffic: sqlx logs every \
             statement, so Postgres can be lit from evidence, and no other adapter \
             reports its round trips. Grey here means <em>not measured</em>, not \
             <em>not used</em>.</p>",
            escape(&unlit_externals.join(", ")),
        );
    }

    html.push_str("</details>");
}

/// One panel per layer, revealed by `:target`.
fn panels(html: &mut String, page: &str, flow: &Flow) {
    for node in &flow.nodes {
        if node.files.is_empty() {
            continue;
        }

        let _ = write!(
            html,
            "<div class=\"layer-files\" id=\"layer-{id}\">\
             <h3>{label} <span class=\"count\">{hits}</span></h3>",
            id = escape(&node.id),
            label = escape(&node.label),
            hits = node.hits,
        );

        html.push_str(
            "<table><thead><tr><th>file</th><th>function</th>\
             <th class=\"num\">line</th><th class=\"num\">n</th></tr></thead><tbody>",
        );

        for file in &node.files {
            for function in &file.functions {
                let _ = write!(
                    html,
                    "<tr><td class=\"mono\">{file}</td><td class=\"mono dim\">{function}</td>\
                     <td class=\"num\"><a data-drawer=\"{modal}\" \
                     href=\"/_profiler/source/page/{page}?file={qfile}\
                     &amp;line={line}\">{line}</a></td>\
                     <td class=\"num dim\">{hits}</td></tr>",
                    file = escape(&file.file),
                    modal = escape(&format!("{}:{}", file.file, function.line)),
                    function = escape(&function.function),
                    page = escape(page),
                    qfile = escape(&urlencode(&file.file)),
                    line = function.line,
                    hits = function.hits,
                );
            }
        }

        html.push_str("</tbody></table></div>");
    }
}

/// Percent-encode what goes in a query string.
///
/// Small and local rather than a dependency: the only thing that ever reaches
/// it is a workspace-relative path, whose alphabet is letters, digits, `-`,
/// `_`, `.` and `/`. Everything outside that is encoded, so a future path with
/// a space or an ampersand in it cannot split the query string.
fn urlencode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());

    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' => {
                encoded.push(byte as char);
            }
            other => {
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }

    encoded
}

/// A phase's own heading, for the source page's breadcrumb.
pub fn phase_label(phase: &Phase) -> String {
    format!("{} {}", phase.method, phase.route)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::Flow;
    use crate::profile::{Kind, LogLine, Profile, Query, Token};
    use crate::{Caller, PageFlow};
    use chrono::Utc;
    use std::sync::Arc;
    use std::time::Duration;

    fn profile_with(logs: Vec<LogLine>, queries: Vec<Query>) -> Arc<Profile> {
        Arc::new(Profile {
            token: Token(1),
            at: Utc::now(),
            kind: Kind::Document,
            method: "GET".into(),
            path: "/admin/users".into(),
            query_string: None,
            route: Some("/admin/users".into()),
            status: 200,
            duration: Duration::from_millis(12),
            tenant: None,
            page: Some("p1".into()),
            response_bytes: None,
            queries,
            logs,
            rss_bytes: None,
        })
    }

    fn log_at(file: &str, line: u32) -> LogLine {
        LogLine {
            level: "INFO".into(),
            target: "phonix_services".into(),
            message: "did a thing".into(),
            fields: Vec::new(),
            source: Some(file.to_owned()),
            line: Some(line),
            caller: Caller::none(),
        }
    }

    /// The whole point of the section: a page load that made no queries still
    /// draws its layers.
    #[test]
    fn a_page_load_with_no_sql_still_draws_its_layers() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/billing.rs", 88)],
            Vec::new(),
        )];
        let flow = PageFlow::of(&profiles);
        let html = section("p1", &flow, None);

        assert!(html.contains("Application"), "the services box is drawn");
        assert!(
            html.contains("layer-phonix-services"),
            "and it is clickable, because it has a file behind it"
        );
    }

    /// An unlit layer must not offer a panel, or clicking it opens nothing.
    #[test]
    fn a_layer_nothing_touched_is_not_a_link() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/billing.rs", 88)],
            Vec::new(),
        )];
        let html = section("p1", &PageFlow::of(&profiles), None);

        assert!(
            !html.contains("#layer-phonix-storage"),
            "storage was never touched and must not be a link"
        );
    }

    /// The report escapes everything it prints, and SVG is not an exception -
    /// a `<` reaching the document as markup is the same bug wherever it lands.
    #[test]
    fn a_path_cannot_carry_markup_into_the_diagram() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/<script>.rs", 1)],
            Vec::new(),
        )];
        let html = section("p1", &PageFlow::of(&profiles), None);

        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    /// The glyphs are constants this file wrote, so they must reach the page as
    /// drawing instructions rather than as escaped text.
    #[test]
    fn a_layer_draws_its_icon() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/billing.rs", 88)],
            Vec::new(),
        )];
        let html = section("p1", &PageFlow::of(&profiles), None);

        assert!(html.contains("class=\"ic\""), "every box carries a glyph");
        assert!(
            !html.contains("&lt;path"),
            "an escaped icon prints its path data instead of drawing it"
        );
    }

    #[test]
    fn a_query_string_cannot_be_split_by_a_path() {
        assert_eq!(urlencode("phonix-db/src/a.rs"), "phonix-db/src/a.rs");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }

    /// Only the database edge is labelled with a time. A count on the others is
    /// fine; a duration would be invented.
    #[test]
    fn only_the_database_edge_shows_a_duration() {
        let caller = Caller::none();
        let query = Query {
            sql: "SELECT 1".into(),
            caller,
            elapsed: Some(Duration::from_millis(4)),
            rows_returned: None,
            rows_affected: None,
        };
        let profiles = vec![profile_with(Vec::new(), vec![query])];
        let flow = Flow::of(&profiles);

        for edge in &flow.edges {
            if edge.to != "postgres" {
                assert!(edge.elapsed.is_none());
            }
        }
    }

    #[test]
    fn a_single_request_page_load_draws_no_phase_strip() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/billing.rs", 88)],
            Vec::new(),
        )];
        let html = section("p1", &PageFlow::of(&profiles), None);

        assert!(
            !html.contains("whole load"),
            "one request is not a choice worth offering"
        );
    }

    /// The commonest empty case, and the one that reads as a bug if it says
    /// nothing: a document that queried nothing and logged nothing.
    #[test]
    fn a_diagram_with_nothing_in_it_says_why() {
        let profiles = vec![profile_with(Vec::new(), Vec::new())];
        let html = section("p1", &PageFlow::of(&profiles), None);

        assert!(html.contains("Nothing recorded here"));
        assert!(
            html.contains("profiler.filter"),
            "an all-grey diagram has to name the knob that widens it"
        );
    }

    #[test]
    fn the_boxes_fit_inside_the_canvas() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/billing.rs", 88)],
            Vec::new(),
        )];
        let rows = rows_of(&Flow::of(&profiles));

        assert!(x_of(COLUMNS - 1) + BOX_W + PAD <= width());
        assert!(rows[ROWS - 1].y + rows[ROWS - 1].height + PAD <= height(&rows));
    }

    /// The compaction that makes the panel worth looking at: a page load that
    /// touched two layers must not spend six full rows saying so.
    #[test]
    fn rows_nothing_touched_are_squeezed() {
        let profiles = vec![profile_with(
            vec![log_at("phonix-services/src/billing.rs", 88)],
            Vec::new(),
        )];
        let flow = Flow::of(&profiles);
        let rows = rows_of(&flow);

        let services = flow.node("phonix-services").expect("on the spine").row;

        assert!(rows[services].live, "the layer that ran keeps its full row");
        assert!(
            rows.iter().filter(|row| !row.live).count() >= 4,
            "the rows nothing touched should be bands"
        );
        assert!(
            height(&rows) < PAD * 2 + ROWS * BOX_H + (ROWS - 1) * GAP_Y,
            "a squeezed diagram must be shorter than a full one"
        );
    }
}
