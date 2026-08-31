//! Fail-fast configuration checks.
//!
//! These run once at startup. The intent is that a misconfigured process dies
//! immediately with a precise message, rather than serving traffic and failing
//! later on the first cache write or the hundredth tenant.

use std::path::PathBuf;

use secrecy::{ExposeSecret, SecretString};

use crate::{RunMode, model::*};

/// The MFA key committed to `config/development.toml`.
///
/// Public by construction - it is in the repository - which is exactly why
/// production refuses to start with it.
pub const DEVELOPMENT_MFA_KEY: &str = "cGhvbml4LWRldmVsb3BtZW50LW9ubHktbWZhLWtleSE=";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("PHONIX_ENV must be 'development' or 'production', got '{0}'")]
    UnknownEnvironment(String),

    #[error("configuration file not found: {0}")]
    MissingBase(PathBuf),

    #[error("failed to read configuration: {0}")]
    Build(#[source] config::ConfigError),

    #[error("configuration does not match the expected schema: {0}")]
    Deserialize(#[source] config::ConfigError),

    #[error("invalid configuration: {0}")]
    Invalid(String),

    #[error(
        "{secret} is empty. In production it must be supplied via the {env_var} \
         environment variable - never committed to config/*.toml"
    )]
    MissingSecret {
        secret: &'static str,
        env_var: &'static str,
    },
}

impl ConfigError {
    fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
}

/// Validate a fully-merged configuration.
pub fn check(cfg: &AppConfig, mode: RunMode) -> Result<(), ConfigError> {
    check_server(&cfg.server)?;
    check_database(&cfg.database)?;
    check_redis(&cfg.redis)?;
    check_rabbitmq(&cfg.rabbitmq)?;
    check_telemetry(&cfg.telemetry)?;
    check_tenancy(&cfg.tenancy)?;
    check_security(&cfg.security)?;
    check_smtp(&cfg.smtp)?;
    check_storage(&cfg.storage)?;

    if mode.is_production() {
        check_production_secrets(cfg)?;
        check_production_hardening(cfg)?;
    }

    Ok(())
}

fn check_server(server: &ServerConfig) -> Result<(), ConfigError> {
    if server.port == 0 {
        return Err(ConfigError::invalid("server.port must not be 0"));
    }
    if !matches!(server.scheme.as_str(), "http" | "https") {
        return Err(ConfigError::invalid(format!(
            "server.scheme must be 'http' or 'https', got '{}'",
            server.scheme
        )));
    }
    if server.base_domain.trim().is_empty() {
        return Err(ConfigError::invalid("server.base_domain must not be empty"));
    }
    if server.body_limit_bytes == 0 {
        return Err(ConfigError::invalid(
            "server.body_limit_bytes must be greater than 0",
        ));
    }
    if server.cors.enabled && server.cors.allowed_origins.is_empty() {
        return Err(ConfigError::invalid(
            "server.cors.enabled is true but server.cors.allowed_origins is empty",
        ));
    }
    Ok(())
}

fn check_database(db: &DatabaseConfig) -> Result<(), ConfigError> {
    if db.username.trim().is_empty() {
        return Err(ConfigError::invalid("database.username must not be empty"));
    }
    if db.catalog_database.trim().is_empty() {
        return Err(ConfigError::invalid(
            "database.catalog_database must not be empty",
        ));
    }
    if db.tenant_database_prefix.trim().is_empty() {
        return Err(ConfigError::invalid(
            "database.tenant_database_prefix must not be empty",
        ));
    }
    // The prefix is concatenated with a slug to form a Postgres identifier, so
    // it has to be a legal identifier start on its own.
    let prefix_ok = db
        .tenant_database_prefix
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && db
            .tenant_database_prefix
            .starts_with(|c: char| c.is_ascii_lowercase() || c == '_');
    if !prefix_ok {
        return Err(ConfigError::invalid(format!(
            "database.tenant_database_prefix '{}' must contain only [a-z0-9_] and \
             start with a letter or underscore",
            db.tenant_database_prefix
        )));
    }

    check_pool("database.catalog_pool", &db.catalog_pool)?;
    check_pool("database.tenant_pool", &db.tenant_pool)?;

    if db.tenant_registry.max_cached_pools == 0 {
        return Err(ConfigError::invalid(
            "database.tenant_registry.max_cached_pools must be at least 1",
        ));
    }
    Ok(())
}

