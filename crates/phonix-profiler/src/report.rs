//! The report: server-rendered HTML, no framework, no build step.
//!
//! Drawn on the server for a reason worth keeping even after there is a
//! bundle: the profiler has to work on a page whose application has panicked,
//! on a build that is half-finished, and against a server whose tenant
//! resolution is broken. Every dependency it takes on is another way to be
//! unavailable at the moment it is wanted. Expanding a stack is `<details>`,
//! not a click handler, for exactly that reason.
//!
//! # There is a script now, and it changes none of that
//!
//! `report.js` adds the tab strip, the source modal and the sidebar toggle. It
//! is compiled into this binary and served from `/_profiler/report.js`, like
//! `toolbar.js` beside it - so it is not a dependency on anything outside this
//! process, which is what the paragraph above is actually about.
//!
//! **Every page here must still be complete without it.** Tabs are cards that
//! are otherwise stacked, the modal link is an ordinary `<a href>` to a page
//! that exists, and the diagram's layer panels are `:target`, not a handler. If
//! the script fails to parse, the report is the page it was before it existed.
//! Anything added here that cannot degrade that way belongs on the server.
//!
//! The styling is inline and deliberately unlike the application's. This is a
//! tool, not a screen, and it should never be mistaken for one in a
//! screenshot.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use crate::flow::PageFlow;
use crate::highlight::{self, Lang};
use crate::page::PageSummary;
use crate::profile::{Kind, Profile, Query, Token};
use crate::source::Snippet;

/// Wrap a body in the shell.
///
/// `crumb` is what this page is, shown after the brand; `actions` is the
/// right-hand side of the header; `nav` is the context-specific half of the
/// sidebar, already rendered, and may be empty.
///
/// The `<main>` carries `data-tabs`: `report.js` turns the cards inside it into
/// a tab strip if there is more than one. Without the script they stay stacked,
/// which is what this page has always been.
pub fn page(title: &str, crumb: &str, actions: &str, nav: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\"/>\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"/>\n\
         <title>{title} · phonix profiler</title>\n\
         <style>{STYLE}</style>\n\
         </head>\n\
         <body>\n\
         <header class=\"top\">\n\
         <button class=\"side-toggle\" data-side-toggle aria-expanded=\"false\" \
         aria-label=\"Menu\">≡</button>\n\
         <a class=\"brand\" href=\"/_profiler\"><span class=\"mark\">◆</span> \
         phonix <b>profiler</b></a>\n\
         <span class=\"crumb\">{crumb}</span>\n\
         <span class=\"grow\"></span>\n\
         {actions}\n\
         </header>\n\
         <div class=\"shell\">\n\
         <aside class=\"side\"><nav>\n\
         <a class=\"side-link\" href=\"/_profiler\">All requests</a>\n\
         {nav}\n\
         </nav></aside>\n\
         <main class=\"wrap\" data-tabs>{body}</main>\n\
         </div>\n\
         <script src=\"/_profiler/report.js\" defer></script>\n\
         </body>\n\
         </html>\n",
        title = escape(title),
        crumb = escape(crumb),
    )
}

/// One link in the sidebar.
///
/// `current` draws it as where you already are rather than somewhere to go.
pub(crate) fn side_link(nav: &mut String, href: &str, label: &str, current: bool) {
    let _ = write!(
        nav,
        "<a class=\"side-link{on}\" href=\"{href}\">{label}</a>",
        on = if current { " on" } else { "" },
        href = escape(href),
        label = escape(label),
    );
}

/// The index: what this process has seen, newest first.
pub fn index(profiles: &[Arc<Profile>], show_all: bool, held: usize) -> String {
    let mut body = String::new();

    let actions = format!(
        "<span class=\"held\">{held} held</span>\
         <a class=\"button{active}\" href=\"/_profiler{query}\">{label}</a>",
        active = if show_all { " on" } else { "" },
        query = if show_all { "" } else { "?all=1" },
        label = if show_all {
            "assets shown"
        } else {
            "show assets"
        },
    );

    if profiles.is_empty() {
        body.push_str(&empty(
            "Nothing recorded yet",
            "Load a page in the application and come back. Profiles live in \
             memory for this process only, so a restart clears them - and the \
             watcher restarts on every save.",
        ));

        return page("Requests", "requests", &actions, "", &body);
    }

    // Read off the rows on screen rather than the whole ring: these have to
    // describe the table underneath them or they are just decoration.
    let slowest = profiles
        .iter()
        .map(|profile| profile.duration)
        .max()
        .unwrap_or_default();
    let failed = profiles
        .iter()
        .filter(|profile| profile.status >= 400)
        .count();
    let statements: usize = profiles.iter().map(|profile| profile.queries.len()).sum();

    metrics(
        &mut body,
        &[
            Metric::plain("shown", profiles.len().to_string()),
            Metric::timed("slowest", slowest),
            Metric::plain("statements", statements.to_string()),
            Metric::flagged("failed", failed.to_string(), failed > 0),
        ],
    );

    body.push_str("<section class=\"card\">");
    body.push_str(
        "<table>\n<thead><tr>\
         <th>time</th><th>kind</th><th class=\"num\">status</th><th>route</th>\
         <th class=\"num\">took</th><th class=\"num\">sql</th><th class=\"num\">n</th>\
         <th>tenant</th><th></th></tr></thead>\n<tbody>\n",
    );

    for profile in profiles {
        let _ = writeln!(
            body,
            "<tr class=\"{status_class}\">\
             <td class=\"dim mono nowrap\"><a href=\"/_profiler/{token}\">{when}</a></td>\
             <td>{kind}</td>\
             <td class=\"num\">{status}</td>\
             <td class=\"route\"><a href=\"/_profiler/{token}\">\
             <span class=\"method\">{method}</span> <span class=\"mono\">{route}</span></a></td>\
             <td class=\"num\">{time}</td>\
             <td class=\"num dim\">{sql}</td>\
             <td class=\"num dim\">{queries}</td>\
             <td class=\"dim\">{tenant}</td>\
             <td class=\"num\">{group}</td>\
             </tr>",
            status_class = status_class(profile.status),
            token = profile.token,
            when = profile.at.format("%H:%M:%S%.3f"),
            kind = kind_pill(profile.kind),
            status = status_chip(profile.status),
            method = escape(&profile.method),
            route = escape(profile.route_or_path()),
            time = timing(profile.duration),
            sql = if profile.queries.is_empty() {
                "-".to_owned()
            } else {
                timing(profile.query_time())
            },
            queries = if profile.queries.is_empty() {
                "-".to_owned()
            } else {
                profile.queries.len().to_string()
            },
            tenant = profile
                .tenant
                .as_deref()
                .map(escape)
                .unwrap_or_else(|| "-".to_owned()),
            // The group, when the request belongs to one. An asset and a
            // health probe do not, and never will - nothing on those paths
            // sends the header and neither is a navigation.
            group = profile
                .page
                .as_deref()
                .map(|id| {
                    format!(
                        "<a class=\"chip\" href=\"/_profiler/page/{id}\">page&nbsp;load</a>",
                        id = escape(id)
                    )
                })
                .unwrap_or_default(),
        );
    }

    body.push_str("</tbody>\n</table>\n</section>\n");

    page("Requests", "requests", &actions, "", &body)
}

