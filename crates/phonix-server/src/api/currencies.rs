//! `/api/v1/currencies` - the first resource, and the one that proves the rest.
//!
//! Chosen because it exercises every decision in the ADR and invents nothing:
//! reading is ungated (so a key with no scopes at all still gets a 200, which
//! is how the authentication path is proved end to end), writing requires
//! `Pages.Administration.Settings` (so a scoped key proves the gate), and the
//! service already audits the change.
//!
//! # The wire type is not the row
//!
//! [`CurrencyResource`] is declared here and converted from
//! `phonix_core::locale::WorkspaceCurrency` by hand. That is the rule the whole
//! version depends on: an internal rename has to stop this file compiling
//! rather than silently change a published payload. It also keeps `name` and
//! `minor_units` in the response - they come from the compiled ISO table, not
//! from the workspace's row, and a client formatting an amount needs them.

use axum::Json;
use axum::http::StatusCode;
use phonix_core::locale::Currency;
use phonix_core::money::WorkspaceCurrency;
use phonix_core::query::{Page, PageRequest};
use phonix_services::currency;
use serde::Deserialize;
use utoipa::ToSchema;

use super::auth::ApiCaller;
use super::json::ApiJson;
use super::paging::{ListParams, ListRequest, PageEnvelope, cut};
use super::path::ApiPath;
use super::problem::Problem;

/// A currency this workspace deals in.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = Currency)]
pub struct CurrencyResource {
    /// ISO 4217, upper case.
    #[schema(example = "EUR")]
    pub code: String,
    /// The currency's name, from the compiled ISO table rather than the
    /// workspace's row - so it is the same in every workspace.
    #[schema(example = "Euro")]
    pub name: String,
    /// Decimal places. 0 for the yen, 2 for the euro, 3 for the dinar. What a
    /// client needs to render an amount without guessing.
    #[schema(example = 2)]
    pub minor_units: u8,
    /// What this workspace prints, when it has an opinion. `null` means use
    /// the code.
    #[schema(example = "€")]
    pub symbol: Option<String>,
    /// Whether a picker in this workspace offers it. A currency is switched
    /// off rather than deleted, because posted documents still have to
    /// resolve.
    pub enabled: bool,
}

impl From<&WorkspaceCurrency> for CurrencyResource {
    fn from(row: &WorkspaceCurrency) -> Self {
        Self {
            code: row.currency.code().to_owned(),
            name: row.currency.name().to_owned(),
            minor_units: row.currency.minor_units(),
            symbol: row.symbol.clone(),
            enabled: row.is_enabled,
        }
    }
}

/// What `PUT /currencies/{code}` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = CurrencySave)]
pub struct SaveCurrency {
    /// Whether pickers should offer it.
    pub enabled: bool,
    /// What to print. Omit or send `null` to fall back to the code.
    #[schema(example = "€")]
    #[serde(default)]
    pub symbol: Option<String>,
}

