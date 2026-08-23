//! Time-based one-time passwords (RFC 6238), and the HOTP (RFC 4226) they are
//! built on.
//!
//! Small enough to implement rather than depend on: HOTP is an HMAC, a
//! four-byte window selected by the last nibble, and a modulo. What matters is
//! not the arithmetic but the three things around it, which a dependency would
//! not have decided for us anyway:
//!
//! * **SHA-1, deliberately.** Every authenticator app implements the SHA-1
//!   variant and many implement nothing else. HMAC-SHA1 is unaffected by the
//!   collision attacks that retired SHA-1 for signatures - it is a MAC over a
//!   secret key, not a public digest.
//! * **Codes are compared in constant time.** A byte-wise `==` on a six-digit
//!   string leaks how many leading digits were right, which turns a million
//!   guesses into sixty.
//! * **The drift window is bounded and configured centrally.** Each accepted
//!   step is another 30 seconds a shoulder-surfed code stays usable.
//!
//! Nothing here touches the database. The secret arrives decrypted from
//! [`super::vault`] and leaves as digits.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

type HmacSha1 = Hmac<Sha1>;

/// The parameters a deployment fixed, carried together so no call site has to
/// re-derive them.
#[derive(Debug, Clone, Copy)]
pub struct TotpParams {
    pub digits: u8,
    pub step_secs: u64,
    /// Steps either side of the current one that are still accepted.
    pub skew_steps: u8,
}

impl TotpParams {
    pub fn from_config(cfg: &phonix_config::MfaConfig) -> Self {
        Self {
            digits: cfg.totp_digits,
            step_secs: cfg.totp_step_secs,
            skew_steps: cfg.totp_skew_steps,
        }
    }
}

impl Default for TotpParams {
    /// RFC 6238's own defaults, which is also what every authenticator app
    /// assumes when a URI omits them.
    fn default() -> Self {
        Self {
            digits: 6,
            step_secs: 30,
            skew_steps: 1,
        }
    }
}

/// Generate a shared secret from the OS CSPRNG.
pub fn generate_secret(bytes: usize) -> Vec<u8> {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    let mut secret = vec![0u8; bytes];
    OsRng.fill_bytes(&mut secret);
    secret
}

/// Encode a secret the way an authenticator app expects to receive it.
///
/// Unpadded uppercase RFC 4648 base32. The padding is omitted because the apps
/// that accept a typed-in secret mostly choke on `=`.
pub fn encode_secret(secret: &[u8]) -> String {
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, secret)
}

/// Decode a secret a user typed in, ignoring the spaces apps insert for
/// legibility and accepting either case.
pub fn decode_secret(encoded: &str) -> Option<Vec<u8>> {
    let cleaned: String = encoded
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();

    base32::decode(base32::Alphabet::Rfc4648 { padding: false }, &cleaned)
}

/// One HOTP code for an explicit counter (RFC 4226 section 5.3).
pub fn hotp(secret: &[u8], counter: u64, digits: u8) -> String {
    // `new_from_slice` accepts any key length, which is what RFC 4226 requires;
    // the error case is unreachable for HMAC.
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts a key of any length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();

    // Dynamic truncation: the low nibble of the last byte picks where to read
    // four bytes from, so an attacker cannot predict which part of the digest
    // the code came from.
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);

    let modulus = 10u32.pow(u32::from(digits));
    format!("{:0width$}", binary % modulus, width = usize::from(digits))
}

/// The code for a moment in time.
pub fn code_at(secret: &[u8], unix_seconds: u64, params: TotpParams) -> String {
    hotp(secret, unix_seconds / params.step_secs, params.digits)
}

/// Whether a submitted code is one this secret produces near `unix_seconds`.
///
/// Returns the counter that matched, which the caller stores to stop the same
/// code being replayed inside its own window - accepting a code twice would
/// make a shoulder-surfed digit sequence good for a second sign-in.
pub fn verify(
    secret: &[u8],
    submitted: &str,
    unix_seconds: u64,
    params: TotpParams,
) -> Option<u64> {
    let cleaned = normalise_code(submitted);
    if cleaned.len() != usize::from(params.digits) {
        return None;
    }

    let current = unix_seconds / params.step_secs;
    let skew = i64::from(params.skew_steps);

    let mut matched = None;
    for offset in -skew..=skew {
        let counter = match current.checked_add_signed(offset) {
            Some(counter) => counter,
            None => continue,
        };

        // Every candidate is computed and compared even after a match, so the
        // time taken does not reveal *which* step matched. That difference
        // would say whether the caller's clock is ahead or behind, which is a
        // small leak but a free one to close.
        let candidate = hotp(secret, counter, params.digits);
        if bool::from(candidate.as_bytes().ct_eq(cleaned.as_bytes())) {
            matched = Some(counter);
        }
    }

    matched
}

/// Strip the separators authenticator apps and humans introduce.
pub fn normalise_code(submitted: &str) -> String {
    submitted.chars().filter(char::is_ascii_digit).collect()
}

/// The `otpauth://` URI behind the QR code.
///
/// The label is `Issuer:account` and the issuer is repeated as a parameter -
/// that redundancy is what RFC-adjacent apps actually rely on, and omitting
/// either produces an entry that shows up unlabelled in one app or another.
pub fn provisioning_uri(issuer: &str, account: &str, secret: &[u8], params: TotpParams) -> String {
    format!(
        "otpauth://totp/{issuer_label}:{account_label}\
         ?secret={secret}&issuer={issuer_param}&algorithm=SHA1&digits={digits}&period={period}",
        issuer_label = percent_encode(issuer),
        account_label = percent_encode(account),
        secret = encode_secret(secret),
        issuer_param = percent_encode(issuer),
        digits = params.digits,
        period = params.step_secs,
    )
}

