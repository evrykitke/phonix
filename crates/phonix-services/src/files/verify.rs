//! The job: deciding what an uploaded file is, and putting it away.
//!
//! Runs against a row already claimed by a worker. It reads the head of the
//! quarantined object, applies the acceptance policy, hashes the whole of it,
//! and either promotes it into its bucket or removes it - and in both cases
//! writes the outcome and an event **in one transaction**.
//!
//! # Two rules that decide the order of everything here
//!
//! **A row never points at bytes that are not there.** So the object moves
//! first and the row is updated second, and on the refusal path the object is
//! deleted before the row is marked. A crash between the two leaves an orphan
//! object, which the quarantine sweeper collects; the other order would leave a
//! row that says "stored" and a download that 404s, which nothing collects.
//!
//! **The event commits with the change it describes.** The outbox insert is in
//! the same transaction as the status update, so there is no state in which the
//! file is stored and nobody was told, or told and not stored. See
//! [`phonix_db::outbox`].
//!
//! # Retrying is safe because naming is deterministic
//!
//! A second attempt computes the same destination key as the first - the
//! naming strategy is a pure function of the row and the detected type, and it
//! is given `created_at` rather than the clock for exactly that reason. So a
//! retry overwrites its own partial work rather than leaving a second copy.

use chrono::Utc;
use phonix_core::TenantSlug;
use phonix_core::files::{self, Rejection, UploadResult, UploadStatus};
use phonix_db::PgPool;
use phonix_db::files::{self as files_db, FileRow, StoredFile};
use phonix_db::outbox;
use phonix_storage::inspect::{self, HEAD_BYTES};
use phonix_storage::{NamingContext, StorageKey};

use crate::error::{ServiceError, ServiceResult};

use super::Files;

/// Verify one claimed upload and reach a terminal state.
///
/// Returns the [`UploadResult`] that was published. An `Err` means the work
/// could not be done - not that the file was refused, which comes back as an
/// `Ok` whose status is `rejected`. The distinction is the whole reason
/// `rejected` and `failed` are different states: one is an answer about the
/// file, the other is an apology about us.
pub async fn verify(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    row: &FileRow,
) -> ServiceResult<UploadResult> {
    let quarantine_key = quarantine_key_of(tenant, row)?;

    // The size the object actually has, not the one the request said it was
    // sending and not the one recorded at insert. Everything downstream - the
    // limit check, the stored row - reads from here.
    let stat = files_ctx.storage.stat(&quarantine_key).await?;

    let Some(bucket) = files::bucket(&row.bucket) else {
        // Reachable only if a bucket was removed from the code while an upload
        // aimed at it was in flight. It is a refusal rather than a failure: the
        // file has nowhere to go and never will.
        return refuse(
            pool,
            files_ctx,
            row,
            &quarantine_key,
            Rejection::UnknownBucket {
                requested: row.bucket.clone(),
            },
            None,
        )
        .await;
    };

    let head = files_ctx
        .storage
        .read_head(&quarantine_key, HEAD_BYTES)
        .await?;

    let inspection = match inspect::inspect(
        bucket,
        &row.original_name,
        row.declared_content_type.as_deref(),
        &head,
        stat.byte_size,
    ) {
        Ok(inspection) => inspection,
        Err(rejection) => {
            return refuse(pool, files_ctx, row, &quarantine_key, rejection, None).await;
        }
    };

    // A full pass over the bytes. Worth it once: this is the deduplication
    // handle and the only later proof that a stored file has not been altered.
    let checksum = files_ctx.storage.digest(&quarantine_key).await?;

    let segments = files_ctx.naming.segments(&NamingContext {
        bucket: bucket.name,
        file_id: row.id,
        extension: inspection.extension(),
        checksum: Some(&checksum),
        // The row's own timestamp, not the clock. That is what makes a retry
        // compute the destination the first attempt computed.
        at: row.created_at,
    });

    let storage_key = quarantine_key
        .sibling(&segments)
        .map_err(|err| ServiceError::Storage(err.into()))?;

    let stored = files_ctx
        .storage
        .promote(&quarantine_key, &storage_key)
        .await?;

    let result = UploadResult {
        file_id: row.id,
        bucket: bucket.name.to_owned(),
        status: UploadStatus::Stored,
        original_name: row.original_name.clone(),
        stored_name: Some(storage_key.file_name().to_owned()),
        storage_key: Some(storage_key.as_str().to_owned()),
        content_type: Some(inspection.mime().to_owned()),
        category: Some(inspection.category()),
        byte_size: stored.byte_size,
        checksum_sha256: Some(checksum.clone()),
        rejection: None,
        uploaded_by: row.uploaded_by,
        occurred_at: Utc::now(),
    };

    let write = commit_stored(
        pool,
        row,
        StoredFile {
            storage_key: storage_key.as_str(),
            stored_name: storage_key.file_name(),
            content_type: inspection.mime(),
            category: inspection.category(),
            checksum_sha256: &checksum,
            byte_size: stored.byte_size,
        },
        &result,
    )
    .await;

    if let Err(err) = write {
        // The object moved and the row did not. Put it back, so the retry finds
        // the world as the first attempt found it: without this the next
        // attempt looks in quarantine, finds nothing, and gives up on a file
        // that is sitting perfectly intact one directory away.
        tracing::error!(
            file_id = %row.id,
            error = %err,
            "could not record a promoted upload; returning it to quarantine"
        );

        if let Err(undo) = files_ctx
            .storage
            .promote(&storage_key, &quarantine_key)
            .await
        {
            // Both halves failed. Nothing here can fix it, and the loudest
            // possible log line is the only useful thing left to do: the object
            // is at `storage_key` and the row still says it is in quarantine.
            tracing::error!(
                file_id = %row.id,
                storage_key = %storage_key,
                error = %undo,
                "an upload is stranded outside quarantine with no row pointing at it"
            );
        }

        return Err(err);
    }

    tracing::info!(
        file_id = %row.id,
        bucket = bucket.name,
        content_type = inspection.mime(),
        bytes = stored.byte_size,
        "upload stored"
    );

    Ok(result)
}

