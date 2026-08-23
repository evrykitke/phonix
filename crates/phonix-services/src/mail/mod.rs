//! Sending mail, and deciding which relay sends it.
//!
//! | Module       | What it decides                                       |
//! | ------------ | ----------------------------------------------------- |
//! | [`settings`] | this workspace's own relay: reading and writing it     |
//! | [`message`]  | what an individual message says                       |
//! | this module  | which relay is used, and the sending itself           |
//!
//! # Two levels, resolved in one place
//!
//! A workspace sends through its own relay if it has configured a usable one,
//! and through the system default otherwise. [`resolve`] is the only function
//! that makes that choice - no call site decides, and no call site can be
//! written that forgets the fallback and sends nothing.
//!
//! # Not sending is not an error
//!
//! [`resolve`] returns `Option`, and `None` - no relay configured anywhere - is
//! an ordinary answer. It has to be: a developer's machine with no credentials
//! must still be able to add a user, and an invitation whose link exists but
//! was not delivered is recoverable (copy the link, or re-send once mail
//! works) in a way that a failed *creation* is not.
//!
//! So the caller decides what an undelivered message means. [`Dispatch`] is
//! what it gets back, and the invitation flow reports it to the screen rather
//! than failing the request that created the account.
//!
//! # What is deliberately not here
//!
//! No queue and no retry. A relay that is briefly down loses the message, and
//! the recovery is to re-send the invitation - which is a button somebody
//! presses, not a background job. Putting mail on RabbitMQ is a reasonable
//! later step and a bad first one: it turns "did this send" into a question
//! with no answer at the moment somebody is watching for it.

pub mod message;
pub mod settings;

use std::time::Duration;

use lettre::message::{Mailbox, MessageBuilder, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use phonix_config::{SmtpConfig, SmtpEncryption};
use phonix_core::mail::MailEncryption;
use phonix_db::sqlx::PgPool;
use secrecy::{ExposeSecret, SecretString};

use crate::crypto::vault::SecretVault;
use crate::error::ServiceResult;

pub use message::Mail;

/// What happened to a message.
///
/// Not a `Result`, for the reason in the module note: an undelivered
/// invitation is an outcome the screen reports, not a failure that undoes the
/// account it belongs to.
///
/// Named `Dispatch` rather than the obvious `Delivery` because
/// [`identity::Delivery`](crate::identity::Delivery) already means something
/// else - how a *session* is handed over - and is re-exported at the crate
/// root. Two types with one name, both about handing something to somebody,
/// is a confusion that would be paid for at every call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// Handed to the relay, which accepted it.
    Sent,
    /// No relay is configured, here or in the system default. The message was
    /// never attempted.
    NotConfigured,
    /// A relay was configured and refused, or could not be reached. Carries the
    /// relay's own words, which say more than a house phrase.
    Failed(String),
}

impl Dispatch {
    pub const fn is_sent(&self) -> bool {
        matches!(self, Self::Sent)
    }

    /// A sentence for the screen, when there is something to say.
    ///
    /// `None` for a successful send: a screen that says "and the email was
    /// sent" after every action is one nobody reads.
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Sent => None,
            Self::NotConfigured => Some(
                "No mail relay is configured, so no email was sent. \
                 Share the link below instead."
                    .to_owned(),
            ),
            Self::Failed(reason) => Some(format!(
                "The email could not be sent: {reason}. Share the link below instead."
            )),
        }
    }
}

/// A relay, resolved and ready to send.
///
/// Holds the password as a [`SecretString`], so it is redacted in `Debug` and
/// cannot reach a log line by way of a derived formatter.
#[derive(Clone)]
pub struct Relay {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretString,
    pub from: Mailbox,
    pub reply_to: Option<Mailbox>,
    pub encryption: MailEncryption,
    pub timeout: Duration,
    /// Whether this came from the workspace's own settings or the system
    /// default. For the settings screen, which says which one is in force.
    pub tenant_override: bool,
}

impl std::fmt::Debug for Relay {
    /// Names the relay, never the credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("tenant_override", &self.tenant_override)
            .finish_non_exhaustive()
    }
}

/// Which relay this workspace sends through.
///
/// The workspace's own if it has configured a usable one, the system default
/// otherwise, and `None` if neither can send. **This is the only place that
/// decision is made.**
///
/// A tenant override whose password cannot be opened - a key rotated out from
/// under it - is treated as no override rather than as a failure: falling back
/// to the system default delivers the message, and refusing to send anything
/// because a workspace once configured a relay is the worse of the two.
pub async fn resolve(
    pool: &PgPool,
    config: &SmtpConfig,
    vault: Option<&SecretVault>,
) -> ServiceResult<Option<Relay>> {
    if let Some(vault) = vault
        && let Some(relay) = settings::as_relay(pool, vault, config.timeout_secs).await?
    {
        return Ok(Some(relay));
    }

    Ok(system_relay(config))
}

/// The system default as a relay, if it can send at all.
fn system_relay(config: &SmtpConfig) -> Option<Relay> {
    if !config.is_usable() {
        return None;
    }

    let from = mailbox(&config.from_name, &config.from_address)?;

    Some(Relay {
        host: config.host.clone(),
        port: config.port,
        username: config.username.clone(),
        password: config.password.clone(),
        from,
        reply_to: config
            .reply_to
            .as_deref()
            .and_then(|address| mailbox("", address)),
        encryption: match config.encryption {
            SmtpEncryption::StartTls => MailEncryption::StartTls,
            SmtpEncryption::Implicit => MailEncryption::Implicit,
            SmtpEncryption::None => MailEncryption::None,
        },
        timeout: Duration::from_secs(config.timeout_secs),
        tenant_override: false,
    })
}

