//! What one anonymous caller may ask for, and how often.
//!
//! Everything the public screens do runs before anybody has said who they are.
//! Signing in spends an Argon2 verification that is *deliberately* slow - 200
//! to 500 ms on the production box, by `[security.password]`. Asking for a
//! password reset opens an SMTP conversation. Creating a workspace creates a
//! Postgres database, runs its migrations, writes a permission tree and
//! inserts a catalog row.
//!
//! None of those asks who is calling first, so this does.
//!
//! # The key is the whole design
//!
//! A rate limiter keyed on something the caller controls is not a rate limiter.
//! Send a different value each time and each request arrives as a brand new
//! client with a brand new allowance - the counting still happens, it simply
//! never counts the same thing twice.
//!
//! That is not hypothetical here. The audit-trail code reads the **first**
//! entry of `X-Forwarded-For`, and nginx is configured with
//! `$proxy_add_x_forwarded_for`, which *appends* the real address to whatever
//! the client sent. So `X-Forwarded-For: <anything>` puts an attacker-chosen
//! value in first place on every request. Good enough for a log line a person
//! reads with judgement; useless as a key.
//!
//! So this reads exactly one source, named by
//! `security.rate_limit.client_ip_header`, and nothing else. Empty means the
//! peer address of the TCP connection - unforgeable, and correct whenever this
//! process is reachable directly. A header is right only when every request has
//! passed through something that overwrites it. See `config/base.toml`.
//!
//! # Fixed windows, in memory
//!
//! A counter and an expiry per key, and the counter resets when the window
//! does. Not a sliding window and not a token bucket: both are better shaped,
//! and neither is worth the arithmetic against an attacker whose actual budget
//! is "one workspace an hour instead of thousands".
//!
//! In memory rather than Redis, and that is a choice with a consequence. Redis
//! would survive a restart and be shared between nodes; there is one node, and
//! [`phonix_cache`] is documented as fail-open - which for a limiter means it
//! disappears exactly when something is going wrong. A process restart resets
//! every window, which is a real hole. It is a smaller one than "the limiter
//! stops existing when Redis hiccups".

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use phonix_config::{AppConfig, RateLimitConfig};

/// How expensive the thing behind a request is.
///
/// The variants are ordered by cost, and each carries its own allowance. A
/// request that is none of them is not limited here at all - see [`classify`]
/// for what that covers and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Viewing a public screen, and the small reads those screens make.
    Page,
    /// Presenting a credential: a password, a code, a token, an MFA answer.
    Action,
    /// Creating a workspace - a database, a schema and a permission tree.
    Signup,
}

impl Tier {
    /// This tier's allowance, from configuration.
    fn allowance(self, config: &RateLimitConfig) -> (u32, Duration) {
        match self {
            Self::Page => (
                config.page_requests,
                Duration::from_secs(config.page_window_secs),
            ),
            Self::Action => (
                config.action_requests,
                Duration::from_secs(config.action_window_secs),
            ),
            Self::Signup => (
                config.signup_requests,
                Duration::from_secs(config.signup_window_secs),
            ),
        }
    }

    /// For the log line, so a refusal says which allowance ran out.
    fn name(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Action => "action",
            Self::Signup => "signup",
        }
    }
}

