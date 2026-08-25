//! `books.invoices`, its lines, and the tax each line carried.
//!
//! # A draft is rewritten; a posted invoice is not
//!
//! [`save_draft`] deletes the lines and writes them again, which is the right
//! shape for a form that submits the whole document: reconciling a reordering
//! in SQL is how a line ends up under the wrong number. It refuses to touch
//! anything that is not a draft, in the statement as well as in the service -
//! `WHERE status = 'draft'` is what makes that true of the database rather than
//! only of this codebase.
//!
//! # Amounts cross as text
//!
//! `NUMERIC` has no lossless integer binding in the driver, and the whole point
//! of [`Money`] is that it is exact. So every amount is bound as `$n::numeric`
//! and read back with `::text`, exactly as `core.exchange_rates` does.

use app_books::invoice::{
    CheckedInvoice, Invoice, InvoiceLine, InvoiceStatus, InvoiceSummary, InvoiceTotals,
    LineTaxSnapshot, PartySnapshot,
};
use app_books::quantity::Quantity;
use chrono::NaiveDate;
use phonix_core::identity::UserId;
use phonix_core::locale::{Country, Currency};
use phonix_core::money::{ExchangeRate, Money, Rate, Rounding};
use phonix_master::address::PostalAddress;
use phonix_tax::code::TaxKind;
use phonix_tax::compute::{DocumentTax, Pricing, RoundingLevel};
use phonix_tax::group::AppliedTax;
use phonix_tax::rate::TaxRate;
use sqlx::{FromRow, PgConnection, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// Which invoices a screen is asking for.
///
/// A struct rather than four arguments so a caller cannot pass a status where a
/// party id goes - both would compile, and the result is a list of the wrong
/// documents.
#[derive(Debug, Clone, Copy, Default)]
pub struct InvoiceFilter<'a> {
    /// Only this customer's.
    pub party_id: Option<Uuid>,
    /// Only documents in this state.
    pub status: Option<InvoiceStatus>,
    /// Issued on or after.
    pub from: Option<NaiveDate>,
    /// Issued on or before.
    pub to: Option<NaiveDate>,
    /// Kept so the type is not a lifetime-free struct today and a breaking
    /// change tomorrow, when a search term arrives.
    pub search: Option<&'a str>,
}

/// A list row.
struct SummaryRow(InvoiceSummary);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for SummaryRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let currency = currency_of(row, "currency_code")?;
        let stored_status: String = row.try_get("status")?;

        // Refused rather than defaulted: the status decides whether a document
        // is owed, editable and countable, and guessing would put a draft in a
        // revenue figure.
        let status =
            InvoiceStatus::parse(&stored_status).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "status".to_owned(),
                source: format!("unrecognised invoice status '{stored_status}'").into(),
            })?;

        Ok(Self(InvoiceSummary {
            id: row.try_get("id")?,
            number: row.try_get("number")?,
            status,
            party_id: row.try_get("party_id")?,
            party_name: row.try_get("party_name")?,
            issued_on: row.try_get("issued_on")?,
            due_on: row.try_get("due_on")?,
            currency,
            net: money_of(row, "net_amount", currency)?,
            tax: money_of(row, "tax_amount", currency)?,
            gross: money_of(row, "gross_amount", currency)?,
            line_count: row.try_get("line_count")?,
        }))
    }
}

/// Every invoice a list screen should show.
///
/// Reads the header only, plus a count of the lines. A list that carried every
/// line would fetch three tables to draw a total, and the total is already on
/// the header - stored, not recomputed.
pub async fn list<'e, E>(
    executor: E,
    filter: InvoiceFilter<'_>,
) -> Result<Vec<InvoiceSummary>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, SummaryRow>(
        "SELECT i.id, i.number, i.status, i.party_id, i.party_name,
                i.issued_on, i.due_on, i.currency_code,
                i.net_amount::text   AS net_amount,
                i.tax_amount::text   AS tax_amount,
                i.gross_amount::text AS gross_amount,
                (SELECT count(*) FROM books.invoice_lines l WHERE l.invoice_id = i.id)
                    AS line_count
           FROM books.invoices i
          WHERE ($1::uuid IS NULL OR i.party_id = $1)
            AND ($2::text IS NULL OR i.status = $2)
            AND ($3::date IS NULL OR i.issued_on >= $3)
            AND ($4::date IS NULL OR i.issued_on <= $4)
          ORDER BY i.issued_on DESC, i.created_at DESC",
    )
    .bind(filter.party_id)
    .bind(filter.status.map(InvoiceStatus::as_str))
    .bind(filter.from)
    .bind(filter.to)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// One invoice, whole: its header, its lines, and the tax on each line.
