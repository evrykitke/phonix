//! Tax codes, their effective-dated rates, and the groups a document line
//! points at.
//!
//! # One permission over three tables
//!
//! `Master.Taxes.Edit` gates codes, rates and groups alike. They are one act:
//! adding a tax means giving it a rate and putting it in a group, and a grant
//! that allowed two of the three would leave a code nothing can reach.
//!
//! # The rate history is audited against the code
//!
//! "VAT went from 17.5% to 20% on that date, and this is who entered it" is one
//! story. Splitting it across a `tax_code` trail and a `tax_rate` trail is how
//! the second half stops being read, so a rate change is recorded as a change
//! to the code, with the windows on each side.
//!
//! # Resolving is not computing
//!
//! [`treatment_on`] turns a group and a date into the snapshot a document
//! keeps. It does no arithmetic - that is `phonix_tax::compute`, which takes
//! the snapshot and never sees a database.

use chrono::NaiveDate;
use phonix_core::form::Submission;
use phonix_core::msg;
use phonix_core::permissions;
use phonix_db::error::DbError;
use phonix_db::master::tax::{self as store, GroupWrite};
use phonix_db::sqlx::PgPool;
use phonix_tax::code::{TaxCode, TaxCodeInput, TaxCodeSummary};
use phonix_tax::group::{TaxGroup, TaxGroupInput, TaxTreatment};
use phonix_tax::rate::{TaxRateInput, TaxRatePeriod, TaxRateRow};
use uuid::Uuid;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};

// --- codes --------------------------------------------------------------

/// Every tax code this workspace has defined.
pub async fn list_codes(pool: &PgPool, caller: &Caller) -> ServiceResult<Vec<TaxCode>> {
    caller.require(permissions::TAXES)?;
    Ok(store::list_codes(pool).await?)
}

/// Every tax code, with what each is charged at on `on`.
///
/// Two queries rather than a join, and rather than one query per code: the
/// exclusion constraint guarantees at most one live window per code, so
/// [`rates_on`](phonix_db::master::tax::rates_on) returns them all in a single
/// round trip and the two are zipped here.
///
/// `on` is passed in rather than read from the clock, so a screen showing "what
/// applies today" and a preview showing "what would apply on the document date"
/// are the same function.
pub async fn list_codes_on(
    pool: &PgPool,
    caller: &Caller,
    on: NaiveDate,
) -> ServiceResult<Vec<TaxCodeSummary>> {
    caller.require(permissions::TAXES)?;

    let codes = store::list_codes(pool).await?;
    let rates = store::rates_on(pool, on).await?;

    Ok(codes
        .into_iter()
        .map(|code| TaxCodeSummary {
            rate_today: rates
                .iter()
                .find(|(id, _)| *id == code.id)
                .map(|(_, period)| period.rate),
            code,
        })
        .collect())
}

/// One tax code.
pub async fn find_code(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<TaxCode> {
    caller.require(permissions::TAXES)?;

    store::find_code(pool, id)
        .await?
        .ok_or_else(|| ServiceError::rejected("tax", msg!("error.tax.gone")))
}

/// The editable part of one code, for the form to open on.
pub async fn edit_code(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<TaxCodeInput> {
    Ok(TaxCodeInput::from_code(&find_code(pool, caller, id).await?))
}

/// Create a tax code, or change one that exists.
pub async fn save_code(
    pool: &PgPool,
    caller: &Caller,
    draft: TaxCodeInput,
) -> ServiceResult<Submission<TaxCodeInput>> {
    caller.require(permissions::TAXES_EDIT)?;
    acting_user(caller)?;

    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected(err.field(), err.message())),
    };

    match checked.id {
        None => {
            let id = match store::insert_code(pool, &checked, caller.user_id()).await {
                Ok(id) => id,
                Err(DbError::CodeExists { code, .. }) => return Ok(code_taken(&code)),
                Err(err) => return Err(err.into()),
            };

            let stored = TaxCodeInput {
                id: Some(id),
                ..checked
            };

            audit::created(
                pool,
                caller,
                Target::new(kinds::TAX_CODE, id)
                    .named(&stored.name)
                    .fact("code", &stored.code)
                    // A tax with no rate charges nothing and refuses every
                    // document that references it, so saying so here stops the
                    // trail reading as though the tax was ready.
                    .fact("rates", "none until a rate is added"),
                &stored,
            )
            .await;

            Ok(Submission::Saved(stored))
        }
        Some(id) => {
            let before = match store::find_code(pool, id).await? {
                Some(code) => TaxCodeInput::from_code(&code),
                None => return Ok(Submission::rejected("tax", msg!("error.tax.gone"))),
            };

            match store::update_code(pool, id, &checked, caller.user_id()).await {
                Ok(true) => {}
                Ok(false) => return Ok(Submission::rejected("tax", msg!("error.tax.gone"))),
                Err(DbError::CodeExists { code, .. }) => return Ok(code_taken(&code)),
                Err(err) => return Err(err.into()),
            }

            audit::updated(
                pool,
                caller,
                Target::new(kinds::TAX_CODE, id).named(&checked.name),
                &before,
                &checked,
            )
            .await;

            Ok(Submission::Saved(checked))
        }
    }
}

