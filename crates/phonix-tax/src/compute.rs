//! The arithmetic: lines in, per-line and per-tax totals out.
//!
//! A pure function. It reads only what is on the document - the resolved
//! treatments, the pricing basis, the two rounding policies - so the browser
//! previewing an invoice and the server posting it produce the same figures by
//! construction rather than by two implementations agreeing.
//!
//! # The two policies live on the document, stored
//!
//! [`RoundingLevel`] and [`phonix_core::Rounding`] are not settings read at
//! computation time; they are columns on the document. Reconciliation disputes
//! come from these two being implicit - two systems agreeing on every rate and
//! every line, disagreeing by a cent, and neither able to say why. A document
//! that records both can always explain itself, including after the workspace
//! changes its default.
//!
//! # How the compound chain is computed
//!
//! Each tax contributes a factor, and a compound tax's factor is taken over the
//! base *plus everything before it*:
//!
//! ```text
//! accumulated = 0
//! for tax in sequence order:
//!     base       = if tax.is_compound { 1 + accumulated } else { 1 }
//!     factor     = base * rate
//!     accumulated += factor
//! ```
//!
//! Those factors depend on the rates alone, not on the amount - which is what
//! makes tax-inclusive pricing exact rather than iterative: the net is
//! `gross / (1 + accumulated)`, with the compound ordering already inside the
//! accumulation.
//!
//! # Where the rounding happens, and where it does not
//!
//! Every intermediate is carried at the storage scale of four places. The
//! minor-unit rounding - the one that turns a figure into money somebody pays -
//! happens **once**, and [`RoundingLevel`] says where:
//!
//! * [`RoundingLevel::Line`] rounds each line's tax. Lines add up to the total
//!   because the total is their sum.
//! * [`RoundingLevel::Document`] rounds each tax code's document total once,
//!   and gives each line the difference between the running rounded total and
//!   what has already been given out. Lines still add up to the total exactly,
//!   and the total is the one a person checking the document by hand would get.
//!
//! # Tax-inclusive pricing balances to the gross
//!
//! Under [`Pricing::Inclusive`] the gross is the given - it is the price that
//! was quoted - so the derived figures have to add back to it. Any cent left
//! over after rounding goes to the **net**, never to the tax: the tax is what
//! gets remitted and filed, and moving a cent into it to make a subtraction
//! work is moving a cent of somebody else's money.

use phonix_core::locale::Currency;
use phonix_core::money::{Money, MoneyError, Rounding};
use phonix_core::{Message, msg};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::code::TaxKind;
use crate::group::{AppliedTax, TaxTreatment};
use crate::rate::{RATE_ONE, TaxRate};

/// Decimal places the compound factors are carried at.
///
/// Twelve, which is two rates multiplied together - the deepest a factor ever
/// gets, because a compound tax multiplies its own rate by an accumulation of
/// rates. Wider than either operand, so no factor is rounded on the way in.
pub const FACTOR_SCALE: u32 = 12;

/// `10^FACTOR_SCALE`: the factor meaning "times one".
const FACTOR_ONE: i128 = 1_000_000_000_000;

/// Whether a line's amount already has tax in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pricing {
    /// The line amount is the net. Tax is added to it.
    #[default]
    Exclusive,
    /// The line amount is the gross. The net is derived from it.
    ///
    /// Retail, and most of the world's consumer pricing. The number on the
    /// shelf is the number at the till.
    Inclusive,
}

impl Pricing {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::Inclusive => "inclusive",
        }
    }

    /// Read a stored value back. `None` rather than a default: getting this
    /// wrong changes every amount on the document by the tax rate.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "exclusive" => Some(Self::Exclusive),
            "inclusive" => Some(Self::Inclusive),
            _ => None,
        }
    }

    pub fn label(self) -> Message {
        match self {
            Self::Exclusive => msg!("tax.pricing.exclusive"),
            Self::Inclusive => msg!("tax.pricing.inclusive"),
        }
    }
}

