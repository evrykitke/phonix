//! `/api/v1/users` - the people with an account on this workspace.
//!
//! Currencies proved the machinery; this proves the two things it could not.
//! Its read is **gated**, on `Pages.Administration.Users`, where the currency
//! list is ungated - so a key with no scopes gets a 403 here and a 200 there,
//! which is the scope intersection working rather than merely compiling. And
//! it is addressed by a UUID rather than by a code the caller already knows,
//! so a client has to have read the list to reach a row.
//!
//! # The writes, and the escalation question they had to answer
//!
//! ADR 0002 held these back with a stated worry: roles and status move
//! together, and *a key must not be able to hand itself a role*. Two things
//! answer it, and neither is new machinery:
//!
//! * **Changing roles requires `Users.ChangePermissions`**, asked for by the
//!   service only when the roles actually differ - so renaming somebody needs
//!   `Users.Edit` alone. A key can only carry that permission if its owner
//!   holds it, and its owner holding it means they can already do this in a
//!   browser.
//! * **A key cannot widen itself.** Its power is its owner's current grants
//!   intersected with its own scopes, re-read on every request. Granting its
//!   owner more changes the first half and not the second, so the key it was
//!   presented with reaches no further than it did a moment ago.
//!
//! What is left is that somebody with `Users.ChangePermissions` can escalate
//! *themselves*, which has always been true of that permission and is what it
//! means. It is not a property of this surface, and refusing here would only
//! mean the browser and the API disagree about who may administer a workspace.
//!
//! Four writes, three different gates:
//!
//! ```text
//! POST /users                     Users.Create  (+ChangePermissions with roles)
//! POST /users/{id}/invitation     Users.Create
//! PUT  /users/{id}                Users.Edit    (+ChangePermissions if roles move)
//! PUT  /users/{id}/permissions    Users.ChangePermissions
//! ```
//!
//! # What is deliberately still not here
//!
//! Setting somebody's password. An account gets one exactly once, from the
//! person who will use it, by opening an invitation link - see
//! `invitation::accept`. An endpoint that set one for somebody else would be
//! an account takeover with a friendly name, and there is no client that needs
//! it.
//!
//! # Why this pages in memory
//!
//! `directory::list` is unpaged, and its own doc comment says that is
//! deliberate: the workspace that needs server-side paging needs a cursor and
//! a server-side search designed together, and neither should be guessed at.
//! This handler is not the place to force that decision - a public endpoint
//! quietly growing a second, different paging story for the same rows the
//! screen shows is how the two drift apart. It reads what the screen reads and
//! cuts the page here; when the service grows a `PageRequest`, this passes it
//! down and nothing on the wire changes.

use axum::Json;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use phonix_core::authorization::PermissionSet;
use phonix_core::authorization::grants::UserPermissionView;
use phonix_core::form::Submission;
use phonix_core::identity::directory::UserListing;
use phonix_core::identity::user::{UserId, UserStatus};
use phonix_core::identity::{InvitationIssued, UserEdit, UserInvite};
use phonix_core::query::{Page, PageRequest};
use phonix_services::ServiceError;
use phonix_services::authorization::grants;
use phonix_services::identity::{directory, invitation};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::ApiCaller;
use super::json::ApiJson;
use super::paging::{ListParams, ListRequest, PageEnvelope, cut};
use super::path::ApiPath;
use super::problem::Problem;

/// Where an account stands.
///
/// Declared here rather than deriving `ToSchema` on `UserStatus`, for the
/// reason the whole module set exists: a variant renamed in `phonix-core` has
/// to stop this file compiling. It is also the one enum in `v1` that a client
/// will match on exhaustively, so adding a variant is a real event and should
/// look like one from here.
#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = UserStatus)]
pub enum UserStatusResource {
    /// Invited, but has not set a password or verified their address yet.
    Pending,
    /// Ordinary.
    Active,
    /// Blocked by an administrator. Reversible.
    Suspended,
    /// Left the organization. Kept so documents they touched still resolve.
    Deactivated,
}

impl From<UserStatus> for UserStatusResource {
    fn from(status: UserStatus) -> Self {
        match status {
            UserStatus::Pending => Self::Pending,
            UserStatus::Active => Self::Active,
            UserStatus::Suspended => Self::Suspended,
            UserStatus::Deactivated => Self::Deactivated,
        }
    }
}

