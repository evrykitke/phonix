//! The frame around every page, and the rule that keeps it honest.
//!
//! Desk's markup is in `crates/phonix-desk/templates`, not in this file.
//! Askama compiles those `.html` files into the binary, so there is still one
//! artefact to deploy and nothing is read from disk at request time - but the
//! markup is HTML, in files an editor understands, rather than `format!`
//! strings in Rust.
//!
//! # Why that swap was worth making
//!
//! The version this replaces escaped by hand, and its own doc comment admitted
//! what that costs: `esc` was applied at every interpolation because "this one
//! is safe" is a judgement that has to be right every time or not made at all.
//! Askama escapes `.html` output by default. The judgement is now made only
//! where a template writes `|safe`, and none of Desk's do - which is a property
//! you can check by grepping ten files rather than by reading every handler.
//!
//! Pages compose by inheritance for the same reason. A `Page { body: String }`
//! would need `{{ body|safe }}` in the frame and would have reintroduced the
//! hole one level up; `{% extends %}` has no such seam.
//!
//! # Everything else is unchanged
//!
//! No wasm, no script, no external asset. **Every page must be complete without
//! JavaScript** - not "degrades gracefully", complete. Tabs are sections
//! stacked in order, a detail link is an ordinary `<a href>`, collapsing is
//! `<details>`, every action is a `<form method="post">`. See
//! `docs/adr/0005-phonix-desk.md` section 3 for why: a wasm panic freezes every
//! handler on a page at once, and the moment Desk is wanted is the moment the
//! product is misbehaving.
//!
//! # English only
//!
//! Desk has no language switcher and no locale overlay. It is an internal tool
//! for a small team, and the sentences it writes are literals in the templates.
//! The one exception is a [`phonix_core::Message`] coming back from a service,
//! which is a key: [`message`] renders those against the built-in English
//! catalog.

use askama::Template;
use axum::response::Response;
use phonix_core::Message;
use phonix_core::i18n::catalog;

/// Render a service's message key as an English sentence.
pub fn message(message: &Message) -> String {
    catalog::builtin_only().render(message)
}

/// What the signed-in frame needs, on every page that has one.
///
/// A struct rather than three fields repeated across every page's template
/// struct: `shell.html` names `chrome.who`, so a page that forgets one of these
/// does not compile, and adding a fourth is one edit rather than five.
pub struct Chrome {
    /// Whose session this is. Shown in the top bar, and written by whoever
    /// created the account - so it reaches the page escaped, like everything
    /// else.
    pub who: String,
    /// `development`, `production`. At the foot of the sidebar, because it is
    /// a fact about the box rather than about the page.
    pub environment: String,
    /// Which navigation entry to mark. A `&'static str` compared in the
    /// template rather than an enum: there are four of them, spelled once in
    /// `shell.html`, and a page that names one that does not exist simply
    /// highlights nothing.
    pub current: &'static str,
}

impl Chrome {
    pub fn new(who: &str, environment: &str, current: &'static str) -> Self {
        Self {
            who: who.to_owned(),
            environment: environment.to_owned(),
            current,
        }
    }
}

/// A way out of a page that only has something to say.
pub struct Link {
    pub href: &'static str,
    pub label: &'static str,
}

/// A page that reports and offers nothing: a dead setup link, a 404, a 500.
#[derive(Template)]
#[template(path = "message.html")]
pub struct MessagePage {
    pub title: String,
    pub heading: String,
    pub detail: String,
    pub extra: Option<String>,
    pub back: Option<Link>,
}

impl MessagePage {
    pub fn new(heading: &str, detail: &str) -> Self {
        Self {
            title: heading.to_owned(),
            heading: heading.to_owned(),
            detail: detail.to_owned(),
            extra: None,
            back: None,
        }
    }

    pub fn extra(mut self, extra: &str) -> Self {
        self.extra = Some(extra.to_owned());
        self
    }

    pub fn back(mut self, href: &'static str, label: &'static str) -> Self {
        self.back = Some(Link { href, label });
        self
    }
}

