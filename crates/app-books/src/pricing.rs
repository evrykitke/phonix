//! What an invoice comes to.
//!
//! # The seam
//!
//! This module owns exactly one job: turning `quantity × unit price` into a
//! line amount, and handing the result to [`phonix_tax::compute`]. It does no
//! tax arithmetic of its own and it touches no database. `phonix-tax` never
//! learns what an invoice is; this never learns what a rate table is.
//!
//! That is what makes the preview honest. The browser calls [`PricedInvoice::
//! compute`] as the person types, the server calls the same function when the
//! invoice is posted, and the two agree by construction rather than by two
//! implementations happening to match.
//!
//! # One rounding per line, and it is not the visible one
//!
//! A line amount is `unit_price.scale_by(quantity, 1)`, rounded once, at the
//! storage scale of four places. The rounding somebody *sees* - to the
//! currency's minor unit - happens later and once, inside `phonix_tax`, where
//! [`RoundingLevel`](phonix_tax::compute::RoundingLevel) says whether it lands
//! on each line or on the document total.
//!
//! Rounding the line amount to cents here and then taxing it would be rounding
//! twice, which is how a thousand-unit order at 0.0125 each comes out wrong.

use phonix_core::locale::Currency;
use phonix_core::money::{Money, MoneyError, Rounding};
use phonix_core::{Message, msg};
use phonix_tax::compute::{DocumentTax, Pricing, RoundingLevel, TaxDocument, TaxLine};
use phonix_tax::group::TaxTreatment;

use crate::quantity::Quantity;

/// One line, ready to price.
///
/// The treatment is already resolved against the document's date - that is
/// [`phonix_services::master::tax::treatment_on`]'s job, and it is deliberately
/// done before this crate sees it so that pricing stays a pure function of what
/// is on the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedLine {
    pub quantity: Quantity,
    pub unit_price: Money,
    pub treatment: TaxTreatment,
}

impl PricedLine {
    /// A line outside the scope of tax. Not the same as a zero-rated one.
    pub fn untaxed(quantity: Quantity, unit_price: Money) -> Self {
        Self {
            quantity,
            unit_price,
            treatment: TaxTreatment::none(),
        }
    }

    /// `quantity × unit price`, rounded once, at the storage scale.
    pub fn amount(&self, rounding: Rounding) -> Result<Money, PricingError> {
        self.quantity
            .times(self.unit_price, rounding)
            .map_err(PricingError::Amount)
    }
}

/// An invoice, ready to price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedInvoice {
    /// Every amount on the document. Taken explicitly so an invoice with no
    /// lines still has totals, for the reason [`Money::total`] takes one.
    pub currency: Currency,
    pub pricing: Pricing,
    pub rounding_level: RoundingLevel,
    pub rounding: Rounding,
    pub lines: Vec<PricedLine>,
}

impl PricedInvoice {
    /// Work out what the document comes to.
    ///
    /// Every intermediate stays at the storage scale until `phonix_tax` rounds
    /// once, where the policies on the document say it should.
    pub fn compute(&self) -> Result<DocumentTax, PricingError> {
        let mut lines = Vec::with_capacity(self.lines.len());

        for line in &self.lines {
            if line.unit_price.currency() != self.currency {
                return Err(PricingError::Amount(MoneyError::CurrencyMismatch {
                    expected: self.currency,
                    found: line.unit_price.currency(),
                }));
            }

            lines.push(TaxLine {
                amount: line.amount(self.rounding)?,
                treatment: line.treatment.clone(),
            });
        }

        phonix_tax::compute(&TaxDocument {
            currency: self.currency,
            pricing: self.pricing,
            rounding_level: self.rounding_level,
            rounding: self.rounding,
            lines,
        })
        .map_err(PricingError::Tax)
    }
}

/// What can go wrong pricing an invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PricingError {
    #[error(transparent)]
    Amount(MoneyError),
    #[error(transparent)]
    Tax(phonix_tax::compute::TaxError),
}

impl PricingError {
    pub fn message(self) -> Message {
        match self {
            Self::Amount(inner) => inner.message(),
            Self::Tax(inner) => inner.message(),
        }
    }

    /// The words to put beside the totals when they cannot be worked out.
    ///
    /// A total that silently reads zero because the arithmetic failed is worse
    /// than no total: somebody sends it.
    pub fn heading(self) -> Message {
        msg!("books.error.cannot_price")
    }
}

#[cfg(test)]
mod tests {
    use phonix_tax::code::TaxKind;
    use phonix_tax::group::AppliedTax;
    use phonix_tax::rate::TaxRate;
    use uuid::Uuid;

    use super::*;

    fn usd() -> Currency {
        Currency::parse("USD").expect("a real currency")
    }

    fn money(amount: &str) -> Money {
        Money::parse(usd(), amount).expect("a valid amount")
    }

    fn quantity(raw: &str) -> Quantity {
        Quantity::parse(raw).expect("a valid quantity")
    }

    fn vat(percent: &str) -> TaxTreatment {
        TaxTreatment {
            tax_group_id: Uuid::from_bytes([9; 16]),
            group_code: "STD".to_owned(),
            taxes: vec![AppliedTax {
                tax_code_id: Uuid::from_bytes([1; 16]),
                code: "VAT".to_owned(),
                name: "VAT".to_owned(),
                kind: TaxKind::Vat,
                rate: TaxRate::parse_percent(percent).expect("a valid rate"),
                is_compound: false,
                is_recoverable: true,
                sequence: 0,
            }],
        }
    }

