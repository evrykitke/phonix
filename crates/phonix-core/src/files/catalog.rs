//! What a file *is*, decided from its bytes.
//!
//! The catalogue below is the single list of types this application will store,
//! and it is declared in code for the same reason the permission tree is: the
//! code is what enforces it. A type nobody listed here cannot be uploaded, and
//! adding one is a code change that a reviewer sees.
//!
//! # The extension is a claim, not evidence
//!
//! A browser sends two things about a file it is uploading: the name the user
//! chose, and a MIME type the *operating system* guessed from that name. Both
//! are attacker-controlled - `payload.exe` renamed to `holiday.png` arrives
//! declaring `image/png`, because that is what Windows says a `.png` is. So
//! neither decides anything here. [`detect`] reads the leading bytes and
//! answers from those; the declared name and type are kept only so a rejection
//! can say what the caller *claimed*, and so a download can offer the original
//! name back.
//!
//! # Where a signature is not enough
//!
//! Two families of format are containers that many types share, and their
//! leading bytes say only which container:
//!
//! * **Zip** - every OOXML document (`.docx`, `.xlsx`, `.pptx`) and every ODF
//!   one is a zip archive, and so is a zip archive.
//! * **CFB** - Microsoft's pre-2007 compound file, shared by `.doc`, `.xls`,
//!   `.ppt` and Outlook's `.msg`.
//!
//! For those, detection finds the container and then reads a little further for
//! the marker the specific format puts near the front. When that fails the
//! answer is the container itself, which is a real type with real rules - not a
//! guess dressed up as a document.
//!
//! # And where there are no bytes to read at all
//!
//! Plain text, CSV and JSON have no signature; there is nothing to look at
//! except the content. [`looks_like_text`] is what stands in for a signature
//! there: valid UTF-8 with no control bytes other than tab, carriage return and
//! newline. That is a genuine test - a PE header or an ELF header fails it on
//! the first byte - and it is the only reason a `.csv` can be accepted at all.

use crate::{Message, msg};
use serde::{Deserialize, Serialize};

/// The broad kind of thing a file is, for icons, filters and bucket policy.
///
/// Coarser than the MIME type on purpose: a bucket says "images" rather than
/// listing nine of them, so adding AVIF to the catalogue does not also mean
/// editing every bucket that already accepts pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCategory {
    Image,
    Document,
    Spreadsheet,
    Presentation,
    Text,
    Data,
    Archive,
    Audio,
    Video,
}

impl FileCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
            Self::Spreadsheet => "spreadsheet",
            Self::Presentation => "presentation",
            Self::Text => "text",
            Self::Data => "data",
            Self::Archive => "archive",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// What a screen calls it.
    pub fn label(self) -> Message {
        match self {
            Self::Image => msg!("file.type.image"),
            Self::Document => msg!("file.type.document"),
            Self::Spreadsheet => msg!("file.type.spreadsheet"),
            Self::Presentation => msg!("file.type.presentation"),
            Self::Text => msg!("file.type.text"),
            Self::Data => msg!("file.type.data"),
            Self::Archive => msg!("file.type.archive"),
            Self::Audio => msg!("file.type.audio"),
            Self::Video => msg!("file.type.video"),
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [
            Self::Image,
            Self::Document,
            Self::Spreadsheet,
            Self::Presentation,
            Self::Text,
            Self::Data,
            Self::Archive,
            Self::Audio,
            Self::Video,
        ]
        .into_iter()
        .find(|category| category.as_str() == raw)
    }
}

/// A fixed byte pattern at a fixed offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub offset: usize,
    pub magic: &'static [u8],
}

impl Signature {
    const fn at(offset: usize, magic: &'static [u8]) -> Self {
        Self { offset, magic }
    }

    /// Whether `bytes` carries this pattern.
    ///
    /// `get` rather than a slice index: this crate compiles to WebAssembly,
    /// where an out-of-bounds index is not a caught panic but a frozen tab, and
    /// a truncated upload is an ordinary thing to be handed.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes
            .get(self.offset..self.offset.saturating_add(self.magic.len()))
            .is_some_and(|window| window == self.magic)
    }
}

