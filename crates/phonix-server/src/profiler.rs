//! Whether this binary has a profiler, and where it attaches.
//!
//! Everything `cfg`-dependent about the profiler is in this file. The rest of
//! the server calls the same four methods either way and never says
//! `#[cfg(feature = "profiler")]`, which is the point: a gate spread across a
//! startup function is a gate somebody eventually gets half-right.
//!
//! See `docs/adr/0004-development-profiler.md`, section 8 for the two gates and
//! section 10 for the placement.

use axum::Router;
use phonix_config::AppConfig;
use phonix_telemetry::ExtraLayer;

/// The profiler, if this build has one and configuration asked for it.
///
/// Without the `profiler` feature this is an empty struct and every method
/// below is a no-op the optimiser removes.
#[derive(Debug, Clone, Default)]
pub struct Profiling {
    #[cfg(feature = "profiler")]
    handle: Option<phonix_profiler::Profiler>,
}

impl Profiling {
    /// Build the profiler and the tracing layer it needs.
    ///
    /// Called before `phonix_telemetry::init`, because the layer has to be in
    /// the registry from the first event - a profiler that starts working on
    /// the second request is one that never explains the first.
    ///
    /// # Errors
    ///
    /// If `profiler.filter` is not a valid tracing filter. Fatal, like the
    /// other filters: a process that silently ignores what it was told to
    /// record is worse than one that will not start.
    #[cfg(feature = "profiler")]
    pub fn start(config: &AppConfig) -> Result<(Self, Vec<ExtraLayer>), String> {
        if !config.profiler.enabled {
            return Ok((Self::default(), Vec::new()));
        }

        // The source root is handed over rather than discovered: the profiler
        // crate depends on nothing and has no business deciding where this
        // checkout is, and `phonix-config` already owns that question. Without
        // it the flow diagram simply does not offer to show a file.
        let profiler = phonix_profiler::Profiler::new(config.profiler.capacity)
            .with_source_root(phonix_config::workspace_root());
        let layer = profiler
            .tracing_layer(&config.profiler.filter, config.profiler.backtraces)
            .map_err(|err| format!("profiler.filter is not a valid tracing filter: {err}"))?;

        Ok((
            Self {
                handle: Some(profiler),
            },
            vec![layer],
        ))
    }

    /// The same, for a build with no profiler in it.
    ///
    /// This is the deployed shape: `phonix-deploy` builds with
    /// `--bin-features ssr`, which drops the `profiler` that the workspace
    /// manifest otherwise asks for. A development build takes the arm above.
    ///
    /// A configuration asking for a profiler is still reported rather than
    /// ignored. The developer who set `enabled = true` and got nothing would
    /// otherwise spend the afternoon debugging the profiler instead of reading
    /// a build flag.
    #[cfg(not(feature = "profiler"))]
    pub fn start(config: &AppConfig) -> Result<(Self, Vec<ExtraLayer>), String> {
        if config.profiler.enabled {
            eprintln!(
                "phonix-server: profiler.enabled is true, but this binary was built \
                 without the profiler feature, so there is no profiler. That is \
                 what a release built by phonix-deploy is; an ordinary \
                 cargo leptos watch has one. Set profiler.enabled = false to \
                 silence this."
            );
        }

        Ok((Self::default(), Vec::new()))
    }

    /// Whether anything is being collected, for the line the server logs at
    /// startup.
    pub fn is_on(&self) -> bool {
        #[cfg(feature = "profiler")]
        {
            self.handle.is_some()
        }

        #[cfg(not(feature = "profiler"))]
        {
            false
        }
    }

    /// Record which route each request matched.
    ///
    /// `route_layer`, not `layer`, and that is the entire reason this is a
    /// separate call: the route comes from `MatchedPath`, which axum inserts
    /// *during* routing, so nothing wrapping the router can read it.
    ///
    /// This only annotates. The profile itself is opened and filed by
    /// [`Self::instrument`], which has to sit further out to see the tenant -
    /// see `phonix_profiler::middleware` for the requirement that splits them.
    ///
    /// Call this after the last route that should carry a route pattern.
    pub fn mark_routes<S>(&self, router: Router<S>) -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        #[cfg(feature = "profiler")]
        match self.handle.as_ref() {
            Some(_) => router.route_layer(axum::middleware::from_fn(
                phonix_profiler::middleware::route,
            )),
            None => router,
        }