///
/// Three queries and a stitch rather than one join, for the reason
/// [`crate::master::party::find`] takes four: a join across two one-to-many
/// children multiplies the rows and then has to be un-multiplied in Rust, which
/// is the kind of code that quietly loses the second tax on a line.
pub async fn find(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<Invoice>, DbError> {
    let Some(row) = sqlx::query(
        "SELECT id, number, status, party_id, party_code, party_name, party_tax_id,
                party_line1, party_line2, party_city, party_region,
                party_postal_code, party_country_code,
                issued_on, due_on, currency_code, base_currency_code,
                exchange_rate::text     AS exchange_rate,
                rate_date,
                base_gross_amount::text AS base_gross_amount,
                pricing, rounding_level, rounding,
                net_amount::text   AS net_amount,
                tax_amount::text   AS tax_amount,
                gross_amount::text AS gross_amount,
                notes, posted_at, posted_by, created_at, updated_at
           FROM books.invoices
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)?
    else {
        return Ok(None);
    };

    let currency = currency_of(&row, "currency_code").map_err(DbError::Query)?;
    let lines = lines_of(pool, id, currency).await?;

    Ok(Some(decode_invoice(&row, currency, lines)?))
}

/// The lines on one invoice, with their taxes, in document order.
async fn lines_of(
    pool: &sqlx::PgPool,
    invoice_id: Uuid,
    currency: Currency,
) -> Result<Vec<InvoiceLine>, DbError> {
    let line_rows = sqlx::query(
        "SELECT id, line_no, description,
                quantity::text     AS quantity,
                unit_price::text   AS unit_price,
                net_amount::text   AS net_amount,
                tax_amount::text   AS tax_amount,
                gross_amount::text AS gross_amount,
                tax_group_id, tax_group_code
           FROM books.invoice_lines
          WHERE invoice_id = $1
          ORDER BY line_no",
    )
    .bind(invoice_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)?;

    // One query for every line's taxes rather than one per line: an invoice of
    // fifty lines is fifty round trips otherwise, on a screen somebody is
    // waiting for.
    let tax_rows = sqlx::query(
        "SELECT t.line_id, t.sequence, t.tax_code_id, t.tax_code, t.tax_name,
                t.tax_kind, t.rate::text AS rate, t.is_compound, t.is_recoverable,
                t.taxable_amount::text AS taxable_amount,
                t.tax_amount::text     AS tax_amount
           FROM books.invoice_line_taxes t
           JOIN books.invoice_lines l ON l.id = t.line_id
          WHERE l.invoice_id = $1
          ORDER BY t.line_id, t.sequence",
    )
    .bind(invoice_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)?;

    let mut lines = Vec::with_capacity(line_rows.len());
    for row in &line_rows {
        let id: Uuid = row.try_get("id").map_err(DbError::Query)?;

        let mut taxes = Vec::new();
        for tax in &tax_rows {
            let line_id: Uuid = tax.try_get("line_id").map_err(DbError::Query)?;
            if line_id == id {
                taxes.push(decode_line_tax(tax, currency)?);
            }
        }

        let raw_quantity: String = row.try_get("quantity").map_err(DbError::Query)?;
        let quantity = Quantity::parse(&raw_quantity).map_err(|err| {
            DbError::Query(sqlx::Error::ColumnDecode {
                index: "quantity".to_owned(),
                source: Box::new(err),
            })
        })?;

        lines.push(InvoiceLine {
            id,
            line_no: row.try_get("line_no").map_err(DbError::Query)?,
            description: row.try_get("description").map_err(DbError::Query)?,
            quantity,
            unit_price: money_of(row, "unit_price", currency).map_err(DbError::Query)?,
            net: money_of(row, "net_amount", currency).map_err(DbError::Query)?,
            tax: money_of(row, "tax_amount", currency).map_err(DbError::Query)?,
            gross: money_of(row, "gross_amount", currency).map_err(DbError::Query)?,
            tax_group_id: row.try_get("tax_group_id").map_err(DbError::Query)?,
            tax_group_code: row.try_get("tax_group_code").map_err(DbError::Query)?,
            taxes,
        });
    }

    Ok(lines)
}

/// Everything needed to store a draft.
///
/// The priced totals arrive already computed, by `app_books::pricing` - the
/// same code the browser previewed with. This layer stores; it does not add up.
#[derive(Debug, Clone)]
pub struct DraftWrite<'a> {
    pub checked: &'a CheckedInvoice,
    pub party: &'a PartySnapshot,
    pub priced: &'a DocumentTax,
    pub actor: Option<UserId>,
}

