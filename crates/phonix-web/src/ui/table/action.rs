//! Things a grid can do: to one row, or to the list as a whole.
//!
//! # Gating is declared, not remembered
//!
//! Every action carries the permission it needs. The grid hides the ones the
//! viewer does not hold - not disables, hides: a greyed-out "Delete" tells
//! someone what they are missing and gives them nothing to do about it.
//!
//! This is presentation, and it is the weaker half of the pair. **The server
//! decides.** `Caller::require` in the service is what actually refuses the
//! request; hiding a button only stops it being clicked by accident. An action
//! whose `require` is wrong is an untidy screen, and an action whose service is
//! ungated is a hole - so a new action starts at the service and works
//! outwards.
//!
//! # Two kinds, because a link is not a click
//!
//! [`ActionKind::Link`] renders an `<A>`: it can be middle-clicked, copied,
//! opened in a tab, and the router handles it. [`ActionKind::Run`] renders a
//! `<button>` and calls back with the row. Anything that changes data is a
//! `Run`; anything that goes somewhere is a `Link`. A `Run` that navigates and
//! a `Link` that mutates are both bugs.
//!
//! # Clicking the row
//!
//! [`RowAction::on_row_click`] marks one link as the thing a click anywhere on
//! the row does. It is a property of the *action*, not of the grid, so that
//! there is only ever one destination written down: a `row_href` on the
//! configuration beside an Open action pointing at the same page is two copies
//! of one URL, and the day they disagree the row and its menu go to different
//! screens.
//!
//! The action stays in the menu. The click is a shortcut to it and never a
//! replacement for it - which is also the whole of the keyboard story, because
//! a `<tr>` is not focusable and giving it `tabindex` would announce a second
//! link to a screen reader that already has the real one two cells along.
//!
//! Only a [`Link`](ActionKind::Link) may take it. A row that deleted itself
//! when somebody clicked it to read it would be indefensible, and
//! [`GridConfig::action`](super::config::GridConfig::action) refuses the
//! combination in debug builds rather than leaving it to be found.

use std::sync::Arc;

use leptos::prelude::*;
use phonix_core::identity::AuthUser;

use super::handle::GridHandle;
use crate::components::page::Tone;
use crate::icons::Icon;

/// Whether an action means anything for a particular row.
type Applies<T> = Arc<dyn Fn(&T) -> bool + Send + Sync>;

/// Where a link action points, for a particular row.
type Destination<T> = Arc<dyn Fn(&T) -> String + Send + Sync>;

/// What happens when an action is chosen.
pub enum ActionKind<T: 'static> {
    /// Go somewhere. The destination is built from the row.
    Link(Destination<T>),
    /// Do something to this row. The handle is how the action reports what
    /// happened and asks for the table to be re-read - see [`GridHandle`].
    Run(Callback<(T, GridHandle)>),
}

impl<T: 'static> Clone for ActionKind<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Link(href) => Self::Link(Arc::clone(href)),
            Self::Run(run) => Self::Run(*run),
        }
    }
}

/// Something offered on one row.
pub struct RowAction<T: 'static> {
    pub(crate) label: String,
    pub(crate) icon: Icon,
    pub(crate) tone: Tone,
    pub(crate) permission: Option<&'static str>,
    pub(crate) kind: ActionKind<T>,
    /// Whether this action means anything for this particular row.
    pub(crate) available: Option<Applies<T>>,
    /// Text to confirm with before running. `None` runs immediately.
    pub(crate) confirm: Option<String>,
    /// Whether a click on the row itself performs this action.
    pub(crate) row_click: bool,
}

impl<T: 'static> Clone for RowAction<T> {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            icon: self.icon,
            tone: self.tone,
            permission: self.permission,
            kind: self.kind.clone(),
            available: self.available.clone(),
            confirm: self.confirm.clone(),
            row_click: self.row_click,
        }
    }
}

