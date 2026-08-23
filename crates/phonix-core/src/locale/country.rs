//! ISO 3166-1 alpha-2 country codes.
//!
//! Two letters, validated against the list, because a country stored as free
//! text is a country nobody can group by. "USA", "U.S.", "United States" and
//! "us" are four rows and one country, and the report that has to add them up
//! is written long after the form that let them in.
//!
//! The names here are the English short names, lightly plainened where the
//! official form is unhelpful in a dropdown (`United Kingdom`, not `United
//! Kingdom of Great Britain and Northern Ireland`). They are labels, not
//! identifiers - the code is the identifier, and it is what is stored.

use core::fmt;

use serde::{Deserialize, Serialize};

/// One country, territory or dependency with an alpha-2 code.
///
/// Ordering is by `code`; a picker that wants alphabetical labels sorts by
/// [`Country::name`] itself, because that order depends on the language the
/// names are in and this list is in English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Country {
    code: &'static str,
    name: &'static str,
}

impl Country {
    /// Look up a code. Case-insensitive, trimmed.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, UnknownCountry> {
        let raw = raw.as_ref().trim();

        if raw.len() != 2 {
            return Err(UnknownCountry);
        }

        COUNTRIES
            .iter()
            .find(|country| country.code.eq_ignore_ascii_case(raw))
            .copied()
            .ok_or(UnknownCountry)
    }

    /// The two-letter code, upper case, as stored.
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// The English short name, for a label.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Every country, ordered by code.
    pub const fn all() -> &'static [Self] {
        COUNTRIES
    }

    /// Every country, ordered by name - which is the order a person scanning a
    /// dropdown expects.
    pub fn all_by_name() -> Vec<Self> {
        let mut countries = COUNTRIES.to_vec();
        countries.sort_by_key(|country| country.name);
        countries
    }
}

impl fmt::Display for Country {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl AsRef<str> for Country {
    fn as_ref(&self) -> &str {
        self.code
    }
}

/// Serialised as the bare code - see the note on `Currency`, for the same
/// reason.
impl Serialize for Country {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code)
    }
}

