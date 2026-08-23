//! Argon2id password hashing.
//!
//! Three things here are load-bearing and easy to get wrong:
//!
//! 1. **Hashing is CPU-bound and slow on purpose** (~19 MiB, tens of
//!    milliseconds). Running it inline would block a tokio worker thread for
//!    that whole time, so every hash and every verify goes through
//!    `spawn_blocking`.
//!
//! 2. **A missing account must cost the same as a wrong password.** If the
//!    "no such user" path returns immediately while the "wrong password" path
//!    spends 50 ms hashing, the difference is trivially measurable and the
//!    login form becomes an account-enumeration oracle. [`Hasher::verify_dummy`]
//!    exists solely to burn that time.
//!
//! 3. **Parameters travel inside the hash.** The stored value is a full PHC
//!    string, so raising the cost later re-hashes on next sign-in rather than
//!    invalidating every password at once.

use std::sync::Arc;
use std::time::Instant;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, ParamsBuilder, Version};
use phonix_config::PasswordConfig;
use secrecy::{ExposeSecret, SecretString};

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("argon2 parameters are invalid: {0}")]
    Params(String),

    #[error("could not hash password: {0}")]
    Hash(String),

    /// The stored value is not a parseable PHC string. Means a corrupted or
    /// hand-edited row, never a wrong password.
    #[error("stored password hash is malformed")]
    MalformedHash,

    #[error("password hashing task failed to run")]
    TaskPanicked,
}

/// A configured Argon2id hasher.
///
/// Cheap to clone and safe to share: `Argon2` holds only its parameters.
#[derive(Clone)]
pub struct Hasher {
    argon2: Arc<Argon2<'static>>,
    warn_above_ms: u64,
    /// A real hash of a throwaway password, computed once at startup, used to
    /// spend verification time when there is no account to check against.
    dummy_hash: Arc<String>,
}

impl Hasher {
    /// Build from configuration. Validates the parameters up front so a bad
    /// combination fails at boot rather than on the first sign-in.
    pub fn new(cfg: &PasswordConfig) -> Result<Self, PasswordError> {
        let params: Params = ParamsBuilder::new()
            .m_cost(cfg.memory_kib)
            .t_cost(cfg.iterations)
            .p_cost(cfg.parallelism)
            .build()
            .map_err(|err| PasswordError::Params(err.to_string()))?;

        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        // Computed with the same parameters as everything else, so the time it
        // burns matches a real verification rather than approximating it.
        let salt = SaltString::generate(&mut OsRng);
        let dummy_hash = argon2
            .hash_password(b"phonix-timing-equaliser", &salt)
            .map_err(|err| PasswordError::Hash(err.to_string()))?
            .to_string();

        Ok(Self {
            argon2: Arc::new(argon2),
            warn_above_ms: cfg.warn_above_ms,
            dummy_hash: Arc::new(dummy_hash),
        })
    }

    /// Hash a new password, returning a PHC string ready to store.
    pub async fn hash(&self, plain: &SecretString) -> Result<String, PasswordError> {
        let argon2 = Arc::clone(&self.argon2);
        let warn_above_ms = self.warn_above_ms;
        // Copied into the blocking task. The original stays wrapped in
        // `SecretString`; this copy lives only for the duration of the hash.
        let plain = plain.expose_secret().to_owned();

        spawn_hashing(move || {
            let started = Instant::now();
            let salt = SaltString::generate(&mut OsRng);
            let hashed = argon2
                .hash_password(plain.as_bytes(), &salt)
                .map_err(|err| PasswordError::Hash(err.to_string()))?
                .to_string();
            warn_if_slow(started, warn_above_ms, "hash");
            Ok(hashed)
        })
        .await
    }

    /// Check a password against a stored PHC string.
    ///
    /// A wrong password is `Ok(false)`, not an error: it is the expected
    /// outcome, and treating it as a fault leads to it being logged at error
    /// level on every typo.
    pub async fn verify(&self, plain: &SecretString, stored: &str) -> Result<bool, PasswordError> {
        let argon2 = Arc::clone(&self.argon2);
        let warn_above_ms = self.warn_above_ms;
        let plain = plain.expose_secret().to_owned();
        let stored = stored.to_owned();

        spawn_hashing(move || {
            let started = Instant::now();
            let parsed = PasswordHash::new(&stored).map_err(|_| PasswordError::MalformedHash)?;
            // Constant-time comparison lives inside `verify_password`.
            let matched = argon2.verify_password(plain.as_bytes(), &parsed).is_ok();
            warn_if_slow(started, warn_above_ms, "verify");
            Ok(matched)
        })
        .await
    }

    /// Spend the same time a real verification would, and discard the result.
    ///
    /// Call this on every sign-in path that has no hash to check - unknown
    /// address, soft-deleted account, an account with no password set - so the
    /// response time does not reveal which addresses exist.
    pub async fn verify_dummy(&self, plain: &SecretString) {
        if let Err(err) = self.verify(plain, &self.dummy_hash).await {
            // Nothing depends on the outcome; a failure here would only mean
            // the equaliser itself is broken, which is worth knowing.
            tracing::warn!(error = %err, "timing equaliser failed");
        }
    }

