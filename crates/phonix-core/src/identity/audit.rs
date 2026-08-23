//! The security trail, as a screen reads it.
//!
//! `phonix_db::identity::audit` writes and stores these; this is the shape the
//! browser is allowed to see. The two differ in one deliberate way: the stored
//! row carries a free-form `detail` object, and this carries a rendered summary
//! of it. That object holds whatever the use case that wrote it thought useful
//! (previous policy values, permission names, a workspace slug), and shipping
//! it verbatim would be shipping an unversioned internal structure to a client
//! that would then depend on it.
//!
//! # A list row and a detail page want different things
//!
//! [`AuditEvent`] is one line of the trail: enough to scan a page of them and
//! find the one worth opening. [`AuditEventDetail`] is that one, opened - and
//! it answers a different question depending on what happened:
//!
//! * **Something changed.** A policy was relaxed, a role's grants were edited.
//!   The useful answer is a *diff*: which fields, from what, to what. That is
//!   [`FieldChange`], and the trail carries them only for the events that
//!   recorded a before and an after.
//! * **Something happened.** A sign-in, a lockout, a recovery code spent.
//!   There is nothing to diff, so the useful answer is a sentence -
//!   [`AuditEvent::narration`].
//!
//! Both are computed from the stored row rather than stored: the shape of a
//! diff is presentation, and freezing one into the table would mean migrating
//! history every time the wording changed.
//!
//! # The CRUD events here are history, not a vocabulary
//!
//! `role_created`, `user_updated`, `organization_profile_changed` and the rest
//! of that family are no longer written. Record changes go to the change trail,
//! [`crate::audit::entity`], which names the record they are about and costs a
//! `const` rather than a migration to extend.
//!
//! They are still *rendered*, because the rows written before the split are
//! still in `identity_events` and a trail whose past is rewritten by a
//! deployment is not a trail. Nothing new should be added to that family: a new
//! event here is a new CHECK constraint in every tenant database, which is the
//! cost the change trail exists to remove.

use serde::{Deserialize, Serialize};

use super::user::UserId;
use crate::i18n::{Catalog, datetime};
use crate::{Message, msg};

// The diff vocabulary lives in `crate::audit::change`: the change trail renders
// the same three things, and one differ is the point. Re-exported here because
// this module's own types are described in terms of it.
pub use crate::audit::change::{Change, ChangeKind, Fact, FieldChange};

/// One entry in the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: i64,
    /// The stored event name, e.g. `mfa_enrolled`. Stable; the label is not.
    pub event: String,
    pub succeeded: bool,
    pub user_id: Option<UserId>,
    /// Who it happened to, as recorded at the time. Kept even when the account
    /// is later deleted, which is the point of an audit trail.
    pub email: Option<String>,
    pub ip: Option<String>,
    /// Why it failed, or what changed - a short line, already rendered.
    pub summary: Option<String>,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

impl AuditEvent {
    /// The event name as a sentence.
    ///
    /// A [`Message`] rather than a `String`: this is the heading of every row
    /// in the security trail, and a row that reads in English on a French
    /// screen is the one place an audit is least forgiving about.
    ///
    /// The stored name is the key's suffix but not the key itself - `msg!`
    /// needs a literal to check at compile time, so the arms are written out.
    /// An event added by a later release falls through to the stored name with
    /// its underscores opened out, deliberately untranslated: it is a column
    /// value, and inventing words for it would hide that this build has never
    /// heard of the event.
    pub fn label(&self) -> Message {
        match self.event.as_str() {
            "signup" => msg!("audit.event.signup"),
            "login" => msg!("audit.event.login"),
            "logout" => msg!("audit.event.logout"),
            "password_change" => msg!("audit.event.password_change"),
            "password_reset_requested" => msg!("audit.event.password_reset_requested"),
            "password_reset_completed" => msg!("audit.event.password_reset_completed"),
            "email_verification_sent" => msg!("audit.event.email_verification_sent"),
            "email_verified" => msg!("audit.event.email_verified"),
            "mfa_enrolled" => msg!("audit.event.mfa_enrolled"),
            "mfa_challenge" => msg!("audit.event.mfa_challenge"),
            "mfa_removed" => msg!("audit.event.mfa_removed"),
            "mfa_recovery_used" => msg!("audit.event.mfa_recovery_used"),
            "mfa_recovery_generated" => msg!("audit.event.mfa_recovery_generated"),
            "account_locked" => msg!("audit.event.account_locked"),
            "account_unlocked" => msg!("audit.event.account_unlocked"),
            "role_changed" => msg!("audit.event.role_changed"),
            "session_revoked" => msg!("audit.event.session_revoked"),
            "invitation_sent" => msg!("audit.event.invitation_sent"),
            "invitation_accepted" => msg!("audit.event.invitation_accepted"),
            "password_policy_changed" => msg!("audit.event.password_policy_changed"),
            "mfa_policy_changed" => msg!("audit.event.mfa_policy_changed"),
            "user_permissions_changed" => msg!("audit.event.user_permissions_changed"),
            "role_permissions_changed" => msg!("audit.event.role_permissions_changed"),
            "user_updated" => msg!("audit.event.user_updated"),
            "mail_settings_changed" => msg!("audit.event.mail_settings_changed"),
            "role_created" => msg!("audit.event.role_created"),
            "role_updated" => msg!("audit.event.role_updated"),
            "role_deleted" => msg!("audit.event.role_deleted"),
            "organization_profile_changed" => msg!("audit.event.organization_profile_changed"),
            "organization_logo_changed" => msg!("audit.event.organization_logo_changed"),
            other => {
                let mut label = other.replace('_', " ");
                if let Some(first) = label.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                Message::literal(label)
            }
        }
    }

