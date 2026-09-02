//! `/api/v1/api-keys` - the credentials that open this surface, administered
//! from it.
//!
//! # The obvious objection, answered
//!
//! A key that can mint a key looks like an escalation. It is not, and the
//! reason is the same intersection ADR 0002 §3 rests on: `issue` refuses a
//! scope the *issuer* does not hold, and `authenticate` re-intersects with the
//! owner's current grants on every request. So the widest key a key can mint is
//! a key exactly as wide as itself - a copy, not a promotion - and it is issued
//! by, and acts as, the same person either way. Nothing is reachable through
//! the second key that was not already reachable through the first.
//!
//! What it buys is the thing an integrator actually asks for: rotation without
//! a browser. A deployment that has to open an administration screen to replace
//! a credential is a deployment whose credentials do not get replaced.
//!
//! # Revoking is not deleting, so it is not `DELETE`
//!
//! The row stays. It keeps its name, its scopes, its `last_used_at` and the
//! reason somebody gave for stopping it, because "which key was that, and who
//! stopped it" is a question asked long after the key is dead. `DELETE
//! /api-keys/{id}` would promise a row that was gone and answer with one that
//! is still listed, so the endpoint is named after what it does.

use axum::Json;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use phonix_core::form::Submission;
use phonix_core::identity::{ApiKeyDraft, ApiKeyIssued, ApiKeySummary, KeyState};
use phonix_services::ServiceError;
use phonix_services::identity::api_key;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::ApiCaller;
use super::json::ApiJson;
use super::paging::{ListParams, ListRequest, PageEnvelope};
use super::path::ApiPath;
use super::problem::Problem;

/// Whether a key would be accepted right now.
///
/// Computed against the server's clock rather than left to the caller's.
/// `expires_at` is in the response too, but a client comparing it against a
/// device clock that is four minutes fast will decide a live key is dead.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ApiKeyState)]
pub enum KeyStateResource {
    /// It works.
    Live,
    /// Its expiry has passed. Still worth revoking - see [`revoke`].
    Expired,
    /// Somebody stopped it. Beats expiry: a key that was stopped is stopped,
    /// whatever its dates say.
    Revoked,
}

impl From<KeyState> for KeyStateResource {
    fn from(state: KeyState) -> Self {
        match state {
            KeyState::Live => Self::Live,
            KeyState::Expired => Self::Expired,
            KeyState::Revoked => Self::Revoked,
        }
    }
}