impl<T: 'static> RowAction<T> {
    /// An action that goes somewhere - an edit screen, a detail page.
    pub fn link(
        label: impl Into<String>,
        icon: Icon,
        href: impl Fn(&T) -> String + Send + Sync + 'static,
    ) -> Self {
        Self::of(label, icon, ActionKind::Link(Arc::new(href)))
    }

    /// An action that does something to the row.
    ///
    /// The closure receives the row and a [`GridHandle`]. Anything that reaches
    /// a service must be gated there as well: `require` hides the button, and
    /// `Caller::require` is what refuses the request.
    pub fn run(
        label: impl Into<String>,
        icon: Icon,
        on_run: impl Fn(T, GridHandle) + Send + Sync + 'static,
    ) -> Self {
        Self::of(
            label,
            icon,
            ActionKind::Run(Callback::new(move |(row, grid)| on_run(row, grid))),
        )
    }

    fn of(label: impl Into<String>, icon: Icon, kind: ActionKind<T>) -> Self {
        Self {
            label: label.into(),
            icon,
            tone: Tone::Neutral,
            permission: None,
            kind,
            available: None,
            confirm: None,
            row_click: false,
        }
    }

    /// The permission needed to see this action.
    ///
    /// Name a constant from [`phonix_core::authorization::names`], never a
    /// string literal - a typo in a literal hides the action from everybody and
    /// looks exactly like a permission nobody has been granted.
    #[must_use]
    pub const fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    /// How loud the action looks. [`Tone::Danger`] for the destructive one.
    #[must_use]
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Only offer this action on rows it applies to.
    ///
    /// "Reset two-factor" on an account that has none is an offer to do
    /// nothing; the row is a better place to decide that than the service is.
    #[must_use]
    pub fn when(mut self, available: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        self.available = Some(Arc::new(available));
        self
    }

    /// Ask before running. The text is the question.
    ///
    /// Only for actions that cannot be undone by repeating them.
    #[must_use]
    pub fn confirm(mut self, question: impl Into<String>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    /// Also do this when the row itself is clicked.
    ///
    /// For the one action on a list that is what somebody came to the list
    /// for - Open, on a log they are reading rather than administering. At
    /// most one action per grid may say it, and it has to be a
    /// [`link`](Self::link); see the module documentation for why the click is
    /// a shortcut to a menu entry rather than a setting of its own.
    ///
    /// Filtering applies unchanged. An action hidden by
    /// [`require`](Self::require) or ruled out by [`when`](Self::when) takes
    /// the row click with it, so a viewer who may not open a row cannot open
    /// one by clicking it.
    #[must_use]
    pub const fn on_row_click(mut self) -> Self {
        self.row_click = true;
        self
    }

    /// Whether a click on the row performs this action.
    pub const fn opens_on_row_click(&self) -> bool {
        self.row_click
    }

    /// Whether this viewer may see the action at all.
    pub fn permitted(&self, user: Option<&AuthUser>) -> bool {
        permitted(self.permission, user)
    }

    /// Whether this action applies to this row.
    pub fn applies_to(&self, row: &T) -> bool {
        self.available
            .as_ref()
            .is_none_or(|available| available(row))
    }
}

/// Something offered above the table, about the list rather than a row.
#[derive(Clone)]
pub struct ToolbarAction {
    pub(crate) label: String,
    pub(crate) icon: Icon,
    pub(crate) permission: Option<&'static str>,
    pub(crate) kind: ToolbarKind,
    /// Drawn as the screen's primary button rather than a quiet one.
    pub(crate) primary: bool,
}

#[derive(Clone)]
pub enum ToolbarKind {
    Link(String),
    Run(Callback<()>),
}

impl ToolbarAction {
    /// The usual "New ..." button: goes to a form.
    pub fn link(label: impl Into<String>, icon: Icon, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon,
            permission: None,
            kind: ToolbarKind::Link(href.into()),
            primary: false,
        }
    }

    /// A toolbar button that does something in place.
    pub fn run(
        label: impl Into<String>,
        icon: Icon,
        on_run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon,
            permission: None,
            kind: ToolbarKind::Run(Callback::new(move |()| on_run())),
            primary: false,
        }
    }

    /// The permission needed to see it. See [`RowAction::require`].
    #[must_use]
    pub const fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    /// Draw it as the screen's main action. At most one action should say this.
    #[must_use]
    pub const fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn permitted(&self, user: Option<&AuthUser>) -> bool {
        permitted(self.permission, user)
    }
}