    /// Whether this entry is worth drawing attention to.
    ///
    /// A failure always is. So is anything that changed what somebody may do,
    /// because that is what an audit is read to find.
    pub fn is_notable(&self) -> bool {
        !self.succeeded || NOTABLE_EVENTS.contains(&self.event.as_str())
    }

    /// What happened, as a sentence.
    ///
    /// One key per event *per outcome*, and the whole sentence in each. The
    /// version before this held a past-tense clause and an infinitive and glued
    /// one of them into `"{who} {past}"` or `"{who} tried to {infinitive}"`.
    /// That is English grammar written into Rust: German splits the verb - "hat
    /// sich angemeldet" against "hat versucht, sich anzumelden" - so no frame
    /// can take both forms of one clause, and the sentence has to belong to the
    /// catalog whole.
    ///
    /// The address and the reason are appended as their *own* sentences rather
    /// than as trailing clauses. A clause has to agree with what it joins; a
    /// sentence does not, so concatenation stays safe in any language.
    ///
    /// The date comes through [`i18n::datetime`](crate::i18n::datetime), month
    /// names and assembly order both.
    pub fn narration(&self, catalog: &Catalog) -> String {
        let who = match self.email.as_deref() {
            Some(email) => email.to_owned(),
            None => catalog.render(&msg!("audit.actor.unknown")),
        };
        let when = datetime::moment_long(catalog, self.occurred_at);

        let mut sentence = catalog.render(&self.said(&who, &when));

        if let Some(ip) = &self.ip {
            sentence.push(' ');
            sentence.push_str(&catalog.render(&msg!("audit.said.from_address", ip = ip.clone())));
        }

        if !self.succeeded {
            // The server's own reason, which is recorded here precisely
            // because the sign-in form is not allowed to say it out loud.
            let message = match &self.summary {
                Some(reason) => {
                    msg!("audit.said.failed_because", reason = reason.clone())
                }
                None => msg!("audit.said.failed"),
            };

            sentence.push(' ');
            sentence.push_str(&catalog.render(&message));
        }

        sentence
    }

