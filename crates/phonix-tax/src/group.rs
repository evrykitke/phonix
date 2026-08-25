//! A tax group: what a document line actually references.
//!
//! # Why a line never references a code
//!
//! "VAT 20%" is a group with one member. "GST 18%" is a group with CGST 9% and
//! SGST 9%. Quebec is a group with GST and a compound QST. A line that pointed
//! at a *code* would work in the first case and need a schema change for the
//! second - and the second is most of the world.
//!
//! One indirection buys India, Canada and US district rates at once. It costs a
//! join, and the join is worth it.
//!
//! # Sequence is not decoration
//!
//! Compound tax is computed on the base *plus the taxes before it*, so "before"
//! has to mean something. [`TaxGroupMember::sequence`] is that ordering,
//! stored, rather than whatever a query happened to return - because a total
//! that depends on a row order nobody declared is a total that changes when
//! somebody adds an index.
//!
//! # What a line stores is the resolution, not the group
//!
//! Rates change. A reprinted 2024 invoice must show 2024's rate and 2024's
//! name, so a line stores [`AppliedTax`] - the resolved values - and keeps the
//! group id only to say where they came from. Same discipline as the audit
//! trail's from/to shape: record what was true, not a pointer to what is true
//! now.

use chrono::NaiveDate;
use phonix_core::Message;
use phonix_core::locale::Country;
use phonix_core::msg;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::code::{MAX_CODE_LEN, MAX_NAME_LEN, TaxCode, TaxCodeError, TaxKind};
use crate::rate::{TaxRate, TaxRatePeriod};

/// The most taxes one group may hold.
///
/// Not a technical limit. A group of more than this is a modelling mistake -
/// the deepest real arrangement anyone files is three - and the ceiling is what
/// stops a compound chain whose arithmetic nobody can check by hand.
pub const MAX_MEMBERS: usize = 8;

/// A named tax treatment a line can point at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxGroup {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    /// Where it applies. `None` for a group the workspace uses everywhere.
    pub country: Option<Country>,
    pub is_active: bool,
    /// In sequence order, always.
    pub members: Vec<TaxGroupMember>,
}

impl TaxGroup {
    /// Whether any member is compound, which is what makes the order matter to
    /// a reader as well as to the arithmetic.
    pub fn is_compound(&self) -> bool {
        self.members.iter().any(|member| member.is_compound)
    }
}

/// One tax inside a group, and where it falls in the compound order.
///
/// Carries the code's identity as well as its id, because the settings screen
/// that lists a group's members should not have to fetch every code to name
/// them - and because a member is what an [`AppliedTax`] is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxGroupMember {
    pub tax_code_id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: TaxKind,
    pub is_compound: bool,
    pub is_recoverable: bool,
    /// Position in the compound order, ascending. Gaps are allowed; only the
    /// relative order is read.
    pub sequence: i16,
}

/// A group being created or edited on a screen.
///
/// Members are a list of code ids **in the order they should apply**. The
/// screen reorders by dragging; `sequence` is derived from the position, so
/// there is no number for somebody to get wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxGroupInput {
    pub id: Option<Uuid>,
    pub code: String,
    pub name: String,
    pub country: Option<Country>,
    pub is_active: bool,
    /// Tax code ids, in application order.
    pub members: Vec<Uuid>,
}

impl TaxGroupInput {
    pub fn blank() -> Self {
        Self {
            id: None,
            code: String::new(),
            name: String::new(),
            country: None,
            is_active: true,
            members: Vec::new(),
        }
    }

    /// Re-open an existing group for editing.
    pub fn from_group(group: &TaxGroup) -> Self {
        Self {
            id: Some(group.id),
            code: group.code.clone(),
            name: group.name.clone(),
            country: group.country,
            is_active: group.is_active,
            members: group
                .members
                .iter()
                .map(|member| member.tax_code_id)
                .collect(),
        }
    }

    /// Trim, and say what is still wrong.
    pub fn check(&self) -> Result<TaxGroupInput, TaxGroupError> {
        let code = self.code.trim();
        let name = self.name.trim();

        if code.is_empty() {
            return Err(TaxGroupError::Code(TaxCodeError::CodeRequired));
        }
        if code.chars().count() > MAX_CODE_LEN {
            return Err(TaxGroupError::Code(TaxCodeError::CodeTooLong));
        }
        if !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(TaxGroupError::Code(TaxCodeError::CodeShape));
        }
        if name.is_empty() {
            return Err(TaxGroupError::Code(TaxCodeError::NameRequired));
        }
        if name.chars().count() > MAX_NAME_LEN {
            return Err(TaxGroupError::Code(TaxCodeError::NameTooLong));
        }
        // A group with no members is not zero-rated, it is *nothing* - a line
        // pointing at it would attract no tax and no explanation of why. A
        // zero-rated supply is a group holding a code whose rate is zero, and
        // the difference is visible on the document.
        if self.members.is_empty() {
            return Err(TaxGroupError::NoMembers);
        }
        if self.members.len() > MAX_MEMBERS {
            return Err(TaxGroupError::TooManyMembers);
        }

