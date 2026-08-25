//! Tax: the vocabulary, and the arithmetic.
//!
//! No I/O. Lines in, per-line and per-tax totals out - which is what lets the
//! browser preview the exact figures the server will post, with no round trip
//! and no possibility of the two disagreeing. Storage lives in the `master`
//! app; this crate never learns that a database exists.
//!
//! | Module | Question it answers |
//! | ------ | ------------------- |
//! | [`rate`] | How much, as a number a `NUMERIC(9, 6)` holds exactly? |
//! | [`code`] | Which tax is this, where does it apply, and is it recoverable? |
//! | [`group`] | Which taxes does a document line actually attract? |
//! | [`compute`] | What are the numbers, and how were they arrived at? |
//!
//! # A line references a group, never a code
//!
//! "VAT 20%" is a group with one member. "GST 18%" is a group with CGST 9% and
//! SGST 9%. Quebec's compound arrangement is a group with two members and
//! `is_compound` on the second. That single decision is what makes this model
//! work in the EU, India, Canada and the United States without a schema change,
//! and [`group::TaxGroupMember::sequence`] is what makes compound ordering
//! deterministic rather than dependent on how a query happened to sort.
//!
//! # This crate may not panic
//!
//! Same rule as `phonix-core`, and for the same reason: it is compiled into the
//! WebAssembly bundle, where `wasm32-unknown-unknown` aborts rather than
//! unwinds and a single panic freezes every handler on the page at once. A
//! fallible thing returns a `Result`.

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

pub mod code;
pub mod compute;
pub mod group;
pub mod rate;

pub use code::{
    MAX_CODE_LEN, MAX_NAME_LEN, TaxCode, TaxCodeError, TaxCodeInput, TaxCodeSummary, TaxKind,
};
pub use compute::{
    DocumentTax, LineResult, LineTax, Pricing, RoundingLevel, TaxDocument, TaxError, TaxLine,
    TaxTotal, compute,
};
pub use group::{
    AppliedTax, MAX_MEMBERS, TaxGroup, TaxGroupError, TaxGroupInput, TaxGroupMember, TaxTreatment,
    member_from,
};
pub use rate::{
    MAX_RATE_SCALED, RATE_ONE, RATE_SCALE, TaxRate, TaxRateError, TaxRateInput, TaxRatePeriod,
    TaxRateRow,
};