fn check_pool(label: &str, pool: &PoolConfig) -> Result<(), ConfigError> {
    if pool.max_connections == 0 {
        return Err(ConfigError::invalid(format!(
            "{label}.max_connections must be at least 1"
        )));
    }
    if pool.min_connections > pool.max_connections {
        return Err(ConfigError::invalid(format!(
            "{label}.min_connections ({}) exceeds max_connections ({})",
            pool.min_connections, pool.max_connections
        )));
    }
    if pool.acquire_timeout_secs == 0 {
        return Err(ConfigError::invalid(format!(
            "{label}.acquire_timeout_secs must be greater than 0"
        )));
    }
    // A connection recycled before it can go idle churns the pool pointlessly.
    if pool.max_lifetime_secs > 0 && pool.max_lifetime_secs < pool.idle_timeout_secs {
        return Err(ConfigError::invalid(format!(
            "{label}.max_lifetime_secs ({}) is shorter than idle_timeout_secs ({}); \
             connections would be retired before they could be reused",
            pool.max_lifetime_secs, pool.idle_timeout_secs
        )));
    }
    Ok(())
}

fn check_redis(redis: &RedisConfig) -> Result<(), ConfigError> {
    if !redis.enabled {
        return Ok(());
    }
    if redis.key_prefix.trim().is_empty() {
        return Err(ConfigError::invalid(
            "redis.key_prefix must not be empty - it is what isolates tenants in the cache",
        ));
    }
    if redis.key_prefix.contains(':') {
        return Err(ConfigError::invalid(
            "redis.key_prefix must not contain ':' - the separator is added automatically",
        ));
    }
    if redis.database > 15 {
        return Err(ConfigError::invalid(
            "redis.database must be 0-15 for a default Redis configuration",
        ));
    }
    Ok(())
}

fn check_rabbitmq(mq: &RabbitMqConfig) -> Result<(), ConfigError> {
    if !mq.enabled {
        return Ok(());
    }
    if !matches!(
        mq.exchange_kind.as_str(),
        "direct" | "fanout" | "topic" | "headers"
    ) {
        return Err(ConfigError::invalid(format!(
            "rabbitmq.exchange_kind must be direct|fanout|topic|headers, got '{}'",
            mq.exchange_kind
        )));
    }
    if mq.exchange.trim().is_empty() {
        return Err(ConfigError::invalid("rabbitmq.exchange must not be empty"));
    }
    if mq.pool.max_size == 0 {
        return Err(ConfigError::invalid(
            "rabbitmq.pool.max_size must be at least 1",
        ));
    }
    if mq.retry_initial_backoff_ms > mq.retry_max_backoff_ms {
        return Err(ConfigError::invalid(
            "rabbitmq.retry_initial_backoff_ms exceeds retry_max_backoff_ms",
        ));
    }

    let mut seen = std::collections::HashSet::new();
    for consumer in &mq.consumers {
        if !seen.insert(consumer.name.as_str()) {
            return Err(ConfigError::invalid(format!(
                "duplicate rabbitmq consumer name '{}'",
                consumer.name
            )));
        }
        if consumer.routing_keys.is_empty() {
            return Err(ConfigError::invalid(format!(
                "rabbitmq consumer '{}' has no routing_keys, so its queue would \
                 never receive anything",
                consumer.name
            )));
        }
        if consumer.concurrency == 0 {
            return Err(ConfigError::invalid(format!(
                "rabbitmq consumer '{}' has concurrency 0",
                consumer.name
            )));
        }
    }
    Ok(())
}

