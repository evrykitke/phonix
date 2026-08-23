//! A log file that stops growing, and old ones that go away.
//!
//! # Why this is not `tracing_appender::rolling`
//!
//! That appender rolls on **time** only - minutely, hourly, daily, never - and
//! its retention is a **count** of files, not an age. Neither is what a disk
//! filling up needs:
//!
//! * A daily file has no size at all until the day is over. One afternoon at
//!   `debug` produces a gigabyte, and nothing rotates it until midnight.
//! * `max_files = 14` means fourteen files. With daily rotation that is
//!   fourteen days; the moment size rotation exists it could be fourteen
//!   minutes, and the setting silently stops meaning what it says.
//!
//! So this appender rolls on **whichever comes first**, and prunes on **age**:
//!
//! ```text
//!   var/development.2026-08-21.001.log   <- written to
//!      development.2026-08-21.002.log
//!      development.2026-08-20.001.log
//!      development.2026-08-18.001.log    <- older than retention_days: deleted
//! ```
//!
//! A file is rotated when the next line would push it past `max_file_size_mb`,
//! or when the date stamp changes. The sequence number restarts each period, so
//! a name says both when a file was written and which part of that period it
//! is.
//!
//! # Age, judged by the filesystem rather than by the name
//!
//! Pruning reads each file's modification time instead of parsing its name.
//! That is deliberate: it means files written by an *older* naming scheme are
//! still cleaned up rather than accumulating for ever because they do not match
//! the pattern this version writes. The name is for people; the mtime is what
//! the sweep trusts.
//!
//! # Single-threaded by construction
//!
//! There is no lock in here. The appender is handed to
//! `tracing_appender::non_blocking`, which moves it onto one worker thread and
//! is the only thing that ever writes to it - so `&mut self` is the honest
//! signature, and a mutex would be a lock nobody contends for.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use phonix_config::Rotation;

/// How a file is named, and when a new one is started.
#[derive(Debug, Clone)]
pub struct RollingConfig {
    pub directory: PathBuf,
    /// `development`, after `{env}` has been expanded.
    pub prefix: String,
    /// `log`, without the dot.
    pub suffix: String,
    /// The period a date stamp represents. `Never` means no stamp at all.
    pub rotation: Rotation,
    /// Roll when the next line would exceed this. `0` disables the size cap.
    pub max_bytes: u64,
    /// Delete files not modified within this many days. `0` keeps everything.
    pub retention_days: u64,
    /// Keep at most this many files, newest first, after the age sweep. `0` is
    /// unlimited.
    ///
    /// A backstop under `retention_days` rather than a replacement for it: a
    /// process logging hard enough to produce two hundred files inside the
    /// retention window is one this should still bound.
    pub max_files: usize,
}

/// A log file that rolls on size and on time, and prunes what it leaves behind.
pub struct RollingFile {
    config: RollingConfig,
    /// The open file, and how much has been written to it.
    file: File,
    written: u64,
    /// The date stamp currently in use, so a change is a rotation.
    period: String,
    sequence: u32,
}

impl RollingFile {
    /// Open (or reopen) the current log file.
    ///
    /// Appends to the newest file for this period when there is room in it, so
    /// restarting the process does not abandon a file that is one line old -
    /// and does not overwrite it either.
    pub fn open(config: RollingConfig) -> io::Result<Self> {
        fs::create_dir_all(&config.directory)?;

        let period = period_stamp(config.rotation, SystemTime::now());
        let sequence = latest_sequence(&config, &period).unwrap_or(1);

        let path = file_path(&config, &period, sequence);
        let written = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);

        // The newest file for this period is already full, so start the next
        // one rather than pushing it past the cap on the first line.
        let (sequence, written) = if config.max_bytes > 0 && written >= config.max_bytes {
            (sequence.saturating_add(1), 0)
        } else {
            (sequence, written)
        };

        let file = open_appending(&file_path(&config, &period, sequence))?;

        let appender = Self {
            config,
            file,
            written,
            period,
            sequence,
        };

        // At startup as well as at every roll: a process that was down over a
        // weekend should not need a rotation before it cleans up.
        appender.prune();

