//! Files on the disk this process can see.
//!
//! The default backend, and the only one so far. Objects live under a
//! configured root, one directory per tenant:
//!
//! ```text
//!   resources/uploads/
//!     acme/
//!       _quarantine/0199c4f2e1a37b8d9e5f0a1b2c3d4e5f.part
//!       attachments/2026/08/0199c4f2e1a37b8d9e5f0a1b2c3d4e5f.pdf
//!       avatars/2026/08/0199c51a....png
//!     globex/
//!       ...
//! ```
//!
//! # Every write is atomic
//!
//! Bytes go to a temporary file beside the destination, are flushed to the
//! platter, and are then renamed into place. A rename within one directory is
//! atomic on every filesystem this runs on, so an object at its key is always a
//! complete object - there is no window in which a reader can see half of one,
//! and a process killed mid-upload leaves a `.tmp` file rather than a truncated
//! document.
//!
//! The `sync_all` before the rename is what makes that true after a power
//! cut rather than merely after a crash: without it the rename can reach the
//! disk before the contents do.
//!
//! # Why the root is canonicalised once
//!
//! So that "is this path inside the root" is a question about two absolute
//! paths and not about how many `..` segments cancel out. Keys cannot contain
//! `..` at all - see [`crate::key`] - and the check in [`LocalDisk::path_for`]
//! is the second lock on that door, not the first.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncWriteExt};

use crate::key::StorageKey;
use crate::{FileStorage, ObjectStat, ObjectWriter, StorageError, StorageResult, hex};

/// Objects on a local filesystem.
#[derive(Debug, Clone)]
pub struct LocalDisk {
    root: PathBuf,
}

impl LocalDisk {
    /// Open a storage root, creating it if it is not there.
    ///
    /// Creating rather than requiring: a fresh checkout has no
    /// `resources/uploads`, and refusing to start over a directory the process
    /// is about to make would be a rule with no purpose.
    pub async fn open(root: impl Into<PathBuf>) -> StorageResult<Self> {
        let requested = root.into();

        fs::create_dir_all(&requested)
            .await
            .map_err(|source| StorageError::io("creating the storage root", source))?;

        // Resolved once, here, so every later containment check compares two
        // absolute paths. On Windows this yields a `\\?\` prefixed path, which
        // is only ever used internally.
        let root = fs::canonicalize(&requested)
            .await
            .map_err(|source| StorageError::io("resolving the storage root", source))?;

        tracing::info!(root = %root.display(), "local file storage ready");

        Ok(Self { root })
    }

    /// The root every object is under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a key lands on disk.
    ///
    /// The key's segments are already known to be free of `..`, of separators
    /// and of anything a filesystem reads specially, so this is a join. The
    /// component check afterwards is belt and braces: it costs one pass over a
    /// short path, and what it guards against is somebody later adding a
    /// constructor to `StorageKey` that skips validation.
    fn path_for(&self, key: &StorageKey) -> StorageResult<PathBuf> {
        let mut path = self.root.clone();
        path.push(key.tenant());

        for segment in key.segments() {
            path.push(segment);
        }

        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| StorageError::InvalidKey(crate::InvalidStorageKey::Traversal))?;

