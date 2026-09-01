//! Capture what built this binary, so the report can say so.
//!
//! # Why a build script at all
//!
//! The compiler's own version is not available to the code it compiles. There
//! is `CARGO_PKG_RUST_VERSION`, but that is the `rust-version` field - the
//! minimum this crate claims to support, not the toolchain in front of you,
//! and printing one while labelling it the other is worse than printing
//! nothing. The same goes for dependency versions: `Cargo.lock` is the record
//! of what was actually compiled, and only a build script can read it at the
//! right moment.
//!
//! # Why it does not fail
//!
//! Nothing here is load-bearing - it is a panel in a development tool. If
//! `rustc` cannot be run or the lockfile cannot be read, the value becomes
//! "unknown" and the build carries on. A profiler that refuses to compile
//! because it could not label itself would be an absurd trade.

use std::process::Command;

/// The dependencies worth naming, in the order the panel lists them.
///
/// Not everything in the lockfile: a wall of three hundred crates answers no
/// question. These are the ones whose version changes how the application
/// behaves, and the ones a bug report needs.
const INTERESTING: &[&str] = &[
    "leptos",
    "axum",
    "sqlx",
    "tokio",
    "tower-http",
    "serde",
    "chrono",
];

fn main() {
    // The lockfile is four levels up from this crate; re-read it when it moves,
    // and re-run when this file changes. Without these the values are baked in
    // at the first build and then quietly go stale.
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../../Cargo.lock");

    println!("cargo::rustc-env=PHONIX_RUSTC={}", rustc_version());
    println!(
        "cargo::rustc-env=PHONIX_TARGET={}",
        env("TARGET", "unknown")
    );
    println!(
        "cargo::rustc-env=PHONIX_PROFILE={}",
        env("PROFILE", "unknown")
    );
    println!("cargo::rustc-env=PHONIX_DEPS={}", dependencies());
}

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_owned())
}

/// `rustc 1.98.0 (abcdef123 2026-05-01)` reduced to `1.98.0`.
fn rustc_version() -> String {
    let rustc = env("RUSTC", "rustc");

    let Ok(output) = Command::new(rustc).arg("-V").output() else {
        return "unknown".to_owned();
    };

    let Ok(text) = String::from_utf8(output.stdout) else {
        return "unknown".to_owned();
    };

    // "rustc 1.98.0 (hash date)" - the middle word is the answer, and the rest
    // is the build hash nobody reads off a panel.
    text.split_whitespace()
        .nth(1)
        .unwrap_or("unknown")
        .to_owned()
}

/// `leptos=0.8.0,axum=0.8.9,...` from the lockfile.
///
/// Parsed by hand rather than with a TOML crate: adding a build dependency to
/// read seven version numbers would cost more compile time than the panel is
/// worth, and the shape of a `[[package]]` block is fixed by cargo.
fn dependencies() -> String {
    let Ok(lock) = std::fs::read_to_string("../../Cargo.lock") else {
        return String::new();
    };

    let mut found: Vec<(usize, String)> = Vec::new();
    let mut name: Option<String> = None;

    for line in lock.lines() {
        let line = line.trim();

        if line == "[[package]]" {
            name = None;
            continue;
        }

        if let Some(value) = field(line, "name") {
            name = Some(value.to_owned());
            continue;
        }

        if let Some(version) = field(line, "version")
            && let Some(package) = name.take()
            && let Some(rank) = INTERESTING.iter().position(|wanted| *wanted == package)
        {
            found.push((rank, format!("{package}={version}")));
        }
    }

    // Listed in the order `INTERESTING` declares, not the lockfile's alphabetical
    // one, so the panel reads top-down as the stack does.
    found.sort_by_key(|(rank, _)| *rank);

    found
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>()
        .join(",")
}

/// `name = "value"` -> `value`.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;

    rest.trim().strip_prefix('"')?.strip_suffix('"')
}
