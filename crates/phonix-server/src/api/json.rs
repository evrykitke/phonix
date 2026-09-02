//! `Json`, with this router's error body instead of axum's.
//!
//! `axum::Json`'s rejection is plain text with no code in it. Inside the
//! application that is invisible - a server function never produces one -
//! but out here it is the answer a client gets for the most ordinary mistake
//! there is: a body with a field missing or a comma in the wrong place. A
//! caller that has to parse `problem+json` for every other failure and plain
//! text for this one has been handed two error formats.
//!
//! So the wrapper exists only to translate the rejection, and the three
//! translations are the whole of it:
//!
//! * **Syntactically not JSON** - 400. Nothing was understood, so there is no
//!   field to name.
//! * **JSON, but not this shape** - 422. Understood and refused, which is what
//!   422 means and what every validation failure in this API already answers.
//! * **No `Content-Type: application/json`** - 415, naming what was wanted.
//!
//! # `Option<ApiJson<T>>`, for the endpoints whose body is optional
//!
//! Some requests are a verb with nothing to say: revoking a key without giving
//! a reason. Requiring `Content-Type: application/json` and a literal `{}` for
//! those would answer the most natural spelling of the request - a bare `POST`
//! - with a 415, which is a confusing way to say "that was fine".
//!
//! So `ApiJson` also implements [`OptionalFromRequest`], mirroring what
//! `axum::Json` does: **no `Content-Type` header at all** means `None`, and
//! anything else is parsed and refused exactly as it would be if the body were
//! required. A caller that sends a malformed body still gets a 400 rather than
//! having it quietly ignored, which is the half that matters.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, OptionalFromRequest};
use axum::http::{Request, StatusCode};

use super::problem::Problem;

/// `axum::Json`, answering with a problem document.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request(request: Request<axum::body::Body>, state: &S) -> Result<Self, Problem> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(translate(rejection)),
        }
    }
}

impl<T, S> OptionalFromRequest<S> for ApiJson<T>
where
    axum::Json<T>: OptionalFromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request(
        request: Request<axum::body::Body>,
        state: &S,
    ) -> Result<Option<Self>, Problem> {
        match <axum::Json<T> as OptionalFromRequest<S>>::from_request(request, state).await {
            Ok(Some(axum::Json(value))) => Ok(Some(Self(value))),
            Ok(None) => Ok(None),
            Err(rejection) => Err(translate(rejection)),
        }
    }
}

fn translate(rejection: JsonRejection) -> Problem {
    // `body_text` is axum's own sentence, and it is a good one: it names the
    // line and column of a syntax error and the missing field of a shape
    // error. It carries nothing but what the caller sent.
    let detail = rejection.body_text();

    match rejection {
        JsonRejection::JsonDataError(_) => {
            Problem::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", detail)
        }
        JsonRejection::MissingJsonContentType(_) => Problem::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Send the body as `Content-Type: application/json`.",
        ),
        // Syntax errors, a body that could not be read, and whatever axum adds
        // later. All of them mean the same thing to a caller: nothing was
        // understood, so there is no field to point at.
        _ => Problem::new(StatusCode::BAD_REQUEST, "malformed_body", detail),
    }
}
