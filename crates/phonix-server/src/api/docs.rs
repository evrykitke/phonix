//! The page at `/api/v1/docs`, and why it is not the one the crate ships.
//!
//! `utoipa-scalar`'s default template loads `@scalar/api-reference` from
//! jsdelivr with no version in the URL. That is a page which is blank without
//! internet, and which executes whatever that CDN answers with today inside
//! our own origin - on the one page a customer's developer reads *before* they
//! decide to trust us. So the bundle is vendored into `public/` by
//! `node tools/vendor-scalar.mjs` and served from the site root, exactly as the
//! editor bundle is; see [`super::scalar_bundle`].
//!
//! Nothing else about the template changes. The `$spec` placeholder is what
//! `Scalar::to_html` substitutes the serialised document into, so it has to
//! survive verbatim.

use super::scalar_bundle::SCALAR_SRC;

/// The template, with the vendored bundle's hashed path baked in.
///
/// A `String` rather than a `&'static str` because the path carries a content
/// hash that is only known at compile time through a constant, and
/// `custom_html` takes anything that becomes a `Cow`.
pub fn template() -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
    <title>Phonix API</title>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1"/>
    <link rel="icon" href="/favicon.svg"/>
</head>
<body>
<script id="api-reference" type="application/json">
    $spec
</script>
<script src="{SCALAR_SRC}"></script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Scalar::to_html` substitutes the document into this placeholder. Lose
    /// it and the page renders an empty reference with no error anywhere.
    #[test]
    fn keeps_the_spec_placeholder() {
        assert!(template().contains("$spec"));
    }

    /// The whole point of the file. A CDN host here is the bug this module
    /// exists to prevent, and it would come back the first time somebody
    /// copies the upstream template.
    #[test]
    fn loads_the_bundle_from_this_origin() {
        let html = template();
        assert!(html.contains(SCALAR_SRC), "the vendored bundle is not linked");
        assert!(!html.contains("//cdn."), "the page still reaches for a CDN");
    }
}
