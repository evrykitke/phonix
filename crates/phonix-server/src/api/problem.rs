//! What `/api/v1` answers when something goes wrong.
//!
//! RFC 9457 `application/problem+json`, rather than an envelope of our own: it
//! is the one error format an HTTP client library might already understand, and
//! the field names are not worth arguing about.
//!
//! # `code` is the contract, and it is not a translation key
//!
//! Every user-facing string in Phonix is a `Message` key resolved by the view.
//! That is right for a browser and wrong here: a key labels a *sentence*, the
//! sentence is translated, and neither is a stable thing for a script to branch
//! on. So the machine-readable half is [`phonix_core::Error::code`] - which
//! already exists, is already coarse, and already has a test tying it to a
//! status.
//!
//! `title` and `detail` are for a person reading a log. They are English, they
//! are not a contract, and they may be reworded in any release.
//!
//! # Nothing sensitive can reach here
//!
//! Not by care taken in this module, but by the conversion it goes through:
//! `ServiceError -> Error` logs the cause server-side and replaces it with a
//! coarse label, so a connection string, a constraint name or a key
//! description cannot cross. That is also why no handler in this module
//! matches on `ServiceError` directly - the only exception is a field
//! rejection, which contains nothing but what the caller submitted.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use phonix_core::Error as CoreError;
use phonix_services::ServiceError;
use serde::Serialize;
use utoipa::ToSchema;

/// One rejected field.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FieldProblem {
    /// The field as the request spelled it.
    #[schema(example = "symbol")]
    pub field: String,
    /// The stable identifier of the sentence below. A client that renders its
    /// own wording keys off this.
    #[schema(example = "error.currency.gone")]
    pub code: String,
    /// The same thing in English, for a caller who does not want a catalog.
    pub message: String,
}

/// The body of every failed request.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(title = "Problem")]
pub struct Problem {
    /// A URI identifying the *kind* of problem. Stable, and not expected to
    /// resolve to anything.
    #[schema(example = "urn:phonix:problem:validation")]
    pub r#type: String,
    /// A short English summary of the kind. Not a contract.
    pub title: String,
    #[serde(skip)]
    #[schema(ignore)]
    pub retry_after_secs: Option<u64>,
    #[schema(example = 422)]
    pub status: u16,
    /// **The machine-readable half.** Stable within a major version.
    #[schema(example = "validation")]
    pub code: String,
    /// What went wrong this time, in English.
    pub detail: String,
    /// Present when individual fields were refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldProblem>,
}

impl Problem {
    /// A problem with a code this module names itself.
    ///
    /// For the conditions that are the API's own - no credential, a workspace
    /// that has not been sold the API, a path that does not exist - as opposed
    /// to the ones a use case reports.
    pub fn new(status: StatusCode, code: &str, detail: impl Into<String>) -> Self {
        Self {
            r#type: format!("urn:phonix:problem:{code}"),
            title: title_for(code).to_owned(),
            status: status.as_u16(),
            code: code.to_owned(),
            detail: detail.into(),
            errors: Vec::new(),
            retry_after_secs: None,
        }
    }

    /// The answer to a request that presented no usable credential.
    ///
    /// Every way of failing - absent, malformed, unknown, revoked, expired -
    /// arrives here as one answer. Telling somebody probing for tokens that
    /// theirs is *revoked* rather than unknown confirms they had a real one.
    pub fn unauthenticated(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", detail)
    }

    /// Carry the wait in a `Retry-After` header as well as in the sentence.
    ///
    /// The header is the half a client library acts on without being taught to;
    /// `detail` is for the person reading a log. Both, because a caller that
    /// only reads one should still back off.
    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after_secs = Some(secs);
        self
    }
}

/// What a use case reported, as a problem.
///
/// Field rejections are intercepted before the coarse conversion, because they
/// are the one part of a `ServiceError` a caller genuinely needs: a form on
/// somebody else's phone has to know *which* field was refused. Everything else
/// goes through `Error`, which is where the logging and the stripping live.
impl From<ServiceError> for Problem {
    fn from(err: ServiceError) -> Self {
        if let ServiceError::Rejected(rejections) = &err {
            let errors: Vec<FieldProblem> = rejections
                .iter()
                .map(|rejection| FieldProblem {
                    field: rejection.field.clone(),
                    code: rejection.message.key.clone(),
                    message: rejection.message.render_builtin(),
                })
                .collect();

            let detail = errors
                .iter()
                .map(|problem| format!("{}: {}", problem.field, problem.message))
                .collect::<Vec<_>>()
                .join("; ");

            let mut problem = Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", detail);
            problem.errors = errors;
            return problem;
        }

        CoreError::from(err).into()
    }
}

