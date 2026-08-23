//! The people in this workspace, the roles, and what each may do.
//!
//! # The editor submits a set, not a diff
//!
//! Both save endpoints take the whole ticked set. A diff computed in the
//! browser would be a diff against whatever that tab loaded, which may be
//! somebody else's screen from ten minutes ago - and applying it would silently
//! undo their change. Working the difference out server-side, from what is
//! stored now, makes two administrators saving the same screen idempotent
//! rather than a race.
//!
//! Every one of these states its permission inside the service it calls. There
//! is no check here, deliberately: a second one in this file would be a second
//! place to forget, and the first one is the one that runs whether the request
//! came from this page, a script, or a future API.

use leptos::prelude::*;
use phonix_core::authorization::{
    PermissionSet, RoleDetail, RoleInput, RoleSummary, UserPermissionView,
};
use phonix_core::form::Submission;
use phonix_core::identity::{InvitationIssued, UserEdit, UserId, UserInvite, UserListing};
use phonix_core::mail::{MailSettings, MailSettingsInput};
use phonix_core::organization::OrganizationProfile;
use uuid::Uuid;

/// Everyone in this workspace, with their roles.
#[server(name = ListUsers, prefix = "/api", endpoint = "admin/users")]
pub async fn list_users() -> Result<Vec<UserListing>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::list(&pool, &caller)
        .await
        .map_err(service_error)
}

/// The editable part of one account, for the edit form to open on.
///
/// A separate call from [`list_users`] rather than a row picked out of it. A
/// `UserListing` is what a row *shows* - rendered, and mostly not editable -
/// and a form that edited one would be editing the display of an account
/// rather than the account.
#[server(name = LoadUserEdit, prefix = "/api", endpoint = "admin/users/edit")]
pub async fn user_edit(user_id: UserId) -> Result<UserEdit, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::edit(&pool, &caller, user_id)
        .await
        .map_err(service_error)
}

/// The roles the edit form offers.
///
/// Not [`list_roles`], which is the roles *screen* and is gated on `Roles`.
/// Editing an account needs the names to choose between and nothing more, so it
/// is gated on `Users` - see the service for why the two are separate.
#[server(name = AssignableRoles, prefix = "/api", endpoint = "admin/users/roles")]
pub async fn assignable_roles() -> Result<Vec<RoleSummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::assignable_roles(&pool, &caller)
        .await
        .map_err(service_error)
}

/// Store a changed account.
///
/// Returns a [`Submission`] rather than a `Result` alone, which is what lets a
/// rejected field arrive at the control it names. `ServerFnError` carries a
/// string, so a validation failure modelled as `Err` reaches the browser as
/// `"first_name: required"` - printable at the top of a form and impossible to
/// attach to the input it is about.
#[server(name = UpdateUser, prefix = "/api", endpoint = "admin/users/save")]
pub async fn update_user(draft: UserEdit) -> Result<Submission<UserEdit>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::update_user(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// One account, and what it may do.
#[server(name = UserPermissions, prefix = "/api", endpoint = "admin/user-permissions")]
pub async fn user_permissions(user_id: UserId) -> Result<UserPermissionView, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::grants::user_permissions(&pool, &caller, user_id)
        .await
        .map_err(service_error)
}

/// Store the permissions ticked for one account.
///
/// Returns the view as it now stands rather than `()`, so the screen re-renders
/// from what was actually stored - including the ancestors the server pulled in
/// and the redundant grants it declined to write.
#[server(name = SaveUserPermissions, prefix = "/api", endpoint = "admin/user-permissions/save")]
pub async fn save_user_permissions(
    user_id: UserId,
    permissions: PermissionSet,
) -> Result<UserPermissionView, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::grants::set_user_permissions(
        &pool,
        &caller,
        user_id,
        &permissions,
    )
    .await
    .map_err(service_error)
}

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

