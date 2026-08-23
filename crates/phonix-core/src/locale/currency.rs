//! ISO 4217 currency codes.
//!
//! A currency is not a three-letter string. It carries the one fact that
//! decides how money is stored and rendered - how many digits come after the
//! decimal point - and that number is not 2. It is 0 for the yen, 3 for the
//! Kuwaiti dinar, and getting it wrong is not a formatting bug: an amount held
//! in minor units is off by a factor of a hundred.
//!
//! Nothing in Phonix holds money yet. This type exists first on purpose,
//! because the alternative is a `TEXT` column that becomes a `Money` type's
//! problem later, by which time there are rows in it.
//!
//! # What is deliberately not here
//!
//! No symbol, and no formatting. A symbol is ambiguous ($ is at least a dozen
//! currencies) and placement, grouping and the decimal mark belong to the
//! *locale*, not to the currency - the same euro amount is `1.234,56 EUR` in
//! Berlin and `1 234,56 EUR` in Paris. That pairing is the job of whatever
//! renders an amount, and it should be written once, when there is an amount.

use core::fmt;

use serde::{Deserialize, Serialize};

/// One currency from the ISO 4217 active list.
///
/// Copy and three words wide, so it is passed by value like the code it
/// replaces. Ordering is by `code`, which is what a picker wants.
///
/// `name` and `minor_units` are functions of `code`, so deriving equality over
/// all three is the same relation as comparing codes - and cheaper to read than
/// a hand-written impl that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Currency {
    code: &'static str,
    name: &'static str,
    minor_units: u8,
}

impl Currency {
    /// What a workspace starts with, and the `DEFAULT` on the column.
    ///
    /// A default currency is a guess either way; this one is the guess that
    /// costs least to correct, because a workspace that has not chosen has not
    /// yet stored an amount in it.
    pub const USD: Self = Self {
        code: "USD",
        name: "United States dollar",
        minor_units: 2,
    };

    /// Look up a code. Case-insensitive, trimmed.
    ///
    /// Unknown codes are refused rather than kept as free text: the whole value
    /// of this type is that `minor_units` is always answerable, and a code that
    /// is not in the table has no answer.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, UnknownCurrency> {
        let raw = raw.as_ref().trim();

        if raw.len() != 3 {
            return Err(UnknownCurrency);
        }

        CURRENCIES
            .iter()
            .find(|currency| currency.code.eq_ignore_ascii_case(raw))
            .copied()
            .ok_or(UnknownCurrency)
    }

    /// The three-letter code, upper case, as stored.
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// The English name, for a label.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Digits after the decimal point: 0 for the yen, 2 for the dollar, 3 for
    /// the dinar.
    ///
    /// This is the number an amount is scaled by. A `Money` type storing minor
    /// units multiplies by `10^minor_units`.
    pub const fn minor_units(self) -> u8 {
        self.minor_units
    }

    /// `"USD - United States dollar"`, for a select.
    pub fn label(self) -> String {
        format!("{} - {}", self.code, self.name)
    }

    /// Every currency, ordered by code.
    pub const fn all() -> &'static [Self] {
        CURRENCIES
    }
}

