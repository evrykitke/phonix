//! `master.parties` and the three tables that hang off it.
//!
//! # A party is read whole
//!
//! [`find`] fetches the party, its roles, its addresses and its contacts. Four
//! queries rather than one join, because a join across three one-to-many
//! children multiplies the rows and then has to be un-multiplied in Rust -
//! which is the kind of code that quietly loses the second address.
//!
//! # The list is a different question
//!
//! [`list`] reads one table plus an aggregate of the roles, because a grid
//! shows a name and a couple of badges. See [`crate::identity::user`] for the
//! same split between what a row *shows* and what a form *edits*.

use phonix_core::identity::UserId;
use phonix_core::locale::{Country, Currency};
use phonix_master::address::{AddressPurpose, PartyAddress, PostalAddress};
use phonix_master::contact::PartyContact;
use phonix_master::party::{Party, PartyInput, PartyKind, PartyRole, PartySummary};
use sqlx::{FromRow, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// The unique index that refuses two parties with one code.
///
/// Matched by name rather than by message text, which Postgres localises from
/// `lc_messages` and would stop matching the first time this was deployed to a
/// machine set to French.
const CODE_INDEX: &str = "parties_code_key";

/// Turn the unique-index violation into something a form can render.
fn as_code_conflict(err: sqlx::Error, code: &str) -> DbError {
    match &err {
        sqlx::Error::Database(db) if db.constraint() == Some(CODE_INDEX) => DbError::CodeExists {
            entity: "party",
            code: code.to_owned(),
        },
        _ => DbError::Query(err),
    }
}

// The column list is written out at each reader rather than shared through a
// constant. sqlx 0.9 accepts only `&'static str` as SQL - a deliberate refusal
// of runtime-built statements - so a shared list would have to be interpolated,
// and interpolating SQL to save three lines is the wrong trade.

/// A party row, before its children are attached.
struct PartyRow {
    id: Uuid,
    code: String,
    kind: PartyKind,
    name: String,
    legal_name: Option<String>,
    tax_id: Option<String>,
    country: Option<Country>,
    email: Option<String>,
    phone: Option<String>,
    website: Option<String>,
    currency: Option<Currency>,
    tax_group_id: Option<Uuid>,
    is_active: bool,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for PartyRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let stored_kind: String = row.try_get("kind")?;

        // Refused rather than defaulted. The kind decides whether a legal name
        // means anything and which name goes on a document, so guessing at it
        // puts the wrong name on an invoice.
        let kind = PartyKind::parse(&stored_kind).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "kind".to_owned(),
            source: format!("unrecognised party kind '{stored_kind}'").into(),
        })?;

        Ok(Self {
            id: row.try_get("id")?,
            code: row.try_get("code")?,
            kind,
            name: row.try_get("name")?,
            legal_name: row.try_get("legal_name")?,
            tax_id: row.try_get("tax_id")?,
            country: optional_country(row, "country_code")?,
            email: row.try_get("email")?,
            phone: row.try_get("phone")?,
            website: row.try_get("website")?,
            currency: optional_currency(row, "currency_code")?,
            tax_group_id: row.try_get("tax_group_id")?,
            is_active: row.try_get("is_active")?,
        })
    }
}

/// One list row: the party, plus the roles the apps have claimed.
struct SummaryRow(PartySummary);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for SummaryRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let party = PartyRow::from_row(row)?;
        let stored_roles: Vec<String> = row.try_get("roles")?;

        Ok(Self(PartySummary {
            id: party.id,
            code: party.code,
            kind: party.kind,
            name: party.name,
            country: party.country,
            email: party.email,
            phone: party.phone,
            currency: party.currency,
            is_active: party.is_active,
            roles: decode_roles(stored_roles),
        }))
    }
}

