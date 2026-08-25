//! Commercial master data: the parties a workspace trades with, and the taxes
//! it applies to them.
//!
//! | Module | Use cases |
//! | ------ | --------- |
//! | [`party`] | List, read, save and remove a party; its addresses and contacts |
//! | [`tax`] | Tax codes, their effective-dated rates, and the groups a line points at |
//!
//! # Where the arithmetic is *not*
//!
//! Nowhere here. `phonix-tax` computes; this module stores, gates and records.
//! The one function that bridges the two is [`tax::treatment_on`], which turns
//! a group and a date into the snapshot a document keeps - and it does no
//! arithmetic either.
//!
//! # Every write is attributable
//!
//! Master data is what appears on a document, so every save here calls
//! [`acting_user`](crate::caller::acting_user): a party nobody created is one
//! nobody can be asked about, and `Caller::System` has no account behind it.
//! Installing seed data is the deliberate exception, and there is none yet.

pub mod party;
pub mod tax;
