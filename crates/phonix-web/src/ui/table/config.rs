//! What a grid is told, and what each entity tells it.
//!
//! # The shape of a configuration
//!
//! [`GridConfig`] is the whole contract between a module and the grid. It is
//! built with a chain of `with`-style methods so that a configuration reads
//! down the page as a description of the screen:
//!
//! ```ignore
//! GridConfig::new("users", Source::in_memory(list_users))
//!     .searching("Filter by name, email or role")
//!     .exports_as("users")
//!     .column(Column::new("display_name", "Person", ...).findable().pinned())
//!     .action(RowAction::link("Permissions", Icon::KeySquare, ...)
//!         .require(names::USERS_CHANGE_PERMISSIONS))
//! ```
//!
//! Nothing here is a component. A module contributes a *value*, and the one
//! [`DataGrid`](super::DataGrid) renders it - which is what "each entity
//! extends the grid" means in a language without inheritance: the extension
//! point is the configuration, not a subclass.
//!
//! # Entity configurations live under this module
//!
//! One file per entity, named for it, exporting one function:
//!
//! ```text
//! ui/table/config/users.rs   ->  pub fn users_grid() -> GridConfig<UserListing>
//! ```
//!
//! Keeping them together rather than beside their pages is deliberate. A
//! configuration is a description of data - which fields exist, which are
//! searchable, what may be done to a row - and having them in one place is what
//! makes it obvious when two modules describe the same thing two ways.

pub mod audit;
pub mod changes;
pub mod currencies;
pub mod invoices;
pub mod numbering;
pub mod parties;
pub mod roles;
pub mod taxes;
pub mod users;

use leptos::prelude::Callback;
use phonix_core::query::{PageRequest, Sort};

use super::action::{ActionKind, RowAction, ToolbarAction};
use super::column::Column;
use super::date::DateFilter;
use super::filter::Filter;
use super::source::Source;
use crate::icons::Icon;

/// The page sizes offered when a configuration does not say.
pub const PER_PAGE_CHOICES: &[u32] = &[10, 25, 50, 100];

/// How many rows a picker holds.
///
/// A picker has no pager - see [`GridConfig::choosing`] - so this is the whole
/// list there is to scroll, and the grid says so under the last row when there
/// are more. High enough that most lists are entirely there; low enough that a
/// popover over a half-filled form never becomes a page in its own right.
pub const PICKER_ROWS: u32 = 50;

/// How many rows a page holds, and what else may be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pagination {
    pub default: u32,
    pub choices: &'static [u32],
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            default: 25,
            choices: PER_PAGE_CHOICES,
        }
    }
}

impl Pagination {
    /// A fixed set of page sizes, the first of which is the default.
    ///
    /// Offering a page size the grid then has to clamp would be a control that
    /// lies, so the choices are also the only sizes the grid will use.
    pub const fn of(choices: &'static [u32]) -> Self {
        let default = match choices.first() {
            Some(first) => *first,
            None => 25,
        };

        Self { default, choices }
    }

    /// Start at a size other than the first choice.
    #[must_use]
    pub const fn starting_at(mut self, default: u32) -> Self {
        self.default = default;
        self
    }
}

/// What to show when there is nothing to show.
#[derive(Debug, Clone)]
pub struct Empty {
    pub icon: Icon,
    pub title: String,
    pub detail: String,
}

