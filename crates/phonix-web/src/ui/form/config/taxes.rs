//! The tax code form, the rate form, and the tax group form.
//!
//! Three forms rather than one screen, because they are three different acts
//! performed at three different moments. Defining a tax is settled once.
//! Filing a rate change happens when a government says so, and is the one that
//! must not disturb what is already stored. Putting taxes into a group is a
//! modelling decision, and the order of the members is part of it.
//!
//! # The rate box says percent
//!
//! [`TaxRate`](phonix_tax::rate::TaxRate) holds a *proportion*, because that is
//! what the arithmetic wants. The box says percent, because that is what a
//! government publishes. The two differ by a factor of a hundred, so the
//! conversion happens in exactly one place -
//! [`TaxRate::parse_percent`](phonix_tax::rate::TaxRate::parse_percent) - and
//! this form is the only door that word comes through.

use phonix_core::locale::Country;
use phonix_core::permissions;
use phonix_tax::code::{TaxCode, TaxCodeInput, TaxKind};
use phonix_tax::group::TaxGroupInput;
use phonix_tax::rate::TaxRateInput;
use uuid::Uuid;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::master_fns::{save_tax_code, save_tax_group};
use crate::ui::form::{Choice, Field, FieldValue, FormAction, Then};

/// What "not chosen" is worth, as a select option.
const NOT_SET: &str = "";

/// What this tax is, where it applies, and how it behaves.
pub fn tax_code_form() -> FormConfig<TaxCodeInput> {
    FormConfig::new("tax-code", |draft: TaxCodeInput| async move {
        save_tax_code(draft).await
    })
    .field(
        Field::text("code", l!("field.code"), |m: &TaxCodeInput| {
            FieldValue::text(&m.code)
        })
        .writing(|m, value| m.code = value.as_input())
        .placeholder("VAT20")
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::select(
            "kind",
            l!("field.kind"),
            TaxKind::ALL
                .iter()
                .map(|kind| Choice::new(kind.as_str(), crate::i18n::t(&kind.label())))
                .collect(),
            |m: &TaxCodeInput| FieldValue::choice(m.kind.as_str()),
        )
        // Unrecognised keeps what was there. The kind decides whether an amount
        // posts as a liability, a recoverable asset or a deduction, and a
        // silent change of that is a silent change to the ledger.
        .writing(|m, value| {
            if let Some(kind) = value.as_choice().and_then(TaxKind::parse) {
                m.kind = kind;
            }
        })
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::text("name", l!("field.name"), |m: &TaxCodeInput| {
            FieldValue::text(&m.name)
        })
        .writing(|m, value| m.name = value.as_input())
        .placeholder("VAT standard rate")
        .require(permissions::TAXES_EDIT)
        .full_width()
        .required(),
    )
    .field(
        Field::select(
            "country",
            l!("field.country"),
            country_choices(),
            |m: &TaxCodeInput| FieldValue::choice(m.country.map_or(NOT_SET, Country::code)),
        )
        .writing(|m, value| {
            m.country = value.as_choice().and_then(|code| Country::parse(code).ok());
        })
        .require(permissions::TAXES_EDIT),
    )
    .field(
        Field::text("region_code", l!("field.region"), |m: &TaxCodeInput| {
            FieldValue::text(m.region_code.as_deref().unwrap_or(""))
        })
        .writing(|m, value| {
            let raw = value.as_input();
            m.region_code = (!raw.trim().is_empty()).then_some(raw);
        })
        .placeholder("TX")
        .help(l!("taxes.region_help"))
        .require(permissions::TAXES_EDIT),
    )
    .field(
        Field::toggle("is_compound", l!("taxes.compound"), |m: &TaxCodeInput| {
            FieldValue::Bool(m.is_compound)
        })
        .writing(|m, value| m.is_compound = value.as_bool())
        .help(l!("taxes.compound_help"))
        .require(permissions::TAXES_EDIT),
    )
    .field(
        Field::toggle(
            "is_recoverable",
            l!("taxes.recoverable"),
            |m: &TaxCodeInput| FieldValue::Bool(m.is_recoverable),
        )
        .writing(|m, value| m.is_recoverable = value.as_bool())
        .help(l!("taxes.recoverable_help"))
        .require(permissions::TAXES_EDIT),
    )
    .field(
        Field::toggle("is_active", l!("field.status"), |m: &TaxCodeInput| {
            FieldValue::Bool(m.is_active)
        })
        .writing(|m, value| m.is_active = value.as_bool())
        .require(permissions::TAXES_EDIT),
    )
    .action(
        FormAction::submit(l!("common.save"))
            .icon(Icon::Save)
            .then(Then::Say("Tax saved."))
            .require(permissions::TAXES_EDIT),
    )
}

