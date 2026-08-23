//! Uploading a file, and everything that happens to it afterwards.
//!
//! ```text
//!   request                                  worker
//!   ───────                                  ──────
//!   authorise_upload   permission, bucket
//!         │
//!   storage.begin      bytes stream to _quarantine/<id>.part
//!         │
//!   record_upload      row: status = received  ─────────┐
//!         │                                             │
//!   202 Accepted, here is the id                        │
//!                                                       v
//!                                         verify   claim the row
//!                                             │    read the head
//!                                             │    inspect: what is it, may it stay
//!                                             │    hash the whole of it
//!                                             │    promote out of quarantine
//!                                             v
//!                                    row: stored | rejected     + outbox event
//! ```
//!
//! # Why the request does so little
//!
//! Everything worth doing to an uploaded file has to survive the connection
//! dropping. Deciding what it is, hashing it, moving it, and one day scanning
//! it, are all work that must not be lost because somebody closed a laptop -
//! and none of it is work the person uploading is waiting on. So the request
//! writes bytes and a row and stops, and the row is the queue.
//!
//! That also gives one answer to a question that otherwise has none: what
//! happens when the process dies mid-upload? The bytes are in quarantine under
//! a name nothing points at, the row does not exist, and the sweeper removes
//! them. There is no state to reconcile because there is no second place the
//! truth could be.
//!
//! # The three places a file can be
//!
//! * **Quarantine** - arrived, not yet looked at. Nothing serves from here.
//! * **A bucket** - verified, renamed, and the only place a download reads.
//! * **Gone** - refused, or deleted. The object is removed before the row is
//!   updated, so a row never points at bytes that are not there.

pub mod access;
pub mod upload;
pub mod verify;

pub use access::{
    clear_avatar, clear_logo, delete_file, list, open_for_download, set_avatar, set_logo, summary,
};
pub use upload::{UploadTicket, authorise_upload, discard, record_upload};
pub use verify::verify;

use phonix_storage::{FileStorage, NamingStrategy};

/// What this module needs from the outside world.
///
/// The same shape and the same reason as [`crate::Security`]: a use case takes
/// one parameter instead of three, and adding a dependency later - a virus
/// scanner, a thumbnailer - does not change every signature in the module.
///
/// Both fields are trait objects on purpose. This layer must not be able to
/// name a filesystem path or a naming convention, because the day the backend
/// becomes an object store is the day every place that could name one has to
/// be found again.
#[derive(Clone, Copy)]
pub struct Files<'a> {
    pub storage: &'a dyn FileStorage,
    pub naming: &'a dyn NamingStrategy,
}

impl std::fmt::Debug for Files<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files")
            .field("storage", &self.storage.describe())
            .field("naming", &self.naming.describe())
            .finish()
    }
}
