//! Which of *your* files ran this statement.
//!
//! # Why this is a stack walk and not the tracing registry
//!
//! Everywhere else the profiler reads what the application already emits
//! (`docs/adr/0004-development-profiler.md`, section 4). This is the one thing
//! that route cannot answer. A statement is logged by sqlx, so the event's own
//! metadata points at `sqlx-core/src/logger.rs` on every row, and the span
//! stack around it is one frame - `http` - because the application opens no
//! spans of its own. `#[instrument]` appears nowhere in the workspace, and
//! adding it to the hundreds of functions that touch a database to make a
//! development tool work is exactly the rot section 4 rejects.
//!
//! So the stack is walked directly. The frames are real: an `.await`ed future
//! is polled from its caller's poll, so the chain from the axum handler down to
//! the query is on the stack when sqlx logs it.
//!
//! # What it costs, and when
//!
//! Split deliberately in two.
//!
//! * **On the request:** a stack walk that records instruction pointers and
//!   nothing else. No symbol lookup, no allocation per frame beyond one `Vec`,
//!   no file system access. Measured at **756 ns** a capture on the machine
//!   this was written on, so a screen running forty statements pays about
//!   thirty microseconds for the whole panel.
//! * **When somebody opens the report:** the addresses are turned into
//!   function names and file positions. Symbolication is the expensive half by
//!   two orders of magnitude, and it is paid once, by a human who is waiting
//!   for a page they asked for.
//!
//! Resolving later is sound because a profile never outlives the process that
//! recorded it - the ring is in memory and dies with the binary the addresses
//! point into.

use std::fmt::Write as _;

use serde::{Serialize, Serializer};

/// How deep a walk goes.
///
/// The interesting frames are the shallow ones - the handler, the service, the
/// query function. Below that is the executor, and past a hundred frames of
/// async machinery there is nothing a developer wants.
const MAX_DEPTH: usize = 96;

/// The path fragment that marks a frame as this workspace's own code.
///
/// Every crate here lives at `<root>/crates/<name>`, so one match separates
/// "your code" from the two hundred frames of tokio, hyper and sqlx around it.
const WORKSPACE: &str = "crates";

/// The captured stack behind one statement, unresolved.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// Instruction pointers. Meaningless outside this process, which is the
    /// only place they are ever read.
    addresses: Vec<usize>,
}

/// One line of a resolved stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Frame {
    /// The function, demangled, with the generic noise trimmed.
    pub function: String,
    /// Workspace-relative, so it reads like a path in the repository rather
    /// than a path on the machine that happened to build it.
    pub file: String,
    pub line: u32,
}

