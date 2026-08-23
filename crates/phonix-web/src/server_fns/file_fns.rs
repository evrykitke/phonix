//! Files: asking how an upload went, and what to do with the result.
//!
//! Note what is *not* here. Uploading is a multipart POST to `/files/upload`
//! and downloading is a GET from `/files/{id}/content`, both plain axum routes
//! in `phonix-server`. A server function takes its argument deserialised, which
//! would mean a 25 MB file in memory before any code of ours ran - so the byte
//! ceiling would be something checked after being paid for rather than before.
//!
//! What is left is everything that is a small question with a small answer:
//! where has this upload got to, use that picture, remove that file.
//!
//! # Polling, rather than waiting
//!
//! [`upload_status`] is what an upload control asks repeatedly until the status
//! is terminal. That is the honest shape for work that happens elsewhere: the
//! request that carried the bytes returned as soon as they were safe, and the
//! job that decides whether they may be kept runs on its own.
//!
//! It is normally over in one poll. The verifier is dispatched the instant the
//! bytes land, so by the time the browser asks, a two-megabyte avatar has
//! usually already been read, hashed and moved.

use leptos::prelude::*;
use phonix_core::files::FileSummary;
use phonix_core::query::{Page, PageRequest};
use uuid::Uuid;

/// Where an upload has got to.
///
/// `None` means no such file - which is also the answer somebody gets for a
/// file that exists and is not theirs to see. Telling the two apart would be
/// telling somebody that a file exists.
#[server(name = UploadStatus, prefix = "/api", endpoint = "files/status")]
pub async fn upload_status(id: Uuid) -> Result<Option<FileSummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    match phonix_services::files::summary(&pool, &caller, id).await {
        Ok(summary) => Ok(summary),
        // A refusal and a miss are deliberately the same answer here.
        Err(phonix_services::ServiceError::Forbidden(_)) => Ok(None),
        Err(err) => Err(service_error(err)),
    }
}

/// One page of this workspace's files.
#[server(name = FilePage, prefix = "/api", endpoint = "files/page")]
pub async fn file_page(request: PageRequest) -> Result<Page<FileSummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::files::list(&pool, &caller, &request)
        .await
        .map_err(service_error)
}

/// Use an uploaded picture as the caller's profile picture.
///
/// Takes only the file id: the account acted on is always the caller's, which
/// is the simplest possible guarantee that a request cannot point somebody
/// else's profile at a picture by editing a form field.
#[server(name = SetProfilePicture, prefix = "/api", endpoint = "account/avatar")]
pub async fn set_profile_picture(file_id: Uuid) -> Result<FileSummary, ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;
    let (pool, caller) = pool_and_caller().await?;

    phonix_services::files::set_avatar(&pool, state.files(), &tenant.slug, &caller, file_id)
        .await
        .map_err(service_error)
}

/// Remove the caller's profile picture, and the file behind it.
#[server(name = RemoveProfilePicture, prefix = "/api", endpoint = "account/avatar/remove")]
pub async fn remove_profile_picture() -> Result<(), ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;
    let (pool, caller) = pool_and_caller().await?;

    phonix_services::files::clear_avatar(&pool, state.files(), &tenant.slug, &caller)
        .await
        .map_err(service_error)
}

/// Which picture the caller is using, if any.
///
/// The id rather than a URL: the address of a file is `/files/{id}/content`,
/// which is a fact about the routing table and belongs in one place - see
/// [`content_url`].
#[server(name = MyProfilePicture, prefix = "/api", endpoint = "account/avatar/current")]
pub async fn my_profile_picture() -> Result<Option<Uuid>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;
    let Some(user_id) = caller.user_id() else {
        return Ok(None);
    };

    phonix_services::files::access::avatar_of(&pool, user_id)
        .await
        .map_err(service_error)
}

/// Use an uploaded picture as the organization's logo.
///
/// Unlike a profile picture this is not restricted to whoever uploaded it: the
/// logo belongs to the workspace, so a second administrator replacing it must
/// not be refused because somebody else pressed the button last time. The
/// service requires `Settings` in its place.
#[server(name = SetOrganizationLogo, prefix = "/api", endpoint = "admin/organization/logo")]
pub async fn set_organization_logo(file_id: Uuid) -> Result<FileSummary, ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;
    let (pool, caller) = pool_and_caller().await?;

    phonix_services::files::set_logo(&pool, state.files(), &tenant.slug, &caller, file_id)
        .await
        .map_err(service_error)
}

/// Remove the organization's logo, and the file behind it.
#[server(
    name = RemoveOrganizationLogo,
    prefix = "/api",
    endpoint = "admin/organization/logo/remove"
)]
pub async fn remove_organization_logo() -> Result<(), ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;
    let (pool, caller) = pool_and_caller().await?;

    phonix_services::files::clear_logo(&pool, state.files(), &tenant.slug, &caller)
        .await
        .map_err(service_error)
}

/// Remove a file.
#[server(name = DeleteFile, prefix = "/api", endpoint = "files/delete")]
pub async fn delete_file(id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;
    let (pool, caller) = pool_and_caller().await?;

    phonix_services::files::delete_file(&pool, state.files(), &tenant.slug, &caller, id)
        .await
        .map(|_| ())
        .map_err(service_error)
}

// ---------------------------------------------------------------------------
// Addresses
//
// Compiled into both builds, because the browser needs them to build markup and
// the server needs them to render the same markup during SSR.
// ---------------------------------------------------------------------------

/// Where a stored file's bytes are.
///
/// One function rather than a format string at each call site, so the day the
/// route changes there is one place to change - and so no screen invents a
/// slightly different address that happens to work.
pub fn content_url(id: Uuid) -> String {
    format!("/files/{id}/content")
}

/// Where to POST a file for a given bucket.
pub fn upload_url(bucket: &str) -> String {
    format!("/files/upload?bucket={bucket}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_match_the_routes_the_server_registers() {
        // These two strings are the contract between this crate and
        // `phonix_server::files::routes`. A mismatch is a 404 at runtime and
        // nothing at compile time, which is exactly the kind of thing worth
        // pinning.
        let id = Uuid::from_u128(0x0199c4f2_e1a3_7b8d_9e5f_0a1b2c3d4e5f);

        assert_eq!(
            content_url(id),
            "/files/0199c4f2-e1a3-7b8d-9e5f-0a1b2c3d4e5f/content"
        );
        assert_eq!(upload_url("avatars"), "/files/upload?bucket=avatars");
    }
}
