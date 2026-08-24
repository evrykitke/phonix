//! Creating a workspace.
//!
//! Runs on the bare domain, where there is no tenant: the whole point of these
//! two calls is that one does not exist yet.
//!
//! # Why the tenant check is here and not only on the page
//!
//! A workspace is created on the host somebody first arrived at. From inside
//! `acme.example.com` there is nothing sensible to create - a second workspace
//! reached from the first one's sign-in screen is almost always somebody who
//! meant to sign in, and it costs a catalog row and a whole Postgres database
//! before anybody notices.
//!
//! Hiding the link is worth doing and is not the control: an endpoint is
//! reachable whether or not a page links to it. The check that matters is the
//! one below.

use leptos::prelude::*;
use phonix_core::identity::{SignupInput, SignupResult, SlugAvailability};

/// Whether a workspace address can be taken.
///
/// Called as the user types the organization name, so it reports only free or
/// taken - never *why* a name is unavailable. Saying "that workspace is
/// suspended" would let an anonymous caller enumerate the customer list.
#[server(name = CheckWorkspaceAddress, prefix = "/api", endpoint = "workspace-address")]
pub async fn check_workspace_address(candidate: String) -> Result<SlugAvailability, ServerFnError> {
    use phonix_core::TenantSlug;
    use phonix_core::identity::slug_from_organization_name;

    use crate::state::app_state;

    let state = app_state()?;

    // An empty box is not a rejection, it is an unanswered question.
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return Ok(SlugAvailability {
            slug: String::new(),
            available: false,
            reason: None,
            suggestion: None,
        });
    }

    let Ok(slug) = TenantSlug::parse(trimmed) else {
        // Not a valid address at all. The suggestion is what the same text
        // looks like once it is one, which is usually what the user meant.
        return Ok(SlugAvailability {
            slug: trimmed.to_owned(),
            available: false,
            reason: Some("Use letters, numbers and hyphens.".to_owned()),
            suggestion: slug_from_organization_name(trimmed),
        });
    };

    let available = phonix_services::workspace::onboarding::slug_is_available(
        &state.catalog,
        &state.config,
        &slug,
    )
    .await
    .map_err(|err| ServerFnError::new(phonix_core::Error::from(err)))?;

    Ok(SlugAvailability {
        slug: slug.to_string(),
        available,
        reason: (!available).then(|| "That address is taken.".to_owned()),
        suggestion: (!available).then(|| format!("{slug}-hq")),
    })
}

/// Create a workspace, its database, and its owner account.
///
/// One call for a three-screen wizard: a half-created workspace - a catalog row
/// with no owner, or an owner with no database - is a state nobody wants to
/// reason about later. Everything the wizard collected arrives together and
/// either all of it works or none of it does.
#[server(name = CreateWorkspace, prefix = "/api", endpoint = "create-workspace")]
pub async fn create_workspace(input: SignupInput) -> Result<SignupResult, ServerFnError> {
    use phonix_core::identity::SignupOutcome;
    use secrecy::ExposeSecret;

    use crate::state::app_state;

    use crate::state::optional_tenant;

    let state = app_state()?;

    // Before anything else, including the config switch: this is about where
    // the request arrived, and it is true whether signup is open or shut.
    if optional_tenant().await.is_some() {
        return Ok(SignupResult::NotHere);
    }

    if !state.config.security.signup.enabled {
        return Ok(SignupResult::Closed);
    }

    // Re-validated server-side even though the wizard already did it. The
    // client's checks exist to give fast feedback; they are not a control, and
    // this endpoint is reachable without them.
    let valid = match input.validate() {
        Ok(valid) => valid,
        Err(errors) => return Ok(SignupResult::Rejected(errors)),
    };

    if state.config.security.signup.is_blocked_domain(&valid.email) {
        return Ok(SignupResult::Rejected(vec![
            phonix_core::identity::FieldError::new(
                "email",
                phonix_core::msg!("signup.blocked_domain"),
            ),
        ]));
    }

    let client_ip = client_ip().await;

    let workspace = phonix_services::onboard_workspace(
        &state.catalog,
        &state.config,
        &state.hasher,
        &valid,
        client_ip.as_deref(),
    )
    .await
    .map_err(|err| ServerFnError::new(phonix_core::Error::from(err)))?;

    // The browser is sent to the workspace's own host carrying a single-use
    // token, which it trades there for a session cookie. Cookies are host-only,
    // so this form - running on the bare domain - cannot set one for a
    // subdomain it does not control. See `phonix_server::auth::handoff`.
    let workspace_url = state
        .config
        .server
        .tenant_origin(workspace.tenant.slug.as_str());
    let handoff_url = format!(
        "{workspace_url}/auth/handoff?token={}",
        urlencode(workspace.handoff_token.expose_secret())
    );

    Ok(SignupResult::Created(Box::new(SignupOutcome {
        workspace_slug: workspace.tenant.slug.clone(),
        organization_name: workspace.tenant.display_name.clone(),
        workspace_url,
        handoff_url,
    })))
}

/// The caller's address, for the audit trail.
#[cfg(feature = "ssr")]
async fn client_ip() -> Option<String> {
    let headers: http::HeaderMap = leptos_axum::extract().await.ok()?;

    // `x-forwarded-for` first, since anything behind a proxy sees the proxy in
    // the socket address. Only the first entry is read: the rest are appended
    // by intermediaries and the client controls what it sent.
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_owned())
}

/// Percent-encode a token for a query string.
///
/// The token is URL-safe base64 by construction, so this is belt and braces -
/// but a token that ever gains a `+` or `/` would otherwise arrive mangled and
/// fail to redeem for reasons nobody would enjoy debugging.
#[cfg(feature = "ssr")]
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