        let all_ordinary = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));

        if !all_ordinary {
            return Err(StorageError::InvalidKey(
                crate::InvalidStorageKey::Traversal,
            ));
        }

        Ok(path)
    }

    async fn ensure_parent(path: &Path) -> StorageResult<()> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };

        fs::create_dir_all(parent)
            .await
            .map_err(|source| StorageError::io("creating an object's directory", source))
    }

    fn not_found(key: &StorageKey) -> StorageError {
        StorageError::NotFound {
            key: key.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl FileStorage for LocalDisk {
    fn describe(&self) -> String {
        format!("local disk at {}", self.root.display())
    }

    async fn begin(&self, key: &StorageKey, limit: u64) -> StorageResult<Box<dyn ObjectWriter>> {
        let target = self.path_for(key)?;
        Self::ensure_parent(&target).await?;

        // Beside the destination, so the rename that publishes it is within one
        // directory and therefore atomic. A shared temp directory would put the
        // two on different filesystems on some deployments, and a cross-device
        // rename is a copy - which is not atomic at all.
        let temp = target.with_extension(format!("{}.tmp", uuid::Uuid::now_v7().simple()));

        let file = fs::File::create(&temp)
            .await
            .map_err(|source| StorageError::io("creating a temporary object file", source))?;

        Ok(Box::new(DiskWriter {
            temp,
            target,
            file: Some(file),
            hasher: Sha256::new(),
            written: 0,
            limit,
        }))
    }

    async fn open(&self, key: &StorageKey) -> StorageResult<Box<dyn AsyncRead + Send + Unpin>> {
        let path = self.path_for(key)?;

        match fs::File::open(&path).await {
            Ok(file) => Ok(Box::new(file)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Err(Self::not_found(key))
            }
            Err(source) => Err(StorageError::io("opening an object", source)),
        }
    }

    async fn stat(&self, key: &StorageKey) -> StorageResult<ObjectStat> {
        let path = self.path_for(key)?;

        let metadata = match fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(Self::not_found(key));
            }
            Err(source) => return Err(StorageError::io("reading object metadata", source)),
        };

        // A directory at an object's key is not an object. Reporting its size
        // would let a caller believe there were bytes to read.
        if !metadata.is_file() {
            return Err(Self::not_found(key));
        }

        Ok(ObjectStat {
            byte_size: metadata.len(),
            checksum_sha256: None,
            modified: metadata.modified().ok().map(chrono::DateTime::from),
        })
    }

    async fn delete(&self, key: &StorageKey) -> StorageResult<bool> {
        let path = self.path_for(key)?;

        match fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            // Already gone is the outcome the caller wanted, not a failure.
            // Deleting is the last step of several paths, and half of them run
            // after something else has already cleaned up.
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(StorageError::io("deleting an object", source)),
        }
    }

    async fn promote(&self, from: &StorageKey, to: &StorageKey) -> StorageResult<ObjectStat> {
        let source_path = self.path_for(from)?;
        let target_path = self.path_for(to)?;
        Self::ensure_parent(&target_path).await?;

        match fs::rename(&source_path, &target_path).await {
            Ok(()) => {}

            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(Self::not_found(from));
            }

            // A rename across filesystems is refused rather than performed.
            // That happens when quarantine and the buckets are separate mounts,
            // which is a reasonable thing for an operator to do - so it falls
            // back to a copy rather than failing the upload.
            Err(_) => {
                tracing::debug!(
                    from = %from,
                    to = %to,
                    "rename refused; copying across filesystems instead"
                );

                fs::copy(&source_path, &target_path)
                    .await
                    .map_err(|source| StorageError::io("copying an object", source))?;

                // Only after the copy has succeeded. The other order loses the
                // file if the copy fails.
                if let Err(source) = fs::remove_file(&source_path).await {
                    tracing::warn!(
                        key = %from,
                        error = %source,
                        "copied an object but could not remove the original"
                    );
                }
            }
        }

        self.stat(to).await
    }
}

/// One object being written to a temporary file.
struct DiskWriter {
    temp: PathBuf,
    target: PathBuf,
    /// `None` once the file has been closed, which has to happen before the
    /// rename: Windows refuses to move a file that is still open.
    file: Option<fs::File>,
    hasher: Sha256,
    written: u64,
    limit: u64,
}

