//! Where to bill somebody, and where to send the goods.
//!
//! # The address on a document is a copy, not a reference
//!
//! [`PostalAddress`] is a value with no id. A document stores one of these
//! outright, because a customer who moves next year must not silently rewrite
//! last year's invoices - the same rule the tax snapshot follows, and the same
//! rule the audit trail's from/to shape follows. [`PartyAddress`] is the
//! *record*, which a screen edits; the value is what gets copied off it.
//!
//! # Purpose is not exclusive
//!
//! A party can have several billing addresses - a group with two registered
//! offices does - so `purpose` narrows a picker rather than keying a row. What
//! is at most one per purpose is [`PartyAddress::is_primary`], and that is a
//! default rather than a constraint on what exists.

use phonix_core::Message;
use phonix_core::locale::Country;
use phonix_core::msg;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Longest any single line of an address may be.
pub const MAX_ADDRESS_LINE: usize = 120;

/// What an address is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressPurpose {
    /// Where the invoice goes. The default, because a party that has one
    /// address has this one.
    #[default]
    Billing,
    /// Where the goods go. Often a different country, which is precisely why
    /// it cannot be the same field: destination tax depends on it.
    Shipping,
    Other,
}

impl AddressPurpose {
    pub const ALL: &'static [Self] = &[Self::Billing, Self::Shipping, Self::Other];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Billing => "billing",
            Self::Shipping => "shipping",
            Self::Other => "other",
        }
    }

    /// Read a stored value back.
    ///
    /// Falls back to [`Self::Other`] rather than refusing, unlike a tax kind:
    /// a purpose this build does not know is an address that shows in the wrong
    /// section of a screen, not an amount that posts wrongly.
    pub fn from_stored(raw: &str) -> Self {
        match raw {
            "billing" => Self::Billing,
            "shipping" => Self::Shipping,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> Message {
        match self {
            Self::Billing => msg!("party.address.billing"),
            Self::Shipping => msg!("party.address.shipping"),
            Self::Other => msg!("party.address.other_purpose"),
        }
    }
}

/// An address as a value: what a document copies and keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostalAddress {
    pub line1: Option<String>,
    pub line2: Option<String>,
    pub city: Option<String>,
    /// State, province or county - whatever the country calls the level below
    /// itself. Free text, because there is no list that covers every country.
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<Country>,
}

impl PostalAddress {
    /// Nothing filled in. Not the same as no address at all - a party that has
    /// not been asked yet has this.
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.line1.is_none()
            && self.line2.is_none()
            && self.city.is_none()
            && self.region.is_none()
            && self.postal_code.is_none()
            && self.country.is_none()
    }

    /// The lines, in the order they would be printed, blanks dropped.
    ///
    /// One function rather than every screen assembling its own, because a
    /// document, an envelope and a preview all have to agree - and the one that
    /// prints "None" is always the one nobody tested.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::with_capacity(5);

        for part in [&self.line1, &self.line2] {
            if let Some(value) = part.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                lines.push(value.to_owned());
            }
        }

        // City, region and postcode on one line: that is how a postal service
        // reads them, and three separate lines is how a label ends up taller
        // than the envelope.
        let locality: Vec<&str> = [&self.city, &self.region, &self.postal_code]
            .into_iter()
            .filter_map(|part| part.as_deref().map(str::trim).filter(|v| !v.is_empty()))
            .collect();
        if !locality.is_empty() {
            lines.push(locality.join(", "));
        }

        if let Some(country) = self.country {
            lines.push(country.name().to_owned());
        }

        lines
    }

    /// One line, for a grid cell or a subtitle.
    pub fn one_line(&self) -> String {
        self.lines().join(", ")
    }

    /// Trim every field, turning blanks into nothing.
    pub fn tidied(&self) -> Self {
        fn some(value: &Option<String>) -> Option<String> {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        }

        Self {
            line1: some(&self.line1),
            line2: some(&self.line2),
            city: some(&self.city),
            region: some(&self.region),
            postal_code: some(&self.postal_code),
            country: self.country,
        }
    }

    /// Whether any line is longer than the column holds.
    pub fn check(&self) -> Result<Self, AddressError> {
        let tidied = self.tidied();
        let too_long = [
            &tidied.line1,
            &tidied.line2,
            &tidied.city,
            &tidied.region,
            &tidied.postal_code,
        ]
        .into_iter()
        .flatten()
        .any(|value| value.chars().count() > MAX_ADDRESS_LINE);

        if too_long {
            return Err(AddressError::LineTooLong);
        }
        Ok(tidied)
    }
}

