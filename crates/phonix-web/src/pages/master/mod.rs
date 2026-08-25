//! Master data: who this workspace trades with, and what tax it charges.
//!
//! Not under `/admin`, and not by accident. Keeping a customer list up to date
//! is ordinary commercial work; putting it in the administration area would
//! mean granting the administration area to everybody in sales. The one part
//! that *is* administration - changing a tax rate - is gated separately.
//!
//! ```text
//! /master/parties            the list          a grid
//! /master/parties/new        add one           a form
//! /master/parties/:id        one party         Details | Addresses | Contacts | History
//! /master/taxes              the list          Taxes | Groups
//! /master/taxes/new          define a tax      a form
//! /master/taxes/:id          one tax           Details | Rates | History
//! /master/tax-groups/new     define a group    a form
//! /master/tax-groups/:id     one group         a form
//! ```

pub mod parties;
pub mod party;
pub mod tax;
pub mod tax_group;
pub mod taxes;