/// A rate, and the window it is in force for.
///
/// The submit closure is built by the screen, because a rate belongs to a tax
/// code and the id is not on the draft. That is deliberate: a rate that carried
/// its own code id could be submitted against a different tax than the one the
/// screen is showing.
pub fn tax_rate_form(tax_code_id: Uuid, rate_id: Option<Uuid>) -> FormConfig<TaxRateInput> {
    FormConfig::new("tax-rate", move |draft: TaxRateInput| async move {
        crate::server_fns::master_fns::save_tax_rate(tax_code_id, rate_id, draft).await
    })
    .field(
        // Text rather than a number field: `TaxRate::parse_percent` refuses a
        // seventh significant digit rather than rounding it, and a browser
        // number input would have rounded it before this ever saw it.
        Field::text("percent", l!("field.rate"), |m: &TaxRateInput| {
            FieldValue::text(&m.percent)
        })
        .writing(|m, value| m.percent = value.as_input())
        .placeholder("20")
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::text("valid_from", l!("field.valid_from"), |m: &TaxRateInput| {
            FieldValue::text(m.valid_from.to_string())
        })
        .writing(|m, value| {
            if let Ok(date) = value.as_input().parse::<chrono::NaiveDate>() {
                m.valid_from = date;
            }
        })
        .placeholder("2026-04-01")
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::text("valid_to", l!("field.valid_to"), |m: &TaxRateInput| {
            FieldValue::text(m.valid_to.map(|d| d.to_string()).unwrap_or_default())
        })
        // Blank is open-ended, which is what most rates are: a government
        // announces when a rate starts and says nothing about when it stops.
        .writing(|m, value| {
            m.valid_to = value.as_input().trim().parse::<chrono::NaiveDate>().ok();
        })
        .help(l!("taxes.rate.open_ended_help"))
        .require(permissions::TAXES_EDIT),
    )
    .action(
        FormAction::submit(l!("common.save"))
            .icon(Icon::Save)
            .then(Then::Say("Rate saved."))
            .then(Then::Refresh)
            .then(Then::Close)
            .require(permissions::TAXES_EDIT),
    )
}

/// Which taxes apply together, and in what order.
///
/// `codes` is what the workspace has, fetched by the screen. Only active ones
/// are offered: a retired tax already in a group stays in it - removing it
/// would change what every document using the group comes to - but it is not
/// something to add.
pub fn tax_group_form(codes: Vec<TaxCode>) -> FormConfig<TaxGroupInput> {
    FormConfig::new("tax-group", |draft: TaxGroupInput| async move {
        save_tax_group(draft).await
    })
    .field(
        Field::text("code", l!("field.code"), |m: &TaxGroupInput| {
            FieldValue::text(&m.code)
        })
        .writing(|m, value| m.code = value.as_input())
        .placeholder("STD")
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::text("name", l!("field.name"), |m: &TaxGroupInput| {
            FieldValue::text(&m.name)
        })
        .writing(|m, value| m.name = value.as_input())
        .placeholder("Standard rate")
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::select(
            "country",
            l!("field.country"),
            country_choices(),
            |m: &TaxGroupInput| FieldValue::choice(m.country.map_or(NOT_SET, Country::code)),
        )
        .writing(|m, value| {
            m.country = value.as_choice().and_then(|code| Country::parse(code).ok());
        })
        .require(permissions::TAXES_EDIT),
    )
    .field(
        Field::multi_select(
            "members",
            l!("tax_groups.members"),
            member_choices(&codes),
            |m: &TaxGroupInput| FieldValue::choices(m.members.iter().map(Uuid::to_string)),
        )
        .writing(|m, value| {
            m.members = value
                .as_set()
                .iter()
                .filter_map(|raw| raw.parse::<Uuid>().ok())
                .collect();
        })
        .help(l!("tax_groups.members_help"))
        .require(permissions::TAXES_EDIT)
        .required(),
    )
    .field(
        Field::toggle("is_active", l!("field.status"), |m: &TaxGroupInput| {
            FieldValue::Bool(m.is_active)
        })
        .writing(|m, value| m.is_active = value.as_bool())
        .require(permissions::TAXES_EDIT),
    )
    .action(
        FormAction::submit(l!("common.save"))
            .icon(Icon::Save)
            .then(Then::Say("Tax group saved."))
            .require(permissions::TAXES_EDIT),
    )
}

fn country_choices() -> Vec<Choice> {
    std::iter::once(Choice::new(NOT_SET, l!("common.not_set")))
        .chain(
            Country::all_by_name()
                .into_iter()
                .map(|country| Choice::new(country.code(), country.name())),
        )
        .collect()
}

