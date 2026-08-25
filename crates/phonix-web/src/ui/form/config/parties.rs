//! The party form.
//!
//! # Addresses and contacts are not fields here
//!
//! They are their own rows with their own saves, for the reason the logo is not
//! a field on the organization form: a draft opened before somebody else
//! corrected a postcode would put the old one back as a side effect of changing
//! a phone number. The detail screen puts them in panels beside this form.
//!
//! # The tax group picker is fetched, not written down
//!
//! Which is why this takes its choices as arguments. A list of tax groups
//! hard-coded here would be a second thing to keep in step with the tax screen,
//! and the failure would be a default treatment somebody can choose and nothing
//! can store.
//!
//! # `code` is editable, and that is deliberate
//!
//! Unlike a role's key, which code assigns by, a party's code is the
//! workspace's own reference and correcting a typo in one is an ordinary thing
//! to want. The unique index is what stops it being changed to one already in
//! use, and the service turns that into a message on the field.

use phonix_core::locale::{Country, Currency};
use phonix_core::permissions;
use phonix_master::address::{AddressPurpose, PartyAddressInput};
use phonix_master::contact::PartyContactInput;
use phonix_master::party::{PartyInput, PartyKind, PartyRole, roles};
use phonix_tax::group::TaxGroup;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::master_fns::save_party;
use crate::ui::form::{Choice, Field, FieldValue, FormAction, Then};

/// What "not chosen" is worth, as a select option.
///
/// An empty value rather than a sentinel word: it round-trips through every
/// `parse` here as a failure, which is exactly `None`.
const NOT_SET: &str = "";