/// An address record belonging to a party.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyAddress {
    pub id: Uuid,
    pub party_id: Uuid,
    pub purpose: AddressPurpose,
    /// What to call it on screen: "Head office", "Warehouse 2".
    pub label: Option<String>,
    pub address: PostalAddress,
    /// The one a new document reaches for. At most one per purpose, kept so by
    /// the service rather than by a constraint - a partial unique index would
    /// refuse the moment somebody ticked the new one before unticking the old.
    pub is_primary: bool,
}

/// An address being added or edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyAddressInput {
    pub id: Option<Uuid>,
    pub purpose: AddressPurpose,
    pub label: Option<String>,
    pub address: PostalAddress,
    pub is_primary: bool,
}

impl PartyAddressInput {
    pub fn blank() -> Self {
        Self {
            id: None,
            purpose: AddressPurpose::Billing,
            label: None,
            address: PostalAddress::empty(),
            is_primary: false,
        }
    }

    pub fn from_address(address: &PartyAddress) -> Self {
        Self {
            id: Some(address.id),
            purpose: address.purpose,
            label: address.label.clone(),
            address: address.address.clone(),
            is_primary: address.is_primary,
        }
    }

    pub fn check(&self) -> Result<Self, AddressError> {
        let address = self.address.check()?;
        // An address with nothing in it is a row that says nothing and prints
        // as a blank block on a document.
        if address.is_empty() {
            return Err(AddressError::Empty);
        }

        let label = self
            .label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if label.is_some_and(|value| value.chars().count() > MAX_ADDRESS_LINE) {
            return Err(AddressError::LineTooLong);
        }

        Ok(Self {
            id: self.id,
            purpose: self.purpose,
            label: label.map(str::to_owned),
            address,
            is_primary: self.is_primary,
        })
    }
}

/// What can be wrong with an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("an address needs something in it")]
    Empty,
    #[error("an address line is at most 120 characters")]
    LineTooLong,
}

impl AddressError {
    pub fn message(self) -> Message {
        match self {
            Self::Empty => msg!("party.error.address_empty"),
            Self::LineTooLong => msg!("party.error.address_line_too_long"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> PostalAddress {
        PostalAddress {
            line1: Some("14 Harbour Road".to_owned()),
            line2: None,
            city: Some("Mombasa".to_owned()),
            region: None,
            postal_code: Some("80100".to_owned()),
            country: Country::parse("KE").ok(),
        }
    }

    #[test]
    fn the_printed_lines_drop_the_blanks() {
        // The screen that prints "None" is always the one nobody tested.
        assert_eq!(
            address().lines(),
            vec!["14 Harbour Road", "Mombasa, 80100", "Kenya"]
        );
    }

    #[test]
    fn locality_is_one_line_because_that_is_how_it_is_read() {
        let lines = PostalAddress {
            city: Some("Austin".to_owned()),
            region: Some("TX".to_owned()),
            postal_code: Some("78701".to_owned()),
            ..PostalAddress::empty()
        }
        .lines();

        assert_eq!(lines, vec!["Austin, TX, 78701"]);
    }

    #[test]
    fn an_empty_address_prints_nothing_at_all() {
        assert!(PostalAddress::empty().lines().is_empty());
        assert!(PostalAddress::empty().is_empty());
        assert_eq!(PostalAddress::empty().one_line(), "");
    }

    #[test]
    fn whitespace_is_not_a_value() {
        let tidied = PostalAddress {
            line1: Some("   ".to_owned()),
            city: Some(" Mombasa ".to_owned()),
            ..PostalAddress::empty()
        }
        .tidied();

        assert_eq!(tidied.line1, None);
        assert_eq!(tidied.city.as_deref(), Some("Mombasa"));
    }

    #[test]
    fn a_line_longer_than_the_column_is_refused() {
        let result = PostalAddress {
            line1: Some("x".repeat(MAX_ADDRESS_LINE + 1)),
            ..PostalAddress::empty()
        }
        .check();

        assert_eq!(result, Err(AddressError::LineTooLong));
    }

    #[test]
    fn an_address_record_with_nothing_in_it_is_refused() {
        let result = PartyAddressInput {
            address: PostalAddress::empty(),
            ..PartyAddressInput::blank()
        }
        .check();

        assert_eq!(result, Err(AddressError::Empty));
    }

    #[test]
    fn an_unknown_stored_purpose_is_other_rather_than_a_failed_read() {
        // Unlike a tax kind: the cost here is an address in the wrong section
        // of a screen, not an amount that posts wrongly.
        assert_eq!(
            AddressPurpose::from_stored("billing"),
            AddressPurpose::Billing
        );
        assert_eq!(
            AddressPurpose::from_stored("registered"),
            AddressPurpose::Other
        );
    }

    #[test]
    fn every_purpose_round_trips_through_its_stored_value() {
        for purpose in AddressPurpose::ALL {
            assert_eq!(AddressPurpose::from_stored(purpose.as_str()), *purpose);
        }
    }
}
