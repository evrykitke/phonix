//! What a stored file is called, which is never what it arrived as.
//!
//! `image.png` does not become `image.png` on disk. It becomes
//! `attachments/2026/08/0199c4f2e1a37b8d9e5f0a1b2c3d4e5f.png`, and the name it
//! had is kept in a column, where it can be shown to people and offered back on
//! download without ever being a path.
//!
//! # Four reasons, none of them tidiness
//!
//! 1. **A name from a caller is a path from a caller.** Sanitising it well is
//!    possible; sanitising it correctly on every filesystem, in every locale,
//!    for every future caller, is a promise nobody should make. Generating the
//!    name instead removes the question rather than answering it.
//! 2. **Two people upload `invoice.pdf`.** Under their own names, the second
//!    overwrites the first - silently, and with somebody else's document.
//! 3. **A name is information.** `severance-agreement-j-smith.pdf` in a
//!    directory listing, a backup manifest or a log line has told somebody
//!    something. The stored name says nothing at all.
//! 4. **The extension follows the bytes.** A file detected as a zip is stored
//!    as `.zip` whatever it was called, so nothing downstream is ever handed a
//!    name that disagrees with the content.
//!
//! # Strategies
//!
//! [`NamingStrategy`] is a trait because "how are files laid out" is a decision
//! a deployment gets to make, and because the two answers below suit different
//! backends:
//!
//! * [`DateSharded`] - `bucket/YYYY/MM/<uuid>.<ext>`. The default. A UUIDv7
//!   sorts by time, so the newest files are adjacent both in a listing and on
//!   disk, and no directory ever holds more than a month of uploads.
//! * [`ContentAddressed`] - `bucket/ab/cd/<sha256>.<ext>`. The same bytes land
//!   at the same key, so an identical file uploaded a hundred times occupies
//!   one object. Costs a second pass to know the digest before naming, and
//!   means deleting a file has to ask whether anything else points at it.

use chrono::{DateTime, Datelike, Utc};
use uuid::Uuid;

/// What a strategy is told about the file it is naming.
#[derive(Debug, Clone, Copy)]
pub struct NamingContext<'a> {
    /// The bucket the file belongs to. Always the first segment, whatever the
    /// strategy: it is what the bucket's policy is later found by.
    pub bucket: &'a str,
    /// The row's id. Unique already, and time-ordered when it is a v7.
    pub file_id: Uuid,
    /// The canonical extension of the type **detection** decided on - never the
    /// one the caller's filename carried.
    pub extension: &'a str,
    /// Lowercase hex SHA-256 of the contents, when it is known.
    ///
    /// `None` while a file is still in quarantine, which is exactly why
    /// [`ContentAddressed`] cannot be used to name one: there is nothing to
    /// name it after yet.
    pub checksum: Option<&'a str>,
    /// When the file arrived. Passed in rather than read from the clock so that
    /// naming is a pure function - the same upload replayed produces the same
    /// key, which is what makes a retried job idempotent.
    pub at: DateTime<Utc>,
}

/// How stored files are laid out.
pub trait NamingStrategy: Send + Sync + 'static {
    /// The path segments below the tenant, bucket included.
    ///
    /// Returns segments rather than a string because the segments are what
    /// [`crate::StorageKey`] validates, and handing back a pre-joined path
    /// would mean splitting it again to check it.
    fn segments(&self, context: &NamingContext<'_>) -> Vec<String>;

    /// What this strategy is, for a log line at startup.
    fn describe(&self) -> &'static str;
}

/// `bucket/YYYY/MM/<uuid>.<ext>` - the default.
#[derive(Debug, Clone, Copy, Default)]
pub struct DateSharded;

impl NamingStrategy for DateSharded {
    fn segments(&self, context: &NamingContext<'_>) -> Vec<String> {
        vec![
            context.bucket.to_owned(),
            format!("{:04}", context.at.year()),
            format!("{:02}", context.at.month()),
            file_name(context.file_id, context.extension),
        ]
    }