/// One credential, without anything that could be used as one.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = ApiKey)]
pub struct ApiKeyResource {
    pub id: Uuid,
    /// What it is called on the screen that eventually revokes it.
    #[schema(example = "nightly export")]
    pub name: String,
    /// The last four characters of the token. Enough to answer "is this the
    /// one in the configuration file", useless to anybody reading it over a
    /// shoulder. There is no way to recover the rest: the database holds a
    /// SHA-256 digest and never the token.
    #[schema(example = "wxyz")]
    pub hint: String,
    /// Permission names this key is narrowed to. Its effective power is this
    /// set intersected with whatever its owner holds *now* - so a grant taken
    /// away from the owner is gone from here at the next request, with nothing
    /// on this row changing.
    #[schema(example = json!(["Pages.Administration.Settings"]))]
    pub scopes: Vec<String>,
    /// The account it acts as.
    pub owner_name: String,
    /// Whether it would be accepted right now.
    pub state: KeyStateResource,
    pub created_at: DateTime<Utc>,
    /// `null` for a key that lives until somebody stops it.
    pub expires_at: Option<DateTime<Utc>>,
    /// Written best-effort and coarsely. It exists to answer "is anything
    /// still using this", which nobody asks to the minute - so do not read it
    /// as a precise record of the last request.
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<&ApiKeySummary> for ApiKeyResource {
    fn from(key: &ApiKeySummary) -> Self {
        Self {
            id: key.id,
            name: key.name.clone(),
            hint: key.hint.clone(),
            scopes: key.scopes.clone(),
            owner_name: key.owner_name.clone(),
            state: key.state(Utc::now()).into(),
            created_at: key.created_at,
            expires_at: key.expires_at,
            last_used_at: key.last_used_at,
            revoked_at: key.revoked_at,
        }
    }
}

/// What `POST /api-keys` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = ApiKeyIssue)]
pub struct IssueKey {
    /// Read by whoever later has to decide whether to revoke it. "nightly
    /// export" is a name; "key 3" is not.
    #[schema(example = "nightly export")]
    pub name: String,
    /// Permission names - the strings `GET /permissions` lists. **Empty is
    /// meaningful and often right**: such a key reaches everything ungated and
    /// nothing else, which is the whole of what a read-only integration needs.
    ///
    /// A name that is not in the tree and a name the issuer does not hold are
    /// two different refusals, and both arrive against this field.
    #[schema(example = json!(["Pages.Administration.Users"]))]
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Days from now, or `null` for a key that lives until it is revoked.
    ///
    /// Days rather than an instant because that is the decision being made -
    /// "this contractor is here for three months" - and because a date is
    /// something a caller can accidentally set in the past.
    #[schema(example = 90)]
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

/// A key, and the one response its token ever appears in.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = ApiKeyIssued)]
pub struct IssuedKeyResource {
    pub key: ApiKeyResource,
    /// `phx_...`. **Shown once.** It is not stored anywhere it can be read
    /// back from, so a caller that does not keep this value replaces the key
    /// rather than looking it up.
    #[schema(example = "phx_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")]
    pub secret: String,
}

impl From<ApiKeyIssued> for IssuedKeyResource {
    fn from(issued: ApiKeyIssued) -> Self {
        Self {
            key: ApiKeyResource::from(&issued.key),
            secret: issued.secret,
        }
    }
}

/// What `POST /api-keys/{id}/revoke` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = ApiKeyRevoke)]
pub struct RevokeKey {
    /// Why, for whoever reads the row afterwards. Stored on the key and in the
    /// change trail.
    #[schema(example = "rotated")]
    #[serde(default)]
    pub reason: Option<String>,
}

/// Every key this workspace has issued, live or not.
///
/// Revoked keys are listed, because the history is the point - see the module
/// note. Narrow with `filter[revoked]`.
///
/// Searches the key's name and its owner's display name. Sorts by `created_at`
/// (the default, newest first), `name`, `last_used_at` or `expires_at`.
/// Requires `Pages.Administration.ApiKeys`.
///
/// Paged in SQL rather than in memory, unlike users and roles: a workspace has
/// few keys today and an integrator's will not - every phone build, every
/// customer script and every retired credential is a row that is kept.
#[utoipa::path(
    get,
    path = "/api-keys",
    tag = "api-keys",
    operation_id = "listApiKeys",
    params(
        ListParams,
        ("filter[revoked]" = Option<String>, Query,
            description = "`live` for the keys that still work, `revoked` for the ones \
                           somebody stopped. Anything else narrows nothing.",
            example = "live"),
    ),
    responses(
        (status = 200, description = "One page of keys", body = PageEnvelope<ApiKeyResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry ApiKeys", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<ApiKeyResource>>, Problem> {
    // The `PageRequest` goes down rather than the whole list coming up: this
    // is the shape every resource here will have once its use case takes one,
    // and the wire contract is identical either way.
    let page = api_key::list(&caller.pool, &caller.caller, &request).await?;

    Ok(Json(PageEnvelope::new(
        page.map(|key| ApiKeyResource::from(&key)),
    )))
}

/// Issue a key, and hand back its token once.
///
/// The key acts as the caller: there is no "issue for somebody else"
/// parameter, because that would be a way to obtain a credential for an
/// account whose permissions one does not have.
///
/// Requires `Pages.Administration.ApiKeys.Create`, **and** every scope named
/// must be one the caller already holds - which for a request authenticated by
/// a key means the intersection it is already narrowed to. See the module note
/// on why that is not an escalation.
///
/// `201`, with a `Location`. The response body is the only place the token
/// appears; it is not recoverable from any later read.
#[utoipa::path(
    post,
    path = "/api-keys",
    tag = "api-keys",
    operation_id = "issueApiKey",
    request_body = IssueKey,
    responses(
        (status = 201, description = "The key, with its token", body = IssuedKeyResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry ApiKeys.Create", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "A blank name, or a scope that is unknown or not held", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn issue(
    caller: ApiCaller,
    ApiJson(body): ApiJson<IssueKey>,
) -> Result<
    (
        StatusCode,
        [(axum::http::HeaderName, String); 1],
        Json<IssuedKeyResource>,
    ),
    Problem,
> {
    let draft = ApiKeyDraft {
        name: body.name,
        scopes: body.scopes,
        expires_in_days: body.expires_in_days,
    };

    let issued = api_key::issue(&caller.pool, &caller.caller, draft).await?;

    match issued {
        Submission::Saved(issued) => {
            // The audit trail records who issued it and with what; this
            // records which credential the request arrived on, which is the
            // question asked when a key turns out to have minted another.
            tracing::info!(
                key = ?caller.key_id,
                issued = %issued.key.id,
                scopes = issued.key.scopes.len(),
                "api key issued through the api"
            );

            let location = format!("/api/v1/api-keys/{}", issued.key.id);

            Ok((
                StatusCode::CREATED,
                [(axum::http::header::LOCATION, location)],
                Json(IssuedKeyResource::from(issued)),
            ))
        }
        Submission::Rejected(errors) => Err(Problem::from(ServiceError::Rejected(errors))),
    }
}

/// Stop a key.
///
/// Immediate: the next request presenting it fails the lookup, because
/// liveness is decided in the same statement that finds the row.
///
/// Not idempotent, deliberately. Revoking a key that is already revoked
/// answers `422` rather than `204`, because the alternative is reporting
/// success for an act that did nothing - and "was it already stopped, and
/// when" is exactly what somebody pressing this twice wants to know.
///
/// An **expired** key can still be revoked, and often should be: it stops
/// being a credential that a clock correction could revive.
///
/// The body is optional. A bare `POST` with no `Content-Type` revokes without
/// a reason, because "stop this key" is a complete request and answering it
/// with a 415 for the want of a `{}` would be a confusing way to say so.
///
/// Requires `Pages.Administration.ApiKeys.Revoke`.
#[utoipa::path(
    post,
    path = "/api-keys/{id}/revoke",
    tag = "api-keys",
    operation_id = "revokeApiKey",
    params(("id" = Uuid, Path, description = "The key's id")),
    request_body(content = RevokeKey, description = "Optional. Omit the body entirely to revoke without a reason."),
    responses(
        (status = 204, description = "Stopped. The row stays, listed as revoked."),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry ApiKeys.Revoke", body = Problem),
        (status = 415, description = "A body was sent, but not as JSON", body = Problem),
        (status = 422, description = "No such key, or it was already revoked", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn revoke(
    caller: ApiCaller,
    ApiPath(id): ApiPath<Uuid>,
    body: Option<ApiJson<RevokeKey>>,
) -> Result<StatusCode, Problem> {
    let reason = body
        .and_then(|ApiJson(body)| body.reason)
        .map(|reason| reason.trim().to_owned())
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| "revoked through the api".to_owned());

    api_key::revoke(&caller.pool, &caller.caller, id, &reason).await?;

    tracing::info!(key = ?caller.key_id, revoked = %id, "api key revoked through the api");

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn summary() -> ApiKeySummary {
        ApiKeySummary {
            id: Uuid::nil(),
            name: "nightly export".to_owned(),
            hint: "wxyz".to_owned(),
            scopes: vec!["Pages.Administration.Settings".to_owned()],
            owner_name: "Ada Lovelace".to_owned(),
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn the_resource_carries_no_way_to_authenticate() {
        // The one thing this file must never get wrong. Asserted on the
        // serialised bytes rather than on the struct, because what this
        // guards against is a field added later to make something easier.
        let json = serde_json::to_string(&ApiKeyResource::from(&summary())).expect("it serialises");

        assert!(json.contains("wxyz"), "the hint is what identifies a key");
        assert!(!json.contains("token_hash"));
        assert!(!json.contains("\"secret\""));
        assert!(!json.contains("phx_"));
    }

    #[test]
    fn the_state_is_decided_here_rather_than_by_the_caller() {
        let live = ApiKeyResource::from(&summary());
        let expired = ApiKeyResource::from(&ApiKeySummary {
            expires_at: Some(Utc::now() - Duration::days(1)),
            ..summary()
        });
        let revoked = ApiKeyResource::from(&ApiKeySummary {
            // Expired *and* revoked. Revocation wins, because a key somebody
            // stopped is stopped whatever its dates say.
            expires_at: Some(Utc::now() - Duration::days(1)),
            revoked_at: Some(Utc::now() - Duration::hours(1)),
            ..summary()
        });

        assert!(matches!(live.state, KeyStateResource::Live));
        assert!(matches!(expired.state, KeyStateResource::Expired));
        assert!(matches!(revoked.state, KeyStateResource::Revoked));
    }

    #[test]
    fn the_issued_response_carries_the_token_exactly_once() {
        let issued = IssuedKeyResource::from(ApiKeyIssued {
            key: summary(),
            secret: "phx_abcdef".to_owned(),
        });

        assert_eq!(issued.secret, "phx_abcdef");
        // And the nested key, which is what every later read answers with,
        // still has nowhere to put it.
        let json = serde_json::to_string(&issued.key).expect("it serialises");
        assert!(!json.contains("phx_"));
    }

    #[test]
    fn an_issue_body_may_name_no_scopes_at_all() {
        // Not a mistake to guard against: a key with no scopes reaches what is
        // ungated and nothing else, which is a real and useful shape.
        let body: IssueKey =
            serde_json::from_str(r#"{"name":"read-only probe"}"#).expect("the body parses");

        assert!(body.scopes.is_empty());
        assert_eq!(body.expires_in_days, None);
    }
}
