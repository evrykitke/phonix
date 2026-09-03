//! Whether the box answers: the catalog, Redis and RabbitMQ.
//!
//! The first question when a workspace is misbehaving, and the one this tool
//! is best placed to answer - Desk runs in its own process, so a page that
//! renders at all has already proved that `phonix-server` being wedged is not
//! why you cannot see anything.
//!
//! # Its own page, and not a panel on the workspace list
//!
//! ADR 0005 calls it a panel, and the workspace list is where somebody lands.
//! It is here instead for the reason `queues` is: the home page must stay
//! cheap enough to reload without thinking about it, and these probes are
//! network round trips that are fast when everything is up and up to three
//! seconds each when it is not. Putting them on the landing page would make
//! the screen that says which workspace is wedged slowest at exactly the
//! moment it is wanted - and would couple Desk's first page to infrastructure
//! Desk deliberately does not depend on.
//!
//! # Nothing to press
//!
//! Reloading is the retry. A button that restarts a service would be the
//! remote shell with a login form that ADR 0005 section 12 refuses, and
//! execution stays on SSH.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use phonix_services::desk::dependencies::{self, Check, Standing};

use crate::html::{Chrome, render};
use crate::routes::SignedIn;
use crate::state::DeskState;

pub struct CheckRow {
    pub name: String,
    pub target: String,
    pub method: String,
    pub standing: String,
    /// Split into two flags rather than compared as a string in the template:
    /// a page that spells `"unreachable"` twice is a page where a rename can
    /// silently stop colouring anything.
    pub down: bool,
    pub disabled: bool,
    pub detail: String,
    pub took: String,
}

#[derive(Template)]
#[template(path = "dependencies.html")]
pub struct DependenciesPage {
    pub title: String,
    pub chrome: Chrome,
    pub banner: Option<String>,
    pub rows: Vec<CheckRow>,
    pub unreachable: usize,
    pub all_well: bool,
}

pub async fn index(SignedIn(caller): SignedIn, State(state): State<DeskState>) -> Response {
    // No `internal_error` arm: a probe reports failure rather than returning
    // it, so there is no error here to render a 500 from. That is the whole
    // shape of `dependencies::probe`.
    let report = dependencies::probe(
        &state.catalog,
        &state.config.database,
        &state.config.redis,
        &state.config.rabbitmq,
    )
    .await;

    let rows = report.checks.iter().map(row_for).collect();

    render(&DependenciesPage {
        title: "Dependencies".to_owned(),
        chrome: Chrome::new(
            &caller.user.display_name,
            state.environment(),
            "dependencies",
        ),
        banner: None,
        unreachable: report.unreachable(),
        all_well: report.all_well(),
        rows,
    })
}

fn row_for(check: &Check) -> CheckRow {
    CheckRow {
        name: check.name.to_owned(),
        target: check.target.clone(),
        method: check.method.to_owned(),
        standing: check.standing.as_str().to_owned(),
        down: check.standing.is_unreachable(),
        disabled: check.standing == Standing::Disabled,
        detail: check.detail.clone().unwrap_or_default(),
        took: took(check),
    }
}

/// How long it took, or nothing at all when it was never asked.
///
/// A disabled dependency showing `0ms` would read as a dependency that answered
/// instantly, which is the opposite of what happened.
fn took(check: &Check) -> String {
    if check.standing == Standing::Disabled {
        return "-".to_owned();
    }

    format!("{}ms", check.took.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn check(standing: Standing, took: Duration) -> Check {
        Check {
            name: "redis",
            target: "127.0.0.1:6379".to_owned(),
            method: "TCP connect",
            standing,
            detail: None,
            took,
        }
    }

    #[test]
    fn a_dependency_that_answered_is_timed() {
        let row = row_for(&check(Standing::Reachable, Duration::from_millis(4)));

        assert_eq!(row.took, "4ms");
        assert_eq!(row.standing, "ok");
        assert!(!row.down);
        assert!(!row.disabled);
    }

    /// `0ms` against a dependency nobody asked would read as the fastest
    /// answer of the three.
    #[test]
    fn a_dependency_that_was_never_asked_has_no_timing() {
        let row = row_for(&check(Standing::Disabled, Duration::ZERO));

        assert_eq!(row.took, "-");
        assert!(row.disabled);
        assert!(!row.down, "off is not down");
    }

    /// The page renders, and a failure's reason reaches it. A panel that showed
    /// "unreachable" with no reason would send somebody to the logs for the one
    /// fact it already had.
    #[test]
    fn the_page_renders_and_a_failure_carries_its_reason() {
        let mut failed = check(Standing::Unreachable, Duration::from_millis(3000));
        failed.detail = Some("connection refused".to_owned());

        let rows = vec![row_for(&failed)];
        let rendered = DependenciesPage {
            title: "Dependencies".to_owned(),
            chrome: Chrome::new("Ada", "development", "dependencies"),
            banner: None,
            unreachable: 1,
            all_well: false,
            rows,
        }
        .render()
        .expect("renders");

        assert!(rendered.contains("connection refused"), "the row's own reason");
        assert!(rendered.contains("127.0.0.1:6379"), "and which one it was");
        assert!(rendered.contains("unreachable"), "and the word for it");
        // The banner above the table, which counts rather than explains.
        assert!(rendered.contains("1 dependency(s) did not answer."));
    }

    #[test]
    fn a_dependency_that_is_down_is_flagged_for_the_page() {
        let row = row_for(&check(Standing::Unreachable, Duration::from_millis(3000)));

        assert!(row.down);
        assert_eq!(row.standing, "unreachable");
    }
}
