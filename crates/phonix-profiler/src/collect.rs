//! How events reach the request that caused them.
//!
//! The mechanism is a task-local: the middleware wraps the whole downstream
//! future in [`CURRENT`], and a `tracing` layer appends every event it sees to
//! whatever is in there. Nothing else is instrumented, and nothing has to know
//! the profiler exists.
//!
//! # Why a task-local and not the span registry
//!
//! The registry can be asked which span an event is inside, and a collector
//! could be hung off that span's extensions. That is the more thorough
//! mechanism and it costs a subscriber downcast through `WithContext` to reach
//! it from the middleware. A task-local needs none of that and gets the same
//! answer for the case that matters, because a request is one task.
//!
//! What it gives up, stated so it is not rediscovered as a bug: **work that
//! leaves the request's task is not collected.** A `tokio::spawn` inside a
//! handler, and the connection pool's own background work, emit their events
//! outside this scope. That is the right answer for a request profiler - those
//! events did not belong to the request - but it does mean the query list is
//! what this request ran, not everything the process ran while it waited.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use crate::caller::Caller;
use crate::profile::{LogLine, Query};

/// The most of each kind that one request will hold.
///
/// A loop that runs ten thousand statements is a bug the profiler should
/// report, not join in with: without a cap, profiling it costs more memory
/// than running it did.
const MAX_PER_REQUEST: usize = 500;

tokio::task_local! {
    /// The collector for the request running on this task, if it is profiled.
    pub(crate) static CURRENT: Arc<Mutex<Collected>>;
}

/// What was gathered while one request was in flight.
#[derive(Debug, Default)]
pub(crate) struct Collected {
    pub queries: Vec<Query>,
    pub logs: Vec<LogLine>,
    /// Read off the request span rather than passed in - see [`REQUEST_SPAN`].
    pub tenant: Option<String>,
    /// The route pattern, written in by the inner middleware once axum has
    /// decided what matched - see [`crate::middleware::route`].
    pub route: Option<String>,
    /// Set when a cap was hit, so the report can say so rather than quietly
    /// showing the first five hundred of something.
    pub truncated: bool,
}

impl Collected {
    fn push_query(&mut self, query: Query) {
        if self.queries.len() >= MAX_PER_REQUEST {
            self.truncated = true;
            return;
        }

        self.queries.push(query);
    }

    fn push_log(&mut self, line: LogLine) {
        if self.logs.len() >= MAX_PER_REQUEST {
            self.truncated = true;
            return;
        }

        self.logs.push(line);
    }
}

/// Run `future` with a fresh collector, and hand back what it gathered.
pub(crate) async fn scoped<F, T>(future: F) -> (T, Collected)
where
    F: std::future::Future<Output = T>,
{
    let collector = Arc::new(Mutex::new(Collected::default()));
    let output = CURRENT.scope(Arc::clone(&collector), future).await;

    // The Arc is not necessarily unique - a spawned task could still hold a
    // clone - so the contents are taken out rather than the Arc unwrapped.
    let collected = collector
        .lock()
        .map(|mut held| std::mem::take(&mut *held))
        .unwrap_or_default();

    (output, collected)
}

/// The `tracing` layer that feeds [`CURRENT`].
///
/// Everything it produces belongs to the task it was called on. Added to the
/// `Vec` that `phonix-telemetry` hands to the registry, with its own filter -
/// see [`crate::Profiler::tracing_layer`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EventLayer {
    /// Whether to walk the stack behind each SQL statement.
    ///
    /// The one thing here that costs anything per event rather than per
    /// request, so it is the one thing that can be turned off. See
    /// [`crate::caller`].
    pub backtraces: bool,
}