/// Refuse a file: remove the bytes, record why, and say so.
///
/// The object goes first. A refused file is not kept - there is no state in
/// which this application holds bytes it has decided it will not store - and
/// deleting before the row is marked means a crash in between leaves an orphan
/// object rather than a row pointing at one that is gone.
async fn refuse(
    pool: &PgPool,
    files_ctx: Files<'_>,
    row: &FileRow,
    quarantine_key: &StorageKey,
    rejection: Rejection,
    detected_type: Option<&str>,
) -> ServiceResult<UploadResult> {
    if let Err(err) = files_ctx.storage.delete(quarantine_key).await {
        // Logged rather than returned. The refusal is the answer, and failing
        // to tidy up must not turn it into a retry that refuses the file again.
        tracing::warn!(
            file_id = %row.id,
            error = %err,
            "could not remove a refused upload; the sweeper will collect it"
        );
    }

    let result = UploadResult {
        file_id: row.id,
        bucket: row.bucket.clone(),
        status: UploadStatus::Rejected,
        original_name: row.original_name.clone(),
        stored_name: None,
        storage_key: None,
        content_type: detected_type.map(str::to_owned),
        category: None,
        byte_size: row.byte_size,
        checksum_sha256: None,
        rejection: Some(rejection.clone()),
        uploaded_by: row.uploaded_by,
        occurred_at: Utc::now(),
    };

    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;

    files_db::mark_rejected(&mut *tx, row.id, &rejection, detected_type).await?;
    outbox::enqueue(&mut *tx, UploadResult::ROUTING_KEY, &result).await?;

    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    tracing::info!(
        file_id = %row.id,
        reason = rejection.code(),
        "upload refused"
    );

    Ok(result)
}

/// Record a stored file and its event together.
async fn commit_stored(
    pool: &PgPool,
    row: &FileRow,
    stored: StoredFile<'_>,
    result: &UploadResult,
) -> ServiceResult<()> {
    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;

    files_db::mark_stored(&mut *tx, row.id, stored).await?;
    // In the same transaction as the status change. Publishing outside it is
    // the bug the outbox exists to prevent - see `phonix_db::outbox`.
    outbox::enqueue(&mut *tx, UploadResult::ROUTING_KEY, result).await?;

    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    Ok(())
}

/// The quarantine object this row names, checked against the tenant it is being
/// read for.
///
/// `parse_for` rather than `parse`: the key comes out of a row, and a row is
/// older than a request rather than more trustworthy than one. A key naming
/// another tenant's area is refused here rather than opened.
fn quarantine_key_of(tenant: &TenantSlug, row: &FileRow) -> ServiceResult<StorageKey> {
    let raw = row
        .quarantine_key
        .as_deref()
        // A row with no quarantine key has already reached a terminal state, so
        // there is nothing to verify. The claim query cannot return one, which
        // makes this a wiring error rather than a race.
        .ok_or(ServiceError::NotFound("quarantined upload"))?;

    StorageKey::parse_for(tenant, raw).map_err(|err| ServiceError::Storage(err.into()))
}

/// Whether a failed attempt is worth another one.
///
/// The runner asks this before deciding between putting a row back on the queue
/// and giving up on it. A full disk gets fixed; a key that will not parse will
/// not parse next time either, and retrying it is a queue that never drains.
pub fn is_retryable(err: &ServiceError) -> bool {
    match err {
        ServiceError::Storage(storage) => storage.is_retryable(),
        // The database was unreachable or the transaction lost a race. Both are
        // exactly what a retry is for.
        ServiceError::Db(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use phonix_storage::StorageError;

    use super::*;

    #[test]
    fn only_the_failures_worth_retrying_are_retried() {
        assert!(is_retryable(&ServiceError::Db(phonix_db::DbError::Query(
            sqlx::Error::PoolTimedOut
        ))));
        assert!(is_retryable(&ServiceError::Storage(StorageError::Io {
            context: "writing object bytes",
            source: std::io::Error::new(std::io::ErrorKind::StorageFull, "no space"),
        })));

        // The bytes are gone. They will still be gone in five minutes, and a
        // row that retries for ever is a queue that never drains.
        assert!(!is_retryable(&ServiceError::Storage(
            StorageError::NotFound {
                key: "acme/x".into()
            }
        )));
        assert!(!is_retryable(&ServiceError::NotFound("quarantined upload")));
        assert!(!is_retryable(&ServiceError::Unauthenticated));
    }
}
