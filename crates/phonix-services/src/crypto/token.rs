//! Opaque bearer tokens: session cookies and one-time links.
//!
//! One rule underpins the whole module: **the database stores a digest, never
//! the token**. A dump of `sessions` or `user_tokens` therefore cannot be
//! replayed - the rows contain nothing anyone can present.
//!
//! SHA-256 rather than Argon2 for that digest, which looks wrong next to
//! `password.rs` and is not. Argon2's cost exists to make guessing a
//! *low-entropy* secret expensive. These tokens are 32 bytes straight from the
//! OS CSPRNG: there is no space to grind, nothing to slow down, and a session
//! digest is recomputed on every single request.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};

/// Bytes of entropy in a token. 256 bits: far past any brute-force horizon,
/// and it lands on 43 URL-safe base64 characters, which fits comfortably in a
/// cookie and in a URL.
const TOKEN_BYTES: usize = 32;

/// Length of the stored digest. Matches the `octet_length(token_hash) = 32`
/// constraint on `sessions` and `user_tokens`.
pub const DIGEST_LEN: usize = 32;

/// A freshly minted token: the secret to hand out, and the digest to store.
///
/// The two are produced together so there is no code path that stores a token
/// in clear by forgetting to hash it first.
pub struct IssuedToken {
    /// Give this to the client exactly once. It cannot be recovered afterwards.
    pub secret: SecretString,
    /// Store this.
    pub digest: Vec<u8>,
}

impl IssuedToken {
    /// Generate a new token from the operating system's CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0u8; TOKEN_BYTES];
        // `OsRng` reads the OS entropy source and panics rather than degrading
        // if it is unavailable, which is the correct failure mode here: a
        // predictable session token is worse than no service.
        OsRng.fill_bytes(&mut bytes);

        let secret = URL_SAFE_NO_PAD.encode(bytes);
        let digest = digest_of(&secret);

        Self {
            secret: SecretString::from(secret),
            digest,
        }
    }
}

/// The digest to look up for a token presented by a client.
pub fn digest_of(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// [`digest_of`] for a token still wrapped in [`SecretString`].
pub fn digest_of_secret(token: &SecretString) -> Vec<u8> {
    digest_of(token.expose_secret())
}

/// Reject a presented token whose shape is wrong before it reaches the
/// database.
///
/// Saves an indexed lookup on obvious junk - an expired bookmark, a truncated
/// paste, a scanner probing for tokens - and keeps unbounded input out of a
/// query parameter.
pub fn looks_like_a_token(candidate: &str) -> bool {
    // 32 bytes in unpadded base64 is exactly 43 characters.
    candidate.len() == 43
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_url_safe_and_full_length() {
        let issued = IssuedToken::generate();
        let secret = issued.secret.expose_secret();

        assert_eq!(secret.len(), 43);
        assert!(looks_like_a_token(secret));
        // Must survive a cookie value and a query string untouched.
        assert!(!secret.contains('+') && !secret.contains('/') && !secret.contains('='));
    }

    #[test]
    fn the_digest_matches_what_a_lookup_would_compute() {
        let issued = IssuedToken::generate();

        assert_eq!(issued.digest.len(), DIGEST_LEN);
        assert_eq!(digest_of(issued.secret.expose_secret()), issued.digest);
        assert_eq!(digest_of_secret(&issued.secret), issued.digest);
    }

    #[test]
    fn the_digest_does_not_contain_the_token() {
        let issued = IssuedToken::generate();
        let secret = issued.secret.expose_secret();

        // The point of storing a digest: the stored bytes must not reveal the
        // value anyone could present.
        assert_ne!(issued.digest.as_slice(), secret.as_bytes());
    }

    #[test]
    fn tokens_do_not_repeat() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            let issued = IssuedToken::generate();
            assert!(
                seen.insert(issued.secret.expose_secret().to_owned()),
                "the CSPRNG produced a duplicate token"
            );
        }
    }

    #[test]
    fn malformed_candidates_never_reach_the_database() {
        for bad in [
            "",
            "short",
            &"a".repeat(42),
            &"a".repeat(44),
            // The characters an injection attempt or a mangled paste brings.
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa+/",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=",
        ] {
            assert!(!looks_like_a_token(bad), "{bad:?} should be rejected");
        }

        assert!(looks_like_a_token(&"a".repeat(43)));
        assert!(looks_like_a_token(&format!("{}-_", "a".repeat(41))));
    }
}
