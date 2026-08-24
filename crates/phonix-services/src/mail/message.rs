//! What a message says.
//!
//! # Built here rather than templated
//!
//! There is no template engine and no `templates/` directory, deliberately.
//! Three messages, each a dozen lines, do not pay for a renderer - and a
//! template file is one more thing that can be missing at runtime, in a binary
//! that otherwise cannot be. When a fourth and fifth arrive with content an
//! administrator edits, that is the moment to reconsider, and the shape here -
//! one function per message returning a [`Mail`] - is what makes that a local
//! change.
//!
//! # Both parts, every time
//!
//! Every message is text *and* HTML. The text part is not a fallback nobody
//! reads: it is what a screen reader, a watch, a terminal client and a spam
//! filter all see, and a single-part HTML message is what scores worst at the
//! last of those.
//!
//! # A link is a credential
//!
//! An invitation link signs somebody in. It is stated once, with its expiry
//! next to it, and the message says plainly what to do if it was not expected -
//! which is the only defence against an invitation sent to the wrong address.

/// One message, ready for a relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to_address: String,
    pub to_name: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// "You have been invited to a workspace."
///
/// `link` is absolute and single-use. `expires_in_hours` is stated because an
/// invitation that quietly stops working is a support request, and because a
/// deadline is what makes somebody act on it.
pub fn invitation(
    to_address: &str,
    to_name: &str,
    workspace: &str,
    invited_by: &str,
    link: &str,
    expires_in_hours: i64,
) -> Mail {
    let greeting = if to_name.trim().is_empty() {
        "Hello"
    } else {
        to_name.trim()
    };
    let expiry = hours(expires_in_hours);

    let text = format!(
        "{greeting},\n\n\
         {invited_by} has invited you to join {workspace} on Phonix.\n\n\
         Set your password and sign in:\n{link}\n\n\
         This link works once and expires in {expiry}. If it has expired, ask \
         {invited_by} to send another.\n\n\
         If you were not expecting this, ignore this message - the invitation \
         cannot be used unless you open the link.\n"
    );

    let html = wrap(
        &format!("You have been invited to {}", escape(workspace)),
        &format!(
            "<p>{greeting},</p>\
             <p><strong>{invited_by}</strong> has invited you to join \
              <strong>{workspace}</strong> on Phonix.</p>\
             {button}\
             <p style=\"color:#64748b;font-size:13px\">This link works once and expires in \
              {expiry}. If it has expired, ask {invited_by} to send another.</p>\
             <p style=\"color:#64748b;font-size:13px\">If you were not expecting this, ignore \
              this message - the invitation cannot be used unless you open the link.</p>",
            greeting = escape(greeting),
            invited_by = escape(invited_by),
            workspace = escape(workspace),
            button = button(link, "Set your password"),
            expiry = escape(&expiry),
        ),
    );

    Mail {
        to_address: to_address.to_owned(),
        to_name: to_name.to_owned(),
        // Names the workspace: somebody who belongs to three of them should not
        // have to open the message to know which one this is.
        subject: format!("You have been invited to {workspace}"),
        text,
        html,
    }
}

