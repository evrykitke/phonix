//! Searching, sorting and paging a list that is already in the browser.
//!
//! # Why this is a plain function
//!
//! Everything here is arithmetic and comparison over a slice: no signals, no
//! views, no browser. That is deliberate. It is the part of a data grid that is
//! actually easy to get wrong - a filter that reads columns nobody can see, a
//! sort that runs before the filter, a page that goes out of range as soon as
//! someone types - and being a pure function is what lets those be tested
//! rather than clicked at.
//!
//! # The order of operations is the specification
//!
//! Filter, then search, then sort, then slice. Not because it is faster, but
//! because any other order is wrong:
//!
//! * sorting before searching sorts rows that are about to be thrown away
//! * slicing before sorting pages an unsorted list and then orders the page,
//!   which produces a table whose rows shuffle between pages and whose second
//!   page contains values that belong on the first
//!
//! A server-side source has to make the same promise in SQL - `WHERE`, then
//! `ORDER BY`, then `LIMIT`/`OFFSET`, which is the same sentence.

use phonix_core::query::{Page, PageRequest};

use super::column::Column;
use super::date::DateFilter;
use super::filter::Filter;

/// Every row that matches the search, in the order the sort asks for.
///
/// The whole result, unpaged - which is what an export wants, and what
/// [`apply`] then cuts a page out of.
///
/// `columns` decides what "matching" means: only [`searchable`] columns are
/// looked in, and only a [`sortable`] one can order the result. A request
/// naming a column that is neither is ignored rather than refused - it usually
/// means a stale sort left behind by a column that has since been removed, and
/// an unsorted table is a better answer than an error.
///
/// [`searchable`]: Column::searchable
/// [`sortable`]: Column::sortable
pub fn matched<T: Clone>(
    request: &PageRequest,
    columns: &[Column<T>],
    filters: &[Filter<T>],
    dates: &[DateFilter<T>],
    rows: &[T],
) -> Vec<T> {
    let needle = request.needle();
    let searchable: Vec<&Column<T>> = columns.iter().filter(|c| c.searchable).collect();

    let mut matched: Vec<T> = rows
        .iter()
        // Narrowed by the filters first, then by the search. Both are
        // narrowings so the order between them cannot change the result - but
        // it can change how much work the search does, and the filters are the
        // cheaper question.
        .filter(|row| filters.iter().all(|filter| filter.accepts(row, request)))
        .filter(|row| dates.iter().all(|filter| filter.accepts(row, request)))
        .filter(|row| match &needle {
            None => true,
            Some(needle) => searchable
                .iter()
                .any(|column| column.value(row).contains(needle)),
        })
        .cloned()
        .collect();

    if let Some(sort) = &request.sort
        && let Some(column) = columns.iter().find(|c| c.sortable && c.field == sort.field)
    {
        // `sort_by` rather than `sort_unstable_by`: rows that tie keep the
        // order the source gave them, so sorting by status does not reshuffle
        // the names within each status on every render.
        matched.sort_by(|a, b| {
            let ordering = column.value(a).compare(&column.value(b));

            if sort.direction.is_ascending() {
                ordering
            } else {
                ordering.reverse()
            }
        });
    }

    matched
}