/// Who this party is, and what this workspace does with them.
///
/// `tax_groups` and `currencies` are what the workspace actually has, fetched
/// by the screen. Passing them in rather than fetching here keeps this a pure
/// description of a form.
pub fn party_form(tax_groups: Vec<TaxGroup>, currencies: Vec<Currency>) -> FormConfig<PartyInput> {
    FormConfig::new("party", |draft: PartyInput| async move {
        save_party(draft).await
    })
    // --- who they are ----------------------------------------------------
    .field(
        Field::text("code", l!("field.code"), |m: &PartyInput| {
            FieldValue::text(&m.code)
        })
        .writing(|m, value| m.code = value.as_input())
        .placeholder("ACME01")
        .help(l!("parties.code_help"))
        .require(permissions::PARTIES_EDIT)
        .required(),
    )
    .field(
        Field::select(
            "kind",
            l!("field.kind"),
            PartyKind::ALL
                .iter()
                .map(|kind| Choice::new(kind.as_str(), crate::i18n::t(&kind.label())))
                .collect(),
            |m: &PartyInput| FieldValue::choice(m.kind.as_str()),
        )
        // Unrecognised keeps what was there: the kind decides which name goes
        // on a document, and silently becoming an organization would put the
        // wrong one there.
        .writing(|m, value| {
            if let Some(kind) = value.as_choice().and_then(PartyKind::parse) {
                m.kind = kind;
            }
        })
        .require(permissions::PARTIES_EDIT)
        .required(),
    )
    .field(
        Field::text("name", l!("field.name"), |m: &PartyInput| {
            FieldValue::text(&m.name)
        })
        .writing(|m, value| m.name = value.as_input())
        .placeholder("Acme Trading")
        .require(permissions::PARTIES_EDIT)
        .full_width()
        .required(),
    )
    .field(
        Field::text("legal_name", l!("field.legal_name"), |m: &PartyInput| {
            FieldValue::text(m.legal_name.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.legal_name = optional(value.as_input()))
        .placeholder("Acme Trading Limited")
        .help(l!("parties.legal_name_help"))
        .require(permissions::PARTIES_EDIT)
        .full_width(),
    )
    .field(
        Field::text("tax_id", l!("field.tax_id"), |m: &PartyInput| {
            FieldValue::text(m.tax_id.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.tax_id = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::select(
            "country",
            l!("field.country"),
            country_choices(),
            |m: &PartyInput| FieldValue::choice(m.country.map_or(NOT_SET, Country::code)),
        )
        // Unrecognised - including the blank option - is no country, which is a
        // legitimate answer while somebody is still filling the form in.
        .writing(|m, value| {
            m.country = value.as_choice().and_then(|code| Country::parse(code).ok());
        })
        .require(permissions::PARTIES_EDIT),
    )
    // --- how to reach them ------------------------------------------------
    .field(
        Field::email("email", l!("field.email"), |m: &PartyInput| {
            FieldValue::text(m.email.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.email = optional(value.as_input()))
        .placeholder("accounts@acme.example")
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::text("phone", l!("field.phone"), |m: &PartyInput| {
            FieldValue::text(m.phone.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.phone = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::text("website", l!("field.website"), |m: &PartyInput| {
            FieldValue::text(m.website.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.website = optional(value.as_input()))
        .placeholder("acme.example")
        .require(permissions::PARTIES_EDIT),
    )
    // --- how documents for them are built ---------------------------------
    .field(
        Field::select(
            "currency",
            l!("field.currency"),
            currency_choices(&currencies),
            |m: &PartyInput| FieldValue::choice(m.currency.map_or(NOT_SET, Currency::code)),
        )
        // Blank means "the workspace's own", which is a real answer here -
        // unlike on the organization form, where there is always a base.
        .writing(|m, value| {
            m.currency = value
                .as_choice()
                .and_then(|code| Currency::parse(code).ok());
        })
        .help(l!("parties.currency_help"))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::select(
            "tax_group_id",
            l!("parties.tax_group"),
            tax_group_choices(&tax_groups),
            |m: &PartyInput| {
                FieldValue::choice(
                    m.tax_group_id
                        .map_or_else(|| NOT_SET.to_owned(), |id| id.to_string()),
                )
            },
        )
        .writing(|m, value| {
            m.tax_group_id = value
                .as_choice()
                .and_then(|raw| raw.parse::<uuid::Uuid>().ok());
        })
        .help(l!("parties.tax_group_help"))
        .require(permissions::PARTIES_EDIT),
    )
    // --- what this workspace uses them for --------------------------------
    .field(
        Field::multi_select(
            "roles",
            l!("parties.roles"),
            role_choices(),
            |m: &PartyInput| FieldValue::choices(m.roles.iter().map(PartyRole::as_str)),
        )
        // Anything that does not parse is dropped rather than refused: the
        // vocabulary is open, and a role written by a future app is not this
        // form's to have an opinion about.
        .writing(|m, value| {
            m.roles = value
                .as_set()
                .iter()
                .filter_map(|raw| PartyRole::parse(raw).ok())
                .collect();
        })
        .help(l!("parties.roles_help"))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::toggle("is_active", l!("field.status"), |m: &PartyInput| {
            FieldValue::Bool(m.is_active)
        })
        .writing(|m, value| m.is_active = value.as_bool())
        .require(permissions::PARTIES_EDIT),
    )
    .action(
        FormAction::submit(l!("common.save"))
            .icon(Icon::Save)
            .then(Then::Say("Party saved."))
            .require(permissions::PARTIES_EDIT),
    )
}

/// Where to bill them, or where to send the goods.
///
/// The party id is closed over rather than carried on the draft, for the reason
/// the rate form closes over its tax code: an address that carried its own
/// party id could be submitted against a different party than the one the
/// screen is showing.
pub fn party_address_form(party_id: uuid::Uuid) -> FormConfig<PartyAddressInput> {
    FormConfig::new(
        "party-address",
        move |draft: PartyAddressInput| async move {
            crate::server_fns::master_fns::save_party_address(party_id, draft).await
        },
    )
    .field(
        Field::select(
            "purpose",
            l!("field.purpose"),
            AddressPurpose::ALL
                .iter()
                .map(|purpose| Choice::new(purpose.as_str(), crate::i18n::t(&purpose.label())))
                .collect(),
            |m: &PartyAddressInput| FieldValue::choice(m.purpose.as_str()),
        )
        // Unrecognised falls back to `Other`, which is what the stored value
        // does: the cost of getting this wrong is an address in the wrong
        // section of a screen, not a wrong amount.
        .writing(|m, value| {
            m.purpose = AddressPurpose::from_stored(value.as_choice().unwrap_or(""));
        })
        .require(permissions::PARTIES_EDIT)
        .required(),
    )
    .field(
        Field::text("label", l!("field.label"), |m: &PartyAddressInput| {
            FieldValue::text(m.label.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.label = optional(value.as_input()))
        .placeholder("Head office")
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::text("line1", l!("field.address"), |m: &PartyAddressInput| {
            FieldValue::text(m.address.line1.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.address.line1 = optional(value.as_input()))
        .placeholder("14 Harbour Road")
        .require(permissions::PARTIES_EDIT)
        .full_width(),
    )
    .field(
        Field::text(
            "line2",
            l!("organization.address_line2"),
            |m: &PartyAddressInput| FieldValue::text(m.address.line2.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.address.line2 = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT)
        .full_width(),
    )
    .field(
        Field::text("city", l!("organization.city"), |m: &PartyAddressInput| {
            FieldValue::text(m.address.city.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.address.city = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::text("region", l!("field.region"), |m: &PartyAddressInput| {
            FieldValue::text(m.address.region.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.address.region = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::text(
            "postal_code",
            l!("organization.postal_code"),
            |m: &PartyAddressInput| {
                FieldValue::text(m.address.postal_code.as_deref().unwrap_or(""))
            },
        )
        .writing(|m, value| m.address.postal_code = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::select(
            "country",
            l!("field.country"),
            country_choices(),
            |m: &PartyAddressInput| {
                FieldValue::choice(m.address.country.map_or(NOT_SET, Country::code))
            },
        )
        .writing(|m, value| {
            m.address.country = value.as_choice().and_then(|code| Country::parse(code).ok());
        })
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::toggle(
            "is_primary",
            l!("field.primary"),
            |m: &PartyAddressInput| FieldValue::Bool(m.is_primary),
        )
        // Ticking this unticks whichever other address held it, in the service.
        // A partial unique index would have refused the save the moment
        // somebody ticked the new one before unticking the old, which is the
        // order everybody does it in.
        .writing(|m, value| m.is_primary = value.as_bool())
        .require(permissions::PARTIES_EDIT),
    )
    .action(
        FormAction::submit(l!("common.save"))
            .icon(Icon::Save)
            .then(Then::Say("Address saved."))
            .then(Then::Refresh)
            .then(Then::Close)
            .require(permissions::PARTIES_EDIT),
    )
}

/// Who at that organization to actually write to.
pub fn party_contact_form(party_id: uuid::Uuid) -> FormConfig<PartyContactInput> {
    FormConfig::new(
        "party-contact",
        move |draft: PartyContactInput| async move {
            crate::server_fns::master_fns::save_party_contact(party_id, draft).await
        },
    )
    .field(
        Field::text("name", l!("field.name"), |m: &PartyContactInput| {
            FieldValue::text(&m.name)
        })
        .writing(|m, value| m.name = value.as_input())
        .require(permissions::PARTIES_EDIT)
        .required(),
    )
    .field(
        Field::text(
            "job_title",
            l!("field.job_title"),
            |m: &PartyContactInput| FieldValue::text(m.job_title.as_deref().unwrap_or("")),
        )
        .writing(|m, value| m.job_title = optional(value.as_input()))
        .placeholder("Accounts payable")
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::email("email", l!("field.email"), |m: &PartyContactInput| {
            FieldValue::text(m.email.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.email = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::text("phone", l!("field.phone"), |m: &PartyContactInput| {
            FieldValue::text(m.phone.as_deref().unwrap_or(""))
        })
        .writing(|m, value| m.phone = optional(value.as_input()))
        .require(permissions::PARTIES_EDIT),
    )
    .field(
        Field::toggle(
            "is_primary",
            l!("field.primary"),
            |m: &PartyContactInput| FieldValue::Bool(m.is_primary),
        )
        .writing(|m, value| m.is_primary = value.as_bool())
        .require(permissions::PARTIES_EDIT),
    )
    .action(
        FormAction::submit(l!("common.save"))
            .icon(Icon::Save)
            .then(Then::Say("Contact saved."))
            .then(Then::Refresh)
            .then(Then::Close)
            .require(permissions::PARTIES_EDIT),
    )
}

/// Blank is not a value.
fn optional(raw: String) -> Option<String> {
    (!raw.trim().is_empty()).then_some(raw)
}

/// Every country, by name, with a blank first option.
///
/// By name rather than by code, because that is the order somebody scanning a
/// dropdown expects.
fn country_choices() -> Vec<Choice> {
    std::iter::once(Choice::new(NOT_SET, l!("common.not_set")))
        .chain(
            Country::all_by_name()
                .into_iter()
                .map(|country| Choice::new(country.code(), country.name())),
        )
        .collect()
}

/// The currencies this workspace actually deals in.
///
/// Not every ISO code, unlike the organization form: that one is choosing the
/// workspace's base and has to offer everything, and this one is choosing among
/// what the workspace has switched on. Offering a currency with no rate on file
/// is offering a document that cannot be converted.
fn currency_choices(currencies: &[Currency]) -> Vec<Choice> {
    std::iter::once(Choice::new(NOT_SET, l!("common.not_set")))
        .chain(
            currencies
                .iter()
                .map(|currency| Choice::new(currency.code(), currency.label())),
        )
        .collect()
}

/// The tax groups this workspace has defined, active ones only.
///
/// A disabled group cannot be used on a new document, so offering one here
/// would be offering a default that is refused the moment somebody uses it.
fn tax_group_choices(groups: &[TaxGroup]) -> Vec<Choice> {
    std::iter::once(Choice::new(NOT_SET, l!("common.not_set")))
        .chain(
            groups
                .iter()
                .filter(|group| group.is_active)
                .map(|group| Choice::new(group.id.to_string(), &group.name).detail(&group.code)),
        )
        .collect()
}

/// The roles this build's own apps claim.
///
/// The stored vocabulary is open, so this is what is *offered* rather than what
/// is permitted: a party carrying a role from an app that is not installed
/// keeps it, and simply does not have a tick box here.
fn role_choices() -> Vec<Choice> {
    vec![
        Choice::new(roles::CUSTOMER, l!("parties.role.customer")),
        Choice::new(roles::SUPPLIER, l!("parties.role.supplier")),
        Choice::new(roles::CARRIER, l!("parties.role.carrier")),
        Choice::new(roles::AGENT, l!("parties.role.agent")),
    ]
}

#[cfg(test)]
mod tests {
    use leptos::prelude::Owner;

    use super::*;

    fn form() -> FormConfig<PartyInput> {
        Owner::new().with(|| party_form(Vec::new(), Vec::new()))
    }

    fn field_named<'a>(
        form: &'a FormConfig<PartyInput>,
        name: &str,
    ) -> &'a crate::ui::form::Field<PartyInput> {
        form.fields()
            .iter()
            .find(|field| field.name() == name)
            .unwrap_or_else(|| panic!("{name} is not a field on this form"))
    }

    #[test]
    fn every_field_is_gated_on_the_permission_the_service_requires() {
        for field in form().fields() {
            assert_eq!(
                field.permission(),
                Some(permissions::PARTIES_EDIT),
                "{} is not gated on Parties.Edit",
                field.name(),
            );
        }
    }

    #[test]
    fn only_the_three_that_appear_on_a_document_are_demanded() {
        let demanded: Vec<&'static str> = form()
            .fields()
            .iter()
            .filter(|field| field.is_required())
            .map(|field| field.name())
            .collect();

        assert!(demanded.contains(&"code"));
        assert!(demanded.contains(&"name"));
        assert!(demanded.contains(&"kind"));
        // A party may be filled in over time. Refusing to save a name without
        // an email address helps nobody.
        assert!(!demanded.contains(&"email"));
        assert!(!demanded.contains(&"country"));
    }

    #[test]
    fn addresses_and_contacts_are_not_fields_on_this_form() {
        // They are their own saves - see the module note. A field here would
        // let a stale draft revert somebody else's correction.
        let names: Vec<&'static str> = form().fields().iter().map(|f| f.name()).collect();

        assert!(!names.contains(&"addresses"));
        assert!(!names.contains(&"contacts"));
    }

    #[test]
    fn choosing_the_blank_currency_means_the_workspaces_own() {
        // A real answer here, unlike on the organization form where there is
        // always a base currency.
        let form = form();
        let currency = field_named(&form, "currency");

        let mut draft = PartyInput {
            currency: Currency::parse("KES").ok(),
            ..PartyInput::blank()
        };
        currency.apply(&mut draft, &FieldValue::choice(NOT_SET));

        assert_eq!(draft.currency, None);
    }

    #[test]
    fn an_unrecognised_kind_keeps_the_one_that_was_stored() {
        // The kind decides which name goes on a document.
        let form = form();
        let kind = field_named(&form, "kind");

        let mut draft = PartyInput {
            kind: PartyKind::Person,
            ..PartyInput::blank()
        };

        kind.apply(&mut draft, &FieldValue::choice("household"));
        assert_eq!(draft.kind, PartyKind::Person);

        kind.apply(&mut draft, &FieldValue::choice("organization"));
        assert_eq!(draft.kind, PartyKind::Organization);
    }

    #[test]
    fn a_role_the_form_cannot_parse_is_dropped_rather_than_refused() {
        // The vocabulary is open: a role written by a future app is not this
        // form's to have an opinion about.
        let form = form();
        let field = field_named(&form, "roles");

        let mut draft = PartyInput::blank();
        field.apply(
            &mut draft,
            &FieldValue::choices(["customer", "Not A Role", "supplier"]),
        );

        let held: Vec<&str> = draft.roles.iter().map(PartyRole::as_str).collect();
        assert_eq!(held, vec!["customer", "supplier"]);
    }

    #[test]
    fn a_disabled_tax_group_is_not_offered_as_a_default() {
        // It is refused the moment somebody uses it, so offering it would be
        // offering a default that cannot work.
        let group = |name: &str, is_active: bool| TaxGroup {
            id: uuid::Uuid::new_v4(),
            code: name.to_owned(),
            name: name.to_owned(),
            country: None,
            is_active,
            members: Vec::new(),
        };

        let choices = tax_group_choices(&[group("LIVE", true), group("OLD", false)]);

        // The blank option, plus the live group, and not the retired one.
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().any(|c| c.label == "LIVE"));
        assert!(!choices.iter().any(|c| c.label == "OLD"));
    }

    #[test]
    fn blank_text_is_written_back_as_none() {
        let form = form();
        let tax_id = field_named(&form, "tax_id");

        let mut draft = PartyInput {
            tax_id: Some("VAT123".to_owned()),
            ..PartyInput::blank()
        };
        tax_id.apply(&mut draft, &FieldValue::text("   "));

        assert_eq!(draft.tax_id, None);
    }
}
