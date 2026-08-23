//! Server-side application state (SSR builds only).

use std::sync::Arc;

use axum::extract::FromRef;
use leptos::prelude::*;
use phonix_cache::Cache;
use phonix_config::AppConfig;
use phonix_core::{Error as CoreError, TenantSummary};
use phonix_db::{Catalog, TenantRegistry};
use phonix_messaging::Publisher;
use phonix_services::{Caller, Hasher, SecretVault, Security};

/// Everything a request handler or server function may need.
///
/// Cheap to clone: every field is either an `Arc` or an internally reference-
/// counted handle (pools, connection managers, channels).
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub catalog: Catalog,
    pub tenants: TenantRegistry,
    pub cache: Cache,
    /// `None` when `rabbitmq.enabled = false`.
    pub publisher: Option<Publisher>,
    /// Argon2id at the configured cost. Built once: the parameters are fixed
    /// for the process, and a per-request one would re-derive them every time.
    pub hasher: Arc<Hasher>,
    /// Opens and seals TOTP secrets. Holds the key, so it is built at startup
    /// where a bad key kills the process rather than one user's enrolment.
    pub vault: Arc<SecretVault>,
    /// Where uploaded bytes live.
    ///
    /// A trait object, so nothing above this line can name a filesystem path -
    /// which is what makes swapping in an object store a change to one match
    /// arm in `startup` rather than a search of the whole application.
    pub storage: Arc<dyn phonix_storage::FileStorage>,
    /// How stored files are laid out beneath the tenant. Chosen once from
    /// configuration; see `phonix_storage::naming` for what the choice costs.
    pub naming: Arc<dyn phonix_storage::NamingStrategy>,
    pub leptos_options: LeptosOptions,
}

impl AppState {
    /// The bundle every identity use case takes.
    pub fn security(&self) -> Security<'_> {
        Security {
            config: &self.config.security,
            hasher: &self.hasher,
            vault: &self.vault,
        }
    }

    /// The bundle every file use case takes.
    ///
    /// Borrowed from the `Arc`s rather than cloning them: a use case runs
    /// inside one request and has no reason to outlive the state it was called
    /// from, and handing out owned handles would let one.
    pub fn files(&self) -> phonix_services::Files<'_> {
        phonix_services::Files {
            storage: self.storage.as_ref(),
            naming: self.naming.as_ref(),
        }
    }
}

// Lets axum hand `LeptosOptions` to the Leptos route handlers while the router
// carries the richer `AppState`.
impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

/// Read [`AppState`] out of the Leptos context.
///
/// `phonix-server` provides it per request via `leptos_routes_with_context`.
pub fn app_state() -> Result<AppState, ServerFnError> {
    use_context::<AppState>().ok_or_else(|| {
        // Reaching this means the context was not provided during router setup,
        // which is a wiring bug rather than a bad request.
        ServerFnError::new(
            "AppState missing from the Leptos context; check leptos_routes_with_context",
        )
    })
}

/// Read the tenant that middleware attached to the current request.
pub async fn tenant_from_request() -> Result<TenantSummary, CoreError> {
    use axum::Extension;
    use leptos_axum::extract;

    // The extension is inserted by `phonix_server::middleware::resolve_tenant`,
    // which runs before any Leptos handler.
    let Extension(tenant): Extension<TenantSummary> = extract()
        .await
        .map_err(|_| CoreError::MissingTenantContext)?;

    Ok(tenant)
}

/// The tenant, or `None` on the bare domain.
///
/// Signup and the workspace picker both run on a host with no tenant, so the
/// absence of one is an ordinary state there rather than an error.
pub async fn optional_tenant() -> Option<TenantSummary> {
    tenant_from_request().await.ok()
}

/// The current tenant's connection pool.
pub async fn tenant_pool() -> Result<phonix_db::PgPool, ServerFnError> {
    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;

    let handle = state
        .tenants
        .resolve(&tenant.slug)
        .await
        .map_err(|err| ServerFnError::new(CoreError::from(err)))?;

    Ok(handle.pool.clone())
}

