//! Six-digit codes, for the one place a link will not do.
//!
//! A password reset arrives by email and is finished in a browser, and those
//! are frequently not the same device. A link is the better credential and the
//! worse instruction when the mail is on a phone and the session is on a
//! laptop: it moves the reset to the phone, or it gets retyped by hand out of a
//! URL bar. A code is read off one screen and typed into another, which is what
//! is actually happening.
//!
//! Everything below exists because that convenience costs almost all of the
//! entropy.
//!
//! # A million is not a lot
//!
//! `000000`..`999999` is 2^20 of search space against SHA-256, which a GPU
//! exhausts in well under a second. **Nothing here makes the code hard to
//! guess.** What protects it is entirely outside this module: five attempts,
//! ten minutes, and a row that is burned when either runs out - see
//! [`redeem_code`](phonix_db::identity::one_time_token::redeem_code). The same
//! bargain [`super::super::identity::mfa`] already strikes with TOTP, and for
//! the same reason.
//!
//! This is also why the digest stays SHA-256 rather than becoming Argon2. Argon
//! would not save a code whose whole space is a rounding error, and it would
//! put a 50 ms hash on a path that has to answer identically whether or not the
//! account exists - see the note on timing in
//! [`super::super::identity::password_reset`].
//!
//! # The digest is bound to the account
//!
//! [`digest_for`] hashes the user id together with the code, and that is not
//! decoration. `user_tokens` carries a **unique index** on `token_hash`, built
//! when every token in it was 32 random bytes and a collision was
//! inconceivable. Six digits collide constantly: with a few hundred resets in
//! flight, two people share a code by the birthday bound, and the second
//! `INSERT` would fail on the unique index - turning "somebody else is also
//! resetting their password right now" into an error on *this* user's request.
//!
//! Mixing the user id in makes the stored digest distinct even when the two
//! codes are identical. It has a second effect worth having: a code is only
//! ever meaningful for the account it was issued to, so a code harvested from
//! one mailbox cannot be tried against another.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use phonix_core::identity::UserId;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};

/// Digits in a code. Six, because that is what every authenticator app and
/// every bank has already taught people to expect, and an unfamiliar length is
/// a code people assume they misread.
pub const CODE_DIGITS: usize = 6;

/// One past the largest code. Kept next to [`CODE_DIGITS`] so the two cannot
/// drift.
const CODE_RANGE: u32 = 1_000_000;

/// A freshly minted code: the digits to send, and the digest to store.
pub struct IssuedCode {
    /// Put this in the email. Not recoverable afterwards.
    pub secret: SecretString,
    /// Store this.
    pub digest: Vec<u8>,
}

/// Generate a code for one account.
///
/// Uniform over all million values. The obvious `next_u32() % 1_000_000` is not
/// uniform - `u32::MAX` is not a multiple of a million, so the low values come
/// up slightly more often - and while the bias is far too small to be worth an
/// attack, "slightly more likely" is not a property to leave lying in a
/// credential when rejection sampling costs one loop.
pub fn generate(user_id: UserId) -> IssuedCode {
    // The largest multiple of CODE_RANGE that fits in a u32. Draws at or above
    // this are discarded rather than folded, which is what keeps the
    // distribution flat.
    let ceiling = u32::MAX - (u32::MAX % CODE_RANGE);

    let drawn = loop {
        let mut bytes = [0u8; 4];
        // `OsRng` panics rather than degrading if the OS entropy source is
        // unavailable. That is the right failure here: a predictable reset code
        // is worse than a reset that does not work.
        OsRng.fill_bytes(&mut bytes);

        let candidate = u32::from_le_bytes(bytes);
        if candidate < ceiling {
            break candidate % CODE_RANGE;
        }
    };

    // Zero-padded: `042315` is a six-digit code, and `42315` is a code somebody
    // will report as broken.
    let secret = format!("{drawn:0width$}", width = CODE_DIGITS);
    let digest = digest_for(user_id, &secret);

    IssuedCode {
        secret: SecretString::from(secret),
        digest,
    }
}

/// The digest to compare against, for a code presented for one account.
///
/// The separator matters. Without it, the id bytes and the digits run together
/// and two different (id, code) pairs could in principle produce the same input
/// - a colon cannot appear in either half, so the split is unambiguous.
pub fn digest_for(user_id: UserId, code: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_bytes());
    hasher.update(b":");
    hasher.update(code.as_bytes());
    hasher.finalize().to_vec()
}

/// [`digest_for`] for a code still wrapped in [`SecretString`].
pub fn digest_for_secret(user_id: UserId, code: &SecretString) -> Vec<u8> {
    digest_for(user_id, code.expose_secret())
}

/// Strip what a person types around a code they are copying.
///
/// Spaces, hyphens and non-breaking spaces all arrive from a mail client that
/// grouped the digits for readability, or from somebody typing what they see.
/// Every one of them is a valid attempt against the limit if it is not cleaned
/// up first, and burning somebody's five tries on their own mail client's
/// formatting is the worst way for this feature to fail.
pub fn normalise(input: &str) -> String {
    input.chars().filter(char::is_ascii_digit).collect()
}

