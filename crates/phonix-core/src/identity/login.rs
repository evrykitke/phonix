//! Sign-in payloads and outcomes.

use serde::{Deserialize, Serialize};

use crate::i18n::Message;
use crate::{msg, pmsg};

use super::user::{AuthUser, UserId};

/// Sign-in payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
    /// Extends the session to the "remember me" ceiling instead of the ordinary
    /// absolute one.
    #[serde(default)]
    pub remember_me: bool,
}

/// The outcome of a sign-in attempt.
///
/// Every rejection variant is deliberately vague about *why*: telling an
/// anonymous caller the difference between "no such account" and "wrong
/// password" turns the login form into an account-enumeration oracle. The real
/// reason is recorded in `identity_events` for the people who need it.
///
/// As with signup, a wrong password is `Ok(Rejected)` rather than an `Err` -
/// it is the expected path, not a fault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoginResult {
    /// Fully signed in.
    Success(Box<AuthUser>),
    /// Password accepted, second factor still required.
    MfaRequired { user_id: UserId },
    /// Password accepted, but the workspace requires a second factor this user
    /// has not enrolled and their grace period has run out. They hold a session
    /// that can reach the enrolment screen and nothing else.
    MfaEnrolmentRequired { user_id: UserId },
    /// Password accepted, but it has aged past the workspace's expiry policy or
    /// was flagged for a forced change. Same deal: a session that can reach the
    /// change-password screen and nothing else.
    PasswordChangeRequired { user_id: UserId },
    /// Wrong email, wrong password, unknown account, suspended account.
    Rejected,
    /// Too many failed attempts. The wait is stated because, unlike the
    /// rejection reason, it is not a secret - the caller triggered it.
    Locked { retry_after_secs: u64 },
}

/// The sign-in form. It is the workspace root, so an unauthenticated visitor
/// lands on it without a redirect.
pub const SIGN_IN_PATH: &str = "/";

/// Where an invitation link lands.
///
/// Public, and it has to be: the person following it has no session - that is
/// the whole point of an invitation. It lives here rather than in the service
/// that mints the link so that the route, the guard and the link are one
/// string; a link that does not match its route is a dead invitation nothing
/// catches until somebody clicks it.
pub const INVITATION_ACCEPT_PATH: &str = "/invitations/accept";

/// Where a fully authenticated session begins.
pub const DASHBOARD_PATH: &str = "/dashboard";

/// Where a session that owes a second factor answers for it.
pub const MFA_CHALLENGE_PATH: &str = "/auth/challenge";

/// Where a session the workspace requires a factor from enrols one.
pub const MFA_ENROLMENT_PATH: &str = "/auth/set-up-two-factor";

/// Where a session carrying an expired password sets a new one.
pub const PASSWORD_CHANGE_PATH: &str = "/auth/change-password";

/// The prefix every screen a half-authenticated session may reach sits under.
///
/// [`landing`] uses this to decide what to send such a session away from, so a
/// screen added below `/auth/` is reachable mid-sign-in by construction rather
/// than by remembering to add it to a list.
pub const HALF_AUTHENTICATED_PREFIX: &str = "/auth/";

/// Where a workspace is created. Reachable with no session, like [`SIGN_IN_PATH`].
pub const SIGN_UP_PATH: &str = "/signup";

/// Where somebody who cannot sign in asks for a code and sets a new password.
///
/// Public for the plainest possible reason: the person using it has forgotten
/// the credential a session is made from. Deliberately *not* under
/// [`HALF_AUTHENTICATED_PREFIX`] - nothing has been proved at this point, and
/// putting it there would make it reachable mid-sign-in, which is a different
/// screen for a different situation ([`PASSWORD_CHANGE_PATH`]).
pub const PASSWORD_RESET_PATH: &str = "/forgot-password";

/// Whether `path` is reachable with no session at all.
///
/// The four screens somebody uses *before* they have one: signing in, creating
/// a workspace, accepting an invitation, and resetting a forgotten password.
///
/// # Why this is a function and not four comparisons at each call site
///
/// Three things need this answer and they must never disagree: [`landing`]
/// decides who is turned away, the layout decides which chrome the screen
/// gets, and the server decides which requests are rate limited as public
/// traffic. When the list was written out in each of them, adding
/// `/forgot-password` to [`landing`] left the other two behind - so the new
/// screen was reachable, and rendered inside the signed-in application shell
/// for somebody with no session to put in it.
///
/// A screen is public or it is not. One list, one answer.
pub fn is_public_path(path: &str) -> bool {
    path == SIGN_IN_PATH
        || path == SIGN_UP_PATH
        || path == INVITATION_ACCEPT_PATH
        || path == PASSWORD_RESET_PATH
}

