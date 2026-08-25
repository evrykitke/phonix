//! The organization behind a workspace: who it legally is, where it is, and
//! what it counts in.
//!
//! # Not the same fact as the tenant's display name
//!
//! The catalog already stores a `display_name` per tenant. That one is routing
//! and support metadata - what to call this workspace in an operator's list -
//! and it is set at signup by whoever typed a name into a box. This is the
//! *legal entity*: the name that goes on a document, alongside a registration
//! number and an address. Two different facts, and conflating them means the
//! first invoice carries whatever somebody typed while they were signing up.
//!
//! # One type, not two
//!
//! Unlike [`MailSettings`](crate::mail::MailSettings) there is no secret here,
//! so what a screen reads and what it submits are the same shape. Every field
//! that comes back is a field that can be sent.
//!
//! # Blank is `None`
//!
//! Every optional field is `Option<String>` and [`OrganizationProfile::normalised`]
//! maps a whitespace-only value onto `None`. A form submits `""` for a box
//! nobody typed in, and storing that means `WHERE tax_id IS NULL` misses half
//! the rows it should find.

use serde::{Deserialize, Serialize};

use crate::files::FileId;
use crate::identity::validation::FieldError;
use crate::locale::{Country, Currency, Timezone};
use crate::msg;

/// Longest legal or trading name accepted, in characters.
pub const MAX_NAME_LEN: usize = 200;
/// Longest registration number, tax id, industry or address line.
pub const MAX_LINE_LEN: usize = 120;
/// Longest website. Generous - some tracking-laden URLs are long, and this is
/// only here to stop somebody pasting a document into the box.
pub const MAX_URL_LEN: usize = 300;

/// Everything an organization has told us about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationProfile {
    // --- who it is -------------------------------------------------------
    /// The registered entity name. The one field that is required, because a
    /// profile without it is not a profile.
    pub legal_name: String,
    /// What it trades as, when that differs. Empty for most organizations.
    pub trading_name: Option<String>,
    /// Companies-house number, EIN, CR number - whatever the jurisdiction
    /// calls it. Free text on purpose: there are as many formats as there are
    /// registries, and validating one of them refuses the rest.
    pub registration_number: Option<String>,
    /// VAT, GST, PIN, TIN. Free text for the same reason.
    pub tax_id: Option<String>,
    pub industry: Option<String>,

    // --- how to reach it -------------------------------------------------
    /// The organization's own address, not any user's. Where a customer replies
    /// to a document, which is rarely the relay's from-address.
    pub email: Option<String>,
    pub phone: Option<String>,
    pub website: Option<String>,

    // --- where it is -----------------------------------------------------
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    /// State, province, county or region. One field, because the level that
    /// matters differs by country and asking for the wrong one is worse than
    /// asking for neither.
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<Country>,

    // --- how it counts ---------------------------------------------------
    /// What amounts are denominated in. See [`Currency`] for why this is not a
    /// three-letter string.
    pub currency: Currency,
    /// What "today" means for this organization. See [`Timezone`].
    pub timezone: Timezone,
    /// The month the financial year opens, 1-12. January for most, but April,
    /// July and October are all common enough that guessing is wrong yearly.
    pub fiscal_year_start_month: u8,

    // --- branding --------------------------------------------------------
    /// The uploaded logo that goes on documents, if one has been set.
    ///
    /// Read here, written elsewhere. The profile form does not carry it and
    /// saving the form does not change it - see the service - because a draft
    /// opened before somebody else replaced the logo would put the old one
    /// back on every document without anybody choosing that.
    pub logo_file_id: Option<FileId>,
}

impl OrganizationProfile {
    /// What a workspace has on the day it is created.
    ///
    /// An empty legal name is a real state, not a missing row: the profile is
    /// seeded by the migration so that every read finds one, and the screen
    /// shows a nudge until somebody fills it in.
    pub fn empty() -> Self {
        Self {
            legal_name: String::new(),
            trading_name: None,
            registration_number: None,
            tax_id: None,
            industry: None,
            email: None,
            phone: None,
            website: None,
            address_line1: None,
            address_line2: None,
            city: None,
            region: None,
            postal_code: None,
            country: None,
            currency: Currency::USD,
            timezone: Timezone::utc(),
            fiscal_year_start_month: 1,
            logo_file_id: None,
        }
    }

