//! Dates and times, in words the reader can read.
//!
//! # Why chrono's own formatting is not enough
//!
//! `at.format("%-d %B %Y")` is English. `%B` is "August", always, because
//! chrono's month names are compiled in and there is exactly one set of them
//! unless the `unstable-locales` feature is turned on - and that feature buys a
//! table of CLDR names keyed by a locale *chrono* recognises, which is a second
//! list of languages to keep in step with [`Language`](super::Language).
//!
//! So the names come from the catalog, like every other word. Twelve months,
//! seven weekdays and a handful of patterns, in the same file a translator
//! already has open.
//!
//! # The pattern is a key too, not just the words
//!
//! This is the part that a table of month names alone gets wrong. The three
//! languages this ships in do not agree on how a date is *assembled*:
//!
//! ```text
//! en   23 August 2026        at 14:05 UTC
//! fr   23 août 2026          à 14:05 UTC
//! de   23. August 2026       um 14:05 UTC
//! ```
//!
//! German puts a full stop after the day; all three use a different word to
//! join a date to a time. None of that is expressible as "swap the month name
//! and keep the layout", so the layout is a catalog entry as well -
//! `date.day.long` is `"{day} {month} {year}"`, and German's is
//! `"{day}. {month} {year}"`. A language that wants the year first can have it
//! without a line of Rust changing.
//!
//! # What stays in digits
//!
//! `%Y-%m-%d` is not a date in words - it is a value. It goes in the `value`
//! of an `<input type="date">`, in an export's filename and in a sort key,
//! where a reader is not the audience and a translation would be a bug. Those
//! stay exactly where they are. Only the forms with a month *name* in them come
//! through here.
//!
//! # Zones
//!
//! Everything here renders UTC, and says so. See the note on zones in
//! `phonix_web::ui::table::date`: the workspace's own time zone is a setting
//! that exists and is deliberately not applied to stored instants yet, and a
//! date that silently shifted by a few hours between two screens would be worse
//! than one that is honest about the zone it is in.

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc, Weekday};

use super::catalog::Catalog;
use super::message::Message;
use crate::msg;

/// The month's full name - "August", "août", "August".
///
/// A `match` rather than an indexed table so that every key is a literal `msg!`
/// can check at compile time. Twelve arms is a small price for a mistyped month
/// being a build failure instead of a screen reading `date.month.8`.
///
/// Out-of-range input is impossible from `chrono` and renders as an empty
/// string rather than panicking, because this runs in the wasm bundle where a
/// panic takes the whole page down.
pub fn month(month: u32) -> Option<Message> {
    Some(match month {
        1 => msg!("date.month.1"),
        2 => msg!("date.month.2"),
        3 => msg!("date.month.3"),
        4 => msg!("date.month.4"),
        5 => msg!("date.month.5"),
        6 => msg!("date.month.6"),
        7 => msg!("date.month.7"),
        8 => msg!("date.month.8"),
        9 => msg!("date.month.9"),
        10 => msg!("date.month.10"),
        11 => msg!("date.month.11"),
        12 => msg!("date.month.12"),
        _ => return None,
    })
}

/// The month shortened for a control that has one line - "Aug", "août", "Aug".
///
/// Not derived by truncating the full name: French does not abbreviate most of
/// its months at all, and "Fév" cut to three letters from "Février" would be
/// right by accident and wrong for "Juin".
pub fn month_short(month: u32) -> Option<Message> {
    Some(match month {
        1 => msg!("date.month.short.1"),
        2 => msg!("date.month.short.2"),
        3 => msg!("date.month.short.3"),
        4 => msg!("date.month.short.4"),
        5 => msg!("date.month.short.5"),
        6 => msg!("date.month.short.6"),
        7 => msg!("date.month.short.7"),
        8 => msg!("date.month.short.8"),
        9 => msg!("date.month.short.9"),
        10 => msg!("date.month.short.10"),
        11 => msg!("date.month.short.11"),
        12 => msg!("date.month.short.12"),
        _ => return None,
    })
}

/// A calendar's column heading - two letters, Monday first.
pub fn weekday_short(weekday: Weekday) -> Message {
    match weekday {
        Weekday::Mon => msg!("date.weekday.short.mon"),
        Weekday::Tue => msg!("date.weekday.short.tue"),
        Weekday::Wed => msg!("date.weekday.short.wed"),
        Weekday::Thu => msg!("date.weekday.short.thu"),
        Weekday::Fri => msg!("date.weekday.short.fri"),
        Weekday::Sat => msg!("date.weekday.short.sat"),
        Weekday::Sun => msg!("date.weekday.short.sun"),
    }
}

/// The weekday spoken in full, for a screen reader.
pub fn weekday(weekday: Weekday) -> Message {
    match weekday {
        Weekday::Mon => msg!("date.weekday.mon"),
        Weekday::Tue => msg!("date.weekday.tue"),
        Weekday::Wed => msg!("date.weekday.wed"),
        Weekday::Thu => msg!("date.weekday.thu"),
        Weekday::Fri => msg!("date.weekday.fri"),
        Weekday::Sat => msg!("date.weekday.sat"),
        Weekday::Sun => msg!("date.weekday.sun"),
    }
}