/// Everything one grid needs to know.
///
/// Cheap to clone - every closure inside is behind an `Arc` - so a screen can
/// build one per render without thinking about it.
pub struct GridConfig<T: 'static> {
    /// A stable name for this grid, used for the ids that tie controls to the
    /// table for a screen reader.
    pub(crate) id: &'static str,
    pub(crate) columns: Vec<Column<T>>,
    pub(crate) filters: Vec<Filter<T>>,
    /// Spans of time offered above the table. Separate from `filters` because
    /// a range is not one of a fixed set of choices - see [`DateFilter`].
    pub(crate) date_filters: Vec<DateFilter<T>>,
    pub(crate) actions: Vec<RowAction<T>>,
    pub(crate) toolbar: Vec<ToolbarAction>,
    pub(crate) pagination: Pagination,
    pub(crate) source: Source<T>,
    /// `None` hides the search box - for a list with nothing worth searching.
    pub(crate) search_placeholder: Option<String>,
    /// `Some` puts an export button in the toolbar; the value is the file stem.
    pub(crate) export_stem: Option<&'static str>,
    pub(crate) empty: Empty,
    /// Shown when a search matched nothing, as opposed to there being nothing.
    pub(crate) no_matches: Empty,
    pub(crate) initial_sort: Option<Sort>,
    /// The smallest width the table is allowed to squeeze into before it
    /// scrolls sideways inside its own box. `sm:`-prefixed - see
    /// [`GridConfig::min_width`].
    pub(crate) min_width: &'static str,
    /// Set when this grid is a picker rather than a list - see
    /// [`GridConfig::choosing`].
    pub(crate) choosing: Option<Callback<T>>,
}

impl<T: 'static> Clone for GridConfig<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            columns: self.columns.clone(),
            filters: self.filters.clone(),
            date_filters: self.date_filters.clone(),
            actions: self.actions.clone(),
            toolbar: self.toolbar.clone(),
            pagination: self.pagination.clone(),
            source: self.source.clone(),
            search_placeholder: self.search_placeholder.clone(),
            export_stem: self.export_stem,
            empty: self.empty.clone(),
            no_matches: self.no_matches.clone(),
            initial_sort: self.initial_sort.clone(),
            min_width: self.min_width,
            choosing: self.choosing,
        }
    }
}

