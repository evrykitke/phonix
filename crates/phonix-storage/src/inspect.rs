//! Deciding whether a file may be kept, and what it is.
//!
//! One function, [`inspect`], which is the whole of the acceptance policy. It
//! runs against bytes that are already on disk in quarantine, and it is the
//! only thing standing between "somebody sent this" and "this is stored".
//!
//! # The order of the checks is the design
//!
//! ```text
//!   1. is there anything here at all        an empty file
//!   2. is it within the bucket's size       cheap, and no reading needed
//!   3. what do the bytes say it is          the only question that decides
//!   4. does the name agree with the bytes   a disagreement is refused
//!   5. can it carry code that runs          and does this bucket take that
//!   6. is that kind of thing wanted here    the bucket's category list
//!   7. how large is the picture             pixels, not bytes
//! ```
//!
//! Cheap tests before expensive ones, and the type decided before anything that
//! depends on the type. Steps 1 and 2 need no bytes at all, which is what lets a
//! 3 GB "avatar" be refused without reading a byte of it.
//!
//! # What is *not* here
//!
//! No virus scanning. That is a real thing to want and it is a different shape
//! of work - an external service, a timeout, a verdict that can be "not yet".
//! When it arrives it belongs as another step in the job that calls this, not
//! inside a pure function; this one has to stay something a test can call a
//! thousand times with no I/O.

use phonix_core::files::catalog::{FileType, by_extension, detect, looks_like_text};
use phonix_core::files::image::{self, Dimensions};
use phonix_core::files::{BucketPolicy, Container, FileCategory, Rejection, extension_of};

/// How much of a file [`inspect`] wants.
///
/// Generous, and for one format only. Almost every signature is in the first
/// 16 bytes and every container marker is in the first few kilobytes - but a
/// JPEG's frame header sits behind whatever metadata the camera wrote, and a
/// phone photo can carry a 60 KB thumbnail ahead of it. Reading less would mean
/// silently failing to measure exactly the pictures people upload most.
pub const HEAD_BYTES: usize = 128 * 1024;

/// What a file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inspection {
    /// The catalogue entry the bytes matched.
    pub file_type: &'static FileType,
    /// Its dimensions, when it is a picture whose header this can read.
    pub dimensions: Option<Dimensions>,
}

impl Inspection {
    pub fn mime(&self) -> &'static str {
        self.file_type.mime
    }

    pub fn category(&self) -> FileCategory {
        self.file_type.category
    }

    /// The extension a stored copy is renamed to.
    pub fn extension(&self) -> &'static str {
        self.file_type.extension()
    }
}