/// "23 August 2026" - a date written out, for prose.
pub fn day_long(catalog: &Catalog, date: NaiveDate) -> String {
    assemble(catalog, "date.day.long", date, month(date.month()))
}

/// "23 Aug 2026" - the same date where the space is a button, not a paragraph.
pub fn day_short(catalog: &Catalog, date: NaiveDate) -> String {
    assemble(catalog, "date.day.short", date, month_short(date.month()))
}

/// "23 Aug" - a date that has already said which year it is in.
///
/// For a control whose other end carries the year, or a list where every row is
/// in the current one: repeating it on every line spends the width and tells
/// the reader nothing.
pub fn day_short_no_year(catalog: &Catalog, date: NaiveDate) -> String {
    let Some(name) = month_short(date.month()) else {
        return String::new();
    };

    catalog.render(&msg!(
        "date.day.short_no_year",
        day = date.day().to_string(),
        month = catalog.render(&name),
    ))
}

/// "August 2026" - the heading over a month of a calendar.
pub fn month_year(catalog: &Catalog, date: NaiveDate) -> String {
    let Some(name) = month(date.month()) else {
        return String::new();
    };

    catalog.render(&msg!(
        "date.month_year",
        month = catalog.render(&name),
        year = date.year().to_string(),
    ))
}

/// "14:05" - the clock, which is the same in every language this ships in.
///
/// Twenty-four hours everywhere on purpose. A workspace that wants "2:05 pm"
/// is asking for a per-language clock convention, and that is a setting with a
/// column behind it rather than a formatting decision taken here.
pub fn clock(at: DateTime<Utc>) -> String {
    format!("{:02}:{:02}", at.hour(), at.minute())
}

/// "23 August 2026 at 14:05 UTC" - an instant, written out.
pub fn moment_long(catalog: &Catalog, at: DateTime<Utc>) -> String {
    catalog.render(&msg!(
        "date.moment.long",
        date = day_long(catalog, at.date_naive()),
        time = clock(at),
    ))
}

/// "23 Aug 2026, 14:05 UTC" - an instant in a row of a list.
pub fn moment_short(catalog: &Catalog, at: DateTime<Utc>) -> String {
    catalog.render(&msg!(
        "date.moment.short",
        date = day_short(catalog, at.date_naive()),
        time = clock(at),
    ))
}

/// The shared half of [`day_long`] and [`day_short`].
fn assemble(catalog: &Catalog, key: &str, date: NaiveDate, name: Option<Message>) -> String {
    let Some(name) = name else {
        return String::new();
    };

    let mut message = Message::new(key);
    message.args.push(super::message::Arg {
        name: "day".to_owned(),
        value: date.day().to_string(),
    });
    message.args.push(super::message::Arg {
        name: "month".to_owned(),
        value: catalog.render(&name),
    });
    message.args.push(super::message::Arg {
        name: "year".to_owned(),
        value: date.year().to_string(),
    });

    catalog.render(&message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    fn english() -> Catalog {
        Catalog::builtin(Language::ENGLISH)
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    #[test]
    fn every_month_and_weekday_has_words() {
        let catalog = english();

        for number in 1..=12 {
            let long = month(number).expect("a month");
            let short = month_short(number).expect("a month");
            assert!(!catalog.render(&long).starts_with("date."), "{number}");
            assert!(!catalog.render(&short).starts_with("date."), "{number}");
        }

        for weekday in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            assert!(!catalog.render(&weekday_short(weekday)).starts_with("date."));
            assert!(
                !catalog
                    .render(&super::weekday(weekday))
                    .starts_with("date.")
            );
        }
    }

    #[test]
    fn a_month_outside_the_calendar_is_nothing_rather_than_a_panic() {
        assert!(month(0).is_none());
        assert!(month(13).is_none());
        assert!(month_short(13).is_none());
    }

    #[test]
    fn english_reads_the_way_it_always_did() {
        let catalog = english();

        assert_eq!(day_long(&catalog, day(2026, 2, 2)), "2 February 2026");
        assert_eq!(day_short(&catalog, day(2026, 8, 21)), "21 Aug 2026");
        assert_eq!(day_short_no_year(&catalog, day(2026, 8, 21)), "21 Aug");
        assert_eq!(month_year(&catalog, day(2026, 8, 1)), "August 2026");
    }

    #[test]
    fn an_instant_carries_its_zone() {
        let catalog = english();
        let at = day(2026, 2, 2)
            .and_hms_opt(2, 40, 0)
            .expect("a real time")
            .and_utc();

        assert_eq!(moment_long(&catalog, at), "2 February 2026 at 02:40 UTC");
        assert_eq!(moment_short(&catalog, at), "2 Feb 2026, 02:40 UTC");
    }
}
