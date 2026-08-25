//! The `file_uploads` table: uploads in flight, and the files they became.
//!
//! One table serving two purposes, which is deliberate - see the header comment
//! on migration `0009_files.sql` for why the job and the file share a row and
//! an id.
//!
//! # This module is also the job queue
//!
//! [`claim_batch`] and [`claim_one`] are `SELECT ... FOR UPDATE SKIP LOCKED`
//! against the partial index on outstanding work. That is the whole of the
//! dispatch mechanism, and it is worth being explicit about why it is here
//! rather than in RabbitMQ:
//!
//! * The row is written either way, so the queue costs nothing extra.
//! * Claiming a job and changing its state are then **one transaction**. With a
//!   broker they are two, and everything that goes wrong with background work
//!   lives in the gap between them.
//! * `SKIP LOCKED` is what makes several workers safe without a lock table: a
//!   row another worker holds is passed over rather than waited for.
//!
//! The broker still carries the *result* outward, through the outbox - see
//! [`crate::outbox`]. What it does not carry is the work itself.
//!
//! # Nothing here decides anything
//!
//! No policy, no verification, no naming. A row goes in, a row comes out. What
//! a file is allowed to be lives in `phonix_core::files`, and the deciding is
//! done by `phonix_services::files`.

use chrono::{DateTime, Utc};
use phonix_core::files::{FileCategory, FileSummary, Rejection, UploadStatus};
use phonix_core::identity::UserId;
use phonix_core::query::{Page, PageRequest};
use serde_json::Value as Json;
use sqlx::{AssertSqlSafe, FromRow, PgExecutor, PgPool, Row};
use uuid::Uuid;

use crate::error::DbError;

/// One row, in full.
///
/// Wider than [`FileSummary`], because the job needs fields a screen must never
/// see - where the object lives, how many times the work has been attempted,
/// and what went wrong the last time.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: Uuid,
    pub status: UploadStatus,
    pub bucket: String,
    pub original_name: String,
    pub stored_name: Option<String>,
    pub declared_content_type: Option<String>,
    pub content_type: Option<String>,
    pub category: Option<FileCategory>,
    pub byte_size: u64,
    pub checksum_sha256: Option<String>,
    pub storage_key: Option<String>,
    pub quarantine_key: Option<String>,
    pub rejection: Option<Rejection>,
    pub attempts: u32,
    pub claimed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub uploaded_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
}

impl FileRow {
    /// The narrower view a screen is allowed.
    ///
    /// `uploaded_by_name` is filled in only by [`page`], which joins; a row
    /// fetched on its own carries the id and leaves the name to the caller.
    pub fn to_summary(&self, uploaded_by_name: Option<String>) -> FileSummary {
        FileSummary {
            id: self.id,
            bucket: self.bucket.clone(),
            status: self.status,
            original_name: self.original_name.clone(),
            byte_size: self.byte_size,
            content_type: self.content_type.clone(),
            category: self.category,
            rejection: self.rejection.clone(),
            uploaded_by: self.uploaded_by,
            uploaded_by_name,
            created_at: self.created_at,
        }
    }
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for FileRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let raw_status: String = row.try_get("status")?;

        // Refused rather than defaulted. The CHECK constraint means this can
        // only happen if the table and the enum disagree about the vocabulary,
        // and quietly reading an unknown state as `received` would put a stored
        // file back on the queue.
        let status = UploadStatus::parse(&raw_status).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "status".to_owned(),
            source: format!("unrecognised upload status '{raw_status}'").into(),
        })?;

        let raw_category: Option<String> = row.try_get("category")?;
        let category = raw_category.as_deref().and_then(FileCategory::parse);

        // The stored code is the authority on *whether* a row was rejected; the
        // JSON is the detail behind it. A detail that will not deserialise -
        // written by an older build with a different variant - becomes `None`
        // rather than an error, because the row is still perfectly readable
        // and the status already says it was refused.
        let detail: Option<Json> = row.try_get("rejection_detail")?;
        let rejection = detail.and_then(|json| serde_json::from_value::<Rejection>(json).ok());

        let byte_size: i64 = row.try_get("byte_size")?;
        let attempts: i32 = row.try_get("attempts")?;

        Ok(Self {
            id: row.try_get("id")?,
            status,
            bucket: row.try_get("bucket")?,
            original_name: row.try_get("original_name")?,
            stored_name: row.try_get("stored_name")?,
            declared_content_type: row.try_get("declared_content_type")?,
            content_type: row.try_get("content_type")?,
            category,
            byte_size: u64::try_from(byte_size).unwrap_or(0),
            checksum_sha256: row.try_get("checksum_sha256")?,
            storage_key: row.try_get("storage_key")?,
            quarantine_key: row.try_get("quarantine_key")?,
            rejection,
            attempts: u32::try_from(attempts).unwrap_or(0),
            claimed_at: row.try_get("claimed_at")?,
            last_error: row.try_get("last_error")?,
            uploaded_by: row.try_get("uploaded_by")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            verified_at: row.try_get("verified_at")?,
        })
    }
}