/// "Here is your code." - the one message that carries a secret and no link.
///
/// # Why a code and not a link, when everything else here is a link
///
/// An invitation is opened on whatever device is reading the mail, and that is
/// fine: the invitation *is* the start of the session. A reset is different -
/// somebody is already sitting in front of a browser that is refusing to let
/// them in, and the mail is very often on a phone. A link moves the reset to
/// the phone, or gets copied out of a URL bar by hand. Six digits go the other
/// way, from the phone to the browser, which is the direction the person is
/// already working in.
///
/// # What this message must not do
///
/// **It never says whether an account exists.** It is only ever sent to an
/// address that has one - the caller's silence is what covers the other case -
/// so nothing here has to hedge. And it carries no link at all, so there is
/// nothing in it that a mail scanner following URLs can spend on the user's
/// behalf; a scanner that fetched a reset *link* would consume the token before
/// the person ever saw it.
///
/// The "if you did not ask for this" line is the whole security value of the
/// message for the person who did not: a reset they did not request is the
/// first sign somebody knows their address, and the code alone does nothing
/// until it is typed in.
pub fn password_reset_code(
    to_address: &str,
    to_name: &str,
    workspace: &str,
    code: &str,
    expires_in_mins: i64,
) -> Mail {
    let greeting = if to_name.trim().is_empty() {
        "Hello"
    } else {
        to_name.trim()
    };
    let expiry = minutes(expires_in_mins);

    let text = format!(
        "{greeting},

         Someone asked to reset the password for your {workspace} account.

         Your code is: {code}

         Enter it on the page you started from. It expires in {expiry} and          works once.

         If you did not ask for this, you can ignore this message - your          password has not changed, and the code is useless on its own.
"
    );

    let html = wrap(
        "Your password reset code",
        &format!(
            "<p>{greeting},</p>             <p>Someone asked to reset the password for your               <strong>{workspace}</strong> account.</p>             {digits}             <p style=\"color:#64748b;font-size:13px\">Enter it on the page you started from.               It expires in {expiry} and works once.</p>             <p style=\"color:#64748b;font-size:13px\">If you did not ask for this, you can               ignore this message - your password has not changed, and the code is useless on               its own.</p>",
            greeting = escape(greeting),
            workspace = escape(workspace),
            digits = digits(code),
            expiry = escape(&expiry),
        ),
    );

    Mail {
        to_address: to_address.to_owned(),
        to_name: to_name.to_owned(),
        // The code is deliberately NOT in the subject. Subject lines show on a
        // lock screen, and a code visible without unlocking the phone is a code
        // that did not need the mailbox at all.
        subject: format!("Reset your {workspace} password"),
        text,
        html,
    }
}

/// "Does this relay work?" - what the settings screen sends to prove it.
pub fn relay_test(to_address: &str, workspace: &str, host: &str) -> Mail {
    let text = format!(
        "This is a test message from {workspace} on Phonix.\n\n\
         It was sent through {host}. If you are reading it, that relay works \
         and invitations will be delivered.\n"
    );

    let html = wrap(
        "Your mail relay works",
        &format!(
            "<p>This is a test message from <strong>{workspace}</strong> on Phonix.</p>\
             <p>It was sent through <strong>{host}</strong>. If you are reading it, that relay \
              works and invitations will be delivered.</p>",
            workspace = escape(workspace),
            host = escape(host),
        ),
    );

    Mail {
        to_address: to_address.to_owned(),
        to_name: String::new(),
        subject: format!("Test message from {workspace}"),
        text,
        html,
    }
}

/// "10 minutes" - the short end of the same idea as [`hours`].
fn minutes(minutes: i64) -> String {
    match minutes {
        ..=0 => "less than a minute".to_owned(),
        1 => "1 minute".to_owned(),
        m if m < 120 => format!("{m} minutes"),
        m => hours(m / 60),
    }
}

/// The code, set out to be read off a screen and typed into another one.
///
/// Monospace and spaced, because the failure this is guarding against is a
/// person reading `0` as `O` or losing their place in six identical-width
/// digits. Not a button and not a link: there is nothing to click, and a
/// message that looks clickable is a message somebody waits to be able to act
/// on.
fn digits(code: &str) -> String {
    format!(
        "<p style=\"margin:24px 0;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,          monospace;font-size:30px;font-weight:700;letter-spacing:0.28em;color:#0f172a\">         {code}</p>",
        code = escape(code),
    )
}

/// "24 hours", "3 days" - a duration somebody can act on.
fn hours(hours: i64) -> String {
    match hours {
        ..=0 => "less than an hour".to_owned(),
        1 => "1 hour".to_owned(),
        h if h < 48 => format!("{h} hours"),
        h => {
            let days = h / 24;
            format!("{days} days")
        }
    }
}

/// One call-to-action, as a table.
///
/// A table and inline styles rather than a styled `<a>`: Outlook renders a
/// padded anchor as a bare link, and a button nobody can see is a message that
/// appears to have no way to act on it.
fn button(link: &str, label: &str) -> String {
    let href = escape(link);

    format!(
        "<table role=\"presentation\" cellspacing=\"0\" cellpadding=\"0\" style=\"margin:24px 0\">\
           <tr><td style=\"border-radius:6px;background:#4f46e5\">\
             <a href=\"{href}\" style=\"display:inline-block;padding:11px 20px;\
              font-family:system-ui,-apple-system,'Segoe UI',sans-serif;font-size:14px;\
              font-weight:600;color:#ffffff;text-decoration:none\">{label}</a>\
           </td></tr>\
         </table>\
         <p style=\"color:#64748b;font-size:13px\">If the button does not work, copy this into \
          your browser:<br><span style=\"word-break:break-all\">{href}</span></p>",
        label = escape(label),
    )
}