impl Default for Currency {
    fn default() -> Self {
        Self::USD
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl AsRef<str> for Currency {
    fn as_ref(&self) -> &str {
        self.code
    }
}

/// Serialised as the bare code, so the column and the JSON payload hold the
/// same three characters and neither has to know about this struct.
impl Serialize for Currency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code)
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an ISO 4217 currency code")]
pub struct UnknownCurrency;

/// The ISO 4217 active list, by code.
///
/// Fund and metal codes (`XAU`, `XDR`, `XTS`, `XXX` and the `X..` unit codes)
/// are left out: none of them is a currency an organization invoices in, and
/// including them puts them in the picker.
///
/// `minor_units` is the column that matters. The three-digit currencies and the
/// zero-digit ones are the reason this table is data rather than a `len() == 3`
/// check.
const CURRENCIES: &[Currency] = &[
    c("AED", "UAE dirham", 2),
    c("AFN", "Afghan afghani", 2),
    c("ALL", "Albanian lek", 2),
    c("AMD", "Armenian dram", 2),
    c("ANG", "Netherlands Antillean guilder", 2),
    c("AOA", "Angolan kwanza", 2),
    c("ARS", "Argentine peso", 2),
    c("AUD", "Australian dollar", 2),
    c("AWG", "Aruban florin", 2),
    c("AZN", "Azerbaijani manat", 2),
    c("BAM", "Bosnia-Herzegovina convertible mark", 2),
    c("BBD", "Barbadian dollar", 2),
    c("BDT", "Bangladeshi taka", 2),
    c("BGN", "Bulgarian lev", 2),
    c("BHD", "Bahraini dinar", 3),
    c("BIF", "Burundian franc", 0),
    c("BMD", "Bermudian dollar", 2),
    c("BND", "Brunei dollar", 2),
    c("BOB", "Bolivian boliviano", 2),
    c("BRL", "Brazilian real", 2),
    c("BSD", "Bahamian dollar", 2),
    c("BTN", "Bhutanese ngultrum", 2),
    c("BWP", "Botswana pula", 2),
    c("BYN", "Belarusian ruble", 2),
    c("BZD", "Belize dollar", 2),
    c("CAD", "Canadian dollar", 2),
    c("CDF", "Congolese franc", 2),
    c("CHF", "Swiss franc", 2),
    c("CLP", "Chilean peso", 0),
    c("CNY", "Chinese yuan", 2),
    c("COP", "Colombian peso", 2),
    c("CRC", "Costa Rican colon", 2),
    c("CUP", "Cuban peso", 2),
    c("CVE", "Cape Verdean escudo", 2),
    c("CZK", "Czech koruna", 2),
    c("DJF", "Djiboutian franc", 0),
    c("DKK", "Danish krone", 2),
    c("DOP", "Dominican peso", 2),
    c("DZD", "Algerian dinar", 2),
    c("EGP", "Egyptian pound", 2),
    c("ERN", "Eritrean nakfa", 2),
    c("ETB", "Ethiopian birr", 2),
    c("EUR", "Euro", 2),
    c("FJD", "Fijian dollar", 2),
    c("FKP", "Falkland Islands pound", 2),
    c("GBP", "Pound sterling", 2),
    c("GEL", "Georgian lari", 2),
    c("GHS", "Ghanaian cedi", 2),
    c("GIP", "Gibraltar pound", 2),
    c("GMD", "Gambian dalasi", 2),
    c("GNF", "Guinean franc", 0),
    c("GTQ", "Guatemalan quetzal", 2),
    c("GYD", "Guyanese dollar", 2),
    c("HKD", "Hong Kong dollar", 2),
    c("HNL", "Honduran lempira", 2),
    c("HTG", "Haitian gourde", 2),
    c("HUF", "Hungarian forint", 2),
    c("IDR", "Indonesian rupiah", 2),
    c("ILS", "Israeli new shekel", 2),
    c("INR", "Indian rupee", 2),
    c("IQD", "Iraqi dinar", 3),
    c("IRR", "Iranian rial", 2),
    c("ISK", "Icelandic krona", 0),
    c("JMD", "Jamaican dollar", 2),
    c("JOD", "Jordanian dinar", 3),
    c("JPY", "Japanese yen", 0),
    c("KES", "Kenyan shilling", 2),
    c("KGS", "Kyrgyzstani som", 2),
    c("KHR", "Cambodian riel", 2),
    c("KMF", "Comorian franc", 0),
    c("KPW", "North Korean won", 2),
    c("KRW", "South Korean won", 0),
    c("KWD", "Kuwaiti dinar", 3),
    c("KYD", "Cayman Islands dollar", 2),
    c("KZT", "Kazakhstani tenge", 2),
    c("LAK", "Lao kip", 2),
    c("LBP", "Lebanese pound", 2),
    c("LKR", "Sri Lankan rupee", 2),
    c("LRD", "Liberian dollar", 2),
    c("LSL", "Lesotho loti", 2),
    c("LYD", "Libyan dinar", 3),
    c("MAD", "Moroccan dirham", 2),
    c("MDL", "Moldovan leu", 2),
    c("MGA", "Malagasy ariary", 2),
    c("MKD", "Macedonian denar", 2),
    c("MMK", "Myanmar kyat", 2),
    c("MNT", "Mongolian tugrik", 2),
    c("MOP", "Macanese pataca", 2),
    c("MRU", "Mauritanian ouguiya", 2),
    c("MUR", "Mauritian rupee", 2),
    c("MVR", "Maldivian rufiyaa", 2),
    c("MWK", "Malawian kwacha", 2),
    c("MXN", "Mexican peso", 2),
    c("MYR", "Malaysian ringgit", 2),
    c("MZN", "Mozambican metical", 2),
    c("NAD", "Namibian dollar", 2),
    c("NGN", "Nigerian naira", 2),
    c("NIO", "Nicaraguan cordoba", 2),
    c("NOK", "Norwegian krone", 2),
    c("NPR", "Nepalese rupee", 2),
    c("NZD", "New Zealand dollar", 2),
    c("OMR", "Omani rial", 3),
    c("PAB", "Panamanian balboa", 2),
    c("PEN", "Peruvian sol", 2),
    c("PGK", "Papua New Guinean kina", 2),
    c("PHP", "Philippine peso", 2),
    c("PKR", "Pakistani rupee", 2),
    c("PLN", "Polish zloty", 2),
    c("PYG", "Paraguayan guarani", 0),
    c("QAR", "Qatari riyal", 2),
    c("RON", "Romanian leu", 2),
    c("RSD", "Serbian dinar", 2),
    c("RUB", "Russian ruble", 2),
    c("RWF", "Rwandan franc", 0),
    c("SAR", "Saudi riyal", 2),
    c("SBD", "Solomon Islands dollar", 2),
    c("SCR", "Seychellois rupee", 2),
    c("SDG", "Sudanese pound", 2),
    c("SEK", "Swedish krona", 2),
    c("SGD", "Singapore dollar", 2),
    c("SHP", "Saint Helena pound", 2),
    c("SLE", "Sierra Leonean leone", 2),
    c("SOS", "Somali shilling", 2),
    c("SRD", "Surinamese dollar", 2),
    c("SSP", "South Sudanese pound", 2),
    c("STN", "Sao Tome and Principe dobra", 2),
    c("SVC", "Salvadoran colon", 2),
    c("SYP", "Syrian pound", 2),
    c("SZL", "Swazi lilangeni", 2),
    c("THB", "Thai baht", 2),
    c("TJS", "Tajikistani somoni", 2),
    c("TMT", "Turkmenistan manat", 2),
    c("TND", "Tunisian dinar", 3),
    c("TOP", "Tongan paanga", 2),
    c("TRY", "Turkish lira", 2),
    c("TTD", "Trinidad and Tobago dollar", 2),
    c("TWD", "New Taiwan dollar", 2),
    c("TZS", "Tanzanian shilling", 2),
    c("UAH", "Ukrainian hryvnia", 2),
    c("UGX", "Ugandan shilling", 0),
    c("USD", "United States dollar", 2),
    c("UYU", "Uruguayan peso", 2),
    c("UZS", "Uzbekistani sum", 2),
    c("VED", "Venezuelan digital bolivar", 2),
    c("VES", "Venezuelan bolivar soberano", 2),
    c("VND", "Vietnamese dong", 0),
    c("VUV", "Vanuatu vatu", 0),
    c("WST", "Samoan tala", 2),
    c("XAF", "Central African CFA franc", 0),
    c("XCD", "East Caribbean dollar", 2),
    c("XCG", "Caribbean guilder", 2),
    c("XOF", "West African CFA franc", 0),
    c("XPF", "CFP franc", 0),
    c("YER", "Yemeni rial", 2),
    c("ZAR", "South African rand", 2),
    c("ZMW", "Zambian kwacha", 2),
    c("ZWG", "Zimbabwe gold", 2),
];

/// Shorthand so the table above reads as a table.
const fn c(code: &'static str, name: &'static str, minor_units: u8) -> Currency {
    Currency {
        code,
        name,
        minor_units,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_code_in_any_case() {
        assert_eq!(Currency::parse("usd").unwrap(), Currency::USD);
        assert_eq!(Currency::parse("  USD  ").unwrap(), Currency::USD);
    }

    #[test]
    fn refuses_anything_that_is_not_a_code() {
        for bad in ["", "US", "USDD", "dollars", "$", "ZZZ", "XXX", "XAU"] {
            assert!(Currency::parse(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn the_minor_units_are_not_all_two() {
        // The whole reason this is a table and not a length check.
        assert_eq!(Currency::parse("JPY").unwrap().minor_units(), 0);
        assert_eq!(Currency::parse("USD").unwrap().minor_units(), 2);
        assert_eq!(Currency::parse("KWD").unwrap().minor_units(), 3);
    }

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        // Sorted because the picker renders it in this order, and a currency
        // out of place is a currency nobody finds.
        let mut previous = "";
        for currency in Currency::all() {
            assert!(
                currency.code() > previous,
                "{} is out of order or duplicated",
                currency.code(),
            );
            previous = currency.code();
        }
    }

    #[test]
    fn every_entry_is_a_plausible_iso_4217_row() {
        for currency in Currency::all() {
            assert_eq!(currency.code().len(), 3, "{}", currency.code());
            assert!(
                currency.code().bytes().all(|b| b.is_ascii_uppercase()),
                "{} is not upper-case ascii",
                currency.code(),
            );
            assert!(
                !currency.name().is_empty(),
                "{} has no name",
                currency.code()
            );
            assert!(currency.minor_units() <= 3, "{}", currency.code());
        }
    }

    #[test]
    fn the_default_is_in_the_table() {
        // `USD` is written out twice - once as the const, once as a row - so
        // this is what keeps the two saying the same thing.
        assert_eq!(Currency::parse("USD").unwrap(), Currency::USD);
        assert_eq!(Currency::default(), Currency::USD);
    }

    #[test]
    fn round_trips_through_json_as_a_bare_code() {
        let json = serde_json::to_string(&Currency::parse("JPY").unwrap()).unwrap();
        assert_eq!(json, "\"JPY\"");
        assert_eq!(
            serde_json::from_str::<Currency>(&json)
                .unwrap()
                .minor_units(),
            0,
        );
        assert!(serde_json::from_str::<Currency>("\"ZZZ\"").is_err());
    }
}
