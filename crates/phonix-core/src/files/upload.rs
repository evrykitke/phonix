//! An upload, from the moment the bytes land to the event that says what
//! became of them.
//!
//! # An upload is a job, so it has states rather than a return value
//!
//! The request that carries the bytes does not decide anything. It writes them
//! to a quarantine area, records a row, and answers with an id - and that is
//! all it does, because everything worth doing to an uploaded file is work that
//! must survive the connection dropping halfway through. Deciding what the file
//! is, hashing it, moving it into place, and one day scanning it, all happen in
//! a worker afterwards.
//!
//! ```text
//!   received ──► verifying ──► stored      the file is real and in place
//!                    │
//!                    ├───────► rejected    the file is not acceptable
//!                    └───────► failed      we could not finish the work
//! ```
//!
//! `rejected` and `failed` are deliberately different terminal states. A
//! rejection is an answer about the file - it is too big, or it is not what it
//! said it was - and repeating the upload unchanged will produce it again. A
//! failure is about us: the disk was full, the storage backend was unreachable.
//! Collapsing the two would tell somebody to fix their file when the problem
//! was ours.
//!
//! # What comes back
//!
//! [`UploadResult`] is the event, published as
//! `tenant.<slug>.file.upload.completed` when the job reaches any terminal
//! state. It carries the whole outcome, so a consumer never has to read the
//! database to know what happened. [`FileSummary`] is the narrower view a
//! screen renders - the same facts minus the ones a browser has no use for.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::catalog::FileCategory;
use crate::identity::UserId;
use crate::{Message, msg};

/// Identifies an upload for its whole life, and the stored file afterwards.
///
/// One id, not two. The row that records "somebody is uploading this" becomes
/// the row that records "this file exists", so a link handed out while the job
/// was still running is the same link that works once it finishes.
pub type FileId = Uuid;

/// Where an upload has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    /// Bytes are in quarantine and the job is queued.
    Received,
    /// A worker has claimed it.
    Verifying,
    /// Verified, renamed and moved into place.
    Stored,
    /// Refused. The file is what is wrong; see the [`Rejection`].
    Rejected,
    /// The work could not be completed after the allowed attempts. Nothing to
    /// do with the file itself.
    Failed,
}

impl UploadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Verifying => "verifying",
            Self::Stored => "stored",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [
            Self::Received,
            Self::Verifying,
            Self::Stored,
            Self::Rejected,
            Self::Failed,
        ]
        .into_iter()
        .find(|status| status.as_str() == raw)
    }

    /// What a screen calls it.
    pub fn label(self) -> Message {
        match self {
            Self::Received => msg!("upload.status.received"),
            Self::Verifying => msg!("upload.status.verifying"),
            Self::Stored => msg!("upload.status.stored"),
            Self::Rejected => msg!("upload.status.rejected"),
            Self::Failed => msg!("upload.status.failed"),
        }
    }

    /// Whether nothing more will happen to this upload.
    ///
    /// What a screen polls until, and what the job runner refuses to claim.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stored | Self::Rejected | Self::Failed)
    }

    /// Whether the file exists and can be downloaded.
    pub fn is_available(self) -> bool {
        matches!(self, Self::Stored)
    }
}

/// Why a file was refused.
///
/// Every variant is safe to show the person who uploaded it: it says what was
/// wrong with *their* file, and nothing about how this server is arranged. That
/// is why the detected type appears here as a MIME string and the storage key
/// does not appear at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", content = "detail", rename_all = "snake_case")]
pub enum Rejection {
    /// Nothing arrived, or a file of zero bytes did.
    Empty,

    /// Larger than the bucket allows.
    TooLarge { limit_bytes: u64, actual_bytes: u64 },

    /// The leading bytes match nothing in the catalogue, and the content is not
    /// text either. Whatever it is, this application does not store it.
    UnrecognisedType { declared: Option<String> },

    /// A real, recognised type - in the wrong place. A PDF is a fine thing; it
    /// is not a profile picture.
    TypeNotAllowed { detected: String, bucket: String },