/// Every currency on this workspace's list.
///
/// Ungated, exactly as the screen's read is: every page with an amount on it
/// needs this list, so requiring a permission would mean granting the
/// administration area to anybody who can raise a document.
///
/// Sorts by `code` (the default), `name` or `enabled`. Narrows on
/// `filter[enabled]`.
#[utoipa::path(
    get,
    path = "/currencies",
    tag = "currencies",
    operation_id = "listCurrencies",
    params(ListParams),
    responses(
        (status = 200, description = "One page of the workspace's currencies", body = PageEnvelope<CurrencyResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The workspace has no API access", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<CurrencyResource>>, Problem> {
    let rows = currency::list(&caller.pool).await?;

    Ok(Json(PageEnvelope::new(paginate(rows, &request))))
}

/// One currency, by ISO code.
#[utoipa::path(
    get,
    path = "/currencies/{code}",
    tag = "currencies",
    operation_id = "getCurrency",
    params(("code" = String, Path, description = "ISO 4217 code", example = "EUR")),
    responses(
        (status = 200, description = "The currency", body = CurrencyResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 404, description = "Not on this workspace's list", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get(
    caller: ApiCaller,
    ApiPath(code): ApiPath<String>,
) -> Result<Json<CurrencyResource>, Problem> {
    let currency = parse_code(&code).ok_or_else(|| missing(&code))?;
    let rows = currency::list(&caller.pool).await?;

    rows.iter()
        .find(|row| row.currency == currency)
        .map(|row| Json(CurrencyResource::from(row)))
        .ok_or_else(|| missing(&code))
}

/// Add a currency to the workspace's list, or change how it is shown.
///
/// Idempotent: "use EUR, printed as €" is a statement about the end state, so
/// sending it twice is not an error. Requires
/// `Pages.Administration.Settings` - refused by the service, not by this
/// handler.
#[utoipa::path(
    put,
    path = "/currencies/{code}",
    tag = "currencies",
    operation_id = "saveCurrency",
    params(("code" = String, Path, description = "ISO 4217 code", example = "EUR")),
    request_body = SaveCurrency,
    responses(
        (status = 200, description = "The currency as it now stands", body = CurrencyResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "The body was understood and refused", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn save(
    caller: ApiCaller,
    ApiPath(code): ApiPath<String>,
    ApiJson(input): ApiJson<SaveCurrency>,
) -> Result<Json<CurrencyResource>, Problem> {
    let currency = parse_code(&code).ok_or_else(|| missing(&code))?;

    // An empty string is not a symbol; it is the absence of one, and letting it
    // through would store '' where every reader expects NULL.
    let symbol = input
        .symbol
        .as_deref()
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty());

    currency::save(
        &caller.pool,
        &caller.caller,
        currency,
        input.enabled,
        symbol,
    )
    .await?;

    // The audit trail records the change and who made it; this records which
    // credential it arrived on, which is the question asked when a key turns
    // out to have been somewhere it should not have been.
    tracing::info!(
        // `None` when a person did this from their own signed-in session
        // rather than a script; the audit trail names them either way.
        key = ?caller.key_id,
        currency = currency.code(),
        "currency saved through the api"
    );

    // Read back rather than echoing the request: what was stored is what the
    // caller should see, and the two differ the moment a rule trims something.
    let rows = currency::list(&caller.pool).await?;

    rows.iter()
        .find(|row| row.currency == currency)
        .map(|row| Json(CurrencyResource::from(row)))
        .ok_or_else(|| missing(&code))
}

/// An ISO code, if it is one at all.
///
/// A code this build does not know answers 404 rather than 422, and the two
/// call sites spell that themselves: `/currencies/ZZZ` is an address with
/// nothing at it, and a client walking a list of codes should get the same
/// answer for "not a currency" as for "not one this workspace uses".
fn parse_code(code: &str) -> Option<Currency> {
    Currency::parse(code).ok()
}

fn missing(code: &str) -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("{code} is not on this workspace's currency list."),
    )
}

/// Search, narrow, sort and cut one page - in memory, and deliberately.
///
/// The list is bounded by ISO 4217: a workspace that used every currency there
/// has ever been would have under two hundred rows, and it is read whole on
/// every screen that shows an amount. Paging it in SQL would be three
/// statements to save nothing.
///
/// A resource pages here only while the service it calls hands back the whole
/// list for its own reasons. The moment one of them grows a `PageRequest`
/// parameter, its handler passes the request down instead - the envelope and
/// the query contract do not change either way, which is the point of
/// [`super::paging`] owning the tail rather than each resource owning all of
/// it.
fn paginate(rows: Vec<WorkspaceCurrency>, request: &PageRequest) -> Page<CurrencyResource> {
    let needle = request.needle();

    let mut matching: Vec<&WorkspaceCurrency> = rows
        .iter()
        .filter(|row| match &needle {
            Some(needle) => {
                row.currency.code().to_lowercase().contains(needle)
                    || row.currency.name().to_lowercase().contains(needle)
            }
            None => true,
        })
        .filter(|row| match request.filter("enabled") {
            Some("true") => row.is_enabled,
            Some("false") => !row.is_enabled,
            // A narrowing this reader does not recognise narrows nothing, for
            // the reason `PageRequest::filter` gives.
            _ => true,
        })
        .collect();

    let descending = request
        .sort
        .as_ref()
        .is_some_and(|sort| !sort.direction.is_ascending());

    match request.sort.as_ref().map(|sort| sort.field.as_str()) {
        Some("name") => matching.sort_by_key(|row| row.currency.name()),
        Some("enabled") => matching.sort_by_key(|row| (row.is_enabled, row.currency.code())),
        // Code is the default and the tie-break: two rows that compare equal
        // under any other sort would otherwise swap places between one page
        // and the next, which reads as a row appearing twice.
        _ => matching.sort_by_key(|row| row.currency.code()),
    }
    if descending {
        matching.reverse();
    }

    cut(matching, request, CurrencyResource::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(code: &str, enabled: bool) -> WorkspaceCurrency {
        WorkspaceCurrency {
            currency: Currency::parse(code).expect("a real code"),
            is_enabled: enabled,
            symbol: None,
        }
    }

    fn listed() -> Vec<WorkspaceCurrency> {
        vec![
            row("EUR", true),
            row("USD", true),
            row("JPY", false),
            row("GBP", true),
        ]
    }

    #[test]
    fn the_default_page_is_sorted_by_code() {
        let page = paginate(listed(), &PageRequest::default());

        let codes: Vec<&str> = page.rows.iter().map(|row| row.code.as_str()).collect();
        assert_eq!(codes, vec!["EUR", "GBP", "JPY", "USD"]);
        assert_eq!(page.total, 4);
    }

    #[test]
    fn a_search_looks_at_the_name_as_well_as_the_code() {
        let request = PageRequest {
            search: "yen".to_owned(),
            ..PageRequest::default()
        };

        let page = paginate(listed(), &request.sanitised());

        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].code, "JPY");
    }

    #[test]
    fn the_enabled_filter_narrows_both_ways() {
        let on = paginate(
            listed(),
            &PageRequest::default().filtered_by("enabled", "true"),
        );
        let off = paginate(
            listed(),
            &PageRequest::default().filtered_by("enabled", "false"),
        );

        assert_eq!(on.total, 3);
        assert_eq!(off.total, 1);
        assert_eq!(off.rows[0].code, "JPY");
    }

    #[test]
    fn a_filter_this_resource_does_not_know_narrows_nothing() {
        let page = paginate(
            listed(),
            &PageRequest::default().filtered_by("colour", "green"),
        );

        assert_eq!(page.total, 4);
    }

    #[test]
    fn a_page_past_the_end_comes_back_clamped_rather_than_empty() {
        let request = PageRequest {
            page: 99,
            ..PageRequest::first(2)
        };

        let page = paginate(listed(), &request.sanitised());

        // Four rows, two per page: the last page is the second one, and that
        // is what a pager showing "page 99" needs to be told.
        assert_eq!(page.page, 2);
        assert_eq!(page.rows.len(), 2);
    }

    #[test]
    fn the_minor_units_come_from_the_compiled_table() {
        let page = paginate(listed(), &PageRequest::default());
        let jpy = page
            .rows
            .iter()
            .find(|row| row.code == "JPY")
            .expect("JPY is listed");

        // The workspace's row says nothing about scale, and a client that
        // guessed 2 here would render every yen amount a hundred times small.
        assert_eq!(jpy.minor_units, 0);
        assert_eq!(jpy.name, "Japanese yen");
    }
}
