//! A sales invoice: what it holds, and what state it is in.
//!
//! # Three states, and only one of them can be edited
//!
//! A [`InvoiceStatus::Draft`] has no number and is freely editable.
//! [`InvoiceStatus::Posted`] is the document: numbered, snapshotted, and
//! frozen. [`InvoiceStatus::Voided`] is a posted invoice withdrawn - it keeps
//! its number, because a number that disappears is a gap, and a gap is what an
//! auditor asks about.
//!
//! There is deliberately no path from posted back to draft. An invoice that can
//! be edited after it has been sent is not evidence of anything; a mistake is
//! corrected by voiding it and raising another.
//!
//! # The snapshot is the record
//!
//! [`PartySnapshot`] and the per-line tax detail are copied onto the invoice
//! when it is posted and never re-resolved. A customer who moves, a tax that
//! changes and a rate that drifts must not rewrite a document that was already
//! sent.

use chrono::{DateTime, NaiveDate, Utc};
use phonix_core::identity::UserId;
use phonix_core::locale::Currency;
use phonix_core::money::{ExchangeRate, Money, Rounding};
use phonix_core::{Message, msg};
use phonix_master::address::PostalAddress;
use phonix_tax::compute::{Pricing, RoundingLevel};
use phonix_tax::group::AppliedTax;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::quantity::Quantity;

/// Longest a line's description may be.
pub const MAX_DESCRIPTION_LEN: usize = 500;

/// The most lines one invoice may carry.
///
/// Not a technical limit. It is the point past which the screen stops being
/// usable and the document stops being one somebody reads, and a ceiling that
/// is stated is better than a timeout that is not.
pub const MAX_LINES: usize = 500;

/// Where an invoice is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    /// Editable, deletable, and carrying no number.
    #[default]
    Draft,
    /// Numbered, snapshotted and frozen.
    Posted,
    /// Withdrawn, and keeping its number.
    Voided,
}

impl InvoiceStatus {
    pub const ALL: &'static [Self] = &[Self::Draft, Self::Posted, Self::Voided];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Posted => "posted",
            Self::Voided => "voided",
        }
    }

    /// Read a stored value back.
    ///
    /// `None` rather than a default: the status decides whether a document is
    /// owed, editable and countable, and guessing at it would put a draft in a
    /// revenue figure.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.as_str() == raw)
    }

    pub fn label(self) -> Message {
        match self {
            Self::Draft => msg!("books.status.draft"),
            Self::Posted => msg!("books.status.posted"),
            Self::Voided => msg!("books.status.voided"),
        }
    }

    /// Whether the document may still be changed.
    pub const fn is_editable(self) -> bool {
        matches!(self, Self::Draft)
    }

    /// Whether this status counts towards what the workspace is owed.
    ///
    /// A draft is not a claim on anybody and a voided invoice has been
    /// withdrawn, so only a posted one does.
    pub const fn is_receivable(self) -> bool {
        matches!(self, Self::Posted)
    }
}

/// The customer, as the document records them.
///
/// A copy, not a reference. `party_id` says where it came from and is never
/// re-resolved for a reprint: the name on an invoice is the name that was on it
/// when it was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartySnapshot {
    pub party_id: Uuid,
    /// The workspace's own reference, printed beside the name.
    pub code: String,
    /// The *registered* name where there is one - an invoice is a legal
    /// instrument and names the entity, not its trading style. See
    /// [`phonix_master::party::Party::document_name`].
    pub name: String,
    pub tax_id: Option<String>,
    pub address: PostalAddress,
}

/// What an invoice comes to.
///
/// Stored rather than recomputed on read. The arithmetic is deterministic, so
/// recomputing would give the same answer today - and a rate change, a rounding
/// policy change or a corrected tax would silently give a different one next
/// year, which is the classic way a ledger stops reconciling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceTotals {
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
    /// The same figures in the workspace's own currency, converted once at the
    /// rate below. `None` when the invoice is already in the base currency.
    pub base_gross: Option<Money>,
}