    /// A format that can carry executable content, in a bucket that does not
    /// take one.
    ActiveContentNotAllowed { detected: String },

    /// A picture inside the byte limit and far outside the pixel one.
    ///
    /// Its own variant rather than a second `TooLarge`, because the advice is
    /// different: shrinking a 30,000-pixel image is not the same instruction as
    /// compressing a large file, and a message quoting megabytes at somebody
    /// whose file was 400 KB reads as nonsense.
    ImageTooLarge {
        width: u32,
        height: u32,
        max_width: u32,
        max_height: u32,
    },

    /// The name and the bytes disagree about what this is.
    ///
    /// Not a storage problem - the file is renamed on the way in, so the claim
    /// decides nothing - but it is worth refusing anyway. Somebody sending a
    /// zip called `photo.png` either has a broken tool or is trying something,
    /// and in both cases the honest answer is to say so rather than to store it
    /// silently under a name that means something else.
    Masquerade { declared: String, detected: String },

    /// The upload named a bucket that does not exist.
    UnknownBucket { requested: String },
}

impl Rejection {
    /// Short, stable identifier. Stored on the row and used in dashboards.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLarge { .. } => "too_large",
            Self::UnrecognisedType { .. } => "unrecognised_type",
            Self::TypeNotAllowed { .. } => "type_not_allowed",
            Self::ActiveContentNotAllowed { .. } => "active_content_not_allowed",
            Self::ImageTooLarge { .. } => "image_too_large",
            Self::Masquerade { .. } => "masquerade",
            Self::UnknownBucket { .. } => "unknown_bucket",
        }
    }

    /// One sentence, for the person who chose the file.
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "That file is empty.".to_owned(),

            Self::TooLarge {
                limit_bytes,
                actual_bytes,
            } => format!(
                "That file is {}, and the limit here is {}.",
                super::name::human_size(*actual_bytes),
                super::name::human_size(*limit_bytes)
            ),

            Self::UnrecognisedType { .. } => {
                "That is not a kind of file this application stores.".to_owned()
            }

            Self::TypeNotAllowed { detected, bucket } => {
                let label = super::catalog::by_mime(detected)
                    .map_or(detected.as_str(), |file_type| file_type.label);
                let place =
                    super::bucket::bucket(bucket).map_or(bucket.as_str(), |policy| policy.label);
                format!("A {label} cannot go in {place}.")
            }

            Self::ActiveContentNotAllowed { detected } => {
                let label = super::catalog::by_mime(detected)
                    .map_or(detected.as_str(), |file_type| file_type.label);
                format!("A {label} can contain code that runs, so it is not accepted here.")
            }

            Self::ImageTooLarge {
                width,
                height,
                max_width,
                max_height,
            } => format!(
                "That picture is {width} by {height} pixels, and the limit here is \
                 {max_width} by {max_height}."
            ),

            Self::Masquerade { declared, detected } => {
                let claimed = super::catalog::by_mime(declared)
                    .map_or(declared.as_str(), |file_type| file_type.label);
                let actual = super::catalog::by_mime(detected)
                    .map_or(detected.as_str(), |file_type| file_type.label);
                format!("That file is named like a {claimed} but its contents are a {actual}.")
            }

            Self::UnknownBucket { .. } => {
                "That upload was addressed somewhere that does not exist.".to_owned()
            }
        }
    }

    /// Whether uploading the identical file again could succeed.
    ///
    /// False for every variant: a rejection is a statement about the file, so
    /// retrying it unchanged produces the same answer. Kept as a method rather
    /// than a comment because a screen needs to decide between "try again" and
    /// "choose a different file".
    pub fn is_retryable(&self) -> bool {
        false
    }
}

