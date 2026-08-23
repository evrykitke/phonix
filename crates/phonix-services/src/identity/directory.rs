//! Who is in this workspace.

use phonix_core::authorization::RoleSummary;
use phonix_core::form::{Submission, rejected};
use phonix_core::identity::{UserCard, UserEdit, UserId, UserListing};
use phonix_core::permissions;
use phonix_db::authorization::role as role_store;
use phonix_db::identity::user as store;
use phonix_db::sqlx::PgPool;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};
use phonix_core::msg;

/// Who somebody is, for a card beside their name.
///
/// Gated on `Users`, not on `AuditLogs`. The card is directory data, and what
/// it is being read *next to* does not change what it is: somebody who may read
/// the trail but not the directory may see the address the trail recorded, and
/// not the person behind it. The screen hides the control to match - see
/// `crate::components::user_link` in `phonix-web` - but this is the refusal.
///
/// `None` for an account that has been deleted, rather than an error. A trail
/// row still names them by the address it stored, and "there is no profile to
/// show" is an ordinary answer to a question about somebody who has left.
pub async fn card(
    pool: &PgPool,
    caller: &Caller,
    user_id: UserId,
) -> ServiceResult<Option<UserCard>> {
    caller.require(permissions::USERS)?;

    let Some(row) = store::card(pool, user_id).await? else {
        return Ok(None);
    };

    Ok(Some(UserCard {
        id: row.id,
        display_name: row.display_name,
        email: row.email,
        status: row.status,
        is_owner: row.is_owner,
        roles: row.roles,
        avatar_file_id: row.avatar_file_id,
        // Nothing sets either yet. Named here rather than left out so that the
        // day something does, this is the only line that changes.
        department: None,
        job_title: None,
        last_login_at: row.last_login_at,
        created_at: row.created_at,
    }))
}

/// Every account, with its roles.
///
/// Unpaged, deliberately. A workspace with enough people for that to matter
/// needs a search on the server and a cursor, and building either before there
/// is a workspace that needs it would be guessing at the shape of both.
pub async fn list(pool: &PgPool, caller: &Caller) -> ServiceResult<Vec<UserListing>> {
    caller.require(permissions::USERS)?;
    Ok(store::listings(pool).await?)
}

/// One account, for the screens that open from the list.
pub async fn find(pool: &PgPool, caller: &Caller, user_id: UserId) -> ServiceResult<UserListing> {
    caller.require(permissions::USERS)?;

    store::listings(pool)
        .await?
        .into_iter()
        .find(|user| user.id == user_id)
        .ok_or_else(|| ServiceError::rejected("user", msg!("error.user.gone")))
}

// ---------------------------------------------------------------------------
// Editing one account
// ---------------------------------------------------------------------------

/// The editable part of one account, as the form should open on it.
///
/// Gated on `Users` rather than `Users.Edit`, matching the permission editor:
/// being able to read what an account holds is part of being able to see the
/// list at all, and an administrator who cannot edit gets a form of disabled
/// controls rather than a refusal. [`update_user`] is what requires `Edit`.
pub async fn edit(pool: &PgPool, caller: &Caller, user_id: UserId) -> ServiceResult<UserEdit> {
    caller.require(permissions::USERS)?;
    load(pool, user_id).await
}

/// The roles an account may be put into, for the edit form's choices.
///
/// Gated on `Users`, not on `Roles`. Administering people and administering
/// roles are separate jobs with separate permissions, and somebody who may edit
/// an account has to be able to see the names of the roles they are choosing
/// between - otherwise the control renders empty and the form silently drops
/// every role the account already held.
///
/// Reading the *names* is not reading what each one grants; that stays behind
/// `Roles`.
pub async fn assignable_roles(pool: &PgPool, caller: &Caller) -> ServiceResult<Vec<RoleSummary>> {
    caller.require(permissions::USERS)?;
    Ok(role_store::list(pool).await?)
}

