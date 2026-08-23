//! What a column is: an identifier, a heading, and one way to read a row.
//!
//! # One extractor, four jobs
//!
//! A column declares a single function from a row to a [`Cell`]. That one
//! function is what the grid searches, what it sorts by, what it exports, and -
//! unless the column overrides it - what it draws.
//!
//! The alternative is the usual one: a `render` for the screen, a `sort_key`
//! for ordering, a `search_text` for filtering, an `export` for CSV. Four
//! chances to describe the same column four slightly different ways, and the
//! bug that follows is always the same shape - a column that shows a formatted
//! date, sorts as a string, and puts "3 days ago" in the export.
//!
//! So [`Column::render`] is deliberately narrow: it changes how a value
//! *looks*, never what it *is*. A status column may draw a coloured badge, and
//! it still sorts and exports as the word inside the badge.
//!
//! # Declaring one
//!
//! ```ignore
//! Column::new("last_login_at", "Last sign-in", |u: &UserListing| {
//!     u.last_login_at.map_or(Cell::Empty, Cell::timestamp)
//! })
//! .sortable()
//! .align(Align::End)
//! ```
//!
//! `field` is a stable identifier, not a heading: it keys the sort, the column
//! toggle and - for a server-side source - the `ORDER BY`. Renaming the heading
//! is a wording change; renaming the field is a contract change.

use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use leptos::prelude::*;

/// A value read out of a row, in the shape that says how to compare it.
///
/// Typed rather than stringly so that sorting is right: `9` before `10`, and
/// last March before this January, neither of which survives being compared as
/// text.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// Nothing to show. Sorts before every value and exports as blank.
    Empty,
    Text(String),
    Number(f64),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    /// Several short values - roles, tags, labels.
    List(Vec<String>),
}

impl Cell {
    pub fn text(value: impl Into<String>) -> Self {
        let value: String = value.into();

        if value.is_empty() {
            Self::Empty
        } else {
            Self::Text(value)
        }
    }

    pub fn number(value: impl Into<f64>) -> Self {
        Self::Number(value.into())
    }

    pub const fn bool(value: bool) -> Self {
        Self::Bool(value)
    }

    pub const fn timestamp(at: DateTime<Utc>) -> Self {
        Self::Timestamp(at)
    }

    pub fn list(values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let values: Vec<String> = values.into_iter().map(Into::into).collect();

        if values.is_empty() {
            Self::Empty
        } else {
            Self::List(values)
        }
    }

    /// `Empty` when `None`, so an absent value never renders as "None" by
    /// accident.
    pub fn maybe(value: Option<impl Into<String>>) -> Self {
        value.map_or(Self::Empty, Self::text)
    }

    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// The value as one line of text: what the cell shows when the column has
    /// no renderer, what the export writes, and what a search looks inside.
    ///
    /// The date format is fixed and sortable rather than friendly. A grid is
    /// scanned down a column, where `2026-03-04 09:15` lines up and
    /// "4 March 2026, 9:15 am" does not.
    pub fn to_text(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(value) => value.clone(),
            Self::Number(value) => format_number(*value),
            Self::Bool(true) => "Yes".to_owned(),
            Self::Bool(false) => "No".to_owned(),
            Self::Timestamp(at) => at.format("%Y-%m-%d %H:%M").to_string(),
            Self::List(values) => values.join(", "),
        }
    }

    /// Whether this cell contains `needle`, which is already lowercased.
    pub fn contains(&self, needle: &str) -> bool {
        self.to_text().to_lowercase().contains(needle)
    }

    /// Ascending order within a column.
    ///
    /// `Empty` sorts first, so "never signed in" collects at one end rather
    /// than being scattered by whatever its text happens to be. Mixed variants
    /// in one column would be a mistake in the configuration; they fall back to
    /// comparing text so that a mistake still produces a stable order.
    pub fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Empty, Self::Empty) => Ordering::Equal,
            (Self::Empty, _) => Ordering::Less,
            (_, Self::Empty) => Ordering::Greater,
            (Self::Number(a), Self::Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Self::Timestamp(a), Self::Timestamp(b)) => a.cmp(b),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            // Case-insensitive, because a column sorted A, B, a, b reads as
            // broken to everyone who is not a computer.
            (a, b) => a.to_text().to_lowercase().cmp(&b.to_text().to_lowercase()),
        }
    }
}

