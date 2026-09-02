//! Phonix Desk's own tables, in the catalog.
//!
//! * [`user`]    - who may sign in to Desk.
//! * [`session`] - a signed-in browser, held by digest.
//! * [`audit`]   - what a desk user did.
//!
//! # Why these are not `core.users`, `core.sessions` and `core.entity_events`
//!
//! Those live in a tenant database and are scoped by `Caller`, which carries a
//! workspace and that workspace's grants. A desk user has no workspace, and the
//! catalog has no permissions - so reusing them would mean inventing a
//! synthetic tenant whose administrators could then write the rows that decide
//! who may administer every other tenant. See
//! `docs/adr/0005-phonix-desk.md` section 4.
//!
//! Everything here takes the **catalog** pool. Nothing in this module opens a
//! tenant connection, and nothing in a tenant may read these tables.

pub mod audit;
pub mod session;
pub mod user;

pub use audit::{DeskAction, DeskAuditEntry, DeskAuditRecord, Outcome};
pub use session::DeskSessionRecord;
pub use user::{DeskUserRecord, DeskUserStatus, NewDeskUser};
