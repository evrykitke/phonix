//! What a workspace's own mail relay looks like to a screen.
//!
//! # Two types, because a password only travels one way
//!
//! [`MailSettings`] is what the settings screen *reads*. It has no password
//! field at all - only [`MailSettings::has_password`], a boolean. That is not
//! caution about serialisation, it is the shape of the type: a struct with
//! nowhere to put a password cannot leak one into a template, a log line or a
//! JSON payload, and no future edit to this file can accidentally add it to a
//! response without someone noticing what they are doing.
//!
//! [`MailSettingsInput`] is what the screen *submits*, and its password is an
//! `Option<String>` where `None` means "leave the stored one alone". A form
//! that had to re-send the password to save an unrelated change would have to
//! be given it first, which is the thing being avoided.
//!
//! # Why the encryption mode is repeated here
//!
//! `phonix_config::SmtpEncryption` says the same three words, but that crate is
//! server-only - it reads files and holds `SecretString`s - and this one
//! compiles to wasm. The conversion happens once, where the two meet, rather
//! than by making the browser depend on the configuration loader.

use serde::{Deserialize, Serialize};

use crate::identity::validation::FieldError;
use crate::{Message, msg};

/// How the connection to a relay is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailEncryption {
    /// Connect in clear, then upgrade with STARTTLS. What ports 587 and 2525
    /// expect, and the right default.
    #[default]
    StartTls,
    /// TLS from the first byte. What port 465 expects.
    Implicit,
    /// None at all. The password crosses the wire in clear, so this is for a
    /// relay on localhost and for nothing else.
    None,
}

impl MailEncryption {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartTls => "start_tls",
            Self::Implicit => "implicit",
            Self::None => "none",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "start_tls" => Some(Self::StartTls),
            "implicit" => Some(Self::Implicit),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// What a screen calls this mode.
    ///
    /// "STARTTLS" is a protocol's name and reads the same everywhere; "None" is
    /// an ordinary word and does not. Both go through the catalog, because a
    /// table with one translated entry and two literals is a table nobody can
    /// reason about.
    pub fn label(self) -> Message {
        match self {
            Self::StartTls => msg!("mail.encryption.starttls"),
            Self::Implicit => msg!("mail.encryption.implicit"),
            Self::None => msg!("mail.encryption.none"),
        }
    }

    /// The port this mode is usually found on, for the hint under the field.
    pub const fn usual_port(self) -> u16 {
        match self {
            Self::StartTls => 587,
            Self::Implicit => 465,
            Self::None => 25,
        }
    }
}

/// This workspace's relay, as a screen reads it.
///
/// Carries no password - see the module note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailSettings {
    /// Whether this workspace sends through its own relay. False means it falls
    /// back to the system default, whatever else is stored here.
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub encryption: MailEncryption,
    /// Whether a password is stored. Never the password.
    pub has_password: bool,
}

impl MailSettings {
    /// A workspace that has configured nothing.
    pub fn unset() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: MailEncryption::StartTls.usual_port(),
            username: String::new(),
            from_address: String::new(),
            from_name: String::new(),
            reply_to: None,
            encryption: MailEncryption::StartTls,
            has_password: false,
        }
    }

    /// Whether this override is the one that will actually be used.
    ///
    /// Enabled is not sufficient: a relay with no host or no sending address
    /// cannot send, and falling back to the system default is a better answer
    /// than failing on the first invitation.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.host.trim().is_empty() && self.from_address.contains('@')
    }
}

/// Which relay a workspace would actually send through.
///
/// Lives here rather than in the service that computes it because it is the
/// return type of a server function, and a server function's signature is
/// compiled for the browser as well. A type only the server can name makes the
/// wasm build fail at the signature, which is a confusing place to discover it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayInUse {
    /// This workspace's own.
    Workspace { host: String },
    /// The system default, because the workspace has not configured one.
    SystemDefault { host: String },
    /// Neither is configured. Nothing will be delivered.
    None,
}

impl RelayInUse {
    /// The sentence the settings screen shows above the form.
    pub fn describe(&self) -> String {
        match self {
            Self::Workspace { host } => {
                format!("This workspace sends through its own relay, {host}.")
            }
            Self::SystemDefault { host } => format!(
                "This workspace uses the system relay, {host}. \
                 Turn the override on below to send through your own."
            ),
            Self::None => "No relay is configured, so no email is being sent. \
                 Invitation links have to be shared by hand."
                .to_owned(),
        }
    }
}

/// This workspace's relay, as a screen submits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailSettingsInput {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `None` leaves the stored password untouched. `Some("")` clears it - some
    /// relays authenticate on the username alone, so "no password" has to be
    /// expressible.
    pub password: Option<String>,
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub encryption: MailEncryption,
}

impl MailSettingsInput {
    /// Check what can be checked without connecting to anything.
    ///
    /// Every problem rather than the first, so somebody fixing the form is not
    /// sent round the loop once per field. Only checked when the override is
    /// *on*: a workspace turning it off is not asked to fix the host it is
    /// about to stop using.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();

        if !self.enabled {
            return errors;
        }

