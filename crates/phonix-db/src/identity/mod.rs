//! Identity storage: accounts, credentials, sessions and the audit trail.
//!
//! Everything here operates on **one tenant's** database. There is no tenant
//! column and no tenant filter: isolation is the database boundary, so a query
//! that touches the wrong workspace is a routing bug rather than a missing
//! `WHERE` clause.
//!
//! | Module              | Table it owns                                       |
//! | ------------------- | --------------------------------------------------- |
//! | [`user`]            | `users`                                             |
//! | [`session`]         | `sessions`                                          |
//! | [`one_time_token`]  | `user_tokens`                                       |
//! | [`password_history`]| `password_history`                                  |
//! | [`mfa`]             | `user_mfa_factors`                                  |
//! | [`audit`]           | `identity_events`                                   |
//!
//! Nothing here decides anything. Hashing, digesting, sealing, verifying and
//! the order the five tables are touched in all live in
//! `phonix_services::identity`; these modules turn rows into values and back.

pub mod audit;
pub mod mfa;
pub mod one_time_token;
pub mod password_history;
pub mod session;
pub mod user;

pub use audit::{AuditEntry, AuditRecord, IdentityEvent};
pub use mfa::StoredFactor;
pub use one_time_token::{TokenPurpose, TokenRecord};
pub use session::{ClientFacts, SessionRecord};
pub use user::{NewUser, UserRecord};