    /// The sentence for this event and this outcome.
    ///
    /// Written out rather than built from `self.event`, because `msg!` checks
    /// its key at compile time and can only do that with a literal.
    fn said(&self, who: &str, when: &str) -> Message {
        let who = who.to_owned();
        let when = when.to_owned();

        if self.succeeded {
            match self.event.as_str() {
                "signup" => msg!("audit.said.signup.did", who = who, when = when),
                "login" => msg!("audit.said.login.did", who = who, when = when),
                "logout" => msg!("audit.said.logout.did", who = who, when = when),
                "password_change" => msg!("audit.said.password_change.did", who = who, when = when),
                "password_reset_requested" => msg!(
                    "audit.said.password_reset_requested.did",
                    who = who,
                    when = when
                ),
                "password_reset_completed" => msg!(
                    "audit.said.password_reset_completed.did",
                    who = who,
                    when = when
                ),
                "email_verification_sent" => msg!(
                    "audit.said.email_verification_sent.did",
                    who = who,
                    when = when
                ),
                "email_verified" => msg!("audit.said.email_verified.did", who = who, when = when),
                "mfa_enrolled" => msg!("audit.said.mfa_enrolled.did", who = who, when = when),
                "mfa_challenge" => msg!("audit.said.mfa_challenge.did", who = who, when = when),
                "mfa_removed" => msg!("audit.said.mfa_removed.did", who = who, when = when),
                "mfa_recovery_used" => {
                    msg!("audit.said.mfa_recovery_used.did", who = who, when = when)
                }
                "mfa_recovery_generated" => msg!(
                    "audit.said.mfa_recovery_generated.did",
                    who = who,
                    when = when
                ),
                "account_locked" => msg!("audit.said.account_locked.did", who = who, when = when),
                "account_unlocked" => {
                    msg!("audit.said.account_unlocked.did", who = who, when = when)
                }
                "role_changed" => msg!("audit.said.role_changed.did", who = who, when = when),
                "session_revoked" => msg!("audit.said.session_revoked.did", who = who, when = when),
                "invitation_sent" => msg!("audit.said.invitation_sent.did", who = who, when = when),
                "invitation_accepted" => {
                    msg!("audit.said.invitation_accepted.did", who = who, when = when)
                }
                "password_policy_changed" => msg!(
                    "audit.said.password_policy_changed.did",
                    who = who,
                    when = when
                ),
                "mfa_policy_changed" => {
                    msg!("audit.said.mfa_policy_changed.did", who = who, when = when)
                }
                "user_permissions_changed" => msg!(
                    "audit.said.user_permissions_changed.did",
                    who = who,
                    when = when
                ),
                "role_permissions_changed" => msg!(
                    "audit.said.role_permissions_changed.did",
                    who = who,
                    when = when
                ),
                "user_updated" => msg!("audit.said.user_updated.did", who = who, when = when),
                "mail_settings_changed" => msg!(
                    "audit.said.mail_settings_changed.did",
                    who = who,
                    when = when
                ),
                "role_created" => msg!("audit.said.role_created.did", who = who, when = when),
                "role_updated" => msg!("audit.said.role_updated.did", who = who, when = when),
                "role_deleted" => msg!("audit.said.role_deleted.did", who = who, when = when),
                "organization_profile_changed" => msg!(
                    "audit.said.organization_profile_changed.did",
                    who = who,
                    when = when
                ),
                "organization_logo_changed" => msg!(
                    "audit.said.organization_logo_changed.did",
                    who = who,
                    when = when
                ),
                // An event written by a newer release. Reads awkwardly, and
                // reads, which is the whole requirement.
                other => msg!(
                    "audit.said.unknown.did",
                    who = who,
                    when = when,
                    event = other.replace('_', " ")
                ),
            }
        } else {
            match self.event.as_str() {
                "signup" => msg!("audit.said.signup.tried", who = who, when = when),
                "login" => msg!("audit.said.login.tried", who = who, when = when),
                "logout" => msg!("audit.said.logout.tried", who = who, when = when),
                "password_change" => {
                    msg!("audit.said.password_change.tried", who = who, when = when)
                }
                "password_reset_requested" => msg!(
                    "audit.said.password_reset_requested.tried",
                    who = who,
                    when = when
                ),
                "password_reset_completed" => msg!(
                    "audit.said.password_reset_completed.tried",
                    who = who,
                    when = when
                ),
                "email_verification_sent" => msg!(
                    "audit.said.email_verification_sent.tried",
                    who = who,
                    when = when
                ),
                "email_verified" => msg!("audit.said.email_verified.tried", who = who, when = when),
                "mfa_enrolled" => msg!("audit.said.mfa_enrolled.tried", who = who, when = when),
                "mfa_challenge" => msg!("audit.said.mfa_challenge.tried", who = who, when = when),
                "mfa_removed" => msg!("audit.said.mfa_removed.tried", who = who, when = when),
                "mfa_recovery_used" => {
                    msg!("audit.said.mfa_recovery_used.tried", who = who, when = when)
                }
                "mfa_recovery_generated" => msg!(
                    "audit.said.mfa_recovery_generated.tried",
                    who = who,
                    when = when
                ),
                "account_locked" => msg!("audit.said.account_locked.tried", who = who, when = when),
                "account_unlocked" => {
                    msg!("audit.said.account_unlocked.tried", who = who, when = when)
                }
                "role_changed" => msg!("audit.said.role_changed.tried", who = who, when = when),
                "session_revoked" => {
                    msg!("audit.said.session_revoked.tried", who = who, when = when)
                }
                "invitation_sent" => {
                    msg!("audit.said.invitation_sent.tried", who = who, when = when)
                }
                "invitation_accepted" => msg!(
                    "audit.said.invitation_accepted.tried",
                    who = who,
                    when = when
                ),
                "password_policy_changed" => msg!(
                    "audit.said.password_policy_changed.tried",
                    who = who,
                    when = when
                ),
                "mfa_policy_changed" => msg!(
                    "audit.said.mfa_policy_changed.tried",
                    who = who,
                    when = when
                ),
                "user_permissions_changed" => msg!(
                    "audit.said.user_permissions_changed.tried",
                    who = who,
                    when = when
                ),
                "role_permissions_changed" => msg!(
                    "audit.said.role_permissions_changed.tried",
                    who = who,
                    when = when
                ),
                "user_updated" => msg!("audit.said.user_updated.tried", who = who, when = when),
                "mail_settings_changed" => msg!(
                    "audit.said.mail_settings_changed.tried",
                    who = who,
                    when = when
                ),
                "role_created" => msg!("audit.said.role_created.tried", who = who, when = when),
                "role_updated" => msg!("audit.said.role_updated.tried", who = who, when = when),
                "role_deleted" => msg!("audit.said.role_deleted.tried", who = who, when = when),
                "organization_profile_changed" => msg!(
                    "audit.said.organization_profile_changed.tried",
                    who = who,
                    when = when
                ),
                "organization_logo_changed" => msg!(
                    "audit.said.organization_logo_changed.tried",
                    who = who,
                    when = when
                ),
                other => msg!(
                    "audit.said.unknown.tried",
                    who = who,
                    when = when,
                    event = other.replace('_', " ")
                ),
            }
        }
    }
}