        let mut seen: Vec<Uuid> = Vec::with_capacity(self.members.len());
        for member in &self.members {
            if seen.contains(member) {
                return Err(TaxGroupError::DuplicateMember);
            }
            seen.push(*member);
        }

        Ok(Self {
            id: self.id,
            code: code.to_owned(),
            name: name.to_owned(),
            country: self.country,
            is_active: self.is_active,
            members: self.members.clone(),
        })
    }
}

/// One tax, resolved for a date, as a document line stores it.
///
/// This is the snapshot. Everything a reprint needs is in here, so a document
/// printed in 2030 shows the 2024 name at the 2024 rate even after the code has
/// been renamed and the rate changed twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedTax {
    /// Where it came from. Kept for tracing, never re-resolved on a print.
    pub tax_code_id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: TaxKind,
    pub rate: TaxRate,
    pub is_compound: bool,
    pub is_recoverable: bool,
    pub sequence: i16,
}

/// A group resolved against a date: the taxes that actually apply, in order.
///
/// Produced once, when a document is created or its date changes, and then
/// stored. [`crate::compute`] takes this rather than a group, which is what
/// makes the computation a pure function of what is on the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxTreatment {
    /// The group these came from, for tracing.
    pub tax_group_id: Uuid,
    pub group_code: String,
    pub taxes: Vec<AppliedTax>,
}

impl TaxTreatment {
    /// Nothing applies. Not the same as a zero rate: this is a line outside the
    /// scope of tax altogether, and it carries no group.
    pub fn none() -> Self {
        Self {
            tax_group_id: Uuid::nil(),
            group_code: String::new(),
            taxes: Vec::new(),
        }
    }

