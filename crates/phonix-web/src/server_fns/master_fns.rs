//! Master data: the parties this workspace trades with, and its taxes.
//!
//! Thin, like every other file here: each one parses its input, calls **one**
//! use case, and maps the result. The permission is stated inside the service,
//! not here - a second check in this file would be a second place to forget,
//! and the first one is the one that runs whether the request came from this
//! page, a script, or a future API.
//!
//! # Why `list_parties` takes a role
//!
//! So that an app can ask for its own without knowing about anybody else's.
//! Books asks for `customer`; the master-data screen passes `None` and gets
//! them all. The alternative - a `list_customers` endpoint - would put Books'
//! vocabulary in `master`, which is the direction the whole boundary exists to
//! prevent.

use leptos::prelude::*;
use phonix_core::form::Submission;
use phonix_master::address::{PartyAddress, PartyAddressInput};
use phonix_master::contact::{PartyContact, PartyContactInput};
use phonix_master::party::{Party, PartyInput, PartySummary};
use phonix_tax::code::{TaxCode, TaxCodeInput, TaxCodeSummary};
use phonix_tax::group::{TaxGroup, TaxGroupInput};
use phonix_tax::rate::{TaxRateInput, TaxRateRow};
use uuid::Uuid;

// --- parties ------------------------------------------------------------

/// Every party, or only those an app has claimed.
#[server(name = ListParties, prefix = "/api", endpoint = "master/parties")]
pub async fn list_parties(role: Option<String>) -> Result<Vec<PartySummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::list(&pool, &caller, role.as_deref())
        .await
        .map_err(service_error)
}

/// One party, whole: its roles, addresses and contacts.
#[server(name = PartyDetail, prefix = "/api", endpoint = "master/parties/detail")]
pub async fn party_detail(party_id: Uuid) -> Result<Party, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::detail(&pool, &caller, party_id)
        .await
        .map_err(service_error)
}

/// The editable part of one party, for the form to open on.
///
/// A separate call from [`party_detail`] rather than a field picked out of it,
/// for the reason `user_edit` is separate from `list_users`: a detail is what a
/// screen *shows*, and a form that edited one would be editing the display.
#[server(name = PartyEdit, prefix = "/api", endpoint = "master/parties/edit")]
pub async fn party_edit(party_id: Uuid) -> Result<PartyInput, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::edit(&pool, &caller, party_id)
        .await
        .map_err(service_error)
}

/// Create a party, or store a changed one.
#[server(name = SaveParty, prefix = "/api", endpoint = "master/parties/save")]
pub async fn save_party(draft: PartyInput) -> Result<Submission<PartyInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::save(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// Remove a party no app has claimed.
#[server(name = DeleteParty, prefix = "/api", endpoint = "master/parties/delete")]
pub async fn delete_party(party_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::delete(&pool, &caller, party_id)
        .await
        .map_err(service_error)
}

/// Every address on a party.
#[server(name = PartyAddresses, prefix = "/api", endpoint = "master/parties/addresses")]
pub async fn party_addresses(party_id: Uuid) -> Result<Vec<PartyAddress>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::addresses(&pool, &caller, party_id)
        .await
        .map_err(service_error)
}

/// Add an address, or store a changed one.
#[server(name = SavePartyAddress, prefix = "/api", endpoint = "master/parties/addresses/save")]
pub async fn save_party_address(
    party_id: Uuid,
    draft: PartyAddressInput,
) -> Result<Submission<PartyAddressInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::save_address(&pool, &caller, party_id, draft)
        .await
        .map_err(service_error)
}

/// Remove an address.
#[server(name = DeletePartyAddress, prefix = "/api", endpoint = "master/parties/addresses/delete")]
pub async fn delete_party_address(party_id: Uuid, address_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::delete_address(&pool, &caller, party_id, address_id)
        .await
        .map_err(service_error)
}

/// Every contact on a party.
#[server(name = PartyContacts, prefix = "/api", endpoint = "master/parties/contacts")]
pub async fn party_contacts(party_id: Uuid) -> Result<Vec<PartyContact>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::contacts(&pool, &caller, party_id)
        .await
        .map_err(service_error)
}

/// Add a contact, or store a changed one.
#[server(name = SavePartyContact, prefix = "/api", endpoint = "master/parties/contacts/save")]
pub async fn save_party_contact(
    party_id: Uuid,
    draft: PartyContactInput,
) -> Result<Submission<PartyContactInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::save_contact(&pool, &caller, party_id, draft)
        .await
        .map_err(service_error)
}