/// Add somebody and send them an invitation.
///
/// Returns a [`Submission`], so "somebody already has that address" arrives at
/// the email field rather than as a sentence at the top of the form.
///
/// The link comes back in the payload whether or not the email was delivered -
/// see `phonix_services::identity::invitation`. That is what makes a machine
/// with no relay usable, and it is why the screen shows the link when there is
/// something to say about delivery.
#[server(name = InviteUser, prefix = "/api", endpoint = "admin/users/invite")]
pub async fn invite_user(
    invite: UserInvite,
) -> Result<Submission<InvitationIssued>, ServerFnError> {
    use crate::state::{inviting_context, pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;
    let (state, tenant) = inviting_context().await?;

    phonix_services::identity::invitation::invite(
        &pool,
        &caller,
        &phonix_services::identity::invitation::Inviting {
            config: &state.config,
            hasher: &state.hasher,
            vault: &state.vault,
            workspace_slug: tenant.slug.as_str(),
            workspace_name: &tenant.display_name,
        },
        invite,
    )
    .await
    .map_err(service_error)
}

/// Issue a fresh invitation for somebody who has not accepted yet.
///
/// Safe to press twice: issuing supersedes the outstanding token, so the older
/// link stops working the moment a newer one exists.
#[server(name = ResendInvitation, prefix = "/api", endpoint = "admin/users/invite/resend")]
pub async fn resend_invitation(user_id: UserId) -> Result<InvitationIssued, ServerFnError> {
    use crate::state::{inviting_context, pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;
    let (state, tenant) = inviting_context().await?;

    phonix_services::identity::invitation::resend(
        &pool,
        &caller,
        &phonix_services::identity::invitation::Inviting {
            config: &state.config,
            hasher: &state.hasher,
            vault: &state.vault,
            workspace_slug: tenant.slug.as_str(),
            workspace_name: &tenant.display_name,
        },
        user_id,
    )
    .await
    .map_err(service_error)
}

// ---------------------------------------------------------------------------
// Mail
// ---------------------------------------------------------------------------

/// This workspace's own relay, if it has configured one.
///
/// Carries no password - `MailSettings` has no field for one. Whether a
/// password is stored comes back as a boolean.
#[server(name = LoadMailSettings, prefix = "/api", endpoint = "admin/mail")]
pub async fn mail_settings() -> Result<MailSettings, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::mail::settings::load(&pool, &caller)
        .await
        .map_err(service_error)
}

/// Which relay is actually in force, described for the screen.
///
/// Separate from [`mail_settings`] because the answer depends on the system
/// default as well as on this workspace's row, and the screen has to be able to
/// say "you are using ours" without the administrator inferring it.
#[server(name = MailRelayInUse, prefix = "/api", endpoint = "admin/mail/in-use")]
pub async fn mail_relay_in_use() -> Result<phonix_core::mail::RelayInUse, ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;
    let state = app_state()?;

    phonix_services::mail::settings::in_use(&pool, &caller, &state.config.smtp, Some(&state.vault))
        .await
        .map_err(service_error)
}

/// Store this workspace's relay.
///
/// A `password` of `None` leaves the stored one alone, so saving a changed host
/// does not require handing the form the secret it is not changing.
#[server(name = SaveMailSettings, prefix = "/api", endpoint = "admin/mail/save")]
pub async fn save_mail_settings(
    input: MailSettingsInput,
) -> Result<Submission<MailSettings>, ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;
    let state = app_state()?;

    phonix_services::mail::settings::save(&pool, &caller, &state.vault, input)
        .await
        .map_err(service_error)
}

/// Who this workspace legally is: name, address, currency, time zone.
///
/// Distinct from the tenant's `display_name` in the catalog, which is what an
/// operator calls this workspace. This is the entity that goes on a document.
#[server(name = LoadOrganizationProfile, prefix = "/api", endpoint = "admin/organization")]
pub async fn organization_profile() -> Result<OrganizationProfile, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::workspace::profile::load(&pool, &caller)
        .await
        .map_err(service_error)
}