/// The events worth finding whether or not they succeeded.
///
/// Public because a reader that pages the trail cannot call
/// [`AuditEvent::is_notable`] on rows it has not fetched yet: "only the
/// notable ones" has to become a `WHERE` clause, and it has to be this list
/// rather than a second copy of it.
pub const NOTABLE_EVENTS: &[&str] = &[
    "mfa_recovery_used",
    "account_locked",
    "role_changed",
    "password_policy_changed",
    "mfa_policy_changed",
    "user_permissions_changed",
    "role_permissions_changed",
    // A status change here is what suspends or reinstates somebody, and a role
    // change is what a privilege review chases. Both arrive under this event.
    "user_updated",
    // Redirecting a workspace's mail redirects every invitation and every
    // password reset with it.
    "mail_settings_changed",
    // Defining a role is defining a way to hold permissions, and deleting one
    // silently strips whatever it granted from everybody who held it. Editing
    // one can make it the default, which reaches every account created after.
    "role_created",
    "role_updated",
    "role_deleted",
    // The legal name, registration number and address go on every document
    // this workspace issues, so a change here reaches further than the screen
    // it was made on.
    "organization_profile_changed",
    // The logo goes on the same documents the legal name does, and swapping
    // it is the same kind of act.
    "organization_logo_changed",
];

/// One entry, opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEventDetail {
    pub event: AuditEvent,
    /// The browser the request came from, verbatim. Shown only here, never in
    /// the list: it is long, and it matters exactly once - when somebody is
    /// working out whether two sign-ins were the same person.
    pub user_agent: Option<String>,
    /// Empty for the events that changed nothing, which is what decides
    /// between a diff and a narration.
    pub changes: Vec<FieldChange>,
    pub facts: Vec<Fact>,
}