impl From<CoreError> for Problem {
    fn from(err: CoreError) -> Self {
        let status =
            StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        Self::new(status, err.code(), err.to_string())
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let unauthenticated = self.code == "unauthenticated";
        let retry_after = self.retry_after_secs;

        let mut response = (status, Json(self)).into_response();

        // `Json` writes application/json; the media type is part of what makes
        // this a problem document rather than a body that happens to have
        // these fields.
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );

        // The half a client library backs off on without being taught our
        // vocabulary. Set from the field rather than by each call site, so a
        // 429 cannot be built without it.
        if let Some(secs) = retry_after
            && let Ok(value) = HeaderValue::from_str(&secs.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }

        // What a 401 owes a client: how to authenticate. Without it a correct
        // HTTP client has no way to know a credential was even wanted.
        if unauthenticated {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"phonix\""),
            );
        }

        response
    }
}

/// A short English summary per code.
///
/// A `match` rather than a field on `Error`, because these sentences are read
/// by whoever is holding the failing request - and `phonix-core` compiles into
/// the browser bundle, where nothing needs them.
fn title_for(code: &str) -> &'static str {
    match code {
        "validation" => "The request was not valid.",
        "unauthenticated" => "No usable credential was presented.",
        "forbidden" => "This key may not do that.",
        "api_disabled" => "This workspace does not have API access.",
        "not_found" => "There is nothing at that address.",
        "method_not_allowed" => "That method is not allowed here.",
        "malformed_body" => "The body could not be read as JSON.",
        "unsupported_media_type" => "That body is not in a format this accepts.",
        "conflict" => "That conflicts with something already there.",
        "rate_limited" => "Too many requests.",
        "unknown_tenant" => "That address does not belong to a workspace.",
        "tenant_inactive" => "That workspace is not active.",
        "unavailable" => "Something this depends on is unavailable.",
        _ => "The request could not be completed.",
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::identity::FieldError;
    use phonix_core::msg;

    use super::*;

    #[test]
    fn a_field_rejection_keeps_its_field_and_its_key() {
        let err = ServiceError::Rejected(vec![FieldError::new(
            "currency",
            msg!("error.currency.gone", code = "EUR"),
        )]);

        let problem = Problem::from(err);

        assert_eq!(problem.status, 422);
        assert_eq!(problem.code, "validation");
        assert_eq!(problem.errors.len(), 1);
        assert_eq!(problem.errors[0].field, "currency");
        // The key, so a client can render its own wording, and the English so
        // it does not have to.
        assert_eq!(problem.errors[0].code, "error.currency.gone");
        assert!(problem.errors[0].message.contains("EUR"));
    }

    #[test]
    fn a_key_description_never_reaches_a_caller() {
        let err = ServiceError::Crypto(
            "security.mfa.encryption_key must decode to exactly 32 bytes".to_owned(),
        );

        let problem = Problem::from(err);

        assert_eq!(problem.status, 500);
        assert_eq!(problem.code, "internal");
        assert!(!problem.detail.contains("encryption_key"));
    }

    #[test]
    fn the_two_refusals_are_different_answers() {
        // 401 says "get a credential"; 403 says "get a wider one". A client
        // cannot recover from either if they are the same status.
        assert_eq!(Problem::unauthenticated("no key").status, 401);
        assert_eq!(Problem::from(CoreError::Forbidden).status, 403);
        assert_eq!(Problem::from(CoreError::Forbidden).code, "forbidden");
    }

    #[test]
    fn the_type_is_derived_from_the_code() {
        let problem = Problem::from(CoreError::NotFound("currency".to_owned()));

        assert_eq!(problem.code, "not_found");
        assert_eq!(problem.r#type, "urn:phonix:problem:not_found");
    }
}