impl From<UserStatusResource> for UserStatus {
    fn from(status: UserStatusResource) -> Self {
        match status {
            UserStatusResource::Pending => Self::Pending,
            UserStatusResource::Active => Self::Active,
            UserStatusResource::Suspended => Self::Suspended,
            UserStatusResource::Deactivated => Self::Deactivated,
        }
    }
}

/// A person with an account on this workspace.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = User)]
pub struct UserResource {
    pub id: Uuid,
    #[schema(example = "ada@example.com")]
    pub email: String,
    #[schema(example = "Ada Lovelace")]
    pub display_name: String,
    pub status: UserStatusResource,
    /// Created the workspace. Such an account cannot be suspended or stripped
    /// of its roles, so a client offering those actions should not offer them
    /// on this row.
    pub is_owner: bool,
    pub email_verified: bool,
    /// Holds at least one confirmed second factor.
    pub mfa_enabled: bool,
    /// Role names, which are the same vocabulary an API key's scopes are drawn
    /// from. Ordered as the database returned them.
    pub roles: Vec<String>,
    /// Set while the account is locked out after failed sign-ins. `null` is the
    /// ordinary case, and an instant in the past means the lockout has expired
    /// without anything having cleared the column - so this is not on its own
    /// the answer to "can they sign in".
    pub locked_until: Option<DateTime<Utc>>,
    /// Whether that lockout still held when this row was read. The comparison
    /// `locked_until` needs, done against the server's clock rather than the
    /// caller's.
    pub locked: bool,
    /// `null` for somebody who was invited and never arrived.
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<&UserListing> for UserResource {
    fn from(row: &UserListing) -> Self {
        Self {
            id: row.id,
            email: row.email.clone(),
            display_name: row.display_name.clone(),
            status: row.status.into(),
            is_owner: row.is_owner,
            email_verified: row.email_verified,
            mfa_enabled: row.mfa_enabled,
            roles: row.roles.clone(),
            locked_until: row.locked_until,
            locked: row.locked,
            last_login_at: row.last_login_at,
            created_at: row.created_at,
        }
    }
}

/// Everybody with an account on this workspace.
///
/// Searches display name, address and role name - the three the screen's own
/// search box looks in, so a script and a person typing find the same rows.
///
/// Sorts by `display_name` (the default), `email`, `status`, `mfa_enabled`,
/// `created_at` or `last_login_at`. Ties always break the same way, so paging
/// through the list shows every row exactly once.
#[utoipa::path(
    get,
    path = "/users",
    tag = "users",
    operation_id = "listUsers",
    params(
        ListParams,
        ("filter[status]" = Option<String>, Query,
            description = "One of pending, active, suspended, deactivated.",
            example = "active"),
        ("filter[role]" = Option<String>, Query,
            description = "A role name, matched whole and case-insensitively.",
            example = "Administrator"),
    ),
    responses(
        (status = 200, description = "One page of accounts", body = PageEnvelope<UserResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "No API access, or the key does not carry Users", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<UserResource>>, Problem> {
    let rows = directory::list(&caller.pool, &caller.caller).await?;

    Ok(Json(PageEnvelope::new(paginate(rows, &request))))
}

/// One account, by id.
#[utoipa::path(
    get,
    path = "/users/{id}",
    tag = "users",
    operation_id = "getUser",
    params(("id" = Uuid, Path, description = "The account's id")),
    responses(
        (status = 200, description = "The account", body = UserResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users", body = Problem),
        (status = 404, description = "No such account on this workspace", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get(
    caller: ApiCaller,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<UserResource>, Problem> {
    // `directory::find` is the obvious call and the wrong one: it answers a
    // missing account with `ServiceError::rejected`, which this router renders
    // as a 422 carrying a field error. That is right for a form, where the id
    // came from a hidden input and being wrong is a validation failure. It is
    // wrong here, where the id is the address and there is nothing at it.
    // `list` applies the same `Caller::require`, and `find` scans the same
    // whole list anyway, so this costs nothing and gives a client's router the
    // 404 it expects.
    let rows = directory::list(&caller.pool, &caller.caller).await?;

    rows.iter()
        .find(|row| row.id == id)
        .map(|row| Json(UserResource::from(row)))
        .ok_or_else(missing)
}

/// What `POST /users` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = UserInvite)]
pub struct InviteUser {
    #[schema(example = "ada@example.com")]
    pub email: String,
    #[schema(example = "Ada")]
    pub first_name: String,
    #[schema(example = "Lovelace")]
    pub last_name: String,
    /// Role **names**, the same strings `GET /roles` lists and `User.roles`
    /// carries. Empty is legitimate: an account with no role can sign in and
    /// do nothing, which is a reasonable place to start somebody whose access
    /// is still being decided.
    ///
    /// Naming any role requires `Users.ChangePermissions` on top of
    /// `Users.Create`, because granting a role is granting permissions however
    /// it is spelled.
    #[schema(example = json!(["Bookkeeper"]))]
    #[serde(default)]
    pub roles: Vec<String>,
}

/// What `PUT /users/{id}` accepts.
///
/// Not the email: an address is the account's identity and changing it is not
/// an edit. Not the password either - see the module note.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = UserSave)]
pub struct SaveUser {
    #[schema(example = "Ada")]
    pub first_name: String,
    #[schema(example = "Lovelace")]
    pub last_name: String,
    pub status: UserStatusResource,
    /// Role names. The whole set, not a diff - the same rule the permission
    /// editor follows, for the same reason: a diff would be a diff against
    /// whatever this caller last read.
    ///
    /// Changing this requires `Users.ChangePermissions`; leaving it as it
    /// stands does not.
    pub roles: Vec<String>,
}

/// An invitation, and the link it was sent as.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = Invitation)]
pub struct InvitationResource {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    /// Absolute, single-use, and **the one time it is available**. Issuing a
    /// new invitation supersedes it.
    ///
    /// It comes back whether or not the email was delivered, deliberately: a
    /// workspace whose relay is not configured yet still has to be able to
    /// bring somebody in, and an account that exists with an undelivered
    /// invitation is recoverable where a rolled-back request is not.
    ///
    /// Treat it as a credential. Anybody holding it can set that account's
    /// password.
    pub link: String,
    /// Hours from now.
    #[schema(example = 72)]
    pub expires_in_hours: i64,
    /// Whether the person will receive this without help. `false` means the
    /// link is the only way in, and the caller has to deliver it.
    pub emailed: bool,
    /// What happened to the email, when there is anything to say. `null` when
    /// it was delivered and there is nothing worth reporting.
    pub delivery_note: Option<String>,
}

impl From<InvitationIssued> for InvitationResource {
    fn from(issued: InvitationIssued) -> Self {
        Self {
            emailed: issued.was_emailed(),
            user_id: issued.user_id,
            email: issued.email,
            display_name: issued.display_name,
            link: issued.link,
            expires_in_hours: issued.expires_in_hours,
            delivery_note: issued.delivery_note,
        }
    }
}

/// Everything one account may do, and where each part of it comes from.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = UserPermissions)]
pub struct UserPermissionsResource {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    /// Created the workspace. **Its permissions cannot be edited** - a denial
    /// on the owner is how a workspace ends up with nobody able to administer
    /// it - so a client should not offer the control on this account.
    pub is_owner: bool,
    pub roles: Vec<String>,
    /// The union of what this account's roles grant. Changing a role changes
    /// this, for everybody holding it.
    pub from_roles: Vec<String>,
    /// Individual additions, on top of whatever the roles give.
    pub granted: Vec<String>,
    /// Individual denials, which beat any role grant. A denial names exactly
    /// one permission and does not cascade to what hangs beneath it.
    pub denied: Vec<String>,
    /// What the account may actually do: roles, plus grants, minus denials.
    /// Derived, and the field to read if you only want one - the other three
    /// are for showing *why*.
    pub effective: Vec<String>,
}

impl From<&UserPermissionView> for UserPermissionsResource {
    fn from(view: &UserPermissionView) -> Self {
        Self {
            user_id: view.user_id,
            display_name: view.display_name.clone(),
            email: view.email.clone(),
            is_owner: view.is_owner,
            roles: view.roles.clone(),
            from_roles: view.from_roles.iter().map(str::to_owned).collect(),
            granted: view.overrides.granted.iter().map(str::to_owned).collect(),
            denied: view.overrides.denied.iter().map(str::to_owned).collect(),
            effective: view.effective().iter().map(str::to_owned).collect(),
        }
    }
}

/// What `PUT /users/{id}/permissions` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = UserPermissionsSave)]
pub struct SaveUserPermissions {
    /// The whole **effective** set this account should end up with - not the
    /// overrides. What gets stored is the difference from what the roles
    /// already give, worked out server-side, so that changing a role still
    /// reaches this person afterwards. Storing an effective set would freeze
    /// them at the moment somebody last saved this.
    ///
    /// A set rather than a diff, and idempotent: two administrators saving the
    /// same set twice get the same answer.
    #[schema(example = json!(["Pages", "Pages.Dashboard", "Pages.Administration"]))]
    pub permissions: Vec<String>,
}

/// Invite somebody, creating their account.
///
/// The account is created `pending` with **no password**: the person sets one
/// by opening the link, and opening it is also what proves their address
/// receives mail.
///
/// The mail send is outside the transaction that creates the account and is
/// **not allowed to fail this request** - see [`InvitationResource::link`].
///
/// Requires `Pages.Administration.Users.Create`, plus
/// `Users.ChangePermissions` if any role is named.
#[utoipa::path(
    post,
    path = "/users",
    tag = "users",
    operation_id = "inviteUser",
    request_body = InviteUser,
    responses(
        (status = 201, description = "The account, and its invitation link", body = InvitationResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users.Create", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "A bad address, one already taken, or an unknown role", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn invite(
    caller: ApiCaller,
    ApiJson(body): ApiJson<InviteUser>,
) -> Result<
    (
        StatusCode,
        [(axum::http::HeaderName, String); 1],
        Json<InvitationResource>,
    ),
    Problem,
> {
    let issued = invitation::invite(
        &caller.pool,
        &caller.caller,
        &inviting(&caller),
        UserInvite {
            email: body.email,
            first_name: body.first_name,
            last_name: body.last_name,
            roles: body.roles,
        },
    )
    .await?;

    match issued {
        Submission::Saved(issued) => {
            tracing::info!(
                key = ?caller.key_id,
                invited = %issued.user_id,
                emailed = issued.was_emailed(),
                "user invited through the api"
            );

            let location = format!("/api/v1/users/{}", issued.user_id);

            Ok((
                StatusCode::CREATED,
                [(axum::http::header::LOCATION, location)],
                Json(InvitationResource::from(issued)),
            ))
        }
        Submission::Rejected(errors) => Err(Problem::from(ServiceError::Rejected(errors))),
    }
}

/// Issue a fresh invitation for somebody who has not accepted yet.
///
/// For the invitation that expired, went to a spam folder, or was sent before
/// the relay worked. Safe to call twice: issuing supersedes the outstanding
/// token, so the older link stops working the moment a newer one exists.
///
/// Refused for an account that is not `pending`. Re-inviting an active account
/// would mint a link that sets its password, which is an account takeover with
/// a friendly name.
///
/// Requires `Pages.Administration.Users.Create`.
#[utoipa::path(
    post,
    path = "/users/{id}/invitation",
    tag = "users",
    operation_id = "resendInvitation",
    params(("id" = Uuid, Path, description = "The account's id")),
    responses(
        (status = 200, description = "The new invitation", body = InvitationResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users.Create", body = Problem),
        (status = 422, description = "No such account, or one that has already accepted", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn resend_invitation(
    caller: ApiCaller,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<InvitationResource>, Problem> {
    let issued = invitation::resend(&caller.pool, &caller.caller, &inviting(&caller), id).await?;

    tracing::info!(key = ?caller.key_id, invited = %id, "invitation reissued through the api");

    Ok(Json(InvitationResource::from(issued)))
}

/// Change an account's name, status or roles.
///
/// The workspace owner's **status** cannot be changed and its `Admin` role
/// cannot be taken away: the owner is the one account that must stay able to
/// administer the workspace.
///
/// Requires `Pages.Administration.Users.Edit`, and `Users.ChangePermissions`
/// as well when the roles actually differ from what is stored - so renaming
/// somebody does not need the wider permission.
#[utoipa::path(
    put,
    path = "/users/{id}",
    tag = "users",
    operation_id = "updateUser",
    params(("id" = Uuid, Path, description = "The account's id")),
    request_body = SaveUser,
    responses(
        (status = 200, description = "The account as it now stands", body = UserResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users.Edit", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "No such account, a bad name, an unknown role, or the owner's status", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn update(
    caller: ApiCaller,
    ApiPath(id): ApiPath<UserId>,
    ApiJson(body): ApiJson<SaveUser>,
) -> Result<Json<UserResource>, Problem> {
    // The id comes from the path, never from the body. A draft carrying its
    // own id would let `PUT /users/A` edit account B, and the two would
    // disagree exactly once, in production.
    let draft = UserEdit {
        id,
        first_name: body.first_name,
        last_name: body.last_name,
        status: body.status.into(),
        roles: body.roles,
    };

    match directory::update_user(&caller.pool, &caller.caller, draft).await? {
        Submission::Saved(_) => {
            tracing::info!(key = ?caller.key_id, user = %id, "account edited through the api");
        }
        Submission::Rejected(errors) => {
            return Err(Problem::from(ServiceError::Rejected(errors)));
        }
    }

    // Read back through the listing rather than returning `UserEdit`, so a
    // write and a read of the same account answer with the same object. The
    // service already re-reads what it stored; this re-reads it in the shape
    // this resource publishes.
    Ok(Json(one(&caller, id).await?))
}

/// What one account may do, and where each part of it comes from.
///
/// Gated on `Users` rather than `Users.ChangePermissions`: being able to see
/// what somebody may do is part of being able to see the user list at all.
#[utoipa::path(
    get,
    path = "/users/{id}/permissions",
    tag = "users",
    operation_id = "getUserPermissions",
    params(("id" = Uuid, Path, description = "The account's id")),
    responses(
        (status = 200, description = "The account's permissions, by source", body = UserPermissionsResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users", body = Problem),
        (status = 404, description = "No such account on this workspace", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_permissions(
    caller: ApiCaller,
    ApiPath(id): ApiPath<UserId>,
) -> Result<Json<UserPermissionsResource>, Problem> {
    let view = permissions_of(&caller, id).await?;

    Ok(Json(UserPermissionsResource::from(&view)))
}

/// Replace what one account may do.
///
/// Send the whole **effective** set; the overrides are worked out from it -
/// see [`SaveUserPermissions`].
///
/// The workspace owner is refused, not merely hidden: this mechanism is
/// precisely powerful enough to leave a workspace with nobody able to
/// administer it, in one call.
///
/// Requires `Pages.Administration.Users.ChangePermissions`.
#[utoipa::path(
    put,
    path = "/users/{id}/permissions",
    tag = "users",
    operation_id = "setUserPermissions",
    params(("id" = Uuid, Path, description = "The account's id")),
    request_body = SaveUserPermissions,
    responses(
        (status = 200, description = "The account's permissions as they now stand", body = UserPermissionsResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users.ChangePermissions", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "No such account, or the workspace owner", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn set_permissions(
    caller: ApiCaller,
    ApiPath(id): ApiPath<UserId>,
    ApiJson(body): ApiJson<SaveUserPermissions>,
) -> Result<Json<UserPermissionsResource>, Problem> {
    // A 404 before the write, for the same reason `get` spells one: an id
    // that names nothing is an address with nothing at it, and the service
    // would answer a 422 naming a field the caller never sent.
    permissions_of(&caller, id).await?;

    // `grant` rather than `insert_exact`: naming a permission implies its
    // ancestors, exactly as a tick in the editor does.
    let mut desired = PermissionSet::new();
    for name in &body.permissions {
        let name = name.trim();
        if !name.is_empty() {
            desired.grant(name);
        }
    }

    let view = grants::set_user_permissions(&caller.pool, &caller.caller, id, &desired).await?;

    tracing::info!(
        key = ?caller.key_id,
        user = %id,
        granted = view.overrides.granted.len(),
        denied = view.overrides.denied.len(),
        "individual permissions saved through the api"
    );

    Ok(Json(UserPermissionsResource::from(&view)))
}

/// One account's permission view, with a 404 rather than a field rejection.
///
/// `grants::user_permissions` answers a missing account the way every editor
/// wants and no address does. Same trap as `get`, same answer.
async fn permissions_of(caller: &ApiCaller, id: UserId) -> Result<UserPermissionView, Problem> {
    // Cheaper than it looks: the same `Caller::require` either way, and the
    // list is what `find` scans anyway.
    let rows = directory::list(&caller.pool, &caller.caller).await?;

    if !rows.iter().any(|row| row.id == id) {
        return Err(missing());
    }

    Ok(grants::user_permissions(&caller.pool, &caller.caller, id).await?)
}

/// One account, re-read after a write.
async fn one(caller: &ApiCaller, id: UserId) -> Result<UserResource, Problem> {
    let rows = directory::list(&caller.pool, &caller.caller).await?;

    rows.iter()
        .find(|row| row.id == id)
        .map(UserResource::from)
        .ok_or_else(missing)
}

/// The one 404 this resource answers, spelled once.
///
/// The id is not in the sentence, deliberately: it is the address the caller
/// already used, and repeating it says nothing they did not send.
fn missing() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "There is no account with that id on this workspace.",
    )
}

/// What inviting somebody needs from the outside world.
///
/// The slug builds the link and the display name signs the message, which is
/// why `ApiCaller` keeps the tenant rather than dropping it: resolving the
/// workspace a second time here is a second answer to a question the request
/// already had one for.
fn inviting(caller: &ApiCaller) -> invitation::Inviting<'_> {
    invitation::Inviting {
        config: &caller.state.config,
        hasher: &caller.state.hasher,
        vault: &caller.state.vault,
        workspace_slug: caller.tenant.slug.as_str(),
        workspace_name: &caller.tenant.display_name,
    }
}

/// Search, narrow, sort and cut one page.
fn paginate(rows: Vec<UserListing>, request: &PageRequest) -> Page<UserResource> {
    let needle = request.needle();

    let mut matching: Vec<&UserListing> = rows
        .iter()
        .filter(|row| match &needle {
            Some(needle) => {
                row.display_name.to_lowercase().contains(needle)
                    || row.email.to_lowercase().contains(needle)
                    || row
                        .roles
                        .iter()
                        .any(|role| role.to_lowercase().contains(needle))
            }
            None => true,
        })
        // A status this build does not have narrows to nothing rather than to
        // everything. Unlike an unknown sort field, the caller named a value
        // and meant it, and answering the whole list to `status=retired` reads
        // as "everybody is retired".
        .filter(|row| match request.filter("status") {
            Some(status) => row.status.as_str().eq_ignore_ascii_case(status),
            None => true,
        })
        // Whole rather than substring, because role names nest by convention
        // and "Admin" must not drag in "Administrator".
        .filter(|row| match request.filter("role") {
            Some(role) => row.roles.iter().any(|held| held.eq_ignore_ascii_case(role)),
            None => true,
        })
        .collect();

    let descending = request
        .sort
        .as_ref()
        .is_some_and(|sort| !sort.direction.is_ascending());

    // Every arm breaks its ties, because rows that compare equal under the
    // chosen field would otherwise sit in whatever order the database felt
    // like - and two rows that swap places between one request and the next
    // are a row that appears on both pages and a row that appears on neither.
    match request.sort.as_ref().map(|sort| sort.field.as_str()) {
        Some("email") => matching.sort_by(|a, b| {
            a.email
                .to_lowercase()
                .cmp(&b.email.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        }),
        Some("status") => matching.sort_by(|a, b| {
            a.status
                .as_str()
                .cmp(b.status.as_str())
                .then_with(|| a.id.cmp(&b.id))
        }),
        Some("mfa_enabled") => matching.sort_by(|a, b| {
            a.mfa_enabled
                .cmp(&b.mfa_enabled)
                .then_with(|| a.id.cmp(&b.id))
        }),
        Some("created_at") => {
            matching.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        // `None` sorts first ascending, which puts everybody who has never
        // signed in at the top - the end of the list somebody chasing dormant
        // invitations actually wants.
        Some("last_login_at") => matching.sort_by(|a, b| {
            a.last_login_at
                .cmp(&b.last_login_at)
                .then_with(|| a.id.cmp(&b.id))
        }),
        // Display name is the default, matching the screen, and compares
        // case-insensitively so "ada" and "Ada" are not two neighbourhoods.
        _ => matching.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        }),
    }
    if descending {
        matching.reverse();
    }

    cut(matching, request, UserResource::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, email: &str, status: UserStatus, roles: &[&str]) -> UserListing {
        UserListing {
            id: Uuid::new_v4(),
            email: email.to_owned(),
            display_name: name.to_owned(),
            status,
            is_owner: false,
            email_verified: true,
            mfa_enabled: false,
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            locked_until: None,
            locked: false,
            last_login_at: None,
            created_at: Utc::now(),
        }
    }

    fn directory() -> Vec<UserListing> {
        vec![
            person(
                "ada lovelace",
                "ada@example.com",
                UserStatus::Active,
                &["Administrator"],
            ),
            person(
                "Grace Hopper",
                "grace@example.com",
                UserStatus::Active,
                &["Admin"],
            ),
            person(
                "Alan Turing",
                "alan@example.com",
                UserStatus::Suspended,
                &[],
            ),
            person(
                "Katherine Johnson",
                "kj@example.com",
                UserStatus::Pending,
                &["Administrator"],
            ),
        ]
    }

    fn names(page: &Page<UserResource>) -> Vec<String> {
        page.rows
            .iter()
            .map(|row| row.display_name.clone())
            .collect()
    }

    #[test]
    fn the_default_page_sorts_by_name_ignoring_case() {
        let page = paginate(directory(), &PageRequest::default());

        // Sorting on the raw bytes would put every capital ahead of every
        // lower case letter, and "ada" would land after "Katherine".
        assert_eq!(
            names(&page),
            vec![
                "ada lovelace",
                "Alan Turing",
                "Grace Hopper",
                "Katherine Johnson"
            ]
        );
        assert_eq!(page.total, 4);
    }

    #[test]
    fn a_search_looks_at_the_name_the_address_and_the_roles() {
        let by_name = paginate(
            directory(),
            &PageRequest {
                search: "turing".to_owned(),
                ..PageRequest::default()
            }
            .sanitised(),
        );
        let by_email = paginate(
            directory(),
            &PageRequest {
                search: "kj@".to_owned(),
                ..PageRequest::default()
            }
            .sanitised(),
        );
        let by_role = paginate(
            directory(),
            &PageRequest {
                search: "administrator".to_owned(),
                ..PageRequest::default()
            }
            .sanitised(),
        );

        assert_eq!(names(&by_name), vec!["Alan Turing"]);
        assert_eq!(names(&by_email), vec!["Katherine Johnson"]);
        assert_eq!(
            by_role.total, 2,
            "the role search is a substring, so Administrator matches two"
        );
    }

    #[test]
    fn the_status_filter_narrows_and_is_case_insensitive() {
        let active = paginate(
            directory(),
            &PageRequest::default().filtered_by("status", "ACTIVE"),
        );

        assert_eq!(active.total, 2);
        assert!(
            active
                .rows
                .iter()
                .all(|row| matches!(row.status, UserStatusResource::Active))
        );
    }

    #[test]
    fn a_status_that_does_not_exist_narrows_to_nothing() {
        // The opposite of an unrecognised sort field, and deliberately so: the
        // caller named a value here, and handing back everybody would read as
        // "everybody is retired".
        let page = paginate(
            directory(),
            &PageRequest::default().filtered_by("status", "retired"),
        );

        assert_eq!(page.total, 0);
    }

    #[test]
    fn the_role_filter_matches_the_whole_name_rather_than_a_prefix() {
        let page = paginate(
            directory(),
            &PageRequest::default().filtered_by("role", "Admin"),
        );

        // Grace alone. "Administrator" starts with "Admin", and a substring
        // match would hand a script two people who hold different roles.
        assert_eq!(names(&page), vec!["Grace Hopper"]);
    }

    #[test]
    fn never_signed_in_sorts_to_the_top() {
        let mut rows = directory();
        rows[1].last_login_at = Some(Utc::now());

        let request = PageRequest {
            sort: Some(phonix_core::query::Sort::ascending("last_login_at")),
            ..PageRequest::default()
        };
        let page = paginate(rows, &request.sanitised());

        // Three nulls first, then the one person who has actually been here.
        assert_eq!(page.rows[3].display_name, "Grace Hopper");
        assert!(page.rows[..3].iter().all(|row| row.last_login_at.is_none()));
    }

    #[test]
    fn a_tie_is_broken_by_id_rather_than_left_to_chance() {
        // Everybody sorts equal on this field, so only the tie-break decides -
        // and it has to decide the same way twice, or paging through the list
        // shows a row on two pages and another on none.
        let rows = directory();
        let request = PageRequest::default().filtered_by("status", "active");
        let request = PageRequest {
            sort: Some(phonix_core::query::Sort::ascending("status")),
            ..request
        };

        let once = paginate(rows.clone(), &request.clone().sanitised());
        let twice = paginate(rows, &request.sanitised());

        assert_eq!(names(&once), names(&twice));
    }

    #[test]
    fn a_page_past_the_end_comes_back_clamped() {
        let request = PageRequest {
            page: 99,
            ..PageRequest::first(2)
        };

        let page = paginate(directory(), &request.sanitised());

        assert_eq!(
            page.page, 2,
            "four rows, two per page: the last page is the second"
        );
        assert_eq!(page.rows.len(), 2);
        assert_eq!(
            page.total, 4,
            "the total is what matched, not what fitted on the page"
        );
    }
}
