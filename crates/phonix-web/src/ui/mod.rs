//! The interface kit: pieces that are configured, not written.
//!
//! # How this differs from [`components`](crate::components)
//!
//! `components` is *this application's* furniture. It knows there is a sidebar,
//! a workspace badge, a permission tree. Each piece is written once and used
//! where it belongs.
//!
//! `ui` is furniture that has never heard of Phonix. Nothing here mentions a
//! user, a role or an invoice: a piece takes a configuration describing the
//! shape of some data and renders it. The test of whether something belongs in
//! this module is whether an inventory module could use it unchanged.
//!
//! That line matters more as modules arrive. Inventory will want a list of
//! requisitions that searches, sorts, pages, exports and offers permission-gated
//! actions - which is, feature for feature, what the users list wants. Written
//! as two screens they drift apart within a release. Written as one
//! [`table::DataGrid`] and two configurations, they cannot.
//!
//! # What a module contributes
//!
//! A configuration, not a component. Entity configurations live under
//! [`table::config`], one file per entity, each a function returning a
//! [`table::GridConfig`]. A screen is then the header plus the grid:
//!
//! ```ignore
//! view! {
//!     <PageHeader title="Requisitions" icon=Icon::ClipboardList />
//!     <DataGrid config=requisitions_grid() />
//! }
//! ```

pub mod alert;
pub mod calendar;
pub mod card;
pub mod form;
pub mod table;
pub mod tabs;
pub mod viewer;
