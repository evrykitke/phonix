//! What a request says about the client that sent it.
//!
//! Two strings, both stored on the session row and both shown back to somebody
//! reviewing the devices holding their account. They are read the same way for
//! every adapter, which is the whole reason this is a function rather than four
//! lines repeated at each sign-in path: a browser's sign-in and a phone's
//! describing the same client differently would make one workspace's audit
//! trail disagree with itself.

use http::HeaderMap;

/// Longest user-agent stored.
///
/// This ends up in a column and on a screen, and the header is attacker
/// controlled and unbounded. 256 is comfortably longer than any real browser
/// or mobile client sends, and short enough that nothing has to think about it.
const MAX_USER_AGENT: usize = 256;

/// The address and client a request came from.
///
/// Owned rather than borrowed because the caller needs somewhere to keep the
/// strings while it builds the borrowing `ClientFacts` the repository takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientFactsOwned {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// Read the two facts out of a request's headers.
pub fn facts_of(headers: &HeaderMap) -> ClientFactsOwned {
    // `x-forwarded-for` first: behind a proxy the socket address is the proxy.
    // Only the first entry is read - the rest are appended by intermediaries,
    // and the client controls whatever it sent.
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_owned())
        // An empty header is not an address, and storing "" would make a row
        // that has one indistinguishable from a row that does not.
        .filter(|ip| !ip.is_empty());

    let user_agent = headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(MAX_USER_AGENT).collect());

    ClientFactsOwned { ip, user_agent }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                http::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                http::HeaderValue::from_str(value).expect("a header value"),
            );
        }
        headers
    }

    #[test]
    fn the_client_is_the_first_entry_not_the_last_proxy() {
        let facts = facts_of(&headers(&[(
            "x-forwarded-for",
            "203.0.113.9, 198.51.100.4, 10.0.0.1",
        )]));

        assert_eq!(facts.ip.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn a_missing_or_empty_forwarded_header_is_no_address_at_all() {
        assert_eq!(facts_of(&headers(&[])).ip, None);
        // Not `Some("")`, which would look like a recorded address on a screen
        // and sort as one in a list.
        assert_eq!(facts_of(&headers(&[("x-forwarded-for", "")])).ip, None);
        assert_eq!(facts_of(&headers(&[("x-forwarded-for", "  ")])).ip, None);
    }

    #[test]
    fn a_user_agent_cannot_decide_how_wide_the_column_is() {
        let facts = facts_of(&headers(&[("user-agent", &"a".repeat(4096))]));

        assert_eq!(facts.user_agent.map(|ua| ua.chars().count()), Some(256));
    }

    #[test]
    fn an_ordinary_user_agent_survives_intact() {
        let real = "Phonix/1.0 (iPhone; iOS 18.2)";
        let facts = facts_of(&headers(&[("user-agent", real)]));

        assert_eq!(facts.user_agent.as_deref(), Some(real));
    }
}