    fn describe(&self) -> &'static str {
        "date-sharded (bucket/YYYY/MM/<uuid>.<ext>)"
    }
}

/// `bucket/ab/cd/<sha256>.<ext>` - one object per distinct file.
///
/// The first four hex characters of the digest become two directory levels, so
/// a million files spread over 65,536 directories rather than piling into one.
/// That matters more than it sounds: a directory with a million entries is slow
/// to open on every filesystem and impossible to list on most.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContentAddressed;

impl NamingStrategy for ContentAddressed {
    fn segments(&self, context: &NamingContext<'_>) -> Vec<String> {
        // Without a digest there is nothing to address by. Falling back to the
        // id keeps this total - a strategy that could fail would push a
        // fallback into every caller - and the fallback is still a perfectly
        // good key, it simply does not deduplicate.
        let Some(checksum) = context.checksum.filter(|hex| hex.len() >= 4) else {
            return DateSharded.segments(context);
        };

        let first = checksum.get(..2).unwrap_or("00");
        let second = checksum.get(2..4).unwrap_or("00");

        vec![
            context.bucket.to_owned(),
            first.to_owned(),
            second.to_owned(),
            format!("{checksum}.{}", extension_or_bin(context.extension)),
        ]
    }

    fn describe(&self) -> &'static str {
        "content-addressed (bucket/ab/cd/<sha256>.<ext>)"
    }
}

/// `bucket/<uuid>.<ext>` - everything in one directory per bucket.
///
/// Here because it is what an object store actually wants: S3 has no
/// directories, the sharding above buys nothing there, and a shorter key is a
/// shorter key. Not the default, because the local backend does have
/// directories and would end up with one holding every file ever uploaded.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flat;

impl NamingStrategy for Flat {
    fn segments(&self, context: &NamingContext<'_>) -> Vec<String> {
        vec![
            context.bucket.to_owned(),
            file_name(context.file_id, context.extension),
        ]
    }

    fn describe(&self) -> &'static str {
        "flat (bucket/<uuid>.<ext>)"
    }
}

/// The name bytes are held under before anything has decided what they are.
///
/// The id and nothing else: at this point the extension is unknown, because
/// knowing it is the job that has not run yet. `.part` says plainly that this
/// is not a finished file, so a person looking at the directory during an
/// incident is not misled by it.
pub fn quarantine_name(file_id: Uuid) -> String {
    format!("{}.part", hex_id(file_id))
}

fn file_name(file_id: Uuid, extension: &str) -> String {
    format!("{}.{}", hex_id(file_id), extension_or_bin(extension))
}

/// A UUID as 32 hex characters, without the hyphens.
///
/// Hyphens are legal in a key segment and this drops them anyway: it is four
/// characters shorter, it double-clicks as one word, and it cannot be mistaken
/// for two fields joined by a dash.
fn hex_id(file_id: Uuid) -> String {
    file_id.simple().to_string()
}