/// The taxes a group may be built from.
///
/// Active ones only. A retired tax already in a group stays in it, because
/// removing it would change what every document using that group comes to; it
/// is simply not offered as something to add.
fn member_choices(codes: &[TaxCode]) -> Vec<Choice> {
    codes
        .iter()
        .filter(|code| code.is_active)
        .map(|code| Choice::new(code.id.to_string(), &code.name).detail(&code.code))
        .collect()
}

#[cfg(test)]
mod tests {
    use leptos::prelude::Owner;

    use super::*;

    fn code_form() -> FormConfig<TaxCodeInput> {
        Owner::new().with(tax_code_form)
    }

    fn field_named<'a, T: 'static>(
        form: &'a FormConfig<T>,
        name: &str,
    ) -> &'a crate::ui::form::Field<T> {
        form.fields()
            .iter()
            .find(|field| field.name() == name)
            .unwrap_or_else(|| panic!("{name} is not a field on this form"))
    }

    fn tax(name: &str, is_active: bool) -> TaxCode {
        TaxCode {
            id: Uuid::new_v4(),
            code: name.to_owned(),
            name: name.to_owned(),
            kind: TaxKind::Vat,
            country: None,
            region_code: None,
            is_compound: false,
            is_recoverable: true,
            is_active,
        }
    }

    #[test]
    fn every_field_on_every_form_is_gated_on_the_one_permission() {
        // One gate over codes, rates and groups: they are one act, and a grant
        // that allowed two of the three would leave a code nothing can reach.
        for field in code_form().fields() {
            assert_eq!(field.permission(), Some(permissions::TAXES_EDIT));
        }
        for field in Owner::new().with(|| tax_group_form(Vec::new())).fields() {
            assert_eq!(field.permission(), Some(permissions::TAXES_EDIT));
        }
        for field in Owner::new()
            .with(|| tax_rate_form(Uuid::nil(), None))
            .fields()
        {
            assert_eq!(field.permission(), Some(permissions::TAXES_EDIT));
        }
    }

    #[test]
    fn the_rate_box_is_text_so_a_browser_cannot_round_it_first() {
        // `parse_percent` refuses a seventh significant digit rather than
        // rounding it. A number input would have rounded it on the way in, and
        // the refusal would never happen.
        let form = Owner::new().with(|| tax_rate_form(Uuid::nil(), None));
        let percent = field_named(&form, "percent");

        assert!(matches!(percent.kind, crate::ui::form::FieldKind::Text));
    }

    #[test]
    fn a_blank_end_date_is_open_ended() {
        // What most rates are: a government announces when a rate starts and
        // says nothing about when it stops.
        let form = Owner::new().with(|| tax_rate_form(Uuid::nil(), None));
        let valid_to = field_named(&form, "valid_to");

        let mut draft = TaxRateInput {
            percent: "20".to_owned(),
            valid_from: chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
            valid_to: chrono::NaiveDate::from_ymd_opt(2027, 4, 1),
        };
        valid_to.apply(&mut draft, &FieldValue::text("  "));

        assert_eq!(draft.valid_to, None);
    }

    #[test]
    fn an_unrecognised_kind_keeps_the_one_that_was_stored() {
        let form = code_form();
        let kind = field_named(&form, "kind");

        let mut draft = TaxCodeInput {
            kind: TaxKind::Withholding,
            ..TaxCodeInput::blank()
        };

        kind.apply(&mut draft, &FieldValue::choice("carbon"));
        assert_eq!(draft.kind, TaxKind::Withholding);

        kind.apply(&mut draft, &FieldValue::choice("gst"));
        assert_eq!(draft.kind, TaxKind::Gst);
    }

    #[test]
    fn a_retired_tax_is_not_offered_as_something_to_add_to_a_group() {
        let choices = member_choices(&[tax("LIVE", true), tax("OLD", false)]);

        assert_eq!(choices.len(), 1);
        assert_eq!(choices.first().map(|c| c.label.as_str()), Some("LIVE"));
    }

    #[test]
    fn a_group_demands_at_least_one_tax() {
        // A group with nothing in it is not zero-rated, it is nothing - a line
        // pointing at it would attract no tax and no explanation of why.
        let form = Owner::new().with(|| tax_group_form(Vec::new()));
        let members = field_named(&form, "members");

        assert!(members.is_required());
    }

    #[test]
    fn a_blank_region_is_written_back_as_none() {
        let form = code_form();
        let region = field_named(&form, "region_code");

        let mut draft = TaxCodeInput {
            region_code: Some("TX".to_owned()),
            ..TaxCodeInput::blank()
        };
        region.apply(&mut draft, &FieldValue::text("   "));

        assert_eq!(draft.region_code, None);
    }
}