/// Trailing zeroes dropped: `4`, not `4.0000000`.
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.4}");

        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// Which edge of its cell a column's content sits against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

impl Align {
    pub const fn cell_class(self) -> &'static str {
        match self {
            Self::Start => "text-left",
            Self::Center => "text-center",
            Self::End => "text-right",
        }
    }
}

/// How a row is read for one column.
type Read<T> = Arc<dyn Fn(&T) -> Cell + Send + Sync>;

/// How a cell is drawn, when plain text will not do.
type Draw<T> = Arc<dyn Fn(&T) -> AnyView + Send + Sync>;

/// One column of a [`DataGrid`](super::DataGrid).
///
/// Cheap to clone: the two closures are behind `Arc`, so a configuration can be
/// handed to the grid, the toolbar and the export without being rebuilt.
pub struct Column<T: 'static> {
    pub(crate) field: &'static str,
    /// What the column is called. A `String`, unlike `field`: the field name
    /// is machinery that goes in a sort parameter, the header is a word.
    pub(crate) header: String,
    pub(crate) searchable: bool,
    pub(crate) sortable: bool,
    pub(crate) hideable: bool,
    /// Off until someone turns it on in the column menu. For the detail a few
    /// people need and everyone else would have to scroll past.
    pub(crate) hidden_by_default: bool,
    /// Whether this column survives a phone-width screen.
    ///
    /// Distinct from [`hideable`](Self::hideable), which is about the column
    /// menu, and from [`hidden_by_default`](Self::hidden_by_default), which is
    /// about the viewer's choice. This one is about the screen: below `sm`
    /// there is room for two or three columns, and a table that insists on
    /// seven does not become readable by being scrollable.
    pub(crate) essential: bool,
    pub(crate) align: Align,
    /// Extra classes for this column's cells - a width, a whitespace rule.
    pub(crate) class: &'static str,
    pub(crate) read: Read<T>,
    pub(crate) draw: Option<Draw<T>>,
}

impl<T: 'static> Clone for Column<T> {
    fn clone(&self) -> Self {
        Self {
            field: self.field,
            header: self.header.clone(),
            searchable: self.searchable,
            sortable: self.sortable,
            hideable: self.hideable,
            hidden_by_default: self.hidden_by_default,
            essential: self.essential,
            align: self.align,
            class: self.class,
            read: Arc::clone(&self.read),
            draw: self.draw.clone(),
        }
    }
}

