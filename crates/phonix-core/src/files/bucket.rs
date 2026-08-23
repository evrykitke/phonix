//! What a file is *for*, and what that permits.
//!
//! A bucket is the answer to "why is this being uploaded" - an avatar, an
//! attachment, a spreadsheet to import. It is not a folder, although it becomes
//! one on disk; it is a policy, and the folder falls out of it.
//!
//! Declared in code, like the permission tree and for the same reason: the code
//! is what enforces it. A limit in a configuration file is a limit an operator
//! can raise past what the rest of the system can survive, and a bucket nothing
//! checks is a directory anyone can fill.
//!
//! ```text
//!   avatars       2 MB   pictures only, nothing scriptable   anyone signed in
//!   logos         2 MB   pictures only, nothing scriptable   Settings
//!   attachments  25 MB   documents, pictures, archives       Files.Upload
//!   imports      10 MB   text and tabular data only          Files.Upload
//! ```
//!
//! # Why the limit lives here rather than in `[storage]`
//!
//! Both, in fact. The configuration carries one hard ceiling for the whole
//! server - the number the HTTP layer refuses past, before any of this is
//! consulted - and each bucket carries the smaller number that is right for
//! what it holds. A 25 MB attachment is reasonable; a 25 MB avatar is somebody
//! filling the disk one profile picture at a time, and the difference is a
//! property of the bucket, not of the deployment.

use super::catalog::FileCategory;
use crate::permissions;

/// One bucket's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketPolicy {
    /// Stable, lowercase, and a path segment on disk. Renaming one strands
    /// every file already stored under it, so it is treated like a permission
    /// name: chosen once.
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// The most one file here may weigh.
    pub max_bytes: u64,
    /// Which kinds of thing belong here. A type outside these is refused even
    /// when the bytes are genuinely what they claim to be.
    pub categories: &'static [FileCategory],
    /// Whether a format that can carry executable content - an SVG with a
    /// script in it, a PDF with an open action, a macro-bearing Office file -
    /// may be stored here at all.
    ///
    /// Allowing it is not the same as trusting it: such a file is still never
    /// served inline, whatever the bucket says. This decides only whether it is
    /// accepted, and the answer is no anywhere the file is later shown to
    /// somebody as a picture.
    pub allow_active_content: bool,
    /// The largest picture, in pixels, this bucket will take.
    ///
    /// A separate limit from `max_bytes` because the two catch different
    /// things: a byte count bounds what crosses the wire, and this bounds what
    /// happens when something later decodes it. A 400 KB PNG can be 30,000
    /// pixels square, which is 3.6 GB of memory to open - well inside every
    /// byte limit here. See [`crate::files::image`].
    ///
    /// `None` for buckets that hold no pictures, and for those that hold them
    /// only to hand back unchanged.
    pub max_dimensions: Option<(u32, u32)>,
    /// The permission an uploader must hold. `None` means any fully
    /// authenticated user, which is right for the things people upload about
    /// themselves.
    pub upload_permission: Option<&'static str>,
}

impl BucketPolicy {
    /// Whether this bucket accepts that kind of file.
    pub fn accepts(&self, category: FileCategory) -> bool {
        self.categories.contains(&category)
    }

