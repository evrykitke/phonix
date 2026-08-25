//! Sales invoices, against a live PostgreSQL server.
//!
//! Four things here are worth a live test more than the rest:
//!
//! * **A rollback returns the number.** This is the whole reason `allocate`
//!   takes a `&mut PgConnection` and the whole reason posting is one
//!   transaction. No unit test can reach it: it is a property of a Postgres row
//!   lock, not of any Rust type.
//! * **The document survives its columns.** Quantities at four places, amounts
//!   at four places and rates at six, through `NUMERIC` and back. A digit lost
//!   in transit is an invoice that is quietly wrong and errors nowhere.
//! * **The belt-and-braces index.** The sequence in `core` is what makes
//!   numbering gap-free; the unique index in `books` is what stops a duplicate
//!   reaching the ledger if somebody edits a series by hand.
//! * **A posted invoice cannot be edited**, in the database rather than only in
//!   the service.
//!
//! ```text
//! cargo test -p phonix-db --test books -- --ignored --test-threads=1
//! ```

use app_books::invoice::{CheckedInvoice, CheckedLine, InvoiceStatus, PartySnapshot};
use app_books::pricing::{PricedInvoice, PricedLine};
use app_books::quantity::Quantity;
use chrono::NaiveDate;
use phonix_config::DatabaseConfig;
use phonix_core::locale::Currency;
use phonix_core::money::{Money, Rounding};
use phonix_db::books::invoice as store;
use phonix_db::books::invoice::{DraftWrite, InvoiceFilter};
use phonix_db::error::DbError;
use phonix_db::master::{party, tax};
use phonix_db::numbering::{self, SequenceKey};
use phonix_db::sqlx::{self, PgPool};
use phonix_db::tenancy::provision;
use phonix_master::address::PostalAddress;
use phonix_master::party::{PartyInput, PartyKind};
use phonix_tax::code::{TaxCodeInput, TaxKind};
use phonix_tax::compute::{Pricing, RoundingLevel};
use phonix_tax::group::TaxTreatment;
use phonix_tax::rate::{TaxRate, TaxRatePeriod};
use uuid::Uuid;

const DATABASE: &str = "phonix_test_books";

fn database_config() -> DatabaseConfig {
    phonix_config::load()
        .expect("config loads; these tests read the same .env the server does")
        .database
}

fn usd() -> Currency {
    Currency::parse("USD").expect("a real currency")
}

fn money(amount: &str) -> Money {
    Money::parse(usd(), amount).expect("a valid amount")
}

fn day(year: i32, month: u32, of: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
}

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

/// A customer, a 20% VAT group, and the ids they got.
async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
    let party_id = party::insert(
        pool,
        &PartyInput {
            code: "ACME01".to_owned(),
            name: "Acme".to_owned(),
            legal_name: Some("Acme Trading Limited".to_owned()),
            kind: PartyKind::Organization,
            ..PartyInput::blank()
        },
        None,
    )
    .await
    .expect("a customer");

    let code_id = tax::insert_code(
        pool,
        &TaxCodeInput {
            code: "VAT20".to_owned(),
            name: "VAT standard rate".to_owned(),
            kind: TaxKind::Vat,
            ..TaxCodeInput::blank()
        },
        None,
    )
    .await
    .expect("a tax code");

    tax::save_rate(
        pool,
        code_id,
        None,
        &TaxRatePeriod {
            rate: TaxRate::parse_percent("20").expect("a valid rate"),
            valid_from: day(2020, 1, 1),
            valid_to: None,
        },
        None,
    )
    .await
    .expect("a rate");

    let group_id = tax::save_group(
        pool,
        tax::GroupWrite {
            id: None,
            code: "STD",
            name: "Standard rate",
            country: None,
            is_active: true,
            members: &[code_id],
        },
        None,
    )
    .await
    .expect("a tax group");

    (party_id, group_id)
}

fn snapshot(party_id: Uuid) -> PartySnapshot {
    PartySnapshot {
        party_id,
        code: "ACME01".to_owned(),
        // The *registered* name: an invoice names the entity, not the trading
        // style.
        name: "Acme Trading Limited".to_owned(),
        tax_id: None,
        address: PostalAddress {
            city: Some("Mombasa".to_owned()),
            ..PostalAddress::empty()
        },
    }
}

/// A treatment resolved the way the service resolves one.
async fn treatment(pool: &PgPool, group_id: Uuid, on: NaiveDate) -> TaxTreatment {
    let group = tax::find_group(pool, group_id)
        .await
        .expect("read the group")
        .expect("it is there");
    let rates = tax::rates_on(pool, on).await.expect("rates in force");

    TaxTreatment::resolve(&group, on, &|code_id| {
        rates
            .iter()
            .find(|(id, _)| *id == code_id)
            .map(|(_, period)| *period)
    })
    .expect("resolve the group")
}