impl<'de> Deserialize<'de> for Country {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an ISO 3166-1 alpha-2 country code")]
pub struct UnknownCountry;

/// The ISO 3166-1 alpha-2 list, by code.
const COUNTRIES: &[Country] = &[
    n("AD", "Andorra"),
    n("AE", "United Arab Emirates"),
    n("AF", "Afghanistan"),
    n("AG", "Antigua and Barbuda"),
    n("AI", "Anguilla"),
    n("AL", "Albania"),
    n("AM", "Armenia"),
    n("AO", "Angola"),
    n("AQ", "Antarctica"),
    n("AR", "Argentina"),
    n("AS", "American Samoa"),
    n("AT", "Austria"),
    n("AU", "Australia"),
    n("AW", "Aruba"),
    n("AX", "Aland Islands"),
    n("AZ", "Azerbaijan"),
    n("BA", "Bosnia and Herzegovina"),
    n("BB", "Barbados"),
    n("BD", "Bangladesh"),
    n("BE", "Belgium"),
    n("BF", "Burkina Faso"),
    n("BG", "Bulgaria"),
    n("BH", "Bahrain"),
    n("BI", "Burundi"),
    n("BJ", "Benin"),
    n("BL", "Saint Barthelemy"),
    n("BM", "Bermuda"),
    n("BN", "Brunei Darussalam"),
    n("BO", "Bolivia"),
    n("BQ", "Bonaire, Sint Eustatius and Saba"),
    n("BR", "Brazil"),
    n("BS", "Bahamas"),
    n("BT", "Bhutan"),
    n("BV", "Bouvet Island"),
    n("BW", "Botswana"),
    n("BY", "Belarus"),
    n("BZ", "Belize"),
    n("CA", "Canada"),
    n("CC", "Cocos (Keeling) Islands"),
    n("CD", "Congo, Democratic Republic of the"),
    n("CF", "Central African Republic"),
    n("CG", "Congo"),
    n("CH", "Switzerland"),
    n("CI", "Cote d'Ivoire"),
    n("CK", "Cook Islands"),
    n("CL", "Chile"),
    n("CM", "Cameroon"),
    n("CN", "China"),
    n("CO", "Colombia"),
    n("CR", "Costa Rica"),
    n("CU", "Cuba"),
    n("CV", "Cabo Verde"),
    n("CW", "Curacao"),
    n("CX", "Christmas Island"),
    n("CY", "Cyprus"),
    n("CZ", "Czechia"),
    n("DE", "Germany"),
    n("DJ", "Djibouti"),
    n("DK", "Denmark"),
    n("DM", "Dominica"),
    n("DO", "Dominican Republic"),
    n("DZ", "Algeria"),
    n("EC", "Ecuador"),
    n("EE", "Estonia"),
    n("EG", "Egypt"),
    n("EH", "Western Sahara"),
    n("ER", "Eritrea"),
    n("ES", "Spain"),
    n("ET", "Ethiopia"),
    n("FI", "Finland"),
    n("FJ", "Fiji"),
    n("FK", "Falkland Islands"),
    n("FM", "Micronesia"),
    n("FO", "Faroe Islands"),
    n("FR", "France"),
    n("GA", "Gabon"),
    n("GB", "United Kingdom"),
    n("GD", "Grenada"),
    n("GE", "Georgia"),
    n("GF", "French Guiana"),
    n("GG", "Guernsey"),
    n("GH", "Ghana"),
    n("GI", "Gibraltar"),
    n("GL", "Greenland"),
    n("GM", "Gambia"),
    n("GN", "Guinea"),
    n("GP", "Guadeloupe"),
    n("GQ", "Equatorial Guinea"),
    n("GR", "Greece"),
    n("GS", "South Georgia and the South Sandwich Islands"),
    n("GT", "Guatemala"),
    n("GU", "Guam"),
    n("GW", "Guinea-Bissau"),
    n("GY", "Guyana"),
    n("HK", "Hong Kong"),
    n("HM", "Heard Island and McDonald Islands"),
    n("HN", "Honduras"),
    n("HR", "Croatia"),
    n("HT", "Haiti"),
    n("HU", "Hungary"),
    n("ID", "Indonesia"),
    n("IE", "Ireland"),
    n("IL", "Israel"),
    n("IM", "Isle of Man"),
    n("IN", "India"),
    n("IO", "British Indian Ocean Territory"),
    n("IQ", "Iraq"),
    n("IR", "Iran"),
    n("IS", "Iceland"),
    n("IT", "Italy"),
    n("JE", "Jersey"),
    n("JM", "Jamaica"),
    n("JO", "Jordan"),
    n("JP", "Japan"),
    n("KE", "Kenya"),
    n("KG", "Kyrgyzstan"),
    n("KH", "Cambodia"),
    n("KI", "Kiribati"),
    n("KM", "Comoros"),
    n("KN", "Saint Kitts and Nevis"),
    n("KP", "Korea, Democratic People's Republic of"),
    n("KR", "Korea, Republic of"),
    n("KW", "Kuwait"),
    n("KY", "Cayman Islands"),
    n("KZ", "Kazakhstan"),
    n("LA", "Lao People's Democratic Republic"),
    n("LB", "Lebanon"),
    n("LC", "Saint Lucia"),
    n("LI", "Liechtenstein"),
    n("LK", "Sri Lanka"),
    n("LR", "Liberia"),
    n("LS", "Lesotho"),
    n("LT", "Lithuania"),
    n("LU", "Luxembourg"),
    n("LV", "Latvia"),
    n("LY", "Libya"),
    n("MA", "Morocco"),
    n("MC", "Monaco"),
    n("MD", "Moldova"),
    n("ME", "Montenegro"),
    n("MF", "Saint Martin (French part)"),
    n("MG", "Madagascar"),
    n("MH", "Marshall Islands"),
    n("MK", "North Macedonia"),
    n("ML", "Mali"),
    n("MM", "Myanmar"),
    n("MN", "Mongolia"),
    n("MO", "Macao"),
    n("MP", "Northern Mariana Islands"),
    n("MQ", "Martinique"),
    n("MR", "Mauritania"),
    n("MS", "Montserrat"),
    n("MT", "Malta"),
    n("MU", "Mauritius"),
    n("MV", "Maldives"),
    n("MW", "Malawi"),
    n("MX", "Mexico"),
    n("MY", "Malaysia"),
    n("MZ", "Mozambique"),
    n("NA", "Namibia"),
    n("NC", "New Caledonia"),
    n("NE", "Niger"),
    n("NF", "Norfolk Island"),
    n("NG", "Nigeria"),
    n("NI", "Nicaragua"),
    n("NL", "Netherlands"),
    n("NO", "Norway"),
    n("NP", "Nepal"),
    n("NR", "Nauru"),
    n("NU", "Niue"),
    n("NZ", "New Zealand"),
    n("OM", "Oman"),
    n("PA", "Panama"),
    n("PE", "Peru"),
    n("PF", "French Polynesia"),
    n("PG", "Papua New Guinea"),
    n("PH", "Philippines"),
    n("PK", "Pakistan"),
    n("PL", "Poland"),
    n("PM", "Saint Pierre and Miquelon"),
    n("PN", "Pitcairn"),
    n("PR", "Puerto Rico"),
    n("PS", "Palestine, State of"),
    n("PT", "Portugal"),
    n("PW", "Palau"),
    n("PY", "Paraguay"),
    n("QA", "Qatar"),
    n("RE", "Reunion"),
    n("RO", "Romania"),
    n("RS", "Serbia"),
    n("RU", "Russian Federation"),
    n("RW", "Rwanda"),
    n("SA", "Saudi Arabia"),
    n("SB", "Solomon Islands"),
    n("SC", "Seychelles"),
    n("SD", "Sudan"),
    n("SE", "Sweden"),
    n("SG", "Singapore"),
    n("SH", "Saint Helena, Ascension and Tristan da Cunha"),
    n("SI", "Slovenia"),
    n("SJ", "Svalbard and Jan Mayen"),
    n("SK", "Slovakia"),
    n("SL", "Sierra Leone"),
    n("SM", "San Marino"),
    n("SN", "Senegal"),
    n("SO", "Somalia"),
    n("SR", "Suriname"),
    n("SS", "South Sudan"),
    n("ST", "Sao Tome and Principe"),
    n("SV", "El Salvador"),
    n("SX", "Sint Maarten (Dutch part)"),
    n("SY", "Syrian Arab Republic"),
    n("SZ", "Eswatini"),
    n("TC", "Turks and Caicos Islands"),
    n("TD", "Chad"),
    n("TF", "French Southern Territories"),
    n("TG", "Togo"),
    n("TH", "Thailand"),
    n("TJ", "Tajikistan"),
    n("TK", "Tokelau"),
    n("TL", "Timor-Leste"),
    n("TM", "Turkmenistan"),
    n("TN", "Tunisia"),
    n("TO", "Tonga"),
    n("TR", "Turkiye"),
    n("TT", "Trinidad and Tobago"),
    n("TV", "Tuvalu"),
    n("TW", "Taiwan"),
    n("TZ", "Tanzania"),
    n("UA", "Ukraine"),
    n("UG", "Uganda"),
    n("UM", "United States Minor Outlying Islands"),
    n("US", "United States"),
    n("UY", "Uruguay"),
    n("UZ", "Uzbekistan"),
    n("VA", "Holy See"),
    n("VC", "Saint Vincent and the Grenadines"),
    n("VE", "Venezuela"),
    n("VG", "Virgin Islands (British)"),
    n("VI", "Virgin Islands (U.S.)"),
    n("VN", "Viet Nam"),
    n("VU", "Vanuatu"),
    n("WF", "Wallis and Futuna"),
    n("WS", "Samoa"),
    n("YE", "Yemen"),
    n("YT", "Mayotte"),
    n("ZA", "South Africa"),
    n("ZM", "Zambia"),
    n("ZW", "Zimbabwe"),
];

/// Shorthand so the table above reads as a table.
const fn n(code: &'static str, name: &'static str) -> Country {
    Country { code, name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_code_in_any_case() {
        assert_eq!(Country::parse("gb").unwrap().name(), "United Kingdom");
        assert_eq!(Country::parse("  KE  ").unwrap().name(), "Kenya");
    }

    #[test]
    fn refuses_anything_that_is_not_a_code() {
        // "UK" is the one people reach for, and it is not the code.
        for bad in ["", "U", "USA", "UK", "ZZ", "United States", "12"] {
            assert!(Country::parse(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        let mut previous = "";
        for country in Country::all() {
            assert!(
                country.code() > previous,
                "{} is out of order or duplicated",
                country.code(),
            );
            previous = country.code();
        }
    }

    #[test]
    fn every_entry_is_a_plausible_alpha_2_row() {
        for country in Country::all() {
            assert_eq!(country.code().len(), 2, "{}", country.code());
            assert!(
                country.code().bytes().all(|b| b.is_ascii_uppercase()),
                "{} is not upper-case ascii",
                country.code(),
            );
            assert!(!country.name().is_empty(), "{} has no name", country.code());
        }
    }

    #[test]
    fn the_list_is_long_enough_to_be_the_real_one() {
        // A truncated list is the failure that looks like success: the form
        // works, and one customer cannot find their country.
        assert!(
            Country::all().len() > 240,
            "only {} countries - the list has been truncated",
            Country::all().len(),
        );
    }

    #[test]
    fn sorting_by_name_puts_afghanistan_first_and_not_andorra() {
        let by_name = Country::all_by_name();
        assert_eq!(
            by_name.first().copied().map(Country::name),
            Some("Afghanistan")
        );
        // By code, Andorra is first - so this proves the two orders differ.
        assert_eq!(
            Country::all().first().copied().map(Country::name),
            Some("Andorra"),
        );
    }

    #[test]
    fn round_trips_through_json_as_a_bare_code() {
        let json = serde_json::to_string(&Country::parse("JP").unwrap()).unwrap();
        assert_eq!(json, "\"JP\"");
        assert_eq!(
            serde_json::from_str::<Country>(&json).unwrap().name(),
            "Japan"
        );
        assert!(serde_json::from_str::<Country>("\"ZZ\"").is_err());
    }
}