/// Decide whether this file may be stored, and what it is.
///
/// `head` is the first [`HEAD_BYTES`] of the file; `byte_size` is the length of
/// the whole of it, which is known from the quarantine object rather than from
/// anything the caller said.
///
/// `declared_mime` is the `Content-Type` the browser attached. It decides
/// nothing whatsoever - it appears in one rejection message, so that somebody
/// looking at a refusal can see what their machine claimed.
pub fn inspect(
    bucket: &BucketPolicy,
    original_name: &str,
    declared_mime: Option<&str>,
    head: &[u8],
    byte_size: u64,
) -> Result<Inspection, Rejection> {
    // -- 1 and 2: no bytes needed ----------------------------------------
    if byte_size == 0 || head.is_empty() {
        return Err(Rejection::Empty);
    }

    if byte_size > bucket.max_bytes {
        return Err(Rejection::TooLarge {
            limit_bytes: bucket.max_bytes,
            actual_bytes: byte_size,
        });
    }

    // -- 3: what the bytes say -------------------------------------------
    let declared_extension = extension_of(original_name);
    let claimed = declared_extension.as_deref().and_then(by_extension);
    let truncated = (head.len() as u64) < byte_size;

    let detected = detect(head, declared_extension.as_deref())
        .or_else(|| text_fallback(head, claimed, truncated))
        .ok_or_else(|| Rejection::UnrecognisedType {
            declared: declared_mime.map(str::to_owned),
        })?;

    // -- 4: does the name agree ------------------------------------------
    if let Some(claimed) = claimed
        && !compatible(claimed, detected)
    {
        return Err(Rejection::Masquerade {
            declared: claimed.mime.to_owned(),
            detected: detected.mime.to_owned(),
        });
    }

    // -- 5: can it carry code --------------------------------------------
    if detected.active_content && !bucket.allow_active_content {
        return Err(Rejection::ActiveContentNotAllowed {
            detected: detected.mime.to_owned(),
        });
    }

    // -- 6: is it wanted here --------------------------------------------
    if !bucket.accepts(detected.category) {
        return Err(Rejection::TypeNotAllowed {
            detected: detected.mime.to_owned(),
            bucket: bucket.name.to_owned(),
        });
    }

    // -- 7: pixels, not bytes --------------------------------------------
    let dimensions = (detected.category == FileCategory::Image)
        .then(|| image::dimensions(head, detected.mime))
        .flatten();

    if let Some((max_width, max_height)) = bucket.max_dimensions
        && let Some(measured) = dimensions
        && !measured.fits_within(max_width, max_height)
    {
        return Err(Rejection::ImageTooLarge {
            width: measured.width,
            height: measured.height,
            max_width,
            max_height,
        });
    }

    Ok(Inspection {
        file_type: detected,
        dimensions,
    })
}

/// What to make of a file whose leading bytes match nothing.
///
/// Some formats have no signature at all - plain text, CSV, JSON, SVG - so
/// "nothing matched" is not by itself a refusal. What stands in for a signature
/// is the content test in
/// [`looks_like_text`](phonix_core::files::catalog::looks_like_text), and the
/// claimed extension is then allowed to pick *which* text format it is.
///
/// That is the one place a claim is trusted, and it is safe because every
/// candidate is text either way: the choice between `text/csv` and `text/plain`
/// changes a label, not what the file can do. A claim of `.png` cannot be
/// honoured here, because PNG has a signature and this branch is only reached
/// when no signature matched.
fn text_fallback(
    head: &[u8],
    claimed: Option<&'static FileType>,
    truncated: bool,
) -> Option<&'static FileType> {
    if !looks_like_text(utf8_probe(head, truncated)) {
        return None;
    }

    match claimed {
        // A catalogue entry with neither a signature nor a container is a text
        // format by construction - see the consistency test in
        // `phonix_core::files::catalog`.
        Some(file_type) if file_type.signatures.is_empty() && file_type.container.is_none() => {
            // SVG is the exception among the exceptions: it is text, and it is
            // also the one text format that runs. Prose in a file called
            // `.svg` must not become an image, so it has to look like one -
            // and when it does not, saying so honestly here is what lets the
            // name check below refuse it as the masquerade it is.
            if file_type.mime == "image/svg+xml" && !looks_like_svg(head) {
                by_extension("txt")
            } else {
                Some(file_type)
            }
        }
        _ => by_extension("txt"),
    }
}

/// Whether text is actually an SVG document.
///
/// A tag search rather than an XML parse: an SVG may open with a declaration, a
/// doctype, comments or whitespace, and parsing untrusted XML to find that out
/// would be a larger risk than the one being checked.
fn looks_like_svg(head: &[u8]) -> bool {
    let window = head.get(..head.len().min(4096)).unwrap_or(head);
    let Ok(text) = core::str::from_utf8(window) else {
        return false;
    };

    text.to_ascii_lowercase().contains("<svg")
}

