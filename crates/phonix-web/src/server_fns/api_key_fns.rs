//! The credentials that let other software reach this workspace.
//!
//! Four endpoints for one screen: list the keys, mint one, stop one, and turn
//! the whole surface on or off for this workspace.
//!
//! # Nothing here decides anything
//!
//! Which is worth saying twice for this file in particular, because it is about
//! credentials. `phonix_services::identity::api_key` mints the token, refuses a
//! scope the issuer does not hold, and records the audit row; the licence is
//! `Settings`, checked in the same place. These functions parse, call one use
//! case, and map the result - and the public API answers to the same use cases,
//! so a check written here rather than there would protect one adapter and not
//! the other.
//!
//! # The token exists for exactly one response
//!
//! [`issue_api_key`] is the only endpoint in the application that returns a
//! usable credential, and it returns it once. Nothing stores it, no other
//! endpoint can produce it, and [`list_api_keys`] carries a four-character hint
//! instead - so a screen that redraws after a refresh cannot show it again.

use leptos::prelude::*;
use phonix_core::form::Submission;
use phonix_core::identity::{ApiKeyDraft, ApiKeyIssued, ApiKeySummary};
use phonix_core::query::{Page, PageRequest};
use uuid::Uuid;

/// One page of this workspace's keys, revoked ones included.
///
/// Paged on the server: the list keeps every key a workspace has ever had,
/// because a revoked credential is a fact about who could once reach it.
#[server(name = ListApiKeys, prefix = "/api", endpoint = "admin/api-keys")]
pub async fn list_api_keys(request: PageRequest) -> Result<Page<ApiKeySummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::api_key::list(&pool, &caller, &request)
        .await
        .map_err(service_error)
}

/// Mint a key for the person asking.
///
/// Returns a [`Submission`], so "you do not hold that permission yourself"
/// arrives at the scopes field rather than as a sentence at the top of the
/// form.
#[server(name = IssueApiKey, prefix = "/api", endpoint = "admin/api-keys/issue")]
pub async fn issue_api_key(
    draft: ApiKeyDraft,
) -> Result<Submission<ApiKeyIssued>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::api_key::issue(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// Stop a key.
///
/// Immediate, and not undoable: the row is kept for its history, but no token
/// can be recovered from it, so a key revoked by mistake is replaced rather
/// than restored.
#[server(name = RevokeApiKey, prefix = "/api", endpoint = "admin/api-keys/revoke")]
pub async fn revoke_api_key(id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::api_key::revoke(&pool, &caller, id, "revoked by an administrator")
        .await
        .map_err(service_error)
}

/// Whether the API answers this workspace at all.
///
/// Ungated to read, because the screen that manages keys has to be able to tell
/// whoever is looking at it that every call is currently being refused.
#[server(name = ApiAccess, prefix = "/api", endpoint = "admin/api-keys/access")]
pub async fn api_access() -> Result<bool, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, _) = pool_and_caller().await?;

    phonix_services::identity::api_key::api_enabled(&pool)
        .await
        .map_err(service_error)
}

/// Turn the API on or off for this workspace.
///
/// `Settings`, not one of the API-key permissions: this is what the workspace
/// *has* rather than what somebody inside it may do. See
/// `docs/adr/0002-public-api.md`.
#[server(name = SetApiAccess, prefix = "/api", endpoint = "admin/api-keys/access/set")]
pub async fn set_api_access(enabled: bool) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::api_key::set_api_enabled(&pool, &caller, enabled)
        .await
        .map_err(service_error)
}