    /// What to call this organization on screen.
    ///
    /// The trading name when there is one, because that is the name its
    /// customers know it by.
    pub fn display_name(&self) -> &str {
        self.trading_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.legal_name)
    }

    /// Whether there is enough here to put on a document.
    ///
    /// A name and an address, which is what an invoice, a contract or a letter
    /// needs. Deliberately not "every field is filled" - a tax id is not
    /// something every organization has.
    pub fn is_complete(&self) -> bool {
        !self.legal_name.trim().is_empty()
            && self.country.is_some()
            && filled(&self.address_line1)
            && filled(&self.city)
    }

    /// Trim every field and map blanks onto `None`.
    ///
    /// Run before validating and before storing, so that what is checked and
    /// what is written are the same value.
    #[must_use]
    pub fn normalised(mut self) -> Self {
        self.legal_name = self.legal_name.trim().to_owned();
        // The email is lower-cased along with the rest, so that a profile
        // address matches one typed anywhere else in the application.
        self.email = blank_to_none(self.email).map(|value| value.to_ascii_lowercase());

        for field in [
            &mut self.trading_name,
            &mut self.registration_number,
            &mut self.tax_id,
            &mut self.industry,
            &mut self.phone,
            &mut self.website,
            &mut self.address_line1,
            &mut self.address_line2,
            &mut self.city,
            &mut self.region,
            &mut self.postal_code,
        ] {
            *field = blank_to_none(field.take());
        }

        self
    }

    /// Check what can be checked without asking anybody.
    ///
    /// Every problem rather than the first, so a person fixing the form is not
    /// sent round the loop once per field.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();

        let legal_name = self.legal_name.trim();
        if legal_name.is_empty() {
            errors.push(FieldError::new(
                "legal_name",
                msg!("validation.organization.legal_name_required"),
            ));
        } else if legal_name.chars().count() > MAX_NAME_LEN {
            errors.push(too_long("legal_name", MAX_NAME_LEN));
        }

        check_length(
            &mut errors,
            "trading_name",
            &self.trading_name,
            MAX_NAME_LEN,
        );
        check_length(
            &mut errors,
            "registration_number",
            &self.registration_number,
            MAX_LINE_LEN,
        );
        check_length(&mut errors, "tax_id", &self.tax_id, MAX_LINE_LEN);
        check_length(&mut errors, "industry", &self.industry, MAX_LINE_LEN);
        check_length(
            &mut errors,
            "address_line1",
            &self.address_line1,
            MAX_LINE_LEN,
        );
        check_length(
            &mut errors,
            "address_line2",
            &self.address_line2,
            MAX_LINE_LEN,
        );
        check_length(&mut errors, "city", &self.city, MAX_LINE_LEN);
        check_length(&mut errors, "region", &self.region, MAX_LINE_LEN);
        check_length(&mut errors, "postal_code", &self.postal_code, MAX_LINE_LEN);

        // Only the shape, and only when one was given. These are contact
        // details for a letterhead, not credentials - refusing a valid oddity
        // costs more than accepting one that is wrong.
        if let Some(email) = self.email.as_deref().filter(|v| !v.trim().is_empty())
            && !is_addressish(email)
        {
            errors.push(FieldError::new(
                "email",
                msg!("validation.email.not_an_address"),
            ));
        }

        if let Some(phone) = self.phone.as_deref().filter(|v| !v.trim().is_empty())
            && !is_phonish(phone)
        {
            errors.push(FieldError::new(
                "phone",
                msg!("validation.organization.phone"),
            ));
        }

        if let Some(website) = self.website.as_deref().filter(|v| !v.trim().is_empty()) {
            if website.chars().count() > MAX_URL_LEN {
                errors.push(too_long("website", MAX_URL_LEN));
            } else if !is_websiteish(website) {
                errors.push(FieldError::new(
                    "website",
                    msg!("validation.organization.website"),
                ));
            }
        }

        if !(1..=12).contains(&self.fiscal_year_start_month) {
            errors.push(FieldError::new(
                "fiscal_year_start_month",
                msg!("validation.organization.fiscal_month"),
            ));
        }

        errors
    }
}