/// Where the minor-unit rounding happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundingLevel {
    /// Round each line's tax. The default, and what most invoices show.
    #[default]
    Line,
    /// Round each tax code's document total once.
    ///
    /// What several jurisdictions require, and what a customer gets when they
    /// add the document up themselves.
    Document,
}

impl RoundingLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Document => "document",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "line" => Some(Self::Line),
            "document" => Some(Self::Document),
            _ => None,
        }
    }

    pub fn label(self) -> Message {
        match self {
            Self::Line => msg!("tax.rounding_level.line"),
            Self::Document => msg!("tax.rounding_level.document"),
        }
    }
}

/// One line, as the computation sees it.
///
/// The amount is the net under [`Pricing::Exclusive`] and the gross under
/// [`Pricing::Inclusive`] - which is why the basis belongs to the document
/// rather than to the line: a document where half the lines meant one and half
/// the other has no total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxLine {
    pub amount: Money,
    pub treatment: TaxTreatment,
}

impl TaxLine {
    /// A line outside the scope of tax.
    pub fn untaxed(amount: Money) -> Self {
        Self {
            amount,
            treatment: TaxTreatment::none(),
        }
    }
}

/// A document, as the computation sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxDocument {
    /// Every amount on the document. Taken explicitly so an empty document
    /// still has totals, for the reason [`Money::total`] takes one.
    pub currency: Currency,
    pub pricing: Pricing,
    pub rounding_level: RoundingLevel,
    pub rounding: Rounding,
    pub lines: Vec<TaxLine>,
}

/// One tax on one line, computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineTax {
    /// The snapshot this was computed from, carried through so a line can be
    /// stored and reprinted without resolving anything again.
    pub applied: AppliedTax,
    /// What this tax was charged on - the net, or the net plus the taxes
    /// before it when compound.
    pub taxable: Money,
    pub amount: Money,
}

/// One line, computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineResult {
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
    pub taxes: Vec<LineTax>,
}

/// One tax code's total across the document.
///
/// What a tax summary box prints, and what a return is filed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxTotal {
    pub tax_code_id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: TaxKind,
    pub rate: TaxRate,
    pub is_recoverable: bool,
    pub taxable: Money,
    pub amount: Money,
}

/// The whole document, computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentTax {
    pub currency: Currency,
    pub lines: Vec<LineResult>,
    /// One entry per tax code, in the order the codes first appear.
    pub by_tax: Vec<TaxTotal>,
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
}

impl DocumentTax {
    /// The part of the tax that can be reclaimed, as opposed to a cost.
    pub fn recoverable(&self) -> Result<Money, TaxError> {
        Money::total(
            self.currency,
            self.by_tax
                .iter()
                .filter(|total| total.is_recoverable)
                .map(|total| total.amount),
        )
        .map_err(TaxError::Amount)
    }
}

