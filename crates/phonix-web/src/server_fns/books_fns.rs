//! Sales invoices.
//!
//! # There is no endpoint that prices an invoice
//!
//! Deliberately. The browser prices it locally with `app_books::pricing`, which
//! is the same code the server posts with - that is the whole reason the crate
//! compiles to wasm. A "calculate totals" round trip would be a second
//! implementation of the arithmetic living in the network, and the first thing
//! to disagree with the document.
//!
//! What the browser does need is the resolved tax treatments, which depend on
//! the document's date and on a rate table it cannot see. [`tax_treatments`]
//! hands them over once, and everything after that is local.

use app_books::invoice::{Invoice, InvoiceInput, InvoiceStatus, InvoiceSummary, PostOutcome};
use chrono::NaiveDate;
use leptos::prelude::*;
use phonix_core::form::Submission;
use phonix_tax::group::TaxTreatment;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which invoices a screen is asking for.
///
/// Crosses the wire as one value rather than four arguments, for the reason the
/// repository takes one: a status passed where a party id goes would compile
/// and would list the wrong documents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceQuery {
    pub party_id: Option<Uuid>,
    pub status: Option<InvoiceStatus>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// Every invoice a list screen should show.
#[server(name = ListInvoices, prefix = "/api", endpoint = "books/invoices")]
pub async fn list_invoices(query: InvoiceQuery) -> Result<Vec<InvoiceSummary>, ServerFnError> {
    use phonix_db::books::invoice::InvoiceFilter;

    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::list(
        &pool,
        &caller,
        InvoiceFilter {
            party_id: query.party_id,
            status: query.status,
            from: query.from,
            to: query.to,
            search: None,
        },
    )
    .await
    .map_err(service_error)
}

/// One invoice, whole: its lines and the tax each carried.
#[server(name = InvoiceDetail, prefix = "/api", endpoint = "books/invoices/detail")]
pub async fn invoice_detail(invoice_id: Uuid) -> Result<Invoice, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::find(&pool, &caller, invoice_id)
        .await
        .map_err(service_error)
}

/// The editable part of one invoice, for the form to open on.
#[server(name = InvoiceEdit, prefix = "/api", endpoint = "books/invoices/edit")]
pub async fn invoice_edit(invoice_id: Uuid) -> Result<InvoiceInput, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::edit(&pool, &caller, invoice_id)
        .await
        .map_err(service_error)
}

/// Create a draft, or rewrite one.
#[server(name = SaveInvoice, prefix = "/api", endpoint = "books/invoices/save")]
pub async fn save_invoice(draft: InvoiceInput) -> Result<Submission<InvoiceInput>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::save(&pool, &caller, draft)
        .await
        .map_err(service_error)
}

/// Number a draft and issue it.
///
/// Comes back as a [`PostOutcome`] rather than a `Result`, because two of its
/// three answers are things a screen renders beside the button: somebody else
/// posted it, and this workspace has no number series set up.
#[server(name = PostInvoice, prefix = "/api", endpoint = "books/invoices/post")]
pub async fn post_invoice(invoice_id: Uuid) -> Result<PostOutcome, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::post(&pool, &caller, invoice_id)
        .await
        .map_err(service_error)
}

/// Withdraw a posted invoice. It keeps its number.
#[server(name = VoidInvoice, prefix = "/api", endpoint = "books/invoices/void")]
pub async fn void_invoice(invoice_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::void(&pool, &caller, invoice_id)
        .await
        .map_err(service_error)
}

/// Remove a draft.
#[server(name = DeleteInvoice, prefix = "/api", endpoint = "books/invoices/delete")]
pub async fn delete_invoice(invoice_id: Uuid) -> Result<(), ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::delete(&pool, &caller, invoice_id)
        .await
        .map_err(service_error)
}

/// Every active tax treatment, resolved for a document date.
///
/// Fetched once when the editor opens or the date changes, and then the browser
/// prices every line locally. That is what makes the totals appear as somebody
/// types rather than a third of a second after they stop.
#[server(name = TaxTreatments, prefix = "/api", endpoint = "books/treatments")]
pub async fn tax_treatments(on: NaiveDate) -> Result<Vec<TaxTreatment>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::books::invoice::treatments(&pool, &caller, on)
        .await
        .map_err(service_error)
}
