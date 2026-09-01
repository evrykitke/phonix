//! Whether a page load or a request passes a short list of checks.
//!
//! # What a green tick is allowed to mean
//!
//! It means **these named checks passed**, and nothing more. It is not a
//! verdict on the screen, the query plan or the design. The checks are
//! therefore rendered beside the tick rather than behind it: a badge whose
//! reasoning is hidden is one people either over-trust or stop reading, and
//! both are worse than no badge.
//!
//! # The state that matters most
//!
//! [`Grade::Unknown`]. A page load that recorded nothing passes every check
//! below trivially - no failed response, no error logged, no repeated
//! statement, no time spent - and a green tick there would be the most
//! misleading thing on the page. A request that did no work the profiler can
//! see is *unjudged*, not healthy, and it says so.
//!
//! # Why these four
//!
//! Each is something the profiler already knows for certain, and each has a
//! clear failure a developer can act on. Nothing here is inferred, scored or
//! weighted: a check passes, warns, or fails, and the verdict is the worst of
//! them. A number that blends four signals into one score would hide which one
//! moved.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::page::PageSummary;
use crate::profile::{LogLine, Profile};

/// Over this, the time spent is a failure rather than a note.
///
/// The same number the rest of the report colours timings against - see
/// `report::SLOW`. Kept as its own constant because this one is about a whole
/// page load, and the two may reasonably part company later.
const SLOW: Duration = Duration::from_millis(500);

/// A statement shape repeated this many times is an N+1 rather than a coincidence.
///
/// Twice is ordinary - a screen can legitimately read the same row for two
/// reasons. Five times in one page load is a loop.
const REPEATS_ARE_A_FAULT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// Nothing was recorded, so nothing was judged. Deliberately the lowest
    /// value: it must never be reported as a pass.
    Unknown,
    Pass,
    Warn,
    Fail,
}

impl Grade {
    /// The glyph the report draws.
    pub fn mark(self) -> &'static str {
        match self {
            Self::Pass => "✓",
            Self::Warn => "!",
            Self::Fail => "✕",
            Self::Unknown => "–",
        }
    }

    /// The CSS class carrying its colour.
    pub fn tone(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Unknown => "unknown",
        }
    }

    /// Two or three words, because this sits in a badge.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "healthy",
            Self::Warn => "check this",
            Self::Fail => "unhealthy",
            Self::Unknown => "nothing recorded",
        }
    }
}

/// One named check and how it went.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub grade: Grade,
    /// The measurement, in as few words as will carry it.
    pub detail: String,
}

/// The verdict, and what it was made of.
#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub grade: Grade,
    pub checks: Vec<Check>,
}

impl Health {
    fn of(checks: Vec<Check>) -> Self {
        let grade = checks
            .iter()
            .map(|check| check.grade)
            .max()
            .unwrap_or(Grade::Unknown);

        Self { grade, checks }
    }

    /// Nothing to judge, and nothing pretending otherwise.
    fn unjudged() -> Self {
        Self {
            grade: Grade::Unknown,
            checks: Vec::new(),
        }
    }
}

/// Grade a whole page load.
pub fn of_page(summary: &PageSummary, profiles: &[Arc<Profile>]) -> Health {
    if summary.requests == 0 {
        return Health::unjudged();
    }

    let logged = worst_log_level(profiles.iter().flat_map(|profile| profile.logs.iter()));
    let statements: usize = summary.queries;

    Health::of(vec![
        responses(summary.errors, summary.requests, worst_status(profiles)),
        timing(summary.duration),
        repeats(summary.repeated.iter().map(|(_, count)| *count).max()),
        logs(logged),
        Check {
            name: "statements",
            grade: Grade::Pass,
            detail: format!("{statements} across {} requests", summary.requests),
        },
    ])
}

