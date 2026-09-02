//! Phonix Desk's use cases.
//!
//! Desk is the application the platform is run from: workspaces are created,
//! licensed and stopped there. See `docs/adr/0005-phonix-desk.md`.
//!
//! * [`auth`]    - signing a desk user in, and what a desk session may do.
//! * [`account`] - creating desk accounts, and the setup link that gives one a
//!   password of its own.
//! * [`workspace`] - what Desk may do to a workspace, each act writing an
//!   audit row in a database that workspace cannot edit.
//! * [`trail`]   - reading those rows back. Desk's own read of `desk_audit` is
//!   the only screen in this product that shows it.
//! * [`queues`]  - how far behind the background work is, per workspace, in
//!   counts and timestamps and nothing else.
//!
//! # Why this is not `identity`
//!
//! Everything in [`crate::identity`] is scoped by [`crate::caller::Caller`],
//! which carries a workspace and that workspace's grants; `Caller::require` is
//! the gate every other use case in this crate is written against. A desk user
//! has no workspace, and the catalog has no permissions.
//!
//! Reusing `Caller` would mean minting a synthetic tenant whose administrators
//! could then write the rows deciding who administers every other tenant - so
//! these are separate use cases over separate tables, and the two identities
//! never meet. What *is* shared is the machinery underneath: the same Argon2
//! parameters, the same TOTP implementation, the same sealed-secret vault, the
//! same digest-not-token rule for sessions.
//!
//! # Why the use cases are here rather than in the `phonix-desk` binary
//!
//! For the reason `phonix-server` holds none either: an adapter renders and
//! routes, and the moment it also decides, the decision has no other caller and
//! no test that does not involve HTTP.

pub mod account;
pub mod auth;
pub mod queues;
pub mod trail;
pub mod workspace;

pub use account::{CreatedDeskUser, SetupOutcome, SetupPage};
pub use auth::{ChallengeOutcome, DeskCaller, SignInOutcome};
pub use workspace::LicenceDecision;