/// Create a draft, or rewrite the one whose id the draft carries.
///
/// Returns the invoice's id. Refuses anything that is not a draft, in the
/// statement: `WHERE status = 'draft'` is what makes that true of the database
/// rather than only of the service above it.
pub async fn save_draft(pool: &sqlx::PgPool, write: DraftWrite<'_>) -> Result<Uuid, DbError> {
    let DraftWrite {
        checked,
        party,
        priced,
        actor,
    } = write;

    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    let invoice_id: Uuid = match checked.id {
        Some(id) => {
            // `RETURNING id` rather than a row count, so the update and the
            // insert are the same shape and can share `bind_header` - which is
            // what stops the two column lists drifting apart.
            let updated = sqlx::query_scalar(
                "UPDATE books.invoices
                    SET party_id           = $2,
                        party_code         = $3,
                        party_name         = $4,
                        party_tax_id       = $5,
                        party_line1        = $6,
                        party_line2        = $7,
                        party_city         = $8,
                        party_region       = $9,
                        party_postal_code  = $10,
                        party_country_code = $11,
                        issued_on          = $12,
                        due_on             = $13,
                        currency_code      = $14,
                        pricing            = $15,
                        rounding_level     = $16,
                        rounding           = $17,
                        net_amount         = $18::numeric,
                        tax_amount         = $19::numeric,
                        gross_amount       = $20::numeric,
                        notes              = $21,
                        updated_at         = now(),
                        updated_by         = $22
                  WHERE id = $1 AND status = 'draft'
                RETURNING id",
            );

            // Nothing matched: the invoice is gone, or it is no longer a draft.
            // Either way this write must not happen, and the `WHERE` is what
            // makes that true of the database rather than only of the service.
            bind_header(updated, id, checked, party, priced, actor)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DbError::Query)?
                .ok_or(DbError::InvoiceNotEditable)?
        }
        None => {
            let insert = sqlx::query_scalar(
                "INSERT INTO books.invoices
                     (id, party_id, party_code, party_name, party_tax_id,
                      party_line1, party_line2, party_city, party_region,
                      party_postal_code, party_country_code,
                      issued_on, due_on, currency_code,
                      pricing, rounding_level, rounding,
                      net_amount, tax_amount, gross_amount, notes,
                      created_by, updated_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                         $12, $13, $14, $15, $16, $17,
                         $18::numeric, $19::numeric, $20::numeric, $21, $22, $22)
                 RETURNING id",
            );
            bind_header(insert, Uuid::new_v4(), checked, party, priced, actor)
                .fetch_one(&mut *tx)
                .await
                .map_err(DbError::Query)?
        }
    };

    // Cleared and rewritten rather than reconciled. The form submits the whole
    // document, the order is part of the meaning, and reconciling a reordering
    // in SQL is how a line ends up under the wrong number.
    sqlx::query("DELETE FROM books.invoice_lines WHERE invoice_id = $1")
        .bind(invoice_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    for (index, (line, computed)) in checked.lines.iter().zip(&priced.lines).enumerate() {
        // Line numbers are what a reader refers to, so they start at one and
        // are dense. The ceiling is 500, checked before this is called, so the
        // cast can never reach a SMALLINT's edge.
        let line_no = i16::try_from(index + 1).unwrap_or(i16::MAX);

        let line_id: Uuid = sqlx::query_scalar(
            "INSERT INTO books.invoice_lines
                 (invoice_id, line_no, description, quantity, unit_price,
                  net_amount, tax_amount, gross_amount, tax_group_id, tax_group_code)
             VALUES ($1, $2, $3, $4::numeric, $5::numeric,
                     $6::numeric, $7::numeric, $8::numeric, $9, $10)
             RETURNING id",
        )
        .bind(invoice_id)
        .bind(line_no)
        .bind(&line.description)
        .bind(line.quantity.to_storage_string())
        .bind(line.unit_price.to_storage_string())
        .bind(computed.net.to_storage_string())
        .bind(computed.tax.to_storage_string())
        .bind(computed.gross.to_storage_string())
        .bind(line.tax_group_id)
        .bind(
            computed
                .taxes
                .first()
                .map_or(String::new(), |_| group_code_of(checked, index)),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::Query)?;

        for tax in &computed.taxes {
            sqlx::query(
                "INSERT INTO books.invoice_line_taxes
                     (line_id, sequence, tax_code_id, tax_code, tax_name, tax_kind,
                      rate, is_compound, is_recoverable, taxable_amount, tax_amount)
                 VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8, $9,
                         $10::numeric, $11::numeric)",
            )
            .bind(line_id)
            .bind(tax.applied.sequence)
            .bind(tax.applied.tax_code_id)
            .bind(&tax.applied.code)
            .bind(&tax.applied.name)
            .bind(tax.applied.kind.as_str())
            .bind(tax.applied.rate.to_storage_string())
            .bind(tax.applied.is_compound)
            .bind(tax.applied.is_recoverable)
            .bind(tax.taxable.to_storage_string())
            .bind(tax.amount.to_storage_string())
            .execute(&mut *tx)
            .await
            .map_err(DbError::Query)?;
        }
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(invoice_id)
}

