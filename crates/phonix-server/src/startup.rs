//! Wiring: dependencies, router, middleware, shutdown.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use leptos::prelude::*;
use leptos_axum::{LeptosRoutes, generate_route_list};
use phonix_cache::Cache;
use phonix_config::AppConfig;
use phonix_db::{Catalog, TenantRegistry};
use phonix_messaging::Messaging;
use phonix_web::state::AppState;
use phonix_web::{App, shell};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::{api, auth, files, google, health, jobs, middleware, rate_limit};

/// Build everything and serve until shutdown.
pub async fn run(config: AppConfig) -> Result<()> {
    let config = Arc::new(config);

    tracing::info!(
        name = %config.app.name,
        environment = %config.app.environment,
        "starting phonix-server"
    );

    // --- PostgreSQL --------------------------------------------------------
    let catalog_pool = phonix_db::catalog_pool(&config.database)
        .await
        .context("could not connect to the catalog database")?;
    let catalog = Catalog::new(catalog_pool);

    if config.database.migrate_on_start {
        catalog
            .migrate()
            .await
            .context("catalog migrations failed")?;

        // And every tenant database behind the current schema. A migration that
        // reached only workspaces created after it was written is a migration
        // that has not been applied.
        let sweep = phonix_db::tenancy::migrate_outdated_tenants(&catalog, &config.database)
            .await
            .context("could not read the tenant catalog to migrate it")?;

        if sweep.migrated > 0 || !sweep.failed.is_empty() {
            tracing::info!(
                migrated = sweep.migrated,
                already_current = sweep.current,
                failed = ?sweep.failed,
                "tenant schema sweep complete"
            );
        }
    }

    let tenants = TenantRegistry::new(catalog.clone(), Arc::new(config.database.clone()));

    // --- Redis -------------------------------------------------------------
    let cache = Cache::connect(&config.redis)
        .await
        .context("could not connect to redis")?;

    // --- RabbitMQ ----------------------------------------------------------
    // Messaging is optional: with `rabbitmq.enabled = false` the app still
    // serves pages, it just cannot publish events.
    let (messaging, publisher) = if config.rabbitmq.enabled {
        let messaging = Messaging::connect(Arc::new(config.rabbitmq.clone()))
            .await
            .context("could not connect to rabbitmq")?;
        let publisher = messaging
            .publisher()
            .await
            .context("could not open a rabbitmq publisher channel")?;
        (Some(messaging), Some(publisher))
    } else {
        tracing::warn!("rabbitmq is disabled; events will not be published");
        (None, None)
    };

    // --- File storage ------------------------------------------------------
    // Built before anything can serve, so a storage root that cannot be created
    // - a bad path, a read-only mount - kills the process now rather than one
    // person's upload later.
    let (storage, naming) = build_storage(&config).await?;
    tracing::info!(
        backend = %storage.describe(),
        naming = naming.describe(),
        "file storage ready"
    );

    // --- Leptos ------------------------------------------------------------
    // Reads Cargo.toml's [[workspace.metadata.leptos]] via the LEPTOS_* env
    // vars that cargo-leptos sets.
    let mut leptos_options = get_configuration(None)
        .context("could not read the leptos configuration")?
        .leptos_options;

    // `hash-files = true` in Cargo.toml makes the build emit
    // `phonix.<hash>.js`; `LEPTOS_HASH_FILES` is what makes the *server* ask
    // for that name. They are two halves of one decision, and the env var
    // defaults to false, so a deployment that forgets it serves markup
    // pointing at files the build no longer produces - a 404 for the bundle
    // and a page that renders and then does nothing at all.
    //
    // The build always hashes, so the server always looks for a hash. Leptos
    // reads the manifest from beside the executable, which is where
    // cargo-leptos writes it, so nothing needs configuring per environment.
    leptos_options.hash_files = true;

    let site_addr = leptos_options.site_addr;

    // --- translations ------------------------------------------------------
    // Read once, here, and never again: a catalog that reloaded under a running
    // render could hand two halves of one page two different versions of the
    // same sentence. English is compiled into the binary, so a deployment with
    // no locales directory at all is an ordinary, complete deployment.
    phonix_web::i18n::install(std::path::Path::new(&config.app.locales_dir));

    // --- credentials -------------------------------------------------------
    // Built here, once. A bad Argon2 parameter or a malformed MFA key kills the
    // process now rather than one user's sign-in later.
    let hasher = Arc::new(
        phonix_services::Hasher::new(&config.security.password)
            .context("invalid Argon2 parameters in [security.password]")?,
    );
    let vault = Arc::new(
        phonix_services::SecretVault::from_config(&config.security.mfa)
            .context("invalid [security.mfa] encryption_key")?,
    );

    let state = AppState {
        config: Arc::clone(&config),
        catalog,
        tenants: tenants.clone(),
        cache: cache.clone(),
        publisher,
        hasher,
        vault,
        storage,
        naming,
        leptos_options: leptos_options.clone(),
    };

    let routes = generate_route_list(App);

    // The counters that keep an anonymous caller from spending this server's
    // Argon2 budget, its mail relay, or its disk. Built here so there is
    // exactly one set of them for the life of the process - a limiter
    // constructed per request would count each request against an empty map.
    let throttle = rate_limit::RateLimitState {
        config: Arc::clone(&config),
        limiter: Arc::new(rate_limit::Limiter::new()),
    };

    // --- Router ------------------------------------------------------------
    let app = Router::new()
        // Health endpoints are registered before the tenant middleware so an
        // orchestrator can probe them without a resolvable tenant host.
        .route("/health/live", get(health::liveness))
        .route("/health/ready", get(health::readiness))
        // Reached by a plain browser navigation from the bare domain, carrying
        // a single-use token. Registered before the Leptos routes because it
        // answers with a redirect and a cookie, not with a page.
        .route("/auth/handoff", get(auth::handoff))
        // The Google flow, both halves on this one host - Google will not
        // redirect to a wildcard, so a workspace subdomain can never be the
        // registered URI. See `google` for what follows from that.
        .route("/auth/google/start", get(google::start))
        .route("/auth/google/callback", get(google::callback))
        .leptos_routes_with_context(
            &state,
            routes,
            {
                // Makes AppState reachable from server functions via
                // `use_context::<AppState>()`.
                let state = state.clone();
                move || provide_context(state.clone())
            },
            {
                let options = leptos_options.clone();
                move || shell(options.clone())
            },
        )
        // The state type is named explicitly: `AppState` and `LeptosOptions`
        // both satisfy `FromRef<AppState>`, so inference cannot pick one.
        // `_with_context` so a 404 rendered here still has AppState available -
        // the shared layout runs a server function to show the tenant.
        .fallback(leptos_axum::file_and_error_handler_with_context::<
            AppState,
            _,
        >(
            {
                let state = state.clone();
                move || provide_context(state.clone())
            },
            shell,
        ))
        // 504 rather than tower-http's default 408: the client's request was
        // fine, it was this server that took too long.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(config.server.request_timeout_secs),
        ))
        .layer(RequestBodyLimitLayer::new(config.server.body_limit_bytes))
        // Merged *after* those two layers, and that placement is the whole
        // point: `Router::layer` wraps the routes registered before it, so the
        // file routes do not inherit either.
        //
        // Neither would fit them. The body limit is 2 MiB - right for a form
        // post, and smaller than every bucket, so it would refuse every upload
        // that mattered. The timeout is 30 seconds, which 25 MB does not arrive
        // inside on any connection a person is likely to have. `files::routes`
        // carries its own of both, sized from `[storage]`.
        .merge(files::routes(&state))
        // The public API. `nest`, not `merge`, and that is load-bearing: a
        // nested router keeps its own fallback, so an unknown `/api/v1/...`
        // path answers a problem document instead of the Leptos error page
        // that this router falls back to - which no API client can parse.
        //
        // Above `with_state` for the same reason the file routes are, and
        // below the tenant middleware, so a call resolves its workspace from
        // the host exactly as a page does.
        .nest("/api/v1", api::routes())
        .with_state(state.clone())
        // Layers below here apply to everything, files included. They apply
        // bottom-up: the tenant is resolved first, then tracing wraps it, so
        // every log line for a request already carries its tenant - and an
        // upload handler can read the tenant extension exactly like a page.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::resolve_tenant,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(middleware::make_request_span))
        // Above tenant resolution, and that ordering is the point: layers apply
        // bottom-up, so this runs *first*. Resolving a tenant opens a database
        // pool and may provision one, which is real work to spend on a request
        // that is about to be refused - and refusing before the lookup means a
        // flood of unknown subdomains cannot be used to hammer the catalog.
        //
        // Its own state, not `AppState`: the counters belong to this process,
        // and nothing behind a server function should be able to reach them.
        .layer(axum::middleware::from_fn_with_state(
            throttle,
            rate_limit::enforce,
        ))
        // A panic in one handler returns 500 for that request instead of
        // killing the worker and every connection it was serving.
        .layer(CatchPanicLayer::new());

    let app = if config.server.compression {
        app.layer(CompressionLayer::new())
    } else {
        app
    };

    // --- Background work ---------------------------------------------------
    // Started before the listener, so an upload that arrives in the first
    // second has a worker to be picked up by.
    let background = jobs::spawn(state.clone());

    // --- Serve -------------------------------------------------------------
    let listener = tokio::net::TcpListener::bind(&site_addr)
        .await
        .with_context(|| format!("could not bind to {site_addr}"))?;

    tracing::info!(
        address = %site_addr,
        base_domain = %config.tenancy.base_domain,
        "listening"
    );
    if let Some(default) = config.tenancy.default_tenant() {
        tracing::info!(
            tenant = default,
            "a host without a subdomain will resolve to this tenant"
        );
    }

    // `_with_connect_info`, not the bare version, and the rate limiter depends
    // on it: without it there is no `ConnectInfo` extension, every request keys
    // to the same fallback, and the whole internet shares one allowance.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(config.server.shutdown_timeout_secs))
    .await
    .context("server error")?;

    // --- Shutdown ----------------------------------------------------------
    tracing::info!("draining connections");
    // The workers first: the relay wants its broker connection, and a job
    // half-way through verifying a file wants its database pool. Closing
    // either out from under them would turn an orderly stop into a batch of
    // uploads that have to be retried.
    background.shutdown().await;
    if let Some(messaging) = messaging {
        messaging.close().await;
    }
    tenants.close_all().await;

    Ok(())
}