/// One request, in full.
pub fn detail(profile: &Profile) -> String {
    let mut body = String::new();
    let repeats = profile.repeated_queries().len();

    let _ = write!(
        body,
        "<div class=\"title\">{kind}<span class=\"method big\">{method}</span>\
         <h1 class=\"mono\">{path}</h1>{status}</div>",
        kind = kind_pill(profile.kind),
        method = escape(&profile.method),
        path = escape(&profile.path),
        status = status_chip(profile.status),
    );

    metrics(
        &mut body,
        &[
            Metric::timed("took", profile.duration),
            Metric::timed("sql", profile.query_time()),
            Metric::plain("statements", profile.queries.len().to_string()),
            Metric::flagged("repeated", repeats.to_string(), repeats > 0),
        ],
    );

    body.push_str("<section class=\"card\"><h2>Request</h2><dl class=\"facts\">");
    fact(&mut body, "token", &profile.token.to_string());
    fact(&mut body, "at", &profile.at.to_rfc3339());
    fact(
        &mut body,
        "route",
        profile.route.as_deref().unwrap_or("(no route matched)"),
    );

    if let Some(query_string) = &profile.query_string {
        fact(&mut body, "query string", query_string);
    }

    fact(
        &mut body,
        "tenant",
        profile.tenant.as_deref().unwrap_or("(none)"),
    );

    // A link, not a value: from one slow server function, the group it belongs
    // to is the next thing anybody wants, and it is the view that can show an
    // N+1 spread across its siblings.
    if let Some(page_id) = &profile.page {
        let _ = write!(
            body,
            "<dt>page load</dt><dd><a href=\"/_profiler/page/{id}\">{id}</a></dd>",
            id = escape(page_id),
        );
    }

    fact(
        &mut body,
        "response size",
        &profile
            .response_bytes
            .map(bytes)
            .unwrap_or_else(|| "streamed - not measured".to_owned()),
    );
    fact(
        &mut body,
        "process rss",
        &profile
            .rss_bytes
            .map(bytes)
            .unwrap_or_else(|| "unavailable on this platform".to_owned()),
    );
    body.push_str("</dl>");
    body.push_str(
        "<p class=\"note\">Process RSS is the whole process, not this request. \
         Rust has no per-request memory figure; this one is a gauge of whether the \
         process is growing, and nothing more.</p>",
    );
    body.push_str("</section>");

    repeated(&mut body, profile);
    queries(&mut body, profile);
    logs(&mut body, profile);

    let mut nav = String::new();

    if let Some(group) = &profile.page {
        side_link(
            &mut nav,
            &format!("/_profiler/page/{group}"),
            "This page load",
            false,
        );
    }

    side_link(
        &mut nav,
        &format!("/_profiler/{}", profile.token),
        "This request",
        true,
    );
    side_link(
        &mut nav,
        &format!("/_profiler/api/{}", profile.token),
        "As JSON",
        false,
    );

    page(
        &format!("{} {}", profile.method, profile.path),
        "request",
        "<a class=\"button\" href=\"/_profiler\">all requests</a>",
        &nav,
        &body,
    )
}