/// Store the number and the conversion snapshot, and mark the invoice posted.
///
/// # This must be the allocation's transaction
///
/// Takes a `&mut PgConnection` rather than an executor, for the same reason
/// [`crate::numbering::allocate`] does. The row lock that makes a sequence
/// gap-free is held until the surrounding transaction ends, so the allocation
/// and this `UPDATE` have to be in the same one - a failed post then *returns*
/// the number rather than burning it, and a retry cannot leave a gap.
///
/// `false` when the invoice is not a draft. The `WHERE` is what makes that true
/// of the database rather than only of the service.
pub async fn post(
    conn: &mut PgConnection,
    id: Uuid,
    number: &str,
    conversion: Option<&ExchangeRate>,
    base_gross: Option<Money>,
    actor: Option<UserId>,
) -> Result<bool, DbError> {
    let affected = sqlx::query(
        "UPDATE books.invoices
            SET number             = $2,
                status             = 'posted',
                base_currency_code = $3,
                exchange_rate      = $4::numeric,
                rate_date          = $5,
                base_gross_amount  = $6::numeric,
                posted_at          = now(),
                posted_by          = $7,
                updated_at         = now(),
                updated_by         = $7
          WHERE id = $1 AND status = 'draft'",
    )
    .bind(id)
    .bind(number)
    .bind(conversion.map(|rate| rate.quote.code()))
    .bind(conversion.map(|rate| rate.rate.to_storage_string()))
    .bind(conversion.map(|rate| rate.as_of))
    .bind(base_gross.map(Money::to_storage_string))
    .bind(actor)
    .execute(conn)
    .await
    .map_err(DbError::Query)?
    .rows_affected();

    Ok(affected > 0)
}

/// Withdraw a posted invoice, keeping its number.
///
/// The number stays, because a number that disappears is a gap and a gap is
/// what an auditor asks about. `false` when the invoice was not posted.
pub async fn void<'e, E>(executor: E, id: Uuid, actor: Option<UserId>) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query(
        "UPDATE books.invoices
            SET status = 'voided', updated_at = now(), updated_by = $2
          WHERE id = $1 AND status = 'posted'",
    )
    .bind(id)
    .bind(actor)
    .execute(executor)
    .await
    .map_err(DbError::Query)?
    .rows_affected();

    Ok(affected > 0)
}

/// Remove a draft. `false` when it is not one.
///
/// Only a draft. A posted invoice has a number, and a numbered document that
/// vanishes is the gap the whole sequence design exists to prevent.
pub async fn delete_draft<'e, E>(executor: E, id: Uuid) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query("DELETE FROM books.invoices WHERE id = $1 AND status = 'draft'")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    Ok(affected > 0)
}

/// Whether any invoice has been raised against a party.
///
/// The question `master` cannot answer for itself: there is no foreign key from
/// here into `master.parties`, on purpose, so the *app* is what knows.
pub async fn party_is_used<'e, E>(executor: E, party_id: Uuid) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM books.invoices WHERE party_id = $1)")
        .bind(party_id)
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)
}

// --- decoding -----------------------------------------------------------

