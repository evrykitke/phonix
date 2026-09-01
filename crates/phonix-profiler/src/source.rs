//! A window onto this workspace's own source, for the flow diagram.
//!
//! # Why this is written defensively
//!
//! The report is unauthenticated. It has always served request detail, SQL and
//! log lines, which is bad enough to justify the two gates in
//! `docs/adr/0004-development-profiler.md` section 8 - but all of that is data
//! the process already chose to hold. Reading files off disk is a different
//! kind of surface: get it wrong and `/_profiler` is an arbitrary file read on
//! a port bound to localhost, or, on the shared box, on `*.evrykit.com`.
//!
//! So the rule is that **the client never supplies a path**. It supplies a
//! lookup key, and the key is only honoured if it is a file that this page
//! load actually recorded a frame in. The set of readable files is therefore
//! derived from evidence the process gathered itself, and no request can widen
//! it.
//!
//! Two independent gates, in this order:
//!
//! 1. The requested path must appear in [`Allowed`], built from the frames of
//!    the profiles in scope.
//! 2. The resolved, canonicalised path must still be inside
//!    `<workspace>/crates`. This one does not trust the first: if a frame ever
//!    carried something strange, or the allowlist were built wrongly, the
//!    filesystem check still refuses.
//!
//! Gate 2 alone would be the conventional answer. It is not enough on its own
//! here, because "anything under `crates/`" still includes every file in the
//! repository, and there is no reason the profiler should hand out a file that
//! had nothing to do with the request being examined.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::profile::Profile;

/// Lines shown either side of the interesting one.
///
/// Enough to see the function it sits in without turning the panel into a file
/// browser, which is what an editor is for.
pub const CONTEXT: u32 = 12;

/// The files a given scope is allowed to read.
///
/// Workspace-relative, exactly as a [`crate::Frame`] spells them:
/// `phonix-db/src/tenancy/catalog.rs`.
#[derive(Debug, Default, Clone)]
pub struct Allowed(BTreeSet<String>);

impl Allowed {
    /// Every file that appears in a frame or a log position of these profiles.
    ///
    /// Resolving the stacks is the expensive half of the profiler and it is
    /// paid here, on a request a human made, for the same reason the report
    /// pays it - see [`crate::caller`].
    pub fn of(profiles: &[Arc<Profile>]) -> Self {
        let mut allowed = BTreeSet::new();

        for profile in profiles {
            for query in &profile.queries {
                for frame in query.caller.resolve() {
                    allowed.insert(frame.file);
                }
            }

            for log in &profile.logs {
                for frame in log.caller.resolve() {
                    allowed.insert(frame.file);
                }

                if let Some(source) = &log.source {
                    allowed.insert(source.clone());
                }
            }
        }

        Self(allowed)
    }