impl Default for OrganizationProfile {
    fn default() -> Self {
        Self::empty()
    }
}

/// The twelve months, for the fiscal-year picker.
///
/// Here rather than in the form so that anything rendering a stored profile -
/// a document, a report header - reads the same names.
pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The name of month `1..=12`, or `None` for anything else.
pub fn month_name(month: u8) -> Option<&'static str> {
    MONTHS.get(usize::from(month.checked_sub(1)?)).copied()
}

fn filled(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| !v.trim().is_empty())
}

fn blank_to_none(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn too_long(field: &str, max: usize) -> FieldError {
    FieldError::new(
        field,
        msg!("validation.organization.field_too_long", max = max),
    )
}

fn check_length(errors: &mut Vec<FieldError>, field: &str, value: &Option<String>, max: usize) {
    if let Some(value) = value.as_deref()
        && value.chars().count() > max
    {
        errors.push(too_long(field, max));
    }
}

/// Shape only: something before the @, a dotted domain after it.
fn is_addressish(value: &str) -> bool {
    let value = value.trim();

    match value.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !value.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

/// Permissive on purpose. Phone numbers are written a dozen ways and the only
/// thing worth refusing is a value with no digits in it at all.
fn is_phonish(value: &str) -> bool {
    let digits = value.chars().filter(char::is_ascii_digit).count();

    (5..=20).contains(&digits)
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '(' | ')' | ' ' | '.'))
}

