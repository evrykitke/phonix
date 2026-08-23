//! Where uploaded bytes live.
//!
//! One trait, [`FileStorage`], and one implementation of it - [`LocalDisk`],
//! which writes under a configured root. A second backend is a second file in
//! this crate and nothing else anywhere: the rest of the application holds a
//! `Arc<dyn FileStorage>` and speaks in [`StorageKey`]s, so it has no way to
//! notice which one it has.
//!
//! ```text
//!   phonix-services        "verify this upload and put it away"
//!         |
//!         v
//!   phonix-storage         keys, naming, inspection, the trait   <- here
//!         |
//!         v
//!   LocalDisk              resources/uploads/<tenant>/...
//! ```
//!
//! # The three things this layer guarantees
//!
//! 1. **Every object is inside exactly one tenant's area.** Not by convention -
//!    a [`StorageKey`] cannot be constructed without a validated
//!    [`TenantSlug`](phonix_core::TenantSlug) as its first segment, and every
//!    key is re-validated when it is read back out of a row.
//! 2. **No path is ever built from something a caller supplied.** The name a
//!    file arrives with is kept as data and never becomes a path component; see
//!    [`naming`].
//! 3. **A file is not stored until it has been looked at.** Bytes land in
//!    [`key::QUARANTINE`], and only a verified file is promoted out of it. See
//!    [`inspect`].
//!
//! # Writing is streaming, not buffering
//!
//! [`FileStorage::begin`] hands back an [`ObjectWriter`] that takes chunks. A
//! 25 MB attachment is never held in memory in one piece, the byte ceiling is
//! enforced as the bytes arrive rather than after they have all been accepted,
//! and the SHA-256 is computed on the way past - so hashing costs one pass over
//! data that was being copied anyway.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

pub mod inspect;
pub mod key;
pub mod local;
pub mod naming;

pub use inspect::{Inspection, inspect};
pub use key::{InvalidStorageKey, QUARANTINE, StorageKey};
pub use local::LocalDisk;
pub use naming::{ContentAddressed, DateSharded, Flat, NamingContext, NamingStrategy};

use chrono::{DateTime, Utc};
use tokio::io::{AsyncRead, AsyncReadExt};

pub type StorageResult<T> = Result<T, StorageError>;

/// What is known about a stored object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStat {
    pub byte_size: u64,
    /// Lowercase hex SHA-256.
    ///
    /// `Some` from [`ObjectWriter::finish`], which hashes as it writes.
    /// `None` from [`FileStorage::stat`], which would have to read the whole
    /// object again to know - and a caller that needs it should say so by
    /// asking for [`FileStorage::digest`] rather than being charged for it
    /// every time it wants a size.
    pub checksum_sha256: Option<String>,
    pub modified: Option<DateTime<Utc>>,
}

impl ObjectStat {
    pub fn new(byte_size: u64) -> Self {
        Self {
            byte_size,
            checksum_sha256: None,
            modified: None,
        }
    }
}

/// An object being written.
///
/// Chunk at a time, with the ceiling checked on each one. The three-way ending
/// is deliberate: [`Self::finish`] publishes the object, [`Self::abort`]
/// removes the partial work, and dropping the writer without either is a bug
/// that leaves a temporary file behind - which is why both consume the box.
#[async_trait::async_trait]
pub trait ObjectWriter: Send {
    /// Append bytes.
    ///
    /// Returns [`StorageError::LimitExceeded`] as soon as the total passes the
    /// limit the writer was opened with, so a caller streaming from a socket
    /// stops reading rather than finding out at the end.
    async fn write(&mut self, chunk: &[u8]) -> StorageResult<()>;

    /// Make the object visible under its key, atomically.
    async fn finish(self: Box<Self>) -> StorageResult<ObjectStat>;

    /// Throw away what has been written.
    ///
    /// Infallible on purpose: it is called on a path that is already failing,
    /// and a caller cannot do anything useful with a second error. A backend
    /// that cannot clean up logs it.
    async fn abort(self: Box<Self>);
}

