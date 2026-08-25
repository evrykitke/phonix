//! Raising, pricing, posting and voiding a sales invoice.
//!
//! # Four permissions, because they are four different powers
//!
//! Reading a customer's invoices is not raising one, raising one is not
//! *posting* it - which takes a number nobody can hand back - and posting is
//! not voiding a document that has already been sent.
//!
//! # Posting is the only irreversible act
//!
//! [`post`] takes a number from `core.number_sequences` in the same transaction
//! as the write, so a failure returns the number rather than burning it. What
//! it cannot undo is the *decision*: after it, the document is frozen and a
//! mistake is corrected by voiding it and raising another. That is not a
//! limitation, it is what makes an invoice evidence.
//!
//! # The snapshot happens here
//!
//! The party's name and address are copied onto the draft every time it is
//! saved, and the tax rates are resolved against the document's own date. By
//! the time an invoice is posted, nothing on it needs looking up again.

use app_books::invoice::{
    CheckedInvoice, Invoice, InvoiceInput, InvoiceStatus, InvoiceSummary, PartySnapshot,
    PostOutcome,
};
use app_books::pricing::{PricedInvoice, PricedLine};
use chrono::NaiveDate;
use phonix_core::audit::kinds;
use phonix_core::form::Submission;
use phonix_core::locale::Currency;
use phonix_core::money::{Money, Rounding};
use phonix_core::permissions;
use phonix_core::{Message, msg};
use phonix_db::books::invoice as store;
use phonix_db::books::invoice::{DraftWrite, InvoiceFilter};
use phonix_db::error::DbError;
use phonix_db::numbering::SequenceKey;
use phonix_db::sqlx::PgPool;
use phonix_master::address::AddressPurpose;
use phonix_tax::compute::DocumentTax;
use phonix_tax::group::TaxTreatment;
use uuid::Uuid;

use crate::audit::{self, Target};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};

/// Every invoice a list screen should show.
pub async fn list(
    pool: &PgPool,
    caller: &Caller,
    filter: InvoiceFilter<'_>,
) -> ServiceResult<Vec<InvoiceSummary>> {
    caller.require(permissions::INVOICES)?;
    Ok(store::list(pool, filter).await?)
}

