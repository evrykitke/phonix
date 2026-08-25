//! Layered configuration for Phonix.
//!
//! Sources are merged in this order, later winning over earlier:
//!
//! 1. `config/base.toml`            - required, committed
//! 2. `config/{environment}.toml`   - optional, committed
//! 3. `config/local.toml`           - optional, gitignored, per-machine
//! 4. `PHONIX__*` environment vars  - secrets and deploy-time overrides
//!
//! Environment variables nest with a double underscore, so
//! `PHONIX__DATABASE__PASSWORD` sets `database.password`.
//!
//! Secrets are typed as [`secrecy::SecretString`], whose `Debug` prints
//! `[REDACTED]`. That matters here because the whole config struct is logged at
//! startup.

pub mod model;
pub mod numbering;
pub mod validate;

use std::path::{Path, PathBuf};

use config::{Config, Environment as EnvSource, File, FileFormat};

pub use model::*;
pub use validate::ConfigError;

/// Name of the environment variable that selects the second config layer.
pub const ENV_VAR: &str = "PHONIX_ENV";

/// Which deployment this process is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Development,
    Production,
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
        }
    }

    pub fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }

    fn parse(raw: &str) -> Result<Self, ConfigError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" | "local" => Ok(Self::Development),
            "production" | "prod" => Ok(Self::Production),
            other => Err(ConfigError::UnknownEnvironment(other.to_owned())),
        }
    }
}

/// Load `.env` (if present) then build the configuration.
///
/// `.env` is loaded first so that `PHONIX_ENV` and the `PHONIX__*` secrets it
/// contains are visible to the layering below. Real process environment
/// variables always win over `.env` entries.
pub fn load() -> Result<AppConfig, ConfigError> {
    let _ = dotenvy::dotenv();
    let root = workspace_root();
    load_from(root.join("config"))
}

/// Build the configuration from an explicit config directory.
///
/// Separated from [`load`] so tests can point at a fixture directory.
pub fn load_from(config_dir: impl AsRef<Path>) -> Result<AppConfig, ConfigError> {
    let config_dir = config_dir.as_ref();

    let environment = std::env::var(ENV_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| RunMode::parse(&value))
        .transpose()?
        .unwrap_or(RunMode::Development);

    if !config_dir.join("base.toml").is_file() {
        return Err(ConfigError::MissingBase(config_dir.join("base.toml")));
    }

    let builder = Config::builder()
        .add_source(File::from(config_dir.join("base.toml")).format(FileFormat::Toml))
        .add_source(
            File::from(config_dir.join(format!("{}.toml", environment.as_str())))
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(
            File::from(config_dir.join("local.toml"))
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(
            EnvSource::with_prefix("PHONIX")
                // `PHONIX__DATABASE__PASSWORD` -> ["database", "password"].
                // Both separators are `__` so that single underscores stay
                // intact inside key names such as `key_prefix`.
                .prefix_separator("__")
                .separator("__")
                // Environment values are strings; without this, `port = "5432"`
                // fails to deserialize into a u16.
                .try_parsing(true)
                .list_separator(",")
                .with_list_parse_key("telemetry.directives")
                .with_list_parse_key("server.cors.allowed_origins")
                .with_list_parse_key("tenancy.reserved_subdomains"),
        );

    let mut settings: AppConfig = builder
        .build()
        .map_err(ConfigError::Build)?
        .try_deserialize()
        .map_err(ConfigError::Deserialize)?;

    // The environment file is the authority on which environment this is; a
    // stale `app.environment` in base.toml must not contradict PHONIX_ENV.
    settings.app.environment = environment.as_str().to_owned();

    validate::check(&settings, environment)?;

    Ok(settings)
}

/// Locate the workspace root so the app can be started from any directory.
///
/// Walks up from the executable's directory and from the current directory
/// looking for the marker files a Phonix checkout always has.
pub fn workspace_root() -> PathBuf {
    fn find_upwards(start: &Path) -> Option<PathBuf> {
        start.ancestors().find_map(|dir| {
            let looks_right =
                dir.join("config").join("base.toml").is_file() && dir.join("Cargo.toml").is_file();
            looks_right.then(|| dir.to_path_buf())
        })
    }

    // `cargo leptos` and `cargo run` set this; it is the most reliable signal.
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR")
        && let Some(root) = find_upwards(Path::new(&manifest))
    {
        return root;
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Some(root) = find_upwards(&cwd)
    {
        return root;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(root) = exe.parent().and_then(find_upwards)
    {
        return root;
    }

    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_aliases_are_accepted() {
        assert_eq!(RunMode::parse("dev").unwrap(), RunMode::Development);
        assert_eq!(RunMode::parse(" PROD ").unwrap(), RunMode::Production);
        assert!(RunMode::parse("staging").is_err());
    }
}