/// Whether `path` belongs to somebody who is not yet through the door.
///
/// True for every public screen and for every step of a sign-in that has
/// started but not finished. This is the question the chrome asks: all of these
/// are rendered on their own, without the navigation panel and top bar of an
/// application nobody has been admitted to yet.
pub fn is_signed_out_chrome(path: &str) -> bool {
    is_public_path(path) || path.starts_with(HALF_AUTHENTICATED_PREFIX)
}

/// What to do with a request for `path`, given who is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// Render the path that was asked for.
    Stay,
    /// Send the browser somewhere else instead.
    Redirect(&'static str),
}

/// Decide whether a session may see `path`, or where it belongs instead.
///
/// # Why this is one function
///
/// Each screen used to answer this for itself, from inside its own resource -
/// which meant the answer arrived after its HTML had already been written,
/// making every redirect a visible reload rather than a 302. Three screens also
/// meant three chances to get it wrong, and the case nobody tested was the one
/// that mattered: a half-authenticated session holds no permissions, so every
/// page it reached rendered "not authorised" instead of saying what was
/// actually missing.
///
/// Deciding here, from a path and an optional user, makes the whole matrix
/// testable without a browser, a database, or a session.
///
/// This is navigation, **not** authorization. Nothing here decides what a
/// request may do - `Caller::require` does, per use case, on the server. A
/// session that gets past this function still cannot read a thing it lacks the
/// permission for.
pub fn landing(path: &str, session: Option<&super::user::AuthUser>) -> Landing {
    let is_public = is_public_path(path);
    let is_sign_in_step = path.starts_with(HALF_AUTHENTICATED_PREFIX);

    match session {
        // Finished. The only wrong place to be is one of the screens that
        // exist to get you here - showing a sign-in form to somebody already
        // signed in reads as though the sign-in silently failed.
        Some(user) if user.is_fully_authenticated() => {
            if is_public || is_sign_in_step {
                Landing::Redirect(DASHBOARD_PATH)
            } else {
                Landing::Stay
            }
        }
        // Password accepted, second factor outstanding.
        Some(_) if is_sign_in_step => Landing::Stay,
        Some(_) => Landing::Redirect(MFA_CHALLENGE_PATH),
        // Nobody. `/auth/*` is included here: those screens need a session to
        // mean anything, and without one the honest answer is the form.
        None if is_public => Landing::Stay,
        None => Landing::Redirect(SIGN_IN_PATH),
    }
}

impl LoginResult {
    /// Whether the caller proved they know the password.
    ///
    /// True for the three "yes, but" outcomes as well as `Success`, because all
    /// four hold a session. What separates them is what that session may reach,
    /// which is [`super::user::AuthUser::is_fully_authenticated`]'s job, not
    /// this one's.
    pub fn password_accepted(&self) -> bool {
        matches!(
            self,
            Self::Success(_)
                | Self::MfaRequired { .. }
                | Self::MfaEnrolmentRequired { .. }
                | Self::PasswordChangeRequired { .. }
        )
    }