impl<T: 'static> GridConfig<T> {
    /// A grid with no columns yet, reading from `source`.
    pub fn new(id: &'static str, source: Source<T>) -> Self {
        Self {
            id,
            columns: Vec::new(),
            filters: Vec::new(),
            date_filters: Vec::new(),
            actions: Vec::new(),
            toolbar: Vec::new(),
            pagination: Pagination::default(),
            source,
            search_placeholder: None,
            export_stem: None,
            initial_sort: None,
            // The kit's own fallbacks, for a grid that never said. Every
            // real grid replaces both.
            empty: Empty {
                icon: Icon::ListTree,
                title: crate::l!("grid.empty.title"),
                detail: crate::l!("grid.empty.detail"),
            },
            no_matches: Empty {
                icon: Icon::Search,
                title: crate::l!("grid.no_matches.title"),
                detail: crate::l!("grid.no_matches.detail"),
            },
            min_width: "sm:min-w-[44rem]",
            choosing: None,
        }
    }

    /// Add a column. Order here is order on screen.
    #[must_use]
    pub fn column(mut self, column: Column<T>) -> Self {
        self.columns.push(column);
        self
    }

    /// Add a named narrowing, shown as a dropdown beside the search box.
    ///
    /// A filter on an in-memory grid must say how to answer itself - see
    /// [`Filter::matching`]. One that does not would draw a control that
    /// changes nothing, which is why this refuses it in debug builds rather
    /// than leaving it to be noticed on screen.
    #[must_use]
    pub fn filter(mut self, filter: Filter<T>) -> Self {
        debug_assert!(
            !self.source.is_in_memory() || filter.is_local(),
            "the `{}` filter has no `matching`, and an in-memory grid has no server to ask",
            filter.key(),
        );

        self.filters.push(filter);
        self
    }

    /// Add a span of time, shown as a calendar beside the search box.
    ///
    /// A range on an in-memory grid must say which instant it is about - see
    /// [`DateFilter::at`]. One that does not would draw a calendar that changes
    /// nothing, so this refuses it in debug builds rather than leaving it to be
    /// noticed on screen.
    #[must_use]
    pub fn date_filter(mut self, filter: DateFilter<T>) -> Self {
        debug_assert!(
            !self.source.is_in_memory() || filter.is_local(),
            "the `{}` range has no `at`, and an in-memory grid has no server to ask",
            filter.key(),
        );

        self.date_filters.push(filter);
        self
    }

    /// Add something that can be done to a row.
    ///
    /// At most one may say [`on_row_click`](RowAction::on_row_click), and it
    /// has to be a link. Both are refused in debug builds rather than left to
    /// be discovered: two of them means a row click that does whichever was
    /// declared first, and a `Run` means a row that changes data when somebody
    /// clicks it to read it.
    #[must_use]
    pub fn action(mut self, action: RowAction<T>) -> Self {
        debug_assert!(
            self.choosing.is_none(),
            "`{}` was added to a picker, and a picker draws no row menu",
            action.label,
        );
        debug_assert!(
            !action.opens_on_row_click() || matches!(action.kind, ActionKind::Link(_)),
            "`{}` is what a row click does, so it has to go somewhere - use `RowAction::link`",
            action.label,
        );
        debug_assert!(
            !action.opens_on_row_click()
                || !self.actions.iter().any(RowAction::opens_on_row_click),
            "`{}` is the second action on this grid to claim the row click, and a click \
             can only do one thing",
            action.label,
        );

        self.actions.push(action);
        self
    }

    /// Add something that can be done to the list.
    #[must_use]
    pub fn toolbar(mut self, action: ToolbarAction) -> Self {
        self.toolbar.push(action);
        self
    }

    /// Show a search box with this placeholder.
    ///
    /// The placeholder says what is searched, because only the columns marked
    /// [`searchable`](Column::searchable) are - and a box that silently ignores
    /// the field someone is typing is worse than no box.
    #[must_use]
    pub fn searching(mut self, placeholder: impl Into<String>) -> Self {
        self.search_placeholder = Some(placeholder.into());
        self
    }

    /// Offer an export. `stem` becomes the start of the file name.
    #[must_use]
    pub const fn exports_as(mut self, stem: &'static str) -> Self {
        self.export_stem = Some(stem);
        self
    }

    /// How many rows a page holds, and what else may be chosen.
    ///
    /// Not for a picker, which has neither - see [`choosing`](Self::choosing).
    #[must_use]
    pub fn paginated(mut self, pagination: Pagination) -> Self {
        debug_assert!(
            !self.is_picker(),
            "a picker draws no pager, so this page size would be a control nobody is offered",
        );

        self.pagination = pagination;
        self
    }

    /// What an empty table says.
    #[must_use]
    pub fn empty(
        mut self,
        icon: Icon,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        self.empty = Empty {
            icon,
            title: title.into(),
            detail: detail.into(),
        };
        self
    }

    /// The order the table opens in.
    ///
    /// Name a column that is [`sortable`](Column::sortable); a sort naming
    /// anything else is ignored, which shows up as a table that opens in
    /// whatever order the source returned.
    #[must_use]
    pub fn sorted_by(mut self, sort: Sort) -> Self {
        self.initial_sort = Some(sort);
        self
    }

    /// Make this grid a picker: every row is a choice, and clicking one
    /// answers with it.
    ///
    /// This is how a lookup shows a table instead of a list - see
    /// [`ui::lookup`](crate::ui::lookup). The point of doing it here rather
    /// than writing a second table is that everything an entity's list already
    /// knows how to do comes with it: the same columns, the same search, the
    /// same paging and sorting, the same one description of what the entity is.
    ///
    /// A picker has no row menu and takes no [`action`](Self::action). Inside a
    /// popover over a half-filled form there is nothing to navigate to and
    /// nothing worth doing to a row, and a menu of verbs there would be a way
    /// to leave the form by accident.
    ///
    /// It loses the rest of the list screen's furniture too, and for the same
    /// reason: a picker is a simple table. No export - the rows are somebody
    /// else's field, not a report. No column menu - a panel is not where
    /// anybody arranges a table. No pager - it shows [`PICKER_ROWS`] and says
    /// so, because the way to reach the six hundredth currency is to type its
    /// name, not to walk twelve pages of a popover looking for it. The search
    /// box stays, and it is the whole navigation.
    #[must_use]
    pub fn choosing(mut self, on_choose: Callback<T>) -> Self {
        debug_assert!(
            self.actions.is_empty(),
            "a picker has no row menu, so the actions on it would never be drawn",
        );
        debug_assert!(
            self.pagination == Pagination::default(),
            "a picker draws no pager, so this page size would be a control nobody is offered",
        );

        self.choosing = Some(on_choose);
        self
    }

    /// Whether this grid is a picker.
    pub const fn is_picker(&self) -> bool {
        self.choosing.is_some()
    }

    /// How narrow the table may get before it scrolls inside its own box.
    ///
    /// A Tailwind `min-w-*` class, and it must be `sm:`-prefixed. Below `sm`
    /// the table has to fit the screen: a mobile browser widens its layout
    /// viewport to hold the widest thing in the document, and everything
    /// positioned `fixed` - modals, drawers, their backdrops - is then measured
    /// against that wider box and lands off-screen. Which columns survive down
    /// there is [`Column::essential`](super::Column::essential)'s business, not
    /// this one's.
    #[must_use]
    pub const fn min_width(mut self, class: &'static str) -> Self {
        self.min_width = class;
        self
    }

    /// The request this grid starts from.
    pub fn initial_request(&self) -> PageRequest {
        PageRequest {
            page: 1,
            // A picker's page is the whole list it will ever show, so the size
            // is not the configuration's business - see `choosing`.
            per_page: if self.is_picker() {
                PICKER_ROWS
            } else {
                self.pagination.default
            },
            search: String::new(),
            sort: self.initial_sort.clone(),
            // Every filter opens on its first choice, which is "everything" -
            // so an opening request carries none of them.
            filters: std::collections::BTreeMap::new(),
        }
    }

    /// The fields of the columns that are off until asked for.
    pub fn hidden_by_default(&self) -> Vec<&'static str> {
        self.columns
            .iter()
            .filter(|column| column.hidden_by_default)
            .map(Column::field)
            .collect()
    }

    /// Whether any column can be turned off at all - and so whether the column
    /// menu is worth offering.
    pub fn has_hideable_columns(&self) -> bool {
        self.columns.iter().any(|column| column.hideable)
    }
}