        #[cfg(not(feature = "profiler"))]
        router
    }

    /// Profile every request that reaches `router`.
    ///
    /// Where this goes is load-bearing. It must be **inside** `TraceLayer`, so
    /// it runs within the `http` span whose `tenant` field it reads, and
    /// **outside** `resolve_tenant`, so that the tenant is recorded and the
    /// query that resolved it is run while the collector is open. Attached
    /// below `resolve_tenant` instead - which is where it started life - every
    /// profile reports no tenant and omits the tenant lookup from its query
    /// list.
    ///
    /// Layers apply bottom-up, so "inside `TraceLayer`" means: call this after
    /// `.layer(resolve_tenant)` and before `.layer(TraceLayer)`.
    pub fn instrument<S>(&self, router: Router<S>) -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        #[cfg(feature = "profiler")]
        match self.handle.clone() {
            Some(profiler) => router.layer(axum::middleware::from_fn_with_state(
                profiler,
                phonix_profiler::middleware::collect,
            )),
            None => router,
        }

        #[cfg(not(feature = "profiler"))]
        router
    }

    /// Add `/_profiler` to `router`.
    ///
    /// Call this on a router that already carries the application's outer
    /// layers, so the report sits outside them: it must keep answering when
    /// resolving a tenant is the thing that has broken, it must not be
    /// counted against a rate limit meant for the application, and it must
    /// not log an `http` span into the log it is displaying.
    ///
    /// Call it after [`Self::instrument`] too - otherwise the profiler
    /// profiles its own report.
    pub fn mount<S>(&self, router: Router<S>) -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        #[cfg(feature = "profiler")]
        match self.handle.as_ref() {
            Some(profiler) => router.merge(profiler.routes::<S>()),
            None => router,
        }

        #[cfg(not(feature = "profiler"))]
        router
    }
}

#[cfg(test)]
mod tests {

    /// The profiler finds the request span by name, so renaming the span in
    /// `middleware::make_request_span` would silently drop the tenant from
    /// every profile. This is the test that fails instead.
    #[test]
    fn the_request_span_is_named_what_the_profiler_looks_for() {
        let request = axum::http::Request::builder()
            .uri("/admin/users")
            .body(axum::body::Body::empty())
            .expect("a request with no body builds");

        let span = crate::middleware::make_request_span(&request);

        assert_eq!(
            span.metadata().map(|metadata| metadata.name()),
            Some("http"),
            "phonix_profiler::collect matches this name to find the tenant"
        );
    }

    /// A build without the feature must not collect, whatever the config says.
    #[cfg(not(feature = "profiler"))]
    #[test]
    fn a_build_without_the_feature_has_no_profiler() {
        assert!(!super::Profiling::default().is_on());
    }

    /// What `/_profiler` answers, without a server.
    ///
    /// These are here rather than in `tests/` so they can build a [`Profiling`]
    /// with its field set directly. That is the whole point: the alternative is
    /// a `cargo leptos watch`, a wasm build, a link and a boot, to learn a
    /// status code.
    #[cfg(feature = "profiler")]
    mod mounting {
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt as _;

        use crate::profiler::Profiling;

        async fn status_of(profiling: &Profiling, path: &str) -> StatusCode {
            let router = profiling.mount(Router::new());
            let request = Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("a request with no body builds");

            router
                .oneshot(request)
                .await
                .expect("a router is infallible")
                .status()
        }

        #[tokio::test]
        async fn a_running_profiler_serves_its_report() {
            let profiling = Profiling {
                handle: Some(phonix_profiler::Profiler::new(8)),
            };

            assert!(profiling.is_on());
            assert_eq!(status_of(&profiling, "/_profiler").await, StatusCode::OK);
            assert_eq!(
                status_of(&profiling, "/_profiler/toolbar.js").await,
                StatusCode::OK
            );
        }

        /// The half that `profiler.enabled = false` is supposed to buy. Not
        /// "the page is empty" - the routes are not there at all.
        /// The source view is reachable, and refuses by default.
        ///
        /// Nothing has been recorded against this page, so the allowlist is
        /// empty and every file is refused - which is the behaviour that
        /// matters, because the alternative is an unauthenticated file read.
        #[tokio::test]
        async fn the_source_view_refuses_a_file_no_profile_recorded() {
            let profiling = Profiling {
                handle: Some(phonix_profiler::Profiler::new(8)),
            };

            assert_eq!(
                status_of(
                    &profiling,
                    "/_profiler/source/page/nothing?file=phonix-db/src/lib.rs&line=1"
                )
                .await,
                StatusCode::NOT_FOUND
            );
        }

        #[tokio::test]
        async fn a_profiler_that_is_off_mounts_nothing() {
            let profiling = Profiling::default();

            assert!(!profiling.is_on());
            assert_eq!(
                status_of(&profiling, "/_profiler").await,
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                status_of(&profiling, "/_profiler/toolbar.js").await,
                StatusCode::NOT_FOUND
            );
        }
    }

    /// `profiler.enabled` is the switch, and this is the line that reads it.
    ///
    /// Loading the repository's own `config/` rather than a fixture, because a
    /// fixture that drifts from the real files proves nothing. The `enabled`
    /// flag is then set both ways on the loaded struct, so this does not care
    /// what `development.toml` currently says.
    ///
    /// If this fails to load at all, check `PHONIX_ENV`: set to `production`
    /// it pulls in `production.toml`, whose secrets come from the environment
    /// and are not present in a test run.
    #[cfg(feature = "profiler")]
    #[test]
    fn the_config_key_decides_whether_anything_collects() {
        let mut config =
            phonix_config::load_from("../../config").expect("the repository's config loads");

        config.profiler.enabled = false;
        let (off, layers) = super::Profiling::start(&config).expect("an off profiler starts");
        assert!(!off.is_on());
        assert!(
            layers.is_empty(),
            "an off profiler must not add a tracing layer either"
        );

        config.profiler.enabled = true;
        let (on, layers) = super::Profiling::start(&config).expect("an on profiler starts");
        assert!(on.is_on());
        assert_eq!(layers.len(), 1);
    }
}