/// A display name and an address as one mailbox.
///
/// `None` when the address will not parse. That is not an error worth failing a
/// request over - it means this relay cannot send, which is what the caller
/// already has a branch for.
pub(crate) fn mailbox(name: &str, address: &str) -> Option<Mailbox> {
    let address = address.trim().parse().ok()?;
    let name = name.trim();

    Some(if name.is_empty() {
        Mailbox::new(None, address)
    } else {
        Mailbox::new(Some(name.to_owned()), address)
    })
}

/// Send one message through `relay`.
///
/// Returns [`Dispatch`] rather than `Result` so a relay that refuses does not
/// unwind whatever the message was about. The failure is logged here, with the
/// relay named and the credential not.
pub async fn send(relay: &Relay, mail: Mail) -> Dispatch {
    let to = match mailbox(&mail.to_name, &mail.to_address) {
        Some(to) => to,
        None => {
            return Dispatch::Failed(format!("'{}' is not an address", mail.to_address));
        }
    };

    let mut builder: MessageBuilder = lettre::Message::builder()
        .from(relay.from.clone())
        .to(to)
        .subject(mail.subject);

    if let Some(reply_to) = &relay.reply_to {
        builder = builder.reply_to(reply_to.clone());
    }

    // Both parts, always. A text-only client shows the text; everything else
    // shows the HTML - and a single-part HTML message is what spam filters
    // score hardest.
    let message = match builder.multipart(MultiPart::alternative_plain_html(mail.text, mail.html)) {
        Ok(message) => message,
        Err(err) => return Dispatch::Failed(err.to_string()),
    };

    let transport = match transport(relay) {
        Ok(transport) => transport,
        Err(err) => return Dispatch::Failed(err),
    };

    match transport.send(message).await {
        Ok(_) => {
            tracing::info!(host = %relay.host, tenant_override = relay.tenant_override, "mail sent");
            Dispatch::Sent
        }
        Err(err) => {
            // The relay's own words. Recorded here because the screen shows a
            // shortened form and this is where the whole thing is useful.
            tracing::warn!(host = %relay.host, error = %err, "mail could not be sent");
            Dispatch::Failed(err.to_string())
        }
    }
}

/// Build the transport for one relay.
fn transport(relay: &Relay) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let mut builder = match relay.encryption {
        // `builder_dangerous` is the un-TLS'd constructor. Named that way by
        // lettre on purpose, and correct here only because the mode was asked
        // for explicitly - it is the one that puts the password on the wire in
        // clear, and the settings screen says so.
        MailEncryption::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&relay.host).tls(Tls::None)
        }
        mode => {
            let parameters =
                TlsParameters::new(relay.host.clone()).map_err(|err| err.to_string())?;

            let tls = if mode == MailEncryption::Implicit {
                Tls::Wrapper(parameters)
            } else {
                // Required, not Opportunistic: opportunistic silently continues
                // in clear when a relay does not offer STARTTLS, which is
                // exactly the case where the password must not be sent.
                Tls::Required(parameters)
            };

            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&relay.host).tls(tls)
        }
    };

    builder = builder.port(relay.port).timeout(Some(relay.timeout));

    // Some relays - and a local one in particular - accept mail unauthenticated.
    // Sending empty credentials would be refused where no credentials are fine.
    if !relay.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            relay.username.clone(),
            relay.password.expose_secret().to_owned(),
        ));
    }

    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SmtpConfig {
        SmtpConfig {
            enabled: true,
            host: "smtp.example.com".to_owned(),
            port: 587,
            username: "postmaster".to_owned(),
            password: SecretString::from("hunter2"),
            from_address: "no-reply@example.com".to_owned(),
            from_name: "Example".to_owned(),
            reply_to: None,
            encryption: SmtpEncryption::StartTls,
            timeout_secs: 15,
        }
    }

    #[test]
    fn a_disabled_system_relay_is_no_relay() {
        let disabled = SmtpConfig {
            enabled: false,
            ..config()
        };

        assert!(system_relay(&disabled).is_none());
    }

    #[test]
    fn an_enabled_relay_with_no_host_is_no_relay_either() {
        // Enabled is not the same as usable, and the difference is a message
        // that fails hours later rather than at boot.
        let hostless = SmtpConfig {
            host: String::new(),
            ..config()
        };

        assert!(system_relay(&hostless).is_none());
    }

    #[test]
    fn the_system_relay_carries_the_configured_sender() {
        let relay = system_relay(&config()).unwrap();

        assert_eq!(relay.from.to_string(), "Example <no-reply@example.com>");
        assert!(!relay.tenant_override);
    }

    #[test]
    fn a_relay_never_prints_its_password() {
        // `Debug` is what ends up in a log line by accident, so it is what is
        // asserted rather than the field's type.
        let printed = format!("{:?}", system_relay(&config()).unwrap());

        assert!(
            !printed.contains("hunter2"),
            "the password reached Debug: {printed}"
        );
        assert!(printed.contains("smtp.example.com"));
    }

    #[test]
    fn a_sender_with_no_display_name_is_still_a_mailbox() {
        let plain = mailbox("", "no-reply@example.com").unwrap();

        assert_eq!(plain.to_string(), "no-reply@example.com");
    }

    #[test]
    fn something_that_is_not_an_address_is_not_a_mailbox() {
        assert!(mailbox("Example", "not an address").is_none());
    }

    #[test]
    fn a_successful_send_has_nothing_to_say_and_the_others_do() {
        assert!(Dispatch::Sent.note().is_none());
        assert!(Dispatch::NotConfigured.note().unwrap().contains("link"));
        assert!(
            Dispatch::Failed("relay said no".into())
                .note()
                .unwrap()
                .contains("relay said no")
        );
    }
}