/// Narrow, order and cut `rows` down to the page that was asked for.
pub fn apply<T: Clone>(
    request: &PageRequest,
    columns: &[Column<T>],
    filters: &[Filter<T>],
    dates: &[DateFilter<T>],
    rows: &[T],
) -> Page<T> {
    let request = request.sanitised();
    let matched = matched(&request, columns, filters, dates, rows);
    let total = matched.len() as u64;

    // Clamped first: typing into the search box while on page nine has to land
    // somewhere that exists, or the table goes blank with rows still in it.
    let request = request.clamped_to(total);
    let start = request.offset() as usize;
    let end = start
        .saturating_add(request.limit() as usize)
        .min(matched.len());
    let rows = matched.get(start..end).unwrap_or_default().to_vec();

    Page::new(rows, total, &request)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use phonix_core::query::{DateRange, Sort};

    use super::super::column::{Cell, Column};
    use super::super::filter::FilterChoice;
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Row {
        name: &'static str,
        note: &'static str,
        rank: u32,
    }

    fn row(name: &'static str, note: &'static str, rank: u32) -> Row {
        Row { name, note, rank }
    }

    /// `name` is searched and sorted, `note` is neither, `rank` sorts only.
    fn columns() -> Vec<Column<Row>> {
        vec![
            Column::new("name", "Name", |r: &Row| Cell::text(r.name)).findable(),
            Column::new("note", "Note", |r: &Row| Cell::text(r.note)),
            Column::new("rank", "Rank", |r: &Row| Cell::number(r.rank)).sortable(),
        ]
    }

    fn rows() -> Vec<Row> {
        vec![
            row("Beatrice", "alpha", 2),
            row("alan", "beta", 10),
            row("Cleo", "alpha", 9),
        ]
    }

    fn names(page: &Page<Row>) -> Vec<&'static str> {
        page.rows.iter().map(|r| r.name).collect()
    }

    #[test]
    fn without_a_search_everything_survives() {
        let page = apply(&PageRequest::first(10), &columns(), &[], &[], &rows());

        assert_eq!(page.total, 3);
        assert_eq!(names(&page), ["Beatrice", "alan", "Cleo"]);
    }

    #[test]
    fn a_search_looks_only_in_searchable_columns() {
        let request = PageRequest {
            search: "alpha".into(),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        // "alpha" is in `note`, which nobody may search.
        assert!(page.rows.is_empty());
        assert_eq!(page.total, 0);
    }

    #[test]
    fn a_search_ignores_case() {
        let request = PageRequest {
            search: "ALAN".into(),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(names(&page), ["alan"]);
    }

    #[test]
    fn sorting_text_ignores_case_too() {
        let request = PageRequest {
            sort: Some(Sort::ascending("name")),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(names(&page), ["alan", "Beatrice", "Cleo"]);
    }

    #[test]
    fn sorting_the_other_way_reverses_it() {
        let request = PageRequest {
            sort: Some(Sort::descending("name")),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(names(&page), ["Cleo", "Beatrice", "alan"]);
    }

    #[test]
    fn numbers_order_as_numbers() {
        let request = PageRequest {
            sort: Some(Sort::ascending("rank")),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(names(&page), ["Beatrice", "Cleo", "alan"]);
    }

    #[test]
    fn a_column_that_is_not_sortable_does_not_reorder_anything() {
        let request = PageRequest {
            sort: Some(Sort::ascending("note")),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(names(&page), ["Beatrice", "alan", "Cleo"]);
    }

    #[test]
    fn a_sort_naming_a_column_that_is_gone_is_ignored_rather_than_fatal() {
        let request = PageRequest {
            sort: Some(Sort::ascending("departed")),
            ..PageRequest::first(10)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(page.total, 3);
    }

    #[test]
    fn a_page_holds_only_its_own_rows_and_still_counts_them_all() {
        let request = PageRequest {
            page: 2,
            sort: Some(Sort::ascending("name")),
            ..PageRequest::first(2)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(names(&page), ["Cleo"]);
        assert_eq!(page.total, 3);
        assert_eq!(page.page, 2);
        assert!(page.has_previous());
        assert!(!page.has_next());
    }

    #[test]
    fn searching_from_a_later_page_lands_on_one_that_exists() {
        let request = PageRequest {
            page: 9,
            search: "alan".into(),
            ..PageRequest::first(2)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        assert_eq!(page.page, 1);
        assert_eq!(names(&page), ["alan"]);
    }

    #[test]
    fn the_list_is_searched_before_it_is_sorted_and_cut() {
        let request = PageRequest {
            search: "e".into(),
            sort: Some(Sort::descending("rank")),
            ..PageRequest::first(1)
        };
        let page = apply(&request, &columns(), &[], &[], &rows());

        // "Beatrice" and "Cleo" match; by rank descending Cleo (9) leads.
        assert_eq!(page.total, 2);
        assert_eq!(names(&page), ["Cleo"]);
    }

    #[test]
    fn matching_ignores_paging_entirely() {
        let request = PageRequest {
            sort: Some(Sort::ascending("name")),
            ..PageRequest::first(1)
        };
        let all = matched(&request, &columns(), &[], &[], &rows());

        // One row per page, and still every row here: this is what an export
        // writes, and an export of page one would be the bug.
        assert_eq!(all.len(), 3);
        assert_eq!(all.first().map(|r| r.name), Some("alan"));
    }

    fn ranks() -> Vec<FilterChoice> {
        vec![
            FilterChoice::all("Any rank"),
            FilterChoice::new("high", "High only"),
        ]
    }

    /// Answered in the browser, as an in-memory grid must.
    fn high_only() -> Vec<Filter<Row>> {
        vec![
            Filter::new("rank", "Rank", ranks()).matching(|row: &Row, value| match value {
                "high" => row.rank >= 9,
                _ => true,
            }),
        ]
    }

    #[test]
    fn a_filter_nobody_chose_narrows_nothing() {
        let page = apply(
            &PageRequest::first(10),
            &columns(),
            &high_only(),
            &[],
            &rows(),
        );

        assert_eq!(page.total, 3);
    }

    #[test]
    fn a_chosen_filter_narrows_and_the_count_follows_it() {
        let request = PageRequest::first(10).filtered_by("rank", "high");
        let page = apply(&request, &columns(), &high_only(), &[], &rows());

        // The total has to be what survived, not what exists: the pager sizes
        // itself from it, and a pager for rows nobody can reach is the bug.
        assert_eq!(page.total, 2);
        assert_eq!(names(&page), ["alan", "Cleo"]);
    }

    #[test]
    fn a_filter_and_a_search_both_have_to_be_satisfied() {
        let request = PageRequest {
            search: "alan".into(),
            ..PageRequest::first(10)
        }
        .filtered_by("rank", "high");
        let page = apply(&request, &columns(), &high_only(), &[], &rows());

        assert_eq!(names(&page), ["alan"]);
    }

    #[test]
    fn filtering_from_a_later_page_lands_on_one_that_exists() {
        let request = PageRequest {
            page: 9,
            ..PageRequest::first(2)
        }
        .filtered_by("rank", "high");
        let page = apply(&request, &columns(), &high_only(), &[], &rows());

        assert_eq!(page.page, 1);
        assert_eq!(page.total, 2);
    }

    #[test]
    fn an_export_of_a_filtered_table_is_the_filtered_table() {
        // `matched` is what the export writes. A filter the export ignored
        // would produce a file that does not match the screen it came from.
        let request = PageRequest::first(1).filtered_by("rank", "high");
        let all = matched(&request, &columns(), &high_only(), &[], &rows());

        assert_eq!(all.len(), 2);
    }

    // --- a span of time -------------------------------------------------

    /// Rows that carry an instant, one of which never happened.
    #[derive(Debug, Clone, PartialEq)]
    struct Event {
        name: &'static str,
        at: Option<DateTime<Utc>>,
    }

    fn day(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 12, 0, 0)
            .single()
            .expect("a real instant")
    }

    fn events() -> Vec<Event> {
        vec![
            Event {
                name: "monday",
                at: Some(day(17)),
            },
            Event {
                name: "friday",
                at: Some(day(21)),
            },
            Event {
                name: "next monday",
                at: Some(day(24)),
            },
            Event {
                name: "never",
                at: None,
            },
        ]
    }

    fn when() -> Vec<DateFilter<Event>> {
        vec![DateFilter::new("occurred", "When").at(|event: &Event| event.at)]
    }

    fn columns_of_events() -> Vec<Column<Event>> {
        vec![Column::new("name", "Name", |e: &Event| Cell::text(e.name)).findable()]
    }

    fn happened(page: &Page<Event>) -> Vec<&'static str> {
        page.rows.iter().map(|row| row.name).collect()
    }

    #[test]
    fn a_span_nobody_chose_keeps_every_row_including_the_one_with_no_instant() {
        let page = apply(
            &PageRequest::first(10),
            &columns_of_events(),
            &[],
            &when(),
            &events(),
        );

        assert_eq!(page.total, 4);
    }

    #[test]
    fn a_chosen_span_keeps_the_rows_inside_it_and_drops_the_one_with_no_instant() {
        // Monday to Monday, the end excluded: "next monday" is the first row
        // outside it, and "never" is in no span at all.
        let week = DateRange::between(
            Utc.with_ymd_and_hms(2026, 8, 17, 0, 0, 0)
                .single()
                .expect("a real instant"),
            Utc.with_ymd_and_hms(2026, 8, 24, 0, 0, 0)
                .single()
                .expect("a real instant"),
        );
        let request = PageRequest::first(10).in_range("occurred", week);
        let page = apply(&request, &columns_of_events(), &[], &when(), &events());

        assert_eq!(happened(&page), ["monday", "friday"]);
        // The pager sizes itself from this, so a total that counted the rows a
        // span threw away is a pager for rows nobody can reach.
        assert_eq!(page.total, 2);
    }

    #[test]
    fn a_span_and_a_search_both_have_to_be_satisfied() {
        let request = PageRequest {
            search: "monday".into(),
            ..PageRequest::first(10)
        }
        .in_range("occurred", DateRange::since(day(20)));

        let page = apply(&request, &columns_of_events(), &[], &when(), &events());

        assert_eq!(happened(&page), ["next monday"]);
    }

    #[test]
    fn an_export_of_a_table_narrowed_to_a_span_is_that_span() {
        // `matched` is what the export writes. A range the export ignored would
        // produce a file that does not match the screen it came from.
        let request = PageRequest::first(1).in_range("occurred", DateRange::until(day(21)));
        let all = matched(&request, &columns_of_events(), &[], &when(), &events());

        assert_eq!(all.len(), 1);
    }

    #[test]
    fn an_empty_list_is_one_empty_page_rather_than_nothing() {
        let page = apply(&PageRequest::first(10), &columns(), &[], &[], &[]);

        assert!(page.is_empty());
        assert_eq!(page.page_count(), 1);
        assert_eq!(page.first_row_number(), 0);
    }
}
