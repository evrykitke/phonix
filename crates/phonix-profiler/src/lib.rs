//! The development profiler: what each request did, while it is still on
//! screen.
//!
//! The design, and the reasoning behind every part of it, is
//! `docs/adr/0004-development-profiler.md`. The short version:
//!
//! * A `tracing` layer collects events against the request that caused them,
//!   through a task-local. Nothing in the application is instrumented, and
//!   nothing in it knows this crate exists - see [`collect`].
//! * A middleware turns each request into a [`Profile`] and stamps the
//!   response with `X-Debug-Token` - see [`middleware`].
//! * Profiles live in a bounded ring in memory - see [`store`].
//! * The report is plain server-rendered HTML on `/_profiler`, sharing nothing
//!   with the Leptos application, because a profiler that cannot survive the
//!   application panicking is a profiler that is absent exactly when it is
//!   needed.
//!
//! # This must not exist in production
//!
//! A profile holds SQL, request paths and log lines. The two gates are a cargo
//! feature on `phonix-server` and a config key that `validate::check` refuses
//! to accept outside development. Both, because the failure mode is silent.

pub mod caller;
mod collect;
pub mod diagram;
pub mod flow;
mod inject;
pub mod middleware;
pub mod page;
pub mod profile;
mod report;
mod routes;
mod rss;
pub mod source;
mod store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing_subscriber::layer::Layer;
use tracing_subscriber::{EnvFilter, Registry};

pub use caller::{Caller, Frame};
pub use collect::EventLayer;
pub use flow::{Flow, PageFlow};
pub use middleware::{DEBUG_TOKEN, PAGE_HEADER};
pub use page::{PageEntry, PageSummary};
pub use profile::{Kind, LogLine, Profile, Query, Token};
pub use store::{DEFAULT_CAPACITY, Store};

/// What the profiler's own filter says when nothing is configured.
///
/// `sqlx::query=debug` is the load-bearing half. sqlx logs a statement at
/// DEBUG, and the default console filter does not let DEBUG through - so
/// without this the query panel would be present, correct, and always empty,
/// which is the worst of the three.
///
/// The profiler's filter is its own rather than the console's, deliberately:
/// turning the profiler on should not fill a developer's terminal with every
/// statement the application runs.
pub const DEFAULT_FILTER: &str = "info,sqlx::query=debug";

/// A handle to the profiler: the store, and the pieces that write to it.
///
/// Cheap to clone - it is one `Arc` - because axum wants it as middleware
/// state and as router state, and the tracing layer is built from it before
/// either exists.
#[derive(Debug, Clone)]
pub struct Profiler {
    store: Arc<Store>,
    /// The directory holding `crates/`, when the caller knows it.
    ///
    /// Only the source view needs this, and it is `None` unless something set
    /// it - a profiler that has not been told where the workspace is simply
    /// does not offer to show source, rather than guessing from
    /// `current_exe()` and reading whatever it finds.
    source_root: Option<Arc<PathBuf>>,
}

impl Profiler {
    /// A profiler keeping at most `capacity` profiles.
    pub fn new(capacity: usize) -> Self {
        Self {
            store: Arc::new(Store::new(capacity)),
            source_root: None,
        }
    }

    /// Where this checkout lives, so the report can show the files a stack
    /// named.
    ///
    /// Passed in rather than discovered here: this crate depends on nothing
    /// and knows nothing about the workspace, and `phonix-config` already owns
    /// the question of where the root is. See [`source`] for what is done with
    /// it, which is deliberately very little.
    #[must_use]
    pub fn with_source_root(mut self, root: PathBuf) -> Self {
        self.source_root = Some(Arc::new(root));
        self
    }

    pub(crate) fn source_root(&self) -> Option<&Path> {
        self.source_root.as_deref().map(PathBuf::as_path)
    }

    /// The ring of profiles.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The layer to add to the `Vec` that `phonix-telemetry` gives the
    /// registry.
    ///
    /// It carries its own `EnvFilter`, so what the profiler records is
    /// independent of what the console and the file record. `directives` is
    /// usually [`DEFAULT_FILTER`].
    ///
    /// # Errors
    ///
    /// If `directives` is not a valid filter. The caller should treat that as
    /// fatal for the same reason the other filters do: a process that silently
    /// ignores what it was told to log is worse than one that will not start.
    pub fn tracing_layer(
        &self,
        directives: &str,
        backtraces: bool,
    ) -> Result<Box<dyn Layer<Registry> + Send + Sync>, tracing_subscriber::filter::ParseError>
    {
        let filter = EnvFilter::try_new(directives)?;

        Ok(Box::new(EventLayer { backtraces }.with_filter(filter)))
    }

    /// The report, mounted at `/_profiler`.
    ///
    /// Generic in the surrounding router's state so it can be merged wherever
    /// it needs to be, which is *before* tenant resolution: the profiler has
    /// to answer when resolving a tenant is the thing that is broken.
    pub fn routes<S>(&self) -> axum::Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        routes::router().with_state(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_filter_is_valid_and_lets_statements_through() {
        let profiler = Profiler::new(4);

        assert!(profiler.tracing_layer(DEFAULT_FILTER, true).is_ok());
        assert!(
            DEFAULT_FILTER.contains("sqlx::query=debug"),
            "without this the query panel is always empty"
        );
    }

    #[test]
    fn a_nonsense_filter_is_refused_rather_than_ignored() {
        let profiler = Profiler::new(4);

        assert!(
            profiler
                .tracing_layer("=== not a filter ===", true)
                .is_err()
        );
    }
}