/// An extension that is safe to put in a key, or `bin`.
///
/// The extension comes from the catalogue, which only holds lowercase ASCII, so
/// this never fires in practice. It is here because the key validator would
/// otherwise reject the whole key over a bad extension, turning a catalogue
/// mistake into a failed upload rather than a file called `.bin`.
fn extension_or_bin(extension: &str) -> &str {
    let usable = !extension.is_empty()
        && extension.len() <= 16
        && extension
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());

    if usable { extension } else { "bin" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>(extension: &'a str, checksum: Option<&'a str>) -> NamingContext<'a> {
        NamingContext {
            bucket: "attachments",
            file_id: Uuid::from_u128(0x0199c4f2_e1a3_7b8d_9e5f_0a1b2c3d4e5f),
            extension,
            checksum,
            at: DateTime::from_timestamp(1_755_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn a_stored_file_is_not_called_what_it_arrived_as() {
        let segments = DateSharded.segments(&context("png", None));
        let name = segments.last().unwrap();

        assert!(!name.contains("image"));
        assert!(name.ends_with(".png"));
        assert_eq!(name.len(), 32 + 4);
    }

    #[test]
    fn date_sharding_puts_a_month_in_its_own_directory() {
        let segments = DateSharded.segments(&context("pdf", None));

        assert_eq!(
            segments,
            vec![
                "attachments".to_owned(),
                "2025".to_owned(),
                "08".to_owned(),
                "0199c4f2e1a37b8d9e5f0a1b2c3d4e5f.pdf".to_owned(),
            ]
        );
    }

    #[test]
    fn naming_is_a_pure_function_of_what_it_is_given() {
        // What makes a retried job safe: the second attempt computes the same
        // key as the first, so it overwrites its own partial work rather than
        // leaving a second copy behind.
        let first = DateSharded.segments(&context("pdf", None));
        let second = DateSharded.segments(&context("pdf", None));

        assert_eq!(first, second);
    }

    #[test]
    fn content_addressing_puts_identical_bytes_at_one_key() {
        let digest = "ab".to_owned() + &"cd".repeat(31);

        let first = ContentAddressed.segments(&context("png", Some(&digest)));
        let second = ContentAddressed.segments(&context("png", Some(&digest)));

        assert_eq!(first, second);
        assert_eq!(
            first,
            vec![
                "attachments".to_owned(),
                "ab".to_owned(),
                "cd".to_owned(),
                format!("{digest}.png"),
            ]
        );
    }

    #[test]
    fn content_addressing_without_a_digest_still_produces_a_usable_key() {
        // A file in quarantine has no digest yet. The strategy has to answer
        // something, and answering with a key that works is better than every
        // caller carrying a fallback.
        let segments = ContentAddressed.segments(&context("png", None));

        assert_eq!(segments.first().map(String::as_str), Some("attachments"));
        assert!(segments.last().is_some_and(|name| name.ends_with(".png")));

        // A digest too short to shard by is the same case.
        assert_eq!(
            ContentAddressed.segments(&context("png", Some("ab"))),
            DateSharded.segments(&context("png", None))
        );
    }

    #[test]
    fn every_strategy_produces_segments_a_key_will_accept() {
        let strategies: Vec<Box<dyn NamingStrategy>> = vec![
            Box::new(DateSharded),
            Box::new(ContentAddressed),
            Box::new(Flat),
        ];
        let tenant = phonix_core::TenantSlug::parse("acme").unwrap();
        let digest = "0f".repeat(32);

        for strategy in strategies {
            for extension in ["png", "pdf", "xlsx", "7z"] {
                let segments = strategy.segments(&context(extension, Some(&digest)));
                let key = crate::StorageKey::new(&tenant, &segments);

                assert!(
                    key.is_ok(),
                    "{} produced segments the key validator refuses: {segments:?}",
                    strategy.describe()
                );
            }
        }
    }

    #[test]
    fn a_nonsense_extension_becomes_bin_rather_than_a_broken_key() {
        // Not reachable from the catalogue, which is lowercase ASCII
        // throughout. It matters anyway: refusing here would turn a mistake in
        // a table into an upload that fails for the person who made it.
        for bad in ["", "PNG", "p n g", "../x", &"a".repeat(40)] {
            let segments = DateSharded.segments(&context(bad, None));
            assert!(
                segments.last().is_some_and(|name| name.ends_with(".bin")),
                "{bad:?} was not neutralised: {segments:?}"
            );
        }
    }

    #[test]
    fn a_quarantine_name_says_it_is_not_a_finished_file() {
        let name = quarantine_name(Uuid::from_u128(0x0199c4f2_e1a3_7b8d_9e5f_0a1b2c3d4e5f));

        assert_eq!(name, "0199c4f2e1a37b8d9e5f0a1b2c3d4e5f.part");
        // No extension from the caller: at this point nothing has looked at the
        // bytes, so there is nothing to claim about them.
        assert!(!name.contains(".png"));
    }
}