#[async_trait::async_trait]
impl ObjectWriter for DiskWriter {
    async fn write(&mut self, chunk: &[u8]) -> StorageResult<()> {
        let Some(file) = self.file.as_mut() else {
            return Err(StorageError::io(
                "writing to an object that was already finished",
                std::io::Error::from(std::io::ErrorKind::BrokenPipe),
            ));
        };

        // Checked before the write, not after: the limit is a promise about
        // what reaches the disk, and a chunk that would break it must not be
        // written first and regretted afterwards.
        let total = self.written.saturating_add(chunk.len() as u64);
        if total > self.limit {
            return Err(StorageError::LimitExceeded { limit: self.limit });
        }

        file.write_all(chunk)
            .await
            .map_err(|source| StorageError::io("writing object bytes", source))?;

        // Hashed here rather than in a second pass. The bytes are in cache
        // already, so the digest costs the hash and nothing else.
        self.hasher.update(chunk);
        self.written = total;

        Ok(())
    }

    async fn finish(mut self: Box<Self>) -> StorageResult<ObjectStat> {
        if let Some(mut file) = self.file.take() {
            file.flush()
                .await
                .map_err(|source| StorageError::io("flushing an object", source))?;

            // Contents to the platter before the rename that publishes them.
            // Without this the name can outlive a power cut and the bytes not.
            file.sync_all()
                .await
                .map_err(|source| StorageError::io("syncing an object", source))?;
        }

        fs::rename(&self.temp, &self.target)
            .await
            .map_err(|source| StorageError::io("publishing an object", source))?;

        Ok(ObjectStat {
            byte_size: self.written,
            checksum_sha256: Some(hex(&self.hasher.finalize())),
            modified: Some(chrono::Utc::now()),
        })
    }