/// Grade one request.
pub fn of_profile(profile: &Profile) -> Health {
    let worst_repeat = profile
        .repeated_queries()
        .iter()
        .map(|(_, count)| *count)
        .max();

    Health::of(vec![
        responses(usize::from(profile.status >= 400), 1, Some(profile.status)),
        timing(profile.duration),
        repeats(worst_repeat),
        logs(worst_log_level(profile.logs.iter())),
    ])
}

fn responses(failed: usize, total: usize, worst: Option<u16>) -> Check {
    let grade = match worst {
        Some(status) if status >= 500 => Grade::Fail,
        Some(status) if status >= 400 => Grade::Warn,
        _ => Grade::Pass,
    };

    let detail = match (failed, worst) {
        (0, _) => format!("{total} ok"),
        (_, Some(status)) => format!("{failed} of {total} returned {status}"),
        (_, None) => format!("{failed} of {total} failed"),
    };

    Check {
        name: "responses",
        grade,
        detail,
    }
}

fn timing(total: Duration) -> Check {
    Check {
        name: "server time",
        grade: if total >= SLOW {
            Grade::Fail
        } else {
            Grade::Pass
        },
        detail: format!("{:.1} ms", total.as_secs_f64() * 1000.0),
    }
}

/// The N+1 check, which is the reason page grouping exists at all.
fn repeats(worst: Option<usize>) -> Check {
    let (grade, detail) = match worst {
        Some(count) if count >= REPEATS_ARE_A_FAULT => {
            (Grade::Fail, format!("one statement run {count} times"))
        }
        Some(count) if count > 1 => (Grade::Warn, format!("one statement run {count} times")),
        _ => (Grade::Pass, "no statement repeated".to_owned()),
    };

    Check {
        name: "repeated sql",
        grade,
        detail,
    }
}

fn logs(worst: Option<&str>) -> Check {
    let grade = match worst {
        Some("ERROR") => Grade::Fail,
        Some("WARN") => Grade::Warn,
        _ => Grade::Pass,
    };

    Check {
        name: "log",
        grade,
        detail: match worst {
            Some(level) => format!("worst line is {level}"),
            None => "nothing above info".to_owned(),
        },
    }
}

fn worst_status(profiles: &[Arc<Profile>]) -> Option<u16> {
    profiles.iter().map(|profile| profile.status).max()
}

