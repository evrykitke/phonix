//! Which workspace this request belongs to.

use leptos::prelude::*;
use phonix_core::TenantSummary;

/// The tenant that owns the current request.
///
/// The tenant is resolved from the request host by middleware in
/// `phonix-server` and attached as an axum extension; this reads it back out.
#[server(name = CurrentTenant, prefix = "/api", endpoint = "tenant")]
pub async fn current_tenant() -> Result<TenantSummary, ServerFnError> {
    use crate::state::tenant_from_request;

    // `ServerFnError::new` rather than the `ServerError` variant: the enum is
    // generic over a custom error type, and the variant alone cannot infer it.
    tenant_from_request().await.map_err(ServerFnError::new)
}

/// Where "Continue with Google" goes, if this deployment offers it.
///
/// `None` when Google sign-in is switched off or half-configured, which is what
/// the sign-in screen renders no button for.
///
/// # Why the server builds this URL and not the page
///
/// It is **cross-host**, and the page has no way to work out the other host.
/// Google will not redirect to a wildcard, so the whole flow runs on the one
/// host named by `security.google.redirect_uri` - and that URI's origin is the
/// only place in the configuration where that host is written down.
/// `server.host` is a bind address, and `tenancy.base_domain` is the tenancy
/// root, which in production is one label above the signup host and belongs to
/// something else entirely.
///
/// Public, like `current_tenant`: it returns a URL built from configuration and
/// the slug already in the address bar, and reveals nothing that host does not
/// reveal by answering at all.
#[server(name = GoogleSignInUrl, prefix = "/api", endpoint = "google-sign-in-url")]
pub async fn google_sign_in_url() -> Result<Option<String>, ServerFnError> {
    use crate::state::{app_state, optional_tenant};

    let state = app_state()?;
    let config = &state.config.security.google;

    if !config.is_usable() {
        return Ok(None);
    }

    // Only on a workspace host. On the bare domain there is no workspace to
    // sign in to, and the flow needs one before it starts - see the note in
    // `phonix_server::google` on why the slug may not come from the browser.
    let Some(tenant) = optional_tenant().await else {
        return Ok(None);
    };

    let Some(origin) = config.auth_origin() else {
        // Configured but unparseable. Logged rather than surfaced: the button
        // simply does not appear, and whoever set it up needs to see why.
        tracing::warn!(
            redirect_uri = %config.redirect_uri,
            "security.google.redirect_uri is not an absolute URL, so no sign-in button can be \
             built from it",
        );
        return Ok(None);
    };

    Ok(Some(format!(
        "{origin}/auth/google/start?workspace={}",
        tenant.slug.as_str()
    )))
}

/// Counts rows in the current tenant's `users` table.
///
/// Exists to prove the whole path end to end: host -> tenant -> that tenant's
/// database -> cache. Replace it with real queries.
#[server(name = TenantUserCount, prefix = "/api", endpoint = "tenant-user-count")]
pub async fn tenant_user_count() -> Result<i64, ServerFnError> {
    use phonix_core::permissions;

    use crate::state::{app_state, require_caller, tenant_from_request};

    // Gated even though it only returns a number. An endpoint is reachable
    // whether or not a page links to it, so "who may call this" is decided
    // here rather than by which menu item rendered.
    let caller = require_caller().await?;
    caller
        .require(permissions::DASHBOARD)
        .map_err(|err| ServerFnError::new(phonix_core::Error::from(err)))?;

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;

    let handle = state
        .tenants
        .resolve(&tenant.slug)
        .await
        .map_err(ServerFnError::new)?;

    // Read-through cache: the count is cheap, but this exercises the cache path
    // that expensive queries will use.
    let cache = state.cache.for_tenant(&tenant.slug);

    cache
        .get_or_insert_with::<i64, _, _, ServerFnError>("users:count", || async {
            let (count,): (i64,) = phonix_db::sqlx::query_as("SELECT count(*) FROM users")
                .fetch_one(&handle.pool)
                .await
                .map_err(|err| {
                    // The real cause is logged; the client gets a fixed string
                    // so no SQL or connection detail leaves the server.
                    tracing::error!(error = %err, "tenant user count query failed");
                    ServerFnError::new("database unavailable")
                })?;
            Ok(count)
        })
        .await
}