/// Work out what a document's tax comes to.
pub fn compute(document: &TaxDocument) -> Result<DocumentTax, TaxError> {
    let currency = document.currency;
    let mode = document.rounding;

    // --- exact phase -----------------------------------------------------
    //
    // Everything at the storage scale of four places, with no minor-unit
    // rounding anywhere. Rounding here would be rounding twice.
    let mut raw_lines: Vec<RawLine> = Vec::with_capacity(document.lines.len());
    for line in &document.lines {
        if line.amount.currency() != currency {
            return Err(TaxError::Amount(MoneyError::CurrencyMismatch {
                expected: currency,
                found: line.amount.currency(),
            }));
        }
        raw_lines.push(exact_line(line, document.pricing, mode)?);
    }

    // --- rounding phase --------------------------------------------------
    //
    // One pass per tax code, so `Document` can carry a running total across
    // the lines while `Line` simply rounds each in place. Both leave the lines
    // adding up to the totals exactly.
    let mut amounts: Vec<Vec<i128>> = raw_lines
        .iter()
        .map(|line| vec![0_i128; line.taxes.len()])
        .collect();

    match document.rounding_level {
        RoundingLevel::Line => {
            for (line_index, line) in raw_lines.iter().enumerate() {
                for (tax_index, tax) in line.taxes.iter().enumerate() {
                    let rounded = round_to_minor(tax.amount, currency, mode)?;
                    set(&mut amounts, line_index, tax_index, rounded)?;
                }
            }
        }
        RoundingLevel::Document => {
            // The running-total technique rather than proportional allocation:
            // it preserves the document total by construction, and it does the
            // right thing on a document that mixes positive lines with a
            // negative discount line, where proportional weights do not.
            for code_id in code_order(&raw_lines) {
                let mut exact_so_far: i128 = 0;
                let mut given_so_far: i128 = 0;

                for (line_index, line) in raw_lines.iter().enumerate() {
                    for (tax_index, tax) in line.taxes.iter().enumerate() {
                        if tax.applied.tax_code_id != code_id {
                            continue;
                        }
                        exact_so_far = exact_so_far
                            .checked_add(tax.amount)
                            .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;
                        let target = round_to_minor(exact_so_far, currency, mode)?;
                        let share = target
                            .checked_sub(given_so_far)
                            .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;
                        given_so_far = target;
                        set(&mut amounts, line_index, tax_index, share)?;
                    }
                }
            }
        }
    }

    // --- assembly --------------------------------------------------------
    let mut lines: Vec<LineResult> = Vec::with_capacity(raw_lines.len());
    for (line_index, raw) in raw_lines.iter().enumerate() {
        let shares = amounts.get(line_index).ok_or(TaxError::Internal)?;

        let mut taxes = Vec::with_capacity(raw.taxes.len());
        for (tax_index, tax) in raw.taxes.iter().enumerate() {
            let amount = *shares.get(tax_index).ok_or(TaxError::Internal)?;
            taxes.push(LineTax {
                applied: tax.applied.clone(),
                taxable: money(currency, round_to_minor(tax.taxable, currency, mode)?)?,
                amount: money(currency, amount)?,
            });
        }

        let tax_total = taxes
            .iter()
            .try_fold(0_i128, |sum, tax| sum.checked_add(tax.amount.scaled()))
            .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;

        // Under inclusive pricing the gross is the given, so the net absorbs
        // whatever rounding left over. Under exclusive pricing the net is the
        // given and the gross follows.
        let (net, gross) = match document.pricing {
            Pricing::Exclusive => {
                let net = round_to_minor(raw.net, currency, mode)?;
                let gross = net
                    .checked_add(tax_total)
                    .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;
                (net, gross)
            }
            Pricing::Inclusive => {
                let gross = raw.gross;
                let net = gross
                    .checked_sub(tax_total)
                    .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;
                (net, gross)
            }
        };

        lines.push(LineResult {
            net: money(currency, net)?,
            tax: money(currency, tax_total)?,
            gross: money(currency, gross)?,
            taxes,
        });
    }

    let by_tax = summarise(currency, &lines)?;

    let net =
        Money::total(currency, lines.iter().map(|line| line.net)).map_err(TaxError::Amount)?;
    let tax =
        Money::total(currency, lines.iter().map(|line| line.tax)).map_err(TaxError::Amount)?;
    let gross = net.checked_add(tax).map_err(TaxError::Amount)?;

    Ok(DocumentTax {
        currency,
        lines,
        by_tax,
        net,
        tax,
        gross,
    })
}

/// One line before any minor-unit rounding, at the storage scale.
struct RawLine {
    net: i128,
    gross: i128,
    taxes: Vec<RawTax>,
}

struct RawTax {
    applied: AppliedTax,
    taxable: i128,
    amount: i128,
}