impl<S> Layer<S> for EventLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    /// Take the tenant from the request span the moment it opens.
    ///
    /// It is `Empty` at this point on every real request - `resolve_tenant`
    /// fills it in later, which arrives at [`Self::on_record`] - but a span
    /// created with the field already set would be missed otherwise.
    fn on_new_span(&self, attributes: &Attributes<'_>, _id: &Id, _context: Context<'_, S>) {
        if attributes.metadata().name() != REQUEST_SPAN {
            return;
        }

        let mut fields = Fields::default();
        attributes.record(&mut fields);
        record_tenant(fields);
    }

    /// `resolve_tenant` records the tenant on the surrounding span so every
    /// log line is attributable to one. This reads the same value, which is
    /// why the profiler needs no cooperation from the tenant middleware and no
    /// dependency on the type it resolves.
    fn on_record(&self, id: &Id, values: &Record<'_>, context: Context<'_, S>) {
        let is_request_span = context
            .span(id)
            .is_some_and(|span| span.metadata().name() == REQUEST_SPAN);

        if !is_request_span {
            return;
        }

        let mut fields = Fields::default();
        values.record(&mut fields);
        record_tenant(fields);
    }

    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        // Not profiled: no request on this task, which is the common case for
        // a background job and for everything logged before the first request.
        let Ok(collector) = CURRENT.try_with(Arc::clone) else {
            return;
        };

        let metadata = event.metadata();

        // Walked here, before the lock, because this is the only moment the
        // stack that ran the statement is still on the stack. A frame later -
        // in `push_query`, or anywhere the profile is assembled - and it is
        // gone. Nothing is resolved yet; see `crate::caller`.
        let caller = if self.backtraces && metadata.target().starts_with("sqlx::query") {
            Caller::capture()
        } else {
            Caller::none()
        };

        let mut fields = Fields::default();
        event.record(&mut fields);

        let Ok(mut collected) = collector.lock() else {
            // Poisoned by a panic in another holder. The request is already
            // going to be reported as a 500; losing its log lines is not worth
            // a second panic here.
            return;
        };

        if metadata.target().starts_with("sqlx::query") {
            collected.push_query(fields.into_query(caller));
        } else {
            collected.push_log(fields.into_log(
                metadata.level().as_str(),
                metadata.target(),
                (metadata.file(), metadata.line()),
            ));
        }
    }
}

/// The name of the span `make_request_span` opens around every request.
///
/// A string match on a span name is a weak seam, so it is tested: see
/// `phonix-server`'s `the_request_span_is_named_what_the_profiler_looks_for`.
const REQUEST_SPAN: &str = "http";

/// Keep the tenant from a request span's fields, if it named one.
///
/// Last write wins, because `resolve_tenant` records the real value after the
/// span opened with `Empty`. An empty string is not a value: that is what an
/// unrecorded field renders as.
fn record_tenant(mut fields: Fields) {
    let Some(tenant) = fields.take("tenant").filter(|value| !value.is_empty()) else {
        return;
    };

    let Ok(collector) = CURRENT.try_with(Arc::clone) else {
        return;
    };

    if let Ok(mut collected) = collector.lock() {
        collected.tenant = Some(tenant);
    }
}

/// Note the route this request matched, if a collector is in scope.
///
/// Called from the inner middleware rather than passed into the outer one,
/// because the two run in different places: the collector has to be
/// established outside tenant resolution to see it, and `MatchedPath` only
/// exists inside routing. See [`crate::middleware`].
pub(crate) fn record_route(route: String) {
    let Ok(collector) = CURRENT.try_with(Arc::clone) else {
        return;
    };

    if let Ok(mut collected) = collector.lock() {
        collected.route = Some(route);
    }
}

/// Every field on an event, flattened to strings.
///
/// Strings because that is what the report renders, and because the field set
/// is not ours to know: sqlx names four of them and every other crate in the
/// workspace names its own.
#[derive(Debug, Default)]
struct Fields {
    message: Option<String>,
    named: Vec<(String, String)>,
}

impl Fields {
    fn take(&mut self, name: &str) -> Option<String> {
        let index = self.named.iter().position(|(key, _)| key == name)?;

        Some(self.named.remove(index).1)
    }