/// Which tier a request belongs to, or `None` to let it past uncounted.
///
/// # What is deliberately not counted
///
/// **Static assets.** One page load pulls a stylesheet, a JavaScript shim and a
/// multi-megabyte wasm bundle, so counting them would exhaust a person's page
/// allowance before their first click. In production nginx serves these and
/// they never reach this process; in development they do.
///
/// **Authenticated routes.** They have a better control than a counter:
/// `Caller::require` refuses on the caller's actual permissions. A limiter here
/// would add a second, blunter answer to a question already answered well.
///
/// **Health probes.** An orchestrator polls them on a schedule and must never
/// be told to come back later.
pub fn classify(method: &Method, path: &str) -> Option<Tier> {
    // Assets first: cheapest test, largest volume.
    if path.starts_with("/pkg/") || path.starts_with("/health/") || looks_like_a_file(path) {
        return None;
    }

    match path {
        // One workspace is one database. Its own tier, measured in hours.
        "/api/create-workspace" => return Some(Tier::Signup),

        // A credential is being presented. Each of these ends in a comparison
        // meant to be slow, or in an email being sent.
        "/api/sign-in"
        | "/api/mfa-challenge"
        | "/api/password-reset/request"
        | "/api/password-reset/complete"
        | "/api/invitations/accept"
        | "/auth/handoff"
        | "/auth/google/start"
        | "/auth/google/callback" => return Some(Tier::Action),

        // Cheap reads the public screens make while somebody types. Counted as
        // page traffic - `workspace-address` in particular answers "does this
        // workspace exist", which is worth a ceiling even though it is cheap.
        "/api/tenant"
        | "/api/current-user"
        | "/api/google-sign-in-url"
        | "/api/workspace-address" => return Some(Tier::Page),

        _ => {}
    }

    // Anything else under /api/ is a signed-in call, guarded by `Caller`.
    if path.starts_with("/api/") {
        return None;
    }

    // A public screen being viewed. GET and HEAD only: a POST to one of these
    // paths is not a page view, and no form does it.
    if matches!(*method, Method::GET | Method::HEAD)
        && phonix_core::identity::is_signed_out_chrome(path)
    {
        return Some(Tier::Page);
    }

    None
}

/// Whether the last path segment carries an extension.
///
/// Crude, and it only has to be: it runs after `/pkg/` has already caught the
/// bundle, and it exists for the loose files - `favicon.ico`, `robots.txt`, an
/// image under `public/`. No application route has a dot in its last segment.
fn looks_like_a_file(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
}

/// One key's counter and when it resets.
#[derive(Debug, Clone, Copy)]
struct Window {
    count: u32,
    resets_at: Instant,
}

/// The counters.
///
/// A single `Mutex<HashMap>`, held for the few instructions it takes to bump an
/// integer. Sharding it would matter under contention a limiter this coarse
/// will not see, and an uncontended mutex costs a couple of nanoseconds.
pub struct Limiter {
    windows: Mutex<HashMap<(Tier, String), Window>>,
    /// Number of entries past which a sweep runs on the next decision.
    ///
    /// Without this the map grows for as long as the process lives, one entry
    /// per address that ever arrived - a slow memory leak with an
    /// attacker-controlled rate.
    sweep_above: usize,
}

/// What a decision came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Refused, with how long until the window resets.
    Refuse {
        retry_after_secs: u64,
    },
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            sweep_above: 4_096,
        }
    }

    /// Count one request and say whether it may proceed.
    ///
    /// `now` is a parameter so the tests can move time without sleeping.
    pub fn check_at(
        &self,
        tier: Tier,
        key: &str,
        config: &RateLimitConfig,
        now: Instant,
    ) -> Decision {
        let (limit, window) = tier.allowance(config);

        // A limit of zero would refuse everybody for ever, including whoever is
        // trying to reach the screen that fixes it. Read as "not limited",
        // which is what somebody typing 0 into a config file meant.
        if limit == 0 {
            return Decision::Allow;
        }

        let mut windows = match self.windows.lock() {
            Ok(guard) => guard,
            // A panic while another thread held the lock. The counts are just
            // integers - nothing is half-written - so the sane answer is to
            // keep limiting rather than to fail open or to panic in turn.
            Err(poisoned) => poisoned.into_inner(),
        };

        if windows.len() > self.sweep_above {
            windows.retain(|_, entry| entry.resets_at > now);
        }

        let entry = windows.entry((tier, key.to_owned())).or_insert(Window {
            count: 0,
            resets_at: now + window,
        });

        // Expired: this is the first request of a new window, not the next of
        // an old one.
        if entry.resets_at <= now {
            *entry = Window {
                count: 0,
                resets_at: now + window,
            };
        }

        if entry.count >= limit {
            let remaining = entry.resets_at.saturating_duration_since(now);
            return Decision::Refuse {
                // Never zero: `Retry-After: 0` invites an immediate retry that
                // is certain to be refused again.
                retry_after_secs: remaining.as_secs().max(1),
            };
        }

        entry.count += 1;
        Decision::Allow
    }

    /// [`Self::check_at`] against the clock.
    pub fn check(&self, tier: Tier, key: &str, config: &RateLimitConfig) -> Decision {
        self.check_at(tier, key, config, Instant::now())
    }
}

