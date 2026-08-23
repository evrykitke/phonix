//! The name a caller chose, made safe to keep.
//!
//! A stored file is never written under the name it arrived with - the storage
//! layer generates its own, and nothing on disk is ever derived from caller
//! input. So why sanitise at all?
//!
//! Because the original name is still *kept*: it is what the files list shows,
//! and it is what a download offers the browser to save as. Both of those are
//! places a hostile name does damage without ever touching a filesystem:
//!
//! * `../../../../etc/passwd` in a `Content-Disposition` header is a name the
//!   browser may write relative to the download folder.
//! * A newline or a quote in that header splits it, which is header injection.
//! * A right-to-left override character makes `invoice\u{202E}gnp.exe` render
//!   as `invoiceexe.png` in a list, which is the oldest disguise there is.
//! * `CON`, `NUL` and `LPT1` are device names on Windows, and a save dialog
//!   offered one of them does something other than save a file.
//!
//! So the name is cleaned once, on the way in, and what is stored is already
//! safe for every later use. Cleaning at each point of use would mean each
//! point of use remembering to.

/// Longest name kept. Filesystems stop at 255 bytes and a list has to render
/// it; neither is served by keeping more.
const MAX_NAME_LEN: usize = 200;

/// Longest extension taken seriously.
///
/// Anything past this is not an extension, it is the rest of a name that
/// happens to contain a dot - and treating it as one would let a caller push
/// the real name out of the length budget.
const MAX_EXTENSION_LEN: usize = 16;

/// Windows device names. Reserved with *any* extension, so `NUL.txt` counts.
const RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// What is left of a caller's file name once it cannot do any harm.
///
/// Never fails and never returns an empty string: a name that cleans away to
/// nothing becomes `"file"`, because the alternative is a rejection over
/// something the caller cannot see or fix, for a value that decides nothing.
///
/// The steps, in order:
///
/// 1. Everything up to the last `/` or `\` is dropped. Some browsers send a
///    full path, and every traversal attempt lives in this part.
/// 2. Control characters, quotes and the bidirectional overrides go.
/// 3. Runs of whitespace collapse to one space, and the ends are trimmed.
/// 4. Leading dots go, so nothing becomes a hidden file or `..`.
/// 5. A reserved Windows device name is prefixed rather than refused.
/// 6. The stem is truncated so the whole thing fits, keeping the extension -
///    which is the part that decides how a saved copy opens.
pub fn sanitize_file_name(raw: &str) -> String {
    // rsplit on both separators: a Windows client sends backslashes, and a
    // server that only knows about `/` keeps `..\..\evil` intact.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);

    let cleaned: String = base
        .chars()
        .filter(|ch| {
            !ch.is_control()
                // Would split or escape a Content-Disposition header.
                && !matches!(ch, '"' | '\\' | '/' | '\0')
                // The bidirectional overrides, which reverse how the rest of
                // the name renders without changing what it is.
                && !matches!(*ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
        })
        .collect();

    let collapsed = collapse_whitespace(&cleaned);
    let trimmed = collapsed.trim_start_matches('.').trim();

    if trimmed.is_empty() {
        return "file".to_owned();
    }

    let (stem, extension) = split_extension(trimmed);

    // A device name is renamed rather than refused: the caller did nothing
    // wrong, and `file-NUL.txt` is a perfectly good thing to save.
    let stem = if RESERVED_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        format!("file-{stem}")
    } else {
        stem.to_owned()
    };

    truncate_keeping_extension(&stem, extension)
}

/// The extension of a name, lowercased, without the dot.
///
/// `None` when there is none, when it is absurdly long, or when it is not plain
/// alphanumeric - because the only thing an extension is ever used for here is
/// to break a tie between formats that share a container, and a value that is
/// not shaped like an extension cannot do that.
pub fn extension_of(name: &str) -> Option<String> {
    let (_, extension) = split_extension(name);
    let extension = extension?;

    let usable = !extension.is_empty()
        && extension.len() <= MAX_EXTENSION_LEN
        && extension.chars().all(|ch| ch.is_ascii_alphanumeric());

    usable.then(|| extension.to_ascii_lowercase())
}

/// Split a name into its stem and its extension.
///
/// A leading dot is not a separator - `.gitignore` is a name, not an extension -
/// which is why the split is on the *last* dot at a position past the first
/// character, rather than on the first dot anywhere.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        // `get` rather than slicing: a multi-byte character straddling the
        // index would panic, and this crate runs in the browser.
        Some(index) if index > 0 => {
            let stem = name.get(..index).unwrap_or(name);
            let extension = name.get(index.saturating_add(1)..).unwrap_or("");
            if extension.is_empty() || extension.len() > MAX_EXTENSION_LEN {
                (name, None)
            } else {
                (stem, Some(extension))
            }
        }
        _ => (name, None),
    }
}