/// The session cookie presented with this request, if any.
pub async fn session_token() -> Option<secrecy::SecretString> {
    use crate::server::cookie;

    let state = app_state().ok()?;
    let tenant = tenant_from_request().await.ok()?;
    let headers: http::HeaderMap = leptos_axum::extract().await.ok()?;

    let raw = headers.get(http::header::COOKIE)?.to_str().ok()?;
    let name = state
        .config
        .security
        .session
        .cookie_name_for(tenant.slug.as_str());

    cookie::read(raw, &name).map(secrecy::SecretString::from)
}

/// Who is making this request.
///
/// Returns `None` when there is no session, an expired one, or an account that
/// has since been suspended. Every server function that changes something takes
/// the [`Caller`] this produces and states its permission - see
/// `phonix_services::caller`.
pub async fn current_caller() -> Result<Option<Caller>, ServerFnError> {
    let Some(token) = session_token().await else {
        return Ok(None);
    };

    let state = app_state()?;
    let pool = tenant_pool().await?;

    let auth_user = phonix_services::authenticate_session(&pool, &token, &state.config.security)
        .await
        .map_err(|err| ServerFnError::new(CoreError::from(err)))?;

    Ok(auth_user.map(Caller::user))
}

/// [`current_caller`], refusing anonymous requests.
///
/// For server functions where "not signed in" is a bug in the caller rather
/// than a state to render.
pub async fn require_caller() -> Result<Caller, ServerFnError> {
    current_caller()
        .await?
        .ok_or_else(|| ServerFnError::new(CoreError::Unauthenticated))
}

/// The tenant's pool and the signed-in caller, which most screens need together.
///
/// One helper rather than two calls because the two are always wanted at once
/// and resolving the caller already opened the pool.
pub async fn pool_and_caller() -> Result<(phonix_db::PgPool, Caller), ServerFnError> {
    let pool = tenant_pool().await?;
    let caller = require_caller().await?;
    Ok((pool, caller))
}

/// The state and tenant that inviting somebody needs.
///
/// One helper because the two are always wanted together, and because building
/// `Inviting` at each call site would mean each one deciding for itself where
/// the workspace slug comes from - and the slug is what makes the invitation
/// link point at a host that can set the session cookie.
pub async fn inviting_context() -> Result<(AppState, TenantSummary), ServerFnError> {
    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;

    Ok((state, tenant))
}

/// The security policy this workspace has decided for itself.
///
/// Read per request rather than cached: an administrator tightening the MFA
/// policy expects it to take effect now, and this is one indexed row.
pub async fn workspace_settings() -> Result<phonix_core::WorkspaceSecuritySettings, ServerFnError> {
    let pool = tenant_pool().await?;
    phonix_services::workspace::settings::load(&pool)
        .await
        .map_err(service_error)
}

/// Collapse a service error into the coarse one a browser may see.
///
/// The full cause - a constraint name, a key description, a SQL fragment - is
/// logged inside the service layer and does not cross this boundary. Field
/// rejections survive, because a form needs them and they contain only what the
/// caller typed.
pub fn service_error(err: phonix_services::ServiceError) -> ServerFnError {
    ServerFnError::new(CoreError::from(err))
}

/// Set a `Set-Cookie` header on the response Leptos is building.
pub fn set_response_cookie(value: String) -> Result<(), ServerFnError> {
    let response = use_context::<leptos_axum::ResponseOptions>()
        .ok_or_else(|| ServerFnError::new("ResponseOptions missing from the Leptos context"))?;

    let header = http::HeaderValue::from_str(&value)
        .map_err(|_| ServerFnError::new("could not build the session cookie"))?;

    // `append`, not `insert`: signing in to a second workspace in the same
    // browser must not evict the first one's cookie.
    response.append_header(http::header::SET_COOKIE, header);
    Ok(())
}