/// Work one line out exactly, with no minor-unit rounding anywhere.
fn exact_line(line: &TaxLine, pricing: Pricing, mode: Rounding) -> Result<RawLine, TaxError> {
    // The factors depend on the rates alone, which is what makes inclusive
    // pricing a division rather than a search.
    let mut accumulated: i128 = 0;
    let mut factors: Vec<(i128, i128)> = Vec::with_capacity(line.treatment.taxes.len());

    for tax in &line.treatment.taxes {
        let base = if tax.is_compound {
            FACTOR_ONE
                .checked_add(accumulated)
                .ok_or(TaxError::Amount(MoneyError::OutOfRange))?
        } else {
            FACTOR_ONE
        };
        // scale 12 times scale 6, divided back to scale 12.
        let factor = base
            .checked_mul(tax.rate.scaled())
            .ok_or(TaxError::Amount(MoneyError::OutOfRange))?
            / RATE_ONE;
        accumulated = accumulated
            .checked_add(factor)
            .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;
        factors.push((base, factor));
    }

    let multiplier = FACTOR_ONE
        .checked_add(accumulated)
        .ok_or(TaxError::Amount(MoneyError::OutOfRange))?;

    let net = match pricing {
        Pricing::Exclusive => line.amount,
        // `1 + accumulated` cannot be zero: every rate is non-negative, so the
        // multiplier is at least one.
        Pricing::Inclusive => line
            .amount
            .scale_by(FACTOR_ONE, multiplier, mode)
            .map_err(TaxError::Amount)?,
    };

    let mut taxes = Vec::with_capacity(line.treatment.taxes.len());
    for (tax, (base, factor)) in line.treatment.taxes.iter().zip(factors) {
        taxes.push(RawTax {
            applied: tax.clone(),
            taxable: net
                .scale_by(base, FACTOR_ONE, mode)
                .map_err(TaxError::Amount)?
                .scaled(),
            amount: net
                .scale_by(factor, FACTOR_ONE, mode)
                .map_err(TaxError::Amount)?
                .scaled(),
        });
    }

    let gross = match pricing {
        Pricing::Exclusive => net
            .scale_by(multiplier, FACTOR_ONE, mode)
            .map_err(TaxError::Amount)?
            .scaled(),
        Pricing::Inclusive => line.amount.scaled(),
    };

    Ok(RawLine {
        net: net.scaled(),
        gross,
        taxes,
    })
}

/// Every tax code on the document, in the order it first appears.
///
/// Order is a rendering decision, and first-appearance is the one a reader can
/// follow: the summary box lists the taxes in the order the lines introduced
/// them rather than in whatever order a hash happened to produce.
fn code_order(lines: &[RawLine]) -> Vec<Uuid> {
    let mut order: Vec<Uuid> = Vec::new();
    for line in lines {
        for tax in &line.taxes {
            if !order.contains(&tax.applied.tax_code_id) {
                order.push(tax.applied.tax_code_id);
            }
        }
    }
    order
}

/// Add the lines up per tax code.
fn summarise(currency: Currency, lines: &[LineResult]) -> Result<Vec<TaxTotal>, TaxError> {
    let mut totals: Vec<TaxTotal> = Vec::new();

    for line in lines {
        for tax in &line.taxes {
            match totals
                .iter_mut()
                .find(|total| total.tax_code_id == tax.applied.tax_code_id)
            {
                Some(total) => {
                    total.taxable = total
                        .taxable
                        .checked_add(tax.taxable)
                        .map_err(TaxError::Amount)?;
                    total.amount = total
                        .amount
                        .checked_add(tax.amount)
                        .map_err(TaxError::Amount)?;
                }
                None => totals.push(TaxTotal {
                    tax_code_id: tax.applied.tax_code_id,
                    code: tax.applied.code.clone(),
                    name: tax.applied.name.clone(),
                    kind: tax.applied.kind,
                    rate: tax.applied.rate,
                    is_recoverable: tax.applied.is_recoverable,
                    taxable: tax.taxable,
                    amount: tax.amount,
                }),
            }
        }
    }

    // A code appearing at two different rates on one document is possible - a
    // document spanning a rate change - and the summary shows the rate of the
    // first line. Stated rather than hidden: the per-line figures are the
    // record, and each carries its own rate.
    let _ = currency;
    Ok(totals)
}