/// One invoice, whole.
///
/// Gated on `Invoices` rather than `Invoices.Edit`, matching every other detail
/// screen here: reading a document is part of being able to see the list, and
/// somebody who cannot edit gets a read-only screen rather than a refusal.
pub async fn find(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<Invoice> {
    caller.require(permissions::INVOICES)?;

    store::find(pool, id)
        .await?
        .ok_or_else(|| ServiceError::rejected("invoice", msg!("books.error.gone")))
}

/// The editable part of one invoice, for the form to open on.
pub async fn edit(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<InvoiceInput> {
    Ok(InvoiceInput::from_invoice(&find(pool, caller, id).await?))
}

/// Create a draft, or rewrite one.
///
/// Which of the two it is comes from the draft: `id` absent means create. That
/// is the form's own answer rather than a second parameter, so a screen cannot
/// open the create form and submit it against an existing invoice.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    draft: InvoiceInput,
) -> ServiceResult<Submission<InvoiceInput>> {
    caller.require(if draft.id.is_none() {
        permissions::INVOICES_CREATE
    } else {
        permissions::INVOICES_EDIT
    })?;
    // A document must be attributable: `Caller::System` has no account behind
    // it, and an invoice nobody raised is one nobody can be asked about.
    acting_user(caller)?;

    // The same rules the browser applied, applied again. The browser's check is
    // a courtesy; this one is the control.
    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected(err.field(), err.message())),
    };

    let party = match snapshot_party(pool, checked.party_id).await? {
        Some(party) => party,
        None => {
            return Ok(Submission::rejected(
                "party_id",
                msg!("books.error.party_gone"),
            ));
        }
    };

    let priced = match price(pool, &checked).await? {
        Ok(priced) => priced,
        Err(rejection) => return Ok(rejection.into_submission()),
    };

    let existed = checked.id;
    let id = match store::save_draft(
        pool,
        DraftWrite {
            checked: &checked,
            party: &party,
            priced: &priced,
            actor: caller.user_id(),
        },
    )
    .await
    {
        Ok(id) => id,
        // The `WHERE status = 'draft'` matched nothing. An expected path: two
        // tabs, and the other one posted it.
        Err(DbError::InvoiceNotEditable) => {
            return Ok(Submission::rejected(
                "status",
                msg!("books.error.not_editable"),
            ));
        }
        Err(err) => return Err(err.into()),
    };

    // Books claims the party as a customer.
    //
    // Through the repository rather than through
    // `master::party::claim_role`, which requires `Parties.Edit` - and being
    // allowed to raise an invoice against somebody *is* the authority to mark
    // them a customer. Requiring the master-data permission as well would mean
    // nobody in sales could invoice anyone.
    //
    // It is what stops the party being deleted out from under a document
    // later: `master` has no foreign key into `books` - on purpose - so a role
    // is the only way it can know.
    if let Ok(role) = phonix_master::party::PartyRole::parse(app_books::CUSTOMER_ROLE) {
        phonix_db::master::party::claim_role(pool, checked.party_id, &role).await?;
    }

    // Re-read rather than the draft echoed back, for the reason every save in
    // this application re-reads: the totals were computed here and the line
    // numbers were assigned by the store, and a form showing the draft would
    // show neither.
    let after = edit(pool, caller, id).await?;

    match existed {
        Some(_) => {
            audit::updated(
                pool,
                caller,
                Target::new(kinds::SALES_INVOICE, id)
                    .named(&party.name)
                    .fact("total", priced.gross.to_display_string()),
                &draft,
                &after,
            )
            .await;
        }
        None => {
            audit::created(
                pool,
                caller,
                Target::new(kinds::SALES_INVOICE, id)
                    .named(&party.name)
                    .fact("total", priced.gross.to_display_string())
                    // A draft carries no number, and saying so stops the trail
                    // reading as though a document had been issued.
                    .fact("number", "none until it is posted"),
                &after,
            )
            .await;
        }
    }

    Ok(Submission::Saved(after))
}

