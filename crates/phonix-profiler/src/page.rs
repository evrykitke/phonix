//! One page load: the group of requests a single screen produced.
//!
//! This is the unit the profiler is built around, and
//! `docs/adr/0004-development-profiler.md` section 2 is the argument for it.
//! The short version: a first visit is one document plus N server functions,
//! an in-app navigation is *zero* documents plus N server functions, and a
//! per-request list of forty rows tells nobody which screen asked for what.
//!
//! The reason it is worth the machinery is [`PageSummary::repeated`]. The
//! commonest real performance bug in an application shaped like this one is a
//! screen that makes eleven server calls where two would do, or eleven
//! identical statements spread across four of them. Neither is visible in any
//! single request's profile - which is exactly why nobody finds them.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::profile::{Kind, Profile, Token, millis};

/// Everything the toolbar and the page report need about one page load.
#[derive(Debug, Clone, Serialize)]
pub struct PageSummary {
    pub page: String,
    pub requests: usize,

    /// Server time summed across the group.
    ///
    /// A sum and not an elapsed span, because the calls overlap: a screen
    /// resolving six resources runs them concurrently, and wall time between
    /// the first and the last would also count the developer's own reading
    /// time on an in-app navigation. The sum answers "what did this screen
    /// cost the server", which is the question worth asking.
    #[serde(rename = "duration_ms", serialize_with = "millis")]
    pub duration: Duration,

    #[serde(rename = "sql_ms", serialize_with = "millis")]
    pub sql: Duration,
    pub queries: usize,
    /// Responses of 400 or worse, so the toolbar can colour itself.
    pub errors: usize,

    /// Statement shapes run more than once *across the whole page load*.
    ///
    /// The cross-request version of `Profile::repeated_queries`, and the
    /// reason the grouping exists: the same statement run once in each of
    /// eleven server functions is an N+1 that no single profile can see.
    pub repeated: Vec<(String, usize)>,

    /// Whether a document request is in the group.
    ///
    /// `false` for an in-app navigation, which produces none. The report says
    /// so rather than looking broken.
    pub has_document: bool,

    pub profiles: Vec<PageEntry>,
}

/// One request's row in a page load.
#[derive(Debug, Clone, Serialize)]
pub struct PageEntry {
    pub token: Token,
    pub kind: Kind,
    pub method: String,
    pub path: String,
    pub route: Option<String>,
    pub status: u16,
    #[serde(serialize_with = "millis")]
    pub duration: Duration,
    pub queries: usize,
}

impl PageSummary {
    /// Summarise the profiles belonging to one page load, oldest first.
    pub fn of(page: &str, profiles: &[Arc<Profile>]) -> Self {
        let mut summary = Self {
            page: page.to_owned(),
            requests: profiles.len(),
            duration: Duration::ZERO,
            sql: Duration::ZERO,
            queries: 0,
            errors: 0,
            repeated: Vec::new(),
            has_document: false,
            profiles: Vec::with_capacity(profiles.len()),
        };

        // Shapes are counted across every request in the group, which is the
        // whole point - see the note on `repeated`.
        let mut shapes: Vec<(String, usize)> = Vec::new();

        for profile in profiles {
            summary.duration += profile.duration;
            summary.sql += profile.query_time();
            summary.queries += profile.queries.len();

            if profile.status >= 400 {
                summary.errors += 1;
            }

            if profile.kind == Kind::Document {
                summary.has_document = true;
            }

            for query in &profile.queries {
                let shape = query.shape();

                match shapes.iter_mut().find(|(seen, _)| seen == &shape) {
                    Some((_, count)) => *count += 1,
                    None => shapes.push((shape, 1)),
                }
            }

            summary.profiles.push(PageEntry {
                token: profile.token,
                kind: profile.kind,
                method: profile.method.clone(),
                path: profile.path.clone(),
                route: profile.route.clone(),
                status: profile.status,
                duration: profile.duration,
                queries: profile.queries.len(),
            });
        }

        shapes.retain(|(_, count)| *count > 1);
        shapes.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        summary.repeated = shapes;

        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::Caller;
    use crate::profile::Query;
    use chrono::Utc;

    fn profile(token: u64, path: &str, sql: &[&str], status: u16) -> Arc<Profile> {
        Arc::new(Profile {
            token: Token(token),
            at: Utc::now(),
            kind: Kind::of(path),
            method: "GET".into(),
            path: path.into(),
            query_string: None,
            route: None,
            status,
            duration: Duration::from_millis(10),
            tenant: None,
            page: Some("p1".into()),
            response_bytes: None,
            queries: sql
                .iter()
                .map(|statement| Query {
                    sql: (*statement).to_owned(),
                    caller: Caller::none(),
                    elapsed: Some(Duration::from_millis(2)),
                    rows_returned: None,
                    rows_affected: None,
                })
                .collect(),
            logs: Vec::new(),
            rss_bytes: None,
        })
    }

    /// The one thing a per-request profiler cannot say. One statement in each
    /// of three server functions is an N+1, and no single profile contains
    /// more than one of them.
    #[test]
    fn a_statement_repeated_across_requests_is_the_finding() {
        let group = vec![
            profile(1, "/api/users_list", &["SELECT * FROM roles WHERE id = $1"], 200),
            profile(2, "/api/roles_get", &["SELECT * FROM roles WHERE id = $1"], 200),
            profile(3, "/api/roles_get", &["SELECT * FROM roles WHERE id = $1"], 200),
        ];

        let summary = PageSummary::of("p1", &group);

        assert_eq!(summary.repeated.len(), 1);
        assert_eq!(summary.repeated[0].1, 3);
        for profile in &group {
            assert!(
                profile.repeated_queries().is_empty(),
                "no single request repeats anything, which is why this is the group's job"
            );
        }
    }

    #[test]
    fn the_totals_are_summed_across_the_group() {
        let group = vec![
            profile(1, "/admin/users", &["SELECT 1"], 200),
            profile(2, "/api/users_list", &["SELECT 2", "SELECT 3"], 500),
        ];

        let summary = PageSummary::of("p1", &group);

        assert_eq!(summary.requests, 2);
        assert_eq!(summary.queries, 3);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.duration, Duration::from_millis(20));
        assert_eq!(summary.sql, Duration::from_millis(6));
    }

    /// An in-app navigation makes no document request. The group is still a
    /// page load, and the report has to be able to say which kind it is
    /// looking at rather than appearing to have lost something.
    #[test]
    fn a_group_with_no_document_says_so() {
        let group = vec![profile(1, "/api/users_list", &[], 200)];

        assert!(!PageSummary::of("p1", &group).has_document);
        assert!(PageSummary::of("p1", &[profile(1, "/admin/users", &[], 200)]).has_document);
    }

    #[test]
    fn an_empty_group_summarises_to_nothing_rather_than_panicking() {
        let summary = PageSummary::of("gone", &[]);

        assert_eq!(summary.requests, 0);
        assert_eq!(summary.duration, Duration::ZERO);
        assert!(summary.repeated.is_empty());
    }
}
