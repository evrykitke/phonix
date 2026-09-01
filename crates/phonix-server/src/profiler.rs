//! Whether this binary has a profiler, and where it attaches.
//!
//! Everything `cfg`-dependent about the profiler is in this file. The rest of
//! the server calls the same four methods either way and never says
//! `#[cfg(feature = "profiler")]`, which is the point: a gate spread across a
//! startup function is a gate somebody eventually gets half-right.
//!
//! See `docs/adr/0004-development-profiler.md`, section 8 for the two gates and
//! section 9 for the placement.

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

        let profiler = phonix_profiler::Profiler::new(config.profiler.capacity);
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
}