/// What became of an upload.
///
/// Published as an event once the job reaches a terminal state, and carried
/// whole so a consumer can act on it without a database round trip. It is also
/// what the outbox stores, which is why every field is owned and serialisable
/// rather than borrowed from a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadResult {
    pub file_id: FileId,
    pub bucket: String,
    pub status: UploadStatus,

    /// The name the caller's file had, already sanitised.
    pub original_name: String,
    /// The name it is stored under, which resembles the original in no way.
    /// `None` unless the status is `stored`.
    pub stored_name: Option<String>,
    /// Where the storage backend put it. Present in the event because a
    /// consumer may legitimately need to reach the object; deliberately absent
    /// from [`FileSummary`], because a browser never does.
    pub storage_key: Option<String>,

    /// What the bytes turned out to be. `None` when nothing recognised them.
    pub content_type: Option<String>,
    pub category: Option<FileCategory>,
    pub byte_size: u64,
    /// Lowercase hex SHA-256 of the stored bytes. The handle for deduplication
    /// and the only way to prove later that a file has not been altered.
    pub checksum_sha256: Option<String>,

    pub rejection: Option<Rejection>,
    pub uploaded_by: Option<UserId>,
    pub occurred_at: DateTime<Utc>,
}

impl UploadResult {
    /// The routing key suffix this event is published under.
    ///
    /// One key for every outcome rather than one per status: a consumer that
    /// cares only about successes filters on the status it can already see, and
    /// a consumer watching for rejections would otherwise have to bind a second
    /// key to hear about them at all.
    pub const ROUTING_KEY: &'static str = "file.upload.completed";

    pub fn succeeded(&self) -> bool {
        self.status.is_available()
    }
}