    /// Where the browser goes next, relative to the workspace root.
    ///
    /// Never [`SIGN_IN_PATH`] for an outcome that holds a session: sending a
    /// signed-in browser back to the form is indistinguishable from the sign-in
    /// having done nothing at all.
    pub fn next_path(&self) -> &'static str {
        match self {
            Self::Success(_) => DASHBOARD_PATH,
            Self::MfaRequired { .. } => MFA_CHALLENGE_PATH,
            Self::MfaEnrolmentRequired { .. } => MFA_ENROLMENT_PATH,
            Self::PasswordChangeRequired { .. } => PASSWORD_CHANGE_PATH,
            Self::Rejected | Self::Locked { .. } => SIGN_IN_PATH,
        }
    }

    /// Message to show under the form.
    pub fn message(&self) -> Option<Message> {
        match self {
            Self::Success(_)
            | Self::MfaRequired { .. }
            | Self::MfaEnrolmentRequired { .. }
            | Self::PasswordChangeRequired { .. } => None,
            Self::Rejected => Some(msg!("auth.rejected")),
            Self::Locked { retry_after_secs } => {
                let minutes = retry_after_secs.div_ceil(60).max(1);

                // `pmsg!` rather than a hand-rolled `if minutes == 1`. English
                // has two forms and picking between them inline reads fine
                // until a language with three or six needs the same sentence,
                // at which point the choice has to be the catalog's rather
                // than this function's.
                Some(pmsg!(
                    "auth.locked",
                    i64::try_from(minutes).unwrap_or(i64::MAX)
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejection_never_says_which_half_was_wrong() {
        let message = LoginResult::Rejected.message().unwrap().to_string();
        for leak in ["no such", "unknown", "not found", "suspended", "exists"] {
            assert!(
                !message.to_lowercase().contains(leak),
                "{message:?} leaks {leak:?}"
            );
        }
    }

    #[test]
    fn a_lockout_states_the_wait_in_whole_minutes() {
        assert_eq!(
            LoginResult::Locked {
                retry_after_secs: 900
            }
            .message()
            .unwrap()
            .to_string(),
            "Too many failed attempts. Try again in 15 minutes."
        );
        // Rounds up, and never says "0 minutes".
        assert!(
            LoginResult::Locked {
                retry_after_secs: 30
            }
            .message()
            .unwrap()
            .to_string()
            .contains("1 minute.")
        );
    }

    /// The regression that made [`is_public_path`] a function.
    ///
    /// `/forgot-password` was added to `landing` and to nothing else, so it was
    /// reachable with no session and rendered inside the signed-in application
    /// shell. Anything `landing` treats as public must also get signed-out
    /// chrome, and this asserts the two cannot part company again.
    #[test]
    fn every_public_screen_is_reachable_and_wears_signed_out_chrome() {
        for path in [
            SIGN_IN_PATH,
            SIGN_UP_PATH,
            INVITATION_ACCEPT_PATH,
            PASSWORD_RESET_PATH,
        ] {
            assert!(is_public_path(path), "{path}");
            assert!(is_signed_out_chrome(path), "{path}");
            // Public means exactly this: no session, and the screen renders.
            assert_eq!(landing(path, None), Landing::Stay, "{path}");
        }
    }

    #[test]
    fn a_screen_inside_the_application_is_neither() {
        for path in [DASHBOARD_PATH, "/account", "/admin/users"] {
            assert!(!is_public_path(path), "{path}");
            assert!(!is_signed_out_chrome(path), "{path}");
        }
    }

    #[test]
    fn a_half_finished_sign_in_gets_signed_out_chrome_without_being_public() {
        for path in [MFA_CHALLENGE_PATH, MFA_ENROLMENT_PATH, PASSWORD_CHANGE_PATH] {
            // Not public - reaching these needs a password already accepted.
            assert!(!is_public_path(path), "{path}");
            // But still outside the application, so still bare.
            assert!(is_signed_out_chrome(path), "{path}");
        }
    }

    #[test]
    fn every_half_authenticated_outcome_leads_somewhere_it_can_finish() {
        let user_id = uuid::Uuid::nil();
        let unfinished = [
            LoginResult::MfaRequired { user_id },
            LoginResult::MfaEnrolmentRequired { user_id },
            LoginResult::PasswordChangeRequired { user_id },
        ];

        for outcome in unfinished {
            assert!(outcome.password_accepted());
            assert!(outcome.message().is_none());
            // Somewhere other than the sign-in form, or the browser bounces
            // between the two for ever.
            assert_ne!(outcome.next_path(), SIGN_IN_PATH);
        }

        assert!(!LoginResult::Rejected.password_accepted());
        assert_eq!(LoginResult::Rejected.next_path(), SIGN_IN_PATH);
    }

    #[test]
    fn a_finished_sign_in_lands_on_the_dashboard() {
        // Regression: this used to be `/`, which is the sign-in form. A correct
        // password put the browser straight back where it started, so a
        // successful sign-in was indistinguishable from a broken one.
        let success = LoginResult::Success(Box::new(super::super::AuthUser {
            id: uuid::Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["Admin".into()],
            permissions: crate::authorization::PermissionSet::all(),
            is_owner: true,
            status: super::super::UserStatus::Active,
            mfa_enabled: false,
            mfa_satisfied: true,
            email_verified: true,
        }));

        assert_eq!(success.next_path(), DASHBOARD_PATH);
        assert_ne!(success.next_path(), SIGN_IN_PATH);
    }

    /// An `AuthUser` in one of the states `landing` distinguishes.
    fn user(mfa_enabled: bool, mfa_satisfied: bool) -> super::super::AuthUser {
        super::super::AuthUser {
            id: uuid::Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["Admin".into()],
            permissions: crate::authorization::PermissionSet::all(),
            is_owner: true,
            status: super::super::UserStatus::Active,
            mfa_enabled,
            mfa_satisfied,
            email_verified: true,
        }
    }

    #[test]
    fn nobody_may_reach_anything_but_the_public_screens() {
        assert_eq!(landing(SIGN_IN_PATH, None), Landing::Stay);
        assert_eq!(landing(SIGN_UP_PATH, None), Landing::Stay);
        assert_eq!(landing(INVITATION_ACCEPT_PATH, None), Landing::Stay);
        // The one that is easiest to get wrong: somebody who cannot sign in is
        // exactly who this screen is for, so a guard that sends them to the
        // sign-in form would be a loop with no way out of it.
        assert_eq!(landing(PASSWORD_RESET_PATH, None), Landing::Stay);

        for path in [DASHBOARD_PATH, "/account", "/admin/users"] {
            assert_eq!(
                landing(path, None),
                Landing::Redirect(SIGN_IN_PATH),
                "{path} should send an anonymous visitor to the form",
            );
        }
    }

    #[test]
    fn an_anonymous_visitor_does_not_get_to_wait_at_the_challenge() {
        // The challenge screen without a session has nothing to ask about, and
        // rendering it would invite somebody to type codes at a sign-in that
        // does not exist.
        assert_eq!(
            landing(MFA_CHALLENGE_PATH, None),
            Landing::Redirect(SIGN_IN_PATH)
        );
    }

    #[test]
    fn a_half_authenticated_session_is_held_at_the_sign_in_steps() {
        let waiting = user(true, false);
        assert!(!waiting.is_fully_authenticated());

        assert_eq!(landing(MFA_CHALLENGE_PATH, Some(&waiting)), Landing::Stay);
        assert_eq!(landing(MFA_ENROLMENT_PATH, Some(&waiting)), Landing::Stay);
        assert_eq!(landing(PASSWORD_CHANGE_PATH, Some(&waiting)), Landing::Stay);

        // Everything else, including the screens an anonymous visitor may see:
        // this session has a password behind it and belongs at the challenge,
        // not back at the form.
        for path in [DASHBOARD_PATH, "/account", "/admin/users", SIGN_IN_PATH] {
            assert_eq!(
                landing(path, Some(&waiting)),
                Landing::Redirect(MFA_CHALLENGE_PATH),
                "{path} should send a half-authenticated session to the challenge",
            );
        }
    }

    #[test]
    fn a_finished_session_is_sent_off_the_screens_that_end_a_sign_in() {
        let done = user(true, true);
        assert!(done.is_fully_authenticated());

        for path in [
            SIGN_IN_PATH,
            SIGN_UP_PATH,
            MFA_CHALLENGE_PATH,
            // Somebody who is signed in has not forgotten their password, and
            // the screen for changing one on purpose is in the account
            // settings behind a current-password check.
            PASSWORD_RESET_PATH,
        ] {
            assert_eq!(
                landing(path, Some(&done)),
                Landing::Redirect(DASHBOARD_PATH),
                "{path} is not a place a signed-in session belongs",
            );
        }

        for path in [DASHBOARD_PATH, "/account", "/admin/users"] {
            assert_eq!(landing(path, Some(&done)), Landing::Stay);
        }
    }

    #[test]
    fn a_session_without_mfa_at_all_is_finished() {
        // The common case: the workspace does not require a second factor, so
        // `mfa_satisfied` is meaningless and must not hold anybody back.
        let done = user(false, false);
        assert_eq!(landing(DASHBOARD_PATH, Some(&done)), Landing::Stay);
        assert_eq!(
            landing(SIGN_IN_PATH, Some(&done)),
            Landing::Redirect(DASHBOARD_PATH)
        );
    }

    #[test]
    fn every_path_a_sign_in_can_end_on_is_one_a_half_session_may_reach() {
        // `next_path` and `landing` have to agree. If they drift, a sign-in
        // redirects to a screen that immediately redirects back - a loop the
        // visitor sees as a page that will not load.
        let waiting = user(true, false);

        for result in [
            LoginResult::MfaRequired {
                user_id: uuid::Uuid::nil(),
            },
            LoginResult::MfaEnrolmentRequired {
                user_id: uuid::Uuid::nil(),
            },
            LoginResult::PasswordChangeRequired {
                user_id: uuid::Uuid::nil(),
            },
        ] {
            assert_eq!(
                landing(result.next_path(), Some(&waiting)),
                Landing::Stay,
                "{result:?} sends the browser somewhere it is turned away from",
            );
        }
    }

    #[test]
    fn success_has_nothing_to_say() {
        assert!(LoginResult::Rejected.message().is_some());
        assert!(
            LoginResult::MfaRequired {
                user_id: uuid::Uuid::nil()
            }
            .message()
            .is_none()
        );
    }
}