fn collapse_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_space = false;

    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }

    out
}

/// Fit the name into the budget without losing the extension.
///
/// Truncation is on a character boundary, not a byte one: cutting a name
/// mid-codepoint produces a string that is not valid UTF-8 to write anywhere.
fn truncate_keeping_extension(stem: &str, extension: Option<&str>) -> String {
    let suffix_len = extension.map_or(0, |ext| ext.len().saturating_add(1));
    let budget = MAX_NAME_LEN.saturating_sub(suffix_len);

    let mut kept = String::new();
    for ch in stem.chars() {
        if kept.len().saturating_add(ch.len_utf8()) > budget {
            break;
        }
        kept.push(ch);
    }

    let kept = kept.trim_end();
    let kept = if kept.is_empty() { "file" } else { kept };

    match extension {
        Some(ext) => format!("{kept}.{ext}"),
        None => kept.to_owned(),
    }
}

/// A byte count as a person reads it.
///
/// Powers of 1024 with the units people actually recognise, one decimal place
/// above a kilobyte and none below - `847 B`, `12.4 KB`, `3.1 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64 / 1024.0;
    let mut unit = "KB";

    for next in UNITS {
        unit = next;
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
    }

    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_reduced_to_its_last_segment() {
        assert_eq!(sanitize_file_name("../../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name(r"C:\Users\ada\report.pdf"), "report.pdf");
        // The mixed form, which a check for only one separator would miss.
        assert_eq!(sanitize_file_name(r"a/b\..\..\secret.txt"), "secret.txt");
    }

    #[test]
    fn header_splitting_characters_do_not_survive() {
        let hostile = "in\"voice\r\nContent-Length: 0\r\n\r\n.pdf";
        let safe = sanitize_file_name(hostile);

        assert!(!safe.contains('"'));
        assert!(!safe.contains('\r'));
        assert!(!safe.contains('\n'));
    }

    #[test]
    fn a_disguised_extension_is_undisguised() {
        // U+202E renders the rest of the name backwards, so this shows as
        // "invoiceexe.pdf" in any list that does not strip it.
        let disguised = "invoice\u{202e}fdp.exe";
        let safe = sanitize_file_name(disguised);

        assert_eq!(safe, "invoicefdp.exe");
        assert!(!safe.contains('\u{202e}'));
    }

    #[test]
    fn nothing_becomes_hidden_or_relative() {
        assert_eq!(sanitize_file_name("...."), "file");
        assert_eq!(sanitize_file_name(".."), "file");
        assert_eq!(sanitize_file_name(".env"), "env");
        assert_eq!(sanitize_file_name("   "), "file");
        assert_eq!(sanitize_file_name(""), "file");
    }

    #[test]
    fn windows_device_names_are_renamed_not_refused() {
        assert_eq!(sanitize_file_name("NUL.txt"), "file-NUL.txt");
        assert_eq!(sanitize_file_name("com4"), "file-com4");
        // Not reserved: the check is on the whole stem, not a prefix of it.
        assert_eq!(sanitize_file_name("console.log"), "console.log");
    }

    #[test]
    fn a_long_name_keeps_its_extension() {
        let long = format!("{}.pdf", "a".repeat(500));
        let safe = sanitize_file_name(&long);

        assert!(safe.len() <= MAX_NAME_LEN);
        assert!(
            safe.ends_with(".pdf"),
            "the extension was truncated away: {safe}"
        );
    }

    #[test]
    fn truncation_lands_on_a_character_boundary() {
        // Every character is three bytes, so a byte-wise cut would land inside
        // one and produce something that is not a string.
        let long = format!("{}.txt", "日".repeat(300));
        let safe = sanitize_file_name(&long);

        assert!(safe.len() <= MAX_NAME_LEN);
        assert!(safe.ends_with(".txt"));
        assert!(std::str::from_utf8(safe.as_bytes()).is_ok());
    }

    #[test]
    fn whitespace_is_collapsed_rather_than_kept() {
        assert_eq!(sanitize_file_name("  my   report\t.pdf  "), "my report.pdf");
    }

    #[test]
    fn an_extension_is_only_read_when_it_is_shaped_like_one() {
        assert_eq!(extension_of("photo.PNG").as_deref(), Some("png"));
        assert_eq!(extension_of("archive.tar.gz").as_deref(), Some("gz"));

        assert_eq!(extension_of("no-extension"), None);
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of("trailing."), None);
        // Not an extension, just a sentence with a dot in it.
        assert_eq!(extension_of("report.final version"), None);
        assert_eq!(extension_of(&format!("x.{}", "a".repeat(40))), None);
    }

    #[test]
    fn sizes_read_the_way_people_write_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(847), "847 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(12_700), "12.4 KB");
        assert_eq!(human_size(3_250_586), "3.1 MB");
        assert_eq!(human_size(5_368_709_120), "5.0 GB");
    }
}
