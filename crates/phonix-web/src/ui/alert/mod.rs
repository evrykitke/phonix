//! Alerts: the four ways this application tells somebody what happened.
//!
//! # Why this is one module and not four components
//!
//! Every screen eventually has to say "that worked", "that did not work" or
//! "are you sure?". Written per screen those come out as a `window.confirm`
//! here, a coloured `<div>` there, and a message that never appears at all on
//! the third - because the person had already scrolled past the place it was
//! rendered. The words are the screen's business; *where they appear and what
//! they look like* is not, and that is what lives here.
//!
//! # The four surfaces
//!
//! | Surface | Interrupts? | For |
//! | ------- | ----------- | --- |
//! | [`Channel::Toast`] | no | the default outcome of a form action |
//! | [`Channel::MessageBox`] | yes, one button | an outcome that must be read |
//! | [`Channel::Inline`] | no | a short form, where the message fits beside it |
//! | [`Confirm`] | yes, two buttons | *before* something that cannot be undone |
//!
//! The first three are the same value - an [`Alert`] - posted down a different
//! [`Channel`]. That is deliberate: a screen changes its mind about how loud a
//! message should be far more often than about what the message says, and this
//! way that change is one word.
//!
//! [`Confirm`] is the odd one out, and stays a separate type, because it is the
//! only one that asks a question rather than reporting an answer. It carries a
//! callback and has no tone-only variant: everything else here happens *after*
//! the deed.
//!
//! # Choosing a channel
//!
//! **Toast is the default for a form action.** By the time a save comes back
//! the person is usually looking somewhere else - at the next field, at the
//! tab they switched to - and a line rendered at the top of a form they have
//! scrolled past is a line nobody reads.
//!
//! **A message box when the outcome has to be acknowledged.** It costs a click,
//! which is the point: use it where carrying on without having read the result
//! would be a mistake, not merely untidy.
//!
//! **Inline for a short form**, where the whole form is on screen at once and a
//! toast in the corner is further from the person's eyes than the button they
//! just pressed.
//!
//! # One vocabulary
//!
//! Every surface draws itself from [`Tone::face`] - the same icon, the same
//! green, the same red, the same treatment of a border. Success and failure are
//! *not* different components: a save that fails posts the same shape of alert
//! as one that succeeds, down the same channel, so a screen cannot accidentally
//! offer a good outcome a home and leave a bad one with nowhere to go.
//!
//! # Posting one
//!
//! ```ignore
//! // From an action's chain, which is the usual way - see `ui::form::action`.
//! FormAction::submit("Save changes")
//!     .then(Then::Say("Role saved."))            // the form's own channel
//!     .then(Then::Alert(Alert::success("Role saved.").message_box()))
//!
//! // By hand, from anything that is not a form.
//! Alerts::get().post(Alert::failure(err.to_string()).message_box());
//! ```

pub mod confirm;
pub mod host;

use std::time::Duration;

use leptos::prelude::*;

pub use confirm::Confirm;
pub use host::AlertLayer;

use crate::components::page::Tone;

/// How many toasts may stack before the oldest is pushed off.
///
/// Five outcomes at once is a screen doing something in a loop, and a column of
/// cards tall enough to cover the content it is reporting on is worse than
/// missing the first one.
const STACK: usize = 4;

/// How long a toast that takes itself down stays up.
const LINGER: Duration = Duration::from_secs(5);

/// Where an alert is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// A card in the corner that says its piece and leaves. The default,
    /// because the person is usually not looking where the form is.
    #[default]
    Toast,
    /// A dialog over the page with one button, dismissed by hand.
    MessageBox,
    /// A line inside the form itself. Only a form can render this one, so an
    /// alert posted this way with no form to hold it falls back to a toast -
    /// see [`Alerts::post`].
    Inline,
}

/// Something to tell the person, and how loudly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub channel: Channel,
    pub tone: Tone,
    /// A short heading. Ordinary to leave out on a toast, where the message is
    /// one line; worth setting on a message box, which is big enough to look
    /// empty without one.
    pub title: Option<String>,
    pub message: String,
}

impl Alert {
    /// It worked.
    pub fn success(message: impl Into<String>) -> Self {
        Self::of(Tone::Success, message)
    }

    /// It did not work.
    ///
    /// Takes the server's own words rather than a house phrase, for the same
    /// reason [`GridHandle::warn`](crate::ui::table::GridHandle::warn) does:
    /// "you may not remove the last administrator" is worth more than
    /// "something went wrong", and the service has already written it.
    pub fn failure(message: impl Into<String>) -> Self {
        Self::of(Tone::Danger, message)
    }

    /// It worked, and there is something about it worth knowing.
    pub fn warning(message: impl Into<String>) -> Self {
        Self::of(Tone::Warning, message)
    }

    /// Neither good nor bad.
    pub fn info(message: impl Into<String>) -> Self {
        Self::of(Tone::Neutral, message)
    }

    fn of(tone: Tone, message: impl Into<String>) -> Self {
        Self {
            channel: Channel::default(),
            tone,
            title: None,
            message: message.into(),
        }
    }

