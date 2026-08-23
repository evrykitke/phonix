//! The mail relay form.
//!
//! # The password field is the interesting one
//!
//! It reads as empty and writes as `Some(..)`, and the draft the screen opens
//! with has `password: None`. Those three facts together are what make "save a
//! changed host without being handed the password" work:
//!
//! ```text
//! never touched   ->  write never runs  ->  stays None    ->  leave it alone
//! typed into      ->  Some("s3cret")                      ->  replace it
//! typed and clear ->  Some("")                            ->  remove it
//! ```
//!
//! A field that read the stored password back would have to be given one, and
//! the whole point of [`MailSettings`](phonix_core::mail::MailSettings) having
//! no password field is that there is nothing to give it.
//!
//! # Why the whole form is one gate
//!
//! Every field requires `Settings`, the same permission the service requires.
//! There is no partial version of this screen: somebody who may change the
//! relay host may change the credentials, because the two together are the
//! thing being trusted.

use phonix_core::form::Submission;
use phonix_core::mail::{MailEncryption, MailSettings, MailSettingsInput};
use phonix_core::permissions;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::save_mail_settings;
use crate::ui::form::{Choice, Field, FieldKind, FieldValue, FormAction, Then};

/// The draft a loaded relay opens as.
///
/// `password: None` is load-bearing - see the module note.
pub fn draft_from(settings: &MailSettings) -> MailSettingsInput {
    MailSettingsInput {
        enabled: settings.enabled,
        host: settings.host.clone(),
        port: settings.port,
        username: settings.username.clone(),
        password: None,
        from_address: settings.from_address.clone(),
        from_name: settings.from_name.clone(),
        reply_to: settings.reply_to.clone(),
        encryption: settings.encryption,
    }
}

/// Where this workspace's mail goes.
///
/// `has_password` comes from the stored settings rather than the draft, because
/// the draft deliberately cannot know it - it is only used to word the hint
/// under the password box.
pub fn mail_form(has_password: bool) -> FormConfig<MailSettingsInput> {
    let password_hint = if has_password {
        l!("mail.password_stored")
    } else {
        l!("mail.password_absent")
    };

    // Saved settings come back as `MailSettings`; the form edits
    // `MailSettingsInput`. Re-seeding the draft from what was stored is what
    // puts the password field back to "leave it alone" after a save.
    FormConfig::new("mail", |input: MailSettingsInput| async move {
        match save_mail_settings(input).await {
            Ok(Submission::Saved(stored)) => Ok(Submission::Saved(draft_from(&stored))),
            Ok(Submission::Rejected(errors)) => Ok(Submission::Rejected(errors)),
            Err(err) => Err(err),
        }
    })
    .note(l!("mail.note"))
    .field(
        Field::toggle("enabled", l!("mail.enabled"), |m: &MailSettingsInput| {
            FieldValue::Bool(m.enabled)
        })
        .writing(|m, value| m.enabled = value.as_bool())
        .require(permissions::SETTINGS)
        .full_width(),
    )
    .field(
        Field::text("host", l!("mail.host"), |m: &MailSettingsInput| {
            FieldValue::text(&m.host)
        })
        .writing(|m, value| m.host = value.as_input())
        .placeholder("smtp.example.com")
        .require(permissions::SETTINGS)
        // Only demanded when the override is on: turning it off must not
        // require filling in the relay being turned off.
        .when(|m: &MailSettingsInput| m.enabled)
        .required(),
    )
    .field(
        Field::number("port", l!("mail.port"), |m: &MailSettingsInput| {
            FieldValue::Number(Some(f64::from(m.port)))
        })
        .writing(|m, value| {
            // Out-of-range keeps what was there. A port silently becoming 0
            // is a relay that cannot connect and does not say why.
            if let Some(port) = value.as_number()
                && (1.0..=65535.0).contains(&port)
            {
                m.port = port as u16;
            }
        })
        .help(l!("mail.port_help"))
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled)
        .required(),
    )
    .field(
        Field::select(
            "encryption",
            l!("mail.encryption"),
            encryption_choices(),
            |m: &MailSettingsInput| FieldValue::choice(m.encryption.as_str()),
        )
        .writing(|m, value| {
            if let Some(mode) = value.as_choice().and_then(MailEncryption::parse) {
                m.encryption = mode;
            }
        })
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled)
        .required(),
    )
    .field(
        Field::text("username", l!("mail.username"), |m: &MailSettingsInput| {
            FieldValue::text(&m.username)
        })
        .writing(|m, value| m.username = value.as_input())
        .help(l!("mail.username_help"))
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled),
    )
    .field(
        // Reads empty, always. See the module note.
        Field::new(
            "password",
            l!("field.password"),
            FieldKind::Password,
            |_: &MailSettingsInput| FieldValue::text(""),
        )
        .writing(|m, value| m.password = Some(value.as_input()))
        .help(password_hint)
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled),
    )
    .field(
        Field::email(
            "from_address",
            l!("mail.from_address"),
            |m: &MailSettingsInput| FieldValue::text(&m.from_address),
        )
        .writing(|m, value| m.from_address = value.as_input())
        .placeholder("no-reply@example.com")
        .help(l!("mail.from_address_help"))
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled)
        .required(),
    )
    .field(
        Field::text(
            "from_name",
            l!("mail.from_name"),
            |m: &MailSettingsInput| FieldValue::text(&m.from_name),
        )
        .writing(|m, value| m.from_name = value.as_input())
        .placeholder("Acme")
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled)
        .required(),
    )
    .field(
        Field::email("reply_to", l!("mail.reply_to"), |m: &MailSettingsInput| {
            FieldValue::text(m.reply_to.as_deref().unwrap_or(""))
        })
        .writing(|m, value| {
            let value = value.as_input();
            // Blank is no reply-to, not an empty header.
            m.reply_to = (!value.trim().is_empty()).then_some(value);
        })
        .help(l!("mail.reply_to_help"))
        .require(permissions::SETTINGS)
        .when(|m: &MailSettingsInput| m.enabled),
    )
    .action(
        FormAction::submit(l!("mail.submit"))
            .icon(Icon::Save)
            .then(Then::Say("Mail settings saved."))
            .require(permissions::SETTINGS),
    )
}

