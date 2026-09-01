//! Getting the toolbar's `<script>` tag into a page the profiler does not own.
//!
//! # Why the tag is appended rather than placed
//!
//! The obvious home for it is the Leptos shell, next to `HydrationScripts`.
//! That would make `phonix-web` carry a profiler-shaped hole in its document
//! and need a flag plumbed through `LeptosOptions` to decide whether to fill
//! it - a build with no profiler in it would still have the shape of one. The
//! gate in `docs/adr/0004-development-profiler.md` section 8 is worth more
//! than the tidiness: without the feature there is no crate, and with no crate
//! there is nothing to inject.
//!
//! So the tag is appended to the response body as it goes out, by the same
//! middleware that profiles the request.
//!
//! # Why the end of the document is a safe place to put it
//!
//! Not before `</body>`, which would mean scanning a streamed body for a
//! string that can straddle two chunks. After everything, as one final chunk.
//! The parser hoists a trailing `<script>` into the body, so it lands as the
//! last child of `<body>` - *after* the node `hydrate_body` is walking.
//!
//! That ordering is the whole safety argument. Leptos hydrates `<body>`'s
//! children by position, so an element appended *before* the app's root would
//! shift every index and take the page. Appended after the app's root, it is
//! past the cursor by the time hydration reaches the end, and Leptos already
//! emits its own trailing scripts there when it resolves a suspended chunk.
//!
//! The script itself still does not touch the DOM until hydration has
//! finished - see `toolbar.js`, and section 7 for why that is not negotiable.

use axum::body::{Body, Bytes};
use axum::response::Response;
use futures::StreamExt;

use crate::profile::Kind;

/// Append `script` to `response`'s body, if this is a response a script
/// belongs in.
///
/// Returns the response unchanged when it is not an HTML document: an image,
/// a redirect, a server function's JSON. Injecting into any of those would
/// corrupt it.
pub fn toolbar(response: Response, kind: Kind, script: String) -> Response {
    if !accepts_markup(&response, kind) {
        return response;
    }

    let (mut parts, body) = response.into_parts();

    // A declared length has to grow by exactly what is being added, or the
    // client stops reading early and the tag is truncated. Leptos streams and
    // declares nothing, so this is the branch nobody takes - it exists because
    // the alternative failure is a page that renders and a toolbar that is
    // half a tag.
    if let Some(length) = declared_length(&parts.headers) {
        match (length + script.len() as u64).to_string().parse() {
            Ok(value) => {
                parts.headers.insert(http::header::CONTENT_LENGTH, value);
            }
            // Unreachable - a number is always a valid header value - but the
            // fallback is to leave the response alone rather than to send a
            // body that disagrees with its own length.
            Err(_) => return Response::from_parts(parts, body),
        }
    }

    let appended = body
        .into_data_stream()
        .chain(futures::stream::once(async move {
            Ok::<Bytes, axum::Error>(Bytes::from(script))
        }));

    Response::from_parts(parts, Body::from_stream(appended))
}

/// Whether this response is an HTML document being sent to a browser.
///
/// Three conditions, all of them necessary. The kind rules out server
/// functions and assets before a header is read; the status rules out
/// redirects and error pages, which either have no body worth a toolbar or
/// are not the document the browser will keep; the content type is the last
/// word, because a route classified as a page can still answer with a file.
fn accepts_markup(response: &Response, kind: Kind) -> bool {
    if kind != Kind::Document || !response.status().is_success() {
        return false;
    }

    response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"))
}

fn declared_length(headers: &http::HeaderMap) -> Option<u64> {
    headers
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// The tag itself.
///
/// `defer` so it runs after the document is parsed, and the page id travels on
/// the element rather than in the script's text: the script is one static file
/// every page shares, and only the attribute differs between them. That is
/// also how the document joins its own page group - the browser cannot read
/// the response headers of a navigation, so the id has to arrive in the
/// markup.
pub fn tag(page: &str, token: &str) -> String {
    format!(
        "<script src=\"/_profiler/toolbar.js\" defer \
         data-page=\"{page}\" data-token=\"{token}\"></script>",
        page = attribute(page),
        token = attribute(token),
    )
}

/// Escape a value going into a double-quoted attribute.
///
/// Both values here are hex strings this crate minted, so nothing can reach
/// this that needs escaping. It is here because that is a fact about today's
/// callers and not about the function, and the failure it would prevent is
/// script injection into every page of the application.
fn attribute(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn html(status: StatusCode) -> Response {
        Response::builder()
            .status(status)
            .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from("<html></html>"))
            .expect("a response with a body builds")
    }

    #[test]
    fn an_html_page_takes_the_toolbar() {
        assert!(accepts_markup(&html(StatusCode::OK), Kind::Document));
    }

    /// A server function answers JSON on a path the classifier calls a server
    /// fn. Appending markup to it would corrupt the response the application
    /// is about to deserialise.
    #[test]
    fn a_server_function_does_not() {
        let response = Response::builder()
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("a response with a body builds");

        assert!(!accepts_markup(&response, Kind::ServerFn));
    }

    /// A route classified as a page can still answer with a file - a download
    /// handler, an image served from a catch-all. The content type is what
    /// settles it.
    #[test]
    fn a_page_route_answering_with_a_file_does_not() {
        let response = Response::builder()
            .header(http::header::CONTENT_TYPE, "image/png")
            .body(Body::empty())
            .expect("a response with a body builds");

        assert!(!accepts_markup(&response, Kind::Document));
    }

    #[test]
    fn a_redirect_does_not() {
        assert!(!accepts_markup(
            &html(StatusCode::SEE_OTHER),
            Kind::Document
        ));
    }

    #[tokio::test]
    async fn the_tag_lands_at_the_end_of_the_body() {
        let response = toolbar(html(StatusCode::OK), Kind::Document, tag("abc", "def"));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body collects");
        let text = String::from_utf8(body.to_vec()).expect("the body is utf-8");

        assert!(text.starts_with("<html></html>"));
        assert!(text.ends_with("</script>"));
        assert!(text.contains("data-page=\"abc\""));
    }

    /// A body that declares its length and then sends more is truncated by the
    /// client, which would show as a toolbar that loads on some pages and not
    /// others.
    #[tokio::test]
    async fn a_declared_length_grows_by_what_was_added() {
        let script = tag("abc", "def");
        let mut response = html(StatusCode::OK);
        response.headers_mut().insert(
            http::header::CONTENT_LENGTH,
            "13".parse().expect("a number is a header value"),
        );

        let injected = toolbar(response, Kind::Document, script.clone());
        let declared = declared_length(injected.headers()).expect("the length survives");

        assert_eq!(declared, 13 + script.len() as u64);
    }

    /// Everything reaching `tag` is a token this crate minted. If that ever
    /// stops being true, the attribute is the way out of the tag and into the
    /// page.
    #[test]
    fn nothing_can_break_out_of_the_attribute() {
        let html = tag("a\" onload=\"alert(1)", "b");

        // The letters survive - they are harmless. What does not is every
        // character that could end the attribute or start another one, which
        // is why counting the quotes is the assertion that matters.
        assert!(html.contains("data-page=\"aonloadalert1\""));
        assert_eq!(html.matches('"').count(), 6, "two quotes per attribute");
        assert_eq!(html.matches('=').count(), 3, "one per attribute");
    }
}
