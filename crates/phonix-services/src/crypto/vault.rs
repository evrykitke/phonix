//! Encryption for secrets the server has to be able to read back.
//!
//! Everything else in this module tree is hashed: passwords with Argon2,
//! session tokens and recovery codes with SHA-256. A TOTP secret cannot be,
//! because verifying a code means *recomputing* it, which means holding the
//! secret. So it is encrypted instead, with a key that lives in the environment
//! rather than in the database - a stolen dump is then a pile of ciphertext
//! rather than every user's authenticator app.
//!
//! # Shape of a sealed value
//!
//! ```text
//! [0]      key version   (1 byte)
//! [1..25]  nonce         (24 bytes, random per message)
//! [25..]   ciphertext + Poly1305 tag
//! ```
//!
//! XChaCha20-Poly1305 for the 192-bit nonce: at that size a random nonce per
//! message has no meaningful collision probability, so there is no counter to
//! keep and no way for two servers to reuse one. AES-GCM's 96-bit nonce would
//! have needed either a shared counter or a birthday-bound argument.
//!
//! The version byte is what makes key rotation possible later without a
//! migration that has to decrypt every row up front.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use phonix_config::MfaConfig;
use secrecy::SecretBox;

use crate::error::ServiceError;

/// The key version this build writes. Stored both in the sealed bytes and in
/// `user_mfa_factors.key_version`.
pub const KEY_VERSION: u8 = 1;

const NONCE_LEN: usize = 24;
/// Version byte + nonce + Poly1305 tag. Anything shorter cannot be a sealed
/// value at all.
const OVERHEAD: usize = 1 + NONCE_LEN + 16;

/// Seals and opens the secrets that have to be recoverable.
pub struct SecretVault {
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for SecretVault {
    /// Deliberately opaque. A `Debug` that printed the cipher state would put
    /// key material into whatever log line derived it by accident.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretVault(<key>)")
    }
}

impl SecretVault {
    /// Build from a raw 32-byte key.
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new(key.into()),
        }
    }

    /// Build from the configured base64 key.
    ///
    /// Called once at startup: a deployment with a missing or malformed key
    /// should fail there, not at the moment a user tries to enrol.
    pub fn from_config(cfg: &MfaConfig) -> Result<Self, ServiceError> {
        let key = cfg.encryption_key_bytes().map_err(|problem| {
            ServiceError::Crypto(format!("security.mfa.encryption_key {problem}"))
        })?;

        Ok(Self::new(&key))
    }

    /// Encrypt, binding the result to `context`.
    ///
    /// `context` is authenticated but not encrypted. Passing the owning user's
    /// id means a ciphertext lifted from one row and pasted into another fails
    /// to open: the row is part of what was authenticated, so moving it breaks
    /// the tag.
    pub fn seal(&self, plaintext: &[u8], context: &[u8]) -> Result<Vec<u8>, ServiceError> {
        use argon2::password_hash::rand_core::{OsRng, RngCore};

        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from(nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: context,
                },
            )
            .map_err(|_| ServiceError::Crypto("could not encrypt secret".to_owned()))?;

        let mut sealed = Vec::with_capacity(OVERHEAD + plaintext.len());
        sealed.push(KEY_VERSION);
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ciphertext);

        Ok(sealed)
    }

    /// Decrypt a value produced by [`Self::seal`] with the same `context`.
    ///
    /// Every failure - truncated bytes, an unknown version, a wrong key, a
    /// tampered tag, the wrong context - comes back the same way. There is
    /// nothing useful to tell apart here, and a caller that could would have a
    /// padding-oracle-shaped hole to work with.
    pub fn open(&self, sealed: &[u8], context: &[u8]) -> Result<SecretBox<Vec<u8>>, ServiceError> {
        let failed = || ServiceError::Crypto("could not decrypt secret".to_owned());

        if sealed.len() <= OVERHEAD {
            return Err(failed());
        }
        if sealed[0] != KEY_VERSION {
            // A row written under a key this build does not hold. Distinct in
            // the log, identical to the caller.
            tracing::warn!(
                version = sealed[0],
                expected = KEY_VERSION,
                "sealed secret was written under a different key version"
            );
            return Err(failed());
        }

        // `try_from` rather than the deprecated `from_slice`: the length was
        // checked above, but a panic here would be a denial of service driven
        // by a database value.
        let Ok(nonce) = XNonce::try_from(&sealed[1..1 + NONCE_LEN]) else {
            return Err(failed());
        };
        let plaintext = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &sealed[1 + NONCE_LEN..],
                    aad: context,
                },
            )
            .map_err(|_| failed())?;

        Ok(SecretBox::new(Box::new(plaintext)))
    }
}