/// A container format that several types share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// Zip: OOXML, ODF, and plain archives.
    Zip,
    /// Microsoft compound file binary: the pre-2007 Office formats.
    Cfb,
}

/// One entry in the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileType {
    /// Canonical MIME type. Stored on the row and sent as `Content-Type` on
    /// download - never the one the client declared.
    pub mime: &'static str,
    pub label: &'static str,
    pub category: FileCategory,
    /// Extensions this type is known by. The **first** is canonical and is what
    /// a stored file is renamed to; the rest exist so a `.jpeg` and a `.jpg`
    /// are understood to be the same claim.
    pub extensions: &'static [&'static str],
    /// Patterns that identify it. Empty for the text formats, which have none -
    /// see [`looks_like_text`].
    pub signatures: &'static [Signature],
    /// The container it lives inside, when its own signature is the
    /// container's. Detection reaches these only through [`refine_zip`] and
    /// [`refine_cfb`].
    pub container: Option<Container>,
    /// Whether the format can carry code that a browser would run: script in an
    /// SVG, script in HTML, a macro in a legacy Office file.
    ///
    /// Not a reason to refuse it outright - a workspace may have every right to
    /// store an SVG - but a bucket has to say so, and such a file is never
    /// served inline. See [`crate::files::BucketPolicy::allow_active_content`].
    pub active_content: bool,
}

impl FileType {
    /// The extension a stored copy is given, without the dot.
    ///
    /// Derived from what the bytes turned out to be, never from what the caller
    /// called the file - which is the whole point of renaming.
    pub fn extension(&self) -> &'static str {
        // `first` rather than `[0]`: every entry below has at least one, but a
        // future entry with none must not be a frozen tab.
        self.extensions.first().copied().unwrap_or("bin")
    }

    /// Whether `extension` (without the dot, any case) is one of this type's.
    pub fn owns_extension(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
    }

    /// Whether this type is safe to render in a browser tab.
    ///
    /// Only pictures, and only ones that cannot carry script. Everything else -
    /// including PDF, which has its own scripting engine - is downloaded.
    pub fn is_inline_safe(&self) -> bool {
        !self.active_content && matches!(self.category, FileCategory::Image)
    }
}

// ---------------------------------------------------------------------------
// The catalogue
// ---------------------------------------------------------------------------