/// Remove a contact.
#[server(name = DeletePartyContact, prefix = "/api", endpoint = "master/parties/contacts/delete")]
pub async fn delete_party_contact(party_id: Uuid, contact_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::party::delete_contact(&pool, &caller, party_id, contact_id)
        .await
        .map_err(service_error)
}

// --- taxes --------------------------------------------------------------

/// Every tax code this workspace has defined.
#[server(name = ListTaxCodes, prefix = "/api", endpoint = "master/taxes")]
pub async fn list_tax_codes() -> Result<Vec<TaxCode>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::list_codes(&pool, &caller)
        .await
        .map_err(service_error)
}

/// Every tax code, with what each is charged at today.
///
/// "Today" is decided on the server rather than in the browser, so a tab left
/// open across midnight and a fresh one agree about which rate is in force -
/// and so that the answer is the workspace's own day rather than the reader's.
#[server(name = ListTaxCodesToday, prefix = "/api", endpoint = "master/taxes/today")]
pub async fn list_tax_codes_today() -> Result<Vec<TaxCodeSummary>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;
    let today = chrono::Utc::now().date_naive();

    phonix_services::master::tax::list_codes_on(&pool, &caller, today)
        .await
        .map_err(service_error)
}

/// The editable part of one tax code.
#[server(name = TaxCodeEdit, prefix = "/api", endpoint = "master/taxes/edit")]
pub async fn tax_code_edit(tax_code_id: Uuid) -> Result<TaxCodeInput, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::edit_code(&pool, &caller, tax_code_id)
        .await
        .map_err(service_error)
}

/// Create a tax code, or store a changed one.
#[server(name = SaveTaxCode, prefix = "/api", endpoint = "master/taxes/save")]
pub async fn save_tax_code(draft: TaxCodeInput) -> Result<Submission<TaxCodeInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::save_code(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// Remove a tax code that is in no group.
#[server(name = DeleteTaxCode, prefix = "/api", endpoint = "master/taxes/delete")]
pub async fn delete_tax_code(tax_code_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::delete_code(&pool, &caller, tax_code_id)
        .await
        .map_err(service_error)
}

/// Every rate window on one tax code, newest first.
#[server(name = TaxRates, prefix = "/api", endpoint = "master/taxes/rates")]
pub async fn tax_rates(tax_code_id: Uuid) -> Result<Vec<TaxRateRow>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::rates_of(&pool, &caller, tax_code_id)
        .await
        .map_err(service_error)
}

/// Add a rate window, or move one that exists.
#[server(name = SaveTaxRate, prefix = "/api", endpoint = "master/taxes/rates/save")]
pub async fn save_tax_rate(
    tax_code_id: Uuid,
    rate_id: Option<Uuid>,
    draft: TaxRateInput,
) -> Result<Submission<TaxRateInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::save_rate(&pool, &caller, tax_code_id, rate_id, draft)
        .await
        .map_err(service_error)
}

/// Remove a rate window.
#[server(name = DeleteTaxRate, prefix = "/api", endpoint = "master/taxes/rates/delete")]
pub async fn delete_tax_rate(tax_code_id: Uuid, rate_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::delete_rate(&pool, &caller, tax_code_id, rate_id)
        .await
        .map_err(service_error)
}

/// Every tax group, with its members in sequence order.
#[server(name = ListTaxGroups, prefix = "/api", endpoint = "master/tax-groups")]
pub async fn list_tax_groups() -> Result<Vec<TaxGroup>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::list_groups(&pool, &caller)
        .await
        .map_err(service_error)
}

/// The editable part of one tax group.
#[server(name = TaxGroupEdit, prefix = "/api", endpoint = "master/tax-groups/edit")]
pub async fn tax_group_edit(tax_group_id: Uuid) -> Result<TaxGroupInput, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::edit_group(&pool, &caller, tax_group_id)
        .await
        .map_err(service_error)
}

/// Create a tax group, or store a changed one.
///
/// The membership arrives as the whole ordered list, not a diff - see the
/// note at the top of `admin_fns` for why every editor in this application
/// submits a set.
#[server(name = SaveTaxGroup, prefix = "/api", endpoint = "master/tax-groups/save")]
pub async fn save_tax_group(
    draft: TaxGroupInput,
) -> Result<Submission<TaxGroupInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::save_group(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// Remove a tax group.
#[server(name = DeleteTaxGroup, prefix = "/api", endpoint = "master/tax-groups/delete")]
pub async fn delete_tax_group(tax_group_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::master::tax::delete_group(&pool, &caller, tax_group_id)
        .await
        .map_err(service_error)
}
