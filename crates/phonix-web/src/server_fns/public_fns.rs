//! What a signed-out screen needs to know about the deployment it is part of.
//!
//! One call, made once by the chrome that wraps every public screen. Public in
//! the strongest sense: it takes no session, checks no permission, and returns
//! nothing a visitor could not read off the page it decorates.
//!
//! # Why the browser cannot work any of this out
//!
//! Every field is configuration, and configuration is a server-side fact:
//!
//! * The product name and the environment come from `[app]`.
//! * The footer links come from `[app.links]`, and typically point at a
//!   marketing site this application does not serve.
//! * The workspace suffix comes from `[server]`. It was a literal
//!   `".localhost:3000"` in the signup markup, which was true on one machine
//!   and a promise of a broken address everywhere else.
//!
//! The browser knows its own host, which is *not* the same thing: on the bare
//! domain the host has no workspace label to strip, and in production the
//! signup host and the tenancy root are two different names.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// The deployment, as a signed-out visitor sees it.
///
/// Every link is `Option` and every one defaults to absent. A footer link with
/// an empty `href` reloads the page, which is a strange thing for something
/// labelled "Privacy" to do, and a link to a page this deployment does not
/// serve is worse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicBranding {
    /// `app.name`. A name, so it is never translated.
    pub product: String,
    /// What the badge says, or empty for no badge at all.
    ///
    /// Decided by `AppSection::badge`, not by this crate: an explicit
    /// `app.public_label` when one is set, otherwise the environment name
    /// unless it is production. A test box running with production hardening
    /// sets the label and says so.
    pub environment: String,
    /// What every workspace address ends in, e.g. `.example.com`.
    pub workspace_suffix: String,
    pub privacy_url: Option<String>,
    pub terms_url: Option<String>,
    pub support_url: Option<String>,
    /// The product or company site, which the footer wordmark points at.
    pub website_url: Option<String>,
}

impl PublicBranding {
    /// Whether the footer has any links to draw a row for.
    pub fn has_links(&self) -> bool {
        self.privacy_url.is_some() || self.terms_url.is_some() || self.support_url.is_some()
    }

    /// What to show before the real answer arrives.
    ///
    /// Not a guess at the configuration - a shape with nothing in it. The
    /// chrome renders the same either way, so the top bar and footer do not
    /// appear a beat after the page they frame.
    pub fn unknown() -> Self {
        Self {
            product: String::new(),
            environment: String::new(),
            workspace_suffix: String::new(),
            privacy_url: None,
            terms_url: None,
            support_url: None,
            website_url: None,
        }
    }
}

/// Read the deployment's public identity.
#[server(name = ReadPublicBranding, prefix = "/api", endpoint = "branding")]
pub async fn public_branding() -> Result<PublicBranding, ServerFnError> {
    use crate::state::app_state;

    let state = app_state()?;
    let config = &state.config;

    // The final production deployment says nothing. Anything else names
    // itself, because the single most expensive mistake with two
    // identical-looking copies of an application is not knowing which one is
    // in front of you - and that is *more* true, not less, of a test box
    // deliberately running with production's hardening.
    let environment = config.app.badge().unwrap_or_default().to_owned();

    Ok(PublicBranding {
        product: config.app.name.clone(),
        environment,
        workspace_suffix: config.server.workspace_suffix(),
        privacy_url: config.app.links.privacy().map(str::to_owned),
        terms_url: config.app.links.terms().map(str::to_owned),
        support_url: config.app.links.support().map(str::to_owned),
        website_url: config.app.links.website().map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_configured_means_no_link_row() {
        assert!(!PublicBranding::unknown().has_links());
    }

    #[test]
    fn one_configured_link_is_enough_for_a_row() {
        for branding in [
            PublicBranding {
                privacy_url: Some("https://example.com/p".to_owned()),
                ..PublicBranding::unknown()
            },
            PublicBranding {
                terms_url: Some("https://example.com/t".to_owned()),
                ..PublicBranding::unknown()
            },
            PublicBranding {
                support_url: Some("https://example.com/s".to_owned()),
                ..PublicBranding::unknown()
            },
        ] {
            assert!(branding.has_links());
        }
    }

    #[test]
    fn the_wordmark_link_does_not_draw_a_link_row_on_its_own() {
        // It is the wordmark's destination, not one of the three text links,
        // so a deployment that sets only this gets no separator row.
        let branding = PublicBranding {
            website_url: Some("https://example.com".to_owned()),
            ..PublicBranding::unknown()
        };
        assert!(!branding.has_links());
    }
}