/// Percent-encode everything that is not unreserved in RFC 3986.
///
/// A workspace name can hold spaces, `&`, `#` or a non-ASCII script, and any of
/// those would otherwise end the label or invent a query parameter.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 4226 appendix D test vector: ASCII "12345678901234567890",
    /// counters 0-9. If this ever fails, nothing else in the module matters.
    const RFC4226_SECRET: &[u8] = b"12345678901234567890";
    const RFC4226_CODES: [&str; 10] = [
        "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871",
        "520489",
    ];

    #[test]
    fn matches_the_rfc4226_test_vector() {
        for (counter, expected) in RFC4226_CODES.iter().enumerate() {
            assert_eq!(
                hotp(RFC4226_SECRET, counter as u64, 6),
                *expected,
                "counter {counter}"
            );
        }
    }

    #[test]
    fn matches_the_rfc6238_test_vector() {
        // RFC 6238 appendix B, SHA-1 rows, as 8-digit codes.
        let params = TotpParams {
            digits: 8,
            step_secs: 30,
            skew_steps: 0,
        };

        for (time, expected) in [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
        ] {
            assert_eq!(code_at(RFC4226_SECRET, time, params), *expected, "t={time}");
        }
    }

    #[test]
    fn a_code_is_accepted_within_the_drift_window_and_not_outside_it() {
        let params = TotpParams {
            digits: 6,
            step_secs: 30,
            skew_steps: 1,
        };
        let now = 1_700_000_000u64;
        let code = code_at(RFC4226_SECRET, now, params);

        // The step it was generated in, and one either side.
        assert!(verify(RFC4226_SECRET, &code, now, params).is_some());
        assert!(verify(RFC4226_SECRET, &code, now + 30, params).is_some());
        assert!(verify(RFC4226_SECRET, &code, now - 30, params).is_some());

        // Two steps away is outside the window.
        assert!(verify(RFC4226_SECRET, &code, now + 90, params).is_none());
        assert!(verify(RFC4226_SECRET, &code, now - 90, params).is_none());
    }

    #[test]
    fn a_narrower_window_is_actually_narrower() {
        let strict = TotpParams {
            digits: 6,
            step_secs: 30,
            skew_steps: 0,
        };
        let now = 1_700_000_000u64;
        let code = code_at(RFC4226_SECRET, now, strict);

        assert!(verify(RFC4226_SECRET, &code, now, strict).is_some());
        assert!(verify(RFC4226_SECRET, &code, now + 30, strict).is_none());
    }

    #[test]
    fn verification_reports_which_step_matched() {
        let params = TotpParams::default();
        let now = 1_700_000_000u64;
        let code = code_at(RFC4226_SECRET, now, params);

        // The caller stores this so the same code cannot be replayed while its
        // window is still open.
        assert_eq!(
            verify(RFC4226_SECRET, &code, now, params),
            Some(now / params.step_secs)
        );
    }

    #[test]
    fn the_separators_people_type_are_ignored() {
        let params = TotpParams::default();
        let now = 1_700_000_000u64;
        let code = code_at(RFC4226_SECRET, now, params);
        let spaced = format!("{} {}", &code[..3], &code[3..]);

        assert!(verify(RFC4226_SECRET, &spaced, now, params).is_some());
    }

    #[test]
    fn junk_is_refused_without_pretending_to_be_a_code() {
        let params = TotpParams::default();
        let now = 1_700_000_000u64;

        for bad in ["", "12345", "1234567", "abcdef", "  ", "000000"] {
            // "000000" is a valid shape but astronomically unlikely to be the
            // live code; the point is that none of these panic or match.
            let outcome = verify(RFC4226_SECRET, bad, now, params);
            if bad == "000000" {
                continue;
            }
            assert!(outcome.is_none(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_secret_survives_the_trip_to_an_authenticator_app_and_back() {
        let secret = generate_secret(20);
        let encoded = encode_secret(&secret);

        // 20 bytes is 32 base32 characters, which is the length apps show.
        assert_eq!(encoded.len(), 32);
        assert!(!encoded.contains('='), "padding confuses several apps");
        assert_eq!(decode_secret(&encoded).unwrap(), secret);

        // Apps display the secret in spaced groups and users paste it back
        // that way.
        let spaced = format!("{} {}", &encoded[..16], &encoded[16..]);
        assert_eq!(decode_secret(&spaced).unwrap(), secret);
        assert_eq!(decode_secret(&encoded.to_lowercase()).unwrap(), secret);
    }

    #[test]
    fn generated_secrets_do_not_repeat() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            assert!(seen.insert(generate_secret(20)));
        }
    }

    #[test]
    fn the_provisioning_uri_survives_an_awkward_workspace_name() {
        let uri = provisioning_uri(
            "Phonix & Co",
            "ada@example.com",
            RFC4226_SECRET,
            TotpParams::default(),
        );

        // Nothing unencoded may end the label or start a parameter early.
        assert!(uri.starts_with("otpauth://totp/Phonix%20%26%20Co:ada%40example.com?"));
        assert!(uri.contains("issuer=Phonix%20%26%20Co"));
        assert!(uri.contains("algorithm=SHA1"));
        assert!(uri.contains("digits=6"));
        assert!(uri.contains("period=30"));
        assert!(uri.contains(&format!("secret={}", encode_secret(RFC4226_SECRET))));
    }

    #[test]
    fn eight_digit_codes_are_eight_digits() {
        let params = TotpParams {
            digits: 8,
            ..TotpParams::default()
        };
        let code = code_at(RFC4226_SECRET, 1_700_000_000, params);

        assert_eq!(code.len(), 8);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }
}