    fn invoice(lines: Vec<PricedLine>) -> PricedInvoice {
        PricedInvoice {
            currency: usd(),
            pricing: Pricing::Exclusive,
            rounding_level: RoundingLevel::Line,
            rounding: Rounding::HalfUp,
            lines,
        }
    }

    #[test]
    fn a_line_is_quantity_times_price_and_then_tax() {
        let result = invoice(vec![PricedLine {
            quantity: quantity("3"),
            unit_price: money("19.99"),
            treatment: vat("20"),
        }])
        .compute()
        .unwrap();

        assert_eq!(result.net, money("59.97"));
        assert_eq!(result.tax, money("11.99")); // 20% of 59.97 is 11.994
        assert_eq!(result.gross, money("71.96"));
    }

    #[test]
    fn an_invoice_with_no_lines_still_has_totals() {
        let result = invoice(Vec::new()).compute().unwrap();

        assert!(result.gross.is_zero());
        assert_eq!(result.gross.currency(), usd());
    }

    #[test]
    fn the_line_amount_is_not_rounded_to_cents_before_it_is_taxed() {
        // A thousand at 0.0125 is 12.50 exactly. Rounding the *unit* to a cent
        // first would have made it 1000 x 0.01 = 10.00, and rounding the line
        // amount before tax would round twice.
        let result = invoice(vec![PricedLine::untaxed(quantity("1000"), money("0.0125"))])
            .compute()
            .unwrap();

        assert_eq!(result.net, money("12.50"));
    }

    #[test]
    fn a_discount_line_reduces_the_tax_as_well_as_the_total() {
        // A negative line is an ordinary thing on an invoice, and the tax has
        // to follow it down.
        let result = invoice(vec![
            PricedLine {
                quantity: quantity("1"),
                unit_price: money("100.00"),
                treatment: vat("20"),
            },
            PricedLine {
                quantity: quantity("-1"),
                unit_price: money("10.00"),
                treatment: vat("20"),
            },
        ])
        .compute()
        .unwrap();

        assert_eq!(result.net, money("90.00"));
        assert_eq!(result.tax, money("18.00"));
        assert_eq!(result.gross, money("108.00"));
    }

    #[test]
    fn a_price_in_the_wrong_currency_is_refused_rather_than_added_up() {
        let eur = Currency::parse("EUR").unwrap();
        let result = invoice(vec![PricedLine::untaxed(
            quantity("1"),
            Money::parse(eur, "10.00").unwrap(),
        )])
        .compute();

        assert!(matches!(
            result,
            Err(PricingError::Amount(MoneyError::CurrencyMismatch { .. }))
        ));
    }

    #[test]
    fn inclusive_pricing_gives_back_the_price_that_was_quoted() {
        // What the browser previews as somebody types a retail price.
        let result = PricedInvoice {
            pricing: Pricing::Inclusive,
            ..invoice(vec![PricedLine {
                quantity: quantity("1"),
                unit_price: money("120.00"),
                treatment: vat("20"),
            }])
        }
        .compute()
        .unwrap();

        assert_eq!(result.gross, money("120.00"));
        assert_eq!(result.net, money("100.00"));
        assert_eq!(result.tax, money("20.00"));
    }

    #[test]
    fn the_lines_always_add_up_to_the_document_total() {
        // Whichever rounding level is used. A document whose lines do not sum
        // to its own footer is one nobody can check.
        let lines = || {
            (0..3)
                .map(|_| PricedLine {
                    quantity: quantity("1"),
                    unit_price: money("0.33"),
                    treatment: vat("20"),
                })
                .collect::<Vec<_>>()
        };

        for level in [RoundingLevel::Line, RoundingLevel::Document] {
            let result = PricedInvoice {
                rounding_level: level,
                ..invoice(lines())
            }
            .compute()
            .unwrap();

            let summed = Money::total(usd(), result.lines.iter().map(|line| line.tax)).unwrap();
            assert_eq!(
                summed, result.tax,
                "{level:?} lines do not sum to the total"
            );
        }
    }

    #[test]
    fn a_split_tax_reaches_the_summary_as_two_lines() {
        // India: the return needs CGST and SGST apart, and the invoice has to
        // print them apart.
        let split = TaxTreatment {
            tax_group_id: Uuid::from_bytes([9; 16]),
            group_code: "GST18".to_owned(),
            taxes: vec![
                AppliedTax {
                    tax_code_id: Uuid::from_bytes([1; 16]),
                    code: "CGST".to_owned(),
                    name: "Central GST".to_owned(),
                    kind: TaxKind::Gst,
                    rate: TaxRate::parse_percent("9").unwrap(),
                    is_compound: false,
                    is_recoverable: true,
                    sequence: 0,
                },
                AppliedTax {
                    tax_code_id: Uuid::from_bytes([2; 16]),
                    code: "SGST".to_owned(),
                    name: "State GST".to_owned(),
                    kind: TaxKind::Gst,
                    rate: TaxRate::parse_percent("9").unwrap(),
                    is_compound: false,
                    is_recoverable: true,
                    sequence: 1,
                },
            ],
        };

        let result = invoice(vec![PricedLine {
            quantity: quantity("1"),
            unit_price: money("1000.00"),
            treatment: split,
        }])
        .compute()
        .unwrap();

        assert_eq!(result.by_tax.len(), 2);
        assert_eq!(result.tax, money("180.00"));
        assert_eq!(result.by_tax[0].code, "CGST");
        assert_eq!(result.by_tax[1].code, "SGST");
    }
}
