//! What built this binary, captured by `build.rs`.
//!
//! Everything here is a compile-time constant. Nothing is looked up at runtime,
//! because none of it can change while the process is alive - and a panel that
//! shells out to `rustc` on every page view would be a strange thing to find in
//! a profiler.
//!
//! A missing value is the empty string rather than a panic: see `build.rs` for
//! why nothing here is allowed to fail a build.

/// The compiler that built this, as `1.98.0`.
pub const RUSTC: &str = env!("PHONIX_RUSTC");

/// The target triple.
pub const TARGET: &str = env!("PHONIX_TARGET");

/// `debug` or `release`, which is the thing worth knowing when a stack resolves
/// to nothing - see [`crate::caller`].
pub const PROFILE: &str = env!("PHONIX_PROFILE");

/// `leptos=0.8.0,axum=0.8.9,...`, from the lockfile at build time.
const DEPS: &str = env!("PHONIX_DEPS");

/// The dependency versions, split back into pairs.
pub fn dependencies() -> impl Iterator<Item = (&'static str, &'static str)> {
    DEPS.split(',')
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| entry.split_once('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point is to report the real toolchain rather than the
    /// `rust-version` field, so an empty value means the build script failed
    /// quietly and the panel is lying by omission.
    #[test]
    fn the_toolchain_was_captured() {
        assert!(!RUSTC.is_empty());
        assert_ne!(RUSTC, "unknown", "rustc -V could not be read at build time");
        assert!(
            RUSTC.starts_with(|first: char| first.is_ascii_digit()),
            "expected a version number, got {RUSTC:?}"
        );
    }

    #[test]
    fn the_profile_is_one_of_the_two_that_matter() {
        assert!(matches!(PROFILE, "debug" | "release"), "{PROFILE:?}");
    }

    /// The versions come from `Cargo.lock`, so this also proves the build
    /// script found and parsed it.
    #[test]
    fn the_interesting_dependencies_were_found() {
        let found: Vec<&str> = dependencies().map(|(name, _)| name).collect();

        assert!(found.contains(&"leptos"), "found {found:?}");
        assert!(found.contains(&"axum"), "found {found:?}");

        for (name, version) in dependencies() {
            assert!(!name.is_empty());
            assert!(
                version.starts_with(|first: char| first.is_ascii_digit()),
                "{name} has no version: {version:?}"
            );
        }
    }
}
