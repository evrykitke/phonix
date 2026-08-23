//! Accepting bytes: the part that happens inside the request.
//!
//! Two steps, deliberately separate, with the streaming in between them:
//!
//! 1. [`authorise_upload`] - may this caller put a file in this bucket, and
//!    where would it go? Pure: no I/O, no bytes, nothing to undo.
//! 2. …the transport writes the bytes to the ticket's quarantine key…
//! 3. [`record_upload`] - the bytes are down; record them and queue the work.
//!
//! Splitting it that way is what lets the permission check and the byte ceiling
//! be decided **before** a single byte is accepted. A design that took the
//! bytes first and asked afterwards would let anybody who can reach the
//! endpoint make this server write 25 MB to disk, whether or not they are
//! allowed to upload anything at all.

use phonix_core::files::{BucketPolicy, bucket as bucket_policy, sanitize_file_name};
use phonix_core::{Rejection, TenantSlug, permissions};
use phonix_db::PgPool;
use phonix_db::files::{self as files_db, ReceivedUpload};
use phonix_db::sqlx::PgExecutor;
use phonix_storage::{FileStorage, ObjectStat, QUARANTINE, StorageKey, naming};
use uuid::Uuid;

use crate::caller::Caller;
use crate::error::{ServiceError, ServiceResult};

/// Permission to write bytes, and the place to write them.
///
/// Held by the transport across the streaming, so the id under which the
/// quarantine object was written is the id the row is later created with -
/// bytes and row cannot end up naming different things.
#[derive(Debug, Clone)]
pub struct UploadTicket {
    /// The row's id, decided here because the object's name is derived from it.
    pub file_id: Uuid,
    pub bucket: &'static BucketPolicy,
    pub quarantine_key: StorageKey,
    /// The most bytes that may be written. The bucket's own limit, not the
    /// transport's - the transport's is a coarser ceiling above this one.
    pub limit: u64,
}

/// May this caller upload into this bucket, and where would it go?
///
/// Pure. No database, no filesystem, nothing to unwind - which is what makes it
/// safe to call before accepting any bytes, and what the whole shape of this
/// module is arranged around.
///
/// The bucket arrives from the request, so an unrecognised name is a refusal
/// rather than a default. Falling back to a bucket would mean somebody aiming
/// at `avatars` and mistyping it got the attachment limit instead.
pub fn authorise_upload(
    caller: &Caller,
    tenant: &TenantSlug,
    bucket_name: &str,
) -> ServiceResult<UploadTicket> {
    let bucket = bucket_policy(bucket_name).ok_or_else(|| {
        // `Message::literal` because `Rejection::message` still builds
        // English out of `FileType::label` and `BucketPolicy::label`. Keying
        // this one sentence without keying those labels would produce a French
        // sentence with two English nouns in it, so the three move together or
        // not at all. See `phonix_core::i18n::Message::literal`.
        ServiceError::rejected(
            "bucket",
            phonix_core::Message::literal(
                Rejection::UnknownBucket {
                    requested: bucket_name.to_owned(),
                }
                .message(),
            ),
        )
    })?;

    // `None` means any signed-in person, which is right for the things people
    // upload about themselves. It does *not* mean anybody.
    //
    // It takes both of these, because neither alone says "a signed-in person":
    //
    //   * `require(PAGES)` rules out a half-authenticated session. Such a
    //     session *has* a user id - it is the session an attacker holding a
    //     stolen password holds - so asking only for an id would let it
    //     through. `Caller::can` returns false until the second factor is
    //     satisfied, which is what refuses it here. `Pages` is the right
    //     permission to name: reaching any screen at all requires it, so every
    //     ordinary account has it, and it is the same one that guards *reading*
    //     an avatar in `access::may_read`.
    //
    //   * `acting_user` rules out the system caller, which passes every
    //     permission check by design and is therefore invisible to the line
    //     above. There is no such thing as a profile picture belonging to
    //     nobody, and an upload with no owner is one nobody could later attach
    //     to an account.
    match bucket.upload_permission {
        Some(permission) => caller.require(permission)?,
        None => {
            caller.require(permissions::PAGES)?;
            crate::caller::acting_user(caller)?;
        }
    }

    // v7 rather than v4: the id is the object's name and the row's primary key,
    // and a time-ordered one keeps both the index and the directory listing in
    // the order things happened.
    let file_id = Uuid::now_v7();

    let quarantine_key = StorageKey::new(
        tenant,
        &[QUARANTINE.to_owned(), naming::quarantine_name(file_id)],
    )
    .map_err(|err| ServiceError::Storage(err.into()))?;

    Ok(UploadTicket {
        file_id,
        bucket,
        quarantine_key,
        limit: bucket.max_bytes,
    })
}

