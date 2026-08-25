//! Parties: the organizations and people a workspace trades with.
//!
//! # Deleting one is refused more often than it is allowed
//!
//! Nothing in `master` points at a party except its own children. An app that
//! has raised documents against one holds its id *without* a foreign key -
//! that is the no-cross-schema-FK rule, and it is what makes an app
//! uninstallable. So the database cannot answer "is this party in use", and
//! [`delete`] does not pretend otherwise: it refuses a party that any app has
//! claimed a role on, and offers deactivation instead.
//!
//! That is deliberately conservative. A party with a `customer` role has been
//! used by Books as far as anything here can tell, and a customer deleted out
//! from under a posted invoice is a document that can no longer name who it was
//! for.
//!
//! # Addresses and contacts are their own saves
//!
//! Not fields on the party form, for the reason the organization's logo is not
//! a field on the organization form: a draft opened before somebody else
//! corrected a postcode would otherwise put the old one back as a side effect
//! of changing a phone number.

use phonix_core::form::Submission;
use phonix_core::msg;
use phonix_core::permissions;
use phonix_db::error::DbError;
use phonix_db::master::party as store;
use phonix_db::sqlx::PgPool;
use phonix_master::address::{PartyAddress, PartyAddressInput};
use phonix_master::contact::{PartyContact, PartyContactInput};
use phonix_master::party::{Party, PartyInput, PartyRole, PartySummary};
use uuid::Uuid;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};

/// Every party, or only the ones an app has claimed.
///
/// `role` is how Books asks for customers without knowing anything about
/// suppliers. Passing `None` is the master-data screen, which shows all of
/// them because that is what it is for.
pub async fn list(
    pool: &PgPool,
    caller: &Caller,
    role: Option<&str>,
) -> ServiceResult<Vec<PartySummary>> {
    caller.require(permissions::PARTIES)?;
    Ok(store::list(pool, role).await?)
}

/// One party, whole: its roles, addresses and contacts.
///
/// Gated on `Parties` rather than `Parties.Edit`, matching the role editor:
/// reading a party is part of being able to see the list, and somebody who
/// cannot edit gets a form of disabled controls rather than a refusal.
pub async fn detail(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<Party> {
    caller.require(permissions::PARTIES)?;

    store::find(pool, id)
        .await?
        .ok_or_else(|| ServiceError::rejected("party", msg!("error.party.gone")))
}

/// The editable part of one party, for the form to open on.
pub async fn edit(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<PartyInput> {
    Ok(PartyInput::from_party(&detail(pool, caller, id).await?))
}

/// Create a party, or change one that exists.
///
/// Which of the two it is comes from the draft: `id` absent means create. That
/// is the form's own answer rather than a second parameter, so a screen cannot
/// open the create form and submit it against an existing party.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    draft: PartyInput,
) -> ServiceResult<Submission<PartyInput>> {
    match draft.id {
        None => create(pool, caller, draft).await,
        Some(id) => update(pool, caller, id, draft).await,
    }
}

async fn create(
    pool: &PgPool,
    caller: &Caller,
    draft: PartyInput,
) -> ServiceResult<Submission<PartyInput>> {
    caller.require(permissions::PARTIES_CREATE)?;
    acting_user(caller)?;

    // The same rules the browser applied, applied again. The browser's check is
    // a courtesy; this one is the control.
    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected(err.field(), err.message())),
    };

    let id = match store::insert(pool, &checked, caller.user_id()).await {
        Ok(id) => id,
        Err(DbError::CodeExists { code, .. }) => return Ok(code_taken(&code)),
        Err(err) => return Err(err.into()),
    };

    store::set_roles(pool, id, &checked.roles).await?;

    let stored = PartyInput {
        id: Some(id),
        ..checked
    };

    audit::created(
        pool,
        caller,
        Target::new(kinds::PARTY, id)
            .named(&stored.name)
            .fact("code", &stored.code),
        &stored,
    )
    .await;

    Ok(Submission::Saved(stored))
}

async fn update(
    pool: &PgPool,
    caller: &Caller,
    id: Uuid,
    draft: PartyInput,
) -> ServiceResult<Submission<PartyInput>> {
    caller.require(permissions::PARTIES_EDIT)?;
    acting_user(caller)?;

    let Some(existing) = store::find(pool, id).await? else {
        return Ok(Submission::rejected("party", msg!("error.party.gone")));
    };
    let before = PartyInput::from_party(&existing);

    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected(err.field(), err.message())),
    };

    match store::update(pool, id, &checked, caller.user_id()).await {
        Ok(true) => {}
        Ok(false) => return Ok(Submission::rejected("party", msg!("error.party.gone"))),
        Err(DbError::CodeExists { code, .. }) => return Ok(code_taken(&code)),
        Err(err) => return Err(err.into()),
    }

    store::set_roles(pool, id, &checked.roles).await?;

    // Re-read rather than echoed, for the reason the role editor re-reads: the
    // database normalises, and a form showing the draft would show a change
    // that did not happen.
    let after = edit(pool, caller, id).await?;

    // The whole value on each side rather than a hand-listed set of fields, so
    // a column added to a party later is in the diff without anybody
    // remembering to add it here.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::PARTY, id).named(&after.name),
        &before,
        &after,
    )
    .await;

    Ok(Submission::Saved(after))
}