    async fn abort(mut self: Box<Self>) {
        drop(self.file.take());

        if let Err(err) = fs::remove_file(&self.temp).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            // Logged and not returned: this runs on a path that is already
            // failing for a reason the caller cares about more than this one.
            tracing::warn!(
                path = %self.temp.display(),
                error = %err,
                "could not remove a partial upload"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::TenantSlug;

    use super::*;

    async fn disk() -> (LocalDisk, PathBuf) {
        let root = std::env::temp_dir()
            .join("phonix-storage-tests")
            .join(uuid::Uuid::now_v7().simple().to_string());

        let disk = LocalDisk::open(&root).await.unwrap();
        (disk, root)
    }

    fn key(segments: &[&str]) -> StorageKey {
        let tenant = TenantSlug::parse("acme").unwrap();
        StorageKey::new(&tenant, segments).unwrap()
    }

    #[tokio::test]
    async fn a_written_object_reads_back_with_its_digest() {
        let (disk, _root) = disk().await;
        let key = key(&["attachments", "2026", "08", "a.txt"]);

        let stat = disk.put_bytes(&key, b"hello storage").await.unwrap();

        assert_eq!(stat.byte_size, 13);

        let written = stat
            .checksum_sha256
            .expect("a finished write knows its digest");
        assert_eq!(written.len(), 64);
        // The digest computed while writing must agree with one taken by
        // reading the object back. If it did not, deduplication would be
        // quietly wrong and nothing would say so.
        assert_eq!(disk.digest(&key).await.unwrap(), written);

        assert_eq!(disk.read_head(&key, 5).await.unwrap(), b"hello");
        assert!(disk.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn tenants_land_in_separate_directories() {
        let (disk, root) = disk().await;

        let acme =
            StorageKey::new(&TenantSlug::parse("acme").unwrap(), &["avatars", "a.png"]).unwrap();
        let globex =
            StorageKey::new(&TenantSlug::parse("globex").unwrap(), &["avatars", "a.png"]).unwrap();

        disk.put_bytes(&acme, b"one").await.unwrap();
        disk.put_bytes(&globex, b"two").await.unwrap();

        // The same key below the tenant, and two different objects - which is
        // the whole reason the tenant is part of the key rather than part of a
        // WHERE clause.
        assert_eq!(disk.read_head(&acme, 16).await.unwrap(), b"one");
        assert_eq!(disk.read_head(&globex, 16).await.unwrap(), b"two");

        assert!(root.join("acme").join("avatars").join("a.png").exists());
        assert!(root.join("globex").join("avatars").join("a.png").exists());
    }

    #[tokio::test]
    async fn the_limit_stops_the_bytes_reaching_the_disk() {
        let (disk, _root) = disk().await;
        let key = key(&["attachments", "big.bin"]);

        let mut writer = disk.begin(&key, 8).await.unwrap();
        writer.write(b"12345").await.unwrap();

        let outcome = writer.write(b"678901").await;
        assert!(matches!(
            outcome,
            Err(StorageError::LimitExceeded { limit: 8 })
        ));

        writer.abort().await;
        // Nothing was published, and nothing was left behind either.
        assert!(!disk.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn an_aborted_write_leaves_nothing_behind() {
        let (disk, root) = disk().await;
        let key = key(&["attachments", "abandoned.bin"]);

        let mut writer = disk.begin(&key, 1024).await.unwrap();
        writer.write(b"partial").await.unwrap();
        writer.abort().await;

        assert!(!disk.exists(&key).await.unwrap());

        let directory = root.join("acme").join("attachments");
        let mut entries = fs::read_dir(&directory).await.unwrap();
        let mut left = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            left.push(entry.file_name());
        }
        assert!(left.is_empty(), "a temporary file survived: {left:?}");
    }

    #[tokio::test]
    async fn promotion_moves_an_object_out_of_quarantine() {
        let (disk, _root) = disk().await;
        let held = key(&[crate::QUARANTINE, "0199.part"]);
        let stored = key(&["attachments", "2026", "08", "0199.pdf"]);

        disk.put_bytes(&held, b"%PDF-1.7 ...").await.unwrap();
        let stat = disk.promote(&held, &stored).await.unwrap();

        assert_eq!(stat.byte_size, 12);
        assert!(disk.exists(&stored).await.unwrap());
        // The original must be gone: a copy left in quarantine is a file that
        // was never verified, sitting where nothing will ever look at it again.
        assert!(!disk.exists(&held).await.unwrap());
    }

    #[tokio::test]
    async fn a_missing_object_is_not_found_rather_than_an_io_error() {
        let (disk, _root) = disk().await;
        let key = key(&["attachments", "nothing.pdf"]);

        assert!(matches!(
            disk.stat(&key).await,
            Err(StorageError::NotFound { .. })
        ));
        assert!(matches!(
            disk.open(&key).await,
            Err(StorageError::NotFound { .. })
        ));
        let elsewhere = self::key(&["attachments", "x.pdf"]);
        assert!(matches!(
            disk.promote(&key, &elsewhere).await,
            Err(StorageError::NotFound { .. })
        ));

        // Deleting something that is already gone is the outcome the caller
        // asked for.
        assert!(!disk.delete(&key).await.unwrap());
        assert!(!disk.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn a_directory_is_not_an_object() {
        let (disk, _root) = disk().await;
        let file = key(&["attachments", "2026", "a.txt"]);
        disk.put_bytes(&file, b"x").await.unwrap();

        // "attachments/2026" exists on disk and is not a file. Reporting its
        // size would tell a caller there were bytes to read.
        let directory = key(&["attachments", "2026"]);
        assert!(matches!(
            disk.stat(&directory).await,
            Err(StorageError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn every_path_stays_under_the_root() {
        let (disk, _requested) = disk().await;
        // The canonical root, not the one that was asked for: on Windows those
        // differ by a `\?\` prefix, and comparing against the wrong one would
        // make this test fail on every path rather than on an escaping one.
        let root = disk.root().to_path_buf();

        for segments in [
            vec!["attachments", "a.txt"],
            vec![crate::QUARANTINE, "b.part"],
            vec!["avatars", "2026", "08", "c.png"],
        ] {
            let key = key(&segments);
            disk.put_bytes(&key, b"x").await.unwrap();

            let path = disk.path_for(&key).unwrap();
            assert!(
                path.starts_with(&root) || path.canonicalize().unwrap().starts_with(&root),
                "{} escaped the root",
                path.display()
            );
        }
    }
}