/// The part of a truncated read that is whole characters.
///
/// [`inspect`] sees the first 128 KB of a file, so a text file larger than that
/// arrives cut - possibly through the middle of a multi-byte character, which
/// would make a perfectly good UTF-8 document fail the text test on its last
/// two bytes.
///
/// The trim happens **only** when the cut is at the very end, which
/// `error_len() == None` is precisely the meaning of. A byte that is invalid in
/// the middle is left in place, so a binary file cannot pass by having its
/// first NUL treated as the end of a valid prefix.
fn utf8_probe(head: &[u8], truncated: bool) -> &[u8] {
    if !truncated {
        return head;
    }

    match core::str::from_utf8(head) {
        Ok(_) => head,
        Err(err) if err.error_len().is_none() => head.get(..err.valid_up_to()).unwrap_or(&[]),
        Err(_) => head,
    }
}

/// Whether a claimed type and a detected one describe the same file.
///
/// Exact agreement, or agreement about the container. The second case is not a
/// loophole: a `.docx` whose contents refine to `.xlsx` is still an OOXML zip,
/// and the content has already won - all that differs is which refinement was
/// picked. What it does *not* permit is a claim crossing a container boundary,
/// which is the case that matters: an archive called `photo.png`.
fn compatible(claimed: &'static FileType, detected: &'static FileType) -> bool {
    if claimed.mime == detected.mime {
        return true;
    }

    match (claimed.container, detected.container) {
        (Some(claimed_container), Some(detected_container)) => {
            claimed_container == detected_container
        }
        (Some(container), None) => is_container_itself(detected, container),
        (None, Some(container)) => is_container_itself(claimed, container),
        (None, None) => false,
    }
}

