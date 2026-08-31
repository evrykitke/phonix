//! Session cookie construction and parsing.
//!
//! Lives here, next to the sessions it carries, because two callers need it and
//! they cannot share an axum extractor: `phonix-server` sets cookies from an
//! axum handler, while the Leptos server functions set them through
//! `ResponseOptions`. Both end up writing a `Set-Cookie` header string, so that
//! string is built in one place.
//!
//! # Why the cookie is host-only
//!
//! No `Domain` attribute is ever set. A cookie scoped to `phonix.example.com`
//! would be sent to *every* workspace subdomain, so one workspace's server
//! would receive another's session token on every request. Host-only means
//! `acme.example.com` gets only its own.
//!
//! That is why signup cannot simply set a cookie: the wizard runs on the bare
//! domain and the new workspace lives on a subdomain. The one-time handoff
//! token in `one_time_token.rs` exists to cross that boundary.

use cookie::{Cookie, SameSite, time::Duration as CookieDuration};
use phonix_config::{SameSitePolicy, SessionConfig};
use secrecy::{ExposeSecret, SecretString};

/// Build the `Set-Cookie` value that establishes a session.
///
/// `max_age_secs` should match the session's absolute deadline. It is a hint:
/// the server decides when a session ends, and a client that ignores the
/// attribute simply presents a token the database has already expired.
pub fn set_session(
    cfg: &SessionConfig,
    tenant_slug: &str,
    token: &SecretString,
    max_age_secs: i64,
) -> String {
    let cookie = Cookie::build((
        cfg.cookie_name_for(tenant_slug),
        token.expose_secret().to_owned(),
    ))
    .path("/")
    // Not readable from JavaScript, so an XSS bug cannot simply lift the
    // session token out of `document.cookie`.
    .http_only(true)
    .secure(cfg.secure)
    .same_site(same_site(cfg.same_site))
    .max_age(CookieDuration::seconds(max_age_secs))
    // No `.domain(..)`: see the module note. Host-only is the isolation.
    .build();

    cookie.to_string()
}

/// Build the `Set-Cookie` value that clears a session.
///
/// The attributes must match the ones used to set it - path, secure, same-site
/// - or the browser treats it as a different cookie and the original survives.
pub fn clear_session(cfg: &SessionConfig, tenant_slug: &str) -> String {
    let cookie = Cookie::build((cfg.cookie_name_for(tenant_slug), String::new()))
        .path("/")
        .http_only(true)
        .secure(cfg.secure)
        .same_site(same_site(cfg.same_site))
        .max_age(CookieDuration::seconds(0))
        .build();

    cookie.to_string()
}

/// Pull one cookie's value out of a `Cookie:` request header.
///
/// Returns `None` for a missing or empty cookie. The value is *not* validated
/// here - [`crate::identity::session::is_plausible_token`] does that.
pub fn read(header: &str, name: &str) -> Option<String> {
    Cookie::split_parse(header)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
        .filter(|value| !value.is_empty())
}

fn same_site(policy: SameSitePolicy) -> SameSite {
    match policy {
        SameSitePolicy::Strict => SameSite::Strict,
        SameSitePolicy::Lax => SameSite::Lax,
        SameSitePolicy::None => SameSite::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SessionConfig {
        SessionConfig {
            cookie_name: "phonix_session".into(),
            idle_timeout_mins: 720,
            absolute_timeout_hours: 168,
            remember_me_days: 30,
            secure: true,
            same_site: SameSitePolicy::Lax,
            handoff_ttl_secs: 120,
            purge_interval_mins: 60,
            mobile: phonix_config::MobileSessionConfig {
                idle_timeout_mins: 43_200,
                absolute_timeout_days: 90,
            },
        }
    }

    #[test]
    fn a_session_cookie_carries_every_protective_attribute() {
        let header = set_session(&config(), "acme", &SecretString::from("a".repeat(43)), 3600);

        assert!(header.starts_with("phonix_session_acme="));
        assert!(header.contains("HttpOnly"), "got {header}");
        assert!(header.contains("Secure"), "got {header}");
        assert!(header.contains("SameSite=Lax"), "got {header}");
        assert!(header.contains("Path=/"), "got {header}");
        assert!(header.contains("Max-Age=3600"), "got {header}");
    }

    #[test]
    fn a_session_cookie_is_never_scoped_to_a_parent_domain() {
        // The isolation property: with a Domain attribute this cookie would be
        // sent to every other workspace's host as well.
        let header = set_session(&config(), "acme", &SecretString::from("token"), 3600);
        assert!(
            !header.to_lowercase().contains("domain="),
            "cookie must stay host-only, got {header}"
        );
    }

    #[test]
    fn each_workspace_gets_its_own_cookie_name() {
        let cfg = config();
        // Signing in to a second workspace must not evict the first.
        assert_eq!(cfg.cookie_name_for("acme"), "phonix_session_acme");
        assert_ne!(cfg.cookie_name_for("acme"), cfg.cookie_name_for("globex"));
        // Hyphens are legal in a cookie name but make the pairing harder to
        // read in devtools, so slugs are normalised.
        assert_eq!(
            cfg.cookie_name_for("north-wind"),
            "phonix_session_north_wind"
        );
    }

    #[test]
    fn clearing_matches_the_attributes_used_to_set() {
        let cfg = config();
        let cleared = clear_session(&cfg, "acme");

        // A browser only replaces a cookie when name, path and domain match.
        assert!(cleared.starts_with("phonix_session_acme="));
        assert!(cleared.contains("Path=/"));
        assert!(cleared.contains("Max-Age=0"));
        assert!(cleared.contains("HttpOnly"));
        assert!(cleared.contains("Secure"));
    }

    #[test]
    fn insecure_deployments_omit_the_secure_attribute() {
        // http://localhost would never store a Secure cookie.
        let mut cfg = config();
        cfg.secure = false;
        let header = set_session(&cfg, "acme", &SecretString::from("token"), 60);
        assert!(!header.contains("Secure"), "got {header}");
    }

    #[test]
    fn one_cookie_is_found_among_many() {
        let header = "theme=dark; phonix_session_acme=abc123; _ga=GA1.2.3";
        assert_eq!(
            read(header, "phonix_session_acme").as_deref(),
            Some("abc123")
        );
        assert_eq!(read(header, "theme").as_deref(), Some("dark"));
        assert_eq!(read(header, "phonix_session_globex"), None);
    }

    #[test]
    fn absent_and_empty_cookies_both_read_as_none() {
        assert_eq!(read("", "phonix_session_acme"), None);
        assert_eq!(read("other=1", "phonix_session_acme"), None);
        // A cleared cookie is still sent until the browser drops it.
        assert_eq!(read("phonix_session_acme=", "phonix_session_acme"), None);
    }

    #[test]
    fn a_cookie_name_that_is_a_prefix_of_another_is_not_confused() {
        let header = "phonix_session_acme=one; phonix_session_acme_test=two";
        assert_eq!(read(header, "phonix_session_acme").as_deref(), Some("one"));
        assert_eq!(
            read(header, "phonix_session_acme_test").as_deref(),
            Some("two")
        );
    }
}
