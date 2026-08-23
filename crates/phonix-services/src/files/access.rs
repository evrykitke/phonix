//! Reading a stored file, removing one, and using one as a profile picture.
//!
//! # Who may read what
//!
//! Two rules, and the second is the interesting one:
//!
//! * **Your own file is yours.** Whoever uploaded it can always read it back,
//!   with no permission at all. Otherwise somebody could attach a document and
//!   then be unable to open the thing they just attached.
//! * **A picture everybody is shown is public within the workspace.** Avatars
//!   are rendered beside names in every list on the site, and the organization
//!   logo sits on everything the workspace issues, so requiring `Pages.Files`
//!   to see either would mean a screen full of broken images for ordinary
//!   users. The bucket is what makes this safe rather than a hole: nothing
//!   scriptable can be stored in `avatars` or `logos` at all, so "everybody can
//!   see it" is a statement about pictures and not about arbitrary content.
//!
//! Everything else needs `Pages.Files`.
//!
//! # Only stored files can be read
//!
//! A row is a file from the moment somebody starts uploading, and it is not
//! *readable* until the job has decided it is safe. [`open_for_download`]
//! refuses anything that is not `stored` - which means quarantined bytes have
//! no route out of this application at all, not merely no route anybody has
//! thought to build.

use phonix_core::files::{FileSummary, UploadStatus};
use phonix_core::identity::UserId;
use phonix_core::query::{Page, PageRequest};
use phonix_core::{TenantSlug, permissions};
use phonix_db::PgPool;
use phonix_db::files::{self as files_db, FileRow};
use phonix_db::identity::user as user_db;
use phonix_db::organization as organization_db;
use phonix_db::sqlx::PgExecutor;
use phonix_storage::StorageKey;
use tokio::io::AsyncRead;
use uuid::Uuid;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};

use super::Files;
use phonix_core::msg;

/// The buckets whose contents everybody signed in may look at.
///
/// An avatar sits beside a name in every list; a logo sits in the page header
/// and on every document. Requiring a permission to see either would mean a
/// screen of broken images. Both buckets refuse anything that is not a picture
/// and anything that could carry a script, which is what makes that safe.
const AVATARS: &str = "avatars";
const LOGOS: &str = "logos";

/// One page of the file list.
pub async fn list(
    pool: &PgPool,
    caller: &Caller,
    request: &PageRequest,
) -> ServiceResult<Page<FileSummary>> {
    caller.require(permissions::FILES)?;
    Ok(files_db::page(pool, request).await?)
}

/// One file, as a screen shows it.
///
/// What the upload page polls while a job is running, so it answers for rows in
/// every state - including `received`, which is the state the caller is waiting
/// to see change.
pub async fn summary<'e, E>(
    executor: E,
    caller: &Caller,
    id: Uuid,
) -> ServiceResult<Option<FileSummary>>
where
    E: PgExecutor<'e>,
{
    let Some(row) = files_db::load(executor, id).await? else {
        return Ok(None);
    };

    may_read(caller, &row)?;
    Ok(Some(row.to_summary(None)))
}

/// Open a stored file's bytes.
///
/// Returns the row as well, because the caller has to build a response from it:
/// the content type is the **detected** one, never the declared one, and the
/// name offered for saving is the original.
pub async fn open_for_download(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    caller: &Caller,
    id: Uuid,
) -> ServiceResult<(FileRow, Box<dyn AsyncRead + Send + Unpin>)> {
    let row = files_db::load(pool, id)
        .await?
        .ok_or(ServiceError::NotFound("file"))?;

    may_read(caller, &row)?;

    // Not merely "we have not built a route to quarantine": a row that is not
    // stored has no readable object by construction, and saying so here is what
    // makes that true for every future caller as well.
    if row.status != UploadStatus::Stored {
        return Err(ServiceError::NotFound("file"));
    }

    let key = stored_key(tenant, &row)?;
    let reader = files_ctx.storage.open(&key).await?;

    Ok((row, reader))
}

/// Remove a file: the bytes first, then the row.
///
/// That order, always. A row deleted before its object leaves bytes nothing
/// points at, which only the quarantine sweeper would ever find - and it does
/// not look in buckets.
pub async fn delete_file(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    caller: &Caller,
    id: Uuid,
) -> ServiceResult<bool> {
    let Some(row) = files_db::load(pool, id).await? else {
        // Already gone is the outcome the caller asked for.
        return Ok(false);
    };

    // Deleting is not the same act as uploading, so it has its own permission -
    // except on your own file, which you may always remove.
    if !is_own(caller, &row) {
        caller.require(permissions::FILES_DELETE)?;
    }

    remove_object(files_ctx, tenant, &row).await;

    let removed = files_db::delete(pool, row.id).await?;

    tracing::info!(file_id = %row.id, bucket = row.bucket, "file deleted");
    Ok(removed)
}

