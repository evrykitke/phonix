//! A tax code: which tax this is, where it applies, and how it behaves.
//!
//! # What is deliberately not here
//!
//! The rate. A code outlives its rates - UK VAT has been 15%, 17.5% and 20%
//! under one name - so the rate is an effective-dated row hanging off the code
//! rather than a column on it. See [`crate::rate::TaxRatePeriod`].
//!
//! # The two flags that are not cosmetic
//!
//! * `is_compound` — this tax is computed on the base *plus the taxes before it
//!   in sequence*. Quebec's QST on top of GST is the canonical case.
//! * `is_recoverable` — input tax that can be reclaimed, as opposed to a cost.
//!   It is what separates VAT a business gets back from a sales tax it does
//!   not, and it is a posting consequence rather than a label.

use phonix_core::Message;
use phonix_core::locale::Country;
use phonix_core::msg;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Longest code the column holds.
pub const MAX_CODE_LEN: usize = 20;

/// Longest name the column holds.
pub const MAX_NAME_LEN: usize = 120;

/// Longest region code the column holds - a state, province or district.
pub const MAX_REGION_LEN: usize = 20;

/// What kind of tax this is.
///
/// Five, and the column is check-constrained to them. This is not a
/// classification for reporting - it is what tells a posting routine whether an
/// amount is a liability, a recoverable asset or a deduction from a payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxKind {
    /// Value added tax: charged on output, reclaimed on input.
    Vat,
    /// Goods and services tax. Separate from VAT because the jurisdictions that
    /// use the word also split it - CGST, SGST, IGST - and a report that
    /// groups them has to be able to find them.
    Gst,
    /// Sales tax: charged at the point of sale, not reclaimable, and in the
    /// United States determined by where the buyer is.
    Sales,
    /// Withheld from a payment and remitted on the payee's behalf. The one
    /// kind that *reduces* what is paid rather than increasing it.
    Withholding,
    /// A duty on a specific good.
    Excise,
}

impl TaxKind {
    /// Every kind, in the order a picker should offer them.
    pub const ALL: &'static [Self] = &[
        Self::Vat,
        Self::Gst,
        Self::Sales,
        Self::Withholding,
        Self::Excise,
    ];

    /// The stored value - what the column's CHECK constraint lists.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vat => "vat",
            Self::Gst => "gst",
            Self::Sales => "sales",
            Self::Withholding => "withholding",
            Self::Excise => "excise",
        }
    }

    /// Read a stored value back.
    ///
    /// `None` rather than a default: a kind this build does not know decides
    /// how an amount posts, and guessing at that is worse than a failed read.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == raw)
    }

    /// What a picker shows.
    pub fn label(self) -> Message {
        match self {
            Self::Vat => msg!("tax.kind.vat"),
            Self::Gst => msg!("tax.kind.gst"),
            Self::Sales => msg!("tax.kind.sales"),
            Self::Withholding => msg!("tax.kind.withholding"),
            Self::Excise => msg!("tax.kind.excise"),
        }
    }

    /// Whether this kind normally reduces the payment rather than adding to it.
    ///
    /// Advisory: it seeds the flag on a new code, and the code's own
    /// `is_recoverable` is what actually decides. A withholding code with
    /// `is_recoverable` set is not a contradiction - some jurisdictions do
    /// allow the payee to credit it.
    pub const fn is_deduction(self) -> bool {
        matches!(self, Self::Withholding)
    }
}

/// One tax, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxCode {
    pub id: Uuid,
    /// Short, and unique case-insensitively. What appears on a document.
    pub code: String,
    pub name: String,
    pub kind: TaxKind,
    /// Where it applies. `None` for a code the workspace uses everywhere.
    pub country: Option<Country>,
    /// A state, province or district within the country.
    pub region_code: Option<String>,
    /// Computed on the base plus the taxes before it in sequence.
    pub is_compound: bool,
    /// Input tax that can be reclaimed, as opposed to a cost.
    pub is_recoverable: bool,
    pub is_active: bool,
}

