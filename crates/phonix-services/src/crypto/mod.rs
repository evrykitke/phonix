//! The primitives the stored credentials are defined in terms of.
//!
//! Nothing here opens a connection or runs a statement - these four modules are
//! pure computation, and they sit in this crate for one reason: every column
//! they exist for is in this crate's tables.
//!
//! | Column                            | Module         |
//! | --------------------------------- | -------------- |
//! | `users.password_hash`             | [`password`]   |
//! | `sessions.token_hash`             | [`token`]      |
//! | `user_tokens.token_hash`          | [`token`]      |
//! | `user_mfa_factors.secret_encrypted` (TOTP)          | [`vault`] |
//! | `user_mfa_factors.secret_encrypted` (recovery code) | [`token`] |
//! | the code checked against a TOTP secret              | [`totp`]  |
//!
//! # Why not a crate of their own
//!
//! Because nothing else would ever depend on it. A `phonix-crypto` crate would
//! have exactly one consumer - this one - and the split would buy a dependency
//! edge, a second manifest and a version to keep in step, in exchange for a
//! boundary the compiler already enforces at the module level.
//!
//! # Why not in `phonix-core`
//!
//! Because `phonix-core` compiles to WebAssembly and ships to the browser. A
//! client that can hash a password with the server's parameters, or produce a
//! TOTP code from a secret, is a client holding capabilities it has no business
//! holding. Everything in here is server-only by construction.
//!
//! The rule that separates the two, stated once: **`phonix-core` decides what
//! is acceptable, this module decides what is stored.** `PasswordPolicy` lives
//! in core because the sign-up form has to apply it; the Argon2 parameters live
//! here because nothing outside the server may ever see them.

pub mod password;
pub mod token;
pub mod totp;
pub mod vault;

pub use password::{Hasher, PasswordError};
pub use token::IssuedToken;
pub use totp::TotpParams;
pub use vault::SecretVault;
