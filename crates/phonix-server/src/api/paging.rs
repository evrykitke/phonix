//! Asking for one page over HTTP, and what comes back.
//!
//! `PageRequest` is already the vocabulary the browser, the server functions
//! and the DBAL agree on. This is that vocabulary spelled as query parameters,
//! and nothing more:
//!
//! ```text
//! ?page=1&per_page=25&sort=code&order=asc&q=eur&filter[enabled]=true
//! ```
//!
//! # A bad parameter is clamped, never refused
//!
//! `page=0`, `per_page=1000000`, `sort=nonsense` and `order=sideways` all
//! produce a page rather than a 422. `PageRequest::sanitised` is where that
//! happens - one function, called by every reader - and an API that answers
//! "unprocessable entity" to `?page=0` gives a worse answer than the first
//! page. It also means a client written against a later version of a screen,
//! sorting by a column this build does not have, still gets its rows.
//!
//! # Why the query string is parsed here rather than by serde
//!
//! `filter[enabled]=true` is not a struct field. Deriving `Deserialize` over a
//! flattened map would collect the literal key `filter[enabled]`, which every
//! reader would then have to unwrap for itself. Parsing the pairs directly
//! costs one small function and keeps the bracket syntax in one place.

use std::collections::BTreeMap;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use phonix_core::query::{Page, PageRequest, Sort, SortDirection};
use serde::Serialize;
use utoipa::ToSchema;

use super::problem::Problem;

/// A `PageRequest` built from the query string.
#[derive(Debug, Clone)]
pub struct ListRequest(pub PageRequest);

impl<S> FromRequestParts<S> for ListRequest
where
    S: Send + Sync,
{
    // Infallible in practice - see the module note - but the trait wants a
    // rejection type, and `Problem` is what every other failure in this router
    // answers with.
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parse(parts.uri.query().unwrap_or_default())))
    }
}

/// The parameters, as documentation. Never deserialised into - see above.
#[derive(Debug, Clone, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(dead_code)]
pub struct ListParams {
    /// 1-based. Page zero is the first page.
    #[param(example = 1, minimum = 1)]
    pub page: Option<u32>,
    /// Rows per page. Above 500 is served as 500.
    #[param(example = 25, maximum = 500)]
    pub per_page: Option<u32>,
    /// A field to sort by. One this resource does not know is ignored.
    #[param(example = "code")]
    pub sort: Option<String>,
    /// `asc` or `desc`. Anything else reads as `asc`.
    #[param(example = "asc")]
    pub order: Option<String>,
    /// Free-text search.
    pub q: Option<String>,
}

/// Turn a raw query string into a sanitised request.
fn parse(query: &str) -> PageRequest {
    let mut request = PageRequest::default();
    let mut filters: BTreeMap<String, String> = BTreeMap::new();
    let mut sort_field: Option<String> = None;
    let mut direction = SortDirection::Ascending;

    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            // Unparseable is the same as absent: the default survives and
            // `sanitised` clamps whatever did arrive.
            "page" => {
                if let Ok(page) = value.parse() {
                    request.page = page;
                }
            }
            "per_page" => {
                if let Ok(per_page) = value.parse() {
                    request.per_page = per_page;
                }
            }
            "q" => request.search = value.into_owned(),
            "sort" => sort_field = Some(value.into_owned()),
            "order" => {
                if value.eq_ignore_ascii_case("desc") {
                    direction = SortDirection::Descending;
                }
            }
            other => {
                // `filter[name]=value`. Anything else in the query string is
                // somebody else's parameter and is left alone.
                if let Some(name) = other
                    .strip_prefix("filter[")
                    .and_then(|rest| rest.strip_suffix(']'))
                    && !name.is_empty()
                {
                    filters.insert(name.to_owned(), value.into_owned());
                }
            }
        }
    }

    request.sort = sort_field.map(|field| Sort { field, direction });
    request.filters = filters;

    request.sanitised()
}

/// Where a page sits in the whole list.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PageMeta {
    /// The page these rows are, which may not be the page asked for.
    #[schema(example = 1)]
    pub page: u32,
    #[schema(example = 25)]
    pub per_page: u32,
    /// How many rows matched, not how many exist.
    #[schema(example = 163)]
    pub total: u64,
    #[schema(example = 7)]
    pub page_count: u32,
}

/// A list, and where in it these rows are.
///
/// Rows and pager are separate keys because a client appending to a list has to
/// know whether there is more without counting. A *single* resource is answered
/// unwrapped: wrapping one record in `data` would buy symmetry and cost every
/// caller a dereference for the life of the version.
/// In the specification this appears once per resource, named after what it
/// holds - `PageEnvelope_Currency`. That name is part of the contract, which
/// is why the resource inside it is named `Currency` rather than
/// `CurrencyResource`: the Rust type says where the code lives, `#[schema(as =
/// ...)]` says what a client sees.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PageEnvelope<T> {
    pub data: Vec<T>,
    pub page: PageMeta,
}

impl<T> PageEnvelope<T> {
    /// Wrap a page of already-converted resources.
    pub fn new(page: Page<T>) -> Self {
        let meta = PageMeta {
            page: page.page,
            per_page: page.per_page,
            total: page.total,
            page_count: page.page_count(),
        };

        Self {
            data: page.rows,
            page: meta,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ordinary_query_maps_across() {
        let request = parse("page=3&per_page=10&q=eur&sort=code&order=desc");

        assert_eq!(request.page, 3);
        assert_eq!(request.per_page, 10);
        assert_eq!(request.search, "eur");
        assert_eq!(
            request.sort,
            Some(Sort::descending("code")),
            "order=desc has to reach the sort, not just the field"
        );
    }

    #[test]
    fn nonsense_is_clamped_rather_than_refused() {
        // Every one of these is a request somebody will make, and answering
        // 422 to any of them is worse than answering with rows.
        let request = parse("page=0&per_page=100000&order=sideways&sort=code");

        assert_eq!(request.page, 1);
        assert_eq!(request.per_page, phonix_core::query::MAX_PER_PAGE);
        assert_eq!(request.sort, Some(Sort::ascending("code")));
    }

    #[test]
    fn an_unparseable_number_leaves_the_default_standing() {
        let request = parse("page=three&per_page=");

        assert_eq!(request.page, 1);
        assert_eq!(request.per_page, phonix_core::query::DEFAULT_PER_PAGE);
    }

    #[test]
    fn bracketed_filters_arrive_under_their_own_names() {
        let request = parse("filter[enabled]=true&filter[bucket]=avatar&other=ignored");

        assert_eq!(request.filter("enabled"), Some("true"));
        assert_eq!(request.filter("bucket"), Some("avatar"));
        // Not a filter, and not this router's business either.
        assert_eq!(request.filter("other"), None);
    }

    #[test]
    fn an_empty_filter_is_the_all_of_them_choice() {
        // `sanitised` drops it, so no reader has to check for the empty string
        // as well as for absence.
        let request = parse("filter[enabled]=");

        assert_eq!(request.filter("enabled"), None);
    }

    #[test]
    fn a_malformed_bracket_is_left_alone() {
        let request = parse("filter[=x&filter]=y&filter[]=z");

        assert!(request.filters.is_empty());
    }
}
