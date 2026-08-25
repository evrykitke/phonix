//! Parties and taxes, against a live PostgreSQL server.
//!
//! Three things here are worth a live test more than the rest, because no unit
//! test can reach any of them:
//!
//! * **The exclusion constraint.** `phonix-tax` proves the arithmetic; only
//!   Postgres can prove that two rates for one code cannot be live at the same
//!   time. That is the constraint the whole effective-dated design rests on,
//!   and losing it would not fail anything else - a quarter would be filed
//!   first.
//! * **A rate crosses `NUMERIC(9, 6)` intact.** Six decimal places in and six
//!   out. A rate that loses its sixth place in transit is wrong on every
//!   document that uses it, and nothing errors when it happens.
//! * **`master` really is an ordinary app.** Its tables are reachable from a
//!   connection whose search path is `core,public`, because every statement
//!   qualifies them - and its foreign keys into `core` resolve.
//!
//! Ignored by default: these need a reachable server and the credentials in
//! `.env`. Run them deliberately.
//!
//! ```text
//! cargo test -p phonix-db --test master -- --ignored --test-threads=1
//! ```

use chrono::NaiveDate;
use phonix_config::DatabaseConfig;
use phonix_core::locale::Country;
use phonix_db::error::DbError;
use phonix_db::master::{party, tax};
use phonix_db::sqlx::{self, PgPool};
use phonix_db::tenancy::provision;
use phonix_master::party::{PartyInput, PartyKind, PartyRole, roles};
use phonix_tax::code::{TaxCodeInput, TaxKind};
use phonix_tax::group::TaxTreatment;
use phonix_tax::rate::{TaxRate, TaxRatePeriod};

const DATABASE: &str = "phonix_test_master";

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
}

fn day(year: i32, month: u32, of: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
}

/// A freshly created tenant database, migrated to the current schema.
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

async fn finish(cfg: &DatabaseConfig, pool: PgPool) {
    pool.close().await;
    provision::drop_tenant_database(cfg, DATABASE)
        .await
        .expect("clean up");
}

fn party_input(code: &str, name: &str) -> PartyInput {
    PartyInput {
        code: code.to_owned(),
        name: name.to_owned(),
        kind: PartyKind::Organization,
        country: Country::parse("KE").ok(),
        ..PartyInput::blank()
    }
}

fn tax_input(code: &str, name: &str) -> TaxCodeInput {
    TaxCodeInput {
        code: code.to_owned(),
        name: name.to_owned(),
        kind: TaxKind::Vat,
        ..TaxCodeInput::blank()
    }
}