fn worst_log_level<'a>(lines: impl Iterator<Item = &'a LogLine>) -> Option<&'static str> {
    let mut worst = None;

    for line in lines {
        match line.level.as_str() {
            "ERROR" => return Some("ERROR"),
            "WARN" => worst = Some("WARN"),
            _ => {}
        }
    }

    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::Caller;
    use crate::profile::{Kind, LogLine, Query, Token};
    use chrono::Utc;

    fn profile(status: u16, duration: Duration) -> Profile {
        Profile {
            token: Token(1),
            at: Utc::now(),
            kind: Kind::Document,
            method: "GET".into(),
            path: "/x".into(),
            query_string: None,
            route: None,
            status,
            duration,
            tenant: None,
            page: Some("p".into()),
            response_bytes: None,
            queries: Vec::new(),
            logs: Vec::new(),
            rss_bytes: None,
        }
    }

    fn log(level: &str) -> LogLine {
        LogLine {
            level: level.into(),
            target: "t".into(),
            message: String::new(),
            fields: Vec::new(),
            source: None,
            line: None,
            caller: Caller::none(),
        }
    }

    fn query(sql: &str) -> Query {
        Query {
            sql: sql.into(),
            caller: Caller::none(),
            elapsed: None,
            rows_returned: None,
            rows_affected: None,
        }
    }

    fn summary(profiles: &[Arc<Profile>]) -> PageSummary {
        PageSummary::of("p", profiles)
    }

    /// The whole reason this module is careful: a page that recorded nothing
    /// passes every check by doing nothing, and must not be green.
    #[test]
    fn nothing_recorded_is_not_a_pass() {
        let health = of_page(&summary(&[]), &[]);

        assert_eq!(health.grade, Grade::Unknown);
        assert!(health.checks.is_empty(), "there was nothing to check");
        assert_ne!(health.grade, Grade::Pass);
    }

    /// `Unknown` orders below `Pass`, so `max()` can never promote an unjudged
    /// page to a healthy one.
    #[test]
    fn unknown_never_outranks_a_real_grade() {
        assert!(Grade::Unknown < Grade::Pass);
        assert!(Grade::Pass < Grade::Warn);
        assert!(Grade::Warn < Grade::Fail);
    }

    #[test]
    fn a_quick_clean_page_is_healthy() {
        let profiles = vec![Arc::new(profile(200, Duration::from_millis(40)))];

        assert_eq!(of_page(&summary(&profiles), &profiles).grade, Grade::Pass);
    }

    #[test]
    fn a_server_error_fails_and_a_client_error_warns() {
        let bad = vec![Arc::new(profile(500, Duration::from_millis(10)))];
        let iffy = vec![Arc::new(profile(404, Duration::from_millis(10)))];

        assert_eq!(of_page(&summary(&bad), &bad).grade, Grade::Fail);
        assert_eq!(of_page(&summary(&iffy), &iffy).grade, Grade::Warn);
    }

    #[test]
    fn slow_is_a_failure_and_names_the_time() {
        let slow = vec![Arc::new(profile(200, Duration::from_millis(900)))];
        let health = of_page(&summary(&slow), &slow);

        assert_eq!(health.grade, Grade::Fail);
        assert!(
            health
                .checks
                .iter()
                .any(|check| check.name == "server time" && check.detail.contains("900")),
            "the badge has to say what it measured"
        );
    }

    /// Twice is ordinary; a loop is not. The threshold is the whole point of
    /// the check, so it is pinned from both sides.
    #[test]
    fn a_repeated_statement_warns_and_a_loop_fails() {
        let mut twice = profile(200, Duration::from_millis(10));
        twice.queries = vec![query("SELECT 1"), query("SELECT 1")];
        let twice = vec![Arc::new(twice)];

        let mut loops = profile(200, Duration::from_millis(10));
        loops.queries = (0..REPEATS_ARE_A_FAULT)
            .map(|_| query("SELECT 1"))
            .collect();
        let loops = vec![Arc::new(loops)];

        assert_eq!(of_page(&summary(&twice), &twice).grade, Grade::Warn);
        assert_eq!(of_page(&summary(&loops), &loops).grade, Grade::Fail);
    }

    #[test]
    fn an_error_line_fails_and_a_warning_warns() {
        let mut errored = profile(200, Duration::from_millis(10));
        errored.logs = vec![log("INFO"), log("ERROR")];
        let errored = vec![Arc::new(errored)];

        let mut warned = profile(200, Duration::from_millis(10));
        warned.logs = vec![log("WARN")];
        let warned = vec![Arc::new(warned)];

        assert_eq!(of_page(&summary(&errored), &errored).grade, Grade::Fail);
        assert_eq!(of_page(&summary(&warned), &warned).grade, Grade::Warn);
    }

    /// The verdict is the worst check, never an average - otherwise three
    /// passes bury one failure.
    #[test]
    fn the_verdict_is_the_worst_check_not_the_average() {
        let mut mixed = profile(500, Duration::from_millis(5));
        mixed.logs = vec![log("INFO")];
        let mixed = vec![Arc::new(mixed)];

        let health = of_page(&summary(&mixed), &mixed);

        assert_eq!(health.grade, Grade::Fail);
        assert!(
            health
                .checks
                .iter()
                .filter(|c| c.grade == Grade::Pass)
                .count()
                >= 2,
            "most checks passed, and the verdict is still a failure"
        );
    }

    #[test]
    fn one_request_is_graded_the_same_way() {
        let mut bad = profile(200, Duration::from_millis(10));
        bad.logs = vec![log("ERROR")];

        assert_eq!(of_profile(&bad).grade, Grade::Fail);
        assert_eq!(
            of_profile(&profile(200, Duration::from_millis(10))).grade,
            Grade::Pass
        );
    }
}