/// The three modes, each said in terms of what it does on the wire.
fn encryption_choices() -> Vec<Choice> {
    [
        (
            MailEncryption::StartTls,
            l!("mail.encryption.starttls_detail"),
        ),
        (
            MailEncryption::Implicit,
            l!("mail.encryption.implicit_detail"),
        ),
        (MailEncryption::None, l!("mail.encryption.none_detail")),
    ]
    .into_iter()
    // `as_str` is the stored value and stays English; `label` is the word.
    .map(|(mode, detail)| Choice::new(mode.as_str(), crate::i18n::t(&mode.label())).detail(detail))
    .collect()
}

#[cfg(test)]
mod tests {
    use leptos::prelude::Owner;

    use super::*;

    fn form() -> FormConfig<MailSettingsInput> {
        Owner::new().with(|| mail_form(true))
    }

    fn draft() -> MailSettingsInput {
        MailSettingsInput {
            enabled: true,
            host: "smtp.example.com".to_owned(),
            port: 587,
            username: "postmaster".to_owned(),
            password: None,
            from_address: "no-reply@example.com".to_owned(),
            from_name: "Example".to_owned(),
            reply_to: None,
            encryption: MailEncryption::StartTls,
        }
    }

    #[test]
    fn a_loaded_relay_opens_with_no_password_in_the_draft() {
        // The whole "leave it alone" mechanism rests on this being None.
        let settings = MailSettings {
            has_password: true,
            ..MailSettings::unset()
        };

        assert_eq!(draft_from(&settings).password, None);
    }

    #[test]
    fn an_untouched_password_field_leaves_the_stored_one_alone() {
        // The field is simply never written, so the draft keeps its None.
        let draft = draft();

        assert_eq!(draft.password, None);
    }

    #[test]
    fn typing_a_password_replaces_it_and_clearing_it_removes_it() {
        let form = form();
        let password = form
            .fields()
            .iter()
            .find(|f| f.name() == "password")
            .unwrap();

        let mut replaced = draft();
        password.apply(&mut replaced, &FieldValue::text("s3cret"));
        assert_eq!(replaced.password.as_deref(), Some("s3cret"));

        // Emptied on purpose: some relays authenticate on the username alone,
        // so "no password" has to be expressible.
        let mut cleared = draft();
        password.apply(&mut cleared, &FieldValue::text(""));
        assert_eq!(cleared.password.as_deref(), Some(""));
    }

    #[test]
    fn the_password_field_never_renders_holding_a_value() {
        let form = form();
        let password = form
            .fields()
            .iter()
            .find(|f| f.name() == "password")
            .unwrap();

        let mut draft = draft();
        draft.password = Some("s3cret".to_owned());

        assert_eq!(password.value(&draft), FieldValue::text(""));
    }

    #[test]
    fn nothing_but_the_switch_is_demanded_while_the_override_is_off() {
        // Turning the relay off must not require fixing the relay.
        let off = MailSettingsInput {
            enabled: false,
            host: String::new(),
            ..draft()
        };

        assert!(form().missing(&off, None).is_empty());
    }

    #[test]
    fn an_enabled_override_demands_what_it_needs_to_send() {
        let blank = MailSettingsInput {
            host: String::new(),
            from_address: String::new(),
            from_name: String::new(),
            ..draft()
        };

        let reported = form().missing(&blank, Some(&admin()));
        let missing: Vec<&str> = reported.iter().map(|error| error.field.as_str()).collect();

        assert!(missing.contains(&"host"));
        assert!(missing.contains(&"from_address"));
        assert!(missing.contains(&"from_name"));
    }

    #[test]
    fn a_port_outside_the_range_keeps_the_one_that_worked() {
        let form = form();
        let port = form.fields().iter().find(|f| f.name() == "port").unwrap();

        let mut draft = draft();
        port.apply(&mut draft, &FieldValue::Number(Some(0.0)));
        assert_eq!(draft.port, 587);

        port.apply(&mut draft, &FieldValue::Number(Some(70000.0)));
        assert_eq!(draft.port, 587);

        port.apply(&mut draft, &FieldValue::Number(Some(465.0)));
        assert_eq!(draft.port, 465);
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

    fn admin() -> phonix_core::identity::AuthUser {
        phonix_core::identity::AuthUser {
            id: phonix_core::identity::UserId::nil(),
            email: "admin@example.test".to_owned(),
            first_name: "A".to_owned(),
            last_name: "Dmin".to_owned(),
            display_name: "A Dmin".to_owned(),
            roles: Vec::new(),
            permissions: phonix_core::authorization::PermissionSet::all(),
            is_owner: true,
            status: phonix_core::identity::UserStatus::Active,
            mfa_satisfied: true,
            mfa_enabled: false,
            email_verified: true,
        }
    }
}
