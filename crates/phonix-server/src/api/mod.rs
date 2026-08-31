//! `/api/v1` - the documented, versioned surface.
//!
//! Not to be confused with the server functions, which also live under `/api`.
//! Those are this browser talking to its own server: unversioned, undocumented,
//! and free to change the day the screen above them changes. **This** is what a
//! mobile app, a customer's script and anything else outside the building talk
//! to, and none of them can be redeployed the afternoon we rename a field.
//!
//! See `docs/adr/0002-public-api.md`. The four decisions this module exists to
//! enforce:
//!
//! * **It calls `phonix-services` directly.** The same use cases the server
//!   functions call, with the same `Caller::require` inside them. Wrapping the
//!   server functions would have coupled a published surface to
//!   `ServerFnError` and to cookie authentication.
//! * **The wire types live here.** `CurrencyResource` is not `CurrencyRow`.
//!   The hand-written conversion is what makes an internal rename a compile
//!   error instead of a silent breaking change to a payload somebody's phone
//!   depends on.
//! * **Errors are RFC 9457 problem documents**, with a machine code that is not
//!   a translation key. See [`problem`].
//! * **Additive only.** Inside `v1`: new endpoints, new optional request
//!   fields, new response fields. Anything else is `v2`.
//!
//! # Why this is nested rather than merged
//!
//! [`Router::nest`] keeps this router's own fallback, so an unknown
//! `/api/v1/...` path answers a problem document rather than the application's
//! Leptos error page - which no client can parse. `merge` cannot: two
//! fallbacks in one router is a panic at startup.

use std::sync::Arc;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::routing::get;
use phonix_web::state::AppState;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_scalar::{Scalar, Servable};

pub mod auth;
pub mod currencies;
pub mod docs;
pub mod json;
pub mod paging;
pub mod problem;
pub mod scalar_bundle;
pub mod session;
pub mod users;

use problem::Problem;

/// The document, and the parts of it that are not derived from a handler.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Phonix API",
        version = "1.0.0",
        description = "The versioned surface. Every call is authenticated with an API key \
                       issued from the workspace's administration area, and acts as the \
                       person who issued it, narrowed to the scopes they chose.",
    ),
    // Every path below is relative to this. The router is nested at
    // `/api/v1` in `startup`, and without saying so here a generated client
    // would call `/currencies` and get the application's 404 page. Relative
    // rather than absolute because the host is the workspace's own subdomain,
    // which this document cannot know and every caller already does.
    servers(
        (url = "/api/v1", description = "This workspace"),
    ),
    tags(
        (name = "auth", description = "Signing a person in, and what their session may do."),
        (name = "currencies", description = "The currencies a workspace transacts in."),
        (name = "users", description = "The people with an account on this workspace."),
    ),
    modifiers(&BearerSecurity),
)]
struct ApiDoc;

/// Declares the one way in, so the documentation has an `Authorize` button and
/// a generated client knows what to send.
struct BearerSecurity;

impl Modify for BearerSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "api_key",
                SecurityScheme::Http(
                    utoipa::openapi::security::HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("phx")
                        .description(Some(
                            "An API key: `Authorization: Bearer phx_...`. Issued once and \
                             never recoverable - a lost key is replaced, not looked up.",
                        ))
                        .build(),
                ),
            );
        } else {
            // No handler declared a schema, so there are no components to hang
            // this on. Not reachable while any route exists, and a silent
            // no-op is the wrong failure - a document without its security
            // scheme generates clients that cannot authenticate.
            let mut components = utoipa::openapi::Components::new();
            components.add_security_scheme(
                "api_key",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
            openapi.components = Some(components);
        }
    }
}

/// Every route, and the document they build between them.
///
/// Separate from [`routes`] only so the tests below can hold the `OpenApi` on
/// its own. Nothing else builds a document: this is the single list, and
/// `routes!` registers a handler and its documentation together - so an
/// endpoint outside the specification is not something anybody can write by
/// accident.
fn parts() -> (Router<AppState>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        // Authentication first, because it is what a client reads first: the
        // documentation page lists tags in registration order, and "how do I
        // sign in" before "what can I read" is the order somebody integrating
        // actually needs.
        .routes(routes!(session::sign_in))
        .routes(routes!(session::answer_mfa))
        .routes(routes!(session::viewer))
        .routes(routes!(session::sign_out))
        .routes(routes!(currencies::list))
        .routes(routes!(currencies::get, currencies::save))
        .routes(routes!(users::list))
        .routes(routes!(users::get))
        .split_for_parts()
}

