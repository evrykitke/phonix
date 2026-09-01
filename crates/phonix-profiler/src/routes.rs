//! `/_profiler` - the report, and the JSON behind it.
//!
//! Mounted outside the application's outer layers, so it answers on any host
//! and keeps answering when resolving a tenant is the thing that has broken.
//! See `phonix_server::profiler::Profiling::mount`.

use axum::Router;
use axum::extract::{Path, Query as QueryParams, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::Profiler;
use crate::flow::PageFlow;
use crate::page::PageSummary;
use crate::profile::Token;
use crate::report;
use crate::source::{self, Allowed};

/// How many rows the index draws.
///
/// The ring is larger than this. A page of a hundred is already more than
/// anybody reads, and the interesting request is nearly always in the first
/// ten.
const PAGE_SIZE: usize = 100;

pub fn router() -> Router<Profiler> {
    Router::new()
        .route("/_profiler", get(index))
        // Before the token route, so a literal segment is not read as a token.
        // axum prefers the static path either way; the order is for whoever
        // reads this next.
        .route("/_profiler/toolbar.js", get(toolbar_js))
        .route("/_profiler/report.js", get(report_js))
        .route("/_profiler/page/{page}", get(page_report))
        .route("/_profiler/source/page/{page}", get(source_view))
        .route("/_profiler/{token}", get(detail))
        .route("/_profiler/api/recent", get(recent_json))
        .route("/_profiler/api/page/{page}", get(page_json))
        .route("/_profiler/api/{token}", get(detail_json))
}

/// The report's own interactivity, compiled into the binary.
///
/// Same arrangement as the toolbar below and for the same reasons. Everything
/// it does is an enhancement - tabs, the modal, the sidebar toggle - so a
/// failure to load leaves the report exactly as it was before there was a
/// script, which is a page that still works.
async fn report_js() -> Response {
    javascript(include_str!("report.js"))
}

/// The toolbar, compiled into the binary.
///
/// `include_str!` rather than a file read, for the same reason the report has
/// no build step: there is nothing to deploy, nothing to find at runtime, and
/// no way for the served toolbar to disagree with the server that serves it.
///
/// Never cached. It changes whenever the binary does, and a developer holding
/// a stale toolbar would be debugging the wrong tool.
async fn toolbar_js() -> Response {
    javascript(include_str!("toolbar.js"))
}

/// Never cached: it changes whenever the binary does, and a developer holding a
/// stale script would be debugging the wrong tool.
fn javascript(source: &'static str) -> Response {
    (
        [
            (http::header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (http::header::CACHE_CONTROL, "no-store"),
        ],
        source,
    )
        .into_response()
}

/// Which request within the page load to draw, if not the whole thing.
#[derive(Debug, Deserialize)]
struct PageParams {
    phase: Option<String>,
}

async fn page_report(
    State(profiler): State<Profiler>,
    Path(page): Path<String>,
    QueryParams(params): QueryParams<PageParams>,
) -> Html<String> {
    let profiles = profiler.store().page(&page);
    // An unparseable or unknown phase falls back to the whole load rather than
    // erroring: it is a view preference in a URL, and the page it belongs to is
    // still perfectly renderable.
    let phase = params.phase.as_deref().and_then(parse);

    let summary = PageSummary::of(&page, &profiles);
    let health = crate::health::of_page(&summary, &profiles);

    Html(report::page_load(
        &summary,
        &PageFlow::of(&profiles),
        phase,
        &health,
    ))
}

/// What a file the diagram named looks like.
#[derive(Debug, Deserialize)]
struct SourceParams {
    file: String,
    line: u32,
}

/// Show one file of this checkout, around one line.
///
/// Every refusal is the same 404 page. The two gates are in [`crate::source`];
/// this only decides what is in scope, which is the profiles of one page load.
async fn source_view(
    State(profiler): State<Profiler>,
    Path(page): Path<String>,
    QueryParams(params): QueryParams<SourceParams>,
) -> Response {
    let refused = || (StatusCode::NOT_FOUND, Html(report::no_source(&page))).into_response();

    let Some(root) = profiler.source_root() else {
        return refused();
    };

    let profiles = profiler.store().page(&page);
    let allowed = Allowed::of(&profiles);

    match source::read(root, &allowed, &params.file, params.line) {
        Some(snippet) => Html(report::source(&page, &snippet)).into_response(),
        None => refused(),
    }
}

async fn page_json(
    State(profiler): State<Profiler>,
    Path(page): Path<String>,
) -> Result<Json<PageSummary>, StatusCode> {
    let profiles = profiler.store().page(&page);

    // An empty group is a 404 rather than a summary of nothing, so the toolbar
    // can tell "this page load made no calls yet" apart from "the server
    // restarted and everything it held is gone" - which is the commoner of the
    // two by a distance, and the one worth saying out loud.
    if profiles.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(PageSummary::of(&page, &profiles)))
}

#[derive(Debug, Deserialize)]
struct IndexParams {
    /// Include assets, which are hidden by default because they are ninety per
    /// cent of the rows and none of the interest.
    #[serde(default)]
    all: bool,
}

async fn index(
    State(profiler): State<Profiler>,
    QueryParams(params): QueryParams<IndexParams>,
) -> Html<String> {
    let store = profiler.store();
    // Deeper than the page, then filtered: taking a hundred and *then*
    // dropping the assets would draw twelve rows and look like the profiler
    // had stopped collecting.
    let mut profiles = store.recent(store.len());

    if !params.all {
        profiles.retain(|profile| profile.kind.is_interesting());
    }

    profiles.truncate(PAGE_SIZE);

    Html(report::index(&profiles, params.all, store.len()))
}

async fn detail(State(profiler): State<Profiler>, Path(token): Path<String>) -> Response {
    let found = parse(&token).and_then(|token| profiler.store().get(token));

    match found {
        Some(profile) => Html(report::detail(&profile)).into_response(),
        None => (StatusCode::NOT_FOUND, Html(report::missing(&token))).into_response(),
    }
}

async fn recent_json(State(profiler): State<Profiler>) -> Json<Vec<crate::Profile>> {
    let profiles = profiler
        .store()
        .recent(PAGE_SIZE)
        .iter()
        .map(|profile| (**profile).clone())
        .collect();

    Json(profiles)
}

async fn detail_json(
    State(profiler): State<Profiler>,
    Path(token): Path<String>,
) -> Result<Json<crate::Profile>, StatusCode> {
    let token = parse(&token).ok_or(StatusCode::NOT_FOUND)?;
    let profile = profiler.store().get(token).ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json((*profile).clone()))
}

/// A token from the path, or `None`.
///
/// A malformed token is a 404 rather than a 400: to whoever typed it, "there
/// is no such profile" is the same answer and the more useful one.
fn parse(text: &str) -> Option<Token> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_parses_back_from_its_own_rendering() {
        let token = Token(123_456);

        assert_eq!(parse(&token.to_string()), Some(token));
    }

    #[test]
    fn rubbish_is_not_a_token() {
        assert_eq!(parse("../../etc/passwd"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("zzzz"), None);
    }
}
