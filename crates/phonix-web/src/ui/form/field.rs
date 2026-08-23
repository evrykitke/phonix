//! What a field is: an identifier, a label, a control, and two closures.
//!
//! # One field, one pair of closures
//!
//! A column declares one function, row to [`Cell`]. A field declares two,
//! because a form reads *and* writes:
//!
//! ```ignore
//! Field::text("first_name", "First name", |u: &UserEdit| FieldValue::text(&u.first_name))
//!     .writing(|u, value| u.first_name = value.as_input())
//!     .required()
//! ```
//!
//! The conversion in both directions belongs to the field, which is what lets
//! the draft stay a typed struct while the control between the person and it
//! only ever handles strings and booleans.
//!
//! # `field` is the same identifier the grid uses
//!
//! Deliberately. A [`FieldError`] returned by the server names a field, and the
//! form places the message by matching that name - so the service, the form and
//! the column all have to agree on one spelling. Sharing the identifier is also
//! what lets a test assert that every field of a form names a real column of
//! the entity, which is the cheap way to catch a rename that only went halfway.
//!
//! # A field the viewer may not edit is shown, not hidden
//!
//! [`Field::require`] makes a field read-only rather than removing it. This is
//! the opposite of the rule for actions, and for a specific reason: a hidden
//! action is a button nobody presses, while a hidden *field* still gets
//! submitted (as whatever the draft was initialised with, or as nothing at
//! all) and quietly overwrites a value the viewer was not allowed to see.
//! Showing it disabled says "this exists, and it is not yours to change".
//!
//! [`Cell`]: crate::ui::table::Cell
//! [`FieldError`]: phonix_core::identity::validation::FieldError

use std::sync::Arc;

use phonix_core::identity::AuthUser;

use super::kind::FieldKind;
use super::value::FieldValue;

/// How a draft is read for one field.
type Read<T> = Arc<dyn Fn(&T) -> FieldValue + Send + Sync>;

/// How one field is written back into a draft.
type Write<T> = Arc<dyn Fn(&mut T, &FieldValue) + Send + Sync>;

/// Whether a field applies to this particular draft.
type Applies<T> = Arc<dyn Fn(&T) -> bool + Send + Sync>;

/// One control of a form.
pub struct Field<T: 'static> {
    pub(crate) field: &'static str,
    pub(crate) label: String,
    pub(crate) kind: FieldKind,
    /// A line under the control. For the rule that is not obvious from the
    /// label - "they sign in with this", "leave empty for no limit".
    pub(crate) help: Option<String>,
    pub(crate) placeholder: Option<String>,
    pub(crate) required: bool,
    /// Read-only whatever the viewer holds - a generated code, an email that
    /// cannot change once an account exists.
    pub(crate) fixed: bool,
    /// The permission needed to *edit* it. Without it the control is shown and
    /// disabled; see the note in the module docs.
    pub(crate) permission: Option<&'static str>,
    /// Takes the full width of the form rather than one column.
    pub(crate) wide: bool,
    pub(crate) available: Option<Applies<T>>,
    pub(crate) read: Read<T>,
    pub(crate) write: Option<Write<T>>,
}

impl<T: 'static> Clone for Field<T> {
    fn clone(&self) -> Self {
        Self {
            field: self.field,
            label: self.label.clone(),
            kind: self.kind.clone(),
            help: self.help.clone(),
            placeholder: self.placeholder.clone(),
            required: self.required,
            fixed: self.fixed,
            permission: self.permission,
            wide: self.wide,
            available: self.available.clone(),
            read: Arc::clone(&self.read),
            write: self.write.clone(),
        }
    }
}