        if self.host.trim().is_empty() {
            errors.push(FieldError::new(
                "host",
                msg!("validation.mail.host_required"),
            ));
        }
        if self.port == 0 {
            errors.push(FieldError::new(
                "port",
                msg!("validation.mail.port_required"),
            ));
        }
        // Only the shape. Whether the relay will accept this sender is the
        // relay's answer, and guessing at it here would refuse valid addresses.
        if !is_addressish(&self.from_address) {
            errors.push(FieldError::new(
                "from_address",
                msg!("validation.email.not_an_address"),
            ));
        }
        if self.from_name.trim().is_empty() {
            errors.push(FieldError::new(
                "from_name",
                msg!("validation.mail.from_name_required"),
            ));
        }
        if let Some(reply_to) = &self.reply_to
            && !reply_to.trim().is_empty()
            && !is_addressish(reply_to)
        {
            errors.push(FieldError::new(
                "reply_to",
                msg!("validation.email.not_an_address"),
            ));
        }

        errors
    }

    /// The same input with its text trimmed and its blanks normalised.
    ///
    /// A `reply_to` of `"  "` becomes `None` rather than an empty string that
    /// would be written into a header.
    #[must_use]
    pub fn normalised(mut self) -> Self {
        self.host = self.host.trim().to_owned();
        self.username = self.username.trim().to_owned();
        self.from_address = self.from_address.trim().to_owned();
        self.from_name = self.from_name.trim().to_owned();
        self.reply_to = self
            .reply_to
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        self
    }
}

/// The weakest check that still rejects what is obviously not an address.
///
/// Deliberately not a grammar for RFC 5322. Every stricter version of this
/// function that has ever been written rejects somebody's real address, and the
/// relay is the authority regardless.
fn is_addressish(value: &str) -> bool {
    let value = value.trim();

    match value.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> MailSettingsInput {
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
    fn each_relay_describes_itself_in_terms_of_what_happens_next() {
        let own = RelayInUse::Workspace {
            host: "smtp.acme.com".into(),
        };
        assert!(own.describe().contains("its own relay"));
        assert!(own.describe().contains("smtp.acme.com"));

        let default = RelayInUse::SystemDefault {
            host: "sandbox.smtp.mailtrap.io".into(),
        };
        assert!(default.describe().contains("system relay"));

        // The one that has to be unmistakable: nothing is being delivered.
        assert!(RelayInUse::None.describe().contains("No relay"));
        assert!(RelayInUse::None.describe().contains("shared by hand"));
    }

    #[test]
    fn no_description_has_a_run_of_spaces_in_it() {
        // A wrapped string literal whose trailing backslash went missing reads
        // as a sentence with a gap in the middle, and every `contains` test
        // still passes over it.
        for relay in [
            RelayInUse::Workspace {
                host: "smtp.acme.com".into(),
            },
            RelayInUse::SystemDefault {
                host: "smtp.system.test".into(),
            },
            RelayInUse::None,
        ] {
            let described = relay.describe();

            assert!(!described.contains("  "), "run of spaces in: {described}");
        }
    }

    #[test]
    fn a_complete_override_has_nothing_to_say() {
        assert!(input().validate().is_empty());
    }

    #[test]
    fn a_disabled_override_is_not_asked_to_be_valid() {
        // Turning it off must not require fixing the host you are turning off.
        let disabled = MailSettingsInput {
            enabled: false,
            host: String::new(),
            ..input()
        };

        assert!(disabled.validate().is_empty());
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let broken = MailSettingsInput {
            host: String::new(),
            from_address: "not-an-address".to_owned(),
            from_name: "  ".to_owned(),
            ..input()
        };

        let fields: Vec<&str> = broken
            .validate()
            .iter()
            .map(|e| e.field.clone())
            .map(|f| match f.as_str() {
                "host" => "host",
                "from_address" => "from_address",
                "from_name" => "from_name",
                _ => "other",
            })
            .collect();

        assert!(fields.contains(&"host"));
        assert!(fields.contains(&"from_address"));
        assert!(fields.contains(&"from_name"));
    }

    #[test]
    fn a_blank_reply_to_is_no_reply_to_rather_than_an_empty_header() {
        let normalised = MailSettingsInput {
            reply_to: Some("   ".to_owned()),
            ..input()
        }
        .normalised();

        assert_eq!(normalised.reply_to, None);
    }

    #[test]
    fn an_enabled_override_missing_a_host_falls_back_rather_than_failing() {
        // The alternative is an invitation that fails to send at the moment
        // somebody is added, which is the worst time to discover it.
        let settings = MailSettings {
            enabled: true,
            ..MailSettings::unset()
        };

        assert!(!settings.is_active());
    }

    #[test]
    fn a_complete_override_is_the_one_that_gets_used() {
        let settings = MailSettings {
            enabled: true,
            host: "smtp.example.com".to_owned(),
            from_address: "no-reply@example.com".to_owned(),
            ..MailSettings::unset()
        };

        assert!(settings.is_active());
    }

    #[test]
    fn the_stored_view_has_nowhere_to_put_a_password() {
        // The guarantee is the type, not the serialiser - but the serialised
        // form is what would leak, so it is what is asserted.
        let settings = MailSettings {
            has_password: true,
            ..MailSettings::unset()
        };
        let json = serde_json::to_string(&settings).unwrap();

        assert!(json.contains("has_password"));
        assert!(!json.contains("\"password\""));
    }

    #[test]
    fn an_encryption_mode_survives_the_round_trip_through_storage() {
        for mode in [
            MailEncryption::StartTls,
            MailEncryption::Implicit,
            MailEncryption::None,
        ] {
            assert_eq!(MailEncryption::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn an_unrecognised_encryption_mode_is_refused_rather_than_defaulted() {
        // A silent fallback to StartTls would turn a typo into a downgrade
        // nobody was told about.
        assert_eq!(MailEncryption::parse("tls"), None);
    }
}
