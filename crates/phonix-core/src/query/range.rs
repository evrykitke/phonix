//! A span of time, as a filter carries it.
//!
//! # Two keys, not one
//!
//! [`PageRequest::filters`](super::PageRequest::filters) is a flat map of
//! strings, and a range is two values. Rather than invent an encoding inside
//! one value - `"a..b"`, which then has to be split by every reader and quoted
//! by none of them - a range declared as `occurred` occupies two ordinary
//! filter keys, `occurred_from` and `occurred_to`. Each half is a filter in its
//! own right: absent means unbounded, which is what makes "everything since
//! Monday" expressible without a second control.
//!
//! # Half-open, always
//!
//! `from` is included and `to` is excluded. Every other convention has the same
//! bug in it: a range written as two dates and compared with `<=` either loses
//! the last day entirely (when `to` is midnight) or has to invent
//! `23:59:59.999`, which is wrong by a millisecond for as long as anyone stores
//! microseconds. `[Monday 00:00, Tuesday 00:00)` is exactly Monday and needs no
//! footnote.
//!
//! # Absolute instants, resolved before they are sent
//!
//! What crosses the wire is always two RFC 3339 instants. A name - "this week",
//! "last year" - is resolved by whoever offered the control, never sent. That
//! is deliberate:
//!
//! * a reader does not have to own a calendar, and cannot disagree with the
//!   screen about when a week starts
//! * the range the viewer is looking at in the two date fields is the range
//!   that was asked for, so the control cannot lie
//! * a request stays reproducible: the same request run tomorrow returns the
//!   same rows, which is what an audit needs and what "this week" would break

use chrono::{DateTime, SecondsFormat, Utc};

/// A half-open span of instants: `from` included, `to` excluded.
///
/// Either end may be absent, which means unbounded in that direction. Both
/// absent is [`DateRange::ANY`] - the range that narrows nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DateRange {
    pub from: Option<DateTime<Utc>>,
    /// Exclusive. See the module docs.
    pub to: Option<DateTime<Utc>>,
}

impl DateRange {
    /// The whole of time. What a filter nobody has narrowed carries.
    pub const ANY: Self = Self {
        from: None,
        to: None,
    };

    pub const fn new(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Self {
        Self { from, to }
    }

    /// `[from, to)`.
    pub const fn between(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Self {
            from: Some(from),
            to: Some(to),
        }
    }

    /// Everything at or after `from`.
    pub const fn since(from: DateTime<Utc>) -> Self {
        Self {
            from: Some(from),
            to: None,
        }
    }

    /// Everything before `to`.
    pub const fn until(to: DateTime<Utc>) -> Self {
        Self {
            from: None,
            to: Some(to),
        }
    }

    /// Whether this range narrows anything at all.
    pub const fn is_any(&self) -> bool {
        self.from.is_none() && self.to.is_none()
    }

    /// Whether the two ends are the wrong way round, and so match nothing.
    ///
    /// Reachable only from a hand-written request: every control that produces
    /// one of these calls [`normalised`](Self::normalised) first.
    pub fn is_impossible(&self) -> bool {
        matches!((self.from, self.to), (Some(from), Some(to)) if from >= to)
    }

    /// The same range with its ends the right way round.
    ///
    /// A calendar is clicked in whatever order the eye goes, and a viewer who
    /// picks the 21st and then the 12th means the twelfth to the twenty-first.
    /// Sorting is done where that intent exists - in the control - rather than
    /// on the way into a query, where an inverted range is just a range that
    /// matches nothing and should say so.
    #[must_use]
    pub fn normalised(self) -> Self {
        match (self.from, self.to) {
            (Some(from), Some(to)) if from > to => Self {
                from: Some(to),
                to: Some(from),
            },
            _ => self,
        }
    }

    /// Whether `at` falls inside.
    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        self.from.is_none_or(|from| at >= from) && self.to.is_none_or(|to| at < to)
    }