/// Build the storage backend and the naming strategy from configuration.
///
/// Both are returned as trait objects, which is what keeps the choice here and
/// only here: nothing above this function can name a filesystem path or a
/// directory layout, so a second backend is a new arm below rather than a
/// search of the application.
async fn build_storage(
    config: &AppConfig,
) -> Result<(
    Arc<dyn phonix_storage::FileStorage>,
    Arc<dyn phonix_storage::NamingStrategy>,
)> {
    use phonix_config::{NamingStrategyKind, StorageBackend};

    let storage: Arc<dyn phonix_storage::FileStorage> = match config.storage.backend {
        StorageBackend::Local => {
            let root = config.storage.resolved_root();

            Arc::new(
                phonix_storage::LocalDisk::open(&root)
                    .await
                    .with_context(|| {
                        format!(
                            "could not open the storage root at {}; check [storage].root",
                            root.display()
                        )
                    })?,
            )
        }
    };

    let naming: Arc<dyn phonix_storage::NamingStrategy> = match config.storage.naming {
        NamingStrategyKind::DateSharded => Arc::new(phonix_storage::DateSharded),
        NamingStrategyKind::ContentAddressed => Arc::new(phonix_storage::ContentAddressed),
        NamingStrategyKind::Flat => Arc::new(phonix_storage::Flat),
    };

    Ok((storage, naming))
}

/// Resolve when the process is asked to stop.
///
/// Ctrl+C is the signal Docker Desktop and a terminal both send on Windows;
/// SIGTERM is added on Unix for orchestrators.
async fn shutdown_signal(timeout_secs: u64) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install the Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install the SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C"),
        _ = terminate => tracing::info!("received SIGTERM"),
    }

    tracing::info!(
        timeout_secs,
        "shutting down; waiting for in-flight requests"
    );
}