/// The relay, checked only as far as it can be without connecting.
///
/// A disabled mailer is not checked at all: "no mail on this machine" is a
/// legitimate configuration and the point of the flag. An *enabled* one with no
/// host is not - it fails on the first invitation instead, hours later, in
/// front of whoever was adding a colleague.
fn check_smtp(smtp: &SmtpConfig) -> Result<(), ConfigError> {
    if !smtp.enabled {
        return Ok(());
    }

    if smtp.host.trim().is_empty() {
        return Err(ConfigError::Invalid(
            "smtp.host is empty but smtp.enabled is true".into(),
        ));
    }
    if smtp.port == 0 {
        return Err(ConfigError::Invalid("smtp.port must not be 0".into()));
    }
    // Only the shape, and only the part that is unambiguous. Whether the relay
    // will accept this sender is the relay's answer, not ours.
    if !smtp.from_address.contains('@') {
        return Err(ConfigError::Invalid(format!(
            "smtp.from_address is not an address: '{}'",
            smtp.from_address
        )));
    }
    if smtp.timeout_secs == 0 {
        return Err(ConfigError::Invalid(
            "smtp.timeout_secs must not be 0".into(),
        ));
    }

    Ok(())
}

/// Storage, and one check that is worth more than the rest of them.
///
/// `max_upload_bytes` is the ceiling the HTTP layer refuses past. The per-bucket
/// limits in `phonix_core::files` are what actually decide whether a file is
/// acceptable, and they are read *after* the bytes have arrived. So if this
/// ceiling is the smaller of the two, a bucket's own limit becomes unreachable:
/// an avatar bucket allowing 2 MB behind a 1 MB ceiling means every upload over
/// 1 MB dies as a truncated request rather than as an answer anybody can act on.
///
/// Catching that here rather than in production is the whole point of a
/// fail-fast check: the two numbers live in different files, one of them is
/// code and one is configuration, and nothing else would ever compare them.
fn check_storage(storage: &StorageConfig) -> Result<(), ConfigError> {
    if storage.root.trim().is_empty() {
        return Err(ConfigError::invalid(
            "storage.root is empty; it must name a directory uploads can be written to",
        ));
    }

    let largest_bucket = phonix_core::files::largest_bucket_limit();
    if storage.max_upload_bytes < largest_bucket {
        return Err(ConfigError::invalid(format!(
            "storage.max_upload_bytes is {}, which is below the largest bucket limit of {} \
             declared in phonix_core::files. Every upload above the smaller number would be \
             refused by the transport before the bucket's own limit could answer it",
            storage.max_upload_bytes, largest_bucket
        )));
    }

    if storage.upload_timeout_secs == 0 {
        return Err(ConfigError::invalid(
            "storage.upload_timeout_secs must be greater than zero",
        ));
    }

    if storage.quarantine_ttl_mins == 0 {
        return Err(ConfigError::invalid(
            "storage.quarantine_ttl_mins must be greater than zero, or bytes would be swept \
             away between arriving and being verified",
        ));
    }

    check_upload_jobs(&storage.jobs)
}

fn check_upload_jobs(jobs: &UploadJobsConfig) -> Result<(), ConfigError> {
    if !jobs.enabled {
        // Not an error - a process that only serves pages is a legitimate
        // arrangement - but it is worth saying out loud, because the symptom
        // is uploads that sit at "queued" and never move.
        tracing::warn!(
            "storage.jobs.enabled is false; uploads will be accepted but never verified"
        );
        return Ok(());
    }

    if jobs.concurrency == 0 {
        return Err(ConfigError::invalid(
            "storage.jobs.concurrency must be at least 1 when the worker is enabled",
        ));
    }

    if jobs.poll_interval_secs == 0 {
        return Err(ConfigError::invalid(
            "storage.jobs.poll_interval_secs must be greater than zero",
        ));
    }

    if jobs.max_attempts == 0 {
        return Err(ConfigError::invalid(
            "storage.jobs.max_attempts must be at least 1, or no upload would ever be tried",
        ));
    }

    if jobs.claim_timeout_secs <= jobs.poll_interval_secs {
        return Err(ConfigError::invalid(
            "storage.jobs.claim_timeout_secs must be longer than poll_interval_secs, or a job \
             would be reclaimed by a second worker while the first was still running it",
        ));
    }

    Ok(())
}

