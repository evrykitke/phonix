//! What this workspace requires of its people.
//!
//! Reading is ungated: every user needs the password policy to fill in a form
//! and the MFA policy to be told why they are being sent to enrolment. Writing
//! is `Administration.Settings`, checked in the service layer where the write
//! happens rather than here.

use leptos::prelude::*;
use phonix_core::WorkspaceSecuritySettings;
use phonix_core::identity::FieldError;
use serde::{Deserialize, Serialize};

/// This workspace's current password and MFA policy.
#[server(name = WorkspaceSettings, prefix = "/api", endpoint = "settings")]
pub async fn workspace_settings() -> Result<WorkspaceSecuritySettings, ServerFnError> {
    crate::state::workspace_settings().await
}

/// How the settings form came back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SettingsSaved {
    /// Stored. Carries the settings as they now stand, so the form re-seeds
    /// from the server's answer rather than from what it hoped it sent.
    Saved(Box<WorkspaceSecuritySettings>),
    /// One or more fields were refused. Both policies are validated together,
    /// so an administrator who got two things wrong is told both at once.
    Rejected(Vec<FieldError>),
}

/// Save a policy an administrator submitted.
#[server(name = SaveWorkspaceSettings, prefix = "/api", endpoint = "settings/save")]
pub async fn save_workspace_settings(
    settings: WorkspaceSecuritySettings,
) -> Result<SettingsSaved, ServerFnError> {
    use phonix_services::ServiceError;

    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    match phonix_services::workspace::settings::save(&pool, &caller, &settings).await {
        Ok(()) => Ok(SettingsSaved::Saved(Box::new(settings))),
        // A refused field is an answer the form can render. A refused
        // *permission* is not - that is an error, and stays one.
        Err(ServiceError::Rejected(errors)) => Ok(SettingsSaved::Rejected(errors)),
        Err(err) => Err(service_error(err)),
    }
}