/// An address row.
///
/// A local wrapper rather than an implementation on [`PartyAddress`] itself:
/// `FromRow` belongs to sqlx and `PartyAddress` belongs to `phonix-master`, so
/// this crate may implement one for the other only through a type it owns. That
/// is the orphan rule, and it is the reason every read here is a newtype.
struct AddressRow(PartyAddress);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for AddressRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let stored_purpose: String = row.try_get("purpose")?;

        Ok(Self(PartyAddress {
            id: row.try_get("id")?,
            party_id: row.try_get("party_id")?,
            // Unlike a kind, a purpose this build does not know is an address
            // in the wrong section of a screen rather than a wrong amount.
            purpose: AddressPurpose::from_stored(&stored_purpose),
            label: row.try_get("label")?,
            address: PostalAddress {
                line1: row.try_get("line1")?,
                line2: row.try_get("line2")?,
                city: row.try_get("city")?,
                region: row.try_get("region")?,
                postal_code: row.try_get("postal_code")?,
                country: optional_country(row, "country_code")?,
            },
            is_primary: row.try_get("is_primary")?,
        }))
    }
}

/// A contact row. A local wrapper, for the reason [`AddressRow`] is.
struct ContactRow(PartyContact);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for ContactRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self(PartyContact {
            id: row.try_get("id")?,
            party_id: row.try_get("party_id")?,
            name: row.try_get("name")?,
            job_title: row.try_get("job_title")?,
            email: row.try_get("email")?,
            phone: row.try_get("phone")?,
            is_primary: row.try_get("is_primary")?,
        }))
    }
}

/// Every party, newest names last, for a grid.
///
/// Reads the whole table. A workspace's party list is master data - hundreds,
/// not millions - and the grid it feeds sorts, filters and exports in the
/// browser, which is what makes a search feel instant. When a workspace
/// outgrows that, the grid's [`Source`](phonix_core::query) already carries the
/// page request needed to move it server-side.
pub async fn list<'e, E>(executor: E, role: Option<&str>) -> Result<Vec<PartySummary>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, SummaryRow>(
        "SELECT p.id, p.code, p.kind, p.name, p.legal_name, p.tax_id, p.country_code,
                p.email, p.phone, p.website, p.currency_code, p.tax_group_id, p.is_active,
                COALESCE(
                    ARRAY(
                        SELECT r.role FROM master.party_roles r
                         WHERE r.party_id = p.id
                         ORDER BY r.role
                    ),
                    ARRAY[]::text[]
                ) AS roles
           FROM master.parties p
          WHERE $1::text IS NULL
             OR EXISTS (
                    SELECT 1 FROM master.party_roles r
                     WHERE r.party_id = p.id AND r.role = $1
                )
          ORDER BY lower(p.name)",
    )
    .bind(role)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// One party, whole: its roles, its addresses and its contacts.
