//! The application layer: use cases.
//!
//! A repository answers "what is in this row". A component answers "what does
//! the user see". This crate answers **"what happens when somebody does X"** -
//! and X is usually several rows, a decision or two, and something that must
//! not be half-done if the process dies in the middle.
//!
//! ```text
//!   phonix-web / phonix-server     parse a request, render a response
//!            |
//!            v
//!   phonix-services                sign_in, onboard_workspace, enrol_totp   <- here
//!       |          |
//!       |          +--> phonix-core   the rules: policies, invariants, vocabulary
//!       v
//!   phonix-db                      rows in, rows out
//!            |
//!            v
//!   PostgreSQL
//! ```
//!
//! # Layout
//!
//! | Module        | Use cases                                              |
//! | ------------- | ------------------------------------------------------ |
//! | [`identity`]  | Sign in, answer a challenge, change a password, enrol   |
//! | [`audit`]     | Record what changed about a record, and read it back    |
//! | [`authorization`] | Read and edit what a user or a role may do          |
//! | [`workspace`] | Create a workspace, change its policy                   |
//! | [`mail`]      | Which relay sends, and sending through it               |
//! | [`caller`]    | Who is asking, and whether they may                     |
//! | [`crypto`]    | Not a use case - the primitives the others need         |
//!
//! # Authorization happens here
//!
//! Every use case that changes something takes a [`Caller`] and names its
//! permission on the first line - `caller.require(permissions::USERS_CREATE)?`.
//! A route guard would protect a URL; this protects the operation, which is
//! also reachable from another use case, a background job and any future API.
//! The UI hiding a button is a courtesy on top, not the control.
//!
//! Domains get folders here, not crates. `inventory` will be a folder beside
//! `identity`, with its rows in `phonix_db::inventory` and its vocabulary in
//! `phonix_core::inventory`.
//!
//! # Why the crypto lives here
//!
//! Hashing a password, sealing a TOTP secret and minting a session token are
//! things a *use case does*, not things a table stores. Putting them here is
//! what lets the data access layer keep its own rule: it never receives a
//! credential in a form it could use. By the time anything reaches
//! `phonix-db`, a password is a PHC string, a token is a digest and a shared
//! secret is a sealed blob.
//!
//! They are equally not in `phonix-core`, which compiles to WebAssembly and
//! ships to the browser. A client that could hash with the server's parameters,
//! or produce a TOTP code from a secret, holds a capability it has no business
//! holding.

pub mod audit;
pub mod authorization;
pub mod caller;
pub mod crypto;
pub mod error;
pub mod files;
pub mod identity;
pub mod mail;
pub mod oauth;
pub mod workspace;

pub use caller::Caller;
pub use crypto::{Hasher, SecretVault};
pub use error::{ServiceError, ServiceResult};
pub use files::Files;
pub use identity::authentication::{
    Delivery, SignedIn, authenticate_session, redeem_handoff, sign_in, sign_out,
};
pub use workspace::onboarding::{OnboardedWorkspace, onboard_workspace};

/// Everything a use case in this crate needs from the outside world.
///
/// Built once at startup and passed by reference. It exists so a use case takes
/// one parameter instead of five, and so adding a dependency later - a mailer,
/// a clock - does not change every signature in the crate.
pub struct Security<'a> {
    pub config: &'a phonix_config::SecurityConfig,
    pub hasher: &'a Hasher,
    pub vault: &'a SecretVault,
}

impl std::fmt::Debug for Security<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config carries a key. Nothing here is worth printing.
        f.write_str("Security { .. }")
    }
}