/// Shape only: a dotted host, with or without a scheme.
///
/// Not parsed as a URL - that would need a dependency in the wasm bundle to
/// refuse `example` and accept `example.com`, which is the whole check.
fn is_websiteish(value: &str) -> bool {
    let value = value.trim();
    let host = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    let host = host.split(['/', '?', '#']).next().unwrap_or(host);

    !host.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && !value.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fields a profile complained about, owned so the caller can hold it.
    fn complaints(profile: &OrganizationProfile) -> Vec<String> {
        profile
            .validate()
            .into_iter()
            .map(|error| error.field)
            .collect()
    }

    fn filled_profile() -> OrganizationProfile {
        OrganizationProfile {
            legal_name: "Northwind Trading Limited".to_owned(),
            trading_name: Some("Northwind".to_owned()),
            email: Some("hello@northwind.example".to_owned()),
            phone: Some("+254 20 123 4567".to_owned()),
            website: Some("https://northwind.example".to_owned()),
            address_line1: Some("14 Harbour Road".to_owned()),
            city: Some("Mombasa".to_owned()),
            country: Country::parse("KE").ok(),
            currency: Currency::parse("KES").unwrap(),
            timezone: Timezone::parse("Africa/Nairobi").unwrap(),
            fiscal_year_start_month: 7,
            ..OrganizationProfile::empty()
        }
    }

    #[test]
    fn a_new_workspace_starts_empty_and_valid_only_once_named() {
        let empty = OrganizationProfile::empty();

        // The seeded row is not complete, and says so - but it is a row, and
        // every read finds it.
        assert!(!empty.is_complete());
        let errors = empty.validate();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors.first().map(|e| e.field.as_str()), Some("legal_name"));
    }

    #[test]
    fn a_filled_profile_validates() {
        assert!(filled_profile().validate().is_empty());
        assert!(filled_profile().is_complete());
    }

    #[test]
    fn completeness_is_what_a_document_needs_not_every_field() {
        // No tax id, no registration number, no region - still enough to print.
        let profile = OrganizationProfile {
            tax_id: None,
            registration_number: None,
            region: None,
            ..filled_profile()
        };

        assert!(profile.is_complete());
    }

    #[test]
    fn an_address_without_a_country_is_not_complete() {
        let profile = OrganizationProfile {
            country: None,
            ..filled_profile()
        };

        assert!(!profile.is_complete());
    }

    #[test]
    fn blanks_become_none_so_is_null_means_what_it_says() {
        let submitted = OrganizationProfile {
            legal_name: "  Northwind Trading Limited  ".to_owned(),
            trading_name: Some("   ".to_owned()),
            tax_id: Some(String::new()),
            city: Some("  Mombasa  ".to_owned()),
            ..OrganizationProfile::empty()
        }
        .normalised();

        assert_eq!(submitted.legal_name, "Northwind Trading Limited");
        assert_eq!(submitted.trading_name, None);
        assert_eq!(submitted.tax_id, None);
        assert_eq!(submitted.city.as_deref(), Some("Mombasa"));
    }

    #[test]
    fn the_email_is_lowercased_like_every_other_address() {
        let profile = OrganizationProfile {
            email: Some("  Hello@Northwind.Example ".to_owned()),
            ..OrganizationProfile::empty()
        }
        .normalised();

        assert_eq!(profile.email.as_deref(), Some("hello@northwind.example"));
    }

    #[test]
    fn the_display_name_prefers_what_customers_call_it() {
        assert_eq!(filled_profile().display_name(), "Northwind");

        let no_trading_name = OrganizationProfile {
            trading_name: None,
            ..filled_profile()
        };
        assert_eq!(no_trading_name.display_name(), "Northwind Trading Limited");

        // A trading name of spaces is not a trading name.
        let blank = OrganizationProfile {
            trading_name: Some("   ".to_owned()),
            ..filled_profile()
        };
        assert_eq!(blank.display_name(), "Northwind Trading Limited");
    }

    #[test]
    fn optional_contact_details_are_only_checked_when_given() {
        let none_given = OrganizationProfile {
            email: None,
            phone: None,
            website: None,
            ..filled_profile()
        };

        assert!(none_given.validate().is_empty());
    }

    #[test]
    fn a_malformed_contact_detail_names_its_own_field() {
        let profile = OrganizationProfile {
            email: Some("not-an-address".to_owned()),
            phone: Some("call me".to_owned()),
            website: Some("example".to_owned()),
            ..filled_profile()
        };

        let fields = complaints(&profile);

        assert!(fields.iter().any(|field| field == "email"));
        assert!(fields.iter().any(|field| field == "phone"));
        assert!(fields.iter().any(|field| field == "website"));
    }

    #[test]
    fn a_website_is_accepted_with_or_without_a_scheme() {
        for good in [
            "example.com",
            "www.example.com",
            "http://example.com",
            "https://example.com/about",
            "https://sub.example.co.uk/a/b?c=d",
        ] {
            let profile = OrganizationProfile {
                website: Some(good.to_owned()),
                ..filled_profile()
            };
            assert!(profile.validate().is_empty(), "{good} should be accepted");
        }
    }

    #[test]
    fn phone_numbers_are_accepted_in_the_shapes_people_write_them() {
        for good in [
            "+254 20 123 4567",
            "(020) 7946-0958",
            "020.7946.0958",
            "5551234",
        ] {
            let profile = OrganizationProfile {
                phone: Some(good.to_owned()),
                ..filled_profile()
            };
            assert!(profile.validate().is_empty(), "{good} should be accepted");
        }
    }

    #[test]
    fn the_fiscal_year_must_open_in_a_real_month() {
        for bad in [0, 13, 255] {
            let profile = OrganizationProfile {
                fiscal_year_start_month: bad,
                ..filled_profile()
            };
            assert!(
                complaints(&profile)
                    .iter()
                    .any(|field| field == "fiscal_year_start_month"),
                "{bad} was accepted",
            );
        }
    }

    #[test]
    fn month_names_are_one_based_and_bounded() {
        assert_eq!(month_name(1), Some("January"));
        assert_eq!(month_name(7), Some("July"));
        assert_eq!(month_name(12), Some("December"));
        assert_eq!(month_name(0), None);
        assert_eq!(month_name(13), None);
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        // Somebody who got three things wrong should be told three times, not
        // sent round the loop.
        let profile = OrganizationProfile {
            legal_name: String::new(),
            email: Some("nope".to_owned()),
            fiscal_year_start_month: 0,
            ..filled_profile()
        };

        assert_eq!(profile.validate().len(), 3);
    }

    #[test]
    fn round_trips_through_json() {
        let profile = filled_profile();
        let json = serde_json::to_string(&profile).unwrap();
        assert_eq!(
            serde_json::from_str::<OrganizationProfile>(&json).unwrap(),
            profile,
        );
    }
}
