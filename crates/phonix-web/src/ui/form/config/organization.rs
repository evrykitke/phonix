//! The organization profile form.
//!
//! # The logo is not a field
//!
//! It has no [`Field`] here and `save_organization_profile` ignores whatever
//! `logo_file_id` a submission carries. Attaching one is its own call, made
//! from its own panel beside this form, because a draft opened before somebody
//! else replaced the logo would otherwise put the old one back on every
//! document as a side effect of correcting a postcode.
//!
//! # Why the whole form is one gate
//!
//! Every field requires `Settings`, the same permission the service requires.
//! There is no partial version of this screen: somebody who may change the
//! registered name may change the address, because the two together are what
//! appears on a document and it is the document that is being trusted.
//!
//! # The pickers are the domain tables
//!
//! Country, currency and time zone are drawn from
//! [`phonix_core::locale`](phonix_core::locale) rather than from a list written
//! here. A second list would be a second thing to keep in step with the
//! validator, and the failure would be a code somebody can choose and nothing
//! can store.

use phonix_core::locale::{Country, Currency, Timezone};
use phonix_core::organization::{MONTHS, OrganizationProfile};
use phonix_core::permissions;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::save_organization_profile;
use crate::ui::form::{Choice, Field, FieldValue, FormAction, Then};

/// What "no country chosen" is worth, as a select option.
///
/// An empty value rather than a sentinel word: it round-trips through
/// [`Country::parse`] as a failure, which is exactly `None`.
const NO_COUNTRY: &str = "";