/// Whether a viewer holds an optional permission.
///
/// Nobody is nobody: while the session is still resolving, `user` is `None` and
/// every gated action stays hidden. Showing them for the moment before the
/// answer arrives would make a page flicker with buttons the viewer may not
/// have - and would be the wrong way round to be wrong.
fn permitted(permission: Option<&'static str>, user: Option<&AuthUser>) -> bool {
    match permission {
        None => true,
        Some(permission) => user.is_some_and(|user| user.can(permission)),
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::PermissionSet;
    use phonix_core::identity::{UserId, UserStatus};

    use super::*;

    /// Run `build` inside a reactive owner.
    ///
    /// `RowAction::run` stores a `Callback`, and leptos allocates those in an
    /// arena belonging to the current owner. Components always have one; tests
    /// have to say so.
    fn owned<T>(build: impl FnOnce() -> T) -> T {
        Owner::new().with(build)
    }

    fn viewer(permissions: PermissionSet) -> AuthUser {
        AuthUser {
            id: UserId::nil(),
            email: "viewer@example.test".to_owned(),
            first_name: "V".to_owned(),
            last_name: "Iewer".to_owned(),
            display_name: "V Iewer".to_owned(),
            roles: Vec::new(),
            permissions,
            is_owner: false,
            status: UserStatus::Active,
            mfa_enabled: false,
            mfa_satisfied: true,
            email_verified: true,
        }
    }

    #[test]
    fn nothing_claims_the_row_click_unless_it_says_so() {
        let action = RowAction::<u8>::link("Open", Icon::Eye, |row| format!("/{row}"));

        assert!(!action.opens_on_row_click());
        assert!(action.on_row_click().opens_on_row_click());
    }

    #[test]
    fn a_row_click_is_gated_by_the_action_it_is_a_shortcut_to() {
        // The grid reads the destination out of the actions it has already
        // filtered, so this is the whole of the gating: an action a viewer
        // cannot see takes the row click away with it. A row click with a
        // permission of its own would be a second gate to keep in step, and
        // the day the two disagreed the quiet one would be the one that let
        // somebody through.
        let action = RowAction::<u8>::link("Open", Icon::Eye, |row| format!("/{row}"))
            .on_row_click()
            .require(phonix_core::permissions::USERS_EDIT);

        assert!(!action.permitted(Some(&viewer(PermissionSet::new()))));
        assert!(action.permitted(Some(&viewer(PermissionSet::all()))));
    }

    #[test]
    fn an_ungated_action_is_offered_to_everybody() {
        let action = RowAction::<u8>::link("Open", Icon::Eye, |row| format!("/{row}"));

        assert!(action.permitted(None));
    }

    #[test]
    fn a_gated_action_is_hidden_from_a_viewer_without_the_permission() {
        let action = RowAction::<u8>::link("Open", Icon::Eye, |row| format!("/{row}"))
            .require(phonix_core::permissions::USERS_EDIT);

        assert!(!action.permitted(Some(&viewer(PermissionSet::new()))));
        assert!(action.permitted(Some(&viewer(PermissionSet::all()))));
    }

    #[test]
    fn a_gated_action_stays_hidden_while_nobody_is_known_yet() {
        let action = ToolbarAction::link("New", Icon::Plus, "/new")
            .require(phonix_core::permissions::USERS_CREATE);

        assert!(!action.permitted(None));
    }

    #[test]
    fn a_row_action_can_decline_rows_it_means_nothing_for() {
        let action = owned(|| {
            RowAction::<u8>::run("Reset", Icon::ShieldOff, |_, _| {}).when(|row| *row > 3)
        });

        assert!(action.applies_to(&4));
        assert!(!action.applies_to(&1));
    }

    #[test]
    fn an_action_without_an_opinion_applies_to_every_row() {
        let action = owned(|| RowAction::<u8>::run("Reset", Icon::ShieldOff, |_, _| {}));

        assert!(action.applies_to(&0));
    }
}