/// The shell every message shares.
///
/// No external stylesheet, no web font and no remote image: a mail client that
/// blocks all three - which is most of them, by default - has to render the
/// same message as one that does not.
fn wrap(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width\">\
         <title>{title}</title></head>\
         <body style=\"margin:0;padding:24px;background:#f8fafc\">\
           <div style=\"max-width:520px;margin:0 auto;padding:28px;background:#ffffff;\
            border:1px solid #e2e8f0;border-radius:10px;\
            font-family:system-ui,-apple-system,'Segoe UI',sans-serif;font-size:15px;\
            line-height:1.55;color:#0f172a\">{body}</div>\
         </body></html>",
        title = escape(title),
    )
}

/// The five characters that turn a name into markup.
///
/// Applied to every interpolated value without exception. A display name is
/// attacker-controlled - somebody can call themselves whatever they like - and
/// the one place it is guaranteed to be rendered as HTML is an email.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }

    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Mail {
        invitation(
            "ada@example.com",
            "Ada",
            "Acme",
            "Grace Hopper",
            "https://acme.phonix.local/invitations/abc123",
            48,
        )
    }

    #[test]
    fn an_invitation_carries_the_link_in_both_parts() {
        // The text part is not decoration: it is what a terminal client, a
        // watch and a screen reader show.
        let mail = invite();

        assert!(
            mail.text
                .contains("https://acme.phonix.local/invitations/abc123")
        );
        assert!(
            mail.html
                .contains("https://acme.phonix.local/invitations/abc123")
        );
    }

    #[test]
    fn the_subject_names_the_workspace() {
        // Somebody in three workspaces should not have to open it to know
        // which one this is.
        assert_eq!(invite().subject, "You have been invited to Acme");
    }

    #[test]
    fn an_invitation_states_its_expiry_and_what_to_do_about_it() {
        let mail = invite();

        assert!(mail.text.contains("2 days"));
        assert!(mail.text.contains("send another"));
    }

    #[test]
    fn an_invitation_tells_an_unexpecting_reader_to_ignore_it() {
        assert!(invite().text.contains("not expecting this"));
    }

    #[test]
    fn a_display_name_cannot_smuggle_markup_into_the_html() {
        // Somebody can call themselves whatever they like, and an email is the
        // one place their name is certain to be rendered as HTML.
        let mail = invitation(
            "ada@example.com",
            "Ada",
            "<script>alert(1)</script>",
            "Grace",
            "https://example.test/x",
            24,
        );

        assert!(!mail.html.contains("<script>"));
        assert!(mail.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn a_link_with_query_parameters_survives_escaping() {
        // `&` between parameters must become `&amp;` in HTML and stay `&` in
        // the text part, or the link breaks in one of the two.
        let mail = invitation(
            "ada@example.com",
            "Ada",
            "Acme",
            "Grace",
            "https://example.test/i?token=a&next=/dashboard",
            24,
        );

        assert!(mail.html.contains("token=a&amp;next=/dashboard"));
        assert!(mail.text.contains("token=a&next=/dashboard"));
    }

    #[test]
    fn a_nameless_recipient_is_greeted_rather_than_addressed_as_nothing() {
        let mail = invitation(
            "ada@example.com",
            "  ",
            "Acme",
            "Grace",
            "https://x.test",
            24,
        );

        assert!(mail.text.starts_with("Hello,"));
    }

    #[test]
    fn an_expiry_reads_as_something_somebody_can_act_on() {
        assert_eq!(hours(1), "1 hour");
        assert_eq!(hours(24), "24 hours");
        assert_eq!(hours(72), "3 days");
        // An already-expired invitation should not read as "0 hours".
        assert_eq!(hours(0), "less than an hour");
    }

    #[test]
    fn a_test_message_names_the_relay_it_proves() {
        let mail = relay_test("admin@example.com", "Acme", "smtp.acme.com");

        assert!(mail.text.contains("smtp.acme.com"));
        assert!(mail.html.contains("smtp.acme.com"));
    }

    #[test]
    fn every_message_is_a_complete_document() {
        // A fragment renders as source in some clients.
        for mail in [invite(), relay_test("a@b.test", "Acme", "smtp.acme.com")] {
            assert!(mail.html.starts_with("<!doctype html>"));
            assert!(mail.html.ends_with("</html>"));
        }
    }
}