/// The limiter and the numbers it reads, as middleware state.
///
/// Its own state type rather than a field on `AppState`: the counters belong to
/// this process and to this file, and nothing behind a server function has any
/// business reaching them. Keeping them out of `AppState` also keeps
/// [`Limiter`] in the server crate, where it can stay private to the middleware
/// that owns it.
#[derive(Clone)]
pub struct RateLimitState {
    pub config: Arc<AppConfig>,
    pub limiter: Arc<Limiter>,
}

/// Refuse anonymous traffic that is asking for too much.
pub async fn enforce(
    State(state): State<RateLimitState>,
    request: Request,
    next: Next,
) -> Response {
    let config = &state.config.security.rate_limit;

    if !config.enabled {
        return next.run(request).await;
    }

    let Some(tier) = classify(request.method(), request.uri().path()) else {
        return next.run(request).await;
    };

    let key = client_key(config, request.headers(), &request);

    match state.limiter.check(tier, &key, config) {
        Decision::Allow => next.run(request).await,
        Decision::Refuse { retry_after_secs } => {
            // At warn: an anonymous client hitting a ceiling is either an
            // attack or a bug in this application, and both are worth seeing.
            tracing::warn!(
                tier = tier.name(),
                client = %key,
                path = %request.uri().path(),
                "rate limit exceeded"
            );

            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                // Plain text, not a rendered page: rendering one would spend
                // more of this server than the request being refused.
                "Too many requests. Try again shortly.",
            )
                .into_response()
        }
    }
}

/// What to count this request against.
///
/// One source, chosen by configuration, and no fallback chain between headers -
/// a chain is a bypass, because the caller picks which link answers by omitting
/// the ones above it.
///
/// The socket address *is* used when the configured header is absent, and that
/// is not the same thing: it is the one value the caller cannot choose, so
/// falling back to it can only narrow an allowance, never widen one.
fn client_key(config: &RateLimitConfig, headers: &HeaderMap, request: &Request) -> String {
    if let Some(name) = config.ip_header()
        && let Some(value) = headers.get(&name).and_then(|value| value.to_str().ok())
    {
        let value = value.trim();
        if !value.is_empty() {
            // Bounded: this becomes a map key, and an unbounded header should
            // not decide how much memory one request costs.
            return value.chars().take(64).collect();
        }
    }

    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| peer_key(addr.ip()))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The address of the peer, with the port dropped.