    /// The two filter keys a range declared as `key` occupies.
    ///
    /// One function so that the control writing them and the reader looking
    /// for them cannot spell the suffix differently.
    pub fn keys(key: &str) -> (String, String) {
        (format!("{key}_from"), format!("{key}_to"))
    }

    /// One end as it crosses the wire.
    ///
    /// Seconds and a literal `Z`: the value is read by a human in a log as
    /// often as by a parser, and a trailing `+00:00` on a value that is always
    /// UTC is noise.
    pub fn encode(at: DateTime<Utc>) -> String {
        at.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// One end as it arrives.
    ///
    /// `None` for anything unparseable, which is the same forgiveness
    /// [`PageRequest::filter`](super::PageRequest::filter) shows an unknown
    /// key: a stale or hand-mangled value should narrow nothing, not refuse the
    /// page.
    pub fn decode(value: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(value.trim())
            .ok()
            .map(|at| at.with_timezone(&Utc))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
            .single()
            .expect("a real instant")
    }

    #[test]
    fn a_range_nobody_set_narrows_nothing() {
        assert!(DateRange::ANY.is_any());
        assert!(DateRange::ANY.contains(at(1, 0)));
        assert_eq!(DateRange::default(), DateRange::ANY);
    }

    #[test]
    fn the_end_is_excluded_and_the_start_is_not() {
        // The whole point of half-open: one day is [midnight, next midnight)
        // and the last instant of it belongs to that day, not to both.
        let day = DateRange::between(at(12, 0), at(13, 0));

        assert!(day.contains(at(12, 0)));
        assert!(day.contains(at(12, 23)));
        assert!(!day.contains(at(13, 0)));
        assert!(!day.contains(at(11, 23)));
    }

    #[test]
    fn one_end_alone_is_unbounded_in_the_other_direction() {
        assert!(DateRange::since(at(12, 0)).contains(at(31, 0)));
        assert!(!DateRange::since(at(12, 0)).contains(at(11, 0)));

        assert!(DateRange::until(at(12, 0)).contains(at(1, 0)));
        assert!(!DateRange::until(at(12, 0)).contains(at(12, 0)));
    }

    #[test]
    fn a_range_picked_backwards_is_turned_around() {
        let backwards = DateRange::between(at(21, 0), at(12, 0));

        assert!(backwards.is_impossible());
        assert_eq!(
            backwards.normalised(),
            DateRange::between(at(12, 0), at(21, 0))
        );
    }

    #[test]
    fn normalising_leaves_a_half_bounded_range_alone() {
        let since = DateRange::since(at(12, 0));

        assert_eq!(since.normalised(), since);
        assert!(!since.is_impossible());
    }

    #[test]
    fn an_instant_survives_the_round_trip_it_actually_takes() {
        let encoded = DateRange::encode(at(12, 9));

        assert_eq!(encoded, "2026-08-12T09:00:00Z");
        assert_eq!(DateRange::decode(&encoded), Some(at(12, 9)));
    }

    #[test]
    fn an_offset_that_is_not_utc_is_read_as_the_instant_it_names() {
        // Nothing this application writes looks like this, but a hand-written
        // request can, and 09:00+03:00 is 06:00Z whoever wrote it.
        assert_eq!(
            DateRange::decode("2026-08-12T09:00:00+03:00"),
            Some(at(12, 6))
        );
    }

    #[test]
    fn a_value_that_is_not_a_date_narrows_nothing_rather_than_failing() {
        assert_eq!(DateRange::decode("last tuesday"), None);
        assert_eq!(DateRange::decode(""), None);
        // A bare date is not RFC 3339 and is not accepted: the wire format is
        // an instant, and guessing which midnight this meant is how a range
        // ends up an hour out.
        assert_eq!(DateRange::decode("2026-08-12"), None);
    }

    #[test]
    fn both_halves_of_a_range_are_spelled_from_one_place() {
        let (from, to) = DateRange::keys("occurred");

        assert_eq!(from, "occurred_from");
        assert_eq!(to, "occurred_to");
    }
}