/// Bind the header columns, in the order both statements above list them.
///
/// One function so the insert and the update cannot drift: they take the same
/// values in the same order, and a column added to one has to be added to the
/// other before this compiles.
fn bind_header<'q, O>(
    query: sqlx::query::QueryScalar<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>,
    id: Uuid,
    checked: &'q CheckedInvoice,
    party: &'q PartySnapshot,
    priced: &'q DocumentTax,
    actor: Option<UserId>,
) -> sqlx::query::QueryScalar<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments> {
    query
        .bind(id)
        .bind(party.party_id)
        .bind(&party.code)
        .bind(&party.name)
        .bind(&party.tax_id)
        .bind(&party.address.line1)
        .bind(&party.address.line2)
        .bind(&party.address.city)
        .bind(&party.address.region)
        .bind(&party.address.postal_code)
        .bind(party.address.country.map(Country::code))
        .bind(checked.issued_on)
        .bind(checked.due_on)
        .bind(checked.currency.code())
        .bind(checked.pricing.as_str())
        .bind(checked.rounding_level.as_str())
        .bind(rounding_str(checked.rounding))
        .bind(priced.net.to_storage_string())
        .bind(priced.tax.to_storage_string())
        .bind(priced.gross.to_storage_string())
        .bind(&checked.notes)
        .bind(actor)
}

/// The group code a line's taxes came from, for the row's own record.
fn group_code_of(checked: &CheckedInvoice, index: usize) -> String {
    checked
        .lines
        .get(index)
        .and_then(|line| line.tax_group_id)
        .map(|id| id.to_string())
        .unwrap_or_default()
}

/// The stored spelling of a rounding mode.
///
/// `Rounding` is `phonix-core`'s and has no `as_str`, because nothing else
/// stores it. Written here rather than added there so that `core` does not gain
/// a method for one app's column.
const fn rounding_str(rounding: Rounding) -> &'static str {
    match rounding {
        Rounding::HalfUp => "half_up",
        Rounding::HalfEven => "half_even",
    }
}

fn parse_rounding(raw: &str) -> Option<Rounding> {
    match raw {
        "half_up" => Some(Rounding::HalfUp),
        "half_even" => Some(Rounding::HalfEven),
        _ => None,
    }
}

fn currency_of(row: &sqlx::postgres::PgRow, column: &str) -> Result<Currency, sqlx::Error> {
    let code: String = row.try_get(column)?;
    Currency::parse(&code).map_err(|err| sqlx::Error::ColumnDecode {
        index: column.to_owned(),
        source: Box::new(err),
    })
}

/// An amount column, read back as text so no digit is lost in the driver.
fn money_of(
    row: &sqlx::postgres::PgRow,
    column: &str,
    currency: Currency,
) -> Result<Money, sqlx::Error> {
    let digits: String = row.try_get(column)?;
    Money::parse(currency, &digits).map_err(|err| sqlx::Error::ColumnDecode {
        index: column.to_owned(),
        source: Box::new(err),
    })
}

fn decode_line_tax(
    row: &sqlx::postgres::PgRow,
    currency: Currency,
) -> Result<LineTaxSnapshot, DbError> {
    let stored_kind: String = row.try_get("tax_kind").map_err(DbError::Query)?;
    let kind = TaxKind::parse(&stored_kind).ok_or_else(|| {
        DbError::CorruptRow(format!(
            "unrecognised tax kind '{stored_kind}' on an invoice line"
        ))
    })?;

    let digits: String = row.try_get("rate").map_err(DbError::Query)?;
    let rate = TaxRate::parse(&digits).map_err(|err| {
        DbError::CorruptRow(format!("unusable tax rate on an invoice line: {err}"))
    })?;

    Ok(LineTaxSnapshot {
        applied: AppliedTax {
            tax_code_id: row.try_get("tax_code_id").map_err(DbError::Query)?,
            code: row.try_get("tax_code").map_err(DbError::Query)?,
            name: row.try_get("tax_name").map_err(DbError::Query)?,
            kind,
            rate,
            is_compound: row.try_get("is_compound").map_err(DbError::Query)?,
            is_recoverable: row.try_get("is_recoverable").map_err(DbError::Query)?,
            sequence: row.try_get("sequence").map_err(DbError::Query)?,
        },
        taxable: money_of(row, "taxable_amount", currency).map_err(DbError::Query)?,
        amount: money_of(row, "tax_amount", currency).map_err(DbError::Query)?,
    })
}

