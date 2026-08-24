//! Phonix server: Axum + Leptos SSR, database per tenant.

// This binary instantiates `phonix_web::app::App`, whose view is one deeply
// nested tuple type per screen, and the compiler works out its layout
// recursively. The default 128 is exceeded by the shell alone. Same reason as
// the identical attribute in `phonix_web`; this is the type checker's stack,
// not ours.
#![recursion_limit = "512"]

mod auth;
mod files;
mod google;
mod health;
mod jobs;
mod middleware;
mod rate_limit;
mod startup;

use std::process::ExitCode;

use phonix_config::ConfigError;

fn main() -> ExitCode {
    // Configuration and telemetry are brought up before the async runtime so a
    // misconfigured process fails with a plain message instead of a panic
    // inside a worker thread.
    let config = match phonix_config::load() {
        Ok(config) => config,
        Err(err) => {
            report_config_error(&err);
            return ExitCode::FAILURE;
        }
    };

    let _telemetry = match phonix_telemetry::init(&config.telemetry, &config.app.environment) {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("failed to initialise logging: {err}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!(error = %err, "failed to build the tokio runtime");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(startup::run(config)) {
        Ok(()) => {
            tracing::info!("phonix-server stopped cleanly");
            ExitCode::SUCCESS
        }
        Err(err) => {
            // `{err:#}` prints the whole anyhow chain, which is usually where
            // the actionable cause is.
            tracing::error!(error = format!("{err:#}"), "phonix-server failed");
            eprintln!("phonix-server failed: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Report a configuration failure to stderr.
///
/// Logging is not up yet at this point, so this writes directly and spells out
/// the fix rather than only the symptom.
fn report_config_error(err: &ConfigError) {
    eprintln!("phonix-server could not start: {err}");

    if let ConfigError::MissingBase(path) = err {
        eprintln!();
        eprintln!("Expected to find configuration at: {}", path.display());
        eprintln!("Run the server from the workspace root, or set CARGO_MANIFEST_DIR.");
    }
    if matches!(err, ConfigError::MissingSecret { .. }) {
        eprintln!();
        eprintln!("Copy .env.example to .env and fill in the secrets, or export them.");
    }
}