impl<T: 'static> Column<T> {
    /// A column that shows the value it reads, hideable, neither searchable
    /// nor sortable until it says so.
    pub fn new(
        field: &'static str,
        header: impl Into<String>,
        read: impl Fn(&T) -> Cell + Send + Sync + 'static,
    ) -> Self {
        Self {
            field,
            header: header.into(),
            searchable: false,
            sortable: false,
            hideable: true,
            essential: false,
            hidden_by_default: false,
            align: Align::Start,
            class: "",
            read: Arc::new(read),
            draw: None,
        }
    }

    /// The search box looks in this column.
    #[must_use]
    pub const fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }

    /// The heading can be clicked to sort by this column.
    #[must_use]
    pub const fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }

    /// searchable and sortable.
    #[must_use]
    pub const fn findable(self) -> Self {
        self.searchable().sortable()
    }

    /// Always on screen: not offered in the column menu.
    ///
    /// For the column that says which row this is. A table whose every column
    /// can be hidden can be turned into a grid of anonymous buttons.
    #[must_use]
    pub const fn pinned(mut self) -> Self {
        self.hideable = false;
        self
    }

    /// Present, but off until asked for.
    #[must_use]
    pub const fn hidden(mut self) -> Self {
        self.hidden_by_default = true;
        self
    }

    /// Kept on a phone, where most columns are dropped.
    ///
    /// Below `sm` only essential columns are drawn. Mark the one that says
    /// which row this is, and the one or two facts someone came to the screen
    /// to check - not more: three columns is what 390 pixels holds before the
    /// table starts scrolling sideways and taking the page with it.
    ///
    /// Nothing is lost by not being essential. The column is still exported,
    /// still searched, still sorted, and reappears the moment the screen is
    /// wide enough.
    #[must_use]
    pub const fn essential(mut self) -> Self {
        self.essential = true;
        self
    }

    #[must_use]
    pub const fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Extra Tailwind classes for this column's cells - typically a width.
    #[must_use]
    pub const fn class(mut self, class: &'static str) -> Self {
        self.class = class;
        self
    }

    /// Draw the cell as something other than its text.
    ///
    /// Changes appearance only. Searching, sorting and export still read the
    /// [`Cell`], which is what keeps a badge that says "Active" sorting under
    /// A.
    #[must_use]
    pub fn render(mut self, draw: impl Fn(&T) -> AnyView + Send + Sync + 'static) -> Self {
        self.draw = Some(Arc::new(draw));
        self
    }

    pub fn field(&self) -> &'static str {
        self.field
    }

    pub fn header(&self) -> &str {
        &self.header
    }

    /// What hides this column below `sm`, if anything.
    ///
    /// Done with a class rather than by leaving the cell out, so the server and
    /// the browser render the same table however wide the window is. A table
    /// whose *shape* depends on a media query is a table that cannot be
    /// hydrated: the browser would meet a row with fewer cells than the one it
    /// was sent.
    pub const fn responsive_class(&self) -> &'static str {
        if self.essential {
            ""
        } else {
            "hidden sm:table-cell"
        }
    }

    /// This column's value for one row.
    pub fn value(&self, row: &T) -> Cell {
        (self.read)(row)
    }

    /// This column's cell for one row, drawn.
    pub fn view(&self, row: &T) -> AnyView {
        match &self.draw {
            Some(draw) => draw(row),
            None => {
                let text = self.value(row).to_text();

                if text.is_empty() {
                    // An em dash rather than nothing: an empty cell and a cell
                    // whose value failed to load look identical otherwise.
                    view! { <span class="text-content-subtle">"—"</span> }.into_any()
                } else {
                    text.into_any()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_sort_as_numbers_not_as_text() {
        assert_eq!(Cell::number(9).compare(&Cell::number(10)), Ordering::Less);
        assert_eq!(
            Cell::text("9").compare(&Cell::text("10")),
            Ordering::Greater
        );
    }

    #[test]
    fn nothing_sorts_before_something() {
        assert_eq!(Cell::Empty.compare(&Cell::text("a")), Ordering::Less);
        assert_eq!(Cell::text("a").compare(&Cell::Empty), Ordering::Greater);
    }

    #[test]
    fn text_sorts_without_regard_to_case() {
        assert_eq!(
            Cell::text("apple").compare(&Cell::text("Banana")),
            Ordering::Less
        );
    }

    #[test]
    fn an_empty_string_is_an_empty_cell() {
        assert!(Cell::text("").is_empty());
        assert!(Cell::list(Vec::<String>::new()).is_empty());
        assert!(Cell::maybe(None::<String>).is_empty());
    }

    #[test]
    fn a_list_reads_and_searches_as_its_members() {
        let cell = Cell::list(["Admin", "Buyer"]);

        assert_eq!(cell.to_text(), "Admin, Buyer");
        assert!(cell.contains("buyer"));
    }

    #[test]
    fn a_flag_reads_as_a_word_so_the_export_says_something() {
        assert_eq!(Cell::bool(true).to_text(), "Yes");
        assert_eq!(Cell::bool(false).to_text(), "No");
    }

    #[test]
    fn whole_numbers_do_not_grow_a_decimal_point() {
        assert_eq!(Cell::number(4).to_text(), "4");
        assert_eq!(Cell::number(4.5).to_text(), "4.5");
    }

    #[test]
    fn a_renderer_changes_the_look_and_not_the_value() {
        let column = Column::new("status", "Status", |row: &&str| Cell::text(*row))
            .render(|_| ().into_any());

        assert_eq!(column.value(&"Active"), Cell::text("Active"));
    }
}