    /// Whether a stored hash was produced with weaker parameters than the ones
    /// now configured.
    ///
    /// Sign-in is the only moment the plaintext is available, so it is the only
    /// moment a hash can be upgraded. Call this after a successful verify and
    /// re-hash when it returns true.
    pub fn needs_rehash(&self, stored: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(stored) else {
            // Unparseable, so it certainly should not stay as it is.
            return true;
        };

        if parsed.algorithm != argon2::ARGON2ID_IDENT {
            return true;
        }

        let Ok(stored_params) = Params::try_from(&parsed) else {
            return true;
        };
        let current = self.argon2.params();

        stored_params.m_cost() < current.m_cost()
            || stored_params.t_cost() < current.t_cost()
            || stored_params.p_cost() < current.p_cost()
    }
}

/// Run a hashing closure on the blocking pool.
///
/// Argon2 deliberately burns CPU for tens of milliseconds. On an async worker
/// thread that is tens of milliseconds during which nothing else on that thread
/// makes progress - at any concurrency, that is how a login storm turns into a
/// site-wide latency spike.
async fn spawn_hashing<T, F>(work: F) -> Result<T, PasswordError>
where
    F: FnOnce() -> Result<T, PasswordError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| PasswordError::TaskPanicked)?
}

fn warn_if_slow(started: Instant, threshold_ms: u64, operation: &str) {
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if threshold_ms > 0 && elapsed_ms > threshold_ms {
        tracing::warn!(
            operation,
            elapsed_ms,
            threshold_ms,
            "argon2 is slower than expected; the host may be overloaded"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap parameters. Every test here exercises plumbing, not cost, and the
    /// real 19 MiB settings would make the suite crawl.
    fn test_config() -> PasswordConfig {
        PasswordConfig {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
            warn_above_ms: 0,
        }
    }

    fn hasher() -> Hasher {
        Hasher::new(&test_config()).expect("parameters should be valid")
    }

    #[tokio::test]
    async fn a_password_verifies_against_its_own_hash() {
        let hasher = hasher();
        let password = SecretString::from("correct horse battery staple");

        let stored = hasher.hash(&password).await.unwrap();

        assert!(hasher.verify(&password, &stored).await.unwrap());
        assert!(
            !hasher
                .verify(&SecretString::from("wrong"), &stored)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn the_same_password_hashes_differently_every_time() {
        let hasher = hasher();
        let password = SecretString::from("correct horse battery staple");

        let first = hasher.hash(&password).await.unwrap();
        let second = hasher.hash(&password).await.unwrap();

        // Distinct salts. Equal hashes would mean a database dump reveals which
        // accounts share a password.
        assert_ne!(first, second);
        assert!(hasher.verify(&password, &first).await.unwrap());
        assert!(hasher.verify(&password, &second).await.unwrap());
    }

    #[tokio::test]
    async fn the_stored_form_carries_its_parameters() {
        let stored = hasher()
            .hash(&SecretString::from("correct horse battery staple"))
            .await
            .unwrap();

        // PHC string: $argon2id$v=19$m=64,t=1,p=1$<salt>$<hash>
        assert!(stored.starts_with("$argon2id$"), "got {stored}");
        assert!(stored.contains("m=64"));
        assert!(stored.contains("t=1"));
    }

    #[tokio::test]
    async fn a_corrupt_stored_hash_is_an_error_not_a_silent_mismatch() {
        let hasher = hasher();
        let password = SecretString::from("correct horse battery staple");

        // Returning Ok(false) here would make a mangled column look exactly
        // like a wrong password, and nobody would ever find out.
        let err = hasher.verify(&password, "not-a-phc-string").await;
        assert!(matches!(err, Err(PasswordError::MalformedHash)));
    }

    #[tokio::test]
    async fn weaker_stored_parameters_are_flagged_for_rehashing() {
        let weak = hasher();
        let password = SecretString::from("correct horse battery staple");
        let stored = weak.hash(&password).await.unwrap();

        // Same settings: nothing to do.
        assert!(!weak.needs_rehash(&stored));

        // Raised cost: the old hash should be upgraded on next sign-in.
        let stronger = Hasher::new(&PasswordConfig {
            memory_kib: 256,
            iterations: 2,
            parallelism: 1,
            warn_above_ms: 0,
        })
        .unwrap();
        assert!(stronger.needs_rehash(&stored));

        // A hash from the stronger settings must not be downgraded by a server
        // that happens to be configured lower.
        let strong_hash = stronger.hash(&password).await.unwrap();
        assert!(!weak.needs_rehash(&strong_hash));

        assert!(weak.needs_rehash("garbage"));
    }

    #[tokio::test]
    async fn the_timing_equaliser_completes_without_matching_anything() {
        let hasher = hasher();
        // No assertion on the value - there is none. What matters is that it
        // runs the full argon2 cost and does not panic or error.
        hasher.verify_dummy(&SecretString::from("anything")).await;
    }

    #[test]
    fn impossible_parameters_are_rejected_at_construction() {
        // m_cost must be at least 8 * p_cost.
        let bad = PasswordConfig {
            memory_kib: 8,
            iterations: 1,
            parallelism: 64,
            warn_above_ms: 0,
        };
        assert!(matches!(Hasher::new(&bad), Err(PasswordError::Params(_))));
    }
}