/// The file sink's two size limits, and the one combination that is a trap.
///
/// Turning off *both* rotation and the size cap leaves a single file that grows
/// until the disk is full - which is the state this whole appender exists to
/// prevent, and which is reachable by setting two innocuous-looking values.
fn check_file_log(file: &FileLogConfig) -> Result<(), ConfigError> {
    if !file.enabled {
        return Ok(());
    }

    if file.file_name_prefix.trim().is_empty() {
        return Err(ConfigError::invalid(
            "telemetry.file.file_name_prefix is empty; log files would have no name",
        ));
    }

    if matches!(file.rotation, Rotation::Never) && file.max_file_size_mb == 0 {
        return Err(ConfigError::invalid(
            "telemetry.file has rotation = \"never\" and max_file_size_mb = 0, which is one \
             log file that grows until the disk is full. Set one of them",
        ));
    }

    if file.retention_days == 0 && file.max_files == 0 {
        // Not fatal - an operator whose log shipper does the reaping is
        // entitled to say so - but it is worth one line at startup, because the
        // symptom is a directory nobody looks at until it is full.
        tracing::warn!(
            "telemetry.file keeps every log file for ever: retention_days and max_files are both 0"
        );
    }

    Ok(())
}

fn check_telemetry(telemetry: &TelemetryConfig) -> Result<(), ConfigError> {
    check_file_log(&telemetry.file)?;

    const LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];
    if !LEVELS.contains(&telemetry.level.to_ascii_lowercase().as_str()) {
        return Err(ConfigError::invalid(format!(
            "telemetry.level must be one of {LEVELS:?}, got '{}'",
            telemetry.level
        )));
    }
    if !telemetry.console.enabled && !telemetry.file.enabled {
        return Err(ConfigError::invalid(
            "both telemetry.console.enabled and telemetry.file.enabled are false - \
             the process would produce no logs at all",
        ));
    }
    if telemetry.file.enabled && telemetry.file.directory.trim().is_empty() {
        return Err(ConfigError::invalid(
            "telemetry.file.directory must not be empty when file logging is enabled",
        ));
    }
    Ok(())
}

fn check_tenancy(tenancy: &TenancyConfig) -> Result<(), ConfigError> {
    if tenancy.base_domain.trim().is_empty() {
        return Err(ConfigError::invalid(
            "tenancy.base_domain must not be empty",
        ));
    }
    if tenancy.strategy == TenantStrategy::Header && tenancy.header_name.trim().is_empty() {
        return Err(ConfigError::invalid(
            "tenancy.header_name is required when tenancy.strategy = 'header'",
        ));
    }
    // A default tenant that is itself reserved would be unreachable.
    if let Some(default) = tenancy.default_tenant()
        && tenancy.is_reserved(default)
    {
        return Err(ConfigError::invalid(format!(
            "tenancy.default_tenant '{default}' is also listed in reserved_subdomains"
        )));
    }
    Ok(())
}