/// One page load: every request a single screen produced.
///
/// The report `docs/adr/0004-development-profiler.md` section 2 is really
/// about. A per-request view cannot answer "why is this screen slow" when the
/// answer is spread across nine server functions.
pub fn page_load(summary: &PageSummary, flow: &PageFlow, phase: Option<Token>) -> String {
    let mut body = String::new();
    let actions = "<a class=\"button\" href=\"/_profiler\">all requests</a>";

    let _ = write!(
        body,
        "<div class=\"title\"><h1>Page load</h1><code class=\"chip\">{page}</code></div>",
        page = escape(&summary.page),
    );

    if summary.requests == 0 {
        body.push_str(&empty(
            "Nothing recorded for this page load",
            "Either the screen has made no server calls yet, or the process has \
             restarted since - the watcher does that on every save, and profiles \
             do not survive it.",
        ));

        return page("Page load", "page load", actions, "", &body);
    }

    metrics(
        &mut body,
        &[
            Metric::plain("requests", summary.requests.to_string()),
            Metric::timed("server time", summary.duration),
            Metric::timed("sql", summary.sql),
            Metric::flagged("failed", summary.errors.to_string(), summary.errors > 0),
        ],
    );

    body.push_str(
        "<p class=\"note lead\">Server time is summed across the group, not elapsed: \
         these calls overlap, and the wall clock between the first and the last \
         would also be counting how long somebody sat looking at the screen.</p>",
    );

    if !summary.has_document {
        body.push_str(
            "<p class=\"note lead\">No document request in this group. That is what an \
             in-app navigation looks like - the browser never asked the server for a \
             page - and it is not a gap in the recording.</p>",
        );
    }

    body.push_str(&crate::diagram::section(&summary.page, flow, phase));

    if !summary.repeated.is_empty() {
        body.push_str("<section class=\"card warnish\"><h2>Repeated across the page load</h2>");
        body.push_str(
            "<p class=\"note\">The same statement run more than once by this screen, \
             counted across every request in it. One statement in each of eleven \
             server functions is an N+1 that no single profile can show you.</p>",
        );
        body.push_str(
            "<table><thead><tr><th class=\"num\">times</th><th>statement</th>\
             </tr></thead><tbody>",
        );

        for (shape, count) in &summary.repeated {
            let _ = write!(
                body,
                "<tr><td class=\"num warn strong\">{count}</td>\
                 <td><code class=\"sql\">{}</code></td></tr>",
                highlight::block(Lang::Sql, shape)
            );
        }

        body.push_str("</tbody></table></section>");
    }

    let _ = write!(
        body,
        "<section class=\"card\"><h2>Requests <span class=\"count\">{}</span></h2>",
        summary.requests
    );
    body.push_str(
        "<table><thead><tr><th>kind</th><th class=\"num\">status</th><th>route</th>\
         <th class=\"num\">took</th><th class=\"num\">n</th><th></th></tr></thead><tbody>",
    );

    for entry in &summary.profiles {
        let _ = write!(
            body,
            "<tr class=\"{status_class}\">\
             <td>{kind}</td>\
             <td class=\"num\">{status}</td>\
             <td class=\"route\"><a href=\"/_profiler/{token}\">\
             <span class=\"method\">{method}</span> <span class=\"mono\">{route}</span></a></td>\
             <td class=\"num\">{time}</td>\
             <td class=\"num dim\">{queries}</td>\
             <td class=\"num\"><a class=\"chip\" href=\"/_profiler/{token}\">open</a></td></tr>",
            status_class = status_class(entry.status),
            kind = kind_pill(entry.kind),
            status = status_chip(entry.status),
            token = entry.token,
            method = escape(&entry.method),
            route = escape(entry.route.as_deref().unwrap_or(&entry.path)),
            time = timing(entry.duration),
            queries = entry.queries,
        );
    }

    body.push_str("</tbody></table></section>");

    // The sidebar is the group itself: every request in this page load, so
    // moving between them never goes back through the index.
    let mut nav = String::new();

    side_link(
        &mut nav,
        &format!("/_profiler/page/{}", summary.page),
        "This page load",
        true,
    );

    for entry in &summary.profiles {
        side_link(
            &mut nav,
            &format!("/_profiler/{}", entry.token),
            &format!(
                "{} {}",
                entry.method,
                entry.route.as_deref().unwrap_or(&entry.path)
            ),
            false,
        );
    }

    page("Page load", "page load", actions, &nav, &body)
}

/// The token resolved to nothing.
///
/// Takes the raw text rather than a `Token`, so that a malformed token is
/// echoed as what was actually asked for instead of as some placeholder number
/// that was never in a URL.
pub fn missing(token: &str) -> String {
    page(
        "Not found",
        "not found",
        "<a class=\"button\" href=\"/_profiler\">all requests</a>",
        "",
        &empty(
            &format!("No profile {}", escape(token)),
            "Profiles live in memory and only for this process. A restart - which \
             the watcher does on every save - drops all of them, and the oldest are \
             evicted once the ring is full.",
        ),
    )
}

/// The N+1 panel for one request.
fn repeated(body: &mut String, profile: &Profile) {
    let repeats = profile.repeated_queries();

    if repeats.is_empty() {
        return;
    }

    body.push_str("<section class=\"card warnish\"><h2>Repeated statements</h2>");
    body.push_str(
        "<p class=\"note\">The same statement run more than once in this one request. \
         Usually a loop that should have been a single query.</p>",
    );
    body.push_str(
        "<table><thead><tr><th class=\"num\">times</th><th>statement</th></tr></thead><tbody>",
    );

    for (shape, count) in repeats {
        let _ = write!(
            body,
            "<tr><td class=\"num warn strong\">{count}</td>\
             <td><code class=\"sql\">{}</code></td></tr>",
            highlight::block(Lang::Sql, &shape)
        );
    }

    body.push_str("</tbody></table></section>");
}

fn queries(body: &mut String, profile: &Profile) {
    let _ = write!(
        body,
        "<section class=\"card\"><h2>Statements <span class=\"count\">{}</span></h2>",
        profile.queries.len()
    );

    if profile.queries.is_empty() {
        body.push_str(
            "<p class=\"empty-line\">None. If that is a surprise, check that the \
             profiler's filter still lets <code>sqlx::query=debug</code> through - \
             it is separate from the one in <code>[telemetry]</code>.</p></section>",
        );

        return;
    }

    body.push_str(
        "<table class=\"statements\"><thead><tr><th class=\"num\">#</th>\
         <th class=\"num\">took</th><th class=\"num\">rows</th>\
         <th>statement, and the code that ran it</th></tr></thead><tbody>",
    );

    for (index, query) in profile.queries.iter().enumerate() {
        let rows = match (query.rows_returned, query.rows_affected) {
            (Some(returned), _) => returned.to_string(),
            (None, Some(affected)) => format!("{affected}*"),
            (None, None) => "-".to_owned(),
        };

        let _ = write!(
            body,
            "<tr><td class=\"num dim\">{n}</td><td class=\"num\">{time}</td>\
             <td class=\"num dim\">{rows}</td>\
             <td><code class=\"sql\">{sql}</code>{stack}</td></tr>",
            n = index + 1,
            time = query.elapsed.map(timing).unwrap_or_else(|| "-".to_owned()),
            sql = highlight::block(Lang::Sql, &query.sql),
            stack = stack(query),
        );
    }

    body.push_str("</tbody></table></section>");
}

