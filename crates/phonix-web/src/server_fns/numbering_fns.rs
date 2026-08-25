//! Document number series: what they look like, and where they have got to.
//!
//! # Previewing is not allocating
//!
//! There is no endpoint here that hands out a number. Allocation belongs in the
//! transaction that inserts the document it numbers - see
//! `phonix_services::numbering` - and a server function that returned "the next
//! invoice number" would promise something the save might not keep.
//!
//! What this file does offer is the *format* rendered against a sample counter,
//! which is a different act and entirely safe. It runs on the server rather
//! than in the browser for one reason: `{FY}` reads the organization's fiscal
//! year opening, and a preview that guessed at that would disagree with the
//! number actually issued.

use leptos::prelude::*;
use phonix_core::numbering::{NumberSeries, SeriesSaved, SeriesSettings};

/// Every series this workspace has, in app and document-type order.
#[server(name = ListNumberSeries, prefix = "/api", endpoint = "numbering")]
pub async fn list_number_series() -> Result<Vec<NumberSeries>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::numbering::list(&pool, &caller, None)
        .await
        .map_err(service_error)
}

/// Store a changed series.
///
/// Comes back as a [`SeriesSaved`] rather than a `Result`, because two of its
/// four outcomes are things a form has to render next to a field: a mask that
/// does not parse, and a format change that would reissue a number.
#[server(name = SaveNumberSeries, prefix = "/api", endpoint = "numbering/save")]
pub async fn save_number_series(settings: SeriesSettings) -> Result<SeriesSaved, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    phonix_services::numbering::apply_settings(&pool, &caller, &settings)
        .await
        .map_err(service_error)
}

/// What a mask would produce, against a sample counter.
///
/// Safe to show at any time, unlike a real number: it renders a sample rather
/// than taking one. Returns the message the parser gave when the mask does not
/// parse, so the box can say what is wrong with what was typed.
#[server(name = PreviewNumberFormat, prefix = "/api", endpoint = "numbering/preview")]
pub async fn preview_number_format(
    pattern: String,
    scope_key: String,
) -> Result<Result<String, String>, ServerFnError> {
    use phonix_core::numbering::Pattern;
    use phonix_services::numbering::NumberGenerator;

    use crate::state::{pool_and_caller, service_error};

    let (pool, _) = pool_and_caller().await?;

    let parsed = match Pattern::parse(&pattern) {
        Ok(parsed) => parsed,
        Err(err) => return Ok(Err(err.to_string())),
    };

    // Through the generator rather than through `Pattern::preview` directly, so
    // the fiscal year the preview shows is provably the one the posting path
    // would use.
    let generator = NumberGenerator::open(&pool).await.map_err(service_error)?;
    let today = chrono::Utc::now().date_naive();

    Ok(Ok(generator.preview(&parsed, today, &scope_key)))
}