impl AuditEventDetail {
    /// Whether this entry recorded a change to something, as opposed to
    /// something merely happening.
    pub fn is_entity_change(&self) -> bool {
        !self.changes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built-in English, which is what these tests assert the words of.
    fn english() -> Catalog {
        Catalog::builtin(crate::Language::ENGLISH)
    }

    fn event(name: &str, succeeded: bool) -> AuditEvent {
        AuditEvent {
            id: 1,
            event: name.to_owned(),
            succeeded,
            user_id: None,
            email: None,
            ip: None,
            summary: None,
            occurred_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn known_events_read_as_sentences() {
        // On the key, because the words belong to the catalog now - and on
        // the rendering too, because a key alone does not prove a sentence.
        assert_eq!(
            event("mfa_enrolled", true).label().key,
            "audit.event.mfa_enrolled"
        );
        assert_eq!(
            event("user_permissions_changed", true).label().to_string(),
            "Individual permissions changed"
        );
    }

    #[test]
    fn editing_an_account_reads_as_itself_rather_than_as_the_fallback() {
        // The fallback would give "User updated" and "recorded user updated",
        // which reads and says nothing about what an audit is looking for.
        let edited = event("user_updated", true);

        assert_eq!(edited.label().to_string(), "Account edited");
        assert!(edited.narration(&english()).contains("edited an account"));
        assert!(
            event("user_updated", false)
                .narration(&english())
                .contains("tried to edit an account")
        );
    }

    #[test]
    fn a_status_or_role_change_is_worth_finding_whether_or_not_it_succeeded() {
        // A suspension and a role grant both arrive under this event, and both
        // are what a privilege review pages the trail for.
        assert!(NOTABLE_EVENTS.contains(&"user_updated"));
        assert!(event("user_updated", true).is_notable());
    }

    #[test]
    fn an_event_this_build_does_not_know_still_reads_as_something() {
        // The case that matters on a rolling deploy: a row written by a newer
        // release, read by an older screen.
        // A literal, not a key: this build has never heard of the event, and
        // the stored name opened out is the honest answer.
        let unknown = event("widget_exploded", true).label();
        assert!(unknown.literal);
        assert_eq!(unknown.to_string(), "Widget exploded");
    }

    fn at(event_name: &str, succeeded: bool) -> AuditEvent {
        AuditEvent {
            occurred_at: chrono::DateTime::from_timestamp(1_770_000_000, 0).unwrap(),
            ..event(event_name, succeeded)
        }
    }

    #[test]
    fn an_event_with_nothing_to_diff_narrates_as_a_sentence() {
        let signed_in = AuditEvent {
            email: Some("ada@example.test".to_owned()),
            ip: Some("203.0.113.7".to_owned()),
            ..at("login", true)
        };

        // Two sentences now, not one clause glued onto another: the address
        // stands on its own so a translation can put it where it belongs.
        assert_eq!(
            signed_in.narration(&english()),
            "ada@example.test signed in on 2 February 2026 at 02:40 UTC. From 203.0.113.7.",
        );
    }

    #[test]
    fn a_failure_narrates_as_an_attempt_and_says_why() {
        let refused = AuditEvent {
            email: Some("nobody@example.test".to_owned()),
            summary: Some("no such account".to_owned()),
            ..at("login", false)
        };

        assert_eq!(
            refused.narration(&english()),
            "nobody@example.test tried to sign in on 2 February 2026 at 02:40 UTC. \
             It did not succeed: no such account.",
        );
    }

    #[test]
    fn an_event_with_no_account_behind_it_still_names_somebody() {
        assert!(
            at("login", false)
                .narration(&english())
                .starts_with("Somebody tried to sign in")
        );
    }

    #[test]
    fn an_event_this_build_does_not_know_still_narrates() {
        // Same rolling-deploy case as the label test, in the other direction:
        // the sentence has to survive a verb nobody wrote.
        let narration = at("widget_exploded", true).narration(&english());

        assert!(
            narration.starts_with("Somebody recorded widget exploded"),
            "{narration}"
        );
    }

    #[test]
    fn the_notable_list_and_the_notable_test_cannot_disagree() {
        // The reader that pages the trail turns NOTABLE_EVENTS into SQL while
        // is_notable answers for a row already in hand. Two answers to one
        // question is the bug this forecloses.
        for name in NOTABLE_EVENTS {
            assert!(
                event(name, true).is_notable(),
                "{name} is in the list and not notable"
            );
        }
    }

    #[test]
    fn failures_and_privilege_changes_are_notable() {
        assert!(event("login", false).is_notable());
        assert!(event("role_permissions_changed", true).is_notable());
        assert!(event("mfa_recovery_used", true).is_notable());

        // An ordinary successful sign-in is not.
        assert!(!event("login", true).is_notable());
    }
}