    /// A heading above the message.
    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Send it somewhere other than the default.
    #[must_use]
    pub const fn through(mut self, channel: Channel) -> Self {
        self.channel = channel;
        self
    }

    #[must_use]
    pub const fn toast(self) -> Self {
        self.through(Channel::Toast)
    }

    #[must_use]
    pub const fn message_box(self) -> Self {
        self.through(Channel::MessageBox)
    }

    #[must_use]
    pub const fn inline(self) -> Self {
        self.through(Channel::Inline)
    }

    /// Whether this one takes itself off the screen.
    ///
    /// Good news does; bad news does not. A failure that vanished after five
    /// seconds is a failure the person can miss entirely, and the one thing
    /// worse than an error message is an error message nobody saw.
    pub const fn fades(&self) -> bool {
        matches!(self.tone, Tone::Success | Tone::Neutral | Tone::Brand)
    }
}

/// One alert on screen, and the identity the list keys on.
///
/// The id is not the message: two saves in a row produce two identical
/// sentences, and keying on the text would make the second one replace the
/// first silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub id: u64,
    pub alert: Alert,
}

/// The one place alerts go.
///
/// Provided once by [`crate::app::App`] and read by anything that has something
/// to report. `Copy`, because everything in it is a signal handle - an action's
/// closure can hold one across an `await` without lifetimes or clones.
#[derive(Clone, Copy)]
pub struct Alerts {
    toasts: RwSignal<Vec<Posted>>,
    /// One at a time. A second message box behind the first is a dialog nobody
    /// knows is there, and dismissing the front one would reveal a message
    /// about something that happened before it.
    boxed: RwSignal<Option<Posted>>,
    asking: RwSignal<Option<Confirm>>,
    next: RwSignal<u64>,
}

impl Alerts {
    /// Make alerts available to everything rendered below. The host calls this
    /// once, and mounts an [`AlertLayer`] to draw them.
    pub fn provide() -> Self {
        let alerts = Self::new();
        provide_context(alerts);
        alerts
    }

    fn new() -> Self {
        Self {
            toasts: RwSignal::new(Vec::new()),
            boxed: RwSignal::new(None),
            asking: RwSignal::new(None),
            next: RwSignal::new(0),
        }
    }

    /// The alerts for this tree.
    ///
    /// Falls back to a detached set rather than panicking, on the same argument
    /// as [`Viewer::get`](crate::ui::viewer::Viewer::get): a kit component must
    /// render in a test that mounted no host. What comes back is a bin with no
    /// bell attached - posting to it is not an error and shows nothing.
    pub fn get() -> Self {
        use_context::<Self>().unwrap_or_else(Self::new)
    }

    /// Show an alert, wherever it says it belongs.
    pub fn post(self, alert: Alert) {
        let id = self.claim();
        let fades = alert.fades();
        let posted = Posted { id, alert };

        match posted.alert.channel {
            Channel::MessageBox => self.boxed.set(Some(posted)),
            // `Inline` has no home here - only a form can draw one - and a
            // message shown in the wrong place beats a message dropped.
            Channel::Toast | Channel::Inline => {
                self.toasts.update(|toasts| {
                    toasts.push(posted);

                    if toasts.len() > STACK {
                        toasts.remove(0);
                    }
                });

                if fades {
                    self.fade(id);
                }
            }
        }
    }

    /// Ask before doing something. The answer arrives as a callback, because a
    /// dialog cannot be waited on the way `window.confirm` could.
    pub fn ask(self, confirm: Confirm) {
        self.asking.set(Some(confirm));
    }

    /// Take one toast down.
    pub fn dismiss(self, id: u64) {
        self.toasts
            .update(|toasts| toasts.retain(|posted| posted.id != id));
    }

    /// Close the message box.
    pub fn close(self) {
        self.boxed.set(None);
    }

    /// Answer the question, and take it off the screen either way.
    ///
    /// The dialog is cleared *before* the callback runs. What was confirmed
    /// often posts an alert of its own, and a dialog still up when that arrives
    /// covers it.
    pub fn answer(self, yes: bool) {
        let Some(confirm) = self.asking.get_untracked() else {
            return;
        };

        self.asking.set(None);

        if yes {
            confirm.on_confirm.run(());
        }
    }

    fn claim(self) -> u64 {
        let id = self.next.get_untracked();
        self.next.set(id + 1);
        id
    }

    /// Start the clock on a toast that takes itself down.
    #[cfg(feature = "hydrate")]
    fn fade(self, id: u64) {
        leptos::prelude::set_timeout(move || self.dismiss(id), LINGER);
    }