/// The context bytes for this workspace's relay password.
///
/// A distinct domain from [`user_context`] and deliberately not parameterised:
/// there is one relay row per tenant database, and the tenant boundary is the
/// database. Binding it as associated data means a sealed relay password lifted
/// into `user_mfa_factors` - or the other way round - fails to open rather than
/// decrypting into the wrong meaning.
pub fn mail_context() -> Vec<u8> {
    b"phonix:mail-settings".to_vec()
}

/// The context bytes for a user's own secret.
pub fn user_context(user_id: uuid::Uuid) -> Vec<u8> {
    let mut context = b"phonix:mfa:".to_vec();
    context.extend_from_slice(user_id.as_bytes());
    context
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> SecretVault {
        SecretVault::new(&[7u8; 32])
    }

    #[test]
    fn a_secret_sealed_for_one_purpose_will_not_open_as_another() {
        // The point of binding the context: a relay password lifted into a
        // user's MFA row must fail to open, not decrypt into a wrong meaning.
        let sealed = vault().seal(b"relay password", &mail_context()).unwrap();

        assert!(
            vault()
                .open(&sealed, &user_context(uuid::Uuid::nil()))
                .is_err()
        );
        assert!(vault().open(&sealed, &mail_context()).is_ok());
    }

    #[test]
    fn a_sealed_secret_comes_back_intact() {
        use secrecy::ExposeSecret;

        let context = user_context(uuid::Uuid::nil());
        let sealed = vault().seal(b"shared secret", &context).unwrap();

        let opened = vault().open(&sealed, &context).unwrap();
        assert_eq!(opened.expose_secret().as_slice(), b"shared secret");
    }

    #[test]
    fn the_sealed_bytes_do_not_contain_the_secret() {
        let context = user_context(uuid::Uuid::nil());
        let sealed = vault().seal(b"shared secret", &context).unwrap();

        assert!(
            !sealed.windows(13).any(|window| window == b"shared secret"),
            "the plaintext survived into the stored bytes"
        );
        assert_eq!(sealed[0], KEY_VERSION);
    }

    #[test]
    fn the_same_secret_seals_differently_every_time() {
        let context = user_context(uuid::Uuid::nil());
        let first = vault().seal(b"shared secret", &context).unwrap();
        let second = vault().seal(b"shared secret", &context).unwrap();

        // A fresh nonce per message. Equal ciphertexts would mean two users
        // with the same secret were visibly the same in the table.
        assert_ne!(first, second);
    }

    #[test]
    fn a_ciphertext_moved_to_another_user_will_not_open() {
        // The whole point of authenticating the user id: a row lifted from one
        // account and pasted into another must not hand over a working factor.
        let alice = user_context(uuid::Uuid::from_u128(1));
        let mallory = user_context(uuid::Uuid::from_u128(2));

        let sealed = vault().seal(b"alice's secret", &alice).unwrap();
        assert!(vault().open(&sealed, &mallory).is_err());
        assert!(vault().open(&sealed, &alice).is_ok());
    }

    #[test]
    fn tampering_is_detected() {
        let context = user_context(uuid::Uuid::nil());
        let sealed = vault().seal(b"shared secret", &context).unwrap();

        for index in [0, 1, sealed.len() - 1] {
            let mut tampered = sealed.clone();
            tampered[index] ^= 0xff;
            assert!(
                vault().open(&tampered, &context).is_err(),
                "a flipped byte at {index} went undetected"
            );
        }

        // Truncation, which is what a badly-typed column or a partial write
        // would produce.
        assert!(vault().open(&sealed[..OVERHEAD], &context).is_err());
        assert!(vault().open(&[], &context).is_err());
    }

    #[test]
    fn another_key_cannot_open_it() {
        let context = user_context(uuid::Uuid::nil());
        let sealed = vault().seal(b"shared secret", &context).unwrap();

        let other = SecretVault::new(&[9u8; 32]);
        assert!(other.open(&sealed, &context).is_err());
    }

    #[test]
    fn the_debug_output_holds_no_key_material() {
        let rendered = format!("{:?}", SecretVault::new(&[0xab; 32]));
        assert_eq!(rendered, "SecretVault(<key>)");
        assert!(!rendered.contains("ab"));
    }
}
