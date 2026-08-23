//! Auditing: what happened, and what changed.
//!
//! Two trails, because they answer two different questions and a reader is
//! asking one of them at a time:
//!
//! | Trail | Table | Question |
//! | ----- | ----- | -------- |
//! | Security | `identity_events` | Who signed in, who was locked out, who spent a recovery code? |
//! | Change | `entity_events` | Who created, edited or deleted this record? |
//!
//! The security trail is [`crate::identity::audit`], where it started and where
//! it stays. This module is the change trail, plus the vocabulary both of them
//! render a diff with - see [`change`].
//!
//! # Why they are separate tables
//!
//! The security trail is keyed by an account, because that is what a sign-in
//! happens to. A record edit is not about an account; it is about a record, and
//! folding it into the security trail cost a CHECK-constraint migration per
//! entity while still leaving nothing in the row that said *which* record. See
//! [`entity`] for the full argument.
//!
//! Existing rows were not moved. A trail whose past is rewritten by a
//! deployment is not a trail: the CRUD entries written before this split stay
//! on the security screen and simply stop being added to.
//!
//! # How much of it a workspace wants
//!
//! The change trail is the one that grows without bound, so an organization
//! decides which kinds it records and how long it keeps them - see [`policy`].
//! The security trail has no such switch: it is the record of who got in, and
//! that is not an organization's to turn off.

pub mod change;
pub mod entity;
pub mod policy;

pub use change::{Change, ChangeKind, Fact, FieldChange};
pub use entity::{
    ENTITY_KINDS, EntityAction, EntityChange, EntityChangeDetail, EntityKind, kind, kinds,
};
pub use policy::AuditPolicy;