/// One invoice, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: Uuid,
    /// `None` while it is a draft. Taken from the sequence at post.
    pub number: Option<String>,
    pub status: InvoiceStatus,
    pub party: PartySnapshot,
    pub issued_on: NaiveDate,
    pub due_on: Option<NaiveDate>,
    pub currency: Currency,
    /// The whole conversion snapshot: base currency, rate and the day the rate
    /// was published. `None` when the invoice is in the base currency, because
    /// there is nothing to convert and a rate of one is not evidence of a
    /// quotation.
    pub rate: Option<ExchangeRate>,
    pub pricing: Pricing,
    pub rounding_level: RoundingLevel,
    pub rounding: Rounding,
    pub totals: InvoiceTotals,
    pub notes: Option<String>,
    pub lines: Vec<InvoiceLine>,
    pub posted_at: Option<DateTime<Utc>>,
    pub posted_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Invoice {
    /// What to call this document on screen.
    ///
    /// A draft has no number, so it is called a draft rather than shown as a
    /// blank - and never shown the number it is *going* to get, which would
    /// promise something the post may not keep.
    pub fn reference(&self) -> Message {
        match &self.number {
            Some(_) => msg!("books.invoice.numbered"),
            None => msg!("books.invoice.draft"),
        }
    }

    /// The number, or the word for a draft. For a heading.
    pub fn number_or_draft(&self) -> String {
        self.number.clone().unwrap_or_default()
    }

    pub const fn is_editable(&self) -> bool {
        self.status.is_editable()
    }
}

/// One line, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub id: Uuid,
    /// Position on the document, from one. What the reader sees.
    pub line_no: i16,
    pub description: String,
    pub quantity: Quantity,
    pub unit_price: Money,
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
    /// Where the treatment came from. Kept for tracing, never re-resolved.
    pub tax_group_id: Option<Uuid>,
    pub tax_group_code: String,
    /// The resolved taxes, in the order they applied. This is the snapshot that
    /// makes a 2030 reprint show 2026's rate.
    pub taxes: Vec<LineTaxSnapshot>,
}

/// One tax on one line, as the document records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineTaxSnapshot {
    pub applied: AppliedTax,
    pub taxable: Money,
    pub amount: Money,
}

/// One invoice as a list row.
///
/// A separate type from [`Invoice`] rather than a slimmer read of it, for the
/// reason `UserListing` is separate from `UserEdit`: a row is *rendered*, and a
/// list that carried every line would fetch three tables to draw a total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceSummary {
    pub id: Uuid,
    pub number: Option<String>,
    pub status: InvoiceStatus,
    pub party_id: Uuid,
    pub party_name: String,
    pub issued_on: NaiveDate,
    pub due_on: Option<NaiveDate>,
    pub currency: Currency,
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
    pub line_count: i64,
}

impl InvoiceSummary {
    /// Whether this document is past its due date and still owed.
    ///
    /// Takes today explicitly rather than reading the clock, so a grid renders
    /// the same on the server and in the browser - a row that says "overdue" on
    /// one side and not the other is a hydration mismatch.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        self.status.is_receivable() && self.due_on.is_some_and(|due| due < today)
    }
}

/// How a post turned out.
///
/// Outcomes rather than errors for the two that are expected: a document
/// somebody else already posted, and a workspace whose number series has not
/// been set up. Both are things a screen renders beside a button, and modelling
/// either as a failure would make every caller unwrap something ordinary.
///
/// Here rather than in the service so that it can cross the wire - the screen
/// that renders it runs in the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostOutcome {
    /// Posted. Carries the number, because that is the one thing the screen
    /// needs to say back.
    Posted { number: String },
    /// It is not a draft any more - somebody else posted it first.
    NotADraft,
    /// There is no active `sales_invoice` series in this workspace.
    ///
    /// A configuration problem rather than a mistake, and one with a specific
    /// fix - the numbering settings screen - so it is worth its own outcome
    /// instead of a generic failure.
    NoSeries,
}

impl PostOutcome {
    /// What to say about it.
    pub fn message(&self) -> Message {
        match self {
            Self::Posted { .. } => msg!("books.posted"),
            Self::NotADraft => msg!("books.error.not_editable"),
            Self::NoSeries => msg!("books.error.no_series"),
        }
    }
}

/// A line being written on a screen.
///
/// The quantity and unit price arrive **as typed**, because a box that has not
/// been finished is not yet a number and refusing to hold it would mean the
/// field could not be edited. They become a [`Quantity`] and a [`Money`] at
/// [`check`](InvoiceInput::check).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceLineInput {
    /// `None` for a line being added.
    pub id: Option<Uuid>,
    pub description: String,
    pub quantity: String,
    pub unit_price: String,
    /// `None` is a line outside the scope of tax, which is not the same as a
    /// zero-rated one - that is a group whose rate is zero.
    pub tax_group_id: Option<Uuid>,
}

impl InvoiceLineInput {
    pub fn blank() -> Self {
        Self {
            id: None,
            description: String::new(),
            quantity: "1".to_owned(),
            unit_price: String::new(),
            tax_group_id: None,
        }
    }
}

