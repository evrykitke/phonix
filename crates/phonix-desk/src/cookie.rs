//! The desk session cookie.
//!
//! Its own name and its own attributes, deliberately unlike a workspace's.
//!
//! # `SameSite=Strict`, not `Lax`
//!
//! A workspace session is `Lax` because people follow links into their
//! workspace from mail and from other sites, and a cookie that is not sent on
//! that navigation lands them on a sign-in page for no reason. Desk has no such
//! entry point: it is reached by typing its address or from a bookmark, so
//! `Strict` costs nothing and removes every cross-site request from the set of
//! things that can act as a signed-in operator.
//!
//! # Host-only, always
//!
//! No `Domain` attribute. Desk lives on `console-desk.<base_domain>` and a
//! cookie scoped to the parent domain would be sent to every workspace
//! subdomain on the box - which is to say, a desk session token would arrive at
//! the tenant application on every request anybody makes.

use cookie::{Cookie, SameSite, time::Duration as CookieDuration};
use secrecy::{ExposeSecret, SecretString};

/// The cookie name.
///
/// Distinct from any workspace cookie name by construction, and prefixed
/// `__Host-` in nothing: that prefix forbids a `Path` other than `/` and
/// requires `Secure`, which is right in production and would make Desk
/// unusable over plain http on a laptop. The isolation here comes from the
/// hostname, not from the prefix.
pub const COOKIE_NAME: &str = "phonix_desk_session";

/// Build the `Set-Cookie` value that establishes a desk session.
///
/// `max_age_secs` is a hint that should match the session's absolute deadline.
/// The database decides when a session ends; a client ignoring this attribute
/// simply presents a token that has already expired.
pub fn set(token: &SecretString, max_age_secs: i64, secure: bool) -> String {
    Cookie::build((COOKIE_NAME, token.expose_secret().to_owned()))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .max_age(CookieDuration::seconds(max_age_secs.max(0)))
        .build()
        .to_string()
}

/// Build the `Set-Cookie` value that clears it.
///
/// The attributes must match the ones used to set it, or the browser treats it
/// as a different cookie and the original survives - which would be a sign-out
/// button that does nothing.
pub fn clear(secure: bool) -> String {
    Cookie::build((COOKIE_NAME, String::new()))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .max_age(CookieDuration::seconds(0))
        .build()
        .to_string()
}

/// Pull the desk session token out of a `Cookie:` header.
///
/// The value is not validated here - `desk::auth::authenticate` decides whether
/// it could be one of ours before it reaches the database.
pub fn read(header: &str) -> Option<SecretString> {
    Cookie::split_parse(header)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == COOKIE_NAME)
        .map(|cookie| cookie.value().to_owned())
        .filter(|value| !value.is_empty())
        .map(SecretString::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_is_strict_host_only_and_not_readable_from_script() {
        let set = set(&SecretString::from("abc".to_owned()), 3600, true);

        assert!(set.contains("SameSite=Strict"));
        assert!(set.contains("HttpOnly"));
        assert!(set.contains("Secure"));
        assert!(!set.contains("Domain"), "the cookie must stay host-only");
    }

    /// Clearing has to produce the same cookie identity as setting, or the
    /// browser keeps the original and sign-out silently fails.
    #[test]
    fn clearing_matches_the_attributes_that_set_it() {
        let secure = clear(true);

        assert!(secure.contains("SameSite=Strict"));
        assert!(secure.contains("HttpOnly"));
        assert!(secure.contains("Secure"));
        assert!(secure.contains("Max-Age=0"));
    }

    /// On a laptop there is no TLS, and a `Secure` cookie would never be sent
    /// back - a sign-in that appears to succeed and then does nothing.
    #[test]
    fn insecure_is_available_for_a_laptop() {
        assert!(!set(&SecretString::from("abc".to_owned()), 60, false).contains("Secure"));
    }

    #[test]
    fn reading_finds_the_cookie_among_others() {
        let header = "theme=dark; phonix_desk_session=tok-123; other=x";
        let found = read(header).expect("the cookie is there");

        assert_eq!(found.expose_secret(), "tok-123");
    }

    #[test]
    fn an_empty_cookie_is_not_a_token() {
        assert!(read("phonix_desk_session=").is_none());
        assert!(read("something=else").is_none());
    }
}
