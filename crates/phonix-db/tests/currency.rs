//! Currencies and exchange rates, against a live PostgreSQL server.
//!
//! One thing here is worth a live test more than the rest: **a rate crosses
//! into `NUMERIC(20, 10)` as text and comes back as text.** Every unit test in
//! `phonix-core` proves the arithmetic is exact, and none of them proves the
//! column does not quietly reshape it on the way past. A rate that loses its
//! tenth decimal place in transit is a tenth of a percent of error on every
//! foreign-currency invoice, and nothing errors when it happens.
//!
//! Ignored by default: these need a reachable server and the credentials in
//! `.env`. Run them deliberately.
//!
//! ```text
//! cargo test -p phonix-db --test currency -- --ignored --test-threads=1
//! ```

use chrono::NaiveDate;
use phonix_config::DatabaseConfig;
use phonix_core::locale::Currency;
use phonix_core::money::{ExchangeRate, Money, Rate, Rounding};
use phonix_db::currency;
use phonix_db::sqlx::{self, PgPool};
use phonix_db::tenancy::provision;

const DATABASE: &str = "phonix_test_currency";

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
}

fn code(raw: &str) -> Currency {
    Currency::parse(raw).expect("a currency in the compiled table")
}

fn day(year: i32, month: u32, of: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
}