/// Somewhere bytes can be put and got back.
#[async_trait::async_trait]
pub trait FileStorage: Send + Sync + 'static {
    /// What this backend is, for one line in the startup log.
    fn describe(&self) -> String;

    /// Start writing an object.
    ///
    /// `limit` is a hard ceiling in bytes; passing it is an error rather than a
    /// truncation, because a truncated upload that looked successful would be
    /// a corrupt file nobody was told about.
    async fn begin(&self, key: &StorageKey, limit: u64) -> StorageResult<Box<dyn ObjectWriter>>;

    /// Read an object as a stream.
    async fn open(&self, key: &StorageKey) -> StorageResult<Box<dyn AsyncRead + Send + Unpin>>;

    async fn stat(&self, key: &StorageKey) -> StorageResult<ObjectStat>;

    async fn delete(&self, key: &StorageKey) -> StorageResult<bool>;

    /// Move an object from one key to another within this backend.
    ///
    /// How a file leaves quarantine. Named for what it is used for rather than
    /// `rename`, because on a local disk it is a rename and on an object store
    /// it is a copy followed by a delete - and the caller must not care which.
    async fn promote(&self, from: &StorageKey, to: &StorageKey) -> StorageResult<ObjectStat>;

    // -- provided ---------------------------------------------------------

    async fn exists(&self, key: &StorageKey) -> StorageResult<bool> {
        match self.stat(key).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound { .. }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Write an object that is already in memory.
    ///
    /// For the small things - a generated thumbnail, a test fixture. Anything
    /// arriving from a caller goes through [`Self::begin`] instead, because
    /// buffering it first is exactly what the ceiling exists to prevent.
    async fn put_bytes(&self, key: &StorageKey, bytes: &[u8]) -> StorageResult<ObjectStat> {
        let mut writer = self.begin(key, bytes.len() as u64).await?;

        if let Err(err) = writer.write(bytes).await {
            writer.abort().await;
            return Err(err);
        }

        writer.finish().await
    }

    /// The first `max_bytes` of an object.
    ///
    /// What inspection reads. Bounded because the whole point is to decide what
    /// a file is without loading it: a header is a few hundred bytes, and the
    /// generous ceiling here exists only for JPEG, whose frame header can sit
    /// behind a camera's embedded thumbnail.
    async fn read_head(&self, key: &StorageKey, max_bytes: usize) -> StorageResult<Vec<u8>> {
        let reader = self.open(key).await?;
        let mut limited = reader.take(max_bytes as u64);
        let mut buffer = Vec::with_capacity(max_bytes.min(64 * 1024));

        limited
            .read_to_end(&mut buffer)
            .await
            .map_err(|source| StorageError::io("reading the head of an object", source))?;

        Ok(buffer)
    }

    /// SHA-256 of an object, in lowercase hex.
    ///
    /// A full pass over the bytes, which is why it is a method a caller asks
    /// for rather than something [`Self::stat`] includes.
    async fn digest(&self, key: &StorageKey) -> StorageResult<String> {
        use sha2::{Digest, Sha256};

        let mut reader = self.open(key).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];

        loop {
            let read = reader
                .read(&mut buffer)
                .await
                .map_err(|source| StorageError::io("hashing an object", source))?;

            if read == 0 {
                break;
            }

            match buffer.get(..read) {
                Some(chunk) => hasher.update(chunk),
                // Unreachable: `read` is what the reader just wrote into this
                // buffer. Asking rather than indexing keeps it that way.
                None => break,
            }
        }

        Ok(hex(&hasher.finalize()))
    }
}

/// Bytes as lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    bytes.iter().fold(String::new(), |mut out, byte| {
        // Writing into a String cannot fail; the result is discarded rather
        // than unwrapped so this crate keeps its deny on `unwrap`.
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("no object at '{key}'")]
    NotFound { key: String },

    #[error("object exceeds the {limit} byte limit")]
    LimitExceeded { limit: u64 },

    #[error(transparent)]
    InvalidKey(#[from] InvalidStorageKey),

    #[error("storage failed while {context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    /// The backend cannot do this at all - not a failure, an absence.
    #[error("this storage backend cannot {0}")]
    Unsupported(&'static str),
}

impl StorageError {
    pub(crate) fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }

    /// Whether trying the same operation again could succeed.
    ///
    /// What the job runner asks before deciding between a retry and a dead
    /// letter. An I/O error is usually a full disk or an unreachable mount,
    /// both of which are somebody else's to fix and both of which get fixed; a
    /// bad key or a missing object will be just as bad in five minutes.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Io { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_lowercase_and_zero_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa9]), "000fffa9");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn only_the_failures_worth_retrying_are_retryable() {
        assert!(
            StorageError::io(
                "writing",
                std::io::Error::new(std::io::ErrorKind::StorageFull, "no space")
            )
            .is_retryable()
        );

        // Retrying these produces the same answer, and a retry loop around
        // them is a queue that never drains.
        assert!(!StorageError::NotFound { key: "a/b".into() }.is_retryable());
        assert!(!StorageError::LimitExceeded { limit: 10 }.is_retryable());
        assert!(!StorageError::InvalidKey(InvalidStorageKey::Traversal).is_retryable());
    }
}