/// Store this workspace's own details.
///
/// The submitted profile's `logo_file_id` is ignored: the logo is attached by
/// its own call, so that saving a form opened before somebody else changed the
/// logo cannot put the old one back on every document.
#[server(name = SaveOrganizationProfile, prefix = "/api", endpoint = "admin/organization/save")]
pub async fn save_organization_profile(
    profile: OrganizationProfile,
) -> Result<Submission<OrganizationProfile>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::workspace::profile::save(&pool, &caller, profile)
        .await
        .map_err(service_error)
}

/// Send a test message to whoever asked for it.
///
/// To the caller's own address and nowhere else: a test that could be pointed
/// at an arbitrary recipient is a way to make this server send mail to
/// strangers.
#[server(name = SendTestEmail, prefix = "/api", endpoint = "admin/mail/test")]
pub async fn send_test_email() -> Result<String, ServerFnError> {
    use crate::state::{app_state, pool_and_caller, service_error, tenant_from_request};
    use phonix_core::permissions;

    let (pool, caller) = pool_and_caller().await?;
    caller
        .require(permissions::SETTINGS)
        .map_err(service_error)?;

    let state = app_state()?;
    let tenant = tenant_from_request().await.map_err(ServerFnError::new)?;

    let Some(address) = caller.auth_user().map(|user| user.email.clone()) else {
        return Err(ServerFnError::new(
            "Only a signed-in user can send a test message.",
        ));
    };

    let relay = phonix_services::mail::resolve(&pool, &state.config.smtp, Some(&state.vault))
        .await
        .map_err(service_error)?;

    let Some(relay) = relay else {
        return Ok("No relay is configured, so nothing was sent.".to_owned());
    };

    let host = relay.host.clone();
    let message = phonix_services::mail::message::relay_test(&address, &tenant.display_name, &host);

    Ok(match phonix_services::mail::send(&relay, message).await {
        phonix_services::mail::Dispatch::Sent => {
            format!("Sent to {address} through {host}.")
        }
        phonix_services::mail::Dispatch::NotConfigured => {
            "No relay is configured, so nothing was sent.".to_owned()
        }
        phonix_services::mail::Dispatch::Failed(reason) => {
            format!("{host} refused it: {reason}")
        }
    })
}

/// Every role this workspace has defined.
#[server(name = ListRoles, prefix = "/api", endpoint = "admin/roles")]
pub async fn list_roles() -> Result<Vec<RoleSummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::roles::list(&pool, &caller)
        .await
        .map_err(service_error)
}

/// One role, and everything it grants.
#[server(name = RolePermissions, prefix = "/api", endpoint = "admin/role-permissions")]
pub async fn role_permissions(role_id: Uuid) -> Result<RoleDetail, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::grants::role_permissions(&pool, &caller, role_id)
        .await
        .map_err(service_error)
}

/// Replace what a role grants.
#[server(name = SaveRolePermissions, prefix = "/api", endpoint = "admin/role-permissions/save")]
pub async fn save_role_permissions(
    role_id: Uuid,
    permissions: PermissionSet,
) -> Result<RoleDetail, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::grants::set_role_permissions(
        &pool,
        &caller,
        role_id,
        &permissions,
    )
    .await
    .map_err(service_error)
}

/// One role's details, as the edit form should open on it.
///
/// Separate from [`role_permissions`], which is the same role read as a set of
/// grants. The two tabs of the role screen ask different questions and are
/// saved by different buttons, so they fetch separately rather than sharing one
/// payload that either half would have to pick apart.
#[server(name = LoadRole, prefix = "/api", endpoint = "admin/roles/detail")]
pub async fn role_detail(role_id: Uuid) -> Result<RoleInput, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::roles::detail(&pool, &caller, role_id)
        .await
        .map_err(service_error)
}

