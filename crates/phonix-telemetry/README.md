# phonix-telemetry — logging and telemetry

![architecture](../../docs/architecture.svg)

Two sinks, independently configurable in `config/*.toml`:

- **console** — human-readable in development, JSON in production.
- **file** — rotating files under `telemetry.file.directory`, JSON by default,
  because log files are read by machines far more often than by people.

Both are driven by one `EnvFilter` built from `telemetry.level` plus
`telemetry.directives`. Setting `RUST_LOG` replaces that filter entirely, which
is the escape hatch for debugging a running process.

## What must never appear in a log

Secrets are typed `secrecy::SecretString` throughout, so a stray `{:?}` prints
`[REDACTED]`. `telemetry.tracing.log_bodies` exists for local debugging and
production validation refuses to start with it enabled — request bodies
routinely contain passwords and personal data.

## How it connects

Initialised once by `phonix-server` at startup; every other crate simply uses
`tracing` macros and does not know this crate exists.
