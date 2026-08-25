//! The `master` app's tables: parties, and the tax vocabulary.
//!
//! # Every statement here is qualified
//!
//! `master.parties`, never `parties`. A request runs on `core,public` - the
//! app schemas are deliberately absent from the search path - so an
//! unqualified reference is a loud error rather than a quiet wrong answer. The
//! only place `master` is on the path is its own migration stream, and that is
//! what puts `master._sqlx_migrations` inside `master`.
//!
//! # No foreign key leaves this schema, except into core
//!
//! `parties.currency_code` and the `updated_by` columns point at `core`, which
//! is the one permitted target. Nothing here points at an app, and no app
//! points here: Books references a party by id and resolves it through this
//! module, which is what keeps `DROP SCHEMA books CASCADE` a safe thing to do.
//!
//! # Where the reading is done
//!
//! A [`party::Party`] is four tables - the party, its roles, its addresses, its
//! contacts - and [`party::find`] fetches all four. That is deliberate: the
//! caller wanting a party wants the addresses, and a lazily-loaded address is
//! an address that is missing on the one screen nobody tested. The list screen
//! reads [`party::PartySummary`], which is one table and a lateral aggregate.

pub mod party;
pub mod tax;
