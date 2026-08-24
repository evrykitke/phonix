//! Typed mirror of `config/*.toml`.
//!
//! Every field here has a counterpart in `config/base.toml`. Adding a field
//! without a default means the process refuses to start until the TOML is
//! updated, which is the behaviour we want for anything load-bearing.

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub app: AppSection,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub rabbitmq: RabbitMqConfig,
    pub telemetry: TelemetryConfig,
    pub tenancy: TenancyConfig,
    pub security: SecurityConfig,
    pub smtp: SmtpConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppSection {
    pub name: String,
    /// Overwritten from `PHONIX_ENV` during load; the TOML value is advisory.
    pub environment: String,
    /// Where `<code>.json` translation files are read from at boot.
    ///
    /// Relative to the working directory. Defaulted rather than required,
    /// because English needs no files at all - it is compiled in - so a
    /// deployment that ships one language should not have to say so.
    #[serde(default = "default_locales_dir")]
    pub locales_dir: String,
    /// Where the footer of a public screen points.
    #[serde(default)]
    pub links: PublicLinks,
}

fn default_locales_dir() -> String {
    "locales".to_owned()
}

/// The handful of destinations a signed-out visitor might want.
///
/// Every one is optional and every one defaults to empty, which renders no
/// link at all. That is the point: these are pages this application does not
/// serve, so the alternative to configuring them is not a sensible default, it
/// is a footer full of links to a 404. A deployment that has a privacy policy
/// says where it is; one that has not says nothing.
///
/// Absolute URLs. They may well live on a marketing site that is not this
/// application - which is exactly why they are configuration and not routes.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PublicLinks {
    #[serde(default)]
    pub privacy: String,
    #[serde(default)]
    pub terms: String,
    #[serde(default)]
    pub support: String,
    /// The product or company site. Also what the footer wordmark points at.
    #[serde(default)]
    pub website: String,
}

impl PublicLinks {
    /// One link, or `None` when it is not configured.
    ///
    /// Trimmed, because a value that is spaces is a value somebody meant to
    /// remove - and a footer link with an empty `href` reloads the page.
    fn some(value: &str) -> Option<&str> {
        let value = value.trim();
        (!value.is_empty()).then_some(value)
    }

    pub fn privacy(&self) -> Option<&str> {
        Self::some(&self.privacy)
    }

    pub fn terms(&self) -> Option<&str> {
        Self::some(&self.terms)
    }

    pub fn support(&self) -> Option<&str> {
        Self::some(&self.support)
    }

    pub fn website(&self) -> Option<&str> {
        Self::some(&self.website)
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub base_domain: String,
    pub scheme: String,
    pub shutdown_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub body_limit_bytes: usize,
    pub compression: bool,
    pub cors: CorsConfig,
}

impl ServerConfig {
    /// `host:port`, ready for `TcpListener::bind`.
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// What every workspace address ends in, e.g. `.phonix.local:3000`.
    ///
    /// The signup form shows this beside the address box, so somebody typing
    /// `acme` can see the host they are about to get. It was a literal
    /// `".localhost:3000"` in the markup until this existed, which was correct
    /// on one developer's machine and wrong on the internet - the production
    /// screen promised every new customer a workspace at
    /// `acme.localhost:3000`.
    ///
    /// Built from the same two fields as [`Self::tenant_origin`], and by the
    /// same rule about the default port, so the label and the URL cannot
    /// disagree.
    pub fn workspace_suffix(&self) -> String {
        let default_port = match self.scheme.as_str() {
            "https" => 443,
            _ => 80,
        };
        if self.port == default_port {
            format!(".{}", self.base_domain)
        } else {
            format!(".{}:{}", self.base_domain, self.port)
        }
    }