/// Round a scaled amount to the currency's minor unit, staying at scale 4.
fn round_to_minor(scaled: i128, currency: Currency, mode: Rounding) -> Result<i128, TaxError> {
    Ok(Money::from_scaled(currency, scaled)
        .and_then(|amount| amount.round_to_minor_unit(mode))
        .map_err(TaxError::Amount)?
        .scaled())
}

fn money(currency: Currency, scaled: i128) -> Result<Money, TaxError> {
    Money::from_scaled(currency, scaled).map_err(TaxError::Amount)
}

/// Write into the two-dimensional share table without indexing.
///
/// This crate compiles to wasm, where an out-of-bounds index is not a caught
/// panic but a frozen tab - so the bound is asked about rather than asserted.
fn set(
    amounts: &mut [Vec<i128>],
    line_index: usize,
    tax_index: usize,
    value: i128,
) -> Result<(), TaxError> {
    *amounts
        .get_mut(line_index)
        .and_then(|line| line.get_mut(tax_index))
        .ok_or(TaxError::Internal)? = value;
    Ok(())
}

/// `10^FACTOR_SCALE`, computed rather than trusted, so the constant and the
/// name it is derived from cannot drift apart.
#[cfg(test)]
fn factor_one() -> Option<i128> {
    crate::rate::pow10(FACTOR_SCALE)
}

/// What can go wrong computing a document's tax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaxError {
    #[error(transparent)]
    Amount(#[from] MoneyError),
    /// A share table index that cannot happen - the tables are built from the
    /// lines they are indexed by. Present because this crate may not panic.
    #[error("internal tax computation error")]
    Internal,
}

