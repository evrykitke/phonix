//! What one profiled request is, and the small vocabulary it is described in.
//!
//! Every field here answers a question a developer asks out loud while looking
//! at a screen that is wrong. Nothing is recorded because it was available.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Serialize, Serializer};

use crate::caller::Caller;

/// The identifier a profile is fetched by.
///
/// Short, opaque and per-process. It is a counter mixed with a seed taken once
/// at startup rather than a UUID, for two reasons: it is six characters in a
/// response header and a URL bar, and a restart changes the seed - so a token
/// the browser is still holding from the previous build resolves to "not
/// found" instead of silently to a different request.
///
/// Not a secret. The index lists every token this process holds, and the whole
/// surface is refused in production twice over - see
/// `docs/adr/0004-development-profiler.md` section 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token(pub u64);

/// On the wire a token is its hex rendering, never the number underneath.
///
/// Everything that consumes one - the URL of a report, the `X-Debug-Token`
/// header, a link the toolbar builds out of this JSON - speaks the twelve-hex
/// form, and [`Token::from_str`] only parses that. Serialising the `u64`
/// instead hands a consumer a value that looks usable, parses as a different
/// number, and 404s.
impl Serialize for Token {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:012x}", self.0)
    }
}

impl std::str::FromStr for Token {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(text, 16).map(Token)
    }
}

/// What kind of call this was, decided from the path alone.
///
/// The distinction earns its place because the counts are what a developer
/// reads first: "this screen made one document request and eleven server
/// calls" is a diagnosis, and it is unavailable if everything is just a
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A browser navigation that produced server-rendered HTML.
    Document,
    /// A Leptos server function. `#[server]` mounts these under `/api`.
    ServerFn,
    /// The public REST API, `/api/v1/*`.
    Api,
    /// The wasm bundle, CSS, images - noise, kept so the count is honest.
    Asset,
    /// A file upload or download, which has its own limits and its own router.
    File,
    /// Health probes and anything else that matched none of the above.
    Other,
}

impl Kind {
    /// Classify from the request path.
    ///
    /// Order matters: `/api/v1` is a prefix of nothing else, but `/api/` is a
    /// prefix of it, so the public API has to be tested first or every REST
    /// call is filed as a server function.
    pub fn of(path: &str) -> Self {
        if path.starts_with("/api/v1") {
            return Self::Api;
        }
        if path.starts_with("/api/") {
            return Self::ServerFn;
        }
        if path.starts_with("/files/") || path.starts_with("/uploads/") {
            return Self::File;
        }
        if path.starts_with("/health/") {
            return Self::Other;
        }
        if path.starts_with("/pkg/")
            || path.starts_with("/app-assets/")
            || path.starts_with("/assets/")
            || is_static_file(path)
        {
            return Self::Asset;
        }

        Self::Document
    }

    /// The label the report prints.
    pub fn label(self) -> &'static str {
        match self {
            Self::Document => "page",
            Self::ServerFn => "server fn",
            Self::Api => "api",
            Self::Asset => "asset",
            Self::File => "file",
            Self::Other => "other",
        }
    }

    /// Whether this is worth showing by default.
    ///
    /// Assets are ninety per cent of the rows and none of the interest. They
    /// are collected - a profile that lied about how many requests a page made
    /// would be worse than no profile - but the index hides them behind a
    /// filter rather than making a developer scroll past them.
    pub fn is_interesting(self) -> bool {
        !matches!(self, Self::Asset)
    }
}

/// A path ending in an extension that is served, not rendered.
fn is_static_file(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };

    matches!(
        extension,
        "js" | "wasm"
            | "css"
            | "map"
            | "ico"
            | "svg"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "avif"
            | "woff"
            | "woff2"
            | "ttf"
            | "json"
            | "txt"
    )
}

/// Serialise a duration as milliseconds.
///
/// serde renders a `Duration` as `{"secs":0,"nanos":12000000}`, which every
/// consumer then has to reassemble. Milliseconds as a float is what the report
/// prints and what a toolbar would want, and it is the same unit throughout so
/// a column of them can be compared without reading units.
pub(crate) fn millis<S: Serializer>(
    duration: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(duration.as_secs_f64() * 1000.0)
}

/// The same, for a duration that may be absent.
///
/// `None` stays `null` rather than becoming zero: "not measured" and "took no
/// time" are different answers, and only one of them is a reason to look
/// closer.
fn millis_of<S: Serializer>(
    duration: &Option<Duration>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match duration {
        Some(duration) => millis(duration, serializer),
        None => serializer.serialize_none(),
    }
}