/// The call stack behind one statement, collapsed.
///
/// `<details>` rather than a click handler: this page has no JavaScript, and a
/// stack open by default would bury the statement it belongs to. The summary
/// line is the innermost workspace frame, which is the answer nine times in
/// ten - the rest is there for the tenth.
///
/// Resolving happens here, at render, and not when the statement was recorded.
/// See `crate::caller` for why that split is the whole design.
fn stack(query: &Query) -> String {
    let frames = query.caller.resolve();
    let Some(innermost) = frames.first() else {
        return String::new();
    };

    let mut rendered = String::new();

    let _ = write!(
        rendered,
        "<details class=\"stack\"><summary><span class=\"fn\">{function}</span>\
         <span class=\"at\">{position}</span></summary><ol>",
        function = escape(&innermost.function),
        position = escape(&innermost.position()),
    );

    for frame in &frames {
        let _ = write!(
            rendered,
            "<li><span class=\"fn\">{function}</span><span class=\"at\">{position}</span></li>",
            function = escape(&frame.function),
            position = escape(&frame.position()),
        );
    }

    rendered.push_str("</ol></details>");
    rendered
}

fn logs(body: &mut String, profile: &Profile) {
    let _ = write!(
        body,
        "<section class=\"card\"><h2>Log <span class=\"count\">{}</span></h2>",
        profile.logs.len()
    );

    if profile.logs.is_empty() {
        body.push_str("<p class=\"empty-line\">Nothing logged.</p></section>");

        return;
    }

    body.push_str(
        "<table><thead><tr><th>level</th><th>target</th><th>message</th>\
         <th>where</th></tr></thead><tbody>",
    );

    for line in &profile.logs {
        let fields = line
            .fields
            .iter()
            .map(|(key, value)| {
                format!(
                    "<span class=\"field\"><i>{}</i>{}</span>",
                    escape(key),
                    escape(value)
                )
            })
            .collect::<Vec<_>>()
            .join("");

        let _ = write!(
            body,
            "<tr><td><span class=\"level level-{level_slug}\">{level}</span></td>\
             <td class=\"dim mono nowrap\">{target}</td>\
             <td>{message}{fields}</td>\
             <td class=\"at nowrap\">{position}</td></tr>",
            level_slug = line.level.to_lowercase(),
            level = escape(&line.level),
            target = escape(&line.target),
            message = escape(&line.message),
            // A dependency's line has no position worth printing - see
            // `collect::Fields::into_log`.
            position = line
                .position()
                .map(|at| escape(&at))
                .unwrap_or_else(|| "-".to_owned()),
        );
    }

    body.push_str("</tbody></table></section>");
}

/// One figure and what it is.
struct Metric<'a> {
    label: &'a str,
    value: String,
    /// The class on the value, or nothing.
    tone: &'static str,
}

impl<'a> Metric<'a> {
    fn plain(label: &'a str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            tone: "",
        }
    }

    /// Drawn in the warning colour. For a count that is only interesting when
    /// it is not zero.
    fn flagged(label: &'a str, value: impl Into<String>, flag: bool) -> Self {
        Self {
            label,
            value: value.into(),
            tone: if flag { " warn" } else { "" },
        }
    }

    /// A measured duration, coloured against [`SLOW`].
    fn timed(label: &'a str, duration: Duration) -> Self {
        Self {
            label,
            value: millis(duration),
            tone: tone_of(duration),
        }
    }
}

/// Above this, a duration is drawn as a problem; below it, as fine.
///
/// One threshold for every timing on every panel rather than one per kind of
/// measurement. A page load, a request and a single statement are very
/// different things to be spending half a second on, and a per-panel threshold
/// would be three numbers to keep in your head instead of one. If this ever
/// needs splitting, split it here.
const SLOW: Duration = Duration::from_millis(500);

pub(crate) fn tone_of(duration: Duration) -> &'static str {
    if duration >= SLOW { " bad" } else { " ok" }
}

/// A duration for a table cell, coloured the same way as a metric tile.
fn timing(duration: Duration) -> String {
    format!(
        "<span class=\"t{tone}\">{value}</span>",
        tone = tone_of(duration),
        value = escape(&millis(duration)),
    )
}

fn metrics(body: &mut String, tiles: &[Metric<'_>]) {
    body.push_str("<div class=\"metrics\">");

    for tile in tiles {
        let _ = write!(
            body,
            "<div class=\"metric\"><span class=\"value{flag}\">{value}</span>\
             <span class=\"label\">{label}</span></div>",
            flag = tile.tone,
            value = escape(&tile.value),
            label = escape(tile.label),
        );
    }

    body.push_str("</div>");
}

/// A stated absence, rather than a blank area a developer has to interpret.
fn empty(headline: &str, explanation: &str) -> String {
    format!(
        "<section class=\"card empty\"><p class=\"headline\">{headline}</p>\
         <p class=\"note\">{explanation}</p></section>"
    )
}

fn fact(body: &mut String, name: &str, value: &str) {
    let _ = write!(
        body,
        "<dt>{}</dt><dd class=\"mono\">{}</dd>",
        escape(name),
        escape(value)
    );
}

fn kind_pill(kind: Kind) -> String {
    format!(
        "<span class=\"kind kind-{slug}\">{label}</span>",
        slug = kind_slug(kind),
        label = kind.label(),
    )
}

fn status_chip(status: u16) -> String {
    format!(
        "<span class=\"status {}\">{status}</span>",
        status_class(status)
    )
}

fn kind_slug(kind: Kind) -> &'static str {
    match kind {
        Kind::Document => "document",
        Kind::ServerFn => "serverfn",
        Kind::Api => "api",
        Kind::Asset => "asset",
        Kind::File => "file",
        Kind::Other => "other",
    }
}