/// Render a template into a response, or report the failure.
///
/// A render can only fail on a formatter error, which in practice means an out
/// of memory condition - but it returns a `Result`, and the alternative to
/// handling it is an `unwrap` in the one place a panic takes the page down.
pub fn render(template: &impl Template) -> Response {
    match template.render() {
        Ok(body) => crate::routes::html_response(body),
        Err(err) => crate::routes::internal_error(err, "rendering a page"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The property the hand-written escaping used to depend on discipline for.
    /// A display name is written by whoever created the account and appears in
    /// the chrome of every page they are signed in to.
    #[test]
    fn the_chrome_escapes_who_is_signed_in() {
        let page = crate::routes::workspaces::WorkspacesPage {
            title: "Workspaces".to_owned(),
            chrome: Chrome::new("<img onerror=x>", "development", "workspaces"),
            banner: None,
            confirmation: None,
            rows: Vec::new(),
            total: 0,
            serving: 0,
            stuck: 0,
            unlicensed: 0,
            outdated: 0,
        };

        let rendered = page.render().expect("the page renders");

        assert!(!rendered.contains("<img onerror=x>"));
        // Numeric entities, not `&lt;`. Askama's HTML escaper writes `&#60;`
        // and `&#62;`, which is equivalent and is what a grep for escaping in
        // this codebase now has to look for.
        assert!(rendered.contains("&#60;img onerror=x&#62;"));
    }

    /// Not a style rule. A page reached before there is anywhere to go must not
    /// carry navigation to places that would only redirect back.
    #[test]
    fn a_signed_out_page_has_no_navigation() {
        let rendered = MessagePage::new("Not found", "There is no page at that address.")
            .render()
            .expect("the page renders");

        assert!(!rendered.contains("Sign out"));
        assert!(!rendered.contains("Desk accounts"));
        assert!(!rendered.contains("Audit trail"));
    }

    /// The property every confirm page rests on: reaching it changes nothing,
    /// and the only thing that acts is a `POST`. A `GET` that suspended a
    /// workspace could be fired by a prefetch, a crawler, or an `<img>` on
    /// somebody else's page.
    #[test]
    fn a_confirm_page_only_acts_through_a_post() {
        let page = crate::routes::workspaces::ConfirmPage {
            title: "Acme".to_owned(),
            chrome: Chrome::new("Ada", "development", "workspaces"),
            banner: None,
            heading: "Suspend this workspace?".to_owned(),
            detail: "The workspace stops serving traffic immediately.".to_owned(),
            consequences: vec!["The database is untouched."],
            action: "/workspaces/acme/suspend".to_owned(),
            button: "Suspend the workspace".to_owned(),
            danger: true,
            back: "/workspaces/acme".to_owned(),
        };

        let rendered = page.render().expect("the page renders");

        assert!(rendered.contains(r#"method="post" action="/workspaces/acme/suspend""#));
        // The only other way out is a link back, which acts on nothing.
        assert!(rendered.contains(r#"href="/workspaces/acme""#));
        assert!(!rendered.contains("method=\"get\""));
    }

    /// Search engines have no business here even though nginx is the real gate:
    /// a tool that suspends workspaces should not be indexable if a server
    /// block is ever mistakenly opened up.
    #[test]
    fn every_page_refuses_indexing() {
        let rendered = MessagePage::new("t", "d").render().expect("renders");

        assert!(rendered.contains("noindex, nofollow"));
    }

    /// Desk fetches two things - its stylesheet and its script - and both are
    /// served out of its own binary. A page that reached for a CDN would be
    /// refused by the content security policy at runtime; this catches it at
    /// test time instead.
    ///
    /// The script was once forbidden outright here. What replaced that rule is
    /// narrower and is the part that actually mattered: nothing is fetched
    /// from anywhere but Desk, and no code is written into the page itself -
    /// an inline `<script>` would need `unsafe-inline` in the policy, and the
    /// policy is the thing standing between a tool that suspends workspaces
    /// and somebody else's JavaScript.
    #[test]
    fn a_page_fetches_nothing_from_anywhere_else() {
        let rendered = MessagePage::new("t", "d").render().expect("renders");

        assert!(rendered.contains(crate::assets::STYLESHEET));
        assert!(rendered.contains(crate::assets::SCRIPT));
        assert!(!rendered.contains("//cdn"));
        assert!(!rendered.contains("https://"));
    }

    /// Every script Desk serves is `defer`red, because no page waits on one.
    ///
    /// This is the "complete without the script" rule (ADR 0005 section 3) in
    /// the one place it can be checked mechanically. A blocking `<script>` in
    /// the head would mean the page's first paint depended on code that is
    /// only ever an enhancement.
    #[test]
    fn the_script_never_blocks_a_page() {
        let rendered = MessagePage::new("t", "d").render().expect("renders");

        let tag = rendered
            .split_once("<script")
            .map(|(_, rest)| rest.split_once('>').map(|(tag, _)| tag).unwrap_or(""))
            .expect("there is a script tag");

        assert!(tag.contains("defer"), "the script tag was <script{tag}>");
    }

    /// An inline style would be blocked by `style-src 'self'` and would simply
    /// not apply - silently, which is the worst way for a colour to be wrong.
    #[test]
    fn no_page_carries_an_inline_style_the_policy_would_refuse() {
        let rendered = MessagePage::new("t", "d").render().expect("renders");

        assert!(!rendered.contains(" style=\""));
    }
}
