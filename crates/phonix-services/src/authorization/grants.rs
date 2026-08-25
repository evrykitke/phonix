//! Reading and editing what an account or a role may do.
//!
//! # The editor submits a set, not a diff
//!
//! Both save paths take the whole ticked set and work out the difference here.
//! A diff computed in the browser would be a diff against whatever that tab
//! loaded, which may be minutes old and may be somebody else's screen - and
//! applying it would silently undo their change. A set is idempotent: two
//! administrators saving the same screen twice get the same answer.
//!
//! # Roles and accounts are stored differently on purpose
//!
//! A role's grants are replaced wholesale, because a role *is* its set. An
//! account's are stored as overrides - additions and denials on top of its
//! roles - so that changing a role still reaches the people holding it. Storing
//! an account's effective set would freeze it at the moment somebody last
//! opened the screen. [`phonix_core::authorization::grants`] does that
//! arithmetic; this module writes the rows it produces and records who did it.

use phonix_core::authorization::grants::{PermissionOverrides, UserPermissionView};
use phonix_core::authorization::{PermissionSet, RoleDetail, roles as static_roles};
use phonix_core::identity::UserId;
use phonix_core::permissions;
use phonix_db::authorization::{permission as permission_store, role as role_store};
use phonix_db::identity::user as user_store;
use phonix_db::sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};
use phonix_core::msg;

// ---------------------------------------------------------------------------
// One account
// ---------------------------------------------------------------------------

/// Everything the per-user permission editor renders.
///
/// Gated on `Users` rather than `Users.ChangePermissions`: being able to see
/// what somebody may do is part of being able to see the user list at all, and
/// a read-only view is the honest thing to show an administrator who cannot
/// edit.
pub async fn user_permissions(
    pool: &PgPool,
    caller: &Caller,
    user_id: UserId,
) -> ServiceResult<UserPermissionView> {
    caller.require(permissions::USERS)?;

    let Some(account) = user_store::find_by_id(pool, user_id).await? else {
        return Err(ServiceError::rejected("user", msg!("error.user.gone")));
    };

    let roles = role_store::names_for_user(pool, user_id).await?;
    let overrides = permission_store::overrides_for_user(pool, user_id).await?;

    // The union of the roles' own grants, which is what the screen has to show
    // apart from the individual overrides.
    let mut from_roles = PermissionSet::new();
    for role in &roles {
        if let Some(record) = role_store::find_by_name(pool, role).await? {
            from_roles.extend_from(&role_store::permissions_of(pool, record.id).await?);
        }
    }

    Ok(UserPermissionView {
        user_id,
        display_name: account.display_name,
        email: account.email,
        is_owner: account.is_owner,
        roles,
        from_roles,
        overrides: PermissionOverrides {
            granted: overrides.granted,
            denied: overrides.denied,
        },
    })
}

/// Store the permissions an administrator ticked for one account.
///
/// `desired` is the whole effective set the editor is asking for. What gets
/// written is the difference from what the roles already give - see
/// [`UserPermissionView::overrides_for`].
pub async fn set_user_permissions(
    pool: &PgPool,
    caller: &Caller,
    user_id: UserId,
    desired: &PermissionSet,
) -> ServiceResult<UserPermissionView> {
    caller.require(permissions::USERS_CHANGE_PERMISSIONS)?;
    let changed_by = acting_user(caller)?;

    let current = user_permissions(pool, caller, user_id).await?;

    // The owner is excluded here and not merely hidden in the UI. A denial on
    // the owner is how a workspace ends up with nobody able to administer it,
    // and this mechanism is precisely powerful enough to do it in one click.
    if !current.is_editable() {
        return Err(ServiceError::rejected(
            "permissions",
            msg!("error.owner.permissions_locked"),
        ));
    }

    // The same door as `set_role_permissions`, for the same reason: a per-user
    // override is a grant like any other, and an app that is switched off is
    // switched off for everybody.
    let enabled = crate::workspace::apps::enabled_ids(pool).await?;
    let desired = &desired.clone().for_enabled_apps(&enabled);

    let overrides = UserPermissionView::overrides_for(&current.from_roles, desired);

    if overrides == current.overrides {
        // Nothing to write and nothing to record. An audit trail full of
        // "changed: nothing" entries is one nobody reads.
        return Ok(current);
    }

    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;

    // Replaced wholesale rather than patched: the incoming set is the whole
    // answer, and a partial update would leave behind a row for a permission
    // the administrator just unticked.
    permission_store::clear_all_overrides(&mut *tx, user_id).await?;

    for name in overrides.granted.iter() {
        permission_store::set_override(&mut *tx, user_id, name, true, Some(changed_by)).await?;
    }
    for name in overrides.denied.iter() {
        permission_store::set_override(&mut *tx, user_id, name, false, Some(changed_by)).await?;
    }

    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    // Against the account, not against whoever did it: "what was this person
    // given, and when" is the question asked afterwards, and it is answered on
    // their own page rather than by reading the whole trail for their name.
    // The overrides are both sides of the diff, so the two lists arrive as
    // added and removed rather than as four lists to compare by eye.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::USER, user_id)
            .named(&current.display_name)
            .fact("email", &current.email),
        &current.overrides,
        &overrides,
    )
    .await;

    tracing::info!(
        %user_id,
        %changed_by,
        granted = overrides.granted.len(),
        denied = overrides.denied.len(),
        "individual permissions changed"
    );

    Ok(UserPermissionView {
        overrides,
        ..current
    })
}