///
/// Four round trips, taken deliberately - see the module note. Returns `None`
/// for a party that is not there rather than an error, because "not there" is
/// an answer a screen can render.
pub async fn find(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<Party>, DbError> {
    let Some(row) = sqlx::query_as::<_, PartyRow>(
        "SELECT id, code, kind, name, legal_name, tax_id, country_code,
                email, phone, website, currency_code, tax_group_id, is_active
           FROM master.parties
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)?
    else {
        return Ok(None);
    };

    let stored_roles: Vec<String> =
        sqlx::query_scalar("SELECT role FROM master.party_roles WHERE party_id = $1 ORDER BY role")
            .bind(id)
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;

    let addresses = addresses_of(pool, id).await?;
    let contacts = contacts_of(pool, id).await?;

    Ok(Some(Party {
        id: row.id,
        code: row.code,
        kind: row.kind,
        name: row.name,
        legal_name: row.legal_name,
        tax_id: row.tax_id,
        country: row.country,
        email: row.email,
        phone: row.phone,
        website: row.website,
        currency: row.currency,
        tax_group_id: row.tax_group_id,
        is_active: row.is_active,
        roles: decode_roles(stored_roles),
        addresses,
        contacts,
    }))
}

/// One party by its code, case-insensitively. What an import matches on.
pub async fn find_by_code<'e, E>(executor: E, code: &str) -> Result<Option<Uuid>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar("SELECT id FROM master.parties WHERE lower(code) = lower($1)")
        .bind(code)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

/// Create a party. Returns its id.
///
/// The roles are written separately by [`set_roles`], because they are a set
/// rather than a column and the caller usually has both to do.
pub async fn insert<'e, E>(
    executor: E,
    draft: &PartyInput,
    actor: Option<UserId>,
) -> Result<Uuid, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar(
        "INSERT INTO master.parties
             (code, kind, name, legal_name, tax_id, country_code, email, phone,
              website, currency_code, tax_group_id, is_active, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
         RETURNING id",
    )
    .bind(&draft.code)
    .bind(draft.kind.as_str())
    .bind(&draft.name)
    .bind(&draft.legal_name)
    .bind(&draft.tax_id)
    .bind(draft.country.map(Country::code))
    .bind(&draft.email)
    .bind(&draft.phone)
    .bind(&draft.website)
    .bind(draft.currency.map(Currency::code))
    .bind(draft.tax_group_id)
    .bind(draft.is_active)
    .bind(actor)
    .fetch_one(executor)
    .await
    .map_err(|err| as_code_conflict(err, &draft.code))
}

/// Store a changed party. `false` when there is no such row.
///
/// `updated_at` is set here, at the call site, because this crate keeps no
/// triggers - see `phonix_db`'s own documentation for why.
pub async fn update<'e, E>(
    executor: E,
    id: Uuid,
    draft: &PartyInput,
    actor: Option<UserId>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query(
        "UPDATE master.parties
            SET code          = $2,
                kind          = $3,
                name          = $4,
                legal_name    = $5,
                tax_id        = $6,
                country_code  = $7,
                email         = $8,
                phone         = $9,
                website       = $10,
                currency_code = $11,
                tax_group_id  = $12,
                is_active     = $13,
                updated_at    = now(),
                updated_by    = $14
          WHERE id = $1",
    )
    .bind(id)
    .bind(&draft.code)
    .bind(draft.kind.as_str())
    .bind(&draft.name)
    .bind(&draft.legal_name)
    .bind(&draft.tax_id)
    .bind(draft.country.map(Country::code))
    .bind(&draft.email)
    .bind(&draft.phone)
    .bind(&draft.website)
    .bind(draft.currency.map(Currency::code))
    .bind(draft.tax_group_id)
    .bind(draft.is_active)
    .bind(actor)
    .execute(executor)
    .await
    .map_err(|err| as_code_conflict(err, &draft.code))?
    .rows_affected();

    Ok(affected > 0)
}

/// Remove a party.
///
/// Nothing in `master` refers to a party except its own children, which
/// cascade. An app that has raised documents against one holds its id without a
/// foreign key - that is the no-cross-schema-FK rule - so the *service* is what
/// asks whether anything still points here. See
/// `phonix_services::master::party::delete`.
pub async fn delete<'e, E>(executor: E, id: Uuid) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query("DELETE FROM master.parties WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    Ok(affected > 0)
}

/// Replace the set of roles on a party.
///
/// The whole set, not a diff. Two administrators saving the same screen are
/// then idempotent rather than a race, which is the same argument the
/// permission editor makes - see `phonix_web::server_fns::admin_fns`.
pub async fn set_roles(pool: &sqlx::PgPool, id: Uuid, roles: &[PartyRole]) -> Result<(), DbError> {
    let names: Vec<String> = roles.iter().map(|role| role.as_str().to_owned()).collect();

    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    // Delete what is no longer claimed, then add what is new. Not a delete-all
    // and re-insert: `granted_at` is when an app first claimed this party, and
    // rewriting every row on every save would lose that.
    sqlx::query("DELETE FROM master.party_roles WHERE party_id = $1 AND role <> ALL($2)")
        .bind(id)
        .bind(&names)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    sqlx::query(
        "INSERT INTO master.party_roles (party_id, role)
         SELECT $1, unnest($2::text[])
         ON CONFLICT (party_id, role) DO NOTHING",
    )
    .bind(id)
    .bind(&names)
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    tx.commit().await.map_err(DbError::Query)
}

