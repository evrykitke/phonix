//! Which apps this workspace has, and switching them on and off.
//!
//! Thin, like every other file here. What is worth knowing is what these do
//! *not* carry: no name, no summary, no icon, no dependency list. All of that
//! is `phonix_core::apps::CATALOG`, compiled into the wasm bundle the browser
//! already downloaded, so sending it back over the wire would be sending a
//! constant. What crosses is the part that differs per workspace.

use leptos::prelude::*;
use phonix_core::apps::{AppState, Installed, UninstallOutcome};

/// Every app in this release, and what this workspace has done about each.
#[server(name = AppCatalog, prefix = "/api", endpoint = "apps")]
pub async fn app_catalog() -> Result<Vec<AppState>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::workspace::apps::catalog(&pool, &caller)
        .await
        .map_err(service_error)
}

/// The ids this workspace has switched on.
///
/// Ungated, and the launcher in the top bar is why: it draws for everybody, and
/// a permission to read your own menu would mean only administrators could see
/// theirs.
#[server(name = EnabledApps, prefix = "/api", endpoint = "apps/enabled")]
pub async fn enabled_apps() -> Result<Vec<String>, ServerFnError> {
    use crate::state::{service_error, tenant_pool};

    let pool = tenant_pool().await?;

    phonix_services::workspace::apps::enabled_ids(&pool)
        .await
        .map_err(service_error)
}

/// Switch an app on, with whatever it depends on.
///
/// Returns as soon as the write commits, which is fast - the install dialog's
/// eight seconds are the browser's own, and deliberately not this call's. See
/// `phonix_services::workspace::apps`.
#[server(name = InstallApp, prefix = "/api", endpoint = "apps/install")]
pub async fn install_app(app_id: String) -> Result<Installed, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::workspace::apps::install(&pool, &caller, &app_id)
        .await
        .map_err(service_error)
}

/// Switch an app off. Its data stays exactly where it is.
///
/// Comes back as an [`UninstallOutcome`] rather than a `Result`, because two of
/// its three answers are things the store renders beside the button rather than
/// faults: core cannot be switched off, and something still needs this one.
#[server(name = UninstallApp, prefix = "/api", endpoint = "apps/uninstall")]
pub async fn uninstall_app(app_id: String) -> Result<UninstallOutcome, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::workspace::apps::uninstall(&pool, &caller, &app_id)
        .await
        .map_err(service_error)
}
