//! The `books` app's tables: what the workspace sells.
//!
//! # Every statement here is qualified
//!
//! `books.invoices`, never `invoices`. A request runs on `core,public` - the
//! app schemas are deliberately absent from the search path - so an unqualified
//! reference is a loud error rather than a quiet wrong answer.
//!
//! # This schema points at `core`, and at nothing else
//!
//! `core.currencies` and `core.users` are proper foreign keys. `master.parties`
//! and `master.tax_codes` are referenced **by id, without one** - which is what
//! makes an app uninstallable, and why the columns beside those ids are a
//! snapshot rather than a join.
//!
//! # Numbers are taken in the document's own transaction
//!
//! [`invoice::post`] takes a `&mut PgConnection` for the same reason
//! [`crate::numbering::allocate`] does: the row lock that makes a sequence
//! gap-free is held until the surrounding transaction ends, so the allocation
//! and the `UPDATE` that stores the number have to be in the same one. A failed
//! post then *returns* the number rather than burning it.

pub mod invoice;