    /// On the server nothing is posted and nothing is watching, and there is no
    /// timer to start.
    #[cfg(not(feature = "hydrate"))]
    fn fade(self, _id: u64) {
        let _ = LINGER;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned<T>(build: impl FnOnce() -> T) -> T {
        Owner::new().with(build)
    }

    #[test]
    fn an_alert_is_a_toast_unless_it_says_otherwise() {
        // The default that the module's whole argument rests on: a form action
        // that reports without choosing a surface gets the non-blocking one.
        assert_eq!(Alert::success("Saved.").channel, Channel::Toast);
        assert_eq!(Alert::failure("No.").channel, Channel::Toast);
    }

    #[test]
    fn the_same_message_goes_down_any_channel() {
        let alert = Alert::success("Saved.");

        assert_eq!(alert.clone().message_box().channel, Channel::MessageBox);
        assert_eq!(alert.clone().inline().channel, Channel::Inline);
        assert_eq!(alert.message, "Saved.");
    }

    #[test]
    fn bad_news_stays_on_the_screen_and_good_news_does_not() {
        // The one rule that stops a failure being missed entirely.
        assert!(Alert::success("Saved.").fades());
        assert!(!Alert::failure("Refused.").fades());
        assert!(!Alert::warning("Saved, but.").fades());
    }

    #[test]
    fn every_tone_has_a_face_and_only_danger_colours_its_border() {
        // Four surfaces read this table. A tone that fell back to a default
        // here would be a green that is green in one place and grey in another.
        for tone in [
            Tone::Neutral,
            Tone::Brand,
            Tone::Success,
            Tone::Warning,
            Tone::Danger,
        ] {
            let face = tone.face();

            assert!(face.accent.starts_with("text-"), "{tone:?}");
            assert!(!face.disc.is_empty(), "{tone:?}");
        }

        assert_eq!(Tone::Danger.face().edge, "border-danger");
        assert_eq!(Tone::Success.face().edge, "border-edge");
    }

    #[test]
    fn posting_a_message_box_does_not_touch_the_toasts() {
        owned(|| {
            let alerts = Alerts::new();

            alerts.post(Alert::success("Saved.").message_box());

            assert!(alerts.toasts.get_untracked().is_empty());
            assert_eq!(
                alerts.boxed.get_untracked().unwrap().alert.message,
                "Saved."
            );
        });
    }

    #[test]
    fn an_inline_alert_with_no_form_to_hold_it_becomes_a_toast() {
        // Better in the wrong place than dropped.
        owned(|| {
            let alerts = Alerts::new();

            alerts.post(Alert::failure("Refused.").inline());

            assert_eq!(alerts.toasts.get_untracked().len(), 1);
        });
    }

    #[test]
    fn two_identical_messages_are_two_toasts() {
        // Keyed on an id and not on the text: saving twice has to look like
        // saving twice.
        owned(|| {
            let alerts = Alerts::new();

            alerts.post(Alert::failure("Refused."));
            alerts.post(Alert::failure("Refused."));

            let toasts = alerts.toasts.get_untracked();

            assert_eq!(toasts.len(), 2);
            assert_ne!(toasts[0].id, toasts[1].id);
        });
    }

    #[test]
    fn the_stack_has_a_ceiling_and_the_oldest_goes_first() {
        owned(|| {
            let alerts = Alerts::new();

            for index in 0..STACK + 2 {
                alerts.post(Alert::failure(format!("{index}")));
            }

            let toasts = alerts.toasts.get_untracked();

            assert_eq!(toasts.len(), STACK);
            assert_eq!(toasts[0].alert.message, "2");
        });
    }

    #[test]
    fn only_the_newest_message_box_is_on_screen() {
        owned(|| {
            let alerts = Alerts::new();

            alerts.post(Alert::success("First.").message_box());
            alerts.post(Alert::failure("Second.").message_box());

            assert_eq!(
                alerts.boxed.get_untracked().unwrap().alert.message,
                "Second."
            );
        });
    }

    #[test]
    fn a_confirmed_question_runs_its_callback_once_and_clears() {
        owned(|| {
            let alerts = Alerts::new();
            let ran = RwSignal::new(0);

            alerts.ask(Confirm::new("Delete it?", move || {
                ran.update(|ran| *ran += 1)
            }));
            alerts.answer(true);

            assert_eq!(ran.get_untracked(), 1);
            assert!(alerts.asking.get_untracked().is_none());

            // Answering again is not a second deletion.
            alerts.answer(true);
            assert_eq!(ran.get_untracked(), 1);
        });
    }

    #[test]
    fn a_declined_question_runs_nothing() {
        owned(|| {
            let alerts = Alerts::new();
            let ran = RwSignal::new(false);

            alerts.ask(Confirm::new("Delete it?", move || ran.set(true)));
            alerts.answer(false);

            assert!(!ran.get_untracked());
            assert!(alerts.asking.get_untracked().is_none());
        });
    }

    #[test]
    fn dismissing_takes_down_the_one_named_and_no_other() {
        owned(|| {
            let alerts = Alerts::new();

            alerts.post(Alert::failure("First."));
            alerts.post(Alert::failure("Second."));

            let first = alerts.toasts.get_untracked()[0].id;
            alerts.dismiss(first);

            let toasts = alerts.toasts.get_untracked();

            assert_eq!(toasts.len(), 1);
            assert_eq!(toasts[0].alert.message, "Second.");
        });
    }

    #[test]
    fn posting_with_no_host_is_not_an_error() {
        // What a kit component does in a test that mounted no layer.
        owned(|| {
            Alerts::get().post(Alert::success("Nobody is listening."));
        });
    }
}