#[cfg(test)]
mod tests {
    use super::super::column::Cell;
    use super::*;

    fn config() -> GridConfig<u8> {
        GridConfig::new(
            "test",
            Source::in_memory(|| async { Ok::<_, String>(Vec::<u8>::new()) }),
        )
        .column(Column::new("a", "A", |r: &u8| Cell::number(*r)).pinned())
        .column(Column::new("b", "B", |r: &u8| Cell::number(*r)).hidden())
    }

    #[test]
    fn a_grid_starts_on_the_first_page_at_the_default_size() {
        let request = config().initial_request();

        assert_eq!(request.page, 1);
        assert_eq!(request.per_page, 25);
        assert!(request.sort.is_none());
    }

    #[test]
    fn the_columns_that_start_hidden_are_the_ones_that_said_so() {
        assert_eq!(config().hidden_by_default(), ["b"]);
    }

    #[test]
    fn a_pinned_column_does_not_make_the_column_menu_appear() {
        let only_pinned = GridConfig::new(
            "test",
            Source::in_memory(|| async { Ok::<_, String>(Vec::<u8>::new()) }),
        )
        .column(Column::new("a", "A", |r: &u8| Cell::number(*r)).pinned());

        assert!(!only_pinned.has_hideable_columns());
        assert!(config().has_hideable_columns());
    }

    #[test]
    fn the_first_page_size_offered_is_the_one_it_opens_at() {
        let pagination = Pagination::of(&[10, 50, 100]);

        assert_eq!(pagination.default, 10);
        assert_eq!(pagination.starting_at(50).default, 50);
    }
}