    /// Build a query from sqlx's field names.
    ///
    /// `db.statement` is the full SQL; `summary` is sqlx's own one-line
    /// version, used only when the full one is absent. `elapsed_secs` is
    /// preferred over `elapsed` because it is a number rather than a
    /// `Duration`'s `Debug`, which would have to be parsed back out of
    /// "1.234ms".
    fn into_query(mut self, caller: Caller) -> Query {
        // Each candidate has to be non-empty, not merely present. sqlx sends
        // `db.statement: ""` alongside a perfectly good `summary` for a query
        // it did not prepare - the readiness probe's `SELECT 1` is one - and
        // taking the empty one leaves a blank row in the query panel and an
        // empty shape in the N+1 count, where it looks like a statement run
        // three times.
        let sql = [
            self.take("db.statement"),
            self.take("summary"),
            self.message.take(),
        ]
        .into_iter()
        .flatten()
        .find(|candidate| !candidate.trim().is_empty())
        .unwrap_or_else(|| "<no statement recorded>".to_owned());

        let elapsed = self
            .take("elapsed_secs")
            .and_then(|secs| secs.parse::<f64>().ok())
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
            .map(Duration::from_secs_f64);

        Query {
            sql: sql.trim().to_owned(),
            caller,
            elapsed,
            rows_returned: self.take("rows_returned").and_then(|n| n.parse().ok()),
            rows_affected: self.take("rows_affected").and_then(|n| n.parse().ok()),
        }
    }

    /// Build a log line, keeping where in the workspace it was written.
    ///
    /// `at` is the event's own `file` and `line`. Unlike a query's caller this
    /// costs nothing to know: the application is what emitted the line, so its
    /// metadata already names the right file. A dependency's path is dropped
    /// rather than shown - it is a path on whoever built it, and
    /// `crate::caller` makes the same cut for the same reason.
    fn into_log(self, level: &str, target: &str, at: (Option<&str>, Option<u32>)) -> LogLine {
        let source = at.0.and_then(crate::caller::workspace_relative);

        LogLine {
            level: level.to_owned(),
            target: target.to_owned(),
            message: self.message.unwrap_or_default(),
            fields: self.named,
            line: source.as_ref().and(at.1),
            source,
        }
    }

    fn record(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = Some(value);
            return;
        }