/// One file, as a screen shows it.
///
/// Note what is not here: the storage key, the quarantine key, the attempt
/// count and the last error. A browser has no use for any of them, and the
/// first two describe how this server is laid out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSummary {
    pub id: FileId,
    pub bucket: String,
    pub status: UploadStatus,
    pub original_name: String,
    pub byte_size: u64,
    /// The detected type, `None` while the job has not decided yet.
    pub content_type: Option<String>,
    pub category: Option<FileCategory>,
    pub rejection: Option<Rejection>,
    pub uploaded_by: Option<UserId>,
    pub uploaded_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl FileSummary {
    /// What kind of thing this is, in words - `PNG image`, `Excel workbook`.
    ///
    /// Falls back to the category, and then to nothing at all, because a row
    /// that is still being checked has no type yet and "unknown" would read as
    /// a verdict.
    pub fn type_label(&self) -> Option<Message> {
        if let Some(mime) = self.content_type.as_deref()
            && let Some(file_type) = super::catalog::by_mime(mime)
        {
            // A format's name, out of the mime table - "PNG image", "Excel
            // workbook". Data rather than language: these are the names the
            // formats have, they are the same words in a French file manager,
            // and a catalog of several hundred of them would be a translation
            // burden buying nothing. `Message::literal` is what says so out
            // loud rather than leaving a bare `String` to look like an oversight.
            return Some(Message::literal(file_type.label));
        }

        self.category.map(FileCategory::label)
    }

    /// The size, as a person reads it.
    pub fn size_label(&self) -> String {
        super::name::human_size(self.byte_size)
    }

    /// Whether this file can be downloaded now.
    pub fn is_available(&self) -> bool {
        self.status.is_available()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_through_their_stored_form() {
        for status in [
            UploadStatus::Received,
            UploadStatus::Verifying,
            UploadStatus::Stored,
            UploadStatus::Rejected,
            UploadStatus::Failed,
        ] {
            assert_eq!(UploadStatus::parse(status.as_str()), Some(status));
        }

        assert_eq!(UploadStatus::parse("quarantined"), None);
    }

    #[test]
    fn only_the_three_ends_are_terminal() {
        assert!(!UploadStatus::Received.is_terminal());
        assert!(!UploadStatus::Verifying.is_terminal());
        assert!(UploadStatus::Stored.is_terminal());
        assert!(UploadStatus::Rejected.is_terminal());
        assert!(UploadStatus::Failed.is_terminal());

        // Terminal is not the same as usable, and conflating them would offer a
        // download link for a file that was refused.
        assert!(UploadStatus::Stored.is_available());
        assert!(!UploadStatus::Rejected.is_available());
        assert!(!UploadStatus::Failed.is_available());
    }

    #[test]
    fn a_rejection_says_something_useful_and_nothing_revealing() {
        let too_large = Rejection::TooLarge {
            limit_bytes: 2 * 1024 * 1024,
            actual_bytes: 5 * 1024 * 1024,
        };
        let message = too_large.message();

        assert!(message.contains("5.0 MB"), "{message}");
        assert!(message.contains("2.0 MB"), "{message}");
        assert_eq!(too_large.code(), "too_large");
    }

    #[test]
    fn a_masquerade_names_both_halves_of_the_disagreement() {
        let rejection = Rejection::Masquerade {
            declared: "image/png".into(),
            detected: "application/zip".into(),
        };

        let message = rejection.message();
        assert!(message.contains("PNG image"), "{message}");
        assert!(message.contains("Zip archive"), "{message}");
    }

    #[test]
    fn a_rejection_of_a_bucket_it_does_not_know_still_reads() {
        // The bucket name came from a caller, so it may be anything at all.
        // The message must not depend on it being one of ours.
        let rejection = Rejection::TypeNotAllowed {
            detected: "application/pdf".into(),
            bucket: "whatever".into(),
        };

        assert!(rejection.message().contains("PDF document"));
    }

    #[test]
    fn rejections_survive_the_trip_to_a_browser() {
        let rejection = Rejection::ActiveContentNotAllowed {
            detected: "image/svg+xml".into(),
        };

        let json = serde_json::to_string(&rejection).unwrap();
        assert_eq!(serde_json::from_str::<Rejection>(&json).unwrap(), rejection);
        assert!(json.contains("active_content_not_allowed"), "{json}");
    }

    #[test]
    fn the_summary_a_screen_gets_carries_no_storage_detail() {
        let summary = FileSummary {
            id: Uuid::nil(),
            bucket: "attachments".into(),
            status: UploadStatus::Stored,
            original_name: "Q3 report.pdf".into(),
            byte_size: 3_250_586,
            content_type: Some("application/pdf".into()),
            category: Some(FileCategory::Document),
            rejection: None,
            uploaded_by: None,
            uploaded_by_name: Some("Ada Lovelace".into()),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(
            !json.contains("storage_key"),
            "the storage layout reached the browser: {json}"
        );
        assert!(!json.contains("quarantine"), "{json}");

        assert_eq!(
            summary.type_label().map(|t| t.to_string()).as_deref(),
            Some("PDF document")
        );
        assert_eq!(summary.size_label(), "3.1 MB");
        assert!(summary.is_available());
    }

    #[test]
    fn a_file_still_being_checked_has_no_type_to_show() {
        let summary = FileSummary {
            id: Uuid::nil(),
            bucket: "attachments".into(),
            status: UploadStatus::Received,
            original_name: "unknown".into(),
            byte_size: 10,
            content_type: None,
            category: None,
            rejection: None,
            uploaded_by: None,
            uploaded_by_name: None,
            created_at: Utc::now(),
        };

        // Not "Unknown", which reads as a verdict on a file nobody has looked
        // at yet.
        assert_eq!(summary.type_label(), None);
        assert!(!summary.is_available());
    }

    #[test]
    fn the_event_round_trips_as_json() {
        let result = UploadResult {
            file_id: Uuid::nil(),
            bucket: "attachments".into(),
            status: UploadStatus::Stored,
            original_name: "photo.png".into(),
            stored_name: Some("0199-abc.png".into()),
            storage_key: Some("acme/attachments/2026/08/0199-abc.png".into()),
            content_type: Some("image/png".into()),
            category: Some(FileCategory::Image),
            byte_size: 2048,
            checksum_sha256: Some("a".repeat(64)),
            rejection: None,
            uploaded_by: None,
            occurred_at: Utc::now(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let back: UploadResult = serde_json::from_str(&json).unwrap();

        assert_eq!(back, result);
        assert!(back.succeeded());
    }
}