/// Every type this application will store.
///
/// Order matters only for [`detect`], which returns the first signature that
/// matches; the containers are therefore listed after the formats that would
/// otherwise be shadowed by them.
pub const CATALOGUE: &[FileType] = &[
    // -- Images -----------------------------------------------------------
    FileType {
        mime: "image/png",
        label: "PNG image",
        category: FileCategory::Image,
        extensions: &["png"],
        // The trailing CR LF SUB LF is not decoration: it is how PNG detects a
        // transfer that mangled line endings, and it makes the signature
        // strong enough that nothing else collides with it.
        signatures: &[Signature::at(0, b"\x89PNG\r\n\x1a\n")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/jpeg",
        label: "JPEG image",
        category: FileCategory::Image,
        extensions: &["jpg", "jpeg", "jpe"],
        signatures: &[Signature::at(0, b"\xff\xd8\xff")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/gif",
        label: "GIF image",
        category: FileCategory::Image,
        extensions: &["gif"],
        signatures: &[Signature::at(0, b"GIF87a"), Signature::at(0, b"GIF89a")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/webp",
        label: "WebP image",
        category: FileCategory::Image,
        extensions: &["webp"],
        // RIFF containers all start "RIFF"; the four bytes at offset 8 are what
        // separate a picture from a sound file, so both are required.
        signatures: &[Signature::at(8, b"WEBP")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/avif",
        label: "AVIF image",
        category: FileCategory::Image,
        extensions: &["avif"],
        signatures: &[Signature::at(4, b"ftypavif"), Signature::at(4, b"ftypavis")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/bmp",
        label: "Bitmap image",
        category: FileCategory::Image,
        extensions: &["bmp"],
        signatures: &[Signature::at(0, b"BM")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/tiff",
        label: "TIFF image",
        category: FileCategory::Image,
        extensions: &["tiff", "tif"],
        // Little-endian and big-endian, which is what the two orders mean.
        signatures: &[
            Signature::at(0, b"II\x2a\x00"),
            Signature::at(0, b"MM\x00\x2a"),
        ],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "image/svg+xml",
        label: "SVG image",
        category: FileCategory::Image,
        extensions: &["svg"],
        // No signature: an SVG is XML, and XML may legally begin with a
        // comment, a declaration or whitespace. It is reached through the text
        // path, and it carries `active_content` because an SVG may contain a
        // <script> element that runs on the origin that serves it.
        signatures: &[],
        container: None,
        active_content: true,
    },
    // -- Documents --------------------------------------------------------
    FileType {
        mime: "application/pdf",
        label: "PDF document",
        category: FileCategory::Document,
        extensions: &["pdf"],
        signatures: &[Signature::at(0, b"%PDF-")],
        container: None,
        // PDF has a JavaScript engine and an /OpenAction that fires on open.
        // Whether the reader honours it is the reader's business; this side
        // treats it as executable content and never renders it inline.
        active_content: true,
    },
    FileType {
        mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        label: "Word document",
        category: FileCategory::Document,
        extensions: &["docx"],
        signatures: &[],
        container: Some(Container::Zip),
        active_content: false,
    },
    FileType {
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        label: "Excel workbook",
        category: FileCategory::Spreadsheet,
        extensions: &["xlsx"],
        signatures: &[],
        container: Some(Container::Zip),
        active_content: false,
    },
    FileType {
        mime: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        label: "PowerPoint presentation",
        category: FileCategory::Presentation,
        extensions: &["pptx"],
        signatures: &[],
        container: Some(Container::Zip),
        active_content: false,
    },
    FileType {
        mime: "application/vnd.oasis.opendocument.text",
        label: "OpenDocument text",
        category: FileCategory::Document,
        extensions: &["odt"],
        signatures: &[],
        container: Some(Container::Zip),
        active_content: false,
    },
    FileType {
        mime: "application/vnd.oasis.opendocument.spreadsheet",
        label: "OpenDocument spreadsheet",
        category: FileCategory::Spreadsheet,
        extensions: &["ods"],
        signatures: &[],
        container: Some(Container::Zip),
        active_content: false,
    },
    FileType {
        mime: "application/msword",
        label: "Word document (legacy)",
        category: FileCategory::Document,
        extensions: &["doc"],
        signatures: &[],
        container: Some(Container::Cfb),
        // A legacy Office file is a macro container by design.
        active_content: true,
    },
    FileType {
        mime: "application/vnd.ms-excel",
        label: "Excel workbook (legacy)",
        category: FileCategory::Spreadsheet,
        extensions: &["xls"],
        signatures: &[],
        container: Some(Container::Cfb),
        active_content: true,
    },
    FileType {
        mime: "application/vnd.ms-powerpoint",
        label: "PowerPoint presentation (legacy)",
        category: FileCategory::Presentation,
        extensions: &["ppt"],
        signatures: &[],
        container: Some(Container::Cfb),
        active_content: true,
    },
    // -- Text and data ----------------------------------------------------
    //
    // None of these has a signature. They are reached only through the text
    // path, which is why `looks_like_text` is a security control and not a
    // convenience.
    FileType {
        mime: "text/plain",
        label: "Plain text",
        category: FileCategory::Text,
        extensions: &["txt", "log", "md"],
        signatures: &[],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "text/csv",
        label: "CSV",
        category: FileCategory::Data,
        extensions: &["csv"],
        signatures: &[],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "application/json",
        label: "JSON",
        category: FileCategory::Data,
        extensions: &["json"],
        signatures: &[],
        container: None,
        active_content: false,
    },
    // -- Archives ---------------------------------------------------------
    //
    // Listed after the zip-container documents so that `detect` only falls back
    // to "an archive" once refinement has failed to find a document inside it.
    FileType {
        mime: "application/zip",
        label: "Zip archive",
        category: FileCategory::Archive,
        extensions: &["zip"],
        signatures: &[
            Signature::at(0, b"PK\x03\x04"),
            // An empty archive, and an archive whose first record is a spanning
            // marker. Both are legitimate zips and neither starts PK 03 04.
            Signature::at(0, b"PK\x05\x06"),
            Signature::at(0, b"PK\x07\x08"),
        ],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "application/gzip",
        label: "Gzip archive",
        category: FileCategory::Archive,
        extensions: &["gz", "gzip"],
        signatures: &[Signature::at(0, b"\x1f\x8b")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "application/x-7z-compressed",
        label: "7-Zip archive",
        category: FileCategory::Archive,
        extensions: &["7z"],
        signatures: &[Signature::at(0, b"7z\xbc\xaf\x27\x1c")],
        container: None,
        active_content: false,
    },
    FileType {
        mime: "application/x-ole-storage",
        label: "Compound file",
        category: FileCategory::Document,
        extensions: &["cfb"],
        signatures: &[Signature::at(0, b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1")],
        container: None,
        active_content: true,
    },
];

/// Look a type up by its canonical MIME string.
///
/// Used when a stored row is read back: the row holds what detection decided,
/// and this turns it into the entry whose rules the download path applies.
pub fn by_mime(mime: &str) -> Option<&'static FileType> {
    CATALOGUE
        .iter()
        .find(|file_type| file_type.mime.eq_ignore_ascii_case(mime))
}

/// Look a type up by an extension (without the dot, any case).
///
/// Only ever used to *interpret a claim* - to refine a container, or to say in
/// a rejection what the caller appeared to be sending. It never decides what a
/// file is.
pub fn by_extension(extension: &str) -> Option<&'static FileType> {
    CATALOGUE
        .iter()
        .find(|file_type| file_type.owns_extension(extension))
}

/// What the bytes say this is.
///
/// `declared_extension` is consulted for one thing only: choosing between the
/// several formats that share a container. A `.docx` and a `.xlsx` are the same
/// four leading bytes, and no amount of reading the header separates them - so
/// the claim breaks the tie *within what the container allows*, and can never
/// promote a file out of the container it actually is.
///
/// `None` means nothing in the catalogue matched, which is a rejection - not a
/// reason to fall back to `application/octet-stream` and store it anyway.
pub fn detect(bytes: &[u8], declared_extension: Option<&str>) -> Option<&'static FileType> {
    let matched = CATALOGUE.iter().find(|file_type| {
        file_type
            .signatures
            .iter()
            .any(|signature| signature.matches(bytes))
    })?;

    let refined = match matched.mime {
        "application/zip" => refine_zip(bytes, declared_extension),
        "application/x-ole-storage" => refine_cfb(bytes, declared_extension),
        _ => None,
    };

    Some(refined.unwrap_or(matched))
}

/// Which document, if any, is inside this zip.
///
/// OOXML and ODF both put a recognisable entry first, so the marker is near the
/// front of the file and there is no need to parse the central directory:
///
/// * ODF writes an uncompressed `mimetype` entry first, whose contents are the
///   MIME string itself - so the answer is literally in the first 100 bytes.
/// * OOXML writes `[Content_Types].xml` first, which says only "this is
///   OOXML"; which of the three it is shows in the next few entry names
///   (`word/`, `xl/`, `ppt/`).
///
/// The declared extension is the tie-breaker when neither marker is close
/// enough to the front to be seen, which happens with archives written by tools
/// that order entries differently.
fn refine_zip(bytes: &[u8], declared_extension: Option<&str>) -> Option<&'static FileType> {
    // Enough to cover the first few local file headers of any normal document
    // and not enough to be worth a memory copy.
    let head = bytes.get(..bytes.len().min(4096)).unwrap_or(bytes);

    // ODF states its own type, so it needs no guessing at all.
    for candidate in CATALOGUE {
        if candidate.container == Some(Container::Zip)
            && candidate.mime.starts_with("application/vnd.oasis")
            && contains(head, candidate.mime.as_bytes())
        {
            return Some(candidate);
        }
    }

    if contains(head, b"[Content_Types].xml") {
        let by_part = if contains(head, b"word/") {
            by_extension("docx")
        } else if contains(head, b"xl/") {
            by_extension("xlsx")
        } else if contains(head, b"ppt/") {
            by_extension("pptx")
        } else {
            None
        };

        if let Some(file_type) = by_part {
            return Some(file_type);
        }
    }

    // Nothing recognisable near the front. The claim may still be right, but
    // only within the zip family - `is_ooxml_or_odf` is what stops a `.png`
    // claim turning an archive into a picture.
    let claimed = by_extension(declared_extension?)?;
    (claimed.container == Some(Container::Zip)).then_some(claimed)
}

/// Which legacy Office document is inside this compound file.
///
/// A CFB directory holds entry names as UTF-16LE, and each format writes a
/// distinctive stream: `WordDocument`, `Workbook`, `PowerPoint Document`. They
/// live in the directory sector, which is not at a fixed offset, so this
/// searches rather than seeks.
fn refine_cfb(bytes: &[u8], declared_extension: Option<&str>) -> Option<&'static FileType> {
    let head = bytes.get(..bytes.len().min(16_384)).unwrap_or(bytes);

    for (marker, extension) in [
        ("WordDocument", "doc"),
        ("Workbook", "xls"),
        ("PowerPoint Document", "ppt"),
    ] {
        if contains(head, &utf16le(marker)) {
            return by_extension(extension);
        }
    }

    let claimed = by_extension(declared_extension?)?;
    (claimed.container == Some(Container::Cfb)).then_some(claimed)
}

/// Whether `needle` appears anywhere in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// An ASCII string as the UTF-16LE bytes a CFB directory would hold.
fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

/// Whether these bytes are plausibly text.
///
/// The stand-in for a signature, for the formats that have none. Two tests, and
/// both have to pass:
///
/// 1. **Valid UTF-8.** Not a formality: a PE executable fails on its second
///    byte, and so does every compiled artefact anybody would try to smuggle
///    through as a `.txt`.
/// 2. **No control bytes** except tab, carriage return and newline. This is
///    what rules out the binaries that happen to be valid UTF-8 - a NUL almost
///    anywhere is the giveaway.
///
/// A UTF-8 BOM is stripped before the test, because Excel writes one on every
/// CSV it exports and refusing those would make the feature useless.
pub fn looks_like_text(bytes: &[u8]) -> bool {
    let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);

    // An empty file is not text; it is an empty file, and the upload path
    // rejects it by size before ever reaching here.
    if body.is_empty() {
        return false;
    }

    let Ok(text) = core::str::from_utf8(body) else {
        return false;
    };

    !text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\r' | '\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_internally_consistent() {
        for file_type in CATALOGUE {
            assert!(
                !file_type.extensions.is_empty(),
                "{} has no extension to rename a stored copy to",
                file_type.mime
            );
            assert!(
                file_type.extensions.iter().all(|ext| ext
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())),
                "{} has an extension that is not plain lowercase ascii",
                file_type.mime
            );
            // A type with neither a signature nor a container can only be
            // reached through the text path, so it had better be text-shaped.
            if file_type.signatures.is_empty() && file_type.container.is_none() {
                assert!(
                    matches!(
                        file_type.category,
                        FileCategory::Text | FileCategory::Data | FileCategory::Image
                    ),
                    "{} is unreachable: no signature, no container",
                    file_type.mime
                );
            }
        }
    }

    #[test]
    fn mimes_and_extensions_are_unique() {
        for (index, file_type) in CATALOGUE.iter().enumerate() {
            assert_eq!(
                by_mime(file_type.mime).map(|found| found.mime),
                Some(file_type.mime)
            );

            for extension in file_type.extensions {
                let owners = CATALOGUE
                    .iter()
                    .filter(|other| other.owns_extension(extension))
                    .count();
                assert_eq!(
                    owners, 1,
                    "extension .{extension} is claimed by more than one type (entry {index})"
                );
            }
        }
    }

    #[test]
    fn pictures_are_recognised_from_their_bytes() {
        assert_eq!(
            detect(b"\x89PNG\r\n\x1a\n\x00\x00", None).map(|t| t.mime),
            Some("image/png")
        );
        assert_eq!(
            detect(b"\xff\xd8\xff\xe0JFIF", None).map(|t| t.mime),
            Some("image/jpeg")
        );
        assert_eq!(
            detect(b"GIF89a....", None).map(|t| t.mime),
            Some("image/gif")
        );
        assert_eq!(
            detect(b"RIFF\x00\x00\x00\x00WEBPVP8 ", None).map(|t| t.mime),
            Some("image/webp")
        );
    }

    #[test]
    fn a_renamed_executable_is_not_a_picture() {
        // The attack the whole module exists for: a Windows PE, called
        // holiday.png, declaring image/png because that is what the extension
        // made the browser say.
        let pe = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff";

        assert!(detect(pe, Some("png")).is_none());
        assert!(!looks_like_text(pe));
    }

    #[test]
    fn a_riff_that_is_not_a_picture_is_not_accepted_as_one() {
        // WAVE, not WEBP. Only the four bytes at offset 8 tell them apart, and
        // getting that wrong would let a sound file through as an image.
        let wav = b"RIFF\x24\x08\x00\x00WAVEfmt ";
        assert!(detect(wav, Some("webp")).is_none());
    }

    #[test]
    fn a_zip_is_refined_to_the_document_inside_it() {
        let mut docx = b"PK\x03\x04\x14\x00\x06\x00".to_vec();
        docx.extend_from_slice(b"[Content_Types].xml");
        docx.extend_from_slice(b"....word/document.xml");

        assert_eq!(
            detect(&docx, Some("docx")).map(|t| t.mime),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );

        let mut odt = b"PK\x03\x04\x14\x00\x00\x00mimetype".to_vec();
        odt.extend_from_slice(b"application/vnd.oasis.opendocument.text");
        assert_eq!(
            detect(&odt, None).map(|t| t.mime),
            Some("application/vnd.oasis.opendocument.text")
        );
    }

    #[test]
    fn a_claim_can_only_break_a_tie_inside_the_container() {
        // A plain archive with nothing recognisable in it, claimed as a
        // spreadsheet: allowed, because .xlsx really is a zip.
        let bare = b"PK\x03\x04nothing recognisable here at all";
        assert_eq!(
            detect(bare, Some("xlsx")).map(|t| t.mime),
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        );

        // The same archive claimed as a picture: refused the promotion and
        // stored as what it is.
        assert_eq!(
            detect(bare, Some("png")).map(|t| t.mime),
            Some("application/zip")
        );
        assert_eq!(detect(bare, None).map(|t| t.mime), Some("application/zip"));
    }

    #[test]
    fn a_compound_file_is_refined_by_the_stream_it_carries() {
        let mut xls = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1".to_vec();
        xls.extend_from_slice(&utf16le("Workbook"));

        assert_eq!(
            detect(&xls, Some("doc")).map(|t| t.mime),
            // The bytes win over the claim, which is the point.
            Some("application/vnd.ms-excel")
        );
    }

    #[test]
    fn text_is_recognised_and_binary_masquerading_as_text_is_not() {
        assert!(looks_like_text(b"name,email\nada,ada@example.com\n"));
        assert!(looks_like_text("héllo\ttabbed\r\n".as_bytes()));
        // Excel's BOM.
        assert!(looks_like_text(b"\xef\xbb\xbfname,email\n"));

        assert!(!looks_like_text(b""));
        assert!(!looks_like_text(b"\x7fELF\x02\x01\x01"));
        // Valid UTF-8, but a NUL is not something a person typed.
        assert!(!looks_like_text(b"hello\x00world"));
        // Not valid UTF-8 at all.
        assert!(!looks_like_text(b"\xc3\x28"));
    }

    #[test]
    fn active_content_never_renders_inline() {
        let svg = by_extension("svg").unwrap();
        let png = by_extension("png").unwrap();
        let pdf = by_extension("pdf").unwrap();

        assert!(png.is_inline_safe());
        // Both of these are things a browser will execute if given the chance.
        assert!(!svg.is_inline_safe());
        assert!(!pdf.is_inline_safe());
    }

    #[test]
    fn a_stored_copy_is_renamed_to_the_canonical_extension() {
        let jpeg = by_extension("jpeg").unwrap();
        assert_eq!(jpeg.extension(), "jpg");
        assert!(jpeg.owns_extension("JPG"));
    }
}
