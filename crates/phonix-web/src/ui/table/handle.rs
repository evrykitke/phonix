//! How an action talks back to the grid it was clicked in.
//!
//! # Why a handle and not a context lookup
//!
//! An action's closure is written in a configuration file, long before any grid
//! exists, and it usually runs inside a spawned task after an `await` - by
//! which point reactive context lookups are a guess about which owner is
//! current. Passing the handle in as an argument makes the connection explicit
//! and impossible to get wrong:
//!
//! ```ignore
//! RowAction::run("Reset two-factor", Icon::ShieldOff, |user, grid| {
//!     spawn_local(async move {
//!         match reset_user_mfa(user.id).await {
//!             Ok(_) => { grid.report("Two-factor removed."); grid.refresh(); }
//!             Err(err) => grid.warn(err.to_string()),
//!         }
//!     });
//! })
//! ```
//!
//! # Refreshing is the grid's job, not the caller's
//!
//! An action that changes a row leaves the table showing the row as it was.
//! [`GridHandle::refresh`] re-runs whatever the source is - one fetch for an
//! in-memory grid, one page for a paged one - so a configuration never has to
//! know which kind it is attached to.

use leptos::prelude::*;

use crate::components::page::Tone;

/// A sentence the grid shows above the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridNotice {
    pub message: String,
    pub tone: Tone,
}

/// A grid, as seen from inside one of its actions. Cheap to copy.
#[derive(Clone, Copy)]
pub struct GridHandle {
    pub(crate) refetch: Callback<()>,
    pub(crate) notice: RwSignal<Option<GridNotice>>,
}

impl GridHandle {
    /// Fetch the rows again. Call after anything that changed one.
    pub fn refresh(self) {
        self.refetch.run(());
    }

    /// Say that something worked.
    pub fn report(self, message: impl Into<String>) {
        self.notice.set(Some(GridNotice {
            message: message.into(),
            tone: Tone::Success,
        }));
    }

    /// Say that something did not.
    ///
    /// Takes the server's own words rather than a house phrase: "you may not
    /// remove the owner's second factor" is worth more than "something went
    /// wrong", and the service has already written it.
    pub fn warn(self, message: impl Into<String>) {
        self.notice.set(Some(GridNotice {
            message: message.into(),
            tone: Tone::Danger,
        }));
    }

    /// Take the message down.
    pub fn clear(self) {
        self.notice.set(None);
    }
}