const COLUMNS: &str = "id, status, bucket, original_name, stored_name, declared_content_type, \
     content_type, category, byte_size, checksum_sha256, storage_key, quarantine_key, \
     rejection, rejection_detail, attempts, claimed_at, last_error, uploaded_by, \
     created_at, updated_at, verified_at";

/// What the request that carried the bytes knows.
///
/// Note what is not here: a content type, a category, a checksum, a storage
/// key. None of those is known yet, because knowing them is the work the job
/// exists to do.
#[derive(Debug, Clone)]
pub struct ReceivedUpload<'a> {
    pub id: Uuid,
    pub bucket: &'a str,
    /// Already sanitised - see `phonix_core::files::sanitize_file_name`.
    pub original_name: &'a str,
    pub declared_content_type: Option<&'a str>,
    pub byte_size: u64,
    pub quarantine_key: &'a str,
    pub uploaded_by: Option<UserId>,
}

/// Record bytes that have landed in quarantine.
///
/// The id is supplied rather than generated, because the quarantine object was
/// already written under a name derived from it - the bytes exist before the
/// row does, and they have to be findable if this insert fails.
pub async fn record_received<'e, E>(
    executor: E,
    upload: ReceivedUpload<'_>,
) -> Result<FileRow, DbError>
where
    E: PgExecutor<'e>,
{
    let statement = AssertSqlSafe(format!(
        "INSERT INTO file_uploads
             (id, status, bucket, original_name, declared_content_type, byte_size,
              quarantine_key, uploaded_by)
         VALUES ($1, 'received', $2, $3, $4, $5, $6, $7)
         RETURNING {COLUMNS}"
    ));

    sqlx::query_as::<_, FileRow>(statement)
        .bind(upload.id)
        .bind(upload.bucket)
        .bind(upload.original_name)
        .bind(upload.declared_content_type)
        .bind(i64::try_from(upload.byte_size).unwrap_or(i64::MAX))
        .bind(upload.quarantine_key)
        .bind(upload.uploaded_by)
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)
}

/// One row, whatever state it is in.
pub async fn load<'e, E>(executor: E, id: Uuid) -> Result<Option<FileRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let statement = AssertSqlSafe(format!("SELECT {COLUMNS} FROM file_uploads WHERE id = $1"));

    sqlx::query_as::<_, FileRow>(statement)
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

// ---------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------

/// The clause that decides what is outstanding.
///
/// Two kinds of row: never started, and started by a worker that then stopped
/// existing. The second is why `claimed_at` is a column - without it a process
/// killed mid-verification would leave a row in `verifying` for ever, and the
/// upload would simply never finish.
///
/// `$1` is the claim timeout in seconds, cast explicitly because
/// `make_interval`'s `secs` parameter is `double precision`. Postgres would
/// resolve a bound `int8` through an implicit cast, but relying on cast
/// resolution to make a query work is a thing that stops being true quietly.
const OUTSTANDING: &str = "(status = 'received'
     OR (status = 'verifying'
         AND (claimed_at IS NULL
              OR claimed_at < now() - make_interval(secs => $1::double precision))))";