    /// The `accept` attribute for a file input aimed at this bucket.
    ///
    /// A courtesy, exactly like a hidden button: it stops a person choosing a
    /// file that was never going to be accepted, and it is not a control. The
    /// same policy is applied again on the server, against the bytes rather
    /// than against the name.
    pub fn accept_attribute(&self) -> String {
        super::catalog::CATALOGUE
            .iter()
            .filter(|file_type| self.accepts(file_type.category))
            .filter(|file_type| self.allow_active_content || !file_type.active_content)
            .flat_map(|file_type| file_type.extensions.iter())
            .map(|extension| format!(".{extension}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The maximum any bucket allows.
///
/// What the HTTP layer uses to size the ceiling it enforces before a single
/// byte is written, so a request aimed at the avatar bucket cannot make the
/// server buffer an attachment's worth of data to find that out.
pub fn largest_bucket_limit() -> u64 {
    BUCKETS
        .iter()
        .map(|bucket| bucket.max_bytes)
        .max()
        .unwrap_or(0)
}

const MB: u64 = 1024 * 1024;

/// Every bucket this application has.
pub const BUCKETS: &[BucketPolicy] = &[
    BucketPolicy {
        name: "avatars",
        label: "Profile pictures",
        description: "The picture shown beside somebody's name.",
        max_bytes: 2 * MB,
        categories: &[FileCategory::Image],
        // An avatar is rendered inline in every list on the site. An SVG here
        // would be script running on this origin under somebody else's name,
        // which is the one thing this flag exists to prevent.
        allow_active_content: false,
        // The browser crops and downscales before it uploads, so a real avatar
        // arrives well inside this. It is here as the floor under that: the
        // crop runs on the client, and anything running on the client is a
        // convenience that a hand-written request simply skips.
        max_dimensions: Some((1024, 1024)),
        // Changing your own picture is not an administrative act. The use case
        // still checks that the caller is acting on their own account.
        upload_permission: None,
    },
    BucketPolicy {
        name: "logos",
        label: "Organization logo",
        description: "The mark shown on this workspace's documents.",
        max_bytes: 2 * MB,
        categories: &[FileCategory::Image],
        // The obvious format for a logo is SVG, and it is refused anyway. An
        // SVG is a document that can carry script, and this image is rendered
        // inline on the settings screen and embedded in everything the
        // workspace issues - so it would be script running on this origin,
        // uploaded by an administrator, under the organization's own name.
        // PNG with transparency is the format to use instead.
        allow_active_content: false,
        // Larger than an avatar's ceiling because nothing crops this one: a
        // logo is wide more often than square, and downscaling it to a square
        // is how a wordmark becomes unreadable. This bounds what a decoder is
        // ever asked to open, which is the only thing the limit is for.
        max_dimensions: Some((4096, 4096)),
        // Not a picture of yourself: this is the organization's mark, and
        // changing it changes every document it goes on. The same permission
        // the rest of the settings screen requires.
        upload_permission: Some(permissions::SETTINGS),
    },
    BucketPolicy {
        name: "attachments",
        label: "Attachments",
        description: "Files kept alongside the records that refer to them.",
        max_bytes: 25 * MB,
        categories: &[
            FileCategory::Image,
            FileCategory::Document,
            FileCategory::Spreadsheet,
            FileCategory::Presentation,
            FileCategory::Text,
            FileCategory::Data,
            FileCategory::Archive,
        ],
        // A PDF is the commonest attachment there is, and every PDF counts as
        // active content. Refusing them would leave the bucket useless; they
        // are accepted and always downloaded rather than displayed.
        allow_active_content: true,
        // Attachments are handed back exactly as they arrived; nothing here
        // decodes a picture, so its dimensions are not this bucket's business.
        max_dimensions: None,
        upload_permission: Some(permissions::FILES_UPLOAD),
    },
    BucketPolicy {
        name: "imports",
        label: "Imports",
        description: "Spreadsheets and data files staged for loading.",
        max_bytes: 10 * MB,
        categories: &[
            FileCategory::Text,
            FileCategory::Data,
            FileCategory::Spreadsheet,
        ],
        // Nothing here is ever opened by a browser; it is parsed. A macro-
        // bearing workbook has no business in a machine-read path.
        allow_active_content: false,
        max_dimensions: None,
        upload_permission: Some(permissions::FILES_UPLOAD),
    },
];

/// Look a bucket up by name.
///
/// `None` is a refusal, not a default. The bucket arrives in the upload request
/// and therefore comes from a caller, so an unrecognised one has to be an
/// answer of its own - falling back to "attachments" would let anybody put a
/// 25 MB file wherever the smallest limit was supposed to apply.
pub fn bucket(name: &str) -> Option<&'static BucketPolicy> {
    BUCKETS.iter().find(|bucket| bucket.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::catalog::by_extension;

    #[test]
    fn bucket_names_are_unique_and_path_safe() {
        for bucket in BUCKETS {
            assert!(
                bucket
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '-'),
                "{} is not usable as a path segment",
                bucket.name
            );
            assert_eq!(
                BUCKETS
                    .iter()
                    .filter(|other| other.name == bucket.name)
                    .count(),
                1,
                "duplicate bucket {}",
                bucket.name
            );
            assert!(
                !bucket.categories.is_empty(),
                "{} accepts nothing",
                bucket.name
            );
            assert!(bucket.max_bytes > 0, "{} has no limit", bucket.name);
        }
    }

    #[test]
    fn every_named_permission_is_one_that_exists() {
        // A misspelled permission would fail open in the other direction here -
        // `require` refuses everybody rather than letting them through - but it
        // would still be a bucket nobody can use, discovered in production.
        for bucket in BUCKETS {
            if let Some(permission) = bucket.upload_permission {
                assert!(
                    crate::authorization::is_defined(permission),
                    "{} requires {permission}, which is not in the permission tree",
                    bucket.name
                );
            }
        }
    }

    #[test]
    fn an_unknown_bucket_is_a_refusal_not_a_default() {
        assert!(bucket("attachments").is_some());
        assert!(bucket("Attachments").is_none());
        assert!(bucket("../etc").is_none());
        assert!(bucket("").is_none());
    }

    #[test]
    fn the_avatar_bucket_will_not_take_a_scriptable_picture() {
        let avatars = bucket("avatars").unwrap();
        let svg = by_extension("svg").unwrap();
        let png = by_extension("png").unwrap();

        assert!(avatars.accepts(png.category));
        // The category is right; the format is not, and that is the whole
        // point of keeping the two tests separate.
        assert!(avatars.accepts(svg.category));
        assert!(!avatars.allow_active_content);
    }

    #[test]
    fn the_accept_attribute_lists_only_what_would_be_taken() {
        let avatars = bucket("avatars").unwrap().accept_attribute();

        assert!(avatars.contains(".png"));
        assert!(avatars.contains(".jpg"));
        assert!(
            !avatars.contains(".svg"),
            "offered a file it would refuse: {avatars}"
        );
        assert!(!avatars.contains(".pdf"));

        let imports = bucket("imports").unwrap().accept_attribute();
        assert!(imports.contains(".csv"));
        assert!(!imports.contains(".png"));
    }

    #[test]
    fn the_hard_ceiling_covers_every_bucket() {
        let ceiling = largest_bucket_limit();
        assert!(BUCKETS.iter().all(|bucket| bucket.max_bytes <= ceiling));
    }
}