/// One tax code as a list row: the code, plus what it is charged at *today*.
///
/// The rate is not a column on [`TaxCode`] - a code outlives its rates, and
/// putting one on the record would be the effective-dating mistake this design
/// exists to avoid. But a list of taxes with no rates on it answers none of the
/// questions somebody opens the screen with, so the rate in force on the day
/// they are looking is resolved and carried alongside.
///
/// `None` means no window covers today: a tax that has been defined and not yet
/// priced, which refuses every document that references it and is worth saying
/// out loud on the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxCodeSummary {
    pub code: TaxCode,
    pub rate_today: Option<crate::rate::TaxRate>,
}

/// A tax code being created or edited on a screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxCodeInput {
    /// `None` for one being created.
    pub id: Option<Uuid>,
    pub code: String,
    pub name: String,
    pub kind: TaxKind,
    pub country: Option<Country>,
    pub region_code: Option<String>,
    pub is_compound: bool,
    pub is_recoverable: bool,
    pub is_active: bool,
}

impl TaxCodeInput {
    /// What a blank form opens on.
    ///
    /// VAT and recoverable, because that is what most of the world is creating
    /// when it creates its first tax code, and a default that is usually right
    /// is a field somebody does not have to think about.
    pub fn blank() -> Self {
        Self {
            id: None,
            code: String::new(),
            name: String::new(),
            kind: TaxKind::Vat,
            country: None,
            region_code: None,
            is_compound: false,
            is_recoverable: true,
            is_active: true,
        }
    }

    /// Re-open an existing code for editing.
    pub fn from_code(code: &TaxCode) -> Self {
        Self {
            id: Some(code.id),
            code: code.code.clone(),
            name: code.name.clone(),
            kind: code.kind,
            country: code.country,
            region_code: code.region_code.clone(),
            is_compound: code.is_compound,
            is_recoverable: code.is_recoverable,
            is_active: code.is_active,
        }
    }

    /// Trim, and say what is still wrong.
    ///
    /// Returns the tidied values rather than mutating in place, so a caller
    /// cannot store the untrimmed draft by accident.
    pub fn check(&self) -> Result<TaxCodeInput, TaxCodeError> {
        let code = self.code.trim();
        let name = self.name.trim();
        let region = self
            .region_code
            .as_deref()
            .map(str::trim)
            .filter(|region| !region.is_empty());

        if code.is_empty() {
            return Err(TaxCodeError::CodeRequired);
        }
        if code.chars().count() > MAX_CODE_LEN {
            return Err(TaxCodeError::CodeTooLong);
        }
        // A code reaches a printed document and a filed return. Letters, digits
        // and the two separators - anything else is a paste that went wrong.
        if !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(TaxCodeError::CodeShape);
        }
        if name.is_empty() {
            return Err(TaxCodeError::NameRequired);
        }
        if name.chars().count() > MAX_NAME_LEN {
            return Err(TaxCodeError::NameTooLong);
        }
        if region.is_some_and(|region| region.chars().count() > MAX_REGION_LEN) {
            return Err(TaxCodeError::RegionTooLong);
        }
        // A region without a country is a district of nowhere. Refused rather
        // than ignored, because a US destination rate keyed on the region alone
        // would match a same-named region in another country.
        if region.is_some() && self.country.is_none() {
            return Err(TaxCodeError::RegionWithoutCountry);
        }

        Ok(Self {
            id: self.id,
            code: code.to_owned(),
            name: name.to_owned(),
            kind: self.kind,
            country: self.country,
            region_code: region.map(str::to_owned),
            is_compound: self.is_compound,
            is_recoverable: self.is_recoverable,
            is_active: self.is_active,
        })
    }
}

/// What can be wrong with a tax code somebody typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaxCodeError {
    #[error("a tax code needs a code")]
    CodeRequired,
    #[error("a tax code is at most 20 characters")]
    CodeTooLong,
    #[error("a tax code may contain only letters, digits, hyphens and underscores")]
    CodeShape,
    #[error("a tax code needs a name")]
    NameRequired,
    #[error("a tax code name is at most 120 characters")]
    NameTooLong,
    #[error("a region code is at most 20 characters")]
    RegionTooLong,
    #[error("a region belongs to a country, so name the country too")]
    RegionWithoutCountry,
}

