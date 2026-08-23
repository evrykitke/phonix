//! Taking the table away as a file.
//!
//! # What gets exported
//!
//! What is on screen, in the order it is on screen: the visible columns, and
//! the rows the search and sort produced - not the page. Exporting one page of
//! twenty-five would be a surprising reading of "export", and exporting hidden
//! columns would be a surprising reading of "hidden".
//!
//! For a server-paged grid the browser only holds one page, so that is all it
//! can honestly write. The toolbar says so rather than producing a file that
//! looks complete and is not.
//!
//! # Why CSV, and why the quoting is written out
//!
//! CSV opens in every spreadsheet and needs no library. Its one hazard is
//! escaping, and it is a real hazard: a display name containing a comma, a
//! summary containing a newline, or a value starting with `=` all break or
//! subvert the file. [`to_csv`] handles the first two by RFC 4180 quoting and
//! the third by refusing to let a cell begin with a formula character.

use super::column::Column;

/// The characters a spreadsheet will treat as the start of a formula.
///
/// A cell beginning with one of these is prefixed with an apostrophe. Without
/// it, a display name of `=HYPERLINK(...)` becomes a live formula the moment
/// someone opens the export - which is CSV injection, and the reason an export
/// of user-supplied text is not just string concatenation.
const FORMULA_STARTERS: [char; 5] = ['=', '+', '-', '@', '\t'];

/// The table as CSV: a heading row, then one row per row.
pub fn to_csv<T>(columns: &[&Column<T>], rows: &[T]) -> String {
    let mut out = String::new();

    write_row(
        &mut out,
        columns.iter().map(|column| column.header().to_owned()),
    );

    for row in rows {
        write_row(
            &mut out,
            columns.iter().map(|column| column.value(row).to_text()),
        );
    }

    out
}

fn write_row(out: &mut String, cells: impl Iterator<Item = String>) {
    let mut first = true;

    for cell in cells {
        if !first {
            out.push(',');
        }

        first = false;
        out.push_str(&escape(&cell));
    }

    // CRLF: RFC 4180, and the line ending Excel is least surprised by.
    out.push_str("\r\n");
}

/// One cell, safe to place between commas.
fn escape(value: &str) -> String {
    let defused = match value.chars().next() {
        Some(first) if FORMULA_STARTERS.contains(&first) => format!("'{value}"),
        _ => value.to_owned(),
    };

    if defused.contains([',', '"', '\n', '\r']) {
        // A quote inside a quoted field is written twice. That is the whole of
        // RFC 4180's escaping, and getting it wrong shifts every later column.
        format!("\"{}\"", defused.replace('"', "\"\""))
    } else {
        defused
    }
}

/// A file name that a browser and a file system will both accept.
///
/// `users` becomes `users-2026-08-18.csv`. Dated because an export is a
/// snapshot, and three undated downloads in one folder are indistinguishable.
pub fn file_name(stem: &str) -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d");

    format!("{stem}-{today}.csv")
}

/// Hand `contents` to the browser as a download.
///
/// A blob URL and a synthetic anchor click: the file never leaves the page, so
/// there is no endpoint to secure and no copy of the data on the server.
#[cfg(feature = "hydrate")]
pub fn download(file_name: &str, contents: &str) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;

    // A BOM, because Excel on Windows reads a CSV without one as the local
    // code page and turns every accented name into mojibake.
    let with_bom = format!("\u{feff}{contents}");
    let parts = js_sys::Array::of1(&JsValue::from_str(&with_bom));

    let Ok(blob) = web_sys::Blob::new_with_str_sequence(&parts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };

    if let Ok(element) = leptos::prelude::document().create_element("a")
        && let Some(anchor) = element.dyn_ref::<web_sys::HtmlAnchorElement>()
    {
        anchor.set_href(&url);
        anchor.set_download(file_name);
        anchor.click();
    }

    // Released immediately: the click has already started the download, and a
    // blob left registered holds the whole file in memory until the tab closes.
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[cfg(not(feature = "hydrate"))]
pub fn download(_file_name: &str, _contents: &str) {
    // Exporting is a browser action. On the server this is unreachable, and a
    // panic here would turn a stray call into a 500 rather than nothing.
}

#[cfg(test)]
mod tests {
    use super::super::column::{Cell, Column};
    use super::*;

    fn column(
        field: &'static str,
        header: &'static str,
        read: impl Fn(&&'static str) -> Cell + Send + Sync + 'static,
    ) -> Column<&'static str> {
        Column::new(field, header, read)
    }

    #[test]
    fn the_first_line_is_the_headings() {
        let columns = [column("a", "Name", |r| Cell::text(*r))];
        let refs: Vec<&Column<&str>> = columns.iter().collect();

        assert!(to_csv(&refs, &["Ada"]).starts_with("Name\r\n"));
    }

    #[test]
    fn a_value_with_a_comma_is_quoted() {
        let columns = [column("a", "Name", |r| Cell::text(*r))];
        let refs: Vec<&Column<&str>> = columns.iter().collect();

        assert_eq!(
            to_csv(&refs, &["Lovelace, Ada"]),
            "Name\r\n\"Lovelace, Ada\"\r\n"
        );
    }

    #[test]
    fn a_quote_inside_a_value_is_doubled() {
        let columns = [column("a", "Name", |r| Cell::text(*r))];
        let refs: Vec<&Column<&str>> = columns.iter().collect();

        assert_eq!(
            to_csv(&refs, &["Ada \"the Countess\""]),
            "Name\r\n\"Ada \"\"the Countess\"\"\"\r\n"
        );
    }

    #[test]
    fn a_newline_inside_a_value_is_quoted_rather_than_ending_the_row() {
        let columns = [column("a", "Note", |r| Cell::text(*r))];
        let refs: Vec<&Column<&str>> = columns.iter().collect();
        let csv = to_csv(&refs, &["one\ntwo"]);

        assert_eq!(csv, "Note\r\n\"one\ntwo\"\r\n");
    }

    #[test]
    fn a_value_that_looks_like_a_formula_is_defused() {
        let columns = [column("a", "Name", |r| Cell::text(*r))];
        let refs: Vec<&Column<&str>> = columns.iter().collect();

        assert_eq!(to_csv(&refs, &["=1+1"]), "Name\r\n'=1+1\r\n");
    }

    #[test]
    fn an_empty_cell_is_an_empty_field_not_a_word() {
        let columns = [column("a", "Name", |_| Cell::Empty)];
        let refs: Vec<&Column<&str>> = columns.iter().collect();

        assert_eq!(to_csv(&refs, &["ignored"]), "Name\r\n\r\n");
    }

    #[test]
    fn only_the_columns_it_is_given_are_written() {
        let columns = [
            column("a", "Name", |r| Cell::text(*r)),
            column("b", "Hidden", |_| Cell::text("secret")),
        ];
        let visible: Vec<&Column<&str>> = columns.iter().take(1).collect();

        assert_eq!(to_csv(&visible, &["Ada"]), "Name\r\nAda\r\n");
    }

    #[test]
    fn the_file_name_carries_the_day_it_was_taken() {
        let name = file_name("users");

        assert!(name.starts_with("users-"));
        assert!(name.ends_with(".csv"));
        assert_eq!(name.len(), "users-2026-01-01.csv".len());
    }
}