        self.named.push((field.name().to_owned(), value));
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record(field, format!("{value:?}"));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record(field, value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record(field, value.to_string());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record(field, value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sqlx records the statement under `db.statement` and the timing as a
    /// float. Both names are sqlx's, so this test is what notices an upgrade
    /// renaming one - the symptom otherwise is a query panel that is present,
    /// populated, and says `<no statement recorded>`.
    #[test]
    fn a_sqlx_event_becomes_a_query() {
        let mut fields = Fields::default();
        fields.named.push((
            "db.statement".to_owned(),
            "SELECT id FROM core.currency".to_owned(),
        ));
        fields.named.push(("elapsed_secs".to_owned(), "0.25".to_owned()));
        fields.named.push(("rows_returned".to_owned(), "3".to_owned()));

        let query = fields.into_query(Caller::none());

        assert_eq!(query.sql, "SELECT id FROM core.currency");
        assert_eq!(query.elapsed, Some(Duration::from_millis(250)));
        assert_eq!(query.rows_returned, Some(3));
    }

    #[test]
    fn a_query_without_a_statement_says_so_rather_than_being_empty() {
        let query = Fields::default().into_query(Caller::none());

        assert!(query.sql.contains("no statement"));
    }

    /// A negative or non-finite `elapsed_secs` would panic
    /// `Duration::from_secs_f64`, and a profiler that panics on a malformed
    /// log line takes the request with it.
    #[test]
    fn a_nonsense_elapsed_is_dropped_rather_than_converted() {
        for value in ["-1", "NaN", "inf", "not a number"] {
            let mut fields = Fields::default();
            fields
                .named
                .push(("elapsed_secs".to_owned(), value.to_owned()));

            assert_eq!(fields.into_query(Caller::none()).elapsed, None, "{value} must not convert");
        }
    }

    #[test]
    fn the_message_field_is_the_message_and_not_a_field() {
        let fields = Fields {
            message: Some("request carries no tenant".to_owned()),
            named: vec![("tenant".to_owned(), "acme".to_owned())],
        };

        let line = fields.into_log(
            "INFO",
            "phonix_server::middleware",
            (Some("D:/p/crates/phonix-server/src/middleware.rs"), Some(27)),
        );

        assert_eq!(line.message, "request carries no tenant");
        assert_eq!(line.fields.len(), 1);
    }

    #[test]
    fn a_request_stops_collecting_once_it_is_absurd() {
        let mut collected = Collected::default();

        for _ in 0..(MAX_PER_REQUEST + 10) {
            collected.push_query(Query {
                sql: "SELECT 1".to_owned(),
                caller: Caller::none(),
                elapsed: None,
                rows_returned: None,
                rows_affected: None,
            });
        }

        assert_eq!(collected.queries.len(), MAX_PER_REQUEST);
        assert!(collected.truncated, "the report has to be able to say so");
    }

    /// Nothing outside a profiled request may be collected, and asking must
    /// not panic - `try_with` off a task with no scope is the normal case for
    /// every background job in the process.
    #[tokio::test]
    async fn events_outside_a_request_are_dropped() {
        assert!(CURRENT.try_with(Arc::clone).is_err());
    }

    #[tokio::test]
    async fn a_scope_hands_back_what_was_gathered_inside_it() {
        let (output, collected) = scoped(async {
            if let Ok(collector) = CURRENT.try_with(Arc::clone)
                && let Ok(mut held) = collector.lock()
            {
                held.push_log(LogLine {
                    level: "INFO".to_owned(),
                    target: "test".to_owned(),
                    message: "inside".to_owned(),
                    fields: Vec::new(),
                    source: None,
                    line: None,
                });
            }

            7
        })
        .await;

        assert_eq!(output, 7);
        assert_eq!(collected.logs.len(), 1);
    }

    /// sqlx reports `db.statement: ""` with a usable `summary` for a statement
    /// it did not prepare. Preferring the empty one puts a blank row in the
    /// query panel, and - because every blank row has the same shape - reports
    /// them to each other as a repeated statement.
    #[test]
    fn an_empty_statement_falls_through_to_the_summary() {
        let fields = Fields {
            message: None,
            named: vec![
                ("db.statement".to_owned(), String::new()),
                ("summary".to_owned(), "SELECT 1".to_owned()),
            ],
        };

        assert_eq!(fields.into_query(Caller::none()).sql, "SELECT 1");
    }

    #[test]
    fn a_statement_is_preferred_over_the_summary_when_it_has_one() {
        let fields = Fields {
            message: None,
            named: vec![
                ("db.statement".to_owned(), "SELECT a, b FROM t".to_owned()),
                ("summary".to_owned(), "SELECT a, …".to_owned()),
            ],
        };

        assert_eq!(fields.into_query(Caller::none()).sql, "SELECT a, b FROM t");
    }

    /// The route arrives from a middleware running further in than the one
    /// that opened the collector. Both are on the same task, and that is the
    /// only reason this works.
    #[tokio::test]
    async fn the_inner_middleware_can_write_the_route_into_an_open_collector() {
        let (_, collected) = scoped(async {
            record_route("/admin/users/{id}".to_owned());
        })
        .await;

        assert_eq!(collected.route.as_deref(), Some("/admin/users/{id}"));
    }

    /// `route_layer` runs on requests the outer layer is not watching - the
    /// profiler's own report is mounted outside it. Writing a route with no
    /// collector open has to be a no-op rather than a panic.
    #[tokio::test]
    async fn writing_a_route_with_no_collector_open_is_a_no_op() {
        record_route("/_profiler".to_owned());
    }
}