impl TaxCodeError {
    /// What to say to whoever typed it, and which field to say it on.
    pub fn field(self) -> &'static str {
        match self {
            Self::CodeRequired | Self::CodeTooLong | Self::CodeShape => "code",
            Self::NameRequired | Self::NameTooLong => "name",
            Self::RegionTooLong | Self::RegionWithoutCountry => "region_code",
        }
    }

    pub fn message(self) -> Message {
        match self {
            Self::CodeRequired => msg!("tax.error.code_required"),
            Self::CodeTooLong => msg!("tax.error.code_too_long"),
            Self::CodeShape => msg!("tax.error.code_shape"),
            Self::NameRequired => msg!("tax.error.name_required"),
            Self::NameTooLong => msg!("tax.error.name_too_long"),
            Self::RegionTooLong => msg!("tax.error.region_too_long"),
            Self::RegionWithoutCountry => msg!("tax.error.region_without_country"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> TaxCodeInput {
        TaxCodeInput {
            code: "VAT20".to_owned(),
            name: "VAT standard rate".to_owned(),
            ..TaxCodeInput::blank()
        }
    }

    #[test]
    fn every_kind_round_trips_through_its_stored_value() {
        // The column is check-constrained to exactly these five, so a kind
        // that failed to parse would be a row nothing can read.
        for kind in TaxKind::ALL {
            assert_eq!(TaxKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(TaxKind::parse("carbon"), None);
    }

    #[test]
    fn a_blank_form_opens_on_the_usual_answer() {
        let blank = TaxCodeInput::blank();
        assert_eq!(blank.kind, TaxKind::Vat);
        assert!(blank.is_recoverable);
        assert!(blank.is_active);
        assert!(!blank.is_compound);
    }

    #[test]
    fn a_code_is_trimmed_rather_than_stored_as_typed() {
        let checked = TaxCodeInput {
            code: "  VAT20 ".to_owned(),
            name: " VAT standard rate  ".to_owned(),
            ..input()
        }
        .check()
        .unwrap();

        assert_eq!(checked.code, "VAT20");
        assert_eq!(checked.name, "VAT standard rate");
    }

    #[test]
    fn a_code_that_would_print_badly_on_a_document_is_refused() {
        for bad in ["VAT 20", "VAT/20", "VAT;DROP"] {
            let result = TaxCodeInput {
                code: bad.to_owned(),
                ..input()
            }
            .check();
            assert_eq!(result, Err(TaxCodeError::CodeShape), "{bad:?}");
        }
    }

    #[test]
    fn the_two_required_fields_are_the_two_that_appear_on_a_document() {
        assert_eq!(
            TaxCodeInput {
                code: "   ".to_owned(),
                ..input()
            }
            .check(),
            Err(TaxCodeError::CodeRequired)
        );
        assert_eq!(
            TaxCodeInput {
                name: String::new(),
                ..input()
            }
            .check(),
            Err(TaxCodeError::NameRequired)
        );
    }

    #[test]
    fn a_region_without_a_country_is_a_district_of_nowhere() {
        // Refused rather than ignored: a rate keyed on the region alone would
        // match a same-named region in another country.
        let result = TaxCodeInput {
            region_code: Some("CA".to_owned()),
            country: None,
            ..input()
        }
        .check();

        assert_eq!(result, Err(TaxCodeError::RegionWithoutCountry));
    }

    #[test]
    fn a_blank_region_is_no_region_rather_than_an_empty_one() {
        let checked = TaxCodeInput {
            region_code: Some("   ".to_owned()),
            ..input()
        }
        .check()
        .unwrap();

        assert_eq!(checked.region_code, None);
    }

    #[test]
    fn every_error_names_a_field_the_form_actually_has() {
        for error in [
            TaxCodeError::CodeRequired,
            TaxCodeError::CodeTooLong,
            TaxCodeError::CodeShape,
            TaxCodeError::NameRequired,
            TaxCodeError::NameTooLong,
            TaxCodeError::RegionTooLong,
            TaxCodeError::RegionWithoutCountry,
        ] {
            assert!(
                ["code", "name", "region_code"].contains(&error.field()),
                "{error:?} points at a field the form does not have",
            );
        }
    }
}
