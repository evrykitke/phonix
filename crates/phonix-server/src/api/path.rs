//! `Path`, with this router's error body instead of axum's.
//!
//! The counterpart of [`ApiJson`](super::json::ApiJson), and it exists for the
//! same reason. `axum::extract::Path`'s rejection is **plain text**:
//!
//! ```text
//! HTTP/1.1 400 Bad Request
//! content-type: text/plain; charset=utf-8
//!
//! Invalid URL: Cannot parse `id` with value `1000`: UUID parsing failed
//! ```
//!
//! Inside the application that is invisible - nothing routes a browser through
//! a typed path parameter it did not build itself. Out here it is what a caller
//! gets for a stale id, a truncated paste, or a crawler walking
//! `/users/1`, `/users/2`. A client that has to parse `problem+json` for every
//! other failure and plain text for this one has been handed two error formats,
//! and the second one is undocumented.
//!
//! # It is a 404, not a 400
//!
//! `1000` is not a UUID, so there is no account it could name. That is an
//! address with nothing at it, and the distinction a 400 would draw - "your
//! request was malformed" versus "your request was fine and found nothing" -
//! is one no client can act on differently.
//!
//! It is also the answer this surface already gives for the same situation:
//! `currencies::get` answers `/currencies/ZZZ` with a 404 rather than a 422,
//! spelling out that a code this build does not know and a code this workspace
//! does not use should not look different to somebody walking a list. A
//! malformed id is the same question one layer down.
//!
//! The one thing that would justify a 400 is a *missing* parameter, which is
//! not reachable: the path a route is registered at is what supplies them.

use axum::extract::FromRequestParts;
use axum::extract::rejection::PathRejection;
use axum::http::StatusCode;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use super::problem::Problem;

/// `axum::extract::Path`, answering with a problem document.
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(translate(&rejection)),
        }
    }
}

/// The rejection as a problem document.
///
/// Deliberately **not** carrying axum's sentence through. `ApiJson` does carry
/// its own, because a body rejection names the line, the column and the missing
/// field, all of which are things the caller sent and needs. A path rejection's
/// text names the Rust type it failed to build - "UUID parsing failed: invalid
/// length" - which tells a caller nothing except how this is implemented.
fn translate(rejection: &PathRejection) -> Problem {
    tracing::debug!(rejection = %rejection, "a path parameter could not be read");

    Problem::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "There is nothing at that address. Check the identifier's shape against \
         the specification at /api/v1/openapi.json.",
    )
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;

    async fn handler(ApiPath(id): ApiPath<Uuid>) -> String {
        id.to_string()
    }

    async fn answer(path: &str) -> (StatusCode, String, String) {
        let app: Router<()> = Router::new().route("/things/{id}", get(handler));

        let response = app
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");

        let status = response.status();
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("a body");

        (
            status,
            content_type,
            String::from_utf8_lossy(&body).into_owned(),
        )
    }

    #[tokio::test]
    async fn a_well_formed_id_reaches_the_handler() {
        let id = Uuid::from_u128(7);
        let (status, _, body) = answer(&format!("/things/{id}")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, id.to_string());
    }

    #[tokio::test]
    async fn an_id_that_is_not_one_answers_a_problem_document() {
        // The whole point: before this extractor existed, the answer here was
        // `400 text/plain` with a sentence about Rust's UUID parser in it.
        let (status, content_type, body) = answer("/things/1000").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type, "application/problem+json");

        let problem: serde_json::Value = serde_json::from_str(&body).expect("the body is json");
        assert_eq!(problem["code"], "not_found");
        assert_eq!(problem["status"], 404);
    }

    #[tokio::test]
    async fn the_answer_names_nothing_about_how_this_is_built() {
        // axum's own text names the Rust type it failed to build. A caller can
        // do nothing with that, and a published surface should not describe
        // its own internals to somebody probing it.
        let (_, _, body) = answer("/things/not-a-uuid").await;

        assert!(!body.contains("UUID"));
        assert!(!body.contains("parse"));
        assert!(
            body.contains("openapi.json"),
            "it says where the shapes are written down"
        );
    }
}