/// Record bytes that are safely in quarantine, and queue the work on them.
///
/// `stat` comes from the writer that just finished, so `byte_size` is what was
/// actually written rather than anything the request claimed. Nothing here
/// looks at the bytes: what the file *is* is the job's question, and answering
/// it in the request would be doing the work this design exists to defer.
pub async fn record_upload<'e, E>(
    executor: E,
    ticket: &UploadTicket,
    original_name: &str,
    declared_content_type: Option<&str>,
    stat: &ObjectStat,
    caller: &Caller,
) -> ServiceResult<files_db::FileRow>
where
    E: PgExecutor<'e>,
{
    // Sanitised once, here, so everything downstream - the list, the download
    // header, the audit line - is handed a name that is already safe. See
    // `phonix_core::files::name` for what "safe" is protecting.
    let original_name = sanitize_file_name(original_name);

    let row = files_db::record_received(
        executor,
        ReceivedUpload {
            id: ticket.file_id,
            bucket: ticket.bucket.name,
            original_name: &original_name,
            // Kept so a rejection can quote what the browser claimed. It
            // decides nothing; see `phonix_core::files::catalog`.
            declared_content_type,
            byte_size: stat.byte_size,
            quarantine_key: ticket.quarantine_key.as_str(),
            uploaded_by: caller.user_id(),
        },
    )
    .await?;

    tracing::info!(
        file_id = %row.id,
        bucket = ticket.bucket.name,
        bytes = stat.byte_size,
        "upload received"
    );

    Ok(row)
}

/// Throw away bytes that were written but never recorded.
///
/// The one place the two halves can come apart: the object is written, and then
/// the insert fails. Without this the bytes would sit in quarantine with
/// nothing pointing at them until the sweeper noticed - which it would, but
/// hours later and after logging a mystery.
///
/// Infallible on purpose. It runs on a path that is already failing for a
/// reason the caller cares about more, and a second error here would replace a
/// useful message with a useless one.
pub async fn discard(storage: &dyn FileStorage, key: &StorageKey) {
    if let Err(err) = storage.delete(key).await {
        tracing::warn!(
            key = %key,
            error = %err,
            "could not remove an upload that was never recorded"
        );
    }
}

/// Dispatch a specific upload to a worker, now.
///
/// The fast path, and only the fast path: it claims the row and hands it back
/// so the caller can run the job immediately, so an avatar is usually verified
/// before the browser has finished asking about it.
///
/// `Ok(None)` means somebody else got there first, which is an ordinary outcome
/// rather than an error - the immediate dispatch and the periodic sweep race
/// on purpose, and `SKIP LOCKED` is what makes losing that race harmless.
pub async fn claim_for_verification(
    pool: &PgPool,
    file_id: Uuid,
    claim_timeout_secs: u64,
) -> ServiceResult<Option<files_db::FileRow>> {
    Ok(files_db::claim_one(pool, file_id, claim_timeout_secs).await?)
}

#[cfg(test)]
mod tests {
    use phonix_core::PermissionSet;
    use phonix_core::identity::{AuthUser, UserStatus};
    use phonix_core::permissions as names;

    use super::*;