/// Turn a header row into an invoice.
///
/// Every enum column is refused rather than defaulted. A pricing basis that
/// silently became `exclusive` would change every amount on the document by the
/// tax rate, and a rounding mode that became `half_up` would change the total
/// by a cent - quietly, on a document somebody has already sent.
fn decode_invoice(
    row: &sqlx::postgres::PgRow,
    currency: Currency,
    lines: Vec<InvoiceLine>,
) -> Result<Invoice, DbError> {
    let refuse = |column: &str, value: &str| {
        DbError::CorruptRow(format!("unrecognised {column} '{value}' on an invoice"))
    };

    let stored_status: String = row.try_get("status").map_err(DbError::Query)?;
    let status =
        InvoiceStatus::parse(&stored_status).ok_or_else(|| refuse("status", &stored_status))?;

    let stored_pricing: String = row.try_get("pricing").map_err(DbError::Query)?;
    let pricing =
        Pricing::parse(&stored_pricing).ok_or_else(|| refuse("pricing", &stored_pricing))?;

    let stored_level: String = row.try_get("rounding_level").map_err(DbError::Query)?;
    let rounding_level = RoundingLevel::parse(&stored_level)
        .ok_or_else(|| refuse("rounding level", &stored_level))?;

    let stored_rounding: String = row.try_get("rounding").map_err(DbError::Query)?;
    let rounding =
        parse_rounding(&stored_rounding).ok_or_else(|| refuse("rounding", &stored_rounding))?;

    // The conversion snapshot is all six columns or none - a CHECK constraint
    // says so - so reading one is enough to decide whether to read the rest.
    let base_code: Option<String> = row.try_get("base_currency_code").map_err(DbError::Query)?;
    let (rate, base_gross) = match base_code {
        None => (None, None),
        Some(code) => {
            let base = Currency::parse(&code)
                .map_err(|err| DbError::CorruptRow(format!("unusable base currency: {err}")))?;
            let digits: String = row.try_get("exchange_rate").map_err(DbError::Query)?;
            let parsed = Rate::parse(&digits)
                .map_err(|err| DbError::CorruptRow(format!("unusable exchange rate: {err}")))?;
            let as_of: NaiveDate = row.try_get("rate_date").map_err(DbError::Query)?;

            let rate = ExchangeRate::new(currency, base, parsed, as_of, "invoice")
                .map_err(|err| DbError::CorruptRow(format!("unusable rate snapshot: {err}")))?;
            let gross = money_of(row, "base_gross_amount", base).map_err(DbError::Query)?;

            (Some(rate), Some(gross))
        }
    };

    let address = PostalAddress {
        line1: row.try_get("party_line1").map_err(DbError::Query)?,
        line2: row.try_get("party_line2").map_err(DbError::Query)?,
        city: row.try_get("party_city").map_err(DbError::Query)?,
        region: row.try_get("party_region").map_err(DbError::Query)?,
        postal_code: row.try_get("party_postal_code").map_err(DbError::Query)?,
        country: {
            let stored: Option<String> =
                row.try_get("party_country_code").map_err(DbError::Query)?;
            stored
                .map(|code| {
                    Country::parse(&code)
                        .map_err(|err| DbError::CorruptRow(format!("unusable country: {err}")))
                })
                .transpose()?
        },
    };

    Ok(Invoice {
        id: row.try_get("id").map_err(DbError::Query)?,
        number: row.try_get("number").map_err(DbError::Query)?,
        status,
        party: PartySnapshot {
            party_id: row.try_get("party_id").map_err(DbError::Query)?,
            code: row.try_get("party_code").map_err(DbError::Query)?,
            name: row.try_get("party_name").map_err(DbError::Query)?,
            tax_id: row.try_get("party_tax_id").map_err(DbError::Query)?,
            address,
        },
        issued_on: row.try_get("issued_on").map_err(DbError::Query)?,
        due_on: row.try_get("due_on").map_err(DbError::Query)?,
        currency,
        rate,
        pricing,
        rounding_level,
        rounding,
        totals: InvoiceTotals {
            net: money_of(row, "net_amount", currency).map_err(DbError::Query)?,
            tax: money_of(row, "tax_amount", currency).map_err(DbError::Query)?,
            gross: money_of(row, "gross_amount", currency).map_err(DbError::Query)?,
            base_gross,
        },
        notes: row.try_get("notes").map_err(DbError::Query)?,
        lines,
        posted_at: row.try_get("posted_at").map_err(DbError::Query)?,
        posted_by: row.try_get("posted_by").map_err(DbError::Query)?,
        created_at: row.try_get("created_at").map_err(DbError::Query)?,
        updated_at: row.try_get("updated_at").map_err(DbError::Query)?,
    })
}