///
/// The port changes on every connection, so keying on it would give each
/// request its own allowance and count nothing.
fn peer_key(ip: IpAddr) -> String {
    ip.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RateLimitConfig {
        RateLimitConfig {
            enabled: true,
            page_requests: 3,
            page_window_secs: 60,
            action_requests: 2,
            action_window_secs: 60,
            signup_requests: 1,
            signup_window_secs: 3600,
            client_ip_header: String::new(),
        }
    }

    #[test]
    fn a_client_is_allowed_up_to_the_limit_and_then_refused() {
        let limiter = Limiter::new();
        let config = config();
        let now = Instant::now();

        for attempt in 1..=3 {
            assert_eq!(
                limiter.check_at(Tier::Page, "1.2.3.4", &config, now),
                Decision::Allow,
                "request {attempt} should be allowed"
            );
        }

        assert!(matches!(
            limiter.check_at(Tier::Page, "1.2.3.4", &config, now),
            Decision::Refuse { .. }
        ));
    }

    #[test]
    fn one_clients_exhaustion_does_not_touch_another() {
        let limiter = Limiter::new();
        let config = config();
        let now = Instant::now();

        for _ in 0..4 {
            limiter.check_at(Tier::Page, "1.2.3.4", &config, now);
        }

        assert_eq!(
            limiter.check_at(Tier::Page, "5.6.7.8", &config, now),
            Decision::Allow
        );
    }

    #[test]
    fn the_tiers_have_separate_allowances() {
        // Signing in until the action allowance is gone must not cost somebody
        // the ability to load the page and read why.
        let limiter = Limiter::new();
        let config = config();
        let now = Instant::now();

        for _ in 0..5 {
            limiter.check_at(Tier::Action, "1.2.3.4", &config, now);
        }

        assert!(matches!(
            limiter.check_at(Tier::Action, "1.2.3.4", &config, now),
            Decision::Refuse { .. }
        ));
        assert_eq!(
            limiter.check_at(Tier::Page, "1.2.3.4", &config, now),
            Decision::Allow
        );
    }

    #[test]
    fn the_window_reopens() {
        let limiter = Limiter::new();
        let config = config();
        let now = Instant::now();

        for _ in 0..4 {
            limiter.check_at(Tier::Page, "1.2.3.4", &config, now);
        }

        let later = now + Duration::from_secs(61);
        assert_eq!(
            limiter.check_at(Tier::Page, "1.2.3.4", &config, later),
            Decision::Allow
        );
    }

    #[test]
    fn a_refusal_says_when_to_come_back() {
        let limiter = Limiter::new();
        let config = config();
        let now = Instant::now();

        for _ in 0..2 {
            limiter.check_at(Tier::Action, "1.2.3.4", &config, now);
        }

        // 30 seconds into a 60-second window.
        let midway = now + Duration::from_secs(30);
        match limiter.check_at(Tier::Action, "1.2.3.4", &config, midway) {
            Decision::Refuse { retry_after_secs } => {
                assert!(
                    (25..=35).contains(&retry_after_secs),
                    "expected roughly 30, got {retry_after_secs}"
                );
            }
            Decision::Allow => panic!("should have been refused"),
        }
    }

    #[test]
    fn retry_after_is_never_zero() {
        // The last instant of a window: the remainder rounds to zero seconds,
        // and `Retry-After: 0` invites a retry certain to be refused.
        let limiter = Limiter::new();
        let config = config();
        let now = Instant::now();

        limiter.check_at(Tier::Signup, "1.2.3.4", &config, now);

        let nearly = now + Duration::from_secs(3_600) - Duration::from_millis(1);
        match limiter.check_at(Tier::Signup, "1.2.3.4", &config, nearly) {
            Decision::Refuse { retry_after_secs } => assert_eq!(retry_after_secs, 1),
            Decision::Allow => panic!("should have been refused"),
        }
    }

    #[test]
    fn a_limit_of_zero_reads_as_unlimited() {
        let limiter = Limiter::new();
        let mut config = config();
        config.page_requests = 0;
        let now = Instant::now();

        for _ in 0..50 {
            assert_eq!(
                limiter.check_at(Tier::Page, "1.2.3.4", &config, now),
                Decision::Allow
            );
        }
    }

    #[test]
    fn creating_a_workspace_is_its_own_tier() {
        assert_eq!(
            classify(&Method::POST, "/api/create-workspace"),
            Some(Tier::Signup)
        );
    }

    #[test]
    fn every_credential_endpoint_is_counted_as_an_action() {
        for path in [
            "/api/sign-in",
            "/api/mfa-challenge",
            "/api/password-reset/request",
            "/api/password-reset/complete",
            "/api/invitations/accept",
            "/auth/handoff",
            "/auth/google/start",
            "/auth/google/callback",
        ] {
            assert_eq!(classify(&Method::POST, path), Some(Tier::Action), "{path}");
        }
    }

    #[test]
    fn every_public_screen_is_counted_when_it_is_viewed() {
        for path in ["/", "/signup", "/forgot-password", "/invitations/accept"] {
            assert_eq!(classify(&Method::GET, path), Some(Tier::Page), "{path}");
        }
    }

    #[test]
    fn the_bundle_is_not_counted() {
        // A page load pulls several megabytes across a handful of requests. If
        // those spent the page allowance, the third reload would fail to load
        // the application at all.
        for path in [
            "/pkg/phonix.js",
            "/pkg/phonix_bg.wasm",
            "/favicon.ico",
            "/robots.txt",
        ] {
            assert_eq!(classify(&Method::GET, path), None, "{path}");
        }
    }

    #[test]
    fn health_probes_are_never_refused() {
        // An orchestrator polls on a schedule and would read a 429 as the
        // process being unwell.
        for path in ["/health/live", "/health/ready"] {
            assert_eq!(classify(&Method::GET, path), None, "{path}");
        }
    }

    #[test]
    fn a_signed_in_call_is_left_to_its_permission_check() {
        for path in [
            "/api/admin/users",
            "/api/account/password",
            "/api/files/page",
            "/dashboard",
            "/admin/roles",
        ] {
            assert_eq!(classify(&Method::POST, path), None, "{path}");
            assert_eq!(classify(&Method::GET, path), None, "{path}");
        }
    }

    #[test]
    fn a_header_is_used_only_when_configured() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "9.9.9.9".parse().unwrap());
        headers.insert("x-forwarded-for", "6.6.6.6".parse().unwrap());

        let request = Request::new(axum::body::Body::empty());

        // Nothing configured: the header is ignored entirely, and with no
        // socket address in the extensions this falls through to "unknown".
        let bare = RateLimitConfig {
            client_ip_header: String::new(),
            ..config()
        };
        assert_eq!(client_key(&bare, &headers, &request), "unknown");

        let named = RateLimitConfig {
            client_ip_header: "X-Real-IP".to_owned(),
            ..config()
        };
        assert_eq!(client_key(&named, &headers, &request), "9.9.9.9");
    }

    #[test]
    fn an_empty_header_falls_back_rather_than_bucketing_everyone_together() {
        // A proxy that sets the header to nothing would otherwise give every
        // visitor on the internet one shared allowance.
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "   ".parse().unwrap());

        let request = Request::new(axum::body::Body::empty());
        let named = RateLimitConfig {
            client_ip_header: "x-real-ip".to_owned(),
            ..config()
        };

        assert_eq!(client_key(&named, &headers, &request), "unknown");
    }

    #[test]
    fn a_key_from_a_header_is_bounded() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "a".repeat(5_000).parse().unwrap());

        let request = Request::new(axum::body::Body::empty());
        let named = RateLimitConfig {
            client_ip_header: "x-real-ip".to_owned(),
            ..config()
        };

        assert_eq!(client_key(&named, &headers, &request).len(), 64);
    }

    #[test]
    fn the_port_is_not_part_of_the_key() {
        // It changes on every connection, so keying on it would hand each
        // request a fresh allowance and count nothing at all.
        let one: SocketAddr = "1.2.3.4:51000".parse().unwrap();
        let two: SocketAddr = "1.2.3.4:51001".parse().unwrap();

        assert_eq!(peer_key(one.ip()), peer_key(two.ip()));
    }

    #[test]
    fn the_map_does_not_grow_without_bound() {
        let limiter = Limiter {
            windows: Mutex::new(HashMap::new()),
            sweep_above: 8,
        };
        let config = config();
        let now = Instant::now();

        for n in 0..40 {
            limiter.check_at(Tier::Page, &format!("10.0.0.{n}"), &config, now);
        }

        // Everything is still live, so nothing is swept yet.
        assert!(limiter.windows.lock().unwrap().len() > 8);

        // An hour later every window has expired, and the next decision clears
        // them out rather than keeping one entry per address for ever.
        let later = now + Duration::from_secs(3_600);
        limiter.check_at(Tier::Page, "10.0.0.99", &config, later);

        assert_eq!(limiter.windows.lock().unwrap().len(), 1);
    }
}