/// One SQL statement, as sqlx reported it.
#[derive(Debug, Clone, Serialize)]
pub struct Query {
    /// The statement, as close to verbatim as sqlx logged it.
    pub sql: String,
    /// The stack that ran it, down to this workspace's own files.
    ///
    /// Empty when `profiler.backtraces` is off, and empty - honestly - when
    /// the statement was run by something with no workspace frame on its
    /// stack, which is what the pool's own background work looks like.
    #[serde(skip_serializing_if = "Caller::is_empty")]
    pub caller: Caller,
    /// What sqlx measured, which is the round trip and not the parse.
    #[serde(serialize_with = "millis_of")]
    pub elapsed: Option<Duration>,
    /// Rows returned, when the event carried it.
    pub rows_returned: Option<u64>,
    /// Rows affected, when the event carried it.
    pub rows_affected: Option<u64>,
}

impl Query {
    /// The statement collapsed to its shape, for spotting the same query run
    /// eleven times.
    ///
    /// Whitespace only. Stripping literals would be the more thorough
    /// normalisation and it is not needed: sqlx logs prepared statements, so
    /// the values are already `$1` and the shape is already the identity.
    pub fn shape(&self) -> String {
        self.sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// A log line emitted while the request was in flight.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub level: String,
    pub target: String,
    pub message: String,
    /// Everything other than `message`, rendered as `key=value`.
    pub fields: Vec<(String, String)>,
    /// Where the line was written, workspace-relative.
    ///
    /// Free, unlike a query's caller: a log line's own metadata already names
    /// the file it came from, because the application is what emitted it.
    /// `None` for a line from a dependency, whose source file is not in this
    /// repository and would only be a path on somebody's disk.
    pub source: Option<String>,
    pub line: Option<u32>,
    /// The stack that reached this line.
    ///
    /// Empty when `profiler.backtraces` is off. Unlike [`Self::source`], which
    /// is one point, this is the path taken to get there - and it is the only
    /// evidence the flow diagram has for anything that does not end in a
    /// statement, which is most of what an application does.
    #[serde(skip_serializing_if = "Caller::is_empty")]
    pub caller: Caller,
}

impl LogLine {
    /// `phonix-db/src/tenancy.rs:87`, or nothing when the line came from a
    /// dependency.
    pub fn position(&self) -> Option<String> {
        Some(format!("{}:{}", self.source.as_deref()?, self.line?))
    }
}

/// Everything known about one request.
#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub token: Token,
    pub at: DateTime<Utc>,
    pub kind: Kind,

    pub method: String,
    /// The concrete path, which is what was asked for.
    pub path: String,
    pub query_string: Option<String>,
    /// The route pattern that matched, which is what the code calls it.
    ///
    /// `None` when the request did not reach a route - see
    /// [`crate::middleware`] for why that is a real case and not a bug.
    pub route: Option<String>,

    pub status: u16,
    #[serde(serialize_with = "millis")]
    pub duration: Duration,

    pub tenant: Option<String>,
    /// The page load this belongs to, from `X-Phonix-Page`. Phase two.
    pub page: Option<String>,

    /// From `Content-Length`, when the response declared one.
    ///
    /// Leptos streams its HTML, so a page has no content length and this is
    /// `None`. Measuring the streamed size means wrapping the body, which
    /// phase one does not do - a profiler that changes how the response is
    /// delivered is a profiler that changes what it is measuring.
    pub response_bytes: Option<u64>,

    pub queries: Vec<Query>,
    pub logs: Vec<LogLine>,

    /// Resident set size in bytes at the end of the request, where the
    /// platform will say.
    ///
    /// This is the *process*, not the request, and under any concurrency it is
    /// not attributable to this row. Kept as a gauge of whether the process is
    /// growing, and labelled as exactly that wherever it is drawn. See
    /// `docs/adr/0004-development-profiler.md` section 3.
    pub rss_bytes: Option<u64>,
}

impl Profile {
    /// What the route is called, falling back to the path when nothing matched.
    pub fn route_or_path(&self) -> &str {
        self.route.as_deref().unwrap_or(&self.path)
    }

    /// Total time spent inside sqlx, for the requests where that was measured.
    pub fn query_time(&self) -> Duration {
        self.queries.iter().filter_map(|query| query.elapsed).sum()
    }