/// Use an uploaded picture as the caller's profile picture.
///
/// Three things have to be true, and none of them is a permission:
///
/// 1. The upload has finished and was accepted. A picture still being checked
///    is not one to put beside somebody's name.
/// 2. It is in the avatar bucket - which is what guarantees it is a picture and
///    that it cannot carry a script.
/// 3. The caller uploaded it. Otherwise anybody could point their own account
///    at somebody else's file and keep it alive after they deleted it.
///
/// The picture it replaces is deleted, so changing a photograph ten times
/// leaves one file rather than ten.
pub async fn set_avatar(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    caller: &Caller,
    file_id: Uuid,
) -> ServiceResult<FileSummary> {
    let user_id = acting_user(caller)?;

    let row = files_db::load(pool, file_id)
        .await?
        .ok_or(ServiceError::NotFound("file"))?;

    if row.status != UploadStatus::Stored {
        return Err(ServiceError::rejected(
            "avatar",
            msg!("error.avatar.pending"),
        ));
    }

    if row.bucket != AVATARS {
        return Err(ServiceError::rejected(
            "avatar",
            msg!("error.avatar.wrong_bucket"),
        ));
    }

    if row.uploaded_by != Some(user_id) {
        // Deliberately the same refusal a missing file gets. Telling somebody
        // that a file exists but is not theirs is telling them a file exists.
        return Err(ServiceError::NotFound("file"));
    }

    let previous = user_db::set_avatar(pool, user_id, file_id).await?;
    discard_previous(pool, files_ctx, tenant, previous, file_id).await;

    tracing::info!(user_id = %user_id, file_id = %file_id, "profile picture set");
    Ok(row.to_summary(None))
}

/// Remove the caller's profile picture, and the file behind it.
pub async fn clear_avatar(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    caller: &Caller,
) -> ServiceResult<()> {
    let user_id = acting_user(caller)?;

    let previous = user_db::clear_avatar(pool, user_id).await?;
    discard_previous(pool, files_ctx, tenant, previous, Uuid::nil()).await;

    tracing::info!(user_id = %user_id, "profile picture removed");
    Ok(())
}

/// Which uploaded picture an account is using.
pub async fn avatar_of<'e, E>(executor: E, user_id: UserId) -> ServiceResult<Option<Uuid>>
where
    E: PgExecutor<'e>,
{
    Ok(user_db::avatar_file(executor, user_id).await?)
}

/// Use an uploaded picture as the organization's logo.
///
/// The same three checks as [`set_avatar`], with one difference that matters:
/// ownership is *not* one of them. A logo belongs to the workspace rather than
/// to whoever happened to upload it, so a second administrator replacing it
/// must not be refused because somebody else pressed the button last time.
/// `Settings` is what stands in its place, and it is a stronger gate than
/// ownership was.
pub async fn set_logo(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    caller: &Caller,
    file_id: Uuid,
) -> ServiceResult<FileSummary> {
    caller.require(permissions::SETTINGS)?;
    let changed_by = acting_user(caller)?;

    let row = files_db::load(pool, file_id)
        .await?
        .ok_or(ServiceError::NotFound("file"))?;

    if row.status != UploadStatus::Stored {
        return Err(ServiceError::rejected("logo", msg!("error.logo.pending")));
    }

    // What guarantees it is a picture and that it cannot carry a script - the
    // logo goes on every document this workspace issues, so an SVG here would
    // be script travelling under the organization's own name.
    if row.bucket != LOGOS {
        return Err(ServiceError::rejected(
            "logo",
            msg!("error.logo.wrong_bucket"),
        ));
    }

    let previous = organization_db::set_logo(pool, file_id, Some(changed_by)).await?;

    // Read before `discard_previous` deletes it: the audit records file names
    // rather than ids, and afterwards there is no row left to read one from.
    let displaced = name_of(pool, previous).await;
    discard_previous(pool, files_ctx, tenant, previous, file_id).await;

    record_logo_change(pool, caller, displaced, Some(row.original_name.clone())).await;

    tracing::info!(file_id = %file_id, %changed_by, "organization logo set");
    Ok(row.to_summary(None))
}