impl Frame {
    /// `phonix-db/src/tenancy.rs:87`, which is a thing you can paste.
    pub fn position(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

impl Caller {
    /// Walk the stack now, resolve nothing.
    pub fn capture() -> Self {
        let mut addresses = Vec::with_capacity(32);

        backtrace::trace(|frame| {
            addresses.push(frame.ip() as usize);

            addresses.len() < MAX_DEPTH
        });

        Self { addresses }
    }

    /// An empty stack, for when capture is switched off.
    pub fn none() -> Self {
        Self {
            addresses: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }

    /// Turn the addresses into workspace frames, outermost call last.
    ///
    /// Only this workspace's own files survive. A stack that is ninety per cent
    /// tokio and sqlx is not a stack anybody reads, and the frames that were
    /// dropped are always the same ones.
    ///
    /// The profiler's own frames go too. They are on every stack, they are
    /// always at the top, and they are never the answer.
    pub fn resolve(&self) -> Vec<Frame> {
        let mut frames = Vec::new();

        for address in &self.addresses {
            backtrace::resolve(*address as *mut std::ffi::c_void, |symbol| {
                let Some(path) = symbol.filename() else {
                    return;
                };
                let Some(file) = workspace_relative(&path.to_string_lossy()) else {
                    return;
                };
                let Some(line) = symbol.lineno() else {
                    return;
                };

                frames.push(Frame {
                    function: symbol
                        .name()
                        .map(|name| trim(&name.to_string()))
                        .unwrap_or_else(|| "<unknown>".to_owned()),
                    file,
                    line,
                });
            });
        }

        frames
    }
}

/// The part of a build path that names a file in this repository.
///
/// `D:\...\phonix\crates\phonix-db\src\tenancy.rs` becomes
/// `phonix-db/src/tenancy.rs`. Anything outside `crates/` - the standard
/// library, a registry dependency, the generated code under `target` - is not
/// this workspace and returns `None`.
///
/// Both forms have to be handled, because the two callers get different ones.
/// A frame resolved out of debug info carries the absolute path the compiler
/// was given; a `tracing` event's `metadata().file()` carries the path
/// *relative to the workspace root*, so it has no leading slash and matching
/// only on `/crates/` silently drops every log line's position.
pub(crate) fn workspace_relative(path: &str) -> Option<String> {
    let normalised = path.replace('\\', "/");
    let prefix = format!("{WORKSPACE}/");
    let tail = match normalised.rsplit_once(&format!("/{prefix}")) {
        Some((_, tail)) => tail,
        None => normalised.strip_prefix(&prefix)?,
    };

    // A registry crate that happens to have `crates/` in its own layout would
    // otherwise be reported as ours.
    if normalised.contains("/registry/") || normalised.contains("/.cargo/") {
        return None;
    }

    // The profiler is on every one of these stacks, at the top, always.
    if tail.starts_with("phonix-profiler/") {
        return None;
    }

    Some(tail.to_owned())
}

/// A symbol name with the parts nobody reads taken out.
///
/// Every async function in this workspace symbolicates with machinery in the
/// middle of its path, and the two toolchains spell it differently:
///
/// | | |
/// |---|---|
/// | Itanium | `phonix_db::tenancy::resolve::{{closure}}::h3f2a1b0c9d8e7f65` |
/// | MSVC | `phonix_db::tenancy::registry::impl$0::resolve::async_fn$0` |
///
/// Both name the same function, and left in, the panel is a column of
/// `impl$0` and `{{closure}}` with the answer buried in it.
fn trim(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());

    for segment in name.split("::") {
        if is_noise(segment) {
            continue;
        }

        if !cleaned.is_empty() {
            let _ = write!(cleaned, "::");
        }

        cleaned.push_str(without_disambiguator(segment));
    }

    cleaned
}

/// A path segment that describes the compiler's bookkeeping, not the code.
fn is_noise(segment: &str) -> bool {
    // The trailing `h1a2b3c...` that Itanium mangling appends.
    let is_hash = segment.len() == 17
        && segment.starts_with('h')
        && segment[1..].chars().all(|character| character.is_ascii_hexdigit());

    if is_hash {
        return true;
    }

    // Anything the compiler wrote in braces is machinery rather than a name
    // somebody typed, and the rule is written that way on purpose: legacy
    // mangling spells it `{{closure}}`, v0 spells the same thing
    // `{closure#0}`, and v0 has a whole family besides - `{async_fn#0}`,
    // `{async_block#0}`, `{constructor#0}`, `{shim:vtable#0}`. Matching the
    // shape rather than the list means the next one costs nothing.
    if segment.starts_with('{') && segment.ends_with('}') {
        return true;
    }

    // MSVC's `impl$0`, `async_fn$0`, `closure$1`, `async_block$0`.
    let Some((kind, index)) = segment.split_once('$') else {
        return false;
    };

    matches!(kind, "impl" | "async_fn" | "closure" | "async_block")
        && !index.is_empty()
        && index.chars().all(|character| character.is_ascii_digit())
}

/// `phonix_db[57778ea5703b32e]` without the bracket.
///
/// v0 mangling writes the crate's disambiguator into the first segment of
/// every path. It identifies the *compilation* - it changes when the compiler
/// flags do - so it is never what somebody reading a stack trace wants, and it
/// is long enough to push the part they do want off the line.
///
/// Only a bracket holding nothing but hex is taken, and only a long one. A
/// path segment can legitimately contain brackets - `<[u8]>::to_vec` - and
/// none of those spell a hash.
fn without_disambiguator(segment: &str) -> &str {
    let Some((name, rest)) = segment.split_once('[') else {
        return segment;
    };
    let Some(hash) = rest.strip_suffix(']') else {
        return segment;
    };

    if hash.len() >= 8 && hash.chars().all(|character| character.is_ascii_hexdigit()) {
        name
    } else {
        segment
    }
}

/// On the wire a caller is its resolved frames, because an address is of no
/// use to anything that is not this process.
impl Serialize for Caller {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.resolve().serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole design rests on this: capturing must be cheap enough to do on
    /// every statement, which means it must not resolve anything.
    #[test]
    fn capturing_records_addresses_and_resolves_nothing() {
        let caller = Caller::capture();

        assert!(!caller.is_empty(), "a test has a stack");
        assert!(caller.addresses.len() <= MAX_DEPTH);
    }

    /// The load-bearing assumption of the whole split: an address captured now
    /// can still be turned into a file and a line later. If the binary carried
    /// no debug info every stack in the report would come back empty, and the
    /// panel would be quietly useless rather than absent.
    ///
    /// This deliberately bypasses [`workspace_relative`], because the frames on
    /// *this* stack belong to the profiler, which the filter removes on
    /// purpose - see the test below.
    #[test]
    fn an_address_captured_now_still_resolves_to_a_file_later() {
        let caller = Caller::capture();
        let mut located = None;

        for address in &caller.addresses {
            backtrace::resolve(*address as *mut std::ffi::c_void, |symbol| {
                if let (Some(file), Some(line)) = (symbol.filename(), symbol.lineno()) {
                    located.get_or_insert_with(|| format!("{}:{line}", file.display()));
                }
            });
        }

        assert!(
            located.is_some(),
            "nothing resolved - the build has no debug info, and every stack \
             the profiler records will be empty"
        );
    }

    /// The profiler is on every stack it captures, at the top, always. Its own
    /// frames are exactly the ones that must not be reported as the caller, and
    /// a stack taken from inside this crate is the purest case of that.
    #[test]
    fn the_profilers_own_frames_are_never_reported() {
        assert!(Caller::capture().resolve().is_empty());
    }

    /// Capturing runs on every statement a request makes, so its cost is the
    /// only reason `profiler.backtraces` exists as a switch.
    ///
    /// The bound is deliberately loose - this is a wall clock on a developer's
    /// machine, not a benchmark. What it catches is the change that matters:
    /// swapping the walk for `Backtrace::force_capture`, or resolving symbols
    /// here instead of at render, either of which costs milliseconds rather
    /// than microseconds and turns a page of forty statements into a stall.
    #[test]
    fn capturing_is_microseconds_not_milliseconds() {
        const ROUNDS: u32 = 2_000;

        let started = std::time::Instant::now();

        for _ in 0..ROUNDS {
            std::hint::black_box(Caller::capture());
        }

        let each = started.elapsed() / ROUNDS;

        assert!(
            each < std::time::Duration::from_micros(500),
            "a capture took {each:?}; something now resolves symbols eagerly"
        );
    }

    #[test]
    fn a_dependency_is_not_this_workspace() {
        assert_eq!(
            workspace_relative(
                "C:/Users/x/.cargo/registry/src/index.crates.io-1/sqlx-core-0.9.0/src/logger.rs"
            ),
            None
        );
        assert_eq!(workspace_relative("/rustc/abc/library/core/src/mod.rs"), None);
    }

    /// The profiler is on every stack it captures. Showing itself as the
    /// caller of every statement would make the panel useless.
    #[test]
    fn the_profiler_is_never_the_caller() {
        assert_eq!(
            workspace_relative("D:/p/phonix/crates/phonix-profiler/src/collect.rs"),
            None
        );
    }

    #[test]
    fn a_workspace_path_is_reported_relative_to_the_repository() {
        assert_eq!(
            workspace_relative("D:\\Ak\\phonix\\crates\\phonix-db\\src\\tenancy.rs").as_deref(),
            Some("phonix-db/src/tenancy.rs")
        );
    }

    /// The two callers are handed different shapes. A resolved stack frame
    /// carries an absolute path; a `tracing` event's metadata carries one
    /// already relative to the workspace root. Matching only the absolute form
    /// leaves every log line saying it has no position.
    #[test]
    fn a_path_already_relative_to_the_workspace_is_recognised() {
        assert_eq!(
            workspace_relative("crates/phonix-server/src/middleware.rs").as_deref(),
            Some("phonix-server/src/middleware.rs")
        );
        assert_eq!(
            workspace_relative("crates\\phonix-server\\src\\middleware.rs").as_deref(),
            Some("phonix-server/src/middleware.rs")
        );
    }

    /// The exclusion has to hold for the relative form too, or the profiler
    /// starts naming itself as the source of its own log lines.
    #[test]
    fn a_relative_path_into_the_profiler_is_still_excluded() {
        assert_eq!(workspace_relative("crates/phonix-profiler/src/store.rs"), None);
    }

    /// Every async function in the workspace symbolicates with these in it.
    /// Left in, the panel is a column of `{{closure}}` and `impl$0`.
    #[test]
    fn closure_markers_and_mangling_hashes_are_dropped() {
        assert_eq!(
            trim("phonix_db::tenancy::resolve::{{closure}}::h3f2a1b0c9d8e7f65"),
            "phonix_db::tenancy::resolve"
        );
        assert_eq!(trim("phonix_web::app::App"), "phonix_web::app::App");
    }

    /// The MSVC spelling of the same thing, taken verbatim from a stack this
    /// profiler captured on Windows.
    #[test]
    fn the_msvc_spelling_of_an_async_method_is_dropped_too() {
        assert_eq!(
            trim("phonix_db::tenancy::catalog::impl$3::find_by_slug::async_fn$0"),
            "phonix_db::tenancy::catalog::find_by_slug"
        );
        assert_eq!(
            trim("phonix_server::middleware::resolve_tenant::async_fn$0"),
            "phonix_server::middleware::resolve_tenant"
        );
    }

    /// The dialect this workspace's own Linux builds actually emit, and the
    /// one a Windows box never sees. Found by running these tests in a
    /// container, where the untrimmed name came out as
    /// `zz_demo[57778ea5703b32e]::tests::query::{closure#0}::{closure#0}`.
    #[test]
    fn the_v0_spelling_of_a_closure_is_dropped_too() {
        assert_eq!(
            trim("phonix_db::tenancy::resolve::{closure#0}::{closure#0}"),
            "phonix_db::tenancy::resolve"
        );
        assert_eq!(
            trim("phonix_db::tenancy::find::{async_fn#0}"),
            "phonix_db::tenancy::find"
        );
        assert_eq!(
            trim("phonix_web::app::render::{shim:vtable#0}"),
            "phonix_web::app::render"
        );
    }

    #[test]
    fn a_crate_disambiguator_is_not_part_of_the_name() {
        assert_eq!(
            trim("phonix_db[57778ea5703b32e]::tenancy::resolve"),
            "phonix_db::tenancy::resolve"
        );
    }

    /// A path segment may hold brackets that are not a hash, and taking those
    /// would rename real code - `<[u8]>::to_vec` is the obvious one.
    #[test]
    fn a_bracket_that_is_not_a_disambiguator_survives() {
        assert_eq!(trim("<[u8]>::to_vec"), "<[u8]>::to_vec");
        assert_eq!(trim("phonix_db[zz]::resolve"), "phonix_db[zz]::resolve");
    }

    /// A `$` in a name that is not the compiler's bookkeeping stays. Dropping
    /// a real segment would silently rename the function being reported.
    #[test]
    fn a_dollar_that_is_not_a_compiler_marker_survives() {
        assert_eq!(trim("phonix_db::impl$x::run"), "phonix_db::impl$x::run");
        assert_eq!(trim("phonix_db::weird$0::run"), "phonix_db::weird$0::run");
    }

    /// A hash-shaped final segment is dropped; a real one that merely looks
    /// hexadecimal is not, because it is the wrong length.
    #[test]
    fn a_short_hex_looking_name_is_not_mistaken_for_a_hash() {
        assert_eq!(trim("phonix_db::hash::hab12"), "phonix_db::hash::hab12");
    }
}