fn check_security(security: &SecurityConfig) -> Result<(), ConfigError> {
    let pw = &security.password;

    // The floor from the OWASP Password Storage Cheat Sheet. Below this the
    // hash stops being meaningfully memory-hard, which is the entire reason
    // for choosing Argon2id over a faster function.
    const MIN_MEMORY_KIB: u32 = 19456;

    if pw.memory_kib < MIN_MEMORY_KIB {
        return Err(ConfigError::invalid(format!(
            "security.password.memory_kib is {} KiB, below the {MIN_MEMORY_KIB} KiB \
             floor recommended by OWASP for Argon2id. Lowering it to speed up \
             sign-in defeats the point of the algorithm - raise the hardware instead",
            pw.memory_kib
        )));
    }
    if pw.iterations < 2 {
        return Err(ConfigError::invalid(
            "security.password.iterations must be at least 2",
        ));
    }
    if pw.parallelism == 0 {
        return Err(ConfigError::invalid(
            "security.password.parallelism must be at least 1",
        ));
    }
    // Argon2 requires m_cost >= 8 * p_cost; caught here with a readable message
    // rather than as an opaque error on the first sign-in attempt.
    if pw.memory_kib < 8 * pw.parallelism {
        return Err(ConfigError::invalid(format!(
            "security.password.memory_kib ({}) must be at least 8 x parallelism ({})",
            pw.memory_kib, pw.parallelism
        )));
    }

    let session = &security.session;
    if session.cookie_name.trim().is_empty() {
        return Err(ConfigError::invalid(
            "security.session.cookie_name must not be empty",
        ));
    }
    // The name is concatenated with a tenant slug and written into a header.
    if !session
        .cookie_name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ConfigError::invalid(
            "security.session.cookie_name must contain only letters, digits, '_' and '-'",
        ));
    }
    if session.idle_timeout_mins == 0 {
        return Err(ConfigError::invalid(
            "security.session.idle_timeout_mins must be greater than 0",
        ));
    }
    if session.absolute_timeout_hours == 0 {
        return Err(ConfigError::invalid(
            "security.session.absolute_timeout_hours must be greater than 0",
        ));
    }
    // The absolute deadline is a ceiling. If the idle window were longer, the
    // sliding expiry would be clamped on the very first request and the idle
    // setting would silently do nothing.
    if session.idle_timeout_mins > session.absolute_timeout_hours * 60 {
        return Err(ConfigError::invalid(format!(
            "security.session.idle_timeout_mins ({}) exceeds absolute_timeout_hours \
             ({} h = {} min); the idle window would never be reached",
            session.idle_timeout_mins,
            session.absolute_timeout_hours,
            session.absolute_timeout_hours * 60
        )));
    }
    // The mobile deadlines are the same two rules, asked of the other block.
    // Not folded into a loop over both: the messages have to name the setting a
    // person would edit, and "security.session.idle_timeout_mins" sends
    // somebody to the wrong line.
    let mobile = &session.mobile;
    if mobile.idle_timeout_mins == 0 {
        return Err(ConfigError::invalid(
            "security.session.mobile.idle_timeout_mins must be greater than 0",
        ));
    }
    if mobile.absolute_timeout_days == 0 {
        return Err(ConfigError::invalid(
            "security.session.mobile.absolute_timeout_days must be greater than 0",
        ));
    }
    if mobile.idle_timeout_mins > mobile.absolute_timeout_hours() * 60 {
        return Err(ConfigError::invalid(format!(
            "security.session.mobile.idle_timeout_mins ({}) exceeds              absolute_timeout_days ({} d = {} min); the idle window would never              be reached",
            mobile.idle_timeout_mins,
            mobile.absolute_timeout_days,
            mobile.absolute_timeout_hours() * 60
        )));
    }

    if session.handoff_ttl_secs == 0 {
        return Err(ConfigError::invalid(
            "security.session.handoff_ttl_secs must be greater than 0",
        ));
    }
    // A handoff token is redeemed by an immediate redirect. A long-lived one is
    // a bearer credential for a full session sitting in a URL, which lands in
    // browser history, proxy logs and Referer headers.
    if session.handoff_ttl_secs > 600 {
        return Err(ConfigError::invalid(
            "security.session.handoff_ttl_secs must be at most 600 - the token \
             travels in a URL, so it belongs in history and logs for as short a \
             time as possible",
        ));
    }
    // SameSite=None without Secure is ignored by every current browser, so the
    // cookie would silently not be stored at all.
    if security.session.same_site == SameSitePolicy::None && !security.session.secure {
        return Err(ConfigError::invalid(
            "security.session.same_site = 'none' requires secure = true; browsers \
             reject a SameSite=None cookie that is not Secure",
        ));
    }

    if security.lockout.max_failed_attempts < 0 {
        return Err(ConfigError::invalid(
            "security.lockout.max_failed_attempts must not be negative (0 disables lockout)",
        ));
    }
    if security.lockout.max_failed_attempts > 0 && security.lockout.lockout_mins == 0 {
        return Err(ConfigError::invalid(
            "security.lockout.lockout_mins must be greater than 0 when lockout is enabled - \
             a zero-length lock would expire before it could be observed",
        ));
    }

    check_mfa(&security.mfa)?;
    check_invitations(&security.invitations)?;
    check_workspace_defaults(&security.workspace_defaults)?;

    Ok(())
}

fn check_invitations(invitations: &InvitationConfig) -> Result<(), ConfigError> {
    if invitations.ttl_hours == 0 {
        return Err(ConfigError::Invalid(
            "security.invitations.ttl_hours must not be 0 - an invitation that \
             expires immediately cannot be accepted"
                .into(),
        ));
    }
    // A month. Past that it is not an invitation, it is a standing credential
    // sitting in somebody's inbox.
    if invitations.ttl_hours > 720 {
        return Err(ConfigError::Invalid(
            "security.invitations.ttl_hours must not exceed 720 (30 days)".into(),
        ));
    }
    Ok(())
}