/// Remove the organization's logo, and the file behind it.
pub async fn clear_logo(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    caller: &Caller,
) -> ServiceResult<()> {
    caller.require(permissions::SETTINGS)?;
    let changed_by = acting_user(caller)?;

    let previous = organization_db::clear_logo(pool, Some(changed_by)).await?;

    let displaced = name_of(pool, previous).await;
    discard_previous(pool, files_ctx, tenant, previous, Uuid::nil()).await;

    record_logo_change(pool, caller, displaced, None).await;

    tracing::info!(%changed_by, "organization logo removed");
    Ok(())
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Whether this caller may read this file. See the module docs.
fn may_read(caller: &Caller, row: &FileRow) -> ServiceResult<()> {
    if is_own(caller, row) {
        return Ok(());
    }

    // See the note on AVATARS and LOGOS: both are displayed to everybody, and
    // both are restricted to pictures that cannot carry a script.
    if row.bucket == AVATARS || row.bucket == LOGOS {
        // Still a signed-in person, and still not a half-authenticated
        // session: `require` is what enforces both.
        return caller.require(permissions::PAGES);
    }

    caller.require(permissions::FILES)
}

/// Whether the caller is the person who uploaded this.
///
/// A half-authenticated session has an id and holds nothing, so ownership alone
/// must not be a way past the checks. `Caller::can` returns false until the
/// second factor is satisfied, which is what `may_read` and `delete_file` both
/// lean on after this returns true.
fn is_own(caller: &Caller, row: &FileRow) -> bool {
    match caller.auth_user() {
        Some(user) => user.is_fully_authenticated() && row.uploaded_by == Some(user.id),
        // The system caller is nobody, and nobody uploaded anything.
        None => false,
    }
}

/// The stored object this row names, checked against the tenant reading it.
///
/// `parse_for` rather than `parse`: a key from a row is older than a request
/// rather than more trustworthy than one, and a key naming another tenant's
/// area is refused here instead of opened.
fn stored_key(tenant: &TenantSlug, row: &FileRow) -> ServiceResult<StorageKey> {
    let raw = row
        .storage_key
        .as_deref()
        .ok_or(ServiceError::NotFound("file"))?;

    StorageKey::parse_for(tenant, raw).map_err(|err| ServiceError::Storage(err.into()))
}

/// Delete the object a row names, if it has one.
///
/// Infallible: it runs as part of removing something, and a failure to tidy up
/// must not turn a successful deletion into an error the caller has to handle.
async fn remove_object(files_ctx: Files<'_>, tenant: &TenantSlug, row: &FileRow) {
    // Either place - a file may be deleted while it is still in quarantine.
    let raw = row.storage_key.as_deref().or(row.quarantine_key.as_deref());

    let Some(raw) = raw else {
        return;
    };

    match StorageKey::parse_for(tenant, raw) {
        Ok(key) => {
            if let Err(err) = files_ctx.storage.delete(&key).await {
                tracing::warn!(key = %key, error = %err, "could not remove a file's bytes");
            }
        }
        Err(err) => {
            tracing::error!(
                file_id = %row.id,
                error = %err,
                "a row names an object this tenant may not touch; leaving it alone"
            );
        }
    }
}

/// Remove the picture an account was using before.
///
/// Best effort, and after the account has already been pointed elsewhere: the
/// profile change is the thing that mattered, and a leftover file is a tidiness
/// problem rather than a correctness one.
/// The original name of a stored file, for an audit line.
///
/// Best-effort: a name that cannot be read becomes `None`, which draws as "not
/// set". Failing a logo change because its predecessor's row could not be read
/// would be refusing the thing somebody asked for over the record of it.
async fn name_of(pool: &PgPool, id: Option<Uuid>) -> Option<String> {
    let id = id?;

    match files_db::load(pool, id).await {
        Ok(Some(row)) => Some(row.original_name),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(file_id = %id, error = %err, "could not name the logo being replaced");
            None
        }
    }
}

/// Record a logo change as `{from, to}`, by name.
///
/// Names rather than ids on purpose: a diff of two UUIDs is a diff nobody can
/// act on, and `acme-mark.png` replacing `(not set)` is the whole story. Both
/// sides are `Option`, so setting the first logo reads as an addition and
/// removing one reads as a removal - which is what `ChangeKind` derives from.
async fn record_logo_change(
    pool: &PgPool,
    caller: &Caller,
    from: Option<String>,
    to: Option<String>,
) {
    // Against the organization, so a logo change sits on the same history as
    // the legal name and the address - they are one record to the person
    // reading it, however many tables they live in here.
    audit::changed_json(
        pool,
        caller,
        Target::singleton(kinds::ORGANIZATION),
        serde_json::json!({ "logo": from }),
        serde_json::json!({ "logo": to }),
    )
    .await;
}