        Ok(appender)
    }

    /// The file currently being written to.
    pub fn current_path(&self) -> PathBuf {
        file_path(&self.config, &self.period, self.sequence)
    }

    /// Whether `incoming` bytes would need a new file.
    fn needs_roll(&self, incoming: usize, now: SystemTime) -> bool {
        if period_stamp(self.config.rotation, now) != self.period {
            return true;
        }

        if self.config.max_bytes == 0 {
            return false;
        }

        // Checked *before* writing, so a file never exceeds the cap - rather
        // than after, which would let every file sit one line over it.
        self.written.saturating_add(incoming as u64) > self.config.max_bytes
    }

    /// Start a new file.
    fn roll(&mut self, now: SystemTime) -> io::Result<()> {
        // Flushed before the handle is replaced. Dropping a `File` does not
        // report a failed flush, so buffered lines would vanish silently.
        let _ = self.file.flush();

        let period = period_stamp(self.config.rotation, now);

        // A new period restarts the numbering, so `.001` always means "the
        // first file of that day" rather than "the first since the process
        // started".
        self.sequence = if period == self.period {
            self.sequence.saturating_add(1)
        } else {
            1
        };
        self.period = period;

        self.file = open_appending(&self.current_path())?;
        self.written = 0;

        self.prune();

        Ok(())
    }

    /// Delete what is too old, and then what is simply too much.
    ///
    /// Failures are ignored on purpose. This runs on the logging path, and a
    /// file that cannot be deleted - held open by a log shipper, or on a
    /// read-only mount - must not stop the line that triggered the sweep from
    /// being written. There is also nowhere useful to report it: the reporting
    /// mechanism is the thing that is failing.
    fn prune(&self) {
        let current = self.current_path();
        let mut candidates = self.existing_files();

        // Newest first, so the count cap below keeps the useful end.
        candidates.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));

        // The `> 0` is the whole of the sentinel, and leaving it out is not a
        // missing feature but a catastrophe: zero days would compute a cutoff
        // of *now*, every file would be older than that, and "keep everything"
        // would empty the log directory on the first write.
        let cutoff = (self.config.retention_days > 0)
            .then(|| self.config.retention_days.checked_mul(24 * 60 * 60))
            .flatten()
            .and_then(|seconds| {
                SystemTime::now().checked_sub(std::time::Duration::from_secs(seconds))
            });

        let mut kept = 0usize;

        for (path, modified) in candidates {
            if path == current {
                // Never the file being written to, whatever its age. On
                // Windows this would fail anyway; on Linux it would succeed and
                // leave the process writing to an unlinked inode, which is the
                // worse outcome because nothing looks wrong until the logs are
                // wanted.
                kept = kept.saturating_add(1);
                continue;
            }

            let too_old = cutoff.is_some_and(|cutoff| modified < cutoff);
            let too_many = self.config.max_files > 0 && kept >= self.config.max_files;

            if too_old || too_many {
                let _ = fs::remove_file(&path);
            } else {
                kept = kept.saturating_add(1);
            }
        }
    }

    /// Every log file this appender is responsible for, with its mtime.
    ///
    /// Matched on the prefix and the suffix rather than on the full pattern, so
    /// files left by an earlier naming scheme are swept too. Anything else in
    /// the directory is left strictly alone: the log directory may be somebody
    /// else's as well, and a sweep that deleted by "everything here" would one
    /// day delete something that mattered.
    fn existing_files(&self) -> Vec<(PathBuf, SystemTime)> {
        let Ok(entries) = fs::read_dir(&self.config.directory) else {
            return Vec::new();
        };

        let prefix = format!("{}.", self.config.prefix);
        let suffix = format!(".{}", self.config.suffix);

        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?;

                if !name.starts_with(&prefix) || !name.ends_with(&suffix) {
                    return None;
                }

                let metadata = entry.metadata().ok()?;
                if !metadata.is_file() {
                    return None;
                }

                Some((path, metadata.modified().ok()?))
            })
            .collect()
    }
}

impl Write for RollingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let now = SystemTime::now();

        if self.needs_roll(buf.len(), now) {
            self.roll(now)?;
        }

        let written = self.file.write(buf)?;
        self.written = self.written.saturating_add(written as u64);

        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// `<directory>/<prefix>.<period>.<seq>.<suffix>`.
///
/// The period is omitted when rotation is `never`, which leaves
/// `development.001.log` - still numbered, because the size cap applies
/// whatever the time rotation is set to.
fn file_path(config: &RollingConfig, period: &str, sequence: u32) -> PathBuf {
    let name = if period.is_empty() {
        format!("{}.{sequence:03}.{}", config.prefix, config.suffix)
    } else {
        format!("{}.{period}.{sequence:03}.{}", config.prefix, config.suffix)
    };

    config.directory.join(name)
}

/// The highest sequence number already on disk for this period.
///
/// So a restart continues the series instead of reopening `.001` and appending
/// today's logs underneath last hour's.
fn latest_sequence(config: &RollingConfig, period: &str) -> Option<u32> {
    let entries = fs::read_dir(&config.directory).ok()?;

    let stem = if period.is_empty() {
        format!("{}.", config.prefix)
    } else {
        format!("{}.{period}.", config.prefix)
    };
    let suffix = format!(".{}", config.suffix);

    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;

            let rest = name.strip_prefix(&stem)?.strip_suffix(&suffix)?;
            // Only a bare number. `development.2026-08-21.001.log` matches;
            // `development.2026-08-21.log` from the previous scheme does not,
            // and is left to the age sweep rather than confusing the count.
            rest.parse::<u32>().ok()
        })
        .max()
}