/// Claim up to `limit` outstanding uploads.
///
/// `FOR UPDATE SKIP LOCKED` is what makes this safe with several workers, and
/// with several *processes*: a row another transaction holds is skipped rather
/// than waited on, so two workers polling at the same moment take different
/// work instead of one of them blocking.
///
/// The claim and the state change are one statement, so there is no window in
/// which a row is claimed but not marked - which is the failure mode that makes
/// a naive `SELECT` then `UPDATE` queue hand the same job to two workers.
pub async fn claim_batch(
    pool: &PgPool,
    limit: usize,
    claim_timeout_secs: u64,
) -> Result<Vec<FileRow>, DbError> {
    let statement = AssertSqlSafe(format!(
        "WITH claimed AS (
             SELECT id
               FROM file_uploads
              WHERE {OUTSTANDING}
              ORDER BY created_at
              LIMIT $2
                FOR UPDATE SKIP LOCKED
         )
         UPDATE file_uploads AS f
            SET status     = 'verifying',
                claimed_at = now(),
                attempts   = f.attempts + 1,
                updated_at = now()
           FROM claimed
          WHERE f.id = claimed.id
      RETURNING {}",
        prefixed(COLUMNS, "f.")
    ));

    sqlx::query_as::<_, FileRow>(statement)
        .bind(claim_timeout_secs as i64)
        .bind(limit as i64)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)
}

/// Claim one specific upload, if it is outstanding.
///
/// What the request handler calls the moment the bytes are safely down, so the
/// common case is verified immediately rather than at the next poll. Returns
/// `None` when another worker got there first, which is an ordinary outcome and
/// not an error - the sweep and the immediate dispatch race on purpose.
pub async fn claim_one(
    pool: &PgPool,
    id: Uuid,
    claim_timeout_secs: u64,
) -> Result<Option<FileRow>, DbError> {
    let statement = AssertSqlSafe(format!(
        "WITH claimed AS (
             SELECT id
               FROM file_uploads
              WHERE id = $2
                AND {OUTSTANDING}
                FOR UPDATE SKIP LOCKED
         )
         UPDATE file_uploads AS f
            SET status     = 'verifying',
                claimed_at = now(),
                attempts   = f.attempts + 1,
                updated_at = now()
           FROM claimed
          WHERE f.id = claimed.id
      RETURNING {}",
        prefixed(COLUMNS, "f.")
    ));

    sqlx::query_as::<_, FileRow>(statement)
        .bind(claim_timeout_secs as i64)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)
}

