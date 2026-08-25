//! The `master` app's shared vocabulary.
//!
//! Commercial master data: the people and organizations a workspace trades
//! with, and the tax treatment it applies to them. An ordinary app - always
//! installed for a commercial product, absent from a pure clinical one - and
//! the first citizen after `core`.
//!
//! | Module | Question it answers |
//! | ------ | ------------------- |
//! | [`party`] | Who is on the other side of a document? |
//! | [`address`] | Where do we bill them, and where do we ship? |
//! | [`contact`] | Who at that organization do we actually write to? |
//! | [`tax`] | What tax applies, and what does it come to? |
//!
//! # Why parties are here and not in `core`
//!
//! `core` knows a party *exists* in the same way it knows a document number
//! exists: as identity and mechanism. It does not know a party is a supplier,
//! because "supplier" is a meaning Procurement assigns and "patient" is a
//! meaning a clinical product assigns to the same row. A workspace running
//! neither should not be carrying the table.
//!
//! # A role is a claim an app makes, not a column
//!
//! [`party::PartyRole`] is an open vocabulary: Books marks a party
//! `"customer"`, Procurement marks it `"supplier"`, and the same organization
//! is routinely both. A pair of booleans on the party would have needed a
//! migration per app, and a `kind` column would have forced a choice that is
//! not exclusive in real trade.
//!
//! # This crate may not panic
//!
//! Same rule as `phonix-core` and `phonix-tax`, and for the same reason: it is
//! compiled into the WebAssembly bundle, where one panic freezes every handler
//! on the page at once.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod address;
pub mod contact;
pub mod party;

/// The tax vocabulary, re-exported so a screen about master data needs one
/// import. Tax codes and groups are `master` tables; the arithmetic that reads
/// them is its own crate because it has to stay free of anything but numbers.
pub use phonix_tax as tax;

pub use address::{
    AddressError, AddressPurpose, MAX_ADDRESS_LINE, PartyAddress, PartyAddressInput, PostalAddress,
};
pub use contact::{ContactError, PartyContact, PartyContactInput};
pub use party::{
    MAX_PARTY_CODE_LEN, MAX_PARTY_NAME_LEN, Party, PartyError, PartyInput, PartyKind, PartyRole,
    PartySummary, roles,
};
