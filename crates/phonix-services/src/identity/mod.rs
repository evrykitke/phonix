//! Identity use cases.
//!
//! | Module             | What happens                                        |
//! | ------------------ | --------------------------------------------------- |
//! | [`authentication`] | Sign in, resume a session, sign out                  |
//! | [`directory`]      | Who is in this workspace                             |
//! | [`invitation`]     | Add somebody, and let them accept                    |
//! | [`audit_view`]     | A stored audit row as a screen may read it           |
//! | [`session`]        | Mint a session token; the digest goes to the DBAL    |
//! | [`one_time_token`] | Mint a link token; the digest goes to the DBAL       |
//! | [`mfa`]            | Enrol a factor, answer a challenge, issue recovery codes |
//! | [`password`]       | Change a password under this workspace's policy      |
//!
//! Each one reads what it needs, decides, and writes through
//! `phonix_db`. The decisions themselves - is this password acceptable, must
//! this user hold a second factor - are `phonix_core`'s policies, applied here.

pub mod audit_view;
pub mod authentication;
pub mod directory;
pub mod invitation;
pub mod mfa;
pub mod one_time_token;
pub mod password;
pub mod session;

pub use authentication::{
    Delivery, SignedIn, authenticate_session, redeem_handoff, sign_in, sign_out,
};
pub use mfa::{VerifiedFactor, answer_challenge};
pub use session::OpenedSession;