    pub fn contains(&self, file: &str) -> bool {
        self.0.contains(file)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A stretch of a file around one line.
#[derive(Debug, Clone, Serialize)]
pub struct Snippet {
    /// Workspace-relative, echoed back so the panel can title itself without
    /// trusting what it asked for.
    pub file: String,
    /// The line the frame pointed at, 1-based.
    pub line: u32,
    /// The line number of `lines[0]`, 1-based, so the panel can number the
    /// gutter without assuming the window starts at 1.
    pub start: u32,
    pub lines: Vec<String>,
}

/// Read the window around `line` of `file`, if it is allowed and really there.
///
/// `root` is the workspace root - the directory holding `crates/`. `None` for
/// every refusal, deliberately: a caller that could tell "not allowed" from
/// "not found" could map the filesystem one request at a time.
pub fn read(root: &Path, allowed: &Allowed, file: &str, line: u32) -> Option<Snippet> {
    if !allowed.contains(file) {
        return None;
    }

    let path = resolve(root, file)?;
    let text = std::fs::read_to_string(&path).ok()?;

    Some(window(file, &text, line))
}

/// Turn a workspace-relative file into a real path, or refuse.
///
/// The refusals are the point, so they are listed rather than collapsed:
///
/// * An absolute path, or one with a drive letter, is not workspace-relative
///   and never came from a frame.
/// * A `..` component is rejected before the filesystem is touched, so a
///   symlink cannot be used to make canonicalisation agree after the fact.
/// * The canonical path must still be under `<root>/crates`. This is what
///   catches anything the two checks above did not imagine.
fn resolve(root: &Path, file: &str) -> Option<PathBuf> {
    let relative = Path::new(file);

    if relative.is_absolute() || file.contains(':') {
        return None;
    }

    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }

    let crates = root.join("crates").canonicalize().ok()?;
    let candidate = crates.join(relative).canonicalize().ok()?;

    // `starts_with` on `Path` compares whole components, so this cannot be
    // fooled by a sibling directory whose name merely shares a prefix.
    candidate.starts_with(&crates).then_some(candidate)
}

/// The lines around `line`, clamped to the file.
fn window(file: &str, text: &str, line: u32) -> Snippet {
    let all: Vec<&str> = text.lines().collect();
    let total = all.len() as u32;

    // A frame can point one past the end of a file that has been edited since
    // the process started - the watcher restarts on save, but a profile can
    // outlive an edit by seconds. Clamp rather than return nothing.
    let target = line.clamp(1, total.max(1));
    let start = target.saturating_sub(CONTEXT).max(1);
    let end = (target + CONTEXT).min(total);

    let lines = all
        .get((start as usize - 1)..(end as usize))
        .unwrap_or_default()
        .iter()
        .map(|line| (*line).to_owned())
        .collect();

    Snippet {
        file: file.to_owned(),
        line: target,
        start,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(files: &[&str]) -> Allowed {
        Allowed(files.iter().map(|file| (*file).to_owned()).collect())
    }

    /// Tests run with the crate directory as the working directory, so this is
    /// the real workspace root.
    fn root() -> PathBuf {
        PathBuf::from("../..")
    }

    /// The first gate. A path nobody recorded is not readable, however
    /// ordinary it looks.
    #[test]
    fn a_file_that_was_not_recorded_is_refused() {
        let allowed = allowed(&["phonix-profiler/src/source.rs"]);

        assert!(read(&root(), &allowed, "phonix-config/src/lib.rs", 1).is_none());
    }

    /// The second gate, tested on its own: even a path that somehow reached the
    /// allowlist cannot climb out of `crates/`.
    #[test]
    fn a_traversal_is_refused_even_when_it_is_allowed() {
        let escape = "../../../../Windows/System32/drivers/etc/hosts";
        let allowed = allowed(&[escape]);

        assert!(
            read(&root(), &allowed, escape, 1).is_none(),
            "the allowlist must not be the only thing standing between the \
             report and the filesystem"
        );
    }

    #[test]
    fn an_absolute_path_is_refused() {
        assert!(resolve(&root(), "/etc/passwd").is_none());
        assert!(resolve(&root(), "C:/Windows/win.ini").is_none());
    }

    /// The happy path, against a file that is certainly there.
    #[test]
    fn an_allowed_file_reads_a_window_around_the_line() {
        let file = "phonix-profiler/src/source.rs";
        let snippet =
            read(&root(), &allowed(&[file]), file, 1).expect("this crate's own source is readable");

        assert_eq!(snippet.file, file);
        assert_eq!(snippet.start, 1, "a window at line 1 cannot start earlier");
        assert!(
            snippet
                .lines
                .first()
                .is_some_and(|first| first.contains("//!")),
            "line 1 of this file is its module doc"
        );
        assert!(snippet.lines.len() <= (CONTEXT as usize * 2) + 1);
    }

    #[test]
    fn a_window_in_the_middle_is_centred_on_the_line() {
        let text = (1..=100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");

        let snippet = window("x.rs", &text, 50);

        assert_eq!(snippet.start, 50 - CONTEXT);
        assert_eq!(snippet.line, 50);
        assert_eq!(snippet.lines.len(), (CONTEXT as usize * 2) + 1);
        assert_eq!(snippet.lines[CONTEXT as usize], "line 50");
    }

    /// A profile can outlive an edit by a few seconds, so a line past the end
    /// of the file is an ordinary event rather than an error.
    #[test]
    fn a_line_past_the_end_is_clamped_rather_than_refused() {
        let snippet = window("x.rs", "one\ntwo\nthree", 900);

        assert_eq!(snippet.line, 3);
        assert!(!snippet.lines.is_empty());
    }

    #[test]
    fn an_empty_file_does_not_panic() {
        let snippet = window("x.rs", "", 1);

        assert!(snippet.lines.is_empty());
        assert_eq!(snippet.start, 1);
    }
}