/// Remove a tax code.
///
/// Postgres refuses it the moment the code is in a group, and that refusal is
/// turned into a sentence here rather than left as a constraint name. Retiring
/// a tax is `is_active = false`: a group that lost a member silently would
/// change what every document using it comes to.
pub async fn delete_code(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<()> {
    caller.require(permissions::TAXES_EDIT)?;
    acting_user(caller)?;

    let Some(code) = store::find_code(pool, id).await? else {
        return Err(ServiceError::rejected("tax", msg!("error.tax.gone")));
    };

    // Asked before the delete rather than after the failure, so the answer
    // names the groups instead of naming a constraint.
    let in_use = store::list_groups(pool)
        .await?
        .into_iter()
        .find(|group| group.members.iter().any(|m| m.tax_code_id == id));

    if let Some(group) = in_use {
        return Err(ServiceError::rejected(
            "tax",
            msg!("error.tax.in_group", group = &group.name),
        ));
    }

    if !store::delete_code(pool, id).await? {
        return Err(ServiceError::rejected("tax", msg!("error.tax.gone")));
    }

    audit::deleted(
        pool,
        caller,
        Target::new(kinds::TAX_CODE, id)
            .named(&code.name)
            .fact("code", &code.code),
        &TaxCodeInput::from_code(&code),
    )
    .await;

    Ok(())
}

// --- rates --------------------------------------------------------------

/// Every rate window on one code, newest first.
pub async fn rates_of(
    pool: &PgPool,
    caller: &Caller,
    tax_code_id: Uuid,
) -> ServiceResult<Vec<TaxRateRow>> {
    caller.require(permissions::TAXES)?;
    Ok(store::rates_of(pool, tax_code_id).await?)
}

/// Add a rate window, or move one that exists.
///
/// An overlap comes back as a rejected field rather than an error: two people
/// filing the same rate change on the same afternoon is an expected path
/// through a form, and the exclusion constraint is what settles the race.
pub async fn save_rate(
    pool: &PgPool,
    caller: &Caller,
    tax_code_id: Uuid,
    rate_id: Option<Uuid>,
    draft: TaxRateInput,
) -> ServiceResult<Submission<TaxRateInput>> {
    caller.require(permissions::TAXES_EDIT)?;
    acting_user(caller)?;

    let period = match draft.parse() {
        Ok(period) => period,
        Err(err) => return Ok(Submission::rejected("percent", err.message())),
    };

    let Some(code) = store::find_code(pool, tax_code_id).await? else {
        return Ok(Submission::rejected("tax", msg!("error.tax.gone")));
    };
    let before = store::rates_of(pool, tax_code_id).await?;

    match store::save_rate(pool, tax_code_id, rate_id, &period, caller.user_id()).await {
        Ok(_) => {}
        Err(DbError::TaxRateOverlap) => {
            return Ok(Submission::rejected(
                "valid_from",
                msg!("error.tax.rate_overlap"),
            ));
        }
        Err(err) => return Err(err.into()),
    }

    let after = store::rates_of(pool, tax_code_id).await?;

    // Against the code, not against the rate row - see the module note.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::TAX_CODE, tax_code_id)
            .named(&code.name)
            .fact("rate", period.rate.to_percent_string()),
        &recorded(&before),
        &recorded(&after),
    )
    .await;

    Ok(Submission::Saved(draft))
}