/// An invoice being created or edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceInput {
    pub id: Option<Uuid>,
    pub party_id: Option<Uuid>,
    pub issued_on: NaiveDate,
    pub due_on: Option<NaiveDate>,
    pub currency: Currency,
    pub pricing: Pricing,
    pub rounding_level: RoundingLevel,
    pub rounding: Rounding,
    pub notes: Option<String>,
    pub lines: Vec<InvoiceLineInput>,
}

impl InvoiceInput {
    /// What a blank form opens on.
    ///
    /// `today` and the workspace's own currency are passed in rather than read
    /// here: this crate has no clock and no database, and a default that
    /// guessed at either would be a default that differs between the browser
    /// and the server.
    pub fn blank(today: NaiveDate, currency: Currency) -> Self {
        Self {
            id: None,
            party_id: None,
            issued_on: today,
            due_on: None,
            currency,
            pricing: Pricing::Exclusive,
            rounding_level: RoundingLevel::Line,
            rounding: Rounding::HalfUp,
            notes: None,
            lines: vec![InvoiceLineInput::blank()],
        }
    }

    /// Re-open a stored invoice for editing.
    pub fn from_invoice(invoice: &Invoice) -> Self {
        Self {
            id: Some(invoice.id),
            party_id: Some(invoice.party.party_id),
            issued_on: invoice.issued_on,
            due_on: invoice.due_on,
            currency: invoice.currency,
            pricing: invoice.pricing,
            rounding_level: invoice.rounding_level,
            rounding: invoice.rounding,
            notes: invoice.notes.clone(),
            lines: invoice
                .lines
                .iter()
                .map(|line| InvoiceLineInput {
                    id: Some(line.id),
                    description: line.description.clone(),
                    quantity: line.quantity.to_display_string(),
                    unit_price: line.unit_price.to_storage_string(),
                    tax_group_id: line.tax_group_id,
                })
                .collect(),
        }
    }

    /// Check what was typed, and say what is still wrong.
    ///
    /// Returns the parsed lines so a caller cannot store the draft strings by
    /// accident. Blank lines - no description, no price - are dropped rather
    /// than refused: a form that always offers an empty row at the bottom would
    /// otherwise be unsubmittable.
    pub fn check(&self) -> Result<CheckedInvoice, InvoiceError> {
        let Some(party_id) = self.party_id else {
            return Err(InvoiceError::PartyRequired);
        };

        if let Some(due) = self.due_on
            && due < self.issued_on
        {
            return Err(InvoiceError::DueBeforeIssued);
        }

        let mut lines: Vec<CheckedLine> = Vec::with_capacity(self.lines.len());
        for (index, line) in self.lines.iter().enumerate() {
            let description = line.description.trim();
            let quantity = line.quantity.trim();
            let price = line.unit_price.trim();

            // An untouched row at the bottom of the form is not a line.
            if description.is_empty() && price.is_empty() {
                continue;
            }

            let at = index;
            if description.is_empty() {
                return Err(InvoiceError::DescriptionRequired { line: at });
            }
            if description.chars().count() > MAX_DESCRIPTION_LEN {
                return Err(InvoiceError::DescriptionTooLong { line: at });
            }

            let quantity =
                Quantity::parse(quantity).map_err(|_| InvoiceError::BadQuantity { line: at })?;
            // A line of nothing is a line that prints and charges nothing.
            if quantity.is_zero() {
                return Err(InvoiceError::ZeroQuantity { line: at });
            }

            let unit_price = Money::parse(self.currency, price)
                .map_err(|_| InvoiceError::BadPrice { line: at })?;

            lines.push(CheckedLine {
                id: line.id,
                description: description.to_owned(),
                quantity,
                unit_price,
                tax_group_id: line.tax_group_id,
            });
        }

        if lines.is_empty() {
            return Err(InvoiceError::NoLines);
        }
        if lines.len() > MAX_LINES {
            return Err(InvoiceError::TooManyLines);
        }

        Ok(CheckedInvoice {
            id: self.id,
            party_id,
            issued_on: self.issued_on,
            due_on: self.due_on,
            currency: self.currency,
            pricing: self.pricing,
            rounding_level: self.rounding_level,
            rounding: self.rounding,
            notes: self
                .notes
                .as_deref()
                .map(str::trim)
                .filter(|notes| !notes.is_empty())
                .map(str::to_owned),
            lines,
        })
    }
}

