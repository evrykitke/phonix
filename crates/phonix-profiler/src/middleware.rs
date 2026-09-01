//! The two layers that turn one request into one profile, and why they are
//! two.
//!
//! # The requirement that does not fit in one middleware
//!
//! A profile needs two things that live at opposite ends of the stack.
//!
//! * **The tenant, and the queries that resolved it.** `resolve_tenant` runs
//!   as a `Router::layer`, so it has finished before routing begins. Its
//!   `Span::record("tenant", ..)` and its `SELECT .. FROM tenants` both happen
//!   there. A collector established after it sees neither - the profile says
//!   `tenant: none` on a request that resolved one perfectly well, and its
//!   query list is missing the statement it ran.
//! * **The route pattern.** [`MatchedPath`] is inserted *during* routing, so
//!   nothing wrapping the router can read it. A middleware outside routing
//!   gives every profile a URL and no route, which is most of what a profile
//!   exists to say.
//!
//! No single middleware can be in both places, so there are two:
//!
//! | | attached with | does |
//! |---|---|---|
//! | [`collect`] | `Router::layer`, inside `TraceLayer` and outside `resolve_tenant` | opens the collector, times, files the profile |
//! | [`route`] | `Router::route_layer` | writes [`MatchedPath`] into the collector |
//!
//! Both run on the same task, which is what lets the inner one write into a
//! task-local the outer one opened.
//!
//! A request that matches no route still gets a profile - [`collect`] wraps
//! routing - and it has no route pattern, which is the honest answer rather
//! than a missing row. That is a change from the arrangement this replaces,
//! where a 404 was not profiled at all.

use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use http::HeaderName;

use crate::Profiler;
use crate::collect;
use crate::inject;
use crate::profile::{Kind, Profile};
use crate::rss;

/// The token of the profile for this response, so the browser can follow it.
///
/// Named as Symfony names it, because it is the same idea and somebody will
/// eventually look for it under that name.
pub const DEBUG_TOKEN: HeaderName = HeaderName::from_static("x-debug-token");

/// The page load a request belongs to.
///
/// Set by the toolbar's patched `fetch` on every server call. A navigation
/// carries no such header - the browser is not making that request from
/// script - so for a document the server mints the id instead and hands it to
/// the toolbar in the injected tag. Either way the whole group agrees on one
/// value, which is what makes it a group.
pub const PAGE_HEADER: HeaderName = HeaderName::from_static("x-phonix-page");

/// Note the route pattern this request matched.
///
/// Attach with `route_layer(from_fn(phonix_profiler::middleware::route))`, so
/// it runs inside routing where [`MatchedPath`] exists. It writes into the
/// collector [`collect`] opened further out; with no collector in scope - a
/// request the profiler is not watching - it is a read of a task-local and a
/// return.
pub async fn route(request: Request, next: Next) -> Response {
    if let Some(matched) = request.extensions().get::<MatchedPath>() {
        collect::record_route(matched.as_str().to_owned());
    }

    next.run(request).await
}

/// Profile one request.
///
/// Attach with `layer(from_fn_with_state(profiler, collect))`, positioned
/// inside `TraceLayer` and outside `resolve_tenant` - see this module's
/// documentation for why that placement is the whole point.
pub async fn collect(
    State(profiler): State<Profiler>,
    request: Request,
    next: Next,
) -> Response {
    // Read before the request is consumed. All cheap: three small allocations
    // on a path that is about to touch a database.
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let query_string = request.uri().query().map(str::to_owned);
    let kind = Kind::of(&path);
    let token = profiler.store().mint();

    // A document is the start of a page load and mints the group's id; every
    // other request is expected to arrive already carrying one. The document's
    // own token is reused as the id rather than drawing a second number: it is
    // unique for the same reason the token is, and it makes the group's report
    // reachable from the request that began it.
    let page = request
        .headers()
        .get(&PAGE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| (kind == Kind::Document).then(|| token.to_string()));
    let at = Utc::now();
    let started = Instant::now();

    let (mut response, collected) = collect::scoped(next.run(request)).await;

    let duration = started.elapsed();

    let profile = Profile {
        token,
        at,
        kind,
        method,
        path,
        query_string,
        status: response.status().as_u16(),
        duration,
        // Written in from inside routing. `None` means nothing matched, which
        // is a 404 and is reported as one rather than hidden.
        route: collected.route,
        tenant: collected.tenant,
        page: page.clone(),
        response_bytes: content_length(&response),
        queries: collected.queries,
        logs: collected.logs,
        rss_bytes: rss::current(),
    };

    // Before the profile is filed, so a header is written even if the store is
    // full and this profile is the one evicted a moment later.
    if let Ok(value) = token.to_string().parse() {
        response.headers_mut().insert(DEBUG_TOKEN, value);
    }

    profiler.store().push(profile);

    // Last, so that a page whose profile was never filed still gets a toolbar,
    // and so the tag is appended to the body nothing else is going to touch.
    match page {
        Some(page) => inject::toolbar(response, kind, inject::tag(&page, &token.to_string())),
        None => response,
    }
}

/// The declared response size, when there is one.
///
/// Leptos streams its HTML, so a page declares no length and this is `None`.
/// Measuring the streamed size means wrapping the body in a counting wrapper,
/// which phase one does not do: it would make the profiler part of how the
/// response is delivered, and the first thing anybody would blame for a
/// streaming bug is the tool they turned on to find it.
fn content_length(response: &Response) -> Option<u64> {
    response
        .headers()
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}