fn rate(percent: &str, from: NaiveDate, to: Option<NaiveDate>) -> TaxRatePeriod {
    TaxRatePeriod {
        rate: TaxRate::parse_percent(percent).expect("a valid rate"),
        valid_from: from,
        valid_to: to,
    }
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_party_survives_the_columns_and_comes_back_whole() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let id = party::insert(&pool, &party_input("ACME01", "Acme Trading"), None)
        .await
        .expect("insert a party");

    let customer = PartyRole::parse(roles::CUSTOMER).expect("a valid role");
    let carrier = PartyRole::parse(roles::CARRIER).expect("a valid role");
    party::set_roles(&pool, id, &[customer.clone(), carrier.clone()])
        .await
        .expect("claim two roles");

    let stored = party::find(&pool, id)
        .await
        .expect("read the party")
        .expect("it is there");

    assert_eq!(stored.code, "ACME01");
    assert_eq!(stored.kind, PartyKind::Organization);
    assert_eq!(stored.country, Country::parse("KE").ok());
    // One party, two hats - the reason there is one table and not two.
    assert!(stored.has_role(roles::CUSTOMER));
    assert!(stored.has_role(roles::CARRIER));

    // And an app can find its own without knowing about the other's.
    let customers = party::list(&pool, Some(roles::CUSTOMER))
        .await
        .expect("list customers");
    assert_eq!(customers.len(), 1);
    let suppliers = party::list(&pool, Some(roles::SUPPLIER))
        .await
        .expect("list suppliers");
    assert!(suppliers.is_empty());

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn two_parties_cannot_share_a_code_however_it_is_capitalised() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    party::insert(&pool, &party_input("ACME01", "Acme Trading"), None)
        .await
        .expect("insert the first");

    // The index is on `lower(code)`: `acme01` and `ACME01` are the same
    // customer typed by two people, and letting both exist is how a statement
    // comes out halved.
    let clash = party::insert(&pool, &party_input("acme01", "Acme again"), None).await;

    assert!(
        matches!(clash, Err(DbError::CodeExists { .. })),
        "expected a code conflict a form can render, got {clash:?}"
    );

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_rate_keeps_all_six_decimal_places_across_the_column() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let code_id = tax::insert_code(&pool, &tax_input("SALESTX", "District sales tax"), None)
        .await
        .expect("insert a tax code");

    // 8.625% is 0.086250 - a published US district rate, and one that needs
    // more than four places to survive.
    let published = rate("8.625", day(2026, 1, 1), None);
    tax::save_rate(&pool, code_id, None, &published, None)
        .await
        .expect("record the rate");

    let read = tax::rate_on(&pool, code_id, day(2026, 6, 1))
        .await
        .expect("look the rate up")
        .expect("a rate is in force");

    assert_eq!(read.rate, published.rate);
    assert_eq!(read.rate.to_storage_string(), "0.086250");
    assert_eq!(read.rate.to_percent_string(), "8.625%");

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn two_rates_for_one_tax_can_never_be_live_at_once() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let code_id = tax::insert_code(&pool, &tax_input("VAT", "Value added tax"), None)
        .await
        .expect("insert a tax code");

    // 17.5% until April 2026, then open-ended.
    tax::save_rate(
        &pool,
        code_id,
        None,
        &rate("17.5", day(2024, 1, 1), Some(day(2026, 4, 1))),
        None,
    )
    .await
    .expect("the first window");

    tax::save_rate(
        &pool,
        code_id,
        None,
        &rate("20", day(2026, 4, 1), None),
        None,
    )
    .await
    .expect("the second window, which abuts the first");

    // The half-open window is what makes the changeover day belong to exactly
    // one row.
    let before = tax::rate_on(&pool, code_id, day(2026, 3, 31))
        .await
        .expect("look up")
        .expect("in force");
    let after = tax::rate_on(&pool, code_id, day(2026, 4, 1))
        .await
        .expect("look up")
        .expect("in force");
    assert_eq!(before.rate.to_percent_string(), "17.5%");
    assert_eq!(after.rate.to_percent_string(), "20%");

    // And the constraint this table exists for: a third window across the
    // boundary is refused by Postgres, not by Rust.
    let overlap = tax::save_rate(
        &pool,
        code_id,
        None,
        &rate("15", day(2026, 1, 1), Some(day(2027, 1, 1))),
        None,
    )
    .await;

    assert!(
        matches!(overlap, Err(DbError::TaxRateOverlap)),
        "two live rates must be impossible; got {overlap:?}"
    );

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_split_tax_resolves_into_a_snapshot_in_sequence_order() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    // India: GST 18% is CGST 9% plus SGST 9%, and the return needs them apart.
    // This is the case a single `rate` column on the code cannot express.
    let cgst = tax::insert_code(&pool, &tax_input("CGST", "Central GST"), None)
        .await
        .expect("cgst");
    let sgst = tax::insert_code(&pool, &tax_input("SGST", "State GST"), None)
        .await
        .expect("sgst");

    for id in [cgst, sgst] {
        tax::save_rate(&pool, id, None, &rate("9", day(2020, 1, 1), None), None)
            .await
            .expect("a rate");
    }

    // Deliberately built with SGST first in the members list, so the ordering
    // being read from `sequence` rather than from insertion order is what the
    // assertion below actually proves.
    let group_id = tax::save_group(
        &pool,
        tax::GroupWrite {
            id: None,
            code: "GST18",
            name: "GST 18%",
            country: Country::parse("IN").ok(),
            is_active: true,
            members: &[cgst, sgst],
        },
        None,
    )
    .await
    .expect("save the group");

    let group = tax::find_group(&pool, group_id)
        .await
        .expect("read the group")
        .expect("it is there");
    assert_eq!(group.members.len(), 2);

    let rates = tax::rates_on(&pool, day(2026, 6, 1))
        .await
        .expect("every rate in force on the day");

    let treatment = TaxTreatment::resolve(&group, day(2026, 6, 1), &|code_id| {
        rates
            .iter()
            .find(|(id, _)| *id == code_id)
            .map(|(_, period)| *period)
    })
    .expect("resolve the group");

    let order: Vec<&str> = treatment
        .taxes
        .iter()
        .map(|tax| tax.code.as_str())
        .collect();
    assert_eq!(order, vec!["CGST", "SGST"]);
    assert!(
        treatment
            .taxes
            .iter()
            .all(|tax| tax.rate.to_percent_string() == "9%")
    );

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_tax_that_is_in_a_group_cannot_be_deleted_out_from_under_it() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let code_id = tax::insert_code(&pool, &tax_input("VAT20", "VAT standard"), None)
        .await
        .expect("insert a tax code");
    tax::save_group(
        &pool,
        tax::GroupWrite {
            id: None,
            code: "STD",
            name: "Standard",
            country: None,
            is_active: true,
            members: &[code_id],
        },
        None,
    )
    .await
    .expect("put it in a group");

    // No cascade on `tax_group_members.tax_code_id`, on purpose: a group that
    // lost a member silently would change what every document using it comes
    // to. Retiring a tax is `is_active = false`.
    let refused = tax::delete_code(&pool, code_id).await;
    assert!(
        refused.is_err(),
        "a tax inside a group must not be deletable"
    );

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_partys_addresses_keep_one_primary_per_purpose() {
    use phonix_master::address::{AddressPurpose, PartyAddressInput, PostalAddress};

    let cfg = database_config();
    let pool = fresh(&cfg).await;

    let party_id = party::insert(&pool, &party_input("ACME01", "Acme Trading"), None)
        .await
        .expect("insert a party");

    let address = |city: &str, is_primary: bool| PartyAddressInput {
        address: PostalAddress {
            city: Some(city.to_owned()),
            ..PostalAddress::empty()
        },
        purpose: AddressPurpose::Billing,
        is_primary,
        ..PartyAddressInput::blank()
    };

    party::save_address(&pool, party_id, &address("Nairobi", true), None)
        .await
        .expect("the first address");
    party::save_address(&pool, party_id, &address("Mombasa", true), None)
        .await
        .expect("a second, also ticked primary");

    let stored = party::addresses_of(&pool, party_id)
        .await
        .expect("read the addresses");

    // Kept true by the service rather than by a partial unique index: an index
    // would refuse the save the moment somebody ticked the new one before
    // unticking the old, which is the order everybody does it in.
    let primaries: Vec<&str> = stored
        .iter()
        .filter(|a| a.is_primary)
        .filter_map(|a| a.address.city.as_deref())
        .collect();
    assert_eq!(primaries, vec!["Mombasa"]);
    assert_eq!(stored.len(), 2);

    finish(&cfg, pool).await;
}
