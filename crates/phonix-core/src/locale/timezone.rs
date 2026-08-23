//! An IANA time zone name, e.g. `Africa/Nairobi`.
//!
//! # Why this validates the shape and not the name
//!
//! Checking that `Africa/Nairobi` is a *real* zone means carrying the IANA
//! database, and the only crate that does that for `chrono` compiles its tables
//! into the binary - including the WebAssembly one. This crate is in the
//! browser bundle, and a hundred kilobytes of transition rules is a poor trade
//! for refusing a string nobody is typing by hand.
//!
//! So: the shape is enforced here, the picker offers [`Timezone::common`], and
//! the arithmetic - which is the only thing that needs the real database - is
//! done on the server, where the dependency is free. A name that passes here
//! and is not a zone resolves to UTC there, with a warning, rather than
//! failing a request.
//!
//! # Why an organization has one at all
//!
//! Because "today" is a question with two answers. A date range picked in a
//! browser is resolved in the *viewer's* zone; a report that says "this month"
//! has to mean the organization's month, or a sale made at 9pm on the 31st in
//! Nairobi lands in the wrong one for a director reading it in London.

use core::fmt;

use serde::{Deserialize, Serialize};

/// A validated IANA time zone name.
///
/// Owned rather than `&'static str`, because [`Timezone::parse`] accepts any
/// well-formed name - not only the ones in [`Timezone::common`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Timezone(String);

/// Long enough for `America/Argentina/ComodRivadavia`, the longest name in the
/// database, with room to spare.
const MAX_LEN: usize = 64;

impl Timezone {
    /// UTC, which every deployment can resolve and no deployment gets wrong.
    pub fn utc() -> Self {
        Self("UTC".to_owned())
    }

    /// Accepts `UTC` or `Area/Location`, with an optional third segment.
    ///
    /// Case is preserved, not folded: IANA names are case-sensitive, and
    /// `africa/nairobi` is not a name the database will answer to.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, InvalidTimezone> {
        let raw = raw.as_ref().trim();

        if raw.is_empty() || raw.len() > MAX_LEN {
            return Err(InvalidTimezone::Length { max: MAX_LEN });
        }
        if !raw.is_ascii() {
            return Err(InvalidTimezone::Charset);
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'/'))
        {
            return Err(InvalidTimezone::Charset);
        }

        // `UTC` is the one name with no area, and it is the default, so it is
        // spelled out rather than left to a special case in the segment count.
        if raw == "UTC" {
            return Ok(Self(raw.to_owned()));
        }

        let segments: Vec<&str> = raw.split('/').collect();
        if !(2..=3).contains(&segments.len()) {
            return Err(InvalidTimezone::Shape);
        }
        if segments.iter().any(|segment| segment.is_empty()) {
            return Err(InvalidTimezone::Shape);
        }
        // Every real name starts its segments with a letter. This is what keeps
        // `12/34` and `-/-` out.
        if !segments.iter().all(|segment| {
            segment
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
        }) {
            return Err(InvalidTimezone::Shape);
        }

        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name with underscores opened out, for a label: `America/New York`.
    pub fn label(&self) -> String {
        self.0.replace('_', " ")
    }

    /// The zones offered in the picker.
    ///
    /// Not the whole database - about four hundred of its six hundred names are
    /// aliases and backward-compatibility links, and a dropdown containing both
    /// `Asia/Calcutta` and `Asia/Kolkata` asks a question with no right answer.
    /// Anything outside this list still parses; it just is not offered.
    pub const fn common() -> &'static [&'static str] {
        COMMON
    }
}

impl Default for Timezone {
    fn default() -> Self {
        Self::utc()
    }
}