/// Remove a rate window.
pub async fn delete_rate(
    pool: &PgPool,
    caller: &Caller,
    tax_code_id: Uuid,
    rate_id: Uuid,
) -> ServiceResult<()> {
    caller.require(permissions::TAXES_EDIT)?;
    acting_user(caller)?;

    let Some(code) = store::find_code(pool, tax_code_id).await? else {
        return Err(ServiceError::rejected("tax", msg!("error.tax.gone")));
    };
    let before = store::rates_of(pool, tax_code_id).await?;

    if !store::delete_rate(pool, tax_code_id, rate_id).await? {
        return Err(ServiceError::rejected("rate", msg!("error.tax.rate_gone")));
    }

    let after = store::rates_of(pool, tax_code_id).await?;
    audit::updated(
        pool,
        caller,
        Target::new(kinds::TAX_CODE, tax_code_id).named(&code.name),
        &recorded(&before),
        &recorded(&after),
    )
    .await;

    Ok(())
}

// --- groups -------------------------------------------------------------

/// Every group, with its members in sequence order.
pub async fn list_groups(pool: &PgPool, caller: &Caller) -> ServiceResult<Vec<TaxGroup>> {
    caller.require(permissions::TAXES)?;
    Ok(store::list_groups(pool).await?)
}

/// One group.
pub async fn find_group(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<TaxGroup> {
    caller.require(permissions::TAXES)?;

    store::find_group(pool, id)
        .await?
        .ok_or_else(|| ServiceError::rejected("group", msg!("error.tax_group.gone")))
}

/// The editable part of one group.
pub async fn edit_group(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<TaxGroupInput> {
    Ok(TaxGroupInput::from_group(
        &find_group(pool, caller, id).await?,
    ))
}

/// Create a group, or change one that exists.
///
/// The membership arrives as the whole ordered list, not a diff, for the reason
/// the permission editor submits the whole set: a diff computed in a browser is
/// a diff against whatever that tab loaded, and applying it silently undoes
/// somebody else's change.
pub async fn save_group(
    pool: &PgPool,
    caller: &Caller,
    draft: TaxGroupInput,
) -> ServiceResult<Submission<TaxGroupInput>> {
    caller.require(permissions::TAXES_EDIT)?;
    acting_user(caller)?;

    let checked = match draft.check() {
        Ok(checked) => checked,
        Err(err) => return Ok(Submission::rejected("members", err.message())),
    };

    // Every member has to be a code this workspace actually has. Checked here
    // rather than left to the foreign key, so the answer names the tax rather
    // than the constraint.
    let known: Vec<Uuid> = store::list_codes(pool)
        .await?
        .into_iter()
        .map(|code| code.id)
        .collect();
    if checked.members.iter().any(|id| !known.contains(id)) {
        return Ok(Submission::rejected(
            "members",
            msg!("error.tax.unknown_member"),
        ));
    }

    let before = match checked.id {
        Some(id) => store::find_group(pool, id).await?.map(|g| {
            let mut input = TaxGroupInput::from_group(&g);
            input.id = Some(id);
            input
        }),
        None => None,
    };

    let id = match store::save_group(
        pool,
        GroupWrite {
            id: checked.id,
            code: &checked.code,
            name: &checked.name,
            country: checked.country,
            is_active: checked.is_active,
            members: &checked.members,
        },
        caller.user_id(),
    )
    .await
    {
        Ok(id) => id,
        Err(DbError::CodeExists { code, .. }) => {
            return Ok(Submission::rejected(
                "code",
                msg!("error.tax.code_taken", code = &code),
            ));
        }
        Err(err) => return Err(err.into()),
    };

    let stored = TaxGroupInput {
        id: Some(id),
        ..checked
    };

    match before {
        Some(before) => {
            audit::updated(
                pool,
                caller,
                Target::new(kinds::TAX_GROUP, id).named(&stored.name),
                &before,
                &stored,
            )
            .await;
        }
        None => {
            audit::created(
                pool,
                caller,
                Target::new(kinds::TAX_GROUP, id)
                    .named(&stored.name)
                    .fact("taxes", stored.members.len()),
                &stored,
            )
            .await;
        }
    }

    Ok(Submission::Saved(stored))
}

/// Remove a group.
///
/// Documents keep the snapshot they already resolved, and a party pointing at
/// it is left with no default rather than a dangling one - both of which the
/// foreign keys arrange. What is lost is the ability to raise a *new* document
/// against it, which is what deleting one means.
pub async fn delete_group(pool: &PgPool, caller: &Caller, id: Uuid) -> ServiceResult<()> {
    caller.require(permissions::TAXES_EDIT)?;
    acting_user(caller)?;

    let Some(group) = store::find_group(pool, id).await? else {
        return Err(ServiceError::rejected(
            "group",
            msg!("error.tax_group.gone"),
        ));
    };

    if !store::delete_group(pool, id).await? {
        return Err(ServiceError::rejected(
            "group",
            msg!("error.tax_group.gone"),
        ));
    }

    audit::deleted(
        pool,
        caller,
        Target::new(kinds::TAX_GROUP, id)
            .named(&group.name)
            .fact("code", &group.code),
        &TaxGroupInput::from_group(&group),
    )
    .await;

    Ok(())
}

// --- resolution ---------------------------------------------------------

/// Turn a group and a date into the snapshot a document keeps.
///
/// One round trip for the rates rather than one per member: a group is up to
/// eight codes, and asking eight times to price one line is eight round trips
/// per document.
///
/// Ungated on purpose. This is not a screen - it is what an app calls while
/// pricing a document, and the app has already checked that the caller may
/// raise one. Requiring `Master.Taxes` here would mean everybody who can write
/// an invoice must also be allowed to edit the tax tables.
pub async fn treatment_on(
    pool: &PgPool,
    tax_group_id: Uuid,
    on: NaiveDate,
) -> ServiceResult<TaxTreatment> {
    let Some(group) = store::find_group(pool, tax_group_id).await? else {
        return Err(ServiceError::rejected(
            "group",
            msg!("error.tax_group.gone"),
        ));
    };

    if !group.is_active {
        return Err(ServiceError::rejected(
            "group",
            msg!("error.tax_group.inactive", name = &group.name),
        ));
    }

    let rates = store::rates_on(pool, on).await?;

    TaxTreatment::resolve(&group, on, &|code_id| {
        rates
            .iter()
            .find(|(id, _)| *id == code_id)
            .map(|(_, period)| *period)
    })
    .map_err(|err| ServiceError::rejected("group", err.message()))
}

/// Every active group, resolved against a date.
///
/// What a document screen fetches **once**, so the browser can price every line
/// with no further round trips: `app_books::pricing` takes these and computes
/// the totals locally with the same code the server posts with.
///
/// One query for the groups and one for the rates, rather than two per group.
///
/// A group whose member has no rate on that date is **left out** rather than
/// failing the call. The screen should offer what it can price; a single
/// unpriced tax must not stop somebody raising an invoice that does not use it.
/// [`treatment_on`] is the one that refuses, and it runs when a line actually
/// references the group.
pub async fn treatments_on(pool: &PgPool, on: NaiveDate) -> ServiceResult<Vec<TaxTreatment>> {
    let groups = store::list_groups(pool).await?;
    let rates = store::rates_on(pool, on).await?;

    Ok(groups
        .iter()
        .filter(|group| group.is_active)
        .filter_map(|group| {
            TaxTreatment::resolve(group, on, &|code_id| {
                rates
                    .iter()
                    .find(|(id, _)| *id == code_id)
                    .map(|(_, period)| *period)
            })
            .ok()
        })
        .collect())
}

/// The rate windows, in the shape the audit diff should record them.
///
/// The stored row's id is dropped: a rate window's identity is its dates, and
/// including a uuid would make every re-save look like a change.
fn recorded(rates: &[TaxRateRow]) -> Vec<TaxRatePeriod> {
    rates.iter().map(|row| row.period).collect()
}

fn code_taken(code: &str) -> Submission<TaxCodeInput> {
    Submission::rejected("code", msg!("error.tax.code_taken", code = code))
}