/// Store a changed account.
///
/// Returns a [`Submission`], not a bare `UserEdit`. A form that fails
/// validation is the expected path through a form rather than a failure - see
/// [`phonix_core::form`] - so a rejected name comes back as
/// `Ok(Submission::Rejected(..))` with the field names intact, and `Err` is
/// kept for the caller not being permitted or the database being unwell.
///
/// # What is re-read rather than echoed
///
/// The value returned is loaded again from storage after the write, never the
/// draft that came in. The two differ whenever the database declined something
/// the browser asked for - the owner keeps `Admin` however the role list was
/// submitted - and echoing the draft would leave the form showing a change that
/// did not happen.
pub async fn update_user(
    pool: &PgPool,
    caller: &Caller,
    draft: UserEdit,
) -> ServiceResult<Submission<UserEdit>> {
    caller.require(permissions::USERS_EDIT)?;
    let changed_by = acting_user(caller)?;

    // The same rules the browser applied, applied again. The browser's check is
    // a courtesy; this one is the control.
    if let Some(rejection) = rejected(draft.validate()) {
        return Ok(rejection);
    }

    let Some(account) = store::find_by_id(pool, draft.id).await? else {
        return Ok(Submission::rejected("user", msg!("error.user.gone")));
    };

    let before = load(pool, draft.id).await?;
    let after = normalise(draft);

    // Every role must exist. Without this a typo - or a role deleted while the
    // form was open - silently stores nothing, because the insert matches no
    // row and reports no error.
    let known: Vec<String> = role_store::list(pool)
        .await?
        .into_iter()
        .map(|r| r.name)
        .collect();
    let unknown: Vec<&str> = after
        .roles
        .iter()
        .filter(|name| !known.iter().any(|k| k.eq_ignore_ascii_case(name)))
        .map(String::as_str)
        .collect();

    if !unknown.is_empty() {
        return Ok(Submission::rejected(
            "roles",
            msg!("error.roles.unknown", names = unknown.join(", ")),
        ));
    }

    // Said here rather than left to the store's `OwnerProtected`, which would
    // reach the form as "the workspace owner is protected" against no field at
    // all. The owner is also the one account that must stay able to administer
    // the workspace, which is why this is refused rather than ignored.
    if account.is_owner && after.status != before.status {
        return Ok(Submission::rejected(
            "status",
            msg!("error.owner.status_locked"),
        ));
    }

    if after == before {
        // Nothing to write and nothing to record. An audit trail full of
        // "changed: nothing" entries is one nobody reads.
        return Ok(Submission::Saved(before));
    }

    // Roles are where most permissions come from, so changing them is a
    // permission change however it is spelled. The form disables the control
    // for a caller without this, which hides it and refuses nothing - this is
    // the check that refuses. Asked only when the roles actually differ, so
    // renaming somebody does not require it.
    if after.roles != before.roles {
        caller.require(permissions::USERS_CHANGE_PERMISSIONS)?;
    }

    // Names and status share a transaction; roles cannot join it, because
    // `set_user_roles` opens its own to do its delete-then-insert. Ordered so
    // that the write which can refuse - the status of a protected account -
    // happens before the one that cannot be rolled back with it.
    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;

    if after.first_name != before.first_name || after.last_name != before.last_name {
        store::set_names(
            &mut *tx,
            after.id,
            &after.first_name,
            &after.last_name,
            &after.display_name(),
        )
        .await?;
    }

    if after.status != before.status {
        store::set_status(&mut *tx, after.id, after.status).await?;
    }

    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    if after.roles != before.roles {
        role_store::set_user_roles(pool, after.id, &after.roles, Some(changed_by)).await?;
    }

    // Read back rather than assumed: see the note on this function.
    let stored = load(pool, after.id).await?;

    // On the account's own history rather than on the editor's: "what was done
    // to this person, and by whom" is read from their page. The email is a fact
    // beside the diff, not a field of it - the editor cannot change it here, so
    // showing it as a before and an after would claim otherwise.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::USER, after.id)
            .named(stored.display_name())
            .fact("email", &account.email),
        &before,
        &stored,
    )
    .await;

    tracing::info!(
        user_id = %after.id,
        %changed_by,
        "account edited",
    );

    Ok(Submission::Saved(stored))
}

/// One account as the form holds it: the editable fields and nothing else.
async fn load(pool: &PgPool, user_id: UserId) -> ServiceResult<UserEdit> {
    let Some(account) = store::find_by_id(pool, user_id).await? else {
        return Err(ServiceError::rejected("user", msg!("error.user.gone")));
    };

    let mut roles = role_store::names_for_user(pool, user_id).await?;
    roles.sort_unstable();

    Ok(UserEdit {
        id: account.id,
        first_name: account.first_name,
        last_name: account.last_name,
        status: account.status,
        roles,
    })
}

/// A draft as it will be compared and stored.
///
/// Trimmed and sorted so that "did anything change" is a question about the
/// account rather than about whitespace or the order the tick boxes were
/// clicked in - otherwise every save is a write and every write is an audit
/// entry.
fn normalise(draft: UserEdit) -> UserEdit {
    let mut roles: Vec<String> = draft
        .roles
        .into_iter()
        .map(|role| role.trim().to_owned())
        .filter(|r| !r.is_empty())
        .collect();
    roles.sort_unstable();
    roles.dedup();

    UserEdit {
        id: draft.id,
        first_name: draft.first_name.trim().to_owned(),
        last_name: draft.last_name.trim().to_owned(),
        status: draft.status,
        roles,
    }
}

/// One page of the security trail.
///
/// Gated on `AuditLogs`, which is a separate permission from `Users` on
/// purpose: reading who signed in, from where, and when somebody's permissions
/// changed is a different kind of access from seeing a list of names.
///
/// Paged in SQL rather than fetched whole, unlike [`list`]. The trail grows for
/// as long as the workspace is used and nothing ever deletes from it, so there
/// is no size at which "fetch all of it" stops being wrong - only a date at
/// which it becomes obvious.
pub async fn audit_trail(
    pool: &PgPool,
    caller: &Caller,
    request: &phonix_core::query::PageRequest,
) -> ServiceResult<phonix_core::query::Page<phonix_core::identity::AuditEvent>> {
    use phonix_db::identity::audit;

    caller.require(permissions::AUDIT_LOGS)?;

    Ok(audit::page(pool, request)
        .await?
        .map(super::audit_view::listing))
}

/// One entry of the trail, opened.
///
/// The whole row this time, including the user agent and the stored detail
/// object rendered as a diff - see [`super::audit_view`]. Which is why it is
/// gated the same way the list is: the detail is the part worth protecting.
pub async fn audit_event(
    pool: &PgPool,
    caller: &Caller,
    id: i64,
) -> ServiceResult<phonix_core::identity::AuditEventDetail> {
    use phonix_db::identity::audit;

    caller.require(permissions::AUDIT_LOGS)?;

    audit::find(pool, id)
        .await?
        .map(super::audit_view::described)
        .ok_or_else(|| ServiceError::rejected("event", msg!("error.audit_event.gone")))
}
