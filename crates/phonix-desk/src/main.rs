//! Phonix Desk: the application the platform is run from.
//!
//! A second binary beside `phonix-server`, sharing the same configuration file
//! and the same crates underneath, serving a different surface to a different
//! set of people. Workspaces are created, licensed and stopped here.
//!
//! Read `docs/adr/0005-phonix-desk.md` before changing any of it. The three
//! decisions that shape everything below:
//!
//! * **A desk user is not a `Caller`.** `Caller` is tenant-scoped and every
//!   gate in `phonix-services` is written against it. Desk has its own identity
//!   in the catalog, and the two never meet.
//! * **Server-rendered, no wasm.** A tool wanted when the product is broken
//!   must not be built on the product's hydration. Every page is complete
//!   without JavaScript.
//! * **Loopback, with nginx in front** on `console-desk.<base_domain>`. A
//!   public bind answers any `Host` header and can be reached by address, which
//!   skips `server_name` matching altogether.
//!
//! # Usage
//!
//! ```text
//! phonix-desk                        serve
//! phonix-desk bootstrap <email> <name>   create the first account and print its setup link
//! ```

mod assets;
mod cookie;
mod html;
mod routes;
mod state;

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use phonix_config::{AppConfig, ConfigError};
use phonix_db::tenancy::Catalog;
use phonix_services::crypto::Hasher;
use phonix_services::crypto::vault::SecretVault;

use crate::state::DeskState;

fn main() -> ExitCode {
    // Configuration and telemetry come up before the runtime, so a misconfigured
    // process fails with a plain message rather than a panic inside a worker.
    let config = match phonix_config::load() {
        Ok(config) => config,
        Err(err) => {
            report_config_error(&err);
            return ExitCode::FAILURE;
        }
    };

    let _telemetry =
        match phonix_telemetry::init(&config.telemetry, &config.app.environment, Vec::new()) {
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

    let outcome = match Command::from_args(std::env::args().skip(1)) {
        Ok(Command::Serve) => runtime.block_on(serve(config)),
        Ok(Command::Bootstrap { email, name }) => {
            runtime.block_on(bootstrap(config, &email, &name))
        }
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::FAILURE;
        }
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = format!("{err:#}"), "phonix-desk failed");
            eprintln!("phonix-desk failed: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// What the process was asked to do.
///
/// Hand-parsed rather than pulled in with a CLI crate: there are two commands
/// and one of them is the default, and a dependency for that is a dependency to
/// keep current for the life of the tool.
enum Command {
    Serve,
    Bootstrap { email: String, name: String },
}

const USAGE: &str = "usage:\n  \
    phonix-desk                              serve\n  \
    phonix-desk bootstrap <email> <name>     create the first desk account";

impl Command {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self, &'static str> {
        match args.next().as_deref() {
            None => Ok(Self::Serve),
            Some("serve") => Ok(Self::Serve),
            Some("bootstrap") => {
                let email = args.next().ok_or(USAGE)?;
                let name = args.next().ok_or(USAGE)?;
                Ok(Self::Bootstrap { email, name })
            }
            Some(_) => Err(USAGE),
        }
    }
}

/// Everything both commands need.
async fn build_state(config: AppConfig) -> Result<DeskState> {
    let pool = phonix_db::connect::catalog_pool(&config.database)
        .await
        .context("could not connect to the catalog database")?;
    let catalog = Catalog::new(pool);

    // Desk applies catalog migrations - its own tables live there, and the
    // process that owns them is the one that should create them. Tenant
    // databases are `phonix-server`'s to migrate on start, and Desk will do it
    // deliberately per workspace later.
    catalog
        .migrate()
        .await
        .context("could not migrate the catalog database")?;

    let hasher = Hasher::new(&config.security.password)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .context("could not build the password hasher")?;
    let vault = SecretVault::from_config(&config.security.mfa)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .context("could not open the secret vault - check security.mfa.encryption_key")?;

    Ok(DeskState::new(catalog, Arc::new(config), hasher, vault))
}

async fn serve(config: AppConfig) -> Result<()> {
    let listen = config.desk.listen.clone();
    let state = build_state(config).await?;

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("could not bind {listen}"))?;

    tracing::info!(
        address = %listen,
        environment = %state.environment(),
        "phonix-desk is listening"
    );

    let app = routes::router(state);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("the desk server stopped unexpectedly")?;

    tracing::info!("phonix-desk stopped cleanly");
    Ok(())
}

/// Create the first desk account and print its setup link.
///
/// Run on the box, by somebody who already has SSH. This is the only way an
/// account comes into being without another account already existing, and it is
/// deliberately not a web page: a Desk with no accounts must not have a route
/// that creates one, because that route is reachable by anybody the day nginx
/// is misconfigured.
async fn bootstrap(config: AppConfig, email: &str, name: &str) -> Result<()> {
    use secrecy::ExposeSecret;

    let state = build_state(config).await?;

    // Refused rather than "the first one wins": a second bootstrap on a live
    // deployment would be somebody creating themselves an account on a box they
    // have shell on, which is a real thing to be able to do and not a thing to
    // do silently.
    let existing = phonix_services::desk::account::list(state.pool()).await?;
    if !existing.is_empty() {
        anyhow::bail!(
            "there are already {} desk account(s). Create the next one from Desk itself, \
             so it is attributed to whoever created it.",
            existing.len()
        );
    }

    let created = phonix_services::desk::account::create(
        state.pool(),
        state.desk(),
        email,
        name,
        None,
        Default::default(),
    )
    .await?;

    let hours = state.desk().setup_link_hours;

    println!();
    println!("Created the first desk account for {}.", created.user.email);
    println!();
    println!("Open this on the machine that will hold the authenticator:");
    println!();
    println!("  /setup/{}", created.setup_token.expose_secret());
    println!();
    println!("Prefix it with the address Desk is served at. The link is single-use");
    println!("and expires in {hours} hours.");
    println!();

    Ok(())
}

/// Stop on Ctrl-C or SIGTERM, so systemd's `stop` is not a kill.
async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for ctrl-c");
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => tracing::error!(error = %err, "could not listen for SIGTERM"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => tracing::info!("interrupt received, shutting down"),
        () = terminate => tracing::info!("SIGTERM received, shutting down"),
    }
}

fn report_config_error(err: &ConfigError) {
    eprintln!("phonix-desk could not start: {err}");

    if let ConfigError::MissingBase(path) = err {
        eprintln!();
        eprintln!("Expected to find configuration at: {}", path.display());
        eprintln!("Run from the workspace root, or set CARGO_MANIFEST_DIR.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_means_serve() {
        assert!(matches!(
            Command::from_args(std::iter::empty()),
            Ok(Command::Serve)
        ));
    }

    #[test]
    fn bootstrap_needs_both_an_address_and_a_name() {
        let one = Command::from_args(["bootstrap".to_owned(), "a@b.c".to_owned()].into_iter());
        assert!(one.is_err(), "a name is not optional");

        let both = Command::from_args(
            ["bootstrap".to_owned(), "a@b.c".to_owned(), "Ada".to_owned()].into_iter(),
        );
        assert!(matches!(both, Ok(Command::Bootstrap { .. })));
    }

    #[test]
    fn an_unknown_command_is_refused_rather_than_served() {
        // The failure that matters: `phonix-desk --help` must not silently
        // start a server on the operator console's port.
        assert!(Command::from_args(["--help".to_owned()].into_iter()).is_err());
    }
}