/// Define a role, or change one that exists.
///
/// Which of the two comes from the draft's `id`. A [`Submission`] rather than a
/// `Result` alone, so "a role is already called that" arrives at the name field
/// instead of as a sentence at the top of the form.
#[server(name = SaveRole, prefix = "/api", endpoint = "admin/roles/save")]
pub async fn save_role(draft: RoleInput) -> Result<Submission<RoleInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::roles::save(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// Remove a role, and with it whatever it granted to everybody holding it.
#[server(name = DeleteRole, prefix = "/api", endpoint = "admin/roles/delete")]
pub async fn delete_role(role_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::authorization::roles::delete(&pool, &caller, role_id)
        .await
        .map_err(service_error)
}

/// Remove every second factor from an account.
///
/// The administrator's answer to "I lost my phone and my recovery codes". The
/// account then signs in with a password alone until it enrols again, which is
/// why it is audited and why the screen says so before asking.
#[server(name = ResetUserMfa, prefix = "/api", endpoint = "admin/reset-mfa")]
pub async fn reset_user_mfa(user_id: UserId) -> Result<u64, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::mfa::reset_factors(&pool, &caller, user_id)
        .await
        .map_err(service_error)
}

/// One page of the security trail.
///
/// Paged rather than capped. The old screen fetched the most recent two hundred
/// entries and filtered them in the browser, which meant the answer to "was
/// this address ever locked out" depended on how busy the workspace had been
/// since. The reader now sees the whole table and returns one page of it.
#[server(name = AuditTrail, prefix = "/api", endpoint = "admin/audit")]
pub async fn audit_trail(
    request: phonix_core::query::PageRequest,
) -> Result<phonix_core::query::Page<phonix_core::identity::AuditEvent>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::audit_trail(&pool, &caller, &request)
        .await
        .map_err(service_error)
}

/// One entry of the trail, with its diff.
#[server(name = AuditEntry, prefix = "/api", endpoint = "admin/audit/entry")]
pub async fn audit_event(
    id: i64,
) -> Result<phonix_core::identity::AuditEventDetail, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::audit_event(&pool, &caller, id)
        .await
        .map_err(service_error)
}

/// One page of the change trail.
///
/// The other half of the audit screen. [`audit_trail`] answers "who signed in";
/// this answers "who edited this". Paged for the same reason: nothing deletes
/// from it either.
#[server(name = EntityTrail, prefix = "/api", endpoint = "admin/changes")]
pub async fn entity_trail(
    request: phonix_core::query::PageRequest,
) -> Result<phonix_core::query::Page<phonix_core::audit::EntityChange>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::audit::trail(&pool, &caller, &request)
        .await
        .map_err(service_error)
}

/// One change, with its diff.
#[server(name = EntityChangeEntry, prefix = "/api", endpoint = "admin/changes/entry")]
pub async fn entity_change(
    id: i64,
) -> Result<phonix_core::audit::EntityChangeDetail, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::audit::change(&pool, &caller, id)
        .await
        .map_err(service_error)
}

/// Who somebody is, for the card beside their name.
///
/// Fetched one at a time, on demand. Sending a card with every row of a trail
/// would be a query per row to answer a question about one of them - and most
/// rows are never asked about.
#[server(name = UserCardEntry, prefix = "/api", endpoint = "admin/user-card")]
pub async fn user_card(
    user_id: UserId,
) -> Result<Option<phonix_core::identity::UserCard>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::identity::directory::card(&pool, &caller, user_id)
        .await
        .map_err(service_error)
}

/// Everything that has happened to one record.
///
/// The history section on a detail page. The kind arrives as its stored name
/// rather than as a type, because a server function's arguments cross the wire
/// - an unknown name is refused here rather than turned into an empty list,
/// which would read as "nothing ever happened to this".
#[server(name = EntityHistory, prefix = "/api", endpoint = "admin/changes/history")]
pub async fn entity_history(
    kind: String,
    id: String,
) -> Result<Vec<phonix_core::audit::EntityChange>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let Some(kind) = phonix_core::audit::kind(&kind) else {
        return Err(ServerFnError::new(format!(
            "'{kind}' is not a kind of record."
        )));
    };

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::audit::history(&pool, &caller, kind, &id)
        .await
        .map_err(service_error)
}