/// Number the invoice and freeze it.
///
/// # Why the whole thing is one transaction
///
/// The `UPDATE` that allocates a number takes a row lock Postgres holds until
/// the transaction ends. Allocating and storing in the same transaction is what
/// makes a failed post *return* the number: a retry cannot burn one, and the
/// sequence stays gap-free. Allocating first and storing afterwards would leave
/// a number handed out to a document that was never written.
pub async fn post(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<PostOutcome> {
    caller.require(permissions::INVOICES_POST)?;
    acting_user(caller)?;

    let invoice = find(pool, caller, id).await?;
    if invoice.status != InvoiceStatus::Draft {
        return Ok(PostOutcome::NotADraft);
    }

    // The conversion snapshot, worked out before the transaction opens: it
    // reads two tables and does not need the sequence's row lock held while it
    // does. Every document of one type queues through that one row, so anything
    // that can happen outside it should.
    let conversion = conversion_for(pool, &invoice).await?;

    let generator = crate::numbering::NumberGenerator::open(pool).await?;
    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;

    let key = SequenceKey::new(app_books::APP_ID, app_books::SALES_INVOICE);
    let allocated = match generator.next(&mut tx, key, invoice.issued_on).await {
        Ok(allocated) => allocated,
        // The series is missing or switched off. Rolled back rather than left
        // half-open, and reported as an outcome because the fix is a settings
        // screen rather than a retry.
        Err(ServiceError::Db(DbError::UnusableSequence { .. })) => {
            tx.rollback().await.map_err(phonix_db::DbError::Query)?;
            return Ok(PostOutcome::NoSeries);
        }
        Err(err) => return Err(err),
    };

    let stored = store::post(
        &mut tx,
        id,
        &allocated.number,
        conversion.as_ref().map(|(rate, _)| rate),
        conversion.as_ref().map(|(_, base)| *base),
        caller.user_id(),
    )
    .await?;

    if !stored {
        // Somebody else posted it between the read and the write. Rolling back
        // returns the number, which is the whole reason these are one
        // transaction.
        tx.rollback().await.map_err(phonix_db::DbError::Query)?;
        return Ok(PostOutcome::NotADraft);
    }

    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    // Recorded after the commit, and best-effort like every audit write: losing
    // a trail row is bad, refusing a post because the trail is unwritable is
    // worse.
    let after = find(pool, caller, id).await?;
    audit::updated(
        pool,
        caller,
        Target::new(kinds::SALES_INVOICE, id)
            .named(&invoice.party.name)
            .fact("number", &allocated.number)
            .fact("total", invoice.totals.gross.to_display_string()),
        &InvoiceStatus::Draft,
        &after.status,
    )
    .await;

    Ok(PostOutcome::Posted {
        number: allocated.number,
    })
}

/// Withdraw a posted invoice.
///
/// It keeps its number: a number that disappears is a gap, and a gap is what an
/// auditor asks about. What it loses is its claim on anybody.
pub async fn void(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<()> {
    caller.require(permissions::INVOICES_VOID)?;
    acting_user(caller)?;

    let invoice = find(pool, caller, id).await?;
    if invoice.status != InvoiceStatus::Posted {
        return Err(ServiceError::rejected(
            "status",
            msg!("books.error.not_voidable"),
        ));
    }

    if !store::void(pool, id, caller.user_id()).await? {
        return Err(ServiceError::rejected(
            "status",
            msg!("books.error.not_voidable"),
        ));
    }

    audit::updated(
        pool,
        caller,
        Target::new(kinds::SALES_INVOICE, id)
            .named(&invoice.party.name)
            // Recorded because the document keeps it, and "which number was
            // withdrawn" is the question a gap in the sequence provokes.
            .fact("number", invoice.number.clone().unwrap_or_default())
            .fact("total", invoice.totals.gross.to_display_string()),
        &InvoiceStatus::Posted,
        &InvoiceStatus::Voided,
    )
    .await;

    Ok(())
}

/// Remove a draft.
///
/// Only a draft, and the statement says so too. A posted invoice has a number,
/// and a numbered document that vanishes is the gap the sequence design exists
/// to prevent - voiding is what withdraws one.
pub async fn delete(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<()> {
    caller.require(permissions::INVOICES_EDIT)?;
    acting_user(caller)?;

    let invoice = find(pool, caller, id).await?;
    if !invoice.is_editable() {
        return Err(ServiceError::rejected(
            "status",
            msg!("books.error.not_deletable"),
        ));
    }

    if !store::delete_draft(pool, id).await? {
        return Err(ServiceError::rejected(
            "status",
            msg!("books.error.not_deletable"),
        ));
    }

    audit::deleted(
        pool,
        caller,
        Target::new(kinds::SALES_INVOICE, id)
            .named(&invoice.party.name)
            .fact("total", invoice.totals.gross.to_display_string()),
        &InvoiceInput::from_invoice(&invoice),
    )
    .await;

    Ok(())
}

/// Every active tax treatment, resolved for a date.
///
/// What a document screen fetches once so the browser can price locally. Gated
/// on `Invoices` rather than on `Master.Taxes`: this is somebody raising an
/// invoice, not somebody editing the tax tables.
pub async fn treatments(
    pool: &PgPool,
    caller: &Caller,
    on: NaiveDate,
) -> ServiceResult<Vec<TaxTreatment>> {
    caller.require(permissions::INVOICES)?;
    crate::master::tax::treatments_on(pool, on).await
}

// --- the work ------------------------------------------------------------

/// Copy the customer onto the document.
///
/// The *registered* name where there is one, because an invoice is a legal
/// instrument and names the entity rather than its trading style. The billing
/// address, falling back the way [`Party::address_for`] falls back.
async fn snapshot_party(pool: &PgPool, party_id: Uuid) -> ServiceResult<Option<PartySnapshot>> {
    let Some(party) = phonix_db::master::party::find(pool, party_id).await? else {
        return Ok(None);
    };

    Ok(Some(PartySnapshot {
        party_id: party.id,
        code: party.code.clone(),
        name: party.document_name().to_owned(),
        tax_id: party.tax_id.clone(),
        address: party.postal_address(AddressPurpose::Billing),
    }))
}

/// Why an invoice could not be priced, in a shape a form can render.
enum Rejection {
    Message(&'static str, Message),
}

impl Rejection {
    fn into_submission(self) -> Submission<InvoiceInput> {
        match self {
            Self::Message(field, message) => Submission::rejected(field, message),
        }
    }
}

/// Resolve every line's tax treatment and add the document up.
///
/// The treatments are resolved against the **document's own date**, not today:
/// a backdated invoice is charged at the rate that was in force when it was
/// issued, which is the whole reason rates are effective-dated.
async fn price(
    pool: &PgPool,
    checked: &CheckedInvoice,
) -> ServiceResult<Result<DocumentTax, Rejection>> {
    let mut lines = Vec::with_capacity(checked.lines.len());

    for line in &checked.lines {
        let treatment = match line.tax_group_id {
            // A line outside the scope of tax. Not the same as a zero-rated
            // one, which is a group whose rate is zero.
            None => TaxTreatment::none(),
            Some(group_id) => {
                match crate::master::tax::treatment_on(pool, group_id, checked.issued_on).await {
                    Ok(treatment) => treatment,
                    // The group is gone, switched off, or one of its taxes has
                    // no rate on that date. All three are things to fix on the
                    // line, and the tax service already said which.
                    Err(ServiceError::Rejected(errors)) => {
                        let message = errors.first().map_or_else(
                            || msg!("books.error.cannot_price"),
                            |e| e.message.clone(),
                        );
                        return Ok(Err(Rejection::Message("lines", message)));
                    }
                    Err(err) => return Err(err),
                }
            }
        };

        lines.push(PricedLine {
            quantity: line.quantity,
            unit_price: line.unit_price,
            treatment,
        });
    }

    let priced = PricedInvoice {
        currency: checked.currency,
        pricing: checked.pricing,
        rounding_level: checked.rounding_level,
        rounding: checked.rounding,
        lines,
    };

    match priced.compute() {
        Ok(totals) => Ok(Ok(totals)),
        Err(err) => Ok(Err(Rejection::Message("lines", err.message()))),
    }
}

/// The conversion snapshot, when the invoice is not in the workspace's own
/// currency.
///
/// `None` when it is: there is nothing to convert, and a rate of one is not
/// evidence of a quotation somebody published.
///
/// A missing rate is refused rather than assumed. An invoice whose base amount
/// was invented is one that reconciles against nothing, and the fix - record
/// the rate - is a screen away.
async fn conversion_for(
    pool: &PgPool,
    invoice: &Invoice,
) -> ServiceResult<Option<(phonix_core::money::ExchangeRate, Money)>> {
    let base: Currency = phonix_db::organization::load(pool).await?.profile.currency;
    if base == invoice.currency {
        return Ok(None);
    }

    let Some(rate) =
        crate::currency::rate_on(pool, invoice.currency, base, invoice.issued_on, None).await?
    else {
        return Err(ServiceError::rejected(
            "currency",
            msg!(
                "books.error.no_rate",
                pair = format!("{}/{}", invoice.currency.code(), base.code())
            ),
        ));
    };

    // Converted once, from the document's own total, and stored beside the rate
    // it was converted at. Recomputing this later from today's rate is the
    // classic bug and it silently rewrites history.
    let converted = invoice
        .totals
        .gross
        .convert(&rate, Rounding::HalfUp)
        .map_err(|err| ServiceError::rejected("currency", err.message()))?;

    Ok(Some((rate, converted.base_amount)))
}