/// Remove a party.
///
/// Refused while any app has claimed a role on it - see the module note. The
/// alternative offered is deactivation, which keeps every document that names
/// the party able to resolve it.
pub async fn delete(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<()> {
    caller.require(permissions::PARTIES_DELETE)?;
    acting_user(caller)?;

    let Some(party) = store::find(pool, id).await? else {
        return Err(ServiceError::rejected("party", msg!("error.party.gone")));
    };

    if let Some(role) = party.roles.first() {
        return Err(ServiceError::rejected(
            "party",
            msg!("error.party.in_use", role = role.as_str()),
        ));
    }

    store::delete(pool, id).await?;

    audit::deleted(
        pool,
        caller,
        Target::new(kinds::PARTY, id)
            .named(&party.name)
            .fact("code", &party.code),
        &PartyInput::from_party(&party),
    )
    .await;

    Ok(())
}

/// Claim a party for an app, leaving whatever else is claimed alone.
///
/// What Books calls when an invoice is raised against a party Procurement
/// added. Gated on `Parties.Edit` rather than on anything of the app's own: it
/// writes to master data, and the permission follows the table rather than the
/// caller.
pub async fn claim_role(
    pool: &PgPool,
    caller: &Caller,
    id: Uuid,
    role: &PartyRole,
) -> ServiceResult<()> {
    caller.require(permissions::PARTIES_EDIT)?;
    Ok(store::claim_role(pool, id, role).await?)
}

// --- addresses ----------------------------------------------------------

/// Every address on a party.
pub async fn addresses(
    pool: &PgPool,
    caller: &Caller,
    id: Uuid,
) -> ServiceResult<Vec<PartyAddress>> {
    caller.require(permissions::PARTIES)?;
    Ok(store::addresses_of(pool, id).await?)
}

/// Add an address, or change one that exists.
pub async fn save_address(
    pool: &PgPool,
    caller: &Caller,
    party_id: Uuid,
    draft: PartyAddressInput,
) -> ServiceResult<Submission<PartyAddressInput>> {
    caller.require(permissions::PARTIES_EDIT)?;
    acting_user(caller)?;

    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected("address", err.message())),
    };

    let Some(party) = store::find(pool, party_id).await? else {
        return Ok(Submission::rejected("party", msg!("error.party.gone")));
    };
    let before = party.addresses.clone();

    let id = store::save_address(pool, party_id, &checked, caller.user_id()).await?;
    let after = store::addresses_of(pool, party_id).await?;

    // Recorded against the *party*, not against the address. "Who changed
    // Acme's billing address" is the question somebody asks, and an address
    // with a history of its own is a history nobody navigates to.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::PARTY, party_id)
            .named(&party.name)
            .fact("addresses", after.len()),
        &before,
        &after,
    )
    .await;

    Ok(Submission::Saved(PartyAddressInput {
        id: Some(id),
        ..checked
    }))
}

/// Remove an address.
pub async fn delete_address(
    pool: &PgPool,
    caller: &Caller,
    party_id: Uuid,
    address_id: Uuid,
) -> ServiceResult<()> {
    caller.require(permissions::PARTIES_EDIT)?;
    acting_user(caller)?;

    let Some(party) = store::find(pool, party_id).await? else {
        return Err(ServiceError::rejected("party", msg!("error.party.gone")));
    };
    let before = party.addresses.clone();

    if !store::delete_address(pool, party_id, address_id).await? {
        return Err(ServiceError::rejected(
            "address",
            msg!("error.party.address_gone"),
        ));
    }

    let after = store::addresses_of(pool, party_id).await?;
    audit::updated(
        pool,
        caller,
        Target::new(kinds::PARTY, party_id).named(&party.name),
        &before,
        &after,
    )
    .await;

    Ok(())
}

// --- contacts -----------------------------------------------------------

/// Every contact on a party.
pub async fn contacts(
    pool: &PgPool,
    caller: &Caller,
    id: Uuid,
) -> ServiceResult<Vec<PartyContact>> {
    caller.require(permissions::PARTIES)?;
    Ok(store::contacts_of(pool, id).await?)
}

/// Add a contact, or change one that exists.
pub async fn save_contact(
    pool: &PgPool,
    caller: &Caller,
    party_id: Uuid,
    draft: PartyContactInput,
) -> ServiceResult<Submission<PartyContactInput>> {
    caller.require(permissions::PARTIES_EDIT)?;
    acting_user(caller)?;

    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected(err.field(), err.message())),
    };

    let Some(party) = store::find(pool, party_id).await? else {
        return Ok(Submission::rejected("party", msg!("error.party.gone")));
    };
    let before = party.contacts.clone();

    let id = store::save_contact(pool, party_id, &checked, caller.user_id()).await?;
    let after = store::contacts_of(pool, party_id).await?;

    audit::updated(
        pool,
        caller,
        Target::new(kinds::PARTY, party_id)
            .named(&party.name)
            .fact("contacts", after.len()),
        &before,
        &after,
    )
    .await;

    Ok(Submission::Saved(PartyContactInput {
        id: Some(id),
        ..checked
    }))
}

/// Remove a contact.
pub async fn delete_contact(
    pool: &PgPool,
    caller: &Caller,
    party_id: Uuid,
    contact_id: Uuid,
) -> ServiceResult<()> {
    caller.require(permissions::PARTIES_EDIT)?;
    acting_user(caller)?;

    let Some(party) = store::find(pool, party_id).await? else {
        return Err(ServiceError::rejected("party", msg!("error.party.gone")));
    };
    let before = party.contacts.clone();

    if !store::delete_contact(pool, party_id, contact_id).await? {
        return Err(ServiceError::rejected(
            "contact",
            msg!("error.party.contact_gone"),
        ));
    }

    let after = store::contacts_of(pool, party_id).await?;
    audit::updated(
        pool,
        caller,
        Target::new(kinds::PARTY, party_id).named(&party.name),
        &before,
        &after,
    )
    .await;

    Ok(())
}

/// "That code is taken", said next to the box it was typed into.
fn code_taken(code: &str) -> Submission<PartyInput> {
    Submission::rejected("code", msg!("error.party.code_taken", code = code))
}