/// A freshly created tenant database, migrated to the current schema.
///
/// Dropped and rebuilt rather than reused, so a failed run never poisons the
/// next one and each test starts from what a new workspace actually looks like.
async fn fresh(cfg: &DatabaseConfig) -> PgPool {
    provision::drop_tenant_database(cfg, DATABASE)
        .await
        .expect("drop scratch database");

    let mut conn = phonix_db::maintenance_connection(cfg)
        .await
        .expect("maintenance connection");
    let sql = format!(r#"CREATE DATABASE "{DATABASE}" ENCODING 'UTF8'"#);
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&mut conn)
        .await
        .expect("create scratch database");

    provision::migrate_tenant(cfg, DATABASE)
        .await
        .expect("migrate scratch database");

    phonix_db::tenant_pool(cfg, DATABASE)
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn the_base_currency_is_on_the_list_before_anybody_chooses_one() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    // 0010 seeds the profile at USD; 0015 seeds the list from the profile. So a
    // brand new workspace already has the currency its amounts are denominated
    // in, and no screen has to handle a base currency missing from its own
    // picker.
    let listed = currency::list(&pool).await.expect("list currencies");
    assert_eq!(
        listed.len(),
        1,
        "expected only the base currency, got {listed:?}"
    );

    let only = listed.first().expect("one row");
    assert_eq!(only.currency, Currency::USD);
    assert!(only.is_enabled);
    assert_eq!(only.symbol, None);

    pool.close().await;
    provision::drop_tenant_database(&cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_rate_keeps_all_ten_decimal_places_across_the_column() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let jpy = code("JPY");
    currency::upsert(&pool, jpy, true, Some("¥"), None)
        .await
        .expect("add JPY");

    // Ten significant places, the last of them a 1. If anything on the way
    // through is a float, or the scale is narrower than it claims, this is the
    // digit that disappears.
    let published = Rate::parse("0.0066841237").expect("a rate");
    let stored =
        ExchangeRate::new(jpy, Currency::USD, published, day(2026, 8, 20), "ecb").expect("a pair");

    currency::record_rate(&pool, &stored, None)
        .await
        .expect("record the rate");

    let read = currency::rate_on(&pool, jpy, Currency::USD, day(2026, 8, 24), None)
        .await
        .expect("look the rate up")
        .expect("a rate on file");

    assert_eq!(
        read, stored,
        "the row came back different from the one written"
    );
    assert_eq!(read.rate.to_storage_string(), "0.0066841237");

    // And the arithmetic that matters lands where it should: a million yen at
    // this rate is 6684.12 dollars, and it is 6700.00 if the rate was rounded
    // to four places on the way in.
    let converted = Money::parse(jpy, "1000000")
        .expect("an amount")
        .convert(&read, Rounding::HalfUp)
        .expect("convert");
    assert_eq!(converted.base_amount.to_display_string(), "6684.12");

    pool.close().await;
    provision::drop_tenant_database(&cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_lookup_walks_back_to_the_last_published_rate_and_no_further() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let eur = code("EUR");
    currency::upsert(&pool, eur, true, None, None)
        .await
        .expect("add EUR");

    for (date, rate) in [
        (day(2026, 8, 10), "1.0900"),
        (day(2026, 8, 17), "1.0925"),
        (day(2026, 8, 24), "1.0950"),
    ] {
        let published = ExchangeRate::new(
            eur,
            Currency::USD,
            Rate::parse(rate).expect("a rate"),
            date,
            "ecb",
        )
        .expect("a pair");
        currency::record_rate(&pool, &published, None)
            .await
            .expect("record the rate");
    }

    // Mid-week: the rate published on the Monday, not a blend of Monday and
    // the following Monday. An auditor asks which published rate was used.
    let midweek = currency::rate_on(&pool, eur, Currency::USD, day(2026, 8, 20), None)
        .await
        .expect("look up")
        .expect("a rate on file");
    assert_eq!(midweek.rate.to_string(), "1.0925");
    assert_eq!(midweek.as_of, day(2026, 8, 17));

    // Exactly on a publication date: that day's rate.
    let on_the_day = currency::rate_on(&pool, eur, Currency::USD, day(2026, 8, 24), None)
        .await
        .expect("look up")
        .expect("a rate on file");
    assert_eq!(on_the_day.rate.to_string(), "1.095");

    // Before the earliest one: nothing. Extrapolating backwards would be
    // inventing a quotation, so a document dated then has to be refused rather
    // than posted against a rate nobody published.
    let too_early = currency::rate_on(&pool, eur, Currency::USD, day(2026, 8, 1), None)
        .await
        .expect("look up");
    assert!(
        too_early.is_none(),
        "got {too_early:?} from before the first rate"
    );

    pool.close().await;
    provision::drop_tenant_database(&cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn re_running_a_feed_corrects_the_day_rather_than_doubling_it() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let gbp = code("GBP");
    currency::upsert(&pool, gbp, true, None, None)
        .await
        .expect("add GBP");

    let corrected = |rate: &str| {
        ExchangeRate::new(
            gbp,
            Currency::USD,
            Rate::parse(rate).expect("a rate"),
            day(2026, 8, 24),
            "ecb",
        )
        .expect("a pair")
    };

    currency::record_rate(&pool, &corrected("1.2700"), None)
        .await
        .expect("first run");
    currency::record_rate(&pool, &corrected("1.2750"), None)
        .await
        .expect("second run for the same day");

    let rates = currency::recent_rates(&pool, gbp, Currency::USD, 10)
        .await
        .expect("list rates");
    assert_eq!(
        rates.len(),
        1,
        "the same day from the same source became two rows: {rates:?}"
    );
    assert_eq!(
        rates.first().map(|rate| rate.rate.to_string()),
        Some("1.275".to_owned())
    );

    // A second source on the same day is a different fact, not a correction.
    let manual = ExchangeRate::new(
        gbp,
        Currency::USD,
        Rate::parse("1.2600").expect("a rate"),
        day(2026, 8, 24),
        "manual",
    )
    .expect("a pair");
    currency::record_rate(&pool, &manual, None)
        .await
        .expect("record a second source");

    let both = currency::recent_rates(&pool, gbp, Currency::USD, 10)
        .await
        .expect("list rates");
    assert_eq!(
        both.len(),
        2,
        "a second source should not overwrite the first"
    );

    // Pinned to one feed, the answer is that feed's.
    let pinned = currency::rate_on(&pool, gbp, Currency::USD, day(2026, 8, 24), Some("manual"))
        .await
        .expect("look up")
        .expect("a rate on file");
    assert_eq!(pinned.rate.to_string(), "1.26");

    pool.close().await;
    provision::drop_tenant_database(&cfg, DATABASE)
        .await
        .expect("clean up");
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn switching_a_currency_off_leaves_its_rates_resolvable() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let sek = code("SEK");
    currency::upsert(&pool, sek, true, None, None)
        .await
        .expect("add SEK");
    let published = ExchangeRate::new(
        sek,
        Currency::USD,
        Rate::parse("0.0952").expect("a rate"),
        day(2026, 8, 24),
        "ecb",
    )
    .expect("a pair");
    currency::record_rate(&pool, &published, None)
        .await
        .expect("record the rate");

    assert!(
        currency::set_enabled(&pool, sek, false, None)
            .await
            .expect("disable SEK")
    );

    // Gone from the picker...
    let offered = currency::enabled(&pool).await.expect("enabled currencies");
    assert!(
        offered.iter().all(|row| row.currency != sek),
        "a disabled currency is still being offered"
    );

    // ...and still resolvable, which is the point. A posted document dated
    // last month must not stop reading because somebody tidied a settings
    // screen this morning.
    let still_there = currency::rate_on(&pool, sek, Currency::USD, day(2026, 8, 24), None)
        .await
        .expect("look up")
        .expect("the rate survived");
    assert_eq!(still_there.rate.to_string(), "0.0952");

    // And a code nobody added is not a row.
    assert!(
        !currency::set_enabled(&pool, code("NZD"), true, None)
            .await
            .expect("update a currency that is not on the list")
    );

    pool.close().await;
    provision::drop_tenant_database(&cfg, DATABASE)
        .await
        .expect("clean up");
}