/// Whether a type *is* the container rather than something inside it.
fn is_container_itself(file_type: &'static FileType, container: Container) -> bool {
    match container {
        Container::Zip => file_type.mime == "application/zip",
        Container::Cfb => file_type.mime == "application/x-ole-storage",
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::files::bucket;

    use super::*;

    fn avatars() -> &'static BucketPolicy {
        bucket("avatars").expect("the avatars bucket is declared in code")
    }

    fn attachments() -> &'static BucketPolicy {
        bucket("attachments").expect("the attachments bucket is declared in code")
    }

    fn imports() -> &'static BucketPolicy {
        bucket("imports").expect("the imports bucket is declared in code")
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn an_ordinary_picture_is_accepted_and_named_by_its_bytes() {
        let bytes = png(512, 512);
        let inspection = inspect(
            avatars(),
            "My Photo.PNG",
            Some("image/png"),
            &bytes,
            bytes.len() as u64,
        )
        .unwrap();

        assert_eq!(inspection.mime(), "image/png");
        assert_eq!(inspection.extension(), "png");
        assert_eq!(
            inspection.dimensions,
            Some(Dimensions {
                width: 512,
                height: 512
            })
        );
    }

    #[test]
    fn an_executable_called_a_picture_is_refused() {
        // The whole feature in one test. A Windows PE, named holiday.png,
        // declaring image/png because that is what the extension made the
        // browser say.
        let pe = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00";

        let outcome = inspect(
            attachments(),
            "holiday.png",
            Some("image/png"),
            pe,
            pe.len() as u64,
        );

        assert!(matches!(outcome, Err(Rejection::UnrecognisedType { .. })));
    }

    #[test]
    fn an_archive_wearing_a_pictures_name_is_refused_as_a_masquerade() {
        let zip = b"PK\x03\x04nothing recognisable in here at all";

        let outcome = inspect(
            attachments(),
            "photo.png",
            Some("image/png"),
            zip,
            zip.len() as u64,
        );

        match outcome {
            Err(Rejection::Masquerade { declared, detected }) => {
                assert_eq!(declared, "image/png");
                assert_eq!(detected, "application/zip");
            }
            other => panic!("expected a masquerade, got {other:?}"),
        }
    }

    #[test]
    fn a_document_inside_a_zip_is_not_a_masquerade() {
        let mut docx = b"PK\x03\x04\x14\x00\x06\x00".to_vec();
        docx.extend_from_slice(b"[Content_Types].xml");
        docx.extend_from_slice(b"....word/document.xml");

        let inspection =
            inspect(attachments(), "report.docx", None, &docx, docx.len() as u64).unwrap();

        assert_eq!(inspection.extension(), "docx");

        // And a plain `.zip` refining to nothing stays a zip, with no
        // complaint that the name said zip and the content said zip.
        let bare = b"PK\x03\x04and nothing else";
        let inspection =
            inspect(attachments(), "bundle.zip", None, bare, bare.len() as u64).unwrap();
        assert_eq!(inspection.mime(), "application/zip");
    }

    #[test]
    fn a_scriptable_picture_cannot_be_an_avatar() {
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg">
            <script>fetch('/admin/users')</script></svg>"#;

        // The category is right - an SVG is an image - so only the active
        // content flag stands between this and script running on this origin
        // under somebody else's name.
        match inspect(
            avatars(),
            "me.svg",
            Some("image/svg+xml"),
            svg,
            svg.len() as u64,
        ) {
            Err(Rejection::ActiveContentNotAllowed { detected }) => {
                assert_eq!(detected, "image/svg+xml");
            }
            other => panic!("expected an active-content refusal, got {other:?}"),
        }

        // Attachments do take one: it is never rendered inline there.
        assert!(inspect(attachments(), "logo.svg", None, svg, svg.len() as u64).is_ok());
    }

    #[test]
    fn prose_in_a_file_called_svg_does_not_become_an_image() {
        let prose = b"Dear Ada,\n\nthe meeting is at three.\n";

        // The dangerous outcome would be image/svg+xml - a file a browser is
        // asked to render, chosen on the strength of its name alone. Detection
        // calls the bytes what they are, and the name check then refuses the
        // pair as the disagreement it is.
        match inspect(attachments(), "note.svg", None, prose, prose.len() as u64) {
            Err(Rejection::Masquerade { declared, detected }) => {
                assert_eq!(declared, "image/svg+xml");
                assert_eq!(detected, "text/plain");
            }
            other => panic!("expected a masquerade, got {other:?}"),
        }

        // The same bytes under a name that claims nothing are ordinary text.
        // The refusal above is about the disagreement, not about the content.
        let inspection =
            inspect(attachments(), "note.txt", None, prose, prose.len() as u64).unwrap();
        assert_eq!(inspection.mime(), "text/plain");
    }

    #[test]
    fn a_real_type_in_the_wrong_place_says_so() {
        let pdf = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";

        match inspect(
            avatars(),
            "cv.pdf",
            Some("application/pdf"),
            pdf,
            pdf.len() as u64,
        ) {
            // Refused on active content before the category is even reached,
            // because a PDF can run and the avatar bucket takes nothing that
            // can. Both answers are correct; this is the one that fires first.
            Err(Rejection::ActiveContentNotAllowed { .. }) => {}
            other => panic!("expected a refusal, got {other:?}"),
        }

        // A picture in the imports bucket has no such excuse: it is simply not
        // the kind of thing that belongs there.
        let bytes = png(64, 64);
        match inspect(imports(), "chart.png", None, &bytes, bytes.len() as u64) {
            Err(Rejection::TypeNotAllowed { detected, bucket }) => {
                assert_eq!(detected, "image/png");
                assert_eq!(bucket, "imports");
            }
            other => panic!("expected a category refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_decompression_bomb_is_refused_on_its_pixels() {
        // 30,000 square: a few hundred kilobytes of file, 3.6 GB decoded, and
        // comfortably inside the avatar bucket's byte limit.
        let bomb = png(30_000, 30_000);

        match inspect(avatars(), "me.png", None, &bomb, 300_000) {
            Err(Rejection::ImageTooLarge {
                width,
                height,
                max_width,
                max_height,
            }) => {
                assert_eq!((width, height), (30_000, 30_000));
                assert_eq!((max_width, max_height), (1024, 1024));
            }
            other => panic!("expected a pixel refusal, got {other:?}"),
        }

        // The same picture is fine as an attachment, which stores it and never
        // decodes it.
        assert!(inspect(attachments(), "poster.png", None, &bomb, 300_000).is_ok());
    }

    #[test]
    fn size_is_judged_before_anything_is_read() {
        // `head` here is a lie - it is a valid PNG - and the file is 3 GB. The
        // refusal must come from the length, which is known from the object on
        // disk rather than from anything the caller said.
        let bytes = png(64, 64);

        match inspect(avatars(), "huge.png", None, &bytes, 3_000_000_000) {
            Err(Rejection::TooLarge {
                limit_bytes,
                actual_bytes,
            }) => {
                assert_eq!(limit_bytes, 2 * 1024 * 1024);
                assert_eq!(actual_bytes, 3_000_000_000);
            }
            other => panic!("expected a size refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_file_is_refused_before_anything_else() {
        assert!(matches!(
            inspect(attachments(), "nothing.txt", None, b"", 0),
            Err(Rejection::Empty)
        ));
        // Zero length with bytes in hand, and bytes in hand with zero length:
        // both are the same nothing.
        assert!(matches!(
            inspect(attachments(), "nothing.txt", None, b"abc", 0),
            Err(Rejection::Empty)
        ));
    }

    #[test]
    fn a_csv_is_recognised_by_content_and_labelled_by_its_name() {
        let csv = b"name,email\nada,ada@example.com\n";

        let inspection = inspect(
            imports(),
            "people.csv",
            Some("text/csv"),
            csv,
            csv.len() as u64,
        )
        .unwrap();

        // The claim picked which text format, which is all it is allowed to
        // do; the content is what decided it was text at all.
        assert_eq!(inspection.mime(), "text/csv");

        let unnamed = inspect(imports(), "people", None, csv, csv.len() as u64).unwrap();
        assert_eq!(unnamed.mime(), "text/plain");
    }

    #[test]
    fn a_large_text_file_is_not_refused_over_where_the_read_stopped() {
        // 128 KB of a UTF-8 document, cut through the middle of a character.
        // Without the boundary trim this is "not valid UTF-8" and a perfectly
        // ordinary text file is refused.
        let mut text = "héllo wörld, this is a long document. "
            .repeat(4000)
            .into_bytes();
        let full_length = text.len() as u64 + 10_000;
        text.truncate(HEAD_BYTES.min(text.len()));

        // Force the cut to land inside a multi-byte character.
        while core::str::from_utf8(&text).is_ok() {
            text.pop();
        }

        let inspection = inspect(attachments(), "long.txt", None, &text, full_length).unwrap();
        assert_eq!(inspection.mime(), "text/plain");
    }

    #[test]
    fn a_binary_file_cannot_pass_by_being_text_up_to_its_first_bad_byte() {
        // The hole the boundary trim would open if it trimmed at any invalid
        // byte rather than only at the end: a file that is text for a while and
        // then is not.
        let mut hostile = b"#!/bin/sh\necho hello\n".to_vec();
        hostile.extend_from_slice(&[0x00, 0xff, 0xfe, 0x01, 0x02]);
        hostile.extend(std::iter::repeat_n(0x00, 200));

        let outcome = inspect(
            attachments(),
            "script.txt",
            None,
            &hostile,
            // Claimed to be far larger than the head, so the trim path is the
            // one taken.
            hostile.len() as u64 + 5_000,
        );

        assert!(
            matches!(outcome, Err(Rejection::UnrecognisedType { .. })),
            "binary content was accepted as text: {outcome:?}"
        );
    }

    #[test]
    fn the_declared_content_type_appears_in_a_refusal_and_decides_nothing() {
        let junk = &[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03];

        match inspect(
            attachments(),
            "thing",
            Some("image/png"),
            junk,
            junk.len() as u64,
        ) {
            Err(Rejection::UnrecognisedType { declared }) => {
                assert_eq!(declared.as_deref(), Some("image/png"));
            }
            other => panic!("expected an unrecognised type, got {other:?}"),
        }
    }
}