fn status_class(status: u16) -> &'static str {
    match status {
        200..=299 => "ok",
        300..=399 => "redirect",
        400..=499 => "client-error",
        _ => "server-error",
    }
}

/// A duration in milliseconds, to one decimal place.
///
/// Milliseconds throughout, even for something that took eleven microseconds,
/// so a column of numbers can be compared by eye without reading the units.
pub(crate) fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn bytes(count: u64) -> String {
    const KIB: f64 = 1024.0;

    let value = count as f64;

    if value < KIB {
        return format!("{count} B");
    }
    if value < KIB * KIB {
        return format!("{:.1} KiB", value / KIB);
    }

    format!("{:.1} MiB", value / (KIB * KIB))
}

/// Escape text for HTML.
///
/// Everything drawn here comes from a request: a path, a header, a SQL string,
/// a log message, a symbol name read out of the binary. All of it is
/// attacker-influenced in the sense that matters - the developer reading it is
/// the one being attacked - so nothing reaches the page without passing
/// through this.
/// One file, around one line, as a page of its own.
///
/// A page rather than a panel, because the report has no script - see
/// [`crate::diagram`]. It also means the only thing that ever reads a file is
/// a navigation a human made, which is a much smaller surface than an endpoint
/// a page fetches from.
pub fn source(page_id: &str, snippet: &Snippet) -> String {
    let mut body = String::new();

    let _ = write!(
        body,
        "<div class=\"title\"><h1 class=\"mono\">{file}</h1>         <code class=\"chip\">line {line}</code></div>",
        file = escape(&snippet.file),
        line = snippet.line,
    );

    body.push_str("<section class=\"card\"><h2>Source</h2><pre class=\"src\">");

    // Coloured across the whole window rather than line by line: a block
    // comment or a multi-line string starts on one line and ends on another,
    // and a lexer restarted at every newline gets both wrong. `per_line` closes
    // and reopens the span at each boundary instead - see `crate::highlight`.
    let coloured = highlight::per_line(lang_of(&snippet.file), &snippet.lines.join("\n"));

    for (offset, text) in coloured.iter().enumerate() {
        let number = snippet.start + offset as u32;
        let marked = if number == snippet.line { " here" } else { "" };

        let _ = writeln!(
            body,
            "<span class=\"row{marked}\"><span class=\"ln\">{number}</span>{text}</span>",
        );
    }

    body.push_str("</pre></section>");

    let actions = format!(
        "<a class=\"button\" href=\"/_profiler/page/{page}\">back to the page load</a>",
        page = escape(page_id),
    );

    page("Source", "source", &actions, "", &body)
}

/// Which colouring a file gets.
///
/// Everything under `crates/` is Rust, so this is one `if` and a default rather
/// than a table. `.sql` is here because the migrations are real files a frame
/// could one day name.
fn lang_of(file: &str) -> Lang {
    if file.ends_with(".sql") {
        Lang::Sql
    } else {
        Lang::Rust
    }
}

/// The source view could not show what was asked for.
///
/// One message for every refusal, on purpose. "Not recorded here" and "not on
/// disk" are different to us and must not be different to whoever is asking -
/// see [`crate::source`].
pub fn no_source(page_id: &str) -> String {
    let actions = format!(
        "<a class=\"button\" href=\"/_profiler/page/{page}\">back to the page load</a>",
        page = escape(page_id),
    );

    page(
        "Not found",
        "source",
        &actions,
        "",
        &empty(
            "No source for that",
            "The report only shows files this page load actually recorded a frame              in, read from this checkout. A file that was not on one of these              stacks is not readable here, and neither is anything outside              crates/.",
        ),
    )
}

pub(crate) fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());

    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }

    escaped
}