/// The date stamp for a moment, at the resolution the rotation asks for.
///
/// Formatted from a `SystemTime` rather than taken from a clock type so that
/// the tests can hand it a fixed instant.
fn period_stamp(rotation: Rotation, now: SystemTime) -> String {
    use chrono::{DateTime, Utc};

    if matches!(rotation, Rotation::Never) {
        return String::new();
    }

    let now: DateTime<Utc> = now.into();

    match rotation {
        Rotation::Minutely => now.format("%Y-%m-%d-%H-%M").to_string(),
        Rotation::Hourly => now.format("%Y-%m-%d-%H").to_string(),
        Rotation::Daily => now.format("%Y-%m-%d").to_string(),
        Rotation::Never => String::new(),
    }
}

fn open_appending(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A directory of this test's own.
    ///
    /// A counter rather than a timestamp: these tests run in parallel and
    /// several start inside the same millisecond, and a `Debug` timestamp
    /// contains a colon, which is not a character a Windows path may hold.
    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);

        let dir = std::env::temp_dir()
            .join("phonix-rolling-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));

        fs::create_dir_all(&dir).expect("a temp directory");
        dir
    }

    fn config(directory: PathBuf, max_bytes: u64) -> RollingConfig {
        RollingConfig {
            directory,
            prefix: "development".into(),
            suffix: "log".into(),
            rotation: Rotation::Daily,
            max_bytes,
            retention_days: 3,
            max_files: 0,
        }
    }

    fn log_files(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("the directory")
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_never_grows_past_the_cap() {
        let dir = temp_dir();
        let mut appender = RollingFile::open(config(dir.clone(), 100)).unwrap();

        // Ten lines of forty bytes into a hundred-byte cap.
        for _ in 0..10 {
            appender.write_all(&[b'x'; 40]).unwrap();
        }
        appender.flush().unwrap();

        for name in log_files(&dir) {
            let size = fs::metadata(dir.join(&name)).unwrap().len();
            assert!(
                size <= 100,
                "{name} is {size} bytes, past the hundred-byte cap"
            );
        }
    }

    #[test]
    fn rolling_produces_a_numbered_series() {
        let dir = temp_dir();
        let mut appender = RollingFile::open(config(dir.clone(), 50)).unwrap();

        for _ in 0..5 {
            appender.write_all(&[b'y'; 30]).unwrap();
        }
        appender.flush().unwrap();

        let names = log_files(&dir);
        assert!(names.len() >= 3, "expected several files, got {names:?}");
        assert!(
            names.iter().any(|name| name.ends_with(".001.log")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.ends_with(".002.log")),
            "{names:?}"
        );
    }

    #[test]
    fn a_line_longer_than_the_cap_is_still_written() {
        // The alternative is a log line that is silently dropped, which is the
        // one thing a logger must never do. It gets a file to itself.
        let dir = temp_dir();
        let mut appender = RollingFile::open(config(dir.clone(), 50)).unwrap();

        appender.write_all(&[b'z'; 500]).unwrap();
        appender.flush().unwrap();

        let total: u64 = log_files(&dir)
            .iter()
            .map(|name| fs::metadata(dir.join(name)).unwrap().len())
            .sum();
        assert_eq!(total, 500);
    }

    #[test]
    fn restarting_continues_the_series_rather_than_overwriting_it() {
        let dir = temp_dir();

        {
            let mut appender = RollingFile::open(config(dir.clone(), 100)).unwrap();
            appender.write_all(&[b'a'; 80]).unwrap();
            appender.flush().unwrap();
        }

        // A second process, or the same one after a restart.
        {
            let mut appender = RollingFile::open(config(dir.clone(), 100)).unwrap();
            appender.write_all(&[b'b'; 10]).unwrap();
            appender.flush().unwrap();
        }

        let names = log_files(&dir);
        let first = fs::read(dir.join(names.first().unwrap())).unwrap();

        // The first file kept its 80 bytes and gained the 10 that fit.
        assert_eq!(first.len(), 90);
        assert!(first.starts_with(&[b'a'; 80]));
    }

    #[test]
    fn a_full_file_is_not_reopened_on_restart() {
        let dir = temp_dir();

        {
            let mut appender = RollingFile::open(config(dir.clone(), 100)).unwrap();
            appender.write_all(&[b'a'; 100]).unwrap();
            appender.flush().unwrap();
        }
        {
            let mut appender = RollingFile::open(config(dir.clone(), 100)).unwrap();
            appender.write_all(&[b'b'; 10]).unwrap();
            appender.flush().unwrap();
        }

        let names = log_files(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(fs::metadata(dir.join(&names[0])).unwrap().len(), 100);
        assert_eq!(fs::metadata(dir.join(&names[1])).unwrap().len(), 10);
    }

    #[test]
    fn files_older_than_the_retention_window_are_removed() {
        let dir = temp_dir();

        // Four days old, and named the way an older build would have named it -
        // which is the case the mtime test exists to cover.
        let stale = dir.join("development.2026-08-17.log");
        fs::write(&stale, b"old").unwrap();
        let four_days_ago = SystemTime::now() - Duration::from_secs(4 * 24 * 60 * 60);
        set_modified(&stale, four_days_ago);

        // One day old: inside the window, and must survive.
        let recent = dir.join("development.2026-08-20.001.log");
        fs::write(&recent, b"recent").unwrap();
        set_modified(
            &recent,
            SystemTime::now() - Duration::from_secs(24 * 60 * 60),
        );

        let _appender = RollingFile::open(config(dir.clone(), 0)).unwrap();

        assert!(!stale.exists(), "a four-day-old file survived the sweep");
        assert!(recent.exists(), "a one-day-old file was swept");
    }

    #[test]
    fn the_file_being_written_is_never_swept() {
        let dir = temp_dir();
        let mut settings = config(dir.clone(), 0);
        // Retention of zero days would otherwise make everything - including
        // the open file - older than the cutoff.
        settings.retention_days = 0;
        settings.max_files = 1;

        let appender = RollingFile::open(settings).unwrap();
        let current = appender.current_path();

        // Even with room for exactly one file, the one in use is that one.
        assert!(current.exists());
        drop(appender);
        assert!(current.exists());
    }

    #[test]
    fn nothing_outside_the_prefix_is_touched() {
        let dir = temp_dir();

        // A log directory is often shared. A sweep that deleted by "everything
        // in here" would one day delete something that mattered.
        let stranger = dir.join("nginx.access.log");
        fs::write(&stranger, b"not ours").unwrap();
        set_modified(
            &stranger,
            SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60),
        );

        let other_suffix = dir.join("development.2026-08-01.txt");
        fs::write(&other_suffix, b"not ours either").unwrap();
        set_modified(
            &other_suffix,
            SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60),
        );

        let _appender = RollingFile::open(config(dir.clone(), 0)).unwrap();

        assert!(stranger.exists(), "another program's log was deleted");
        assert!(
            other_suffix.exists(),
            "a file with a different suffix was deleted"
        );
    }

    #[test]
    fn zero_means_unlimited_for_both_caps() {
        let dir = temp_dir();
        let mut settings = config(dir.clone(), 0);
        settings.retention_days = 0;
        settings.max_files = 0;

        let ancient = dir.join("development.2020-01-01.001.log");
        fs::write(&ancient, b"very old").unwrap();
        set_modified(
            &ancient,
            SystemTime::now() - Duration::from_secs(2000 * 24 * 60 * 60),
        );

        let mut appender = RollingFile::open(settings).unwrap();
        appender.write_all(&[b'q'; 10_000]).unwrap();
        appender.flush().unwrap();

        // No size cap: one file, however much goes into it.
        assert!(ancient.exists(), "retention_days = 0 deleted a file anyway");
        assert!(fs::metadata(appender.current_path()).unwrap().len() >= 10_000);
    }

    #[test]
    fn the_period_stamp_matches_the_rotation_asked_for() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_755_000_000);

        assert_eq!(period_stamp(Rotation::Never, at), "");
        assert_eq!(period_stamp(Rotation::Daily, at), "2025-08-12");
        assert_eq!(period_stamp(Rotation::Hourly, at), "2025-08-12-12");
        assert_eq!(period_stamp(Rotation::Minutely, at), "2025-08-12-12-00");
    }

    #[test]
    fn a_changed_period_starts_the_numbering_again() {
        let dir = temp_dir();
        let mut appender = RollingFile::open(config(dir.clone(), 10)).unwrap();

        appender.write_all(b"first").unwrap();
        appender.write_all(b"second").unwrap();
        assert_eq!(appender.sequence, 2);

        // A day has passed.
        appender.period = "1999-01-01".to_owned();
        appender.roll(SystemTime::now()).unwrap();

        // `.001` has to mean "the first file of that day" rather than "the
        // first since the process started".
        assert_eq!(appender.sequence, 1);
    }

    /// Set a file's modification time.
    ///
    /// `set_times` rather than a sleep: the alternative is a test that takes
    /// four days.
    fn set_modified(path: &Path, at: SystemTime) {
        let file = OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(at))
            .unwrap();
    }
}