/// A one-line draft: three at 19.99, standard rate.
async fn draft(pool: &PgPool, party_id: Uuid, group_id: Uuid) -> (CheckedInvoice, PricedInvoice) {
    let issued_on = day(2026, 6, 1);
    let quantity = Quantity::parse("3").expect("a valid quantity");
    let unit_price = money("19.99");

    let checked = CheckedInvoice {
        id: None,
        party_id,
        issued_on,
        due_on: Some(day(2026, 7, 1)),
        currency: usd(),
        pricing: Pricing::Exclusive,
        rounding_level: RoundingLevel::Line,
        rounding: Rounding::HalfUp,
        notes: Some("Thank you.".to_owned()),
        lines: vec![CheckedLine {
            id: None,
            description: "Consulting".to_owned(),
            quantity,
            unit_price,
            tax_group_id: Some(group_id),
        }],
    };

    let priced = PricedInvoice {
        currency: usd(),
        pricing: checked.pricing,
        rounding_level: checked.rounding_level,
        rounding: checked.rounding,
        lines: vec![PricedLine {
            quantity,
            unit_price,
            treatment: treatment(pool, group_id, issued_on).await,
        }],
    };

    (checked, priced)
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_draft_survives_its_columns_and_comes_back_whole() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let (party_id, group_id) = seed(&pool).await;
    let (checked, priced) = draft(&pool, party_id, group_id).await;
    let totals = priced.compute().expect("price the invoice");

    let id = store::save_draft(
        &pool,
        DraftWrite {
            checked: &checked,
            party: &snapshot(party_id),
            priced: &totals,
            actor: None,
        },
    )
    .await
    .expect("store the draft");

    let stored = store::find(&pool, id)
        .await
        .expect("read it back")
        .expect("it is there");

    assert_eq!(stored.status, InvoiceStatus::Draft);
    // A draft carries no number - the whole point of allocating at post.
    assert_eq!(stored.number, None);
    assert_eq!(stored.party.name, "Acme Trading Limited");

    // 3 x 19.99 = 59.97, 20% of which is 11.994 -> 11.99.
    assert_eq!(stored.totals.net, money("59.97"));
    assert_eq!(stored.totals.tax, money("11.99"));
    assert_eq!(stored.totals.gross, money("71.96"));

    let line = stored.lines.first().expect("one line");
    assert_eq!(line.line_no, 1);
    assert_eq!(line.quantity, Quantity::parse("3").unwrap());
    assert_eq!(line.unit_price, money("19.99"));

    // The tax snapshot: this is what makes a 2030 reprint show 2026's rate.
    let tax = line.taxes.first().expect("one tax");
    assert_eq!(tax.applied.code, "VAT20");
    assert_eq!(tax.applied.name, "VAT standard rate");
    assert_eq!(tax.applied.rate.to_percent_string(), "20%");
    assert_eq!(tax.taxable, money("59.97"));
    assert_eq!(tax.amount, money("11.99"));

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn posting_takes_the_next_number_from_the_series_the_app_declared() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let (party_id, group_id) = seed(&pool).await;
    let (checked, priced) = draft(&pool, party_id, group_id).await;
    let totals = priced.compute().expect("price the invoice");

    let id = store::save_draft(
        &pool,
        DraftWrite {
            checked: &checked,
            party: &snapshot(party_id),
            priced: &totals,
            actor: None,
        },
    )
    .await
    .expect("store the draft");

    // The series came from `config/numbering/books.toml`, installed by the
    // runner. Nothing in this test created it.
    let mut tx = pool.begin().await.expect("open the document's transaction");
    let key = SequenceKey::new("books", "sales_invoice");
    let allocated = numbering::allocate(&mut tx, key, checked.issued_on, 1)
        .await
        .expect("take a number");

    assert_eq!(
        allocated.number, "INV-2026-00001",
        "the mask in books.toml is not what reached the document"
    );

    assert!(
        store::post(&mut tx, id, &allocated.number, None, None, None)
            .await
            .expect("store the number")
    );
    tx.commit().await.expect("commit the post");

    let posted = store::find(&pool, id)
        .await
        .expect("read it back")
        .expect("it is there");

    assert_eq!(posted.status, InvoiceStatus::Posted);
    assert_eq!(posted.number.as_deref(), Some("INV-2026-00001"));
    assert!(posted.posted_at.is_some());

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_failed_post_returns_the_number_rather_than_burning_it() {
    // The whole reason posting is one transaction, and the reason `allocate`
    // takes a connection rather than a pool. No unit test can reach this: it is
    // a property of a Postgres row lock.
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let (party_id, group_id) = seed(&pool).await;
    let (checked, priced) = draft(&pool, party_id, group_id).await;
    let totals = priced.compute().expect("price the invoice");

    let id = store::save_draft(
        &pool,
        DraftWrite {
            checked: &checked,
            party: &snapshot(party_id),
            priced: &totals,
            actor: None,
        },
    )
    .await
    .expect("store the draft");

    let key = SequenceKey::new("books", "sales_invoice");

    // A post that takes a number and then fails.
    let mut tx = pool.begin().await.expect("open a transaction");
    let burned = numbering::allocate(&mut tx, key, checked.issued_on, 1)
        .await
        .expect("take a number");
    assert_eq!(burned.number, "INV-2026-00001");
    tx.rollback().await.expect("the post fails");

    // And the next one gets the same number, because the first was returned.
    let mut tx = pool.begin().await.expect("open the real transaction");
    let allocated = numbering::allocate(&mut tx, key, checked.issued_on, 1)
        .await
        .expect("take a number");
    assert_eq!(
        allocated.number, "INV-2026-00001",
        "a rolled-back post burned a number and left a gap"
    );

    assert!(
        store::post(&mut tx, id, &allocated.number, None, None, None)
            .await
            .expect("store the number")
    );
    tx.commit().await.expect("commit");

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn a_posted_invoice_cannot_be_edited_by_the_database() {
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let (party_id, group_id) = seed(&pool).await;
    let (checked, priced) = draft(&pool, party_id, group_id).await;
    let totals = priced.compute().expect("price the invoice");

    let id = store::save_draft(
        &pool,
        DraftWrite {
            checked: &checked,
            party: &snapshot(party_id),
            priced: &totals,
            actor: None,
        },
    )
    .await
    .expect("store the draft");

    let mut tx = pool.begin().await.expect("open a transaction");
    let allocated = numbering::allocate(
        &mut tx,
        SequenceKey::new("books", "sales_invoice"),
        checked.issued_on,
        1,
    )
    .await
    .expect("take a number");
    store::post(&mut tx, id, &allocated.number, None, None, None)
        .await
        .expect("post it");
    tx.commit().await.expect("commit");

    // `WHERE status = 'draft'` is what makes this true of the database rather
    // than only of the service above it.
    let edited = store::save_draft(
        &pool,
        DraftWrite {
            checked: &CheckedInvoice {
                id: Some(id),
                ..checked.clone()
            },
            party: &snapshot(party_id),
            priced: &totals,
            actor: None,
        },
    )
    .await;

    assert!(
        matches!(edited, Err(DbError::InvoiceNotEditable)),
        "a posted invoice must not be editable; got {edited:?}"
    );

    // Nor deletable. Voiding is what withdraws one, and it keeps the number.
    assert!(
        !store::delete_draft(&pool, id)
            .await
            .expect("attempt the delete")
    );
    assert!(store::void(&pool, id, None).await.expect("void it"));

    let voided = store::find(&pool, id)
        .await
        .expect("read it back")
        .expect("it is there");
    assert_eq!(voided.status, InvoiceStatus::Voided);
    assert_eq!(
        voided.number.as_deref(),
        Some("INV-2026-00001"),
        "a voided invoice keeps its number - a number that disappears is a gap"
    );

    finish(&cfg, pool).await;
}

#[tokio::test]
#[ignore = "needs a live PostgreSQL server"]
async fn two_invoices_can_never_share_a_number() {
    // Belt and braces. The sequence in core is what makes numbering gap-free;
    // this index is what stops a duplicate reaching the ledger if somebody
    // edits a series by hand.
    let cfg = database_config();
    let pool = fresh(&cfg).await;
    let (party_id, group_id) = seed(&pool).await;
    let (checked, priced) = draft(&pool, party_id, group_id).await;
    let totals = priced.compute().expect("price the invoice");

    // Bound rather than built inline: `DraftWrite` borrows the snapshot, so a
    // closure returning one would be returning a reference to its own
    // temporary.
    let party = snapshot(party_id);
    let write = DraftWrite {
        checked: &checked,
        party: &party,
        priced: &totals,
        actor: None,
    };

    let first = store::save_draft(&pool, write.clone())
        .await
        .expect("the first draft");
    let second = store::save_draft(&pool, write)
        .await
        .expect("the second draft");

    let mut tx = pool.begin().await.expect("open a transaction");
    store::post(&mut tx, first, "INV-2026-00001", None, None, None)
        .await
        .expect("post the first");
    tx.commit().await.expect("commit");

    let mut tx = pool.begin().await.expect("open a transaction");
    let clash = store::post(&mut tx, second, "INV-2026-00001", None, None, None).await;

    assert!(
        clash.is_err(),
        "two invoices shared a number; the unique index is gone"
    );
    tx.rollback().await.ok();

    // And two drafts are fine, because both carry NULL - which is why the
    // index has to be partial.
    let drafts = store::list(
        &pool,
        InvoiceFilter {
            status: Some(InvoiceStatus::Draft),
            ..InvoiceFilter::default()
        },
    )
    .await
    .expect("list the drafts");
    assert_eq!(drafts.len(), 1, "the unposted draft went missing");

    finish(&cfg, pool).await;
}
