//! The background work, per workspace.
//!
//! Not a screen over the four loops in `phonix-server`'s `jobs.rs` - those run
//! in that process, in memory, and nothing else can see them. This is a screen
//! over the tables they work on, which is the half that survives a restart and
//! the half worth watching: a loop that has stopped shows up here as a number
//! that stops going down, which needs no instrumentation in the server at all.
//!
//! Counts and timestamps only. An upload's file name and an event's payload are
//! a workspace's business data, and Desk may not read those - ADR 0005 section
//! 6. Both reads are aggregates in the repository layer, so this page has no
//! way to widen them.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use chrono::{DateTime, Utc};
use phonix_services::desk::queues;

use crate::html::{Chrome, render};
use crate::routes::{SignedIn, internal_error};
use crate::state::DeskState;

pub struct QueueRow {
    /// The slug and not the display name. This table is eight columns wide
    /// already, and the slug is what somebody types into the workspace page
    /// they are about to open.
    pub slug: String,
    pub readable: bool,
    pub error: String,

    pub uploads_waiting: u64,
    pub uploads_in_flight: u64,
    pub uploads_failed: u64,
    pub uploads_oldest: String,

    pub outbox_unpublished: u64,
    pub outbox_retried: u64,
    pub outbox_oldest: String,
}

#[derive(Template)]
#[template(path = "queues.html")]
pub struct QueuesPage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub rows: Vec<QueueRow>,
    pub busy: usize,
    pub unreadable: usize,
}

pub async fn index(SignedIn(caller): SignedIn, State(state): State<DeskState>) -> Response {
    let survey = match queues::survey(&state.catalog, &state.config.database).await {
        Ok(survey) => survey,
        Err(err) => return internal_error(err, "reading the job queues"),
    };

    let busy = survey.iter().filter(|w| w.outstanding() > 0).count();
    let unreadable = survey.iter().filter(|w| w.error.is_some()).count();

    let rows = survey
        .iter()
        .map(|workspace| QueueRow {
            slug: workspace.slug.as_str().to_owned(),
            readable: workspace.error.is_none(),
            error: workspace.error.clone().unwrap_or_default(),

            uploads_waiting: workspace.uploads.as_ref().map(|q| q.waiting).unwrap_or(0),
            uploads_in_flight: workspace.uploads.as_ref().map(|q| q.in_flight).unwrap_or(0),
            uploads_failed: workspace.uploads.as_ref().map(|q| q.failed).unwrap_or(0),
            uploads_oldest: workspace
                .uploads
                .as_ref()
                .and_then(|q| q.oldest_waiting_at)
                .map(age)
                .unwrap_or_else(|| "-".to_owned()),

            outbox_unpublished: workspace
                .outbox
                .as_ref()
                .map(|b| b.unpublished)
                .unwrap_or(0),
            outbox_retried: workspace.outbox.as_ref().map(|b| b.retried).unwrap_or(0),
            outbox_oldest: workspace
                .outbox
                .as_ref()
                .and_then(|b| b.oldest_at)
                .map(age)
                .unwrap_or_else(|| "-".to_owned()),
        })
        .collect();

    render(&QueuesPage {
        title: "Job queues".to_owned(),
        chrome: Chrome::new(&caller.user.display_name, state.environment(), "queues"),
        banner: None,
        rows,
        busy,
        unreadable,
    })
}

/// How long ago, in the coarsest unit that is still true.
///
/// A backlog of ten from a minute ago is a busy moment; a backlog of ten from
/// Tuesday is a broker that has been unreachable since Tuesday. An absolute
/// timestamp makes the reader do that subtraction, and this page exists to be
/// glanced at.
fn age(at: DateTime<Utc>) -> String {
    let seconds = (Utc::now() - at).num_seconds().max(0);

    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn an_age_uses_the_coarsest_unit_that_is_still_true() {
        let now = Utc::now();

        assert_eq!(age(now - Duration::seconds(5)), "5s");
        assert_eq!(age(now - Duration::minutes(3)), "3m");
        assert_eq!(age(now - Duration::hours(5)), "5h");
        assert_eq!(age(now - Duration::days(2)), "2d");
    }

    /// Clock skew between two machines is real, and a negative age rendering as
    /// "-4s" reads as a bug in the page rather than a bug in the clock.
    #[test]
    fn a_timestamp_in_the_future_does_not_render_as_negative() {
        assert_eq!(age(Utc::now() + Duration::hours(1)), "0s");
    }
}