impl<T: 'static> Field<T> {
    /// A field of any kind. Prefer the named constructors below.
    pub fn new(
        field: &'static str,
        label: impl Into<String>,
        kind: FieldKind,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self {
            field,
            label: label.into(),
            kind,
            help: None,
            placeholder: None,
            required: false,
            fixed: false,
            permission: None,
            wide: false,
            available: None,
            read: Arc::new(read),
            write: None,
        }
    }

    /// A single line of text.
    pub fn text(
        field: &'static str,
        label: impl Into<String>,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(field, label, FieldKind::Text, read)
    }

    /// An email address. The browser validates the shape; the server decides.
    pub fn email(
        field: &'static str,
        label: impl Into<String>,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(field, label, FieldKind::Email, read)
    }

    /// Several lines of text.
    pub fn multiline(
        field: &'static str,
        label: impl Into<String>,
        rows: u8,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(field, label, FieldKind::Multiline { rows }, read).full_width()
    }

    /// A number.
    pub fn number(
        field: &'static str,
        label: impl Into<String>,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(
            field,
            label,
            FieldKind::Number {
                min: None,
                max: None,
                step: None,
            },
            read,
        )
    }

    /// A yes or no.
    pub fn toggle(
        field: &'static str,
        label: impl Into<String>,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(field, label, FieldKind::Toggle, read)
    }

    /// One of a fixed set.
    ///
    /// The choices are owned rather than `&'static`, because a configuration is
    /// built per render and the interesting sets - roles, warehouses, suppliers
    /// - are fetched. A screen that has them passes them in.
    pub fn select(
        field: &'static str,
        label: impl Into<String>,
        choices: Vec<Choice>,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(field, label, FieldKind::Select { choices }, read)
    }

    /// Any number of a fixed set.
    pub fn multi_select(
        field: &'static str,
        label: impl Into<String>,
        choices: Vec<Choice>,
        read: impl Fn(&T) -> FieldValue + Send + Sync + 'static,
    ) -> Self {
        Self::new(field, label, FieldKind::MultiSelect { choices }, read).full_width()
    }

    /// How this field is written back into the draft.
    ///
    /// A field without one is read-only: it renders, it cannot be typed into,
    /// and nothing it holds can reach the draft. That is a legitimate field -
    /// an id, a created-at - and it is also what an unwritable field degrades
    /// to rather than silently discarding what somebody typed.
    #[must_use]
    pub fn writing(mut self, write: impl Fn(&mut T, &FieldValue) + Send + Sync + 'static) -> Self {
        self.write = Some(Arc::new(write));
        self
    }

    /// Must be filled in. Checked in the browser as a courtesy and by the
    /// service as the control - see [`FormConfig`](super::FormConfig).
    #[must_use]
    pub const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// A line of explanation under the control.
    #[must_use]
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Never editable, by anybody.
    #[must_use]
    pub const fn fixed(mut self) -> Self {
        self.fixed = true;
        self
    }

    /// Editable only by a viewer holding this permission. Everyone else sees it
    /// disabled rather than not at all.
    #[must_use]
    pub const fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    /// Takes the whole width of the form.
    #[must_use]
    pub const fn full_width(mut self) -> Self {
        self.wide = true;
        self
    }

    /// Only show this field for drafts it means something for.
    ///
    /// Unlike a permission, this one *does* remove the field - because it is
    /// about the entity rather than the viewer, and a field that does not apply
    /// has nothing to overwrite.
    #[must_use]
    pub fn when(mut self, available: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        self.available = Some(Arc::new(available));
        self
    }

    pub const fn name(&self) -> &'static str {
        self.field
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn is_required(&self) -> bool {
        self.required
    }

    /// The permission needed to edit it, if it is gated at all.
    pub const fn permission(&self) -> Option<&'static str> {
        self.permission
    }

    pub fn value(&self, draft: &T) -> FieldValue {
        (self.read)(draft)
    }

    /// Write `value` into `draft`, if this field can be written at all.
    pub fn apply(&self, draft: &mut T, value: &FieldValue) {
        if let Some(write) = &self.write {
            write(draft, value);
        }
    }

    pub fn applies_to(&self, draft: &T) -> bool {
        self.available
            .as_ref()
            .is_none_or(|available| available(draft))
    }

    /// Whether this viewer may change it.
    ///
    /// Nobody is nobody: while the session is still resolving, `user` is `None`
    /// and a gated field stays locked. Enabling it for the moment before the
    /// answer arrives would be the wrong way round to be wrong.
    pub fn editable_by(&self, user: Option<&AuthUser>) -> bool {
        if self.fixed || self.write.is_none() {
            return false;
        }

        match self.permission {
            None => true,
            Some(permission) => user.is_some_and(|user| user.can(permission)),
        }
    }
}

/// One option of a select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    /// A line under the label, for a set whose members need explaining - what a
    /// role grants, what a status means.
    pub detail: Option<String>,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::PermissionSet;
    use phonix_core::identity::{UserId, UserStatus};

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Draft {
        name: String,
        active: bool,
    }

    fn name_field() -> Field<Draft> {
        Field::text("name", "Name", |d: &Draft| FieldValue::text(&d.name))
            .writing(|d, value| d.name = value.as_input())
            .required()
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
            mfa_satisfied: true,
            mfa_enabled: false,
            email_verified: true,
        }
    }

    #[test]
    fn a_field_reads_and_writes_the_same_place() {
        let field = name_field();
        let mut draft = Draft {
            name: "Ada".into(),
            active: true,
        };

        assert_eq!(field.value(&draft), FieldValue::text("Ada"));

        field.apply(&mut draft, &FieldValue::text("Grace"));

        assert_eq!(draft.name, "Grace");
    }

    #[test]
    fn a_field_with_no_writer_is_read_only_and_discards_nothing_quietly() {
        // It cannot be typed into in the first place, which is the point: the
        // alternative is a control that accepts input and drops it.
        let field = Field::text("name", "Name", |d: &Draft| FieldValue::text(&d.name));
        let mut draft = Draft {
            name: "Ada".into(),
            active: true,
        };

        field.apply(&mut draft, &FieldValue::text("Grace"));

        assert_eq!(draft.name, "Ada");
        assert!(!field.editable_by(None));
    }

    #[test]
    fn a_gated_field_is_locked_for_a_viewer_without_the_permission() {
        let field = name_field().require(phonix_core::permissions::USERS_EDIT);

        assert!(!field.editable_by(Some(&viewer(PermissionSet::new()))));
        assert!(field.editable_by(Some(&viewer(PermissionSet::all()))));
    }

    #[test]
    fn a_gated_field_stays_locked_while_nobody_is_known_yet() {
        let field = name_field().require(phonix_core::permissions::USERS_EDIT);

        assert!(!field.editable_by(None));
    }

    #[test]
    fn an_ungated_field_is_editable_by_anybody_who_can_open_the_form() {
        assert!(name_field().editable_by(None));
    }

    #[test]
    fn a_fixed_field_is_locked_for_everybody_including_the_owner() {
        let field = name_field().fixed();

        assert!(!field.editable_by(Some(&viewer(PermissionSet::all()))));
    }

    #[test]
    fn a_field_can_decline_drafts_it_means_nothing_for() {
        let field = Field::toggle("active", "Active", |d: &Draft| FieldValue::Bool(d.active))
            .when(|d: &Draft| !d.name.is_empty());

        assert!(field.applies_to(&Draft {
            name: "Ada".into(),
            active: true
        }));
        assert!(!field.applies_to(&Draft {
            name: String::new(),
            active: true
        }));
    }
}