    /// Statement shapes that ran more than once, most repeated first.
    ///
    /// This is the N+1 detector, and it is the whole reason the query list is
    /// worth collecting: a screen that runs one statement eleven times has a
    /// loop in it, and nobody finds that by reading eleven near-identical log
    /// lines.
    pub fn repeated_queries(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();

        for query in &self.queries {
            let shape = query.shape();

            match counts.iter_mut().find(|(seen, _)| seen == &shape) {
                Some((_, count)) => *count += 1,
                None => counts.push((shape, 1)),
            }
        }

        counts.retain(|(_, count)| *count > 1);
        counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toolbar builds `/_profiler/<token>` out of this JSON, and the
    /// route parses it as hex. A token serialised as its `u64` would look like
    /// a usable value, parse as an entirely different number, and 404.
    #[test]
    fn a_token_goes_on_the_wire_as_the_text_its_url_uses() {
        let token = Token(48_879);
        let json = serde_json::to_string(&token).expect("a token serialises");

        assert_eq!(json, "\"00000000beef\"");
        assert_eq!(json.trim_matches('"').parse(), Ok(token));
    }

    #[test]
    fn a_token_round_trips_through_its_text() {
        let token = Token(48_879);
        let parsed: Token = token.to_string().parse().expect("token parses back");

        assert_eq!(parsed, token);
    }

    /// The ordering bug this guards is silent: `/api/` is a prefix of
    /// `/api/v1/`, so testing it first files every REST call as a server
    /// function and the counts stop meaning anything.
    #[test]
    fn the_public_api_is_not_filed_as_a_server_function() {
        assert_eq!(Kind::of("/api/v1/currencies"), Kind::Api);
        assert_eq!(Kind::of("/api/currencies/save"), Kind::ServerFn);
    }

    #[test]
    fn a_page_is_a_document_and_a_bundle_is_not() {
        assert_eq!(Kind::of("/admin/users"), Kind::Document);
        assert_eq!(Kind::of("/"), Kind::Document);
        assert_eq!(Kind::of("/pkg/phonix.wasm"), Kind::Asset);
        assert_eq!(Kind::of("/app-assets/scalar.f187dfac27b9.js"), Kind::Asset);
        assert_eq!(Kind::of("/favicon.svg"), Kind::Asset);
    }

    /// A path segment containing a dot must not turn a page into an asset -
    /// a workspace slug or a filename in a route would do it.
    #[test]
    fn only_the_last_segment_decides_whether_a_path_is_a_file() {
        assert_eq!(Kind::of("/admin/v1.2/settings"), Kind::Document);
    }

    /// The JSON endpoints are phase two's input. A `Duration` serialised as
    /// serde's struct would make the toolbar reassemble every number, and
    /// changing it later would break whatever had already been written
    /// against it.
    #[test]
    fn a_duration_goes_on_the_wire_as_milliseconds() {
        let query = Query {
            sql: "SELECT 1".into(),
            caller: Caller::none(),
            elapsed: Some(Duration::from_millis(250)),
            rows_returned: None,
            rows_affected: None,
        };

        let json = serde_json::to_string(&query).expect("a query serialises");

        assert!(json.contains("\"elapsed\":250.0"), "{json}");
        assert!(!json.contains("nanos"), "{json}");
    }

    /// Absent is not zero.
    #[test]
    fn an_unmeasured_duration_is_null_and_not_zero() {
        let query = Query {
            sql: "SELECT 1".into(),
            caller: Caller::none(),
            elapsed: None,
            rows_returned: None,
            rows_affected: None,
        };

        let json = serde_json::to_string(&query).expect("a query serialises");

        assert!(json.contains("\"elapsed\":null"), "{json}");
    }

    #[test]
    fn repeated_statements_are_counted_and_singletons_are_not() {
        let profile = Profile {
            token: Token(1),
            at: Utc::now(),
            kind: Kind::Document,
            method: "GET".into(),
            path: "/admin/users".into(),
            query_string: None,
            route: None,
            status: 200,
            duration: Duration::from_millis(1),
            tenant: None,
            page: None,
            response_bytes: None,
            queries: vec![
                Query {
                    sql: "SELECT 1".into(),
                    caller: Caller::none(),
                    elapsed: None,
                    rows_returned: None,
                    rows_affected: None,
                },
                Query {
                    sql: "SELECT   1".into(),
                    caller: Caller::none(),
                    elapsed: None,
                    rows_returned: None,
                    rows_affected: None,
                },
                Query {
                    sql: "SELECT 2".into(),
                    caller: Caller::none(),
                    elapsed: None,
                    rows_returned: None,
                    rows_affected: None,
                },
            ],
            logs: Vec::new(),
            rss_bytes: None,
        };

        let repeated = profile.repeated_queries();

        assert_eq!(repeated.len(), 1, "only the duplicated shape is reported");
        assert_eq!(repeated.first().map(|(_, count)| *count), Some(2));
    }
}
