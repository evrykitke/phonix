//! Logging and telemetry setup.
//!
//! Two sinks, independently configurable in `config/*.toml`:
//!
//! * **console** - human-readable in development, JSON in production.
//! * **file**    - rotating files under `telemetry.file.directory`, JSON by
//!   default because files are read by machines far more often than by people.
//!
//! Both are driven by one `EnvFilter`, built from `telemetry.level` plus
//! `telemetry.directives`. Setting `RUST_LOG` replaces that filter entirely,
//! which is the escape hatch for debugging a running process.

pub mod rolling;

use std::io;
use std::path::PathBuf;

use phonix_config::{ConsoleLogConfig, FileLogConfig, LogFormat, SpanEvents, TelemetryConfig};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("could not create log directory '{path}': {source}")]
    LogDirectory {
        path: String,
        #[source]
        source: io::Error,
    },

    #[error("could not initialise the file log appender: {0}")]
    Appender(String),

    #[error("invalid log filter '{filter}': {source}")]
    Filter {
        filter: String,
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },

    #[error("a global tracing subscriber was already installed")]
    AlreadyInitialised,
}

/// Keeps the non-blocking file writer's worker thread alive.
///
/// Dropping this flushes and stops the writer, so it must be held for the
/// lifetime of the process - bind it in `main`, do not discard it with `let _`.
#[must_use = "dropping the guard stops the background log writer and loses buffered lines"]
pub struct TelemetryGuard {
    _file_guard: Option<WorkerGuard>,
}

/// Install the global subscriber. Call exactly once, as early in `main` as
/// possible so that startup failures are themselves logged.
///
/// `environment` ("development" / "production") is substituted for `{env}` in
/// `telemetry.file.directory` and `telemetry.file.file_name_prefix`, which is
/// how the configured `var/{env}.log` resolves to `var/development.log`.
pub fn init(cfg: &TelemetryConfig, environment: &str) -> Result<TelemetryGuard, TelemetryError> {
    // RUST_LOG wins outright: when someone sets it they are debugging and do
    // not want the file's opinion.
    let directives = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| cfg.filter_directives());

    let make_filter = || -> Result<EnvFilter, TelemetryError> {
        EnvFilter::try_new(&directives).map_err(|source| TelemetryError::Filter {
            filter: directives.clone(),
            source,
        })
    };

    // Collected into a Vec rather than chained with repeated `.with()` calls:
    // each boxed layer is typed against `Registry`, but chaining would ask the
    // second layer to implement `Layer<Layered<.., Registry>>` instead. A Vec of
    // layers is itself a `Layer`, so one `.with()` call keeps every element's
    // subscriber type at `Registry`.
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();

    if cfg.console.enabled {
        layers.push(
            console_layer(&cfg.console)
                .with_filter(make_filter()?)
                .boxed(),
        );
    }

    let file_guard = if cfg.file.enabled {
        let (layer, guard) = file_layer(&cfg.file, environment)?;
        layers.push(layer.with_filter(make_filter()?).boxed());
        Some(guard)
    } else {
        None
    };

    Registry::default()
        .with(layers)
        .try_init()
        .map_err(|_| TelemetryError::AlreadyInitialised)?;

    tracing::debug!(filter = %directives, "tracing subscriber installed");

    Ok(TelemetryGuard {
        _file_guard: file_guard,
    })
}

/// Build the console layer.
///
/// The `LogFormat` arms produce different concrete types, so each is boxed to a
/// single `Layer` object rather than fighting the type system for a marginal
/// gain on a once-per-process call.
fn console_layer(cfg: &ConsoleLogConfig) -> Box<dyn Layer<Registry> + Send + Sync> {
    let base = tracing_subscriber::fmt::layer()
        .with_writer(io::stdout)
        .with_ansi(cfg.ansi)
        .with_target(cfg.show_target)
        .with_thread_ids(cfg.show_thread_ids)
        .with_line_number(cfg.show_line_numbers)
        .with_file(cfg.show_line_numbers)
        .with_span_events(span_events(cfg.show_span_events));

    match cfg.format {
        LogFormat::Pretty => Box::new(base.pretty()),
        LogFormat::Compact => Box::new(base.compact()),
        LogFormat::Full => Box::new(base),
        LogFormat::Json => Box::new(base.json().flatten_event(true)),
    }
}