/// An invoice whose fields have been parsed, ready to price and store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedInvoice {
    pub id: Option<Uuid>,
    pub party_id: Uuid,
    pub issued_on: NaiveDate,
    pub due_on: Option<NaiveDate>,
    pub currency: Currency,
    pub pricing: Pricing,
    pub rounding_level: RoundingLevel,
    pub rounding: Rounding,
    pub notes: Option<String>,
    pub lines: Vec<CheckedLine>,
}

/// A line whose quantity and price have been parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedLine {
    pub id: Option<Uuid>,
    pub description: String,
    pub quantity: Quantity,
    pub unit_price: Money,
    pub tax_group_id: Option<Uuid>,
}

/// What can be wrong with an invoice somebody typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvoiceError {
    #[error("an invoice needs a customer")]
    PartyRequired,
    #[error("an invoice cannot fall due before it is issued")]
    DueBeforeIssued,
    #[error("an invoice needs at least one line")]
    NoLines,
    #[error("an invoice holds at most 500 lines")]
    TooManyLines,
    #[error("line {line}: a line needs a description")]
    DescriptionRequired { line: usize },
    #[error("line {line}: a description is at most 500 characters")]
    DescriptionTooLong { line: usize },
    #[error("line {line}: that is not a quantity")]
    BadQuantity { line: usize },
    #[error("line {line}: a quantity of nothing charges nothing")]
    ZeroQuantity { line: usize },
    #[error("line {line}: that is not a price")]
    BadPrice { line: usize },
    #[error("a posted invoice cannot be edited")]
    NotEditable,
    #[error("only a posted invoice can be voided")]
    NotVoidable,
}

impl InvoiceError {
    /// Which control to attach the message to.
    ///
    /// A line problem names the row it is on, because "that is not a quantity"
    /// on a fifty-line invoice is not an answer.
    pub fn field(self) -> String {
        match self {
            Self::PartyRequired => "party_id".to_owned(),
            Self::DueBeforeIssued => "due_on".to_owned(),
            Self::NoLines | Self::TooManyLines => "lines".to_owned(),
            Self::NotEditable | Self::NotVoidable => "status".to_owned(),
            Self::DescriptionRequired { line } | Self::DescriptionTooLong { line } => {
                format!("lines.{line}.description")
            }
            Self::BadQuantity { line } | Self::ZeroQuantity { line } => {
                format!("lines.{line}.quantity")
            }
            Self::BadPrice { line } => format!("lines.{line}.unit_price"),
        }
    }