    /// Absolute origin for a tenant, e.g. `http://acme.phonix.local:3000`.
    ///
    /// The port is omitted when it is the scheme's default, so production URLs
    /// come out as `https://acme.example.com` rather than `...:443`.
    pub fn tenant_origin(&self, slug: &str) -> String {
        let default_port = match self.scheme.as_str() {
            "https" => 443,
            _ => 80,
        };
        if self.port == default_port {
            format!("{}://{}.{}", self.scheme, slug, self.base_domain)
        } else {
            format!(
                "{}://{}.{}:{}",
                self.scheme, slug, self.base_domain, self.port
            )
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    pub enabled: bool,
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub allow_credentials: bool,
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretString,
    pub catalog_database: String,
    pub tenant_database_prefix: String,
    pub maintenance_database: String,
    pub ssl_mode: SslMode,
    pub application_name: String,
    pub migrate_on_start: bool,
    pub statement_timeout_secs: u64,
    pub catalog_pool: PoolConfig,
    pub tenant_pool: PoolConfig,
    pub tenant_registry: TenantRegistryConfig,
}

impl DatabaseConfig {
    /// Connection string with the password replaced, for logs and errors.
    ///
    /// There is no non-redacted counterpart on purpose: real connections are
    /// built field-by-field from `PgConnectOptions`, which sidesteps having to
    /// percent-encode passwords containing `@`, `:` or `/`.
    pub fn redacted_url(&self, database: &str) -> String {
        format!(
            "postgres://{}:***@{}:{}/{}",
            self.username, self.host, self.port, database
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,
    #[serde(default = "default_true")]
    pub test_before_acquire: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TenantRegistryConfig {
    /// Upper bound on simultaneously open tenant pools. Total Postgres
    /// connections are roughly `max_cached_pools * tenant_pool.max_connections`.
    pub max_cached_pools: u64,
    pub idle_evict_secs: u64,
    pub lookup_cache_ttl_secs: u64,
    pub lookup_cache_capacity: u64,
}

// ---------------------------------------------------------------------------
// Redis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub database: u8,
    pub username: String,
    pub password: SecretString,
    pub use_tls: bool,
    pub key_prefix: String,
    pub default_ttl_secs: u64,
    pub connect_timeout_secs: u64,
    pub response_timeout_secs: u64,
    /// When true, a cache failure degrades to a miss instead of failing the
    /// request. Correct for a cache; wrong for a session or lock store.
    pub fail_open: bool,
}

// ---------------------------------------------------------------------------
// RabbitMQ
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct RabbitMqConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub vhost: String,
    pub username: String,
    pub password: SecretString,
    pub use_tls: bool,
    pub connect_timeout_secs: u64,
    pub heartbeat_secs: u16,
    pub exchange: String,
    pub exchange_kind: String,
    pub dead_letter_exchange: String,
    pub dead_letter_queue: String,
    pub publisher_confirms: bool,
    pub publish_timeout_secs: u64,
    pub prefetch_count: u16,
    pub max_delivery_attempts: u32,
    pub retry_initial_backoff_ms: u64,
    pub retry_max_backoff_ms: u64,
    pub pool: AmqpPoolConfig,
    #[serde(default)]
    pub consumers: Vec<ConsumerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmqpPoolConfig {
    pub max_size: usize,
    pub create_timeout_secs: u64,
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConsumerConfig {
    pub name: String,
    pub queue: String,
    pub routing_keys: Vec<String>,
    #[serde(default = "default_true")]
    pub durable: bool,
    #[serde(default)]
    pub auto_ack: bool,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    pub level: String,
    #[serde(default)]
    pub directives: Vec<String>,
    pub console: ConsoleLogConfig,
    pub file: FileLogConfig,
    pub tracing: TracingConfig,
}

impl TelemetryConfig {
    /// Assemble the `EnvFilter` directive string: global level first, then the
    /// per-target overrides, which take precedence in `tracing-subscriber`.
    pub fn filter_directives(&self) -> String {
        let mut parts = Vec::with_capacity(self.directives.len() + 1);
        parts.push(self.level.clone());
        parts.extend(self.directives.iter().cloned());
        parts.join(",")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConsoleLogConfig {
    pub enabled: bool,
    pub format: LogFormat,
    pub ansi: bool,
    pub show_target: bool,
    #[serde(default)]
    pub show_thread_ids: bool,
    pub show_line_numbers: bool,
    pub show_span_events: SpanEvents,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileLogConfig {
    pub enabled: bool,
    pub directory: String,
    pub file_name_prefix: String,
    pub file_name_suffix: String,
    pub rotation: Rotation,
    pub format: LogFormat,
    #[serde(default)]
    pub ansi: bool,
    pub show_target: bool,
    pub show_line_numbers: bool,
    pub show_span_events: SpanEvents,
    /// Roll to a new file when the next line would push the current one past
    /// this. `0` disables the size cap.
    ///
    /// The cap `rotation` alone cannot give: a daily file has no size until the
    /// day is over, so one afternoon at `debug` is a gigabyte that nothing
    /// rotates until midnight.
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u64,
    /// Delete log files not modified within this many days. `0` keeps them for
    /// ever.
    ///
    /// An age, not a count - which is what `max_files` is, and why the two are
    /// both here. Once files roll on size, "keep fourteen files" could mean
    /// fourteen minutes.
    #[serde(default = "default_retention_days")]
    pub retention_days: u64,
    /// Keep at most this many files after the age sweep. 0 is unlimited.
    ///
    /// A backstop under `retention_days` rather than a replacement for it: a
    /// process logging hard enough to produce hundreds of files inside the
    /// retention window is one this should still bound.
    pub max_files: usize,
    #[serde(default = "default_true")]
    pub non_blocking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Compact,
    Full,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rotation {
    Minutely,
    Hourly,
    Daily,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanEvents {
    None,
    New,
    Enter,
    Exit,
    Close,
    Active,
    Full,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TracingConfig {
    pub http_spans: bool,
    pub request_id: bool,
    /// Logs request and response bodies. Off outside deep debugging: bodies
    /// routinely contain credentials and personal data.
    pub log_bodies: bool,
}

// ---------------------------------------------------------------------------
// Tenancy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TenancyConfig {
    pub strategy: TenantStrategy,
    pub base_domain: String,
    pub header_name: String,
    pub reserved_subdomains: Vec<String>,
    pub slug_pattern: String,
    /// Fallback when the host carries no subdomain. Empty means "no fallback".
    #[serde(default)]
    pub default_tenant: String,
    pub auto_provision: bool,
    pub unknown_tenant_status: u16,
}

impl TenancyConfig {
    pub fn is_reserved(&self, subdomain: &str) -> bool {
        self.reserved_subdomains
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(subdomain))
    }

    pub fn default_tenant(&self) -> Option<&str> {
        let trimmed = self.default_tenant.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TenantStrategy {
    Subdomain,
    Path,
    Header,
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/// Knobs for authentication.
///
/// The line drawn here: *cost* and *duration* are configuration, because they
/// depend on the hardware and the risk appetite of a deployment. *Policy* -
/// how long a password must be, what a valid email looks like - is not, because
/// the browser has to apply the same rules and cannot read this file. Policy
/// lives in `phonix_core::identity` and is compiled into both sides.
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    pub password: PasswordConfig,
    pub session: SessionConfig,
    pub lockout: LockoutConfig,
    pub signup: SignupConfig,
    pub mfa: MfaConfig,
    pub invitations: InvitationConfig,
    pub password_reset: PasswordResetConfig,
    /// Signing in with a Google account. Off unless configured.
    #[serde(default)]
    pub google: GoogleConfig,
    /// The policies a newly created workspace starts with.
    ///
    /// Only a *starting point*: once a workspace exists, its own row in
    /// `workspace_settings` is the authority and changing this file does not
    /// touch it. That is the whole difference between a deployment default and
    /// an organization's decision.
    #[serde(default)]
    pub workspace_defaults: WorkspaceDefaults,
    /// What an anonymous caller may ask of the public screens.
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

/// How much unauthenticated traffic one client may send.
///
/// Everything here is about the screens somebody reaches *before* they have a
/// session - see [`phonix_core::identity::is_public_path`]. Behind them sit an
/// Argon2 verification, an SMTP conversation and, in one case, the creation of
/// a whole Postgres database, none of which asks anybody who they are first.
///
/// # Three tiers, because the costs differ by orders of magnitude
///
/// A page view is cheap and people reload. A credential attempt costs a
/// deliberately slow hash and is the thing an attacker repeats. Creating a
/// workspace costs a database, a schema, a permission tree and a row in the
/// catalog, and no honest person does it twice in a minute.
///
/// One number for all three would have to be sized for the cheapest, which
/// leaves the expensive ones unprotected.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// Off means no counting at all - not counting-without-refusing.
    pub enabled: bool,

    /// Views of a public screen, and the small reads those screens make.
    pub page_requests: u32,
    pub page_window_secs: u64,

    /// Anything that presents a credential: a password, a six-digit code, an
    /// invitation token, an MFA answer.
    pub action_requests: u32,
    pub action_window_secs: u64,

    /// Creating a workspace. Measured in hours, because each one is a database.
    pub signup_requests: u32,
    pub signup_window_secs: u64,

    /// Which header carries the client's address, or empty for the socket.
    ///
    /// **Getting this wrong makes the whole limiter decorative.** A limiter
    /// keyed on something the caller chooses is not a limiter: the caller sends
    /// a different value each time and gets a fresh allowance with it.
    ///
    /// Empty - the default - keys on the peer address of the TCP connection,
    /// which nobody can forge but which is the proxy's address when there is a
    /// proxy in front. Name a header only when *every* request reaching this
    /// process has passed through something that overwrites it:
    ///
    /// * `x-real-ip` behind nginx, which sets it from `$remote_addr`.
    /// * `cf-connecting-ip` behind Cloudflare - and only if the origin refuses
    ///   connections that did not come from Cloudflare, because otherwise
    ///   anybody who reaches the origin directly writes their own key.
    ///
    /// Never `x-forwarded-for`: nginx *appends* to what the client sent, so its
    /// first entry is attacker-controlled by construction.
    #[serde(default)]
    pub client_ip_header: String,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            page_requests: 120,
            page_window_secs: 60,
            action_requests: 12,
            action_window_secs: 60,
            signup_requests: 3,
            signup_window_secs: 3600,
            client_ip_header: String::new(),
        }
    }
}

impl RateLimitConfig {
    /// The header to read the client address from, lowercased, or `None`.
    ///
    /// `None` means the socket address, which is the safe answer and the
    /// default. A header of whitespace is the same as none rather than a
    /// header nothing ever sets - a limiter that silently keys every request
    /// to the same empty value would refuse the whole internet as one client.
    pub fn ip_header(&self) -> Option<String> {
        let name = self.client_ip_header.trim().to_ascii_lowercase();
        (!name.is_empty()).then_some(name)
    }
}

/// Signing in with a Google account.
///
/// `Default` is "off with nothing filled in", so a deployment that has never
/// heard of this starts up unchanged and the button does not appear.
///
/// # `redirect_uri` is load-bearing twice over
///
/// Google compares it byte for byte against a URI registered in the Cloud
/// console and refuses the exchange on any difference - trailing slash,
/// `http` for `https`, a different port. It cannot contain a wildcard, which
/// is the whole reason this flow runs where it does: workspaces live on
/// `*.example.com` and no single registered URI can cover them, so **one fixed
/// host** starts and finishes every sign-in and hands the session to the
/// workspace afterwards through the existing handoff token.
///
/// Its origin is also where that host *is*. Nothing else in the configuration
/// names the signup host - `server.host` is a bind address and
/// `tenancy.base_domain` is one label above it - so the button on a workspace
/// page is built from this URI's scheme and authority. Changing it moves both
/// halves at once, which is the property worth having.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GoogleConfig {
    /// When false, nothing is registered, the button does not render, and the
    /// endpoints answer as though they did not exist.
    pub enabled: bool,
    /// The OAuth client id from the Google Cloud console. Public by design -
    /// it travels in a URL the browser follows.
    pub client_id: String,
    /// The client secret. **Never in a committed file**: production reads
    /// `PHONIX__SECURITY__GOOGLE__CLIENT_SECRET` from the environment.
    pub client_secret: SecretString,
    /// Absolute URL Google sends the browser back to. Must match a registered
    /// redirect URI exactly. See the note above.
    pub redirect_uri: String,
    /// Restrict sign-in to one Google Workspace domain, e.g. `acme.com`.
    ///
    /// `None` or empty means any Google account, which includes every personal
    /// `@gmail.com` address. Set for a deployment where every member is in one
    /// company - it narrows the account picker and is checked against the
    /// token's own `hd` claim afterwards.
    pub hosted_domain: Option<String>,
}

impl GoogleConfig {
    /// Whether this is filled in well enough to attempt a sign-in.
    ///
    /// `enabled` alone is not it: a deployment that turned this on and left the
    /// client id blank would render a button that leads to a Google error page,
    /// which is worse than no button.
    pub fn is_usable(&self) -> bool {
        self.enabled
            && !self.client_id.trim().is_empty()
            && !self.client_secret.expose_secret().trim().is_empty()
            && !self.redirect_uri.trim().is_empty()
    }

    /// The origin of [`Self::redirect_uri`] - scheme and authority, no path.
    ///
    /// This is the host that starts and finishes every Google sign-in. Derived
    /// rather than configured separately so the two cannot disagree: a start
    /// URL on one host and a callback registered on another is a flow that
    /// fails at the last step, after the person has already consented.
    ///
    /// `None` when the URI will not parse that far, which
    /// [`Self::is_usable`] does not catch - it checks that something is set,
    /// not that it is a URL.
    pub fn auth_origin(&self) -> Option<String> {
        let uri = self.redirect_uri.trim();
        let (scheme, rest) = uri.split_once("://")?;

        if scheme.is_empty() {
            return None;
        }

        let authority = rest.split('/').next()?;
        if authority.is_empty() {
            return None;
        }

        Some(format!("{scheme}://{authority}"))
    }
}

/// Self-service password reset: whether, how long, and how many guesses.
///
/// The three numbers are one decision, not three. A six-digit code is a
/// million values, and none of the entropy is what keeps it safe - the TTL and
/// the attempt limit are. Loosening either without looking at the other is how
/// a code that was fine becomes guessable.
#[derive(Debug, Clone, Deserialize)]
pub struct PasswordResetConfig {
    /// When false the endpoints refuse and the link is not offered. For a
    /// deployment where accounts are managed by an administrator and a reset
    /// should go through them.
    pub enabled: bool,
    /// Minutes before the code stops working.
    ///
    /// Short, and deliberately unlike [`InvitationConfig::ttl_hours`]: the
    /// person who asked for this is sitting at the screen waiting for it. Every
    /// extra minute is more time an unattended mailbox is a way in.
    pub code_ttl_mins: u64,
    /// Wrong codes before the code is destroyed and a new one has to be
    /// requested.
    ///
    /// This is the control. Six digits is a million guesses at an endpoint that
    /// would otherwise answer forever, and each one costs a SHA-256 rather than
    /// an Argon2 hash - nothing about the arithmetic makes guessing expensive,
    /// so the limit is what makes it finite. The same bargain
    /// [`MfaConfig::max_challenge_attempts`] strikes.
    pub max_attempts: i16,
}

impl PasswordResetConfig {
    /// The TTL as seconds, which is what the token store takes.
    pub const fn ttl_secs(&self) -> i64 {
        self.code_ttl_mins as i64 * 60
    }
}

/// How long somebody has to accept an invitation.
#[derive(Debug, Clone, Deserialize)]
pub struct InvitationConfig {
    /// Hours before the link stops working.
    ///
    /// Long enough to survive a weekend and a holiday Monday, short enough that
    /// a link forwarded into a mailing list archive is not a standing way in.
    /// Days rather than minutes because the recipient is not sitting at a
    /// screen waiting for it, which is what separates this from a reset link.
    pub ttl_hours: u64,
}

impl InvitationConfig {
    /// The TTL as seconds, which is what the token store takes.
    pub const fn ttl_secs(&self) -> i64 {
        (self.ttl_hours as i64).saturating_mul(3600)
    }
}

/// Argon2id work factors.
///
/// Defaults follow the OWASP Password Storage Cheat Sheet's first recommended
/// configuration (19 MiB, 2 iterations, 1 lane). Raise `memory_kib` before
/// raising `iterations`: memory hardness is what makes GPU and ASIC attacks
/// expensive, and iterations only cost the defender.
#[derive(Debug, Clone, Deserialize)]
pub struct PasswordConfig {
    /// Memory cost in kibibytes. This is allocated *per concurrent hash*, so
    /// 19 MiB and 20 simultaneous sign-ins is 380 MiB of transient memory.
    pub memory_kib: u32,
    /// Time cost: passes over the memory block.
    pub iterations: u32,
    /// Lanes. Kept at 1 unless a dedicated thread pool is sized for more -
    /// higher parallelism with the same total work weakens the hash.
    pub parallelism: u32,
    /// Bound on how long one hash may take before it is logged as a warning.
    /// A hash that has drifted far past this is a sign the box is overloaded.
    pub warn_above_ms: u64,
}

/// Session cookie and lifetime settings.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub cookie_name: String,
    /// Sign-out after this long without a request. Slides forward on activity.
    pub idle_timeout_mins: u64,
    /// Hard ceiling that activity cannot extend. A session that is kept warm by
    /// a background tab still ends here.
    pub absolute_timeout_hours: u64,
    /// Same as `absolute_timeout_hours` but for "remember me" sign-ins.
    pub remember_me_days: u64,
    /// `Secure` attribute. Must be true wherever the site is served over HTTPS;
    /// production validation refuses to start without it.
    pub secure: bool,
    /// `SameSite`. `lax` is right for a cookie that must survive a top-level
    /// navigation from an email link but not a cross-site POST.
    pub same_site: SameSitePolicy,
    /// How long the one-time token that moves a new session from the signup
    /// host to the workspace host stays valid. Seconds, because it is redeemed
    /// by an immediate redirect.
    pub handoff_ttl_secs: u64,
    /// Delete expired rows this often. 0 disables the sweeper.
    pub purge_interval_mins: u64,
}

impl SessionConfig {
    /// The cookie name for one tenant.
    ///
    /// Suffixed with the slug so that signing in to a second workspace in the
    /// same browser does not evict the first. Both cookies are host-only, so
    /// neither is ever sent to the other workspace; the suffix only prevents
    /// them from overwriting each other when a parent-domain cookie is present.
    pub fn cookie_name_for(&self, slug: &str) -> String {
        format!("{}_{}", self.cookie_name, slug.replace('-', "_"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SameSitePolicy {
    Strict,
    Lax,
    None,
}

impl SameSitePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

/// Online brute-force defence.
#[derive(Debug, Clone, Deserialize)]
pub struct LockoutConfig {
    /// Consecutive failures before the account is locked. 0 disables lockout.
    ///
    /// A lockout is itself a denial-of-service vector - anyone who knows an
    /// address can lock it - so this is deliberately generous and temporary
    /// rather than tight and permanent.
    pub max_failed_attempts: i32,
    /// How long the lock lasts. It expires on its own; no admin action needed.
    pub lockout_mins: u64,
}

/// Self-service onboarding.
#[derive(Debug, Clone, Deserialize)]
pub struct SignupConfig {
    /// When false, `/signup` returns `SignupResult::Closed` and no workspace
    /// can be created through the public form.
    pub enabled: bool,
    /// Require a verified email before the workspace becomes usable. Needs
    /// SMTP, so it stays false until mail delivery exists.
    pub require_email_verification: bool,
    /// Refuse addresses at these domains. Intended for disposable-mail
    /// providers; matched on the exact domain, case-insensitively.
    #[serde(default)]
    pub blocked_email_domains: Vec<String>,
}

impl SignupConfig {
    pub fn is_blocked_domain(&self, email: &str) -> bool {
        let Some(domain) = email.rsplit('@').next() else {
            return false;
        };
        self.blocked_email_domains
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(domain))
    }
}

/// The policies a new workspace is seeded with.
///
/// Deserialised straight into the shared `phonix_core` types, so the TOML, the
/// database row and the settings form are all the same shape. Every field has a
/// default, which means an operator who omits the whole section gets the
/// compiled system defaults rather than a startup failure.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceDefaults {
    pub password: phonix_core::identity::PasswordPolicy,
    pub mfa: phonix_core::identity::MfaPolicy,
    /// What a new workspace records about itself, and for how long.
    ///
    /// A seed like the other two, not a ceiling: an operator running a large
    /// deployment can start every workspace with a retention rather than with
    /// "keep forever", and each workspace can still change it afterwards.
    pub audit: phonix_core::audit::AuditPolicy,
}

impl WorkspaceDefaults {
    /// As one value, for seeding a workspace at creation.
    pub fn as_settings(&self) -> phonix_core::WorkspaceSecuritySettings {
        phonix_core::WorkspaceSecuritySettings {
            password: self.password.clone(),
            mfa: self.mfa.clone(),
            audit: self.audit.clone(),
        }
    }
}

/// Multi-factor authentication parameters.
///
/// These are the deployment's, not the organization's. An organization decides
/// *whether* its users need a second factor (`workspace_settings.mfa_*`); the
/// digits, the step and the acceptance window are a security claim that must
/// hold identically for every workspace on the deployment.
#[derive(Debug, Clone, Deserialize)]
pub struct MfaConfig {
    /// Name the authenticator app shows above the code. Usually the product
    /// name; the workspace slug is appended per user, so people with accounts
    /// in two workspaces can tell the entries apart.
    pub issuer: String,

    /// Digits in a TOTP code. Six is what every authenticator app assumes.
    pub totp_digits: u8,

    /// Seconds each code is valid for. RFC 6238 recommends 30, and apps that
    /// let the user pick still default to it.
    pub totp_step_secs: u64,

    /// How many steps either side of "now" are accepted, for clock drift.
    ///
    /// Each step widens the window a code stays usable in: at 30 seconds and
    /// skew 1, a code lives for 90 seconds rather than 30. That is the trade -
    /// too tight and users with a slow phone clock cannot sign in, too loose
    /// and a shoulder-surfed code stays good for minutes.
    pub totp_skew_steps: u8,

    /// Bytes of shared secret. RFC 4226 requires at least 16 and recommends 20,
    /// which is also what base32-encodes to the 32 characters apps expect.
    pub secret_bytes: usize,

    /// How many recovery codes are issued at a time.
    pub recovery_code_count: usize,

    /// Wrong codes allowed before the half-authenticated session is thrown away
    /// and the password has to be entered again.
    ///
    /// Without a cap, a six-digit code is a million guesses that never expire -
    /// and each one is a cheap HMAC, not an Argon2 hash.
    pub max_challenge_attempts: u32,

    /// How long a half-authenticated session may sit at the challenge screen.
    pub challenge_ttl_mins: u64,

    /// Key that encrypts stored TOTP secrets: 32 bytes, base64.
    ///
    /// Unlike a password, a TOTP secret cannot be hashed - the server has to
    /// reproduce the code, so it needs the secret back. Encrypting it means a
    /// stolen database dump is not by itself every user's authenticator app;
    /// the key lives in the environment, not in the database.
    ///
    /// Rotating it invalidates every enrolment, so it is not a routine act.
    pub encryption_key: SecretString,
}

impl MfaConfig {
    /// The key as raw bytes.
    ///
    /// Returns `Err` with a description rather than the value - this runs at
    /// startup, and the message goes into the log.
    pub fn encryption_key_bytes(&self) -> Result<[u8; 32], &'static str> {
        use base64::Engine as _;

        let raw = self.encryption_key.expose_secret().trim();
        if raw.is_empty() {
            return Err("is empty");
        }

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|_| "is not valid base64")?;

        decoded
            .try_into()
            .map_err(|_| "must decode to exactly 32 bytes")
    }
}

// ---------------------------------------------------------------------------
// Mail
// ---------------------------------------------------------------------------

/// The system-default relay.
///
/// Every workspace that has not set its own credentials sends through this one.
/// A workspace that wants mail from its own domain overrides it in its settings
/// panel, and the override is stored in that tenant's own database - see
/// `phonix_services::mail`, which is the single place the two are resolved
/// against each other.
///
/// The password is a [`SecretString`], so it is redacted in `Debug` and in
/// logs, and it belongs in `.env` (`PHONIX__SMTP__PASSWORD`) rather than in any
/// committed file - the same rule as the database and Redis passwords.
#[derive(Debug, Clone, Deserialize)]
pub struct SmtpConfig {
    /// Whether mail is sent at all.
    ///
    /// Off leaves the rest of the system working: an invitation is still
    /// created and its link still exists, it is simply not delivered. That is
    /// what makes a machine with no relay credentials usable, and it is why
    /// nothing downstream treats "no mailer" as a failure to boot.
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretString,
    /// The address mail is from. Not the same as the username: a relay often
    /// authenticates as one identity and sends as another.
    pub from_address: String,
    pub from_name: String,
    /// Where a reply should go, when that is not the sending address.
    #[serde(default)]
    pub reply_to: Option<String>,
    pub encryption: SmtpEncryption,
    pub timeout_secs: u64,
}

/// How the connection to the relay is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpEncryption {
    /// Connect in clear, then upgrade with STARTTLS. What port 587 and the
    /// Mailtrap sandbox on 2525 expect.
    StartTls,
    /// TLS from the first byte. What port 465 expects.
    Implicit,
    /// No TLS at all. For a relay on localhost and for nothing else - the
    /// password crosses the wire in clear.
    None,
}

impl SmtpConfig {
    /// Whether this configuration could actually send.
    ///
    /// Enabled is not the same as usable: a host or a sending address left
    /// empty is a relay that will fail on the first message rather than at
    /// boot, and the invitation screen would rather say so up front.
    pub fn is_usable(&self) -> bool {
        self.enabled && !self.host.trim().is_empty() && !self.from_address.trim().is_empty()
    }
}

fn default_true() -> bool {
    true
}

fn default_concurrency() -> usize {
    1
}

fn default_max_file_size_mb() -> u64 {
    10
}

fn default_retention_days() -> u64 {
    3
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Where uploaded files live, and how the worker that verifies them behaves.
///
/// Note what this section does *not* contain: which types may be uploaded, and
/// how large each kind of file may be. Those are policy the code enforces, and
/// they live in `phonix_core::files` so that the browser bundle carries the
/// same table - a screen that offered what the server refuses would be a form
/// that fails after the upload rather than before it.
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    /// Where objects go. Relative paths resolve against the workspace root; see
    /// [`StorageConfig::resolved_root`].
    pub root: String,
    pub naming: NamingStrategyKind,
    /// The ceiling the HTTP layer refuses past, before a byte is written.
    ///
    /// A second, coarser limit above the per-bucket ones. It exists because the
    /// bucket is not known until the multipart body has been parsed far enough
    /// to read it, and a request has to be refusable before that.
    pub max_upload_bytes: u64,
    /// How long an upload request may take.
    ///
    /// Its own clock, because 25 MB does not arrive inside the timeout a page
    /// render is judged by.
    pub upload_timeout_secs: u64,
    /// How long unverified bytes may sit before the sweeper removes them.
    pub quarantine_ttl_mins: u64,
    pub jobs: UploadJobsConfig,
}

impl StorageConfig {
    /// The storage root as a concrete path.
    ///
    /// Resolved exactly like the log directory: absolute is used verbatim,
    /// relative is anchored to the workspace root rather than to the process
    /// working directory - otherwise `cargo leptos watch` and a packaged
    /// binary would write uploads into two different places, and the second
    /// one would appear to have lost every file.
    pub fn resolved_root(&self) -> std::path::PathBuf {
        let path = std::path::PathBuf::from(&self.root);

        if path.is_absolute() {
            path
        } else {
            crate::workspace_root().join(path)
        }
    }
}

/// Which implementation of `phonix_storage::FileStorage` is in use.
///
/// One variant so far. It is an enum rather than a bool because the second one -
/// an object store - is a matter of when rather than whether, and a
/// `use_local_disk = true` flag would have to be replaced rather than extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    /// A directory this process can write to.
    Local,
}

/// How stored files are laid out beneath the tenant.
///
/// Mirrors the implementations in `phonix_storage::naming`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamingStrategyKind {
    /// `bucket/YYYY/MM/<uuid>.<ext>` - the default.
    DateSharded,
    /// `bucket/ab/cd/<sha256>.<ext>` - identical bytes share one object.
    ContentAddressed,
    /// `bucket/<uuid>.<ext>` - right for an object store, which has no
    /// directories to shard.
    Flat,
}

/// The worker that turns a received upload into a stored file.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadJobsConfig {
    /// Off leaves uploads stuck at "queued" for ever, which is why it is on by
    /// default. Worth turning off in a process that serves pages and has a
    /// sibling process doing the work.
    pub enabled: bool,
    /// Uploads verified at once per sweep.
    pub concurrency: usize,
    /// The safety net, not the usual path: a job is dispatched as soon as its
    /// bytes land, and this catches the ones whose process died first.
    pub poll_interval_secs: u64,
    /// Attempts before an upload is given up on.
    pub max_attempts: u32,
    /// A job claimed and unfinished for this long is offered to another worker.
    pub claim_timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(scheme: &str, port: u16) -> ServerConfig {
        ServerConfig {
            host: "0.0.0.0".to_owned(),
            port,
            base_domain: "example.com".to_owned(),
            scheme: scheme.to_owned(),
            shutdown_timeout_secs: 30,
            request_timeout_secs: 30,
            body_limit_bytes: 2_097_152,
            compression: true,
            cors: CorsConfig {
                enabled: false,
                allowed_origins: Vec::new(),
                allow_credentials: false,
            },
        }
    }

    #[test]
    fn a_workspace_suffix_hides_the_default_port() {
        // What the signup form prints next to the address box. A visible
        // `:443` there would be the first thing a new customer saw and the
        // first thing they asked about.
        assert_eq!(server("https", 443).workspace_suffix(), ".example.com");
        assert_eq!(server("http", 80).workspace_suffix(), ".example.com");
    }

    #[test]
    fn a_workspace_suffix_keeps_a_port_that_is_not_the_default() {
        // Development, where omitting it would print an address that does not
        // resolve.
        assert_eq!(server("http", 3000).workspace_suffix(), ".example.com:3000");
    }

    #[test]
    fn the_suffix_and_the_origin_agree() {
        // The label beside the box and the URL somebody is actually sent to
        // are built from the same fields; this is what stops them drifting.
        for (scheme, port) in [("https", 443), ("http", 80), ("http", 3000)] {
            let config = server(scheme, port);
            let origin = config.tenant_origin("acme");
            let suffix = config.workspace_suffix();

            assert!(
                origin.ends_with(&format!("acme{suffix}")),
                "{origin} does not end in acme{suffix}"
            );
        }
    }

    #[test]
    fn an_unconfigured_link_is_absent_rather_than_empty() {
        // An empty href reloads the page, which is the worst possible
        // behaviour for something labelled "Privacy".
        let links = PublicLinks {
            privacy: "https://example.com/privacy".to_owned(),
            terms: "   ".to_owned(),
            support: String::new(),
            website: String::new(),
        };

        assert_eq!(links.privacy(), Some("https://example.com/privacy"));
        assert_eq!(links.terms(), None);
        assert_eq!(links.support(), None);
        assert_eq!(links.website(), None);
    }

    #[test]
    fn no_header_named_means_the_socket_address() {
        let config = RateLimitConfig::default();
        assert_eq!(config.ip_header(), None);
    }

    #[test]
    fn a_named_header_is_matched_case_insensitively() {
        // HTTP header names are case-insensitive, and a config file written
        // with the capitals people use in documentation must still match.
        let config = RateLimitConfig {
            client_ip_header: "  X-Real-IP  ".to_owned(),
            ..RateLimitConfig::default()
        };
        assert_eq!(config.ip_header().as_deref(), Some("x-real-ip"));
    }

    #[test]
    fn a_header_of_whitespace_is_the_same_as_none() {
        // Otherwise every request keys to one empty value and the entire
        // internet shares a single allowance.
        let config = RateLimitConfig {
            client_ip_header: "   ".to_owned(),
            ..RateLimitConfig::default()
        };
        assert_eq!(config.ip_header(), None);
    }
}