/// Build the rolling-file layer and its worker guard.
///
/// Files are named `<prefix>.<period>.<seq>.<suffix>`, e.g.
/// `var/development.2026-08-21.001.log`. A new one is started when the next
/// line would push the current file past `max_file_size_mb`, or when the date
/// stamp changes - whichever happens first - and anything not modified within
/// `retention_days` is deleted.
///
/// See [`rolling`] for why this is not `tracing_appender::rolling`: that
/// appender rotates on time only, and its retention is a count of files rather
/// than an age. Neither bounds a disk.
fn file_layer(
    cfg: &FileLogConfig,
    environment: &str,
) -> Result<(Box<dyn Layer<Registry> + Send + Sync>, WorkerGuard), TelemetryError> {
    let directory = resolve_log_dir(&cfg.directory, environment);
    let prefix = cfg.file_name_prefix.replace("{env}", environment);

    std::fs::create_dir_all(&directory).map_err(|source| TelemetryError::LogDirectory {
        path: directory.display().to_string(),
        source,
    })?;

    let appender = rolling::RollingFile::open(rolling::RollingConfig {
        directory: directory.clone(),
        prefix,
        suffix: cfg.file_name_suffix.clone(),
        rotation: cfg.rotation,
        // Megabytes in the configuration because that is the unit an operator
        // thinks in; bytes here because that is what a write is counted in.
        max_bytes: cfg.max_file_size_mb.saturating_mul(1024 * 1024),
        retention_days: cfg.retention_days,
        max_files: cfg.max_files,
    })
    .map_err(|err| TelemetryError::Appender(err.to_string()))?;

    // Writes go to a background thread. A blocking writer would put disk
    // latency directly into the request path.
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let base = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        // ANSI escapes in a log file corrupt it for grep and log shippers.
        .with_ansi(false)
        .with_target(cfg.show_target)
        .with_line_number(cfg.show_line_numbers)
        .with_file(cfg.show_line_numbers)
        .with_span_events(span_events(cfg.show_span_events));

    let layer: Box<dyn Layer<Registry> + Send + Sync> = match cfg.format {
        LogFormat::Pretty => Box::new(base.pretty()),
        LogFormat::Compact => Box::new(base.compact()),
        LogFormat::Full => Box::new(base),
        LogFormat::Json => Box::new(base.json().flatten_event(true)),
    };

    Ok((layer, guard))
}

/// Resolve `telemetry.file.directory` to a concrete path.
///
/// `{env}` expands to the environment name. An absolute path is used verbatim,
/// so a deployment can point logs at `/var/log/phonix` or `D:\logs\phonix`. A
/// relative path is anchored to the workspace root rather than the process
/// working directory - otherwise `cargo leptos watch` and a packaged binary
/// would scatter logs into different places.
fn resolve_log_dir(configured: &str, environment: &str) -> PathBuf {
    let expanded = configured.replace("{env}", environment);
    let path = PathBuf::from(&expanded);

    if path.is_absolute() {
        path
    } else {
        phonix_config::workspace_root().join(path)
    }
}

fn span_events(events: SpanEvents) -> FmtSpan {
    match events {
        SpanEvents::None => FmtSpan::NONE,
        SpanEvents::New => FmtSpan::NEW,
        SpanEvents::Enter => FmtSpan::ENTER,
        SpanEvents::Exit => FmtSpan::EXIT,
        SpanEvents::Close => FmtSpan::CLOSE,
        SpanEvents::Active => FmtSpan::ACTIVE,
        SpanEvents::Full => FmtSpan::FULL,
    }
}