    pub fn message(self) -> Message {
        match self {
            Self::PartyRequired => msg!("books.error.party_required"),
            Self::DueBeforeIssued => msg!("books.error.due_before_issued"),
            Self::NoLines => msg!("books.error.no_lines"),
            Self::TooManyLines => msg!("books.error.too_many_lines"),
            Self::DescriptionRequired { .. } => msg!("books.error.description_required"),
            Self::DescriptionTooLong { .. } => msg!("books.error.description_too_long"),
            Self::BadQuantity { .. } => msg!("books.error.bad_quantity"),
            Self::ZeroQuantity { .. } => msg!("books.error.zero_quantity"),
            Self::BadPrice { .. } => msg!("books.error.bad_price"),
            Self::NotEditable => msg!("books.error.not_editable"),
            Self::NotVoidable => msg!("books.error.not_voidable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd() -> Currency {
        Currency::parse("USD").expect("a real currency")
    }

    fn day(year: i32, month: u32, of: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
    }

    fn line(description: &str, quantity: &str, price: &str) -> InvoiceLineInput {
        InvoiceLineInput {
            id: None,
            description: description.to_owned(),
            quantity: quantity.to_owned(),
            unit_price: price.to_owned(),
            tax_group_id: None,
        }
    }

    fn input(lines: Vec<InvoiceLineInput>) -> InvoiceInput {
        InvoiceInput {
            party_id: Some(Uuid::nil()),
            lines,
            ..InvoiceInput::blank(day(2026, 6, 1), usd())
        }
    }

    #[test]
    fn only_a_draft_can_be_edited() {
        // There is no path from posted back to draft: an invoice that can be
        // edited after it has been sent is not evidence of anything.
        assert!(InvoiceStatus::Draft.is_editable());
        assert!(!InvoiceStatus::Posted.is_editable());
        assert!(!InvoiceStatus::Voided.is_editable());
    }

    #[test]
    fn only_a_posted_invoice_is_owed() {
        // A draft is not a claim on anybody, and a voided one was withdrawn.
        assert!(!InvoiceStatus::Draft.is_receivable());
        assert!(InvoiceStatus::Posted.is_receivable());
        assert!(!InvoiceStatus::Voided.is_receivable());
    }

    #[test]
    fn every_status_round_trips_and_an_unknown_one_is_refused() {
        for status in InvoiceStatus::ALL {
            assert_eq!(InvoiceStatus::parse(status.as_str()), Some(*status));
        }
        // Guessing would put a draft in a revenue figure.
        assert_eq!(InvoiceStatus::parse("approved"), None);
    }

    #[test]
    fn an_untouched_row_at_the_bottom_of_the_form_is_not_a_line() {
        // The form always offers an empty row, so refusing one would make the
        // form unsubmittable.
        let checked = input(vec![line("Consulting", "1", "100.00"), line("", "1", "")])
            .check()
            .unwrap();

        assert_eq!(checked.lines.len(), 1);
    }

    #[test]
    fn an_invoice_of_nothing_is_refused() {
        assert_eq!(
            input(vec![line("", "1", "")]).check(),
            Err(InvoiceError::NoLines)
        );
        assert_eq!(input(Vec::new()).check(), Err(InvoiceError::NoLines));
    }

    #[test]
    fn an_invoice_needs_a_customer() {
        let orphan = InvoiceInput {
            party_id: None,
            ..input(vec![line("Consulting", "1", "100.00")])
        };

        assert_eq!(orphan.check(), Err(InvoiceError::PartyRequired));
    }

    #[test]
    fn a_line_problem_names_the_row_it_is_on() {
        // "That is not a quantity" on a fifty-line invoice is not an answer.
        let result = input(vec![
            line("Consulting", "1", "100.00"),
            line("Travel", "two", "50.00"),
        ])
        .check();

        assert_eq!(result, Err(InvoiceError::BadQuantity { line: 1 }));
        assert_eq!(
            InvoiceError::BadQuantity { line: 1 }.field(),
            "lines.1.quantity"
        );
    }

    #[test]
    fn a_line_of_nothing_is_refused() {
        // It would print and charge nothing, which is a line somebody meant to
        // fill in.
        let result = input(vec![line("Consulting", "0", "100.00")]).check();

        assert_eq!(result, Err(InvoiceError::ZeroQuantity { line: 0 }));
    }

    #[test]
    fn an_invoice_cannot_fall_due_before_it_is_issued() {
        let backwards = InvoiceInput {
            due_on: Some(day(2026, 5, 1)),
            ..input(vec![line("Consulting", "1", "100.00")])
        };

        assert_eq!(backwards.check(), Err(InvoiceError::DueBeforeIssued));
    }

    #[test]
    fn a_negative_line_is_allowed_because_a_discount_is_one() {
        let checked = input(vec![
            line("Consulting", "1", "100.00"),
            line("Introductory discount", "-1", "10.00"),
        ])
        .check()
        .unwrap();

        assert_eq!(checked.lines.len(), 2);
        assert!(checked.lines[1].quantity.is_negative());
    }

    #[test]
    fn blank_notes_are_stored_as_nothing() {
        let checked = InvoiceInput {
            notes: Some("   ".to_owned()),
            ..input(vec![line("Consulting", "1", "100.00")])
        }
        .check()
        .unwrap();

        assert_eq!(checked.notes, None);
    }

    #[test]
    fn overdue_reads_the_same_on_both_sides_of_the_wire() {
        // Today is passed in rather than read from a clock: a row that says
        // "overdue" on the server and not in the browser is a hydration
        // mismatch.
        let summary = |status: InvoiceStatus, due: Option<NaiveDate>| InvoiceSummary {
            id: Uuid::nil(),
            number: None,
            status,
            party_id: Uuid::nil(),
            party_name: "Acme".to_owned(),
            issued_on: day(2026, 5, 1),
            due_on: due,
            currency: usd(),
            net: Money::zero(usd()),
            tax: Money::zero(usd()),
            gross: Money::zero(usd()),
            line_count: 1,
        };

        let due = Some(day(2026, 5, 31));
        assert!(summary(InvoiceStatus::Posted, due).is_overdue(day(2026, 6, 1)));
        assert!(!summary(InvoiceStatus::Posted, due).is_overdue(day(2026, 5, 31)));
        // A draft is not owed, so it cannot be overdue.
        assert!(!summary(InvoiceStatus::Draft, due).is_overdue(day(2026, 6, 1)));
        assert!(!summary(InvoiceStatus::Voided, due).is_overdue(day(2026, 6, 1)));
        // No due date is no deadline.
        assert!(!summary(InvoiceStatus::Posted, None).is_overdue(day(2099, 1, 1)));
    }
}