/// Who this workspace is.
pub fn organization_form() -> FormConfig<OrganizationProfile> {
    FormConfig::new("organization", |profile: OrganizationProfile| async move {
        save_organization_profile(profile).await
    })
    .note(l!("organization.note"))
    // --- who it is -------------------------------------------------------
    .field(
        Field::text(
            "legal_name",
            l!("organization.legal_name"),
            |m: &OrganizationProfile| FieldValue::text(&m.legal_name),
        )
        .writing(|m, value| m.legal_name = value.as_input())
        .placeholder("Northwind Trading Limited")
        .help(l!("organization.legal_name_help"))
        .require(permissions::SETTINGS)
        .full_width()
        .required(),
    )
    .field(
        Field::text(
            "trading_name",
            l!("organization.trading_name"),
            |m: &OrganizationProfile| FieldValue::text(m.trading_name.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.trading_name = optional(value.as_input()))
        .placeholder("Northwind")
        .help(l!("organization.trading_name_help"))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "industry",
            l!("organization.industry"),
            |m: &OrganizationProfile| FieldValue::text(m.industry.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.industry = optional(value.as_input()))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "registration_number",
            l!("organization.registration_number"),
            |m: &OrganizationProfile| {
                FieldValue::text(m.registration_number.as_deref().unwrap_or(""))
            },
        )
        .writing(|m, value| m.registration_number = optional(value.as_input()))
        .help(l!("organization.registration_help"))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "tax_id",
            l!("organization.tax_id"),
            |m: &OrganizationProfile| FieldValue::text(m.tax_id.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.tax_id = optional(value.as_input()))
        .help(l!("organization.tax_id_help"))
        .require(permissions::SETTINGS),
    )
    // --- how to reach it -------------------------------------------------
    .field(
        Field::email(
            "email",
            l!("organization.contact_email"),
            |m: &OrganizationProfile| FieldValue::text(m.email.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.email = optional(value.as_input()))
        .placeholder("hello@northwind.example")
        .help(l!("organization.contact_email_help"))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "phone",
            l!("organization.phone"),
            |m: &OrganizationProfile| FieldValue::text(m.phone.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.phone = optional(value.as_input()))
        .placeholder("+254 20 123 4567")
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "website",
            l!("organization.website"),
            |m: &OrganizationProfile| FieldValue::text(m.website.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.website = optional(value.as_input()))
        .placeholder("northwind.example")
        .require(permissions::SETTINGS),
    )
    // --- where it is -----------------------------------------------------
    .field(
        Field::text(
            "address_line1",
            l!("organization.address"),
            |m: &OrganizationProfile| FieldValue::text(m.address_line1.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.address_line1 = optional(value.as_input()))
        .placeholder("14 Harbour Road")
        .require(permissions::SETTINGS)
        .full_width(),
    )
    .field(
        Field::text(
            "address_line2",
            l!("organization.address_line2"),
            |m: &OrganizationProfile| FieldValue::text(m.address_line2.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.address_line2 = optional(value.as_input()))
        .require(permissions::SETTINGS)
        .full_width(),
    )
    .field(
        Field::text(
            "city",
            l!("organization.city"),
            |m: &OrganizationProfile| FieldValue::text(m.city.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.city = optional(value.as_input()))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "region",
            l!("organization.region"),
            |m: &OrganizationProfile| FieldValue::text(m.region.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.region = optional(value.as_input()))
        .help(l!("organization.region_help"))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::text(
            "postal_code",
            l!("organization.postal_code"),
            |m: &OrganizationProfile| FieldValue::text(m.postal_code.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.postal_code = optional(value.as_input()))
        .require(permissions::SETTINGS),
    )
    .field(
        Field::select(
            "country",
            l!("organization.country"),
            country_choices(),
            |m: &OrganizationProfile| {
                FieldValue::choice(m.country.map_or(NO_COUNTRY, Country::code))
            },
        )
        // Unrecognised - including the blank option - is no country, which is a
        // legitimate answer while somebody is still filling the form in.
        .writing(|m, value| {
            m.country = value.as_choice().and_then(|code| Country::parse(code).ok());
        })
        .require(permissions::SETTINGS),
    )
    // --- how it counts ---------------------------------------------------
    .field(
        Field::select(
            "currency",
            l!("organization.currency"),
            currency_choices(),
            |m: &OrganizationProfile| FieldValue::choice(m.currency.code()),
        )
        // Unrecognised keeps what was there. A currency silently becoming
        // dollars is every stored amount changing meaning at once.
        .writing(|m, value| {
            if let Some(currency) = value
                .as_choice()
                .and_then(|code| Currency::parse(code).ok())
            {
                m.currency = currency;
            }
        })
        .help(l!("organization.currency_help"))
        .require(permissions::SETTINGS)
        .required(),
    )
    .field(
        Field::select(
            "timezone",
            l!("organization.timezone"),
            timezone_choices(),
            |m: &OrganizationProfile| FieldValue::choice(m.timezone.as_str()),
        )
        .writing(|m, value| {
            if let Some(zone) = value
                .as_choice()
                .and_then(|name| Timezone::parse(name).ok())
            {
                m.timezone = zone;
            }
        })
        .help(l!("organization.timezone_help"))
        .require(permissions::SETTINGS)
        .required(),
    )
    .field(
        Field::select(
            "fiscal_year_start_month",
            l!("organization.fiscal_year"),
            month_choices(),
            |m: &OrganizationProfile| FieldValue::choice(m.fiscal_year_start_month.to_string()),
        )
        .writing(|m, value| {
            if let Some(month) = value
                .as_choice()
                .and_then(|raw| raw.parse::<u8>().ok())
                .filter(|month| (1..=12).contains(month))
            {
                m.fiscal_year_start_month = month;
            }
        })
        .require(permissions::SETTINGS)
        .required(),
    )
    .action(
        FormAction::submit(l!("organization.submit"))
            .icon(Icon::Save)
            .then(Then::Say("Organization details saved."))
            .require(permissions::SETTINGS),
    )
}

/// Blank is not a value - see the note on `OrganizationProfile`.
fn optional(raw: String) -> Option<String> {
    (!raw.trim().is_empty()).then_some(raw)
}

/// Every country, by name, with a blank first option.
///
/// By name rather than by code, because that is the order somebody scanning a
/// dropdown expects - `Andorra` first by code puts Afghanistan in the middle.
fn country_choices() -> Vec<Choice> {
    std::iter::once(Choice::new(NO_COUNTRY, "Not set"))
        .chain(
            Country::all_by_name()
                .into_iter()
                .map(|country| Choice::new(country.code(), country.name())),
        )
        .collect()
}

/// Every ISO 4217 currency, by code, labelled with its name.
fn currency_choices() -> Vec<Choice> {
    Currency::all()
        .iter()
        .map(|currency| Choice::new(currency.code(), currency.label()))
        .collect()
}

/// The common IANA zones. Anything outside the list still validates; it is
/// simply not offered - see [`Timezone::common`].
fn timezone_choices() -> Vec<Choice> {
    Timezone::common()
        .iter()
        .map(|name| Choice::new(*name, name.replace('_', " ")))
        .collect()
}

fn month_choices() -> Vec<Choice> {
    MONTHS
        .iter()
        .enumerate()
        .map(|(index, name)| Choice::new((index + 1).to_string(), *name))
        .collect()
}

#[cfg(test)]
mod tests {
    use leptos::prelude::Owner;

    use super::*;

    fn form() -> FormConfig<OrganizationProfile> {
        Owner::new().with(organization_form)
    }

    fn field_named(form: &FormConfig<OrganizationProfile>, name: &str) -> usize {
        form.fields()
            .iter()
            .position(|field| field.name() == name)
            .unwrap_or_else(|| panic!("{name} is not a field on this form"))
    }

    #[test]
    fn every_field_is_gated_on_the_permission_the_service_requires() {
        for field in form().fields() {
            assert_eq!(
                field.permission(),
                Some(permissions::SETTINGS),
                "{} is not gated on Settings",
                field.name(),
            );
        }
    }

    #[test]
    fn the_logo_is_not_a_field_on_this_form() {
        // It is attached by its own call - see the module note. A field here
        // would let a stale draft revert somebody else's change.
        assert!(
            form()
                .fields()
                .iter()
                .all(|field| field.name() != "logo_file_id"),
            "the logo must not be part of the profile form",
        );
    }

    #[test]
    fn only_the_registered_name_and_the_three_settings_are_demanded() {
        let missing: Vec<&'static str> = form()
            .fields()
            .iter()
            .filter(|field| field.is_required())
            .map(|field| field.name())
            .collect();

        assert!(missing.contains(&"legal_name"));
        // An address is not demanded: a workspace may fill this in over time,
        // and refusing to save a name without a postcode helps nobody.
        assert!(!missing.contains(&"city"));
        assert!(!missing.contains(&"tax_id"));

        // The three that always have a value cannot be cleared to nothing.
        assert!(missing.contains(&"currency"));
        assert!(missing.contains(&"timezone"));
        assert!(missing.contains(&"fiscal_year_start_month"));
    }

    #[test]
    fn choosing_the_blank_country_option_means_no_country() {
        let form = form();
        let index = field_named(&form, "country");
        let Some(country) = form.fields().get(index) else {
            panic!("country field vanished");
        };

        let mut draft = OrganizationProfile {
            country: Country::parse("KE").ok(),
            ..OrganizationProfile::empty()
        };
        country.apply(&mut draft, &FieldValue::choice(NO_COUNTRY));

        assert_eq!(draft.country, None);
    }

    #[test]
    fn an_unrecognised_currency_keeps_the_one_that_was_stored() {
        // A currency silently becoming dollars would change what every stored
        // amount means.
        let form = form();
        let index = field_named(&form, "currency");
        let Some(currency) = form.fields().get(index) else {
            panic!("currency field vanished");
        };

        let mut draft = OrganizationProfile {
            currency: Currency::parse("KES").unwrap_or_default(),
            ..OrganizationProfile::empty()
        };

        currency.apply(&mut draft, &FieldValue::choice("ZZZ"));
        assert_eq!(draft.currency.code(), "KES");

        currency.apply(&mut draft, &FieldValue::choice("JPY"));
        assert_eq!(draft.currency.code(), "JPY");
    }

    #[test]
    fn a_month_outside_the_year_is_ignored() {
        let form = form();
        let index = field_named(&form, "fiscal_year_start_month");
        let Some(month) = form.fields().get(index) else {
            panic!("fiscal month field vanished");
        };

        let mut draft = OrganizationProfile {
            fiscal_year_start_month: 7,
            ..OrganizationProfile::empty()
        };

        month.apply(&mut draft, &FieldValue::choice("13"));
        assert_eq!(draft.fiscal_year_start_month, 7);

        month.apply(&mut draft, &FieldValue::choice("4"));
        assert_eq!(draft.fiscal_year_start_month, 4);
    }

    #[test]
    fn blank_text_is_written_back_as_none() {
        let form = form();
        let index = field_named(&form, "tax_id");
        let Some(tax_id) = form.fields().get(index) else {
            panic!("tax id field vanished");
        };

        let mut draft = OrganizationProfile {
            tax_id: Some("VAT123".to_owned()),
            ..OrganizationProfile::empty()
        };

        tax_id.apply(&mut draft, &FieldValue::text("   "));
        assert_eq!(draft.tax_id, None);
    }

    #[test]
    fn the_pickers_offer_the_domain_tables_and_nothing_else() {
        // A second list would be a second thing to keep in step with the
        // validator, and the failure mode is a code nobody can store.
        assert_eq!(currency_choices().len(), Currency::all().len());
        assert_eq!(timezone_choices().len(), Timezone::common().len());
        assert_eq!(month_choices().len(), 12);
        // Every country, plus the blank option.
        assert_eq!(country_choices().len(), Country::all().len() + 1);
    }
}
