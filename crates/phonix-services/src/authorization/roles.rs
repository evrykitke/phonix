//! The roles a workspace has defined: listing them, defining them, removing
//! them.
//!
//! # What is *not* here
//!
//! What a role grants. That is [`grants::set_role_permissions`], and keeping it
//! there is deliberate: every rule about a grant - the ancestors a tick pulls
//! in, the refusal to edit `Admin`, the `{from, to}` audit entry - lives in one
//! function, so a second way to write `role_permissions` cannot disagree with
//! it. [`save`] therefore writes a role's *identity* and nothing about its
//! power, which is also why it is gated on `Roles.Create`/`Roles.Edit` rather
//! than on `Roles.ChangePermissions`.
//!
//! # Three permissions, because they are three different powers
//!
//! Being allowed to rename a role is not being allowed to define one, and
//! neither is being allowed to delete one - which strips whatever it granted
//! from everybody holding it, without touching their account.
//!
//! [`grants::set_role_permissions`]: super::grants::set_role_permissions

use phonix_core::authorization::{RoleInput, RoleSummary, ValidRole, roles as static_roles};
use phonix_core::form::Submission;
use phonix_core::permissions;
use phonix_db::authorization::role as store;
use phonix_db::error::DbError;
use phonix_db::sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};
use phonix_core::msg;

/// Every role, with how many permissions and people it carries.
pub async fn list(pool: &PgPool, caller: &Caller) -> ServiceResult<Vec<RoleSummary>> {
    caller.require(permissions::ROLES)?;
    Ok(store::list(pool).await?)
}

/// One role's details, as the edit form should open on it.
///
/// Gated on `Roles` rather than `Roles.Edit`, matching the account editor:
/// reading what a role is called is part of being able to see the list, and an
/// administrator who cannot edit gets a form of disabled controls rather than a
/// refusal. [`save`] is what requires the stronger permission.
pub async fn detail(pool: &PgPool, caller: &Caller, role_id: Uuid) -> ServiceResult<RoleInput> {
    caller.require(permissions::ROLES)?;

    let Some(role) = store::find_by_id(pool, role_id).await? else {
        return Err(ServiceError::rejected("role", msg!("error.role.gone")));
    };

    Ok(RoleInput {
        id: Some(role.id),
        name: role.name,
        display_name: role.display_name,
        description: role.description,
        is_default: role.is_default,
    })
}

/// Define a role, or change one that exists.
///
/// Which of the two it is comes from the draft: `id` absent means create. That
/// is the form's own answer rather than a second parameter, so a screen cannot
/// open the create form and submit it against an existing role.
///
/// Returns a [`Submission`], so "a role is already called that" arrives at the
/// name field rather than as a sentence at the top of the form.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    draft: RoleInput,
) -> ServiceResult<Submission<RoleInput>> {
    match draft.id {
        None => create(pool, caller, draft).await,
        Some(role_id) => update(pool, caller, role_id, draft).await,
    }
}

async fn create(
    pool: &PgPool,
    caller: &Caller,
    draft: RoleInput,
) -> ServiceResult<Submission<RoleInput>> {
    caller.require(permissions::ROLES_CREATE)?;
    // A role change must be attributable: `Caller::System` has no account
    // behind it, and a role nobody created is one nobody can be asked about.
    acting_user(caller)?;

    // The same rules the browser applied, applied again. The browser's check is
    // a courtesy; this one is the control.
    let role = match draft.validate(false) {
        Ok(role) => role,
        Err(errors) => return Ok(Submission::Rejected(errors)),
    };

    let stored = match store::create(
        pool,
        &role.name,
        &role.display_name,
        role.description.as_deref(),
        role.is_default,
    )
    .await
    {
        Ok(stored) => stored,
        // Not an error: the name is taken, which is a thing to say next to the
        // box somebody typed it into.
        Err(DbError::RoleExists(name)) => {
            return Ok(taken(&name));
        }
        Err(err) => return Err(err.into()),
    };

    let created = as_input(role, stored.id);

    audit::created(
        pool,
        caller,
        Target::new(kinds::ROLE, stored.id)
            .named(&created.display_name)
            // A new role grants nothing until the tree is edited, and saying so
            // here is what stops the trail reading as though it handed out
            // access.
            .fact("grants", "none until the permission tree is saved"),
        &created,
    )
    .await;

    Ok(Submission::Saved(created))
}