    fn tenant() -> TenantSlug {
        TenantSlug::parse("acme").expect("a valid slug")
    }

    fn user_with(permissions: &[&str], mfa_satisfied: bool) -> Caller {
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

    #[test]
    fn a_ticket_points_into_quarantine_and_nowhere_else() {
        let caller = user_with(&[names::FILES, names::FILES_UPLOAD], true);
        let ticket = authorise_upload(&caller, &tenant(), "attachments").unwrap();

        assert!(ticket.quarantine_key.is_quarantine());
        assert_eq!(ticket.quarantine_key.tenant(), "acme");
        assert_eq!(ticket.limit, ticket.bucket.max_bytes);

        // The object's name is the row's id, which is what keeps the bytes and
        // the row from ever naming different things.
        assert!(
            ticket
                .quarantine_key
                .file_name()
                .starts_with(&ticket.file_id.simple().to_string())
        );
    }

    #[test]
    fn a_bucket_that_names_a_permission_enforces_it() {
        let without = user_with(&[names::FILES], true);
        assert!(authorise_upload(&without, &tenant(), "attachments").is_err());

        let with = user_with(&[names::FILES, names::FILES_UPLOAD], true);
        assert!(authorise_upload(&with, &tenant(), "attachments").is_ok());
    }

    #[test]
    fn a_bucket_with_no_permission_still_needs_a_person() {
        // Changing your own picture is not an administrative act, so the avatar
        // bucket names no permission of its own. Every ordinary account holds
        // `Pages` - it is what reaching any screen at all requires - so this is
        // a person, not a privilege.
        let anyone = user_with(&[names::PAGES], true);
        assert!(authorise_upload(&anyone, &tenant(), "avatars").is_ok());

        // What it must not become is "anybody at all". The system caller passes
        // every permission check by design, and there is no such thing as a
        // profile picture belonging to nobody.
        let system = Caller::system("a scheduled sweep");
        assert!(authorise_upload(&system, &tenant(), "avatars").is_err());
    }

    #[test]
    fn a_half_authenticated_session_cannot_upload_anything() {
        // The session an attacker holding a stolen password has: the password
        // is proven and the second factor is not. It holds no permissions, so
        // the attachment bucket refuses it - and the avatar bucket, which asks
        // for none, must refuse it too.
        let half = user_with(&[names::PAGES, names::FILES, names::FILES_UPLOAD], false);

        assert!(authorise_upload(&half, &tenant(), "attachments").is_err());
        assert!(authorise_upload(&half, &tenant(), "avatars").is_err());
    }

    #[test]
    fn an_unknown_bucket_is_refused_rather_than_defaulted() {
        let caller = user_with(&[names::FILES, names::FILES_UPLOAD], true);

        for nonsense in ["", "Attachments", "../etc", "attachment", "_quarantine"] {
            assert!(
                authorise_upload(&caller, &tenant(), nonsense).is_err(),
                "{nonsense:?} was accepted as a bucket"
            );
        }
    }

    #[test]
    fn each_bucket_carries_its_own_ceiling_rather_than_a_shared_one() {
        let caller = user_with(&[names::FILES, names::FILES_UPLOAD], true);

        let avatar = authorise_upload(&caller, &tenant(), "avatars").unwrap();
        let attachment = authorise_upload(&caller, &tenant(), "attachments").unwrap();

        // The ceiling is known before any bytes are accepted, which is the
        // whole reason this function does no I/O.
        assert_eq!(avatar.limit, 2 * 1024 * 1024);
        assert!(attachment.limit > avatar.limit);
    }

    #[test]
    fn two_tickets_never_name_the_same_object() {
        let caller = user_with(&[names::PAGES], true);

        let first = authorise_upload(&caller, &tenant(), "avatars").unwrap();
        let second = authorise_upload(&caller, &tenant(), "avatars").unwrap();

        assert_ne!(first.file_id, second.file_id);
        assert_ne!(first.quarantine_key, second.quarantine_key);
    }
}
