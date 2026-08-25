//! Sales: what this workspace invoices.
//!
//! ```text
//! /sales/invoices          the list          a grid
//! /sales/invoices/new      raise one         the editor
//! /sales/invoices/:id      one invoice       the editor, or the document
//! ```
//!
//! # One route, two screens
//!
//! `/sales/invoices/:id` is the editor while the invoice is a draft and the
//! *document* once it has been posted. One address rather than two, because
//! posting does not move an invoice - it changes what may be done to it, and a
//! link somebody sent last week should still open the thing they meant.

pub mod invoice;
pub mod invoices;