/// Qualify a column list with a table alias.
///
/// `RETURNING` in an `UPDATE ... FROM` sees both tables, and `id` is ambiguous
/// between them. Built here rather than written out twice so the list cannot
/// drift from [`COLUMNS`].
fn prefixed(columns: &str, alias: &str) -> String {
    columns
        .split(',')
        .map(|column| format!("{alias}{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Terminal states
// ---------------------------------------------------------------------------

/// What a verified file turned out to be.
#[derive(Debug, Clone)]
pub struct StoredFile<'a> {
    pub storage_key: &'a str,
    pub stored_name: &'a str,
    pub content_type: &'a str,
    pub category: FileCategory,
    pub checksum_sha256: &'a str,
    /// The size of the object as it was actually written, which is not
    /// necessarily what the request said it was sending.
    pub byte_size: u64,
}

/// Mark an upload stored.
///
/// `quarantine_key` is cleared in the same statement: the object has moved, and
/// a row naming both places would leave the sweeper looking at a path that is
/// no longer there.
pub async fn mark_stored<'e, E>(
    executor: E,
    id: Uuid,
    stored: StoredFile<'_>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE file_uploads
            SET status           = 'stored',
                storage_key      = $2,
                stored_name      = $3,
                content_type     = $4,
                category         = $5,
                checksum_sha256  = $6,
                byte_size        = $7,
                quarantine_key   = NULL,
                rejection        = NULL,
                rejection_detail = NULL,
                last_error       = NULL,
                claimed_at       = NULL,
                verified_at      = now(),
                updated_at       = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(stored.storage_key)
    .bind(stored.stored_name)
    .bind(stored.content_type)
    .bind(stored.category.as_str())
    .bind(stored.checksum_sha256)
    .bind(i64::try_from(stored.byte_size).unwrap_or(i64::MAX))
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Mark an upload refused.
///
/// Terminal on the first look: a rejection is a statement about the file, so
/// there is nothing a retry could change. The quarantine key is cleared because
/// the caller deletes the object before calling this - a refused file is not
/// kept.
pub async fn mark_rejected<'e, E>(
    executor: E,
    id: Uuid,
    rejection: &Rejection,
    detected_type: Option<&str>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    let detail = serde_json::to_value(rejection).ok();

    sqlx::query(
        "UPDATE file_uploads
            SET status           = 'rejected',
                rejection        = $2,
                rejection_detail = $3,
                content_type     = COALESCE($4, content_type),
                quarantine_key   = NULL,
                claimed_at       = NULL,
                verified_at      = now(),
                updated_at       = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(rejection.code())
    .bind(detail)
    .bind(detected_type)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Give an upload back to the queue after a transient failure.
///
/// Back to `received` rather than left in `verifying`, so it is picked up on
/// the next sweep instead of waiting out the claim timeout. The attempt count
/// is not touched here - the claim already incremented it, which is what makes
/// a worker that dies mid-job still consume an attempt.
pub async fn release<'e, E>(executor: E, id: Uuid, error: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE file_uploads
            SET status     = 'received',
                claimed_at = NULL,
                last_error = $2,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(truncate(error, 1000))
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Give up on an upload.
///
/// Not the same as rejecting it. This says the work could not be done - a full
/// disk, an unreachable backend - and telling somebody their file was refused
/// when the problem was ours would send them off to fix a file that is fine.
pub async fn mark_failed<'e, E>(executor: E, id: Uuid, error: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE file_uploads
            SET status     = 'failed',
                claimed_at = NULL,
                last_error = $2,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(truncate(error, 1000))
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Remove a row.
///
/// The object it names is deleted by the caller first: this layer knows about
/// rows and the storage layer knows about bytes, and a repository that deleted
/// files would be a repository that has to be given a storage backend.
pub async fn delete<'e, E>(executor: E, id: Uuid) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query("DELETE FROM file_uploads WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    Ok(result.rows_affected() > 0)
}

/// Uploads that have sat unverified for longer than they should have.
///
/// Reached only when a job never ran at all - a process killed between writing
/// the bytes and recording the row, or a worker that has been disabled. The
/// sweeper deletes the objects and the rows together.
pub async fn expired_quarantine<'e, E>(
    executor: E,
    older_than_mins: u64,
    limit: usize,
) -> Result<Vec<FileRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let statement = AssertSqlSafe(format!(
        "SELECT {COLUMNS}
           FROM file_uploads
          WHERE quarantine_key IS NOT NULL
            AND status IN ('received', 'verifying', 'failed')
            AND created_at < now() - make_interval(mins => $1)
          ORDER BY created_at
          LIMIT $2"
    ));

    sqlx::query_as::<_, FileRow>(statement)
        .bind(older_than_mins as i32)
        .bind(limit as i64)
        .fetch_all(executor)
        .await
        .map_err(DbError::Query)
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// The filter key the date range travels under.
///
/// Named here and spent by both the query and the grid's date picker, so the
/// two cannot disagree about spelling - a mismatch would silently filter
/// nothing rather than fail.
pub const CREATED: &str = "created";

/// The columns a grid may order by.
///
/// A whitelist, not a convenience: `sort.field` arrives from a browser, and the
/// only safe way to put it in an `ORDER BY` is to not put it there at all - to
/// match it against literals this file wrote itself.
const SORTABLE: &[(&str, &str)] = &[
    ("created_at", "f.created_at"),
    ("original_name", "f.original_name"),
    ("byte_size", "f.byte_size"),
    ("status", "f.status"),
    ("bucket", "f.bucket"),
    ("content_type", "f.content_type"),
];

/// One page of the file list.
///
/// Paged in SQL rather than in the browser: unlike the user list, this grows
/// for as long as the workspace is used.
pub async fn page(pool: &PgPool, request: &PageRequest) -> Result<Page<FileSummary>, DbError> {
    let request = request.sanitised();
    let needle = request
        .needle()
        .map(|needle| format!("%{}%", escape_like(&needle)));

    // The bucket and the status are ordinary named filters, so the screen and
    // the query agree on spelling through `PageRequest::filter` rather than
    // through a comment.
    let bucket = request.filter("bucket").map(str::to_owned);
    let status = request.filter("status").map(str::to_owned);

    const WHERE: &str = "WHERE ($1::text IS NULL
                             OR f.original_name ILIKE $1
                             OR f.content_type ILIKE $1)
                           AND ($2::text IS NULL OR f.bucket = $2)
                           AND ($3::text IS NULL OR f.status = $3)
                           AND ($4::timestamptz IS NULL OR f.created_at >= $4)
                           AND ($5::timestamptz IS NULL OR f.created_at < $5)";

    // Half open, and resolved in the browser: `from` is included and `to` is
    // not, which is what makes a span of one day exactly one day.
    let created = request.range(CREATED);

    let counting = AssertSqlSafe(format!("SELECT count(*) FROM file_uploads AS f {WHERE}"));

    let total: i64 = sqlx::query_scalar(counting)
        .bind(needle.as_deref())
        .bind(bucket.as_deref())
        .bind(status.as_deref())
        .bind(created.from)
        .bind(created.to)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

    let total = u64::try_from(total).unwrap_or(0);
    let request = request.clamped_to(total);

    let order = match &request.sort {
        Some(sort) => SORTABLE
            .iter()
            .find(|(field, _)| *field == sort.field)
            .map(|(_, column)| format!("{column} {}", sort.direction.sql())),
        None => None,
    }
    // Newest first, and `id` after it whatever the sort: two files uploaded in
    // the same millisecond would otherwise swap places between one page and the
    // next, which shows up as a row that appears twice.
    .unwrap_or_else(|| "f.created_at DESC".to_owned());

    // Left join, not inner: an upload whose uploader has since been deleted is
    // still a file, and an inner join would make it vanish from the list rather
    // than lose a name.
    let selecting = AssertSqlSafe(format!(
        "SELECT {COLUMNS}, u.display_name AS uploaded_by_name
           FROM file_uploads AS f
           LEFT JOIN users AS u ON u.id = f.uploaded_by
           {WHERE}
          ORDER BY {order}, f.id DESC
          LIMIT $6 OFFSET $7",
        COLUMNS = prefixed(COLUMNS, "f.")
    ));

    let rows = sqlx::query(selecting)
        .bind(needle.as_deref())
        .bind(bucket.as_deref())
        .bind(status.as_deref())
        .bind(created.from)
        .bind(created.to)
        .bind(request.limit() as i64)
        .bind(request.offset() as i64)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;

    let summaries = rows
        .iter()
        .map(|row| {
            let file = FileRow::from_row(row)?;
            let uploaded_by_name: Option<String> = row.try_get("uploaded_by_name")?;
            Ok(file.to_summary(uploaded_by_name))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(DbError::Query)?;

    Ok(Page::new(summaries, total, &request))
}

/// Cut a string to a length the column will take.
///
/// On a character boundary, because a byte-wise cut through a multi-byte
/// character produces something Postgres will refuse - and this is called from
/// the error path, where a second failure is the last thing wanted.
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }

    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    text.get(..end).unwrap_or_default().to_owned()
}