/// Whether this is the right shape to be worth a database round trip.
///
/// Applied after [`normalise`]. Keeps unbounded input out of a query parameter
/// and refuses obvious junk - a scanner, a truncated paste - without spending
/// an attempt on it.
pub fn looks_like_a_code(candidate: &str) -> bool {
    candidate.len() == CODE_DIGITS && candidate.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn a_user() -> UserId {
        UserId::from(uuid::Uuid::from_u128(1))
    }

    #[test]
    fn a_code_is_always_six_digits() {
        let user = a_user();

        for _ in 0..2_000 {
            let issued = generate(user);
            let code = issued.secret.expose_secret();

            assert_eq!(code.len(), CODE_DIGITS, "{code}");
            assert!(looks_like_a_code(code), "{code}");
        }
    }

    #[test]
    fn a_leading_zero_survives() {
        // The zero-padding is the whole reason the code is a string and not a
        // number, so it gets a test of its own rather than being left to the
        // odds of the loop above drawing one.
        let user = a_user();
        let mut seen_leading_zero = false;

        for _ in 0..20_000 {
            let issued = generate(user);
            if issued.secret.expose_secret().starts_with('0') {
                seen_leading_zero = true;
                assert_eq!(issued.secret.expose_secret().len(), CODE_DIGITS);
            }
        }

        assert!(
            seen_leading_zero,
            "20000 draws produced no code below 100000, which is not credible"
        );
    }

    #[test]
    fn the_digest_matches_what_a_lookup_would_compute() {
        let user = a_user();
        let issued = generate(user);

        assert_eq!(issued.digest.len(), 32);
        assert_eq!(
            digest_for(user, issued.secret.expose_secret()),
            issued.digest
        );
        assert_eq!(digest_for_secret(user, &issued.secret), issued.digest);
    }

    #[test]
    fn the_same_code_for_two_accounts_stores_two_different_digests() {
        // The unique index on user_tokens.token_hash is why this matters: two
        // people resetting at once and drawing the same six digits must not
        // collide, or the second one's request fails on somebody else's luck.
        let one = UserId::from(uuid::Uuid::from_u128(1));
        let two = UserId::from(uuid::Uuid::from_u128(2));

        assert_ne!(digest_for(one, "123456"), digest_for(two, "123456"));
    }

    #[test]
    fn a_code_is_useless_against_a_different_account() {
        let one = UserId::from(uuid::Uuid::from_u128(1));
        let two = UserId::from(uuid::Uuid::from_u128(2));

        let issued = generate(one);

        // What a lookup for `two` would compare against never equals what was
        // stored for `one`, so the harvested code simply does not match.
        assert_ne!(
            digest_for_secret(two, &issued.secret),
            issued.digest,
            "a code lifted from one mailbox must not work on another account"
        );
    }

    #[test]
    fn codes_are_spread_across_the_whole_range() {
        // Not a randomness test - a guard against an off-by-one in the
        // rejection sampling that quietly clipped the top or bottom of the
        // range. 5000 draws over 10 buckets: every bucket should be hit.
        let user = a_user();
        let mut buckets = [0usize; 10];

        for _ in 0..5_000 {
            let issued = generate(user);
            let value: u32 = issued.secret.expose_secret().parse().expect("digits");
            assert!(value < CODE_RANGE);
            buckets[(value / 100_000) as usize] += 1;
        }

        assert!(
            buckets.iter().all(|count| *count > 0),
            "some part of the range is unreachable: {buckets:?}"
        );
    }

    #[test]
    fn codes_do_not_repeat_in_any_obvious_way() {
        // A million values and a thousand draws will collide occasionally by
        // the birthday bound - roughly a 40% chance of at least one pair. The
        // guard is against a generator that is stuck or nearly so, not against
        // a collision.
        let user = a_user();
        let mut seen = HashSet::new();

        for _ in 0..1_000 {
            seen.insert(generate(user).secret.expose_secret().to_owned());
        }

        assert!(
            seen.len() > 900,
            "only {} distinct codes in 1000",
            seen.len()
        );
    }

    #[test]
    fn what_a_mail_client_did_to_the_digits_is_undone() {
        assert_eq!(normalise("123 456"), "123456");
        assert_eq!(normalise("123-456"), "123456");
        assert_eq!(normalise("  123456  "), "123456");
        // A non-breaking space, which is what a mail client that "helpfully"
        // grouped the digits actually inserts.
        assert_eq!(normalise("123\u{a0}456"), "123456");
    }

    #[test]
    fn malformed_candidates_never_reach_the_database() {
        for bad in ["", "12345", "1234567", "12345a", &"9".repeat(400)] {
            assert!(!looks_like_a_code(bad), "{bad:?} should be rejected");
        }

        assert!(looks_like_a_code("000000"));
        assert!(looks_like_a_code("999999"));
    }
}