fn check_mfa(mfa: &MfaConfig) -> Result<(), ConfigError> {
    if mfa.issuer.trim().is_empty() {
        return Err(ConfigError::invalid(
            "security.mfa.issuer must not be empty - it is the name the \
             authenticator app shows, and an empty one leaves users with an \
             unlabelled entry they cannot identify",
        ));
    }
    // The issuer goes into an `otpauth://` URI, where a colon separates the
    // issuer from the account name. One inside the issuer produces an entry the
    // app parses wrongly.
    if mfa.issuer.contains(':') {
        return Err(ConfigError::invalid(
            "security.mfa.issuer must not contain ':' - it separates the issuer \
             from the account name in the otpauth:// URI",
        ));
    }
    // RFC 4226 allows 6-8. Fewer than six is guessable; more than eight is not
    // implemented by the apps.
    if !(6..=8).contains(&mfa.totp_digits) {
        return Err(ConfigError::invalid(format!(
            "security.mfa.totp_digits must be between 6 and 8, got {}",
            mfa.totp_digits
        )));
    }
    if !(15..=120).contains(&mfa.totp_step_secs) {
        return Err(ConfigError::invalid(format!(
            "security.mfa.totp_step_secs must be between 15 and 120, got {} - \
             authenticator apps assume 30 and most cannot be told otherwise",
            mfa.totp_step_secs
        )));
    }
    // Each step is a whole extra window in which a code stays valid. Three
    // steps at 30 seconds already means a shoulder-surfed code works for three
    // and a half minutes.
    if mfa.totp_skew_steps > 3 {
        return Err(ConfigError::invalid(format!(
            "security.mfa.totp_skew_steps must be at most 3, got {} - each step \
             keeps a code usable for another {} seconds",
            mfa.totp_skew_steps, mfa.totp_step_secs
        )));
    }
    // RFC 4226 section 4: the shared secret must be at least 128 bits and
    // should be 160.
    if !(16..=64).contains(&mfa.secret_bytes) {
        return Err(ConfigError::invalid(format!(
            "security.mfa.secret_bytes must be between 16 and 64, got {} - \
             RFC 4226 requires at least 16 and recommends 20",
            mfa.secret_bytes
        )));
    }
    if mfa.recovery_code_count > 24 {
        return Err(ConfigError::invalid(
            "security.mfa.recovery_code_count must be at most 24 - a list nobody \
             will store safely is not a recovery plan",
        ));
    }
    // A six-digit code is a million guesses, and each guess costs one HMAC
    // rather than an Argon2 hash. Unlimited attempts make the second factor a
    // formality.
    if mfa.max_challenge_attempts == 0 || mfa.max_challenge_attempts > 10 {
        return Err(ConfigError::invalid(format!(
            "security.mfa.max_challenge_attempts must be between 1 and 10, got {}",
            mfa.max_challenge_attempts
        )));
    }
    if mfa.challenge_ttl_mins == 0 || mfa.challenge_ttl_mins > 60 {
        return Err(ConfigError::invalid(format!(
            "security.mfa.challenge_ttl_mins must be between 1 and 60, got {} - \
             a half-authenticated session is a password already proven, so it \
             must not sit around",
            mfa.challenge_ttl_mins
        )));
    }

    Ok(())
}