/// Return an account to whatever its roles say.
pub async fn clear_user_permissions(
    pool: &PgPool,
    caller: &Caller,
    user_id: UserId,
) -> ServiceResult<UserPermissionView> {
    set_user_permissions(pool, caller, user_id, &{
        let current = user_permissions(pool, caller, user_id).await?;
        current.from_roles
    })
    .await
}

// ---------------------------------------------------------------------------
// One role
// ---------------------------------------------------------------------------

/// A role and everything it grants, for the role editor.
pub async fn role_permissions(
    pool: &PgPool,
    caller: &Caller,
    role_id: Uuid,
) -> ServiceResult<RoleDetail> {
    caller.require(permissions::ROLES)?;

    let summary = role_store::list(pool)
        .await?
        .into_iter()
        .find(|role| role.id == role_id)
        .ok_or_else(|| ServiceError::rejected("role", msg!("error.role.gone")))?;

    let permissions = role_store::permissions_of(pool, role_id).await?;

    Ok(RoleDetail {
        summary,
        permissions,
    })
}

/// Replace what a role grants.
///
/// Reaches everybody holding the role, immediately - permissions are resolved
/// per request, not cached into a session - which is why this is the one path
/// gated on `Roles.ChangePermissions` rather than `Roles.Edit`.
pub async fn set_role_permissions(
    pool: &PgPool,
    caller: &Caller,
    role_id: Uuid,
    desired: &PermissionSet,
) -> ServiceResult<RoleDetail> {
    caller.require(permissions::ROLES_CHANGE_PERMISSIONS)?;
    let changed_by = acting_user(caller)?;

    let current = role_permissions(pool, caller, role_id).await?;

    // `Admin` holds the whole tree by definition, and `sync_static_roles`
    // rewrites it on every deploy. Letting it be edited here would produce a
    // change that silently reverts at the next release - worse than refusing.
    if current
        .summary
        .name
        .eq_ignore_ascii_case(static_roles::ADMIN)
    {
        return Err(ServiceError::rejected(
            "permissions",
            msg!("error.role.admin_is_absolute"),
        ));
    }

    // An app this workspace has not subscribed to cannot be granted its way
    // back in. Enablement is expressed as permissions - see
    // `phonix_services::workspace::apps` - and this is the one door that would
    // otherwise let somebody re-open it by hand: the static roles are rewritten
    // on every boot, but a role the organization defined is not.
    //
    // Dropped silently rather than refused. The editor still draws the whole
    // tree, so a tick beside a switched-off app is a control that should not
    // have been offered rather than a mistake worth an error message - and see
    // `[[grid-permission-gating-is-cosmetic]]` for why the refusal belongs
    // here either way.
    let enabled = crate::workspace::apps::enabled_ids(pool).await?;
    let desired = phonix_core::authorization::grants::normalise(desired).for_enabled_apps(&enabled);

    if desired == current.permissions {
        return Ok(current);
    }

    role_store::set_permissions(pool, role_id, &desired).await?;

    // Wrapped in a named field rather than diffed as two bare lists: a diff
    // whose only field has no name renders as a nameless row, and "Permissions"
    // is the word somebody is looking for on a role's history.
    audit::changed_json(
        pool,
        caller,
        Target::new(kinds::ROLE, role_id).named(&current.summary.display_name),
        serde_json::json!({ "permissions": current.permissions }),
        serde_json::json!({ "permissions": desired }),
    )
    .await;

    tracing::info!(
        role = %current.summary.name,
        %changed_by,
        count = desired.len(),
        "role permissions changed"
    );

    Ok(RoleDetail {
        permissions: desired,
        ..current
    })
}