/// Claim a party for an app, leaving whatever else is claimed alone.
///
/// What an app calls when it is handed a party it did not create - a Books
/// invoice raised against a party Procurement added. Idempotent.
pub async fn claim_role<'e, E>(executor: E, id: Uuid, role: &PartyRole) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO master.party_roles (party_id, role)
         VALUES ($1, $2)
         ON CONFLICT (party_id, role) DO NOTHING",
    )
    .bind(id)
    .bind(role.as_str())
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Every address on a party, primaries first.
pub async fn addresses_of<'e, E>(executor: E, id: Uuid) -> Result<Vec<PartyAddress>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, AddressRow>(
        "SELECT id, party_id, purpose, label, line1, line2, city, region,
                postal_code, country_code, is_primary
           FROM master.party_addresses
          WHERE party_id = $1
          ORDER BY purpose, is_primary DESC, created_at",
    )
    .bind(id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// Add an address, or update the one whose id is given.
pub async fn save_address(
    pool: &sqlx::PgPool,
    party_id: Uuid,
    draft: &phonix_master::address::PartyAddressInput,
    actor: Option<UserId>,
) -> Result<Uuid, DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    let id = match draft.id {
        Some(id) => {
            sqlx::query(
                "UPDATE master.party_addresses
                    SET purpose      = $3,
                        label        = $4,
                        line1        = $5,
                        line2        = $6,
                        city         = $7,
                        region       = $8,
                        postal_code  = $9,
                        country_code = $10,
                        is_primary   = $11,
                        updated_at   = now(),
                        updated_by   = $12
                  WHERE id = $1 AND party_id = $2",
            )
            .bind(id)
            .bind(party_id)
            .bind(draft.purpose.as_str())
            .bind(&draft.label)
            .bind(&draft.address.line1)
            .bind(&draft.address.line2)
            .bind(&draft.address.city)
            .bind(&draft.address.region)
            .bind(&draft.address.postal_code)
            .bind(draft.address.country.map(Country::code))
            .bind(draft.is_primary)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(DbError::Query)?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO master.party_addresses
                 (party_id, purpose, label, line1, line2, city, region,
                  postal_code, country_code, is_primary, updated_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING id",
        )
        .bind(party_id)
        .bind(draft.purpose.as_str())
        .bind(&draft.label)
        .bind(&draft.address.line1)
        .bind(&draft.address.line2)
        .bind(&draft.address.city)
        .bind(&draft.address.region)
        .bind(&draft.address.postal_code)
        .bind(draft.address.country.map(Country::code))
        .bind(draft.is_primary)
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::Query)?,
    };

    // At most one primary per purpose, enforced here rather than by a partial
    // unique index: an index would refuse the save the moment somebody ticked
    // the new one before unticking the old, which is the order everybody does
    // it in.
    if draft.is_primary {
        sqlx::query(
            "UPDATE master.party_addresses
                SET is_primary = FALSE, updated_at = now(), updated_by = $4
              WHERE party_id = $1 AND purpose = $2 AND id <> $3 AND is_primary",
        )
        .bind(party_id)
        .bind(draft.purpose.as_str())
        .bind(id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(id)
}

/// Remove an address. `false` when it was not that party's.
pub async fn delete_address<'e, E>(
    executor: E,
    party_id: Uuid,
    address_id: Uuid,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected =
        sqlx::query("DELETE FROM master.party_addresses WHERE id = $1 AND party_id = $2")
            .bind(address_id)
            .bind(party_id)
            .execute(executor)
            .await
            .map_err(DbError::Query)?
            .rows_affected();

    Ok(affected > 0)
}

/// Every contact on a party, primaries first.
pub async fn contacts_of<'e, E>(executor: E, id: Uuid) -> Result<Vec<PartyContact>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, ContactRow>(
        "SELECT id, party_id, name, job_title, email, phone, is_primary
           FROM master.party_contacts
          WHERE party_id = $1
          ORDER BY is_primary DESC, lower(name)",
    )
    .bind(id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// Add a contact, or update the one whose id is given.
pub async fn save_contact(
    pool: &sqlx::PgPool,
    party_id: Uuid,
    draft: &phonix_master::contact::PartyContactInput,
    actor: Option<UserId>,
) -> Result<Uuid, DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    let id = match draft.id {
        Some(id) => {
            sqlx::query(
                "UPDATE master.party_contacts
                    SET name       = $3,
                        job_title  = $4,
                        email      = $5,
                        phone      = $6,
                        is_primary = $7,
                        updated_at = now(),
                        updated_by = $8
                  WHERE id = $1 AND party_id = $2",
            )
            .bind(id)
            .bind(party_id)
            .bind(&draft.name)
            .bind(&draft.job_title)
            .bind(&draft.email)
            .bind(&draft.phone)
            .bind(draft.is_primary)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(DbError::Query)?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO master.party_contacts
                 (party_id, name, job_title, email, phone, is_primary, updated_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id",
        )
        .bind(party_id)
        .bind(&draft.name)
        .bind(&draft.job_title)
        .bind(&draft.email)
        .bind(&draft.phone)
        .bind(draft.is_primary)
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::Query)?,
    };

    if draft.is_primary {
        sqlx::query(
            "UPDATE master.party_contacts
                SET is_primary = FALSE, updated_at = now(), updated_by = $3
              WHERE party_id = $1 AND id <> $2 AND is_primary",
        )
        .bind(party_id)
        .bind(id)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(id)
}

/// Remove a contact. `false` when it was not that party's.
pub async fn delete_contact<'e, E>(
    executor: E,
    party_id: Uuid,
    contact_id: Uuid,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query("DELETE FROM master.party_contacts WHERE id = $1 AND party_id = $2")
        .bind(contact_id)
        .bind(party_id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    Ok(affected > 0)
}

/// Turn stored role names into roles, dropping any this build cannot parse.
///
/// Dropped rather than refused: the vocabulary is open, so a name written by a
/// future app is a role this build has no opinion about, not a corrupt row. The
/// column's own CHECK is what keeps the shape.
fn decode_roles(stored: Vec<String>) -> Vec<PartyRole> {
    let mut roles: Vec<PartyRole> = stored
        .iter()
        .filter_map(|name| PartyRole::parse(name).ok())
        .collect();
    roles.sort();
    roles.dedup();
    roles
}

/// A country column that may be null, refused rather than defaulted.
fn optional_country(
    row: &sqlx::postgres::PgRow,
    column: &str,
) -> Result<Option<Country>, sqlx::Error> {
    let stored: Option<String> = row.try_get(column)?;
    stored
        .map(|code| {
            Country::parse(&code).map_err(|err| sqlx::Error::ColumnDecode {
                index: column.to_owned(),
                source: Box::new(err),
            })
        })
        .transpose()
}

/// A currency column that may be null.
///
/// Refused rather than defaulted, exactly as `core.currencies` does: a stored
/// code this build cannot resolve has no minor units, and an amount whose scale
/// is a guess is worse than a failed read.
fn optional_currency(
    row: &sqlx::postgres::PgRow,
    column: &str,
) -> Result<Option<Currency>, sqlx::Error> {
    let stored: Option<String> = row.try_get(column)?;
    stored
        .map(|code| {
            Currency::parse(&code).map_err(|err| sqlx::Error::ColumnDecode {
                index: column.to_owned(),
                source: Box::new(err),
            })
        })
        .transpose()
}