/// Escape the wildcards in a search term.
///
/// Without this, a search for `50%` matches everything.
fn escape_like(needle: &str) -> String {
    needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_list_survives_being_qualified() {
        // The `UPDATE ... FROM` in the claim sees two tables and `id` is
        // ambiguous between them, so every column has to carry the alias. Doing
        // it by hand would be a second list to keep in step with the first.
        let qualified = prefixed("id, status, bucket", "f.");
        assert_eq!(qualified, "f.id, f.status, f.bucket");

        let all = prefixed(COLUMNS, "f.");
        assert_eq!(
            all.split(", ").count(),
            COLUMNS.split(',').count(),
            "a column was lost or gained: {all}"
        );
        assert!(all.split(", ").all(|column| column.starts_with("f.")));
    }

    #[test]
    fn search_terms_cannot_smuggle_wildcards() {
        assert_eq!(escape_like("50%"), "50\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn truncation_lands_on_a_character_boundary() {
        // Called from the error path, where the last thing wanted is a second
        // failure - and a cut through a multi-byte character is one.
        let long = "é".repeat(1000);
        let cut = truncate(&long, 1001);

        assert!(cut.len() <= 1001);
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
        assert!(cut.chars().all(|ch| ch == 'é'));

        assert_eq!(truncate("short", 100), "short");
    }

    #[test]
    fn the_outstanding_clause_covers_both_kinds_of_pending_work() {
        // Never started, and started by a worker that then stopped existing.
        // Missing the second is a row that sits in `verifying` for ever.
        assert!(OUTSTANDING.contains("status = 'received'"));
        assert!(OUTSTANDING.contains("status = 'verifying'"));
        assert!(OUTSTANDING.contains("claimed_at IS NULL"));
    }
}