impl TaxError {
    pub fn message(self) -> Message {
        match self {
            Self::Amount(inner) => inner.message(),
            Self::Internal => msg!("tax.error.internal"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::TaxKind;

    fn currency(code: &str) -> Currency {
        Currency::parse(code).expect("a real currency")
    }

    fn id(byte: u8) -> Uuid {
        Uuid::from_bytes([byte; 16])
    }

    fn applied(byte: u8, percent: &str, is_compound: bool, sequence: i16) -> AppliedTax {
        AppliedTax {
            tax_code_id: id(byte),
            code: format!("T{byte}"),
            name: format!("Tax {byte}"),
            kind: TaxKind::Vat,
            rate: TaxRate::parse_percent(percent).expect("a valid rate"),
            is_compound,
            is_recoverable: true,
            sequence,
        }
    }

    fn treatment(taxes: Vec<AppliedTax>) -> TaxTreatment {
        TaxTreatment {
            tax_group_id: id(250),
            group_code: "G".to_owned(),
            taxes,
        }
    }

    fn money_of(code: &str, amount: &str) -> Money {
        Money::parse(currency(code), amount).expect("a valid amount")
    }

    fn document(lines: Vec<TaxLine>) -> TaxDocument {
        TaxDocument {
            currency: currency("USD"),
            pricing: Pricing::Exclusive,
            rounding_level: RoundingLevel::Line,
            rounding: Rounding::HalfUp,
            lines,
        }
    }

    #[test]
    fn the_factor_scale_constant_matches_its_name() {
        assert_eq!(factor_one(), Some(FACTOR_ONE));
    }

    #[test]
    fn a_single_tax_is_the_rate_times_the_line() {
        let doc = document(vec![TaxLine {
            amount: money_of("USD", "100.00"),
            treatment: treatment(vec![applied(1, "20", false, 0)]),
        }]);

        let result = compute(&doc).unwrap();
        assert_eq!(result.net, money_of("USD", "100.00"));
        assert_eq!(result.tax, money_of("USD", "20.00"));
        assert_eq!(result.gross, money_of("USD", "120.00"));
    }

    #[test]
    fn a_document_with_no_lines_still_has_totals() {
        let result = compute(&document(Vec::new())).unwrap();
        assert!(result.net.is_zero());
        assert!(result.gross.is_zero());
        assert_eq!(result.net.currency(), currency("USD"));
    }

    #[test]
    fn a_line_outside_the_scope_of_tax_is_its_own_gross() {
        let doc = document(vec![TaxLine::untaxed(money_of("USD", "50.00"))]);
        let result = compute(&doc).unwrap();

        assert_eq!(result.tax, Money::zero(currency("USD")));
        assert_eq!(result.gross, money_of("USD", "50.00"));
        assert!(result.by_tax.is_empty());
    }

    #[test]
    fn a_split_tax_lands_as_two_lines_on_the_summary() {
        // India: GST 18% is CGST 9% and SGST 9%, and the return needs them
        // apart. This is the case a single `rate` column cannot express.
        let doc = document(vec![TaxLine {
            amount: money_of("USD", "1000.00"),
            treatment: treatment(vec![applied(1, "9", false, 0), applied(2, "9", false, 1)]),
        }]);

        let result = compute(&doc).unwrap();
        assert_eq!(result.tax, money_of("USD", "180.00"));
        assert_eq!(result.by_tax.len(), 2);
        assert_eq!(result.by_tax[0].amount, money_of("USD", "90.00"));
        assert_eq!(result.by_tax[1].amount, money_of("USD", "90.00"));
    }

    #[test]
    fn a_compound_tax_is_charged_on_the_tax_before_it() {
        // Quebec: 5% GST, then 9.975% QST on the GST-inclusive amount.
        let doc = document(vec![TaxLine {
            amount: money_of("USD", "100.00"),
            treatment: treatment(vec![
                applied(1, "5", false, 0),
                applied(2, "9.975", true, 1),
            ]),
        }]);

        let result = compute(&doc).unwrap();
        assert_eq!(result.by_tax[0].amount, money_of("USD", "5.00"));
        // 9.975% of 105.00 = 10.47375, to the cent 10.47.
        assert_eq!(result.by_tax[1].amount, money_of("USD", "10.47"));
        assert_eq!(result.by_tax[1].taxable, money_of("USD", "105.00"));
        assert_eq!(result.gross, money_of("USD", "115.47"));
    }

    #[test]
    fn a_non_compound_tax_after_another_is_still_charged_on_the_net() {
        // The distinction the `is_compound` flag exists for: two 10% taxes
        // side by side are 20% of the net, not 21%.
        let doc = document(vec![TaxLine {
            amount: money_of("USD", "100.00"),
            treatment: treatment(vec![applied(1, "10", false, 0), applied(2, "10", false, 1)]),
        }]);

        let result = compute(&doc).unwrap();
        assert_eq!(result.tax, money_of("USD", "20.00"));
        assert_eq!(result.by_tax[1].taxable, money_of("USD", "100.00"));
    }

    #[test]
    fn inclusive_pricing_gives_back_exactly_the_price_that_was_quoted() {
        // The number on the shelf is the number at the till. If this ever
        // comes out a cent off, the customer is the one who notices.
        let doc = TaxDocument {
            pricing: Pricing::Inclusive,
            ..document(vec![TaxLine {
                amount: money_of("USD", "120.00"),
                treatment: treatment(vec![applied(1, "20", false, 0)]),
            }])
        };

        let result = compute(&doc).unwrap();
        assert_eq!(result.gross, money_of("USD", "120.00"));
        assert_eq!(result.net, money_of("USD", "100.00"));
        assert_eq!(result.tax, money_of("USD", "20.00"));
    }

    #[test]
    fn inclusive_pricing_balances_even_when_the_division_does_not_come_out() {
        // 9.99 at 20% is a net of 8.325, which is not a price. The cent goes
        // to the net, never to the tax - the tax is what gets remitted.
        let doc = TaxDocument {
            pricing: Pricing::Inclusive,
            ..document(vec![TaxLine {
                amount: money_of("USD", "9.99"),
                treatment: treatment(vec![applied(1, "20", false, 0)]),
            }])
        };

        let result = compute(&doc).unwrap();
        assert_eq!(result.gross, money_of("USD", "9.99"));
        assert_eq!(
            result.net.checked_add(result.tax).unwrap(),
            money_of("USD", "9.99"),
            "the derived figures have to add back to the quoted price",
        );
    }

    #[test]
    fn inclusive_pricing_survives_a_compound_chain() {
        let doc = TaxDocument {
            pricing: Pricing::Inclusive,
            ..document(vec![TaxLine {
                amount: money_of("USD", "115.47"),
                treatment: treatment(vec![
                    applied(1, "5", false, 0),
                    applied(2, "9.975", true, 1),
                ]),
            }])
        };

        let result = compute(&doc).unwrap();
        assert_eq!(result.gross, money_of("USD", "115.47"));
        assert_eq!(
            result.net.checked_add(result.tax).unwrap(),
            money_of("USD", "115.47")
        );
        // Back to where the exclusive test started, to the cent.
        assert_eq!(result.net, money_of("USD", "100.00"));
    }

    #[test]
    fn line_rounding_and_document_rounding_can_differ_and_both_add_up() {
        // Three lines of 0.33 at 20% are 0.066 each: 0.07 a line rounded
        // separately, 0.20 rounded once over the document. Both are defensible
        // and the document says which it used - that is the whole point of
        // storing the policy.
        let lines = || {
            vec![
                TaxLine {
                    amount: money_of("USD", "0.33"),
                    treatment: treatment(vec![applied(1, "20", false, 0)]),
                },
                TaxLine {
                    amount: money_of("USD", "0.33"),
                    treatment: treatment(vec![applied(1, "20", false, 0)]),
                },
                TaxLine {
                    amount: money_of("USD", "0.33"),
                    treatment: treatment(vec![applied(1, "20", false, 0)]),
                },
            ]
        };

        let per_line = compute(&document(lines())).unwrap();
        assert_eq!(per_line.tax, money_of("USD", "0.21"));

        let per_document = compute(&TaxDocument {
            rounding_level: RoundingLevel::Document,
            ..document(lines())
        })
        .unwrap();
        assert_eq!(per_document.tax, money_of("USD", "0.20"));

        // Whichever was used, the lines add up to the total. A document whose
        // lines do not sum to its own footer is one nobody can check.
        for result in [&per_line, &per_document] {
            let summed =
                Money::total(currency("USD"), result.lines.iter().map(|line| line.tax)).unwrap();
            assert_eq!(summed, result.tax);
        }
    }

    #[test]
    fn document_rounding_copes_with_a_negative_discount_line() {
        // Proportional allocation goes wrong here; the running-total technique
        // does not. A discount line is an ordinary thing on an invoice.
        let doc = TaxDocument {
            rounding_level: RoundingLevel::Document,
            ..document(vec![
                TaxLine {
                    amount: money_of("USD", "100.00"),
                    treatment: treatment(vec![applied(1, "20", false, 0)]),
                },
                TaxLine {
                    amount: money_of("USD", "-10.005"),
                    treatment: treatment(vec![applied(1, "20", false, 0)]),
                },
            ])
        };

        let result = compute(&doc).unwrap();
        let summed =
            Money::total(currency("USD"), result.lines.iter().map(|line| line.tax)).unwrap();
        assert_eq!(summed, result.tax);
        // 20% of 89.995 is 17.999, which is 18.00 to the cent.
        assert_eq!(result.tax, money_of("USD", "18.00"));
    }

    #[test]
    fn a_currency_with_no_minor_unit_rounds_to_whole_units() {
        // The yen. A tax of 187.5 yen is not a thing anybody can pay.
        let doc = TaxDocument {
            currency: currency("JPY"),
            ..document(vec![TaxLine {
                amount: Money::parse(currency("JPY"), "1875").unwrap(),
                treatment: treatment(vec![applied(1, "10", false, 0)]),
            }])
        };

        let result = compute(&doc).unwrap();
        assert_eq!(result.tax, Money::parse(currency("JPY"), "188").unwrap());
        assert_eq!(result.gross, Money::parse(currency("JPY"), "2063").unwrap());
    }

    #[test]
    fn half_even_and_half_up_disagree_where_they_should() {
        // Two systems agreeing on every rate and every line and differing by a
        // cent is exactly the dispute that storing the mode prevents.
        let line = || TaxLine {
            amount: money_of("USD", "1.25"),
            treatment: treatment(vec![applied(1, "20", false, 0)]),
        };

        let up = compute(&document(vec![line()])).unwrap();
        let even = compute(&TaxDocument {
            rounding: Rounding::HalfEven,
            ..document(vec![line()])
        })
        .unwrap();

        // 20% of 1.25 is 0.25 exactly, so both agree here.
        assert_eq!(up.tax, even.tax);

        // 0.125 is the boundary: half up gives 0.13, half even gives 0.12.
        let boundary = || TaxLine {
            amount: money_of("USD", "1.25"),
            treatment: treatment(vec![applied(1, "10", false, 0)]),
        };
        let up = compute(&document(vec![boundary()])).unwrap();
        let even = compute(&TaxDocument {
            rounding: Rounding::HalfEven,
            ..document(vec![boundary()])
        })
        .unwrap();

        assert_eq!(up.tax, money_of("USD", "0.13"));
        assert_eq!(even.tax, money_of("USD", "0.12"));
    }

    #[test]
    fn a_line_in_the_wrong_currency_is_refused() {
        let doc = document(vec![TaxLine::untaxed(money_of("EUR", "10.00"))]);

        assert!(matches!(
            compute(&doc),
            Err(TaxError::Amount(MoneyError::CurrencyMismatch { .. }))
        ));
    }

    #[test]
    fn recoverable_tax_is_separable_from_the_rest() {
        let doc = document(vec![TaxLine {
            amount: money_of("USD", "100.00"),
            treatment: treatment(vec![
                applied(1, "20", false, 0),
                AppliedTax {
                    is_recoverable: false,
                    ..applied(2, "5", false, 1)
                },
            ]),
        }]);

        let result = compute(&doc).unwrap();
        assert_eq!(result.tax, money_of("USD", "25.00"));
        assert_eq!(result.recoverable().unwrap(), money_of("USD", "20.00"));
    }

    #[test]
    fn a_zero_rate_produces_a_line_on_the_summary_rather_than_nothing() {
        // Zero-rated is not out of scope, and a return has to show it.
        let doc = document(vec![TaxLine {
            amount: money_of("USD", "100.00"),
            treatment: treatment(vec![applied(1, "0", false, 0)]),
        }]);

        let result = compute(&doc).unwrap();
        assert_eq!(result.by_tax.len(), 1);
        assert!(result.by_tax[0].amount.is_zero());
        assert_eq!(result.by_tax[0].taxable, money_of("USD", "100.00"));
    }

    #[test]
    fn the_summary_lists_taxes_in_the_order_the_lines_introduced_them() {
        let doc = document(vec![
            TaxLine {
                amount: money_of("USD", "10.00"),
                treatment: treatment(vec![applied(2, "5", false, 0)]),
            },
            TaxLine {
                amount: money_of("USD", "10.00"),
                treatment: treatment(vec![applied(1, "20", false, 0)]),
            },
        ]);

        let result = compute(&doc).unwrap();
        let order: Vec<&str> = result.by_tax.iter().map(|t| t.code.as_str()).collect();
        assert_eq!(order, vec!["T2", "T1"]);
    }
}
