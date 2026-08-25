//! Books: what the workspace sells, and what it is owed for it.
//!
//! The first app, and the first module here that reaches across every layer
//! below it at once - a party from `master`, a tax group from `master`, a
//! number series and a currency from `core`, and `Money` to add it up.
//!
//! # Nothing here does arithmetic
//!
//! [`invoice::save`] resolves the tax treatments, hands them to
//! `app_books::pricing`, and stores what comes back. The totals are computed by
//! the same code the browser previewed with, which is what makes the preview
//! honest rather than approximately right.
//!
//! # The one rule no type can enforce
//!
//! **Allocate at post, never at create.** A draft that took a number and was
//! then discarded leaves a permanent gap, and a permanent gap is what an
//! auditor asks about. [`invoice::post`] is the only thing here that touches a
//! sequence, and it does it in the same transaction as the write.

pub mod invoice;
