//! Resident set size, where the platform will say.
//!
//! This is the process, not the request. Under any concurrency at all it
//! cannot be attributed to the row it is printed on, and the report says so
//! next to the number rather than here. See
//! `docs/adr/0004-development-profiler.md` section 3 for why the honest
//! per-request figure is not being built.

/// Resident bytes, or `None` where this platform is not one we can ask
/// cheaply.
///
/// "Cheaply" is the whole constraint: this runs on every profiled request, so
/// it may not spawn anything, allocate much, or take a lock. On Linux that is
/// one small read of a pseudo-file. On Windows and macOS the equivalent is a
/// system call this crate would need a dependency to make, and a development
/// profiler is not worth a platform crate for a number section 3 already
/// describes as a gauge.
pub fn current() -> Option<u64> {
    read()
}

#[cfg(target_os = "linux")]
fn read() -> Option<u64> {
    // Field two of /proc/self/statm is the resident set, in pages.
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;

    Some(pages.saturating_mul(page_size()))
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    // Not read from the system: `sysconf` needs libc, and every Linux this
    // runs on uses 4 KiB. A wrong constant here would scale the gauge, which
    // is a gauge either way - it would not make a right number wrong.
    4096
}

#[cfg(not(target_os = "linux"))]
fn read() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asking must never panic, on any platform. That is the only property
    /// worth asserting: on Linux the number is real, and everywhere else the
    /// correct answer is `None`.
    #[test]
    fn asking_is_always_safe() {
        let bytes = current();

        if cfg!(target_os = "linux") {
            assert!(bytes.is_some_and(|value| value > 0), "linux has an answer");
        } else {
            assert_eq!(bytes, None);
        }
    }
}