/// The whole of `/api/v1`, ready to be nested.
///
/// Every route is registered through `routes!`, which registers the handler and
/// its documentation together - so an endpoint that is not in the specification
/// is not something anybody can write by accident.
pub fn routes() -> Router<AppState> {
    let (router, api) = parts();

    // Serialised once, at startup, rather than per request: the document is
    // the same bytes for every caller and every workspace, and generating it
    // is not free. An unserialisable document is a programming error in a
    // `#[utoipa::path]` attribute, and the honest answer to it is an empty
    // body rather than a panic inside a request.
    let spec = Arc::new(api.to_pretty_json().unwrap_or_else(|err| {
        tracing::error!(error = %err, "the openapi document could not be serialised");
        String::new()
    }));

    router
        // The specification, and the page that renders it. Both unauthenticated
        // and both identical for every workspace: this describes the software,
        // not a tenant, and requiring a key to read the page that explains how
        // to use a key is a circle worth not drawing.
        .route(
            "/openapi.json",
            get(move || {
                let spec = Arc::clone(&spec);
                async move {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        spec.to_string(),
                    )
                }
            }),
        )
        // `custom_html`, because the crate's own template pulls the renderer
        // from a CDN at whatever version it answers with today. Ours is
        // vendored into `public/` and served from this origin - see `docs`.
        .merge(Scalar::with_url("/docs", api).custom_html(docs::template()))
        .fallback(unknown_route)
}

/// Anything under `/api/v1` that is not a route.
///
/// Its whole purpose is to be JSON. Without it these fall through to the
/// application's fallback, which renders HTML for a browser that is not there.
async fn unknown_route() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "There is no such endpoint in v1. The specification is at /api/v1/openapi.json.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> utoipa::openapi::OpenApi {
        parts().1
    }

    #[test]
    fn the_document_serialises() {
        // `routes` logs and serves an empty body when this fails, because a
        // panic inside a request is the wrong answer to it. That makes an
        // unserialisable document a silent 200 with nothing in it, and this
        // is the only thing that turns it back into a failure somebody sees.
        let json = document().to_pretty_json().expect("the document serialises");

        assert!(json.contains("\"openapi\""));
    }

    #[test]
    fn every_operation_id_is_unique() {
        // A generated client turns these into method names. Two the same is
        // either a compile error in somebody's SDK or, worse, one endpoint
        // silently shadowing the other - and `routes!` will not notice,
        // because each attribute is written on its own.
        // Walked as JSON rather than through `PathItem`'s eight typed verb
        // fields: this is the document as a client receives it, and a verb
        // added to the crate cannot quietly fall outside the check.
        let json: serde_json::Value =
            serde_json::from_str(&document().to_pretty_json().expect("the document serialises"))
                .expect("the document is json");

        let mut seen: Vec<String> = json["paths"]
            .as_object()
            .expect("the document has paths")
            .values()
            .filter_map(|item| item.as_object())
            .flat_map(|item| item.values())
            .filter_map(|operation| operation.get("operationId"))
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();

        let count = seen.len();
        seen.sort();
        seen.dedup();

        assert_eq!(count, seen.len(), "two operations share an operationId: {seen:?}");
        assert!(count >= 8, "every registered handler should carry one, got {count}");
    }

    #[test]
    fn the_paths_are_relative_to_the_nesting() {
        // The router is nested at `/api/v1` in `startup`, and the `servers`
        // entry is what carries that. A path that spelled the prefix itself
        // would send every generated client to `/api/v1/api/v1/...`.
        let document = document();

        for path in document.paths.paths.keys() {
            assert!(
                !path.starts_with("/api/"),
                "{path} repeats the prefix that `servers` already carries"
            );
        }
    }

    #[test]
    fn users_is_in_the_document_under_its_own_schema_name() {
        let json = document().to_pretty_json().expect("the document serialises");

        assert!(json.contains("\"/users\""));
        assert!(json.contains("\"/users/{id}\""));
        // `UserResource` is where the code lives; `User` is the contract. The
        // schema name is the half a client sees, and renaming the Rust type
        // must not move it.
        assert!(json.contains("\"User\""), "the schema is named for the contract");
        assert!(
            !json.contains("UserResource"),
            "an internal type name reached the published document"
        );
    }
}