impl fmt::Display for Timezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Timezone {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// Deserialize goes through `parse`, so a name arriving as JSON is checked
// exactly like one arriving from the form.
impl<'de> Deserialize<'de> for Timezone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidTimezone {
    #[error("a time zone name must be 1-{max} characters")]
    Length { max: usize },
    #[error("a time zone name may only contain letters, digits, and _ - + /")]
    Charset,
    #[error("a time zone name looks like Area/Location, for example Africa/Nairobi")]
    Shape,
}

/// Common IANA zones, one per populated offset that anybody selects in
/// practice, in the order a picker shows them.
const COMMON: &[&str] = &[
    "UTC",
    // --- Africa ---
    "Africa/Abidjan",
    "Africa/Accra",
    "Africa/Addis_Ababa",
    "Africa/Algiers",
    "Africa/Cairo",
    "Africa/Casablanca",
    "Africa/Dar_es_Salaam",
    "Africa/Johannesburg",
    "Africa/Kampala",
    "Africa/Khartoum",
    "Africa/Lagos",
    "Africa/Nairobi",
    "Africa/Tripoli",
    "Africa/Tunis",
    // --- Americas ---
    "America/Anchorage",
    "America/Argentina/Buenos_Aires",
    "America/Bogota",
    "America/Caracas",
    "America/Chicago",
    "America/Denver",
    "America/Halifax",
    "America/Lima",
    "America/Los_Angeles",
    "America/Mexico_City",
    "America/New_York",
    "America/Panama",
    "America/Phoenix",
    "America/Santiago",
    "America/Sao_Paulo",
    "America/St_Johns",
    "America/Toronto",
    "America/Vancouver",
    // --- Asia ---
    "Asia/Almaty",
    "Asia/Amman",
    "Asia/Baghdad",
    "Asia/Baku",
    "Asia/Bangkok",
    "Asia/Beirut",
    "Asia/Colombo",
    "Asia/Dhaka",
    "Asia/Dubai",
    "Asia/Ho_Chi_Minh",
    "Asia/Hong_Kong",
    "Asia/Jakarta",
    "Asia/Jerusalem",
    "Asia/Kabul",
    "Asia/Karachi",
    "Asia/Kathmandu",
    "Asia/Kolkata",
    "Asia/Kuala_Lumpur",
    "Asia/Kuwait",
    "Asia/Manila",
    "Asia/Qatar",
    "Asia/Riyadh",
    "Asia/Seoul",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Taipei",
    "Asia/Tashkent",
    "Asia/Tbilisi",
    "Asia/Tehran",
    "Asia/Tokyo",
    "Asia/Yangon",
    "Asia/Yerevan",
    // --- Atlantic ---
    "Atlantic/Azores",
    "Atlantic/Reykjavik",
    // --- Australia ---
    "Australia/Adelaide",
    "Australia/Brisbane",
    "Australia/Darwin",
    "Australia/Melbourne",
    "Australia/Perth",
    "Australia/Sydney",
    // --- Europe ---
    "Europe/Amsterdam",
    "Europe/Athens",
    "Europe/Belgrade",
    "Europe/Berlin",
    "Europe/Brussels",
    "Europe/Bucharest",
    "Europe/Budapest",
    "Europe/Copenhagen",
    "Europe/Dublin",
    "Europe/Helsinki",
    "Europe/Istanbul",
    "Europe/Kyiv",
    "Europe/Lisbon",
    "Europe/London",
    "Europe/Madrid",
    "Europe/Moscow",
    "Europe/Oslo",
    "Europe/Paris",
    "Europe/Prague",
    "Europe/Rome",
    "Europe/Stockholm",
    "Europe/Vienna",
    "Europe/Warsaw",
    "Europe/Zurich",
    // --- Pacific ---
    "Pacific/Auckland",
    "Pacific/Fiji",
    "Pacific/Guam",
    "Pacific/Honolulu",
    "Pacific/Port_Moresby",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for good in [
            "UTC",
            "Africa/Nairobi",
            "America/New_York",
            "America/Argentina/Buenos_Aires",
            "Europe/Kyiv",
        ] {
            assert!(Timezone::parse(good).is_ok(), "{good} should parse");
        }
    }

    #[test]
    fn refuses_things_that_are_not_names() {
        for bad in [
            "",
            "Nairobi",                    // no area
            "Africa/",                    // empty segment
            "/Nairobi",                   // empty segment
            "Africa//Nairobi",            // empty segment
            "a/b/c/d",                    // too many segments
            "12/34",                      // segments must start with a letter
            "Africa/Nairobi; DROP TABLE", // punctuation and a space
            "Africa/Nairobi\u{0301}",     // not ascii
        ] {
            assert!(Timezone::parse(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn trims_but_does_not_fold_case() {
        // IANA names are case-sensitive; folding would produce a name the
        // database does not answer to.
        assert_eq!(
            Timezone::parse("  Africa/Nairobi  ").unwrap().as_str(),
            "Africa/Nairobi"
        );
        let lowered = Timezone::parse("africa/nairobi").unwrap();
        assert_eq!(lowered.as_str(), "africa/nairobi");
    }

    #[test]
    fn the_default_is_utc() {
        assert_eq!(Timezone::default().as_str(), "UTC");
    }

    #[test]
    fn every_offered_zone_parses() {
        // The picker must not be able to offer something the validator refuses.
        for name in Timezone::common() {
            assert!(
                Timezone::parse(name).is_ok(),
                "{name} is offered but does not parse"
            );
        }
    }

    #[test]
    fn the_offered_list_has_no_duplicates() {
        let mut seen: Vec<&str> = Timezone::common().to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(
            before,
            seen.len(),
            "the offered zone list contains a duplicate"
        );
    }

    #[test]
    fn labels_open_out_underscores() {
        assert_eq!(
            Timezone::parse("America/New_York").unwrap().label(),
            "America/New York"
        );
    }

    #[test]
    fn round_trips_through_json() {
        let zone = Timezone::parse("Asia/Tokyo").unwrap();
        let json = serde_json::to_string(&zone).unwrap();
        assert_eq!(json, "\"Asia/Tokyo\"");
        assert_eq!(serde_json::from_str::<Timezone>(&json).unwrap(), zone);
        assert!(serde_json::from_str::<Timezone>("\"Nairobi\"").is_err());
    }
}