async fn discard_previous(
    pool: &PgPool,
    files_ctx: Files<'_>,
    tenant: &TenantSlug,
    previous: Option<Uuid>,
    replacement: Uuid,
) {
    let Some(previous) = previous.filter(|id| *id != replacement) else {
        return;
    };

    let row = match files_db::load(pool, previous).await {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(file_id = %previous, error = %err, "could not read the picture being replaced");
            return;
        }
    };

    remove_object(files_ctx, tenant, &row).await;

    if let Err(err) = files_db::delete(pool, previous).await {
        tracing::warn!(file_id = %previous, error = %err, "could not remove the picture being replaced");
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use phonix_core::PermissionSet;
    use phonix_core::identity::{AuthUser, UserStatus};
    use phonix_core::permissions as names;

    use super::*;

    fn caller_with(permissions: &[&str], mfa_satisfied: bool) -> Caller {
        let mut set = PermissionSet::new();
        for permission in permissions {
            set.grant(permission);
        }

        Caller::user(AuthUser {
            id: Uuid::from_u128(7),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["User".into()],
            permissions: set,
            is_owner: false,
            status: UserStatus::Active,
            mfa_enabled: true,
            mfa_satisfied,
            email_verified: true,
        })
    }

    fn row(bucket: &str, uploaded_by: Option<UserId>) -> FileRow {
        FileRow {
            id: Uuid::from_u128(1),
            status: UploadStatus::Stored,
            bucket: bucket.to_owned(),
            original_name: "photo.png".into(),
            stored_name: Some("0199.png".into()),
            declared_content_type: Some("image/png".into()),
            content_type: Some("image/png".into()),
            category: Some(phonix_core::FileCategory::Image),
            byte_size: 1024,
            checksum_sha256: Some("0f".repeat(32)),
            storage_key: Some("acme/avatars/2026/08/0199.png".into()),
            quarantine_key: None,
            rejection: None,
            attempts: 1,
            claimed_at: None,
            last_error: None,
            uploaded_by,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            verified_at: Some(Utc::now()),
        }
    }

    #[test]
    fn your_own_file_needs_no_permission() {
        let caller = caller_with(&[], true);
        let mine = row("attachments", Some(Uuid::from_u128(7)));

        // Otherwise somebody could attach a document and then be unable to open
        // the thing they had just attached.
        assert!(may_read(&caller, &mine).is_ok());
    }

    #[test]
    fn somebody_elses_attachment_needs_the_permission() {
        let theirs = row("attachments", Some(Uuid::from_u128(99)));

        assert!(may_read(&caller_with(&[], true), &theirs).is_err());
        assert!(may_read(&caller_with(&[names::PAGES, names::FILES], true), &theirs).is_ok());
    }

    #[test]
    fn a_profile_picture_is_visible_to_everybody_signed_in() {
        // Avatars are rendered beside names everywhere, so gating them behind
        // Pages.Files would be a directory of broken images. The bucket policy
        // - pictures only, nothing scriptable - is what makes it safe.
        let theirs = row("avatars", Some(Uuid::from_u128(99)));
        let ordinary = caller_with(&[names::PAGES], true);

        assert!(may_read(&ordinary, &theirs).is_ok());
    }

    #[test]
    fn a_half_authenticated_session_reads_nothing_at_all() {
        // The session an attacker with a stolen password holds. Not even its
        // own files, and not even an avatar.
        let half = caller_with(&[names::PAGES, names::FILES], false);

        assert!(may_read(&half, &row("avatars", Some(Uuid::from_u128(99)))).is_err());
        assert!(may_read(&half, &row("attachments", Some(Uuid::from_u128(7)))).is_err());
        // Ownership must not be a way round the second factor.
        assert!(!is_own(
            &half,
            &row("attachments", Some(Uuid::from_u128(7)))
        ));
    }

    #[test]
    fn the_system_caller_owns_nothing() {
        // It passes every permission check by design, so ownership has to be
        // the one thing it cannot claim - there is no file belonging to nobody.
        let system = Caller::system("a scheduled sweep");

        assert!(!is_own(&system, &row("avatars", None)));
        assert!(!is_own(
            &system,
            &row("attachments", Some(Uuid::from_u128(7)))
        ));
    }

    #[test]
    fn a_key_from_another_tenants_row_is_refused_rather_than_opened() {
        let acme = TenantSlug::parse("acme").unwrap();

        let mut hostile = row("attachments", None);
        hostile.storage_key = Some("globex/attachments/2026/08/secret.pdf".into());

        assert!(stored_key(&acme, &hostile).is_err());
        assert!(stored_key(&acme, &row("avatars", None)).is_ok());
    }

    #[test]
    fn a_row_with_no_object_is_not_found_rather_than_a_panic() {
        let acme = TenantSlug::parse("acme").unwrap();

        let mut pending = row("attachments", None);
        pending.storage_key = None;

        assert!(matches!(
            stored_key(&acme, &pending),
            Err(ServiceError::NotFound("file"))
        ));
    }
}