    /// Resolve a group for a document date.
    ///
    /// `rates` answers "what was this code's rate on that day". A member with
    /// no rate in force is an error rather than a zero: a group configured for
    /// a country whose rate table has not been filled in would otherwise
    /// silently invoice at nothing, and nobody notices an invoice that is too
    /// small until the return is filed.
    pub fn resolve(
        group: &TaxGroup,
        on: NaiveDate,
        rates: &dyn Fn(Uuid) -> Option<TaxRatePeriod>,
    ) -> Result<Self, TaxGroupError> {
        let mut taxes = Vec::with_capacity(group.members.len());

        for member in &group.members {
            let Some(period) = rates(member.tax_code_id).filter(|period| period.covers(on)) else {
                return Err(TaxGroupError::NoRateOnDate {
                    code_id: member.tax_code_id,
                });
            };

            taxes.push(AppliedTax {
                tax_code_id: member.tax_code_id,
                code: member.code.clone(),
                name: member.name.clone(),
                kind: member.kind,
                rate: period.rate,
                is_compound: member.is_compound,
                is_recoverable: member.is_recoverable,
                sequence: member.sequence,
            });
        }

        // Sorted here rather than trusted from the caller. The arithmetic reads
        // "the taxes before it in sequence" as "earlier in this vector", and
        // making that true once is cheaper than every reader checking.
        taxes.sort_by_key(|tax| tax.sequence);

        Ok(Self {
            tax_group_id: group.id,
            group_code: group.code.clone(),
            taxes,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.taxes.is_empty()
    }
}

/// A member of a group, ready to be stored, built from a code and its position.
pub fn member_from(code: &TaxCode, position: usize) -> TaxGroupMember {
    TaxGroupMember {
        tax_code_id: code.id,
        code: code.code.clone(),
        name: code.name.clone(),
        kind: code.kind,
        is_compound: code.is_compound,
        is_recoverable: code.is_recoverable,
        // Positions are small and the column is a SMALLINT; a group past
        // `MAX_MEMBERS` is refused long before this could saturate.
        sequence: i16::try_from(position).unwrap_or(i16::MAX),
    }
}

/// What can be wrong with a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaxGroupError {
    #[error(transparent)]
    Code(#[from] TaxCodeError),
    #[error("a tax group needs at least one tax in it")]
    NoMembers,
    #[error("a tax group holds at most 8 taxes")]
    TooManyMembers,
    #[error("a tax appears twice in this group")]
    DuplicateMember,
    #[error("one of this group's taxes has no rate in force on that date")]
    NoRateOnDate { code_id: Uuid },
}

impl TaxGroupError {
    pub fn message(self) -> Message {
        match self {
            Self::Code(inner) => inner.message(),
            Self::NoMembers => msg!("tax.error.group_empty"),
            Self::TooManyMembers => msg!("tax.error.group_too_many"),
            Self::DuplicateMember => msg!("tax.error.group_duplicate"),
            Self::NoRateOnDate { .. } => msg!("tax.error.no_rate_on_date"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, of: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
    }

    fn id(byte: u8) -> Uuid {
        Uuid::from_bytes([byte; 16])
    }

    fn member(byte: u8, sequence: i16, is_compound: bool) -> TaxGroupMember {
        TaxGroupMember {
            tax_code_id: id(byte),
            code: format!("T{byte}"),
            name: format!("Tax {byte}"),
            kind: TaxKind::Gst,
            is_compound,
            is_recoverable: true,
            sequence,
        }
    }

    fn group(members: Vec<TaxGroupMember>) -> TaxGroup {
        TaxGroup {
            id: id(200),
            code: "GST18".to_owned(),
            name: "GST 18%".to_owned(),
            country: None,
            is_active: true,
            members,
        }
    }

    fn period(percent: &str) -> TaxRatePeriod {
        TaxRatePeriod {
            rate: TaxRate::parse_percent(percent).expect("a valid rate"),
            valid_from: day(2020, 1, 1),
            valid_to: None,
        }
    }

    fn input() -> TaxGroupInput {
        TaxGroupInput {
            code: "GST18".to_owned(),
            name: "GST 18%".to_owned(),
            members: vec![id(1), id(2)],
            ..TaxGroupInput::blank()
        }
    }

    #[test]
    fn a_group_with_nothing_in_it_is_refused() {
        // Not zero-rated - nothing. A zero-rated supply is a group holding a
        // code whose rate is zero, and the difference shows on the document.
        let result = TaxGroupInput {
            members: Vec::new(),
            ..input()
        }
        .check();

        assert_eq!(result, Err(TaxGroupError::NoMembers));
    }

    #[test]
    fn the_same_tax_cannot_be_in_a_group_twice() {
        let result = TaxGroupInput {
            members: vec![id(1), id(1)],
            ..input()
        }
        .check();

        assert_eq!(result, Err(TaxGroupError::DuplicateMember));
    }

    #[test]
    fn a_group_deeper_than_anyone_files_is_refused() {
        let result = TaxGroupInput {
            members: (1..=9).map(id).collect(),
            ..input()
        }
        .check();

        assert_eq!(result, Err(TaxGroupError::TooManyMembers));
    }

    #[test]
    fn resolving_puts_the_taxes_in_sequence_order() {
        // The arithmetic reads "the taxes before it" as "earlier in this
        // vector", so making that true once is cheaper than every reader
        // checking.
        let group = group(vec![member(2, 20, true), member(1, 10, false)]);
        let treatment =
            TaxTreatment::resolve(&group, day(2026, 6, 1), &|_| Some(period("9"))).unwrap();

        let order: Vec<&str> = treatment
            .taxes
            .iter()
            .map(|tax| tax.code.as_str())
            .collect();
        assert_eq!(order, vec!["T1", "T2"]);
    }

    #[test]
    fn a_member_with_no_rate_on_the_day_is_an_error_not_a_zero() {
        // An invoice that is silently too small is not noticed until the
        // return is filed.
        let group = group(vec![member(1, 10, false)]);
        let result = TaxTreatment::resolve(&group, day(2026, 6, 1), &|_| None);

        assert_eq!(result, Err(TaxGroupError::NoRateOnDate { code_id: id(1) }));
    }

    #[test]
    fn a_rate_that_had_expired_by_the_document_date_does_not_apply() {
        let group = group(vec![member(1, 10, false)]);
        let expired = TaxRatePeriod {
            valid_to: Some(day(2026, 4, 1)),
            ..period("9")
        };

        let result = TaxTreatment::resolve(&group, day(2026, 6, 1), &|_| Some(expired));
        assert_eq!(result, Err(TaxGroupError::NoRateOnDate { code_id: id(1) }));
    }

    #[test]
    fn the_resolution_carries_the_name_and_rate_of_the_day() {
        // What makes a 2030 reprint of a 2024 invoice show 2024's figures.
        let group = group(vec![member(1, 10, false)]);
        let treatment =
            TaxTreatment::resolve(&group, day(2024, 6, 1), &|_| Some(period("9"))).unwrap();

        let tax = &treatment.taxes[0];
        assert_eq!(tax.name, "Tax 1");
        assert_eq!(tax.rate, TaxRate::parse("0.09").unwrap());
        assert_eq!(treatment.tax_group_id, group.id);
    }

    #[test]
    fn nothing_applying_is_a_treatment_of_its_own() {
        let none = TaxTreatment::none();
        assert!(none.is_empty());
        assert_eq!(none.tax_group_id, Uuid::nil());
    }

    #[test]
    fn a_member_takes_its_sequence_from_where_it_was_dropped() {
        let code = TaxCode {
            id: id(7),
            code: "QST".to_owned(),
            name: "Quebec sales tax".to_owned(),
            kind: TaxKind::Gst,
            country: None,
            region_code: None,
            is_compound: true,
            is_recoverable: true,
            is_active: true,
        };

        let member = member_from(&code, 1);
        assert_eq!(member.sequence, 1);
        assert!(member.is_compound);
        assert_eq!(member.code, "QST");
    }
}