async fn update(
    pool: &PgPool,
    caller: &Caller,
    role_id: Uuid,
    draft: RoleInput,
) -> ServiceResult<Submission<RoleInput>> {
    caller.require(permissions::ROLES_EDIT)?;
    // A role change must be attributable: `Caller::System` has no account
    // behind it, and a role nobody created is one nobody can be asked about.
    acting_user(caller)?;

    let Some(before) = store::find_by_id(pool, role_id).await? else {
        return Ok(Submission::rejected("role", msg!("error.role.gone")));
    };

    // A built-in role keeps its key whatever was submitted - code assigns
    // `Admin` by that string. Validating the submitted name would refuse it
    // against the reserved list it is itself on, so the stored one is used and
    // the check is skipped.
    let is_static = before.is_static;
    let draft = if is_static {
        RoleInput {
            name: before.name.clone(),
            ..draft
        }
    } else {
        draft
    };

    let role = match draft.validate(is_static) {
        Ok(role) => role,
        Err(errors) => return Ok(Submission::Rejected(errors)),
    };

    if let Err(err) = store::update(
        pool,
        role_id,
        &role.name,
        &role.display_name,
        role.description.as_deref(),
        role.is_default,
    )
    .await
    {
        return match err {
            DbError::RoleExists(name) => Ok(taken(&name)),
            err => Err(err.into()),
        };
    }

    // Re-read rather than echoed: the database declines a rename on a static
    // role, and echoing the draft would leave the form showing a change that
    // did not happen.
    let after = detail(pool, caller, role_id).await?;

    // The whole value on each side rather than four hand-listed fields: a
    // column added to a role later is then in the diff without anybody
    // remembering to add it here.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::ROLE, role_id).named(&after.display_name),
        &recorded(&before),
        &after,
    )
    .await;

    Ok(Submission::Saved(after))
}

/// Remove a role.
///
/// Everybody holding it loses whatever it granted, at once - permissions are
/// resolved per request - so the count of people affected is recorded before
/// the row goes, while it is still knowable.
///
/// The static roles are refused twice: here, with a sentence somebody can read,
/// and by the `WHERE NOT is_static` in the statement, which is what makes it
/// true of the database rather than only of this function.
pub async fn delete(pool: &PgPool, caller: &Caller, role_id: Uuid) -> ServiceResult<()> {
    caller.require(permissions::ROLES_DELETE)?;
    // A role change must be attributable: `Caller::System` has no account
    // behind it, and a role nobody created is one nobody can be asked about.
    acting_user(caller)?;

    let Some(role) = store::find_by_id(pool, role_id).await? else {
        return Err(ServiceError::rejected("role", msg!("error.role.gone")));
    };

    if role.is_static || static_roles::is_static(&role.name) {
        return Err(ServiceError::rejected(
            "role",
            msg!("error.role.built_in_delete", name = &role.display_name),
        ));
    }

    // Counted before the delete, because `user_roles` cascades and afterwards
    // there is nothing left to count.
    let holders = store::list(pool)
        .await?
        .into_iter()
        .find(|summary| summary.id == role_id)
        .map_or(0, |summary| summary.user_count);

    store::delete(pool, role_id).await?;

    audit::deleted(
        pool,
        caller,
        Target::new(kinds::ROLE, role_id)
            .named(&role.display_name)
            // Counted before the delete, and recorded here because afterwards
            // nothing can answer "how far did that reach".
            .fact("people_affected", holders),
        &recorded(&role),
    )
    .await;

    Ok(())
}

/// The rejection for a name somebody else already has.
///
/// Its own function because create and update both reach it, and the wording is
/// the part worth keeping identical: two spellings of the same refusal read as
/// two different problems.
fn taken(name: &str) -> Submission<RoleInput> {
    Submission::rejected("name", msg!("error.role.name_taken", name = name))
}

/// A validated role, as the form re-opens on it.
fn as_input(role: ValidRole, id: Uuid) -> RoleInput {
    RoleInput {
        id: Some(id),
        ..role.into_input()
    }
}

/// A stored row in the shape the form uses.
///
/// The audited value on every side of every diff, so an edit and a deletion
/// describe a role with the same field names. Two shapes for one record is two
/// histories that cannot be read down the page together, and the stored row
/// carries `is_static` and `created_at`, neither of which anybody changed.
fn recorded(role: &store::RoleRecord) -> RoleInput {
    RoleInput {
        id: Some(role.id),
        name: role.name.clone(),
        display_name: role.display_name.clone(),
        description: role.description.clone(),
        is_default: role.is_default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_name_is_reported_against_the_box_it_was_typed_into() {
        // As a rejection rather than an `Err`: the alternative reaches the
        // browser as a sentence at the top of a form, next to nothing.
        let rejection = taken("Auditor");

        assert!(!rejection.is_saved());
        assert_eq!(rejection.errors()[0].field, "name");
        // Through `Display`, which renders the built-in English: the point of
        // the assertion is that the role's own name reaches the sentence.
        assert!(
            rejection.errors()[0]
                .message
                .to_string()
                .contains("Auditor")
        );
    }

    #[test]
    fn a_created_role_comes_back_carrying_the_id_it_was_given() {
        // Without it the form stays in create mode and a second save would
        // define a second role.
        let id = Uuid::from_u128(7);
        let role = RoleInput {
            id: None,
            name: "Auditor".into(),
            display_name: "Auditor".into(),
            description: None,
            is_default: false,
        };

        let saved = as_input(role.validate(false).unwrap(), id);

        assert_eq!(saved.id, Some(id));
        assert_eq!(saved.name, "Auditor");
    }
}