/// The seed policies have to be ones a workspace could have saved itself.
///
/// Checked at startup rather than at workspace creation: a deployment whose
/// defaults are impossible would otherwise produce broken workspaces one at a
/// time, each failing at the moment somebody signed up.
fn check_workspace_defaults(defaults: &WorkspaceDefaults) -> Result<(), ConfigError> {
    if let Err(errors) = defaults.as_settings().validate() {
        let detail = errors
            .iter()
            .map(|err| format!("{}: {}", err.field, err.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ConfigError::invalid(format!(
            "security.workspace_defaults is not a policy a workspace could save - {detail}"
        )));
    }
    Ok(())
}

fn check_production_secrets(cfg: &AppConfig) -> Result<(), ConfigError> {
    let empty = |secret: &SecretString| secret.expose_secret().trim().is_empty();

    if empty(&cfg.database.password) {
        return Err(ConfigError::MissingSecret {
            secret: "database.password",
            env_var: "PHONIX__DATABASE__PASSWORD",
        });
    }
    if cfg.redis.enabled && empty(&cfg.redis.password) {
        return Err(ConfigError::MissingSecret {
            secret: "redis.password",
            env_var: "PHONIX__REDIS__PASSWORD",
        });
    }
    if cfg.rabbitmq.enabled && empty(&cfg.rabbitmq.password) {
        return Err(ConfigError::MissingSecret {
            secret: "rabbitmq.password",
            env_var: "PHONIX__RABBITMQ__PASSWORD",
        });
    }
    if empty(&cfg.security.mfa.encryption_key) {
        return Err(ConfigError::MissingSecret {
            secret: "security.mfa.encryption_key",
            env_var: "PHONIX__SECURITY__MFA__ENCRYPTION_KEY",
        });
    }
    // Only when it is switched on. A production deployment that sends no mail
    // is unusual but coherent; one that is configured to send and cannot
    // authenticate is not.
    if cfg.smtp.enabled && empty(&cfg.smtp.password) {
        return Err(ConfigError::MissingSecret {
            secret: "smtp.password",
            env_var: "PHONIX__SMTP__PASSWORD",
        });
    }
    Ok(())
}

fn check_production_hardening(cfg: &AppConfig) -> Result<(), ConfigError> {
    // Checked here as well as in `check_production_secrets`: a key that is
    // present but the wrong length fails at the first enrolment, in the middle
    // of somebody's day, rather than at boot.
    if let Err(problem) = cfg.security.mfa.encryption_key_bytes() {
        return Err(ConfigError::invalid(format!(
            "security.mfa.encryption_key {problem} - supply 32 random bytes, \
             base64-encoded, through PHONIX__SECURITY__MFA__ENCRYPTION_KEY"
        )));
    }
    // The value committed to config/development.toml. Shipping it would mean
    // every deployment shares one key for every TOTP secret.
    if cfg.security.mfa.encryption_key.expose_secret().trim() == DEVELOPMENT_MFA_KEY {
        return Err(ConfigError::invalid(
            "security.mfa.encryption_key is the development key from \
             config/development.toml. Generate a fresh one and supply it through \
             PHONIX__SECURITY__MFA__ENCRYPTION_KEY",
        ));
    }

    if cfg.database.ssl_mode == SslMode::Disable {
        return Err(ConfigError::invalid(
            "database.ssl_mode must not be 'disable' in production",
        ));
    }
    // Auto-provisioning lets an unrecognised Host header create a database.
    if cfg.tenancy.auto_provision {
        return Err(ConfigError::invalid(
            "tenancy.auto_provision must be false in production - otherwise any \
             unrecognised Host header can create a database",
        ));
    }
    // A default tenant means a request with a wrong or missing Host silently
    // reads and writes another tenant's data.
    if cfg.tenancy.default_tenant().is_some() {
        return Err(ConfigError::invalid(
            "tenancy.default_tenant must be empty in production - a fallback tenant \
             would serve another tenant's data to an unresolvable host",
        ));
    }
    if cfg.telemetry.tracing.log_bodies {
        return Err(ConfigError::invalid(
            "telemetry.tracing.log_bodies must be false in production - request \
             bodies routinely contain credentials and personal data",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> PoolConfig {
        PoolConfig {
            max_connections: 5,
            min_connections: 1,
            acquire_timeout_secs: 10,
            idle_timeout_secs: 60,
            max_lifetime_secs: 600,
            test_before_acquire: true,
        }
    }

    #[test]
    fn rejects_min_above_max_connections() {
        let mut p = pool();
        p.min_connections = 99;
        assert!(check_pool("test", &p).is_err());
    }

    #[test]
    fn rejects_lifetime_shorter_than_idle_timeout() {
        let mut p = pool();
        p.idle_timeout_secs = 900;
        p.max_lifetime_secs = 300;
        assert!(check_pool("test", &p).is_err());
    }

    #[test]
    fn accepts_unbounded_lifetime() {
        let mut p = pool();
        p.max_lifetime_secs = 0;
        assert!(check_pool("test", &p).is_ok());
    }
}