const STYLE: &str = "\
:root{color-scheme:dark;\
--bg:#0e1014;--panel:#15181e;--raised:#1b1f27;--line:#252b36;--line-soft:#1e232c;\
--text:#e2e6ee;--dim:#8b94a7;--faint:#5d6675;\
--brand:#7aa2f7;--ok:#8fce6d;--warn:#e5b567;--bad:#f77f8e;--info:#7fd1de;\
--ui:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;\
--mono:ui-monospace,SFMono-Regular,Menlo,Consolas,'Liberation Mono',monospace}\
*{box-sizing:border-box}\
html,body{margin:0;padding:0}\
body{background:var(--bg);color:var(--text);font:13px/1.55 var(--ui);\
-webkit-font-smoothing:antialiased}\
a{color:var(--brand);text-decoration:none}\
a:hover{text-decoration:underline}\
.mono,code{font-family:var(--mono)}\
.dim{color:var(--dim)}\
.nowrap{white-space:nowrap}\
.strong{font-weight:650}\
.warn{color:var(--warn)}\
.grow{flex:1 1 auto}\
\
.top{position:sticky;top:0;z-index:10;display:flex;align-items:center;gap:.85rem;\
padding:.6rem 1.1rem;background:rgba(14,16,20,.92);backdrop-filter:blur(8px);\
border-bottom:1px solid var(--line)}\
.brand{color:var(--text);font-weight:600;letter-spacing:-.01em;white-space:nowrap}\
.brand:hover{text-decoration:none}\
.brand b{font-weight:600;color:var(--brand)}\
.brand .mark{color:var(--brand);margin-right:.15rem}\
.crumb{color:var(--faint);font-size:12px;text-transform:uppercase;letter-spacing:.09em;\
border-left:1px solid var(--line);padding-left:.85rem}\
.held{color:var(--faint);font-size:12px}\
.button{display:inline-block;padding:.28rem .7rem;border:1px solid var(--line);\
border-radius:6px;color:var(--dim);font-size:12px;background:var(--panel)}\
.button:hover{border-color:var(--brand);color:var(--text);text-decoration:none}\
.button.on{border-color:var(--brand);color:var(--brand)}\
\
.wrap{min-width:0;padding:1.4rem 1.1rem 4rem}\
\
/* shell: sidebar beside the report */\
.shell{display:grid;grid-template-columns:216px minmax(0,1fr);max-width:1600px;margin:0 auto;align-items:start}\
.side{position:sticky;top:49px;align-self:start;max-height:calc(100vh - 49px);overflow-y:auto;padding:1.1rem .6rem;border-right:1px solid var(--line-soft)}\
.side nav{display:flex;flex-direction:column;gap:1px}\
.side-link{display:block;padding:.4rem .6rem;border-radius:6px;color:var(--dim);font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}\
.side-link:hover{background:var(--raised);color:var(--text);text-decoration:none}\
.side-link.on{background:var(--raised);color:var(--text);box-shadow:inset 2px 0 0 var(--brand)}\
.side-toggle{display:none;background:none;border:0;color:var(--dim);font-size:18px;line-height:1;cursor:pointer;padding:0 .2rem}\
\
/* tabs: built from the cards by report.js, absent without it */\
.tabs{display:flex;flex-wrap:wrap;gap:.2rem;margin:0 0 .9rem;border-bottom:1px solid var(--line-soft)}\
.tab{background:none;border:0;border-bottom:2px solid transparent;color:var(--dim);font:inherit;font-size:12px;padding:.45rem .7rem;cursor:pointer;display:flex;align-items:center;gap:.35rem}\
.tab:hover{color:var(--text)}\
.tab.on{color:var(--text);border-bottom-color:var(--brand)}\
.tab .count{color:var(--faint);font-size:11px}\
\
/* drawer: the code slides in beside the request, never over it */\
html.drawer-open{overflow:hidden}\
.drawer{position:fixed;inset:0;z-index:50;display:flex;justify-content:flex-end}\
.drawer-back{position:absolute;inset:0;background:rgba(6,8,11,.55)}\
.drawer-box{position:relative;display:flex;flex-direction:column;width:min(720px,100%);height:100%;background:var(--panel);border-left:1px solid var(--line);overflow:hidden;box-shadow:-16px 0 44px rgba(0,0,0,.45);animation:slide-in .16s ease-out}\
@keyframes slide-in{from{transform:translateX(2rem);opacity:.4}to{transform:none;opacity:1}}\
@media (prefers-reduced-motion:reduce){.drawer-box{animation:none}}\
.drawer-top{display:flex;align-items:center;gap:.8rem;padding:.6rem .9rem;border-bottom:1px solid var(--line-soft)}\
.drawer-title{font-family:var(--mono);font-size:12px;color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1 1 auto}\
.drawer-open{font-size:11px;color:var(--dim);white-space:nowrap}\
.drawer-x{background:none;border:0;color:var(--dim);font-size:20px;line-height:1;cursor:pointer;padding:0 .2rem}\
.drawer-x:hover{color:var(--text)}\
.drawer-body{overflow:auto;padding:0;flex:1 1 auto}\
.drawer-body .title{padding:.7rem .95rem 0}\
.drawer-body .card{border-radius:0;border-left:0;border-right:0;border-top:0}\
.drawer-body .card>h2{display:none}\
\
/* syntax colouring, emitted by crate::highlight */\
.k{color:#c792ea}\
.s{color:#a5d6a7}\
.n{color:#f5c77e}\
.c{color:var(--faint);font-style:italic}\
.t{color:#7fd1de}\
.f{color:#82aaff}\
.a{color:#8b94a7}\
.l{color:#e5b567}\
.m{color:#82aaff}\
.v{color:#f77f8e}\
code.sql .k{color:#7aa2f7}\
.title{display:flex;align-items:center;gap:.6rem;flex-wrap:wrap;margin-bottom:1.1rem}\
h1{font-size:1.05rem;font-weight:600;margin:0;letter-spacing:-.01em;word-break:break-all}\
.method.big{font-size:.8rem}\
.note.lead{margin:-.4rem 0 1.1rem;max-width:78ch}\
\
.metrics{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));\
gap:.7rem;margin-bottom:1.2rem}\
.metric{background:var(--panel);border:1px solid var(--line);border-radius:8px;\
padding:.7rem .85rem;display:flex;flex-direction:column;gap:.15rem}\
.metric .value{font-family:var(--mono);font-size:1.15rem;font-weight:600;\
letter-spacing:-.02em}\
.metric .label{color:var(--faint);font-size:11px;text-transform:uppercase;\
letter-spacing:.08em}\
\
.card{background:var(--panel);border:1px solid var(--line);border-radius:10px;\
margin-bottom:1.1rem;overflow:hidden}\
.card.warnish{border-color:#4a3d24}\
.card>h2{font-size:12px;font-weight:600;text-transform:uppercase;letter-spacing:.09em;\
color:var(--dim);margin:0;padding:.7rem .95rem;border-bottom:1px solid var(--line-soft);\
display:flex;align-items:center;gap:.5rem}\
.card>h2 .count{color:var(--faint);font-weight:400;letter-spacing:0}\
.card>.note,.card>.empty-line{margin:0;padding:.7rem .95rem}\
.note{color:var(--dim);font-size:12px}\
.empty-line{color:var(--faint)}\
.card.empty{padding:2.2rem 1.2rem;text-align:center}\
.card.empty .headline{margin:0 0 .4rem;font-size:.95rem;color:var(--text)}\
.card.empty .note{max-width:52ch;margin:0 auto;padding:0}\
\
table{width:100%;border-collapse:collapse;display:block;overflow-x:auto}\
\
/* timings: one threshold, everywhere - crate::report::SLOW */\
.t{font-variant-numeric:tabular-nums}\
.t.ok,.value.ok{color:var(--ok)}\
.t.bad,.value.bad{color:var(--bad);font-weight:600}\
.edge-label.ok{fill:var(--ok)}\
.edge-label.bad{fill:var(--bad);font-weight:600}\
\
/* layer glyphs: stroked, so they take the box's own colour */\
.ic{fill:none;stroke:currentColor;stroke-width:1.5;stroke-linecap:round;stroke-linejoin:round}\
.lay{color:var(--faint)}\
.lay.on{color:var(--brand)}\
.lay.on.ext{color:var(--info)}\
.about{border-top:1px solid var(--line-soft);margin-top:.2rem}\
.about>summary{cursor:pointer;padding:.5rem .95rem;color:var(--faint);font-size:11px;letter-spacing:.04em;text-transform:uppercase}\
.about>summary:hover{color:var(--dim)}\
.about .note{padding:0 .95rem .6rem}\
/* the flow diagram */\
.phases{display:flex;flex-wrap:wrap;gap:.35rem;padding:.7rem .95rem 0}\
.phases .button{font-size:11px}\
.diagram{overflow-x:auto;padding:.8rem .95rem}\
.diagram svg{display:block;max-width:100%;height:auto}\
.lay rect{fill:var(--raised);stroke:var(--line);stroke-width:1}\
.lay text{font-family:var(--ui);pointer-events:none}\
.lay .name{font-size:13px;font-weight:600;fill:var(--faint)}\
.lay .sub{font-size:11px;fill:var(--faint)}\
.lay.on rect{fill:var(--panel);stroke:var(--brand);stroke-width:1.5}\
.lay.on .name{fill:var(--text)}\
.lay.on .sub{fill:var(--dim)}\
.lay.ext rect{stroke-dasharray:4 3}\
.lay.on.ext rect{stroke:var(--info)}\
a:hover>.lay.on rect{stroke:var(--ok)}\
a:focus-visible>.lay rect{stroke:var(--ok);stroke-width:2}\
.edge{fill:none;stroke:var(--brand);opacity:.55}\
.head{fill:var(--brand);opacity:.55}\
.edge-label{font-family:var(--mono);font-size:10px;fill:var(--dim);text-anchor:middle}\
.layer-files{display:none}\
.layer-files:target{display:block;border-top:1px solid var(--line-soft)}\
.layer-files h3{margin:0;padding:.7rem .95rem .2rem;font-size:12px;letter-spacing:.04em;text-transform:uppercase;color:var(--dim)}\
\
/* one file, around one line */\
pre.src{margin:0;padding:.8rem 0;overflow-x:auto;font-family:var(--mono);font-size:12px;line-height:1.55}\
.src .row{display:block;padding:0 .95rem;white-space:pre}\
.src .ln{display:inline-block;width:4ch;margin-right:1.2ch;text-align:right;color:var(--faint);user-select:none}\
.src .row.here{background:var(--raised)}\
.src .row.here .ln{color:var(--warn)}\
thead th{position:sticky;top:0;text-align:left;font-weight:600;font-size:11px;\
text-transform:uppercase;letter-spacing:.07em;color:var(--faint);\
background:var(--raised);padding:.45rem .7rem;white-space:nowrap;\
border-bottom:1px solid var(--line)}\
td{padding:.45rem .7rem;border-bottom:1px solid var(--line-soft);vertical-align:top}\
tbody tr:last-child td{border-bottom:0}\
tbody tr:hover td{background:var(--raised)}\
td.num,th.num{text-align:right;white-space:nowrap}\
td.route{max-width:0;width:45%}\
td.route a{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}\
\
.method{color:var(--faint);font-family:var(--mono);font-size:11px;font-weight:600}\
.kind{display:inline-block;padding:.05rem .45rem;border-radius:5px;font-size:11px;\
font-weight:500;white-space:nowrap;border:1px solid transparent}\
.kind-document{background:#1d2b45;color:#a8c7ff;border-color:#274066}\
.kind-serverfn{background:#1c3328;color:#a7e3c1;border-color:#27503b}\
.kind-api{background:#3a2e1c;color:#f0d19a;border-color:#584627}\
.kind-file{background:#33243c;color:#dcb6ec;border-color:#4b3557}\
.kind-asset,.kind-other{background:var(--raised);color:var(--faint);border-color:var(--line)}\
.status{font-family:var(--mono);font-size:11px;font-weight:650;padding:.05rem .35rem;\
border-radius:4px;background:var(--raised)}\
.status.ok{color:var(--ok)}.status.redirect{color:var(--info)}\
.status.client-error{color:var(--warn)}.status.server-error{color:var(--bad)}\
tr.server-error td,tr.client-error td{background:rgba(247,127,142,.05)}\
.chip{display:inline-block;padding:.05rem .45rem;border:1px solid var(--line);\
border-radius:5px;font-size:11px;color:var(--dim);background:var(--raised);\
font-family:var(--mono)}\
a.chip:hover{border-color:var(--brand);color:var(--brand);text-decoration:none}\
\
code.sql{display:block;font-size:12px;line-height:1.5;white-space:pre-wrap;\
word-break:break-word;color:#cfd8e8}\
.field{display:inline-block;margin-left:.5rem;color:var(--dim);font-family:var(--mono);\
font-size:11px}\
.field i{color:var(--faint);font-style:normal}\
.field i:after{content:'='}\
.level{display:inline-block;min-width:3.2rem;font-family:var(--mono);font-size:11px;\
font-weight:650}\
.level-error{color:var(--bad)}.level-warn{color:var(--warn)}.level-info{color:var(--ok)}\
.level-debug,.level-trace{color:var(--faint)}\
\
.stack{margin-top:.45rem}\
.stack summary{cursor:pointer;list-style:none;display:inline-flex;gap:.5rem;\
flex-wrap:wrap;align-items:baseline;padding:.15rem .5rem;border-radius:5px;\
border:1px solid var(--line-soft);background:var(--raised);max-width:100%}\
.stack summary::-webkit-details-marker{display:none}\
.stack summary:hover{border-color:var(--line)}\
.stack[open] summary{margin-bottom:.35rem}\
.stack ol{margin:0 0 0 .55rem;padding:.3rem 0 .15rem 1.7rem;\
border-left:1px solid var(--line)}\
.stack li{padding:.1rem 0;display:flex;gap:.6rem;flex-wrap:wrap;align-items:baseline}\
.stack li::marker{color:var(--faint);font-size:10px}\
.fn{font-family:var(--mono);font-size:11.5px;color:#c8d3e6;word-break:break-all}\
.at{font-family:var(--mono);font-size:11px;color:var(--faint)}\
\
dl.facts{display:grid;grid-template-columns:max-content 1fr;gap:.3rem 1.1rem;\
margin:0;padding:.8rem .95rem}\
dl.facts dt{color:var(--faint);font-size:12px}\
dl.facts dd{margin:0;font-size:12px;word-break:break-all}\
\
@media (max-width:640px){\
.wrap{padding:1rem .7rem 3rem}\
.crumb{display:none}\
.side-toggle{display:block}\
.shell{grid-template-columns:minmax(0,1fr)}\
.side{display:none;position:static;max-height:none;border-right:0;border-bottom:1px solid var(--line-soft)}\
.shell.side-open .side{display:block}\
.drawer-box{width:100%;border-left:0}\
.metrics{grid-template-columns:repeat(2,1fr)}\
dl.facts{grid-template-columns:1fr;gap:0}\
dl.facts dt{margin-top:.5rem}}\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::Caller;
    use crate::profile::{LogLine, Token};
    use chrono::Utc;

    fn profile() -> Profile {
        Profile {
            token: Token(1),
            at: Utc::now(),
            kind: Kind::Document,
            method: "GET".into(),
            path: "/admin/users".into(),
            query_string: None,
            route: Some("/admin/users".into()),
            status: 200,
            duration: Duration::from_millis(12),
            tenant: Some("acme".into()),
            page: None,
            response_bytes: None,
            queries: Vec::new(),
            logs: Vec::new(),
            rss_bytes: None,
        }
    }

    fn query(sql: &str) -> Query {
        Query {
            sql: sql.to_owned(),
            caller: Caller::none(),
            elapsed: None,
            rows_returned: None,
            rows_affected: None,
        }
    }

    /// The report renders a request's own text back to the developer reading
    /// it. A path is the easiest thing in the world for somebody else to
    /// choose.
    #[test]
    fn a_path_cannot_carry_markup_into_the_report() {
        let mut profile = profile();
        profile.path = "/<script>alert(1)</script>".into();
        profile.route = None;

        let html = detail(&profile);

        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn sql_cannot_carry_markup_either() {
        let mut profile = profile();
        profile.queries.push(query("SELECT '<img onerror=x>'"));

        assert!(!detail(&profile).contains("<img onerror"));
    }

    #[test]
    fn an_empty_index_says_so_rather_than_drawing_an_empty_table() {
        let html = index(&[], false, 0);

        assert!(html.contains("Nothing recorded yet"));
        assert!(!html.contains("<tbody>"));
    }

    #[test]
    fn the_index_links_every_row_to_its_profile() {
        let held = vec![Arc::new(profile())];
        let html = index(&held, false, 1);

        assert!(html.contains("/_profiler/000000000001"));
    }

    /// A streamed response has no content length, and the report has to say
    /// which of "zero bytes" and "not measured" it means.
    #[test]
    fn an_unmeasured_body_is_not_reported_as_empty() {
        let html = detail(&profile());

        assert!(html.contains("not measured"));
    }

    /// A statement with no workspace frame behind it - the pool's own
    /// housekeeping, or a build with `profiler.backtraces` off - draws the
    /// statement and no empty disclosure widget under it.
    #[test]
    fn a_statement_with_no_stack_draws_no_stack() {
        let mut profile = profile();
        profile.queries.push(query("SELECT 1"));

        let html = detail(&profile);

        // The statement is coloured now, so it reaches the page as spans rather
        // than as one string - `crate::highlight`. What matters to this test is
        // unchanged: the statement is drawn, and nothing draws an empty
        // disclosure widget beneath it.
        assert!(html.contains(">SELECT</span>"), "the statement is drawn");
        assert!(html.contains(">1</span>"));
        assert!(!html.contains("<details"));
    }

    /// A log line from a dependency has no position in this repository, and a
    /// blank cell would read as a missing value rather than an absent one.
    #[test]
    fn a_log_line_without_a_position_shows_a_dash() {
        let mut profile = profile();
        profile.logs.push(LogLine {
            level: "INFO".into(),
            target: "hyper::server".into(),
            message: "connection".into(),
            fields: Vec::new(),
            source: None,
            line: None,
            caller: crate::Caller::none(),
        });

        assert!(detail(&profile).contains("<td class=\"at nowrap\">-</td>"));
    }

    #[test]
    fn a_log_line_with_a_position_shows_it() {
        let mut profile = profile();
        profile.logs.push(LogLine {
            level: "INFO".into(),
            target: "phonix_server::middleware".into(),
            message: "unknown tenant".into(),
            fields: Vec::new(),
            source: Some("phonix-server/src/middleware.rs".into()),
            line: Some(51),
            caller: crate::Caller::none(),
        });

        assert!(detail(&profile).contains("phonix-server/src/middleware.rs:51"));
    }

    /// The group with no document member is the in-app navigation case, and
    /// saying nothing there makes a correct report look like a broken one.
    #[test]
    fn a_page_load_with_no_document_explains_itself() {
        let summary = PageSummary {
            page: "abc".into(),
            requests: 1,
            duration: Duration::from_millis(5),
            sql: Duration::ZERO,
            queries: 0,
            errors: 0,
            repeated: Vec::new(),
            has_document: false,
            profiles: Vec::new(),
        };

        assert!(page_load(&summary, &PageFlow::of(&[]), None).contains("in-app navigation"));
    }
}
