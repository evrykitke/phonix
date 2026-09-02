//! Server-rendered HTML, and the rule that keeps it honest.
//!
//! Desk draws its pages here, as strings, with no template engine and no
//! client-side framework. That is not minimalism for its own sake: a wasm panic
//! freezes every handler on a page at once, and the moment Desk is wanted is
//! the moment the product is misbehaving. See
//! `docs/adr/0005-phonix-desk.md` section 3.
//!
//! # The rule, inherited from ADR 0004
//!
//! **Every page must be complete without JavaScript.** Not "degrades
//! gracefully" - complete. Tabs are sections stacked in order, a detail link is
//! an ordinary `<a href>` to a page that exists, collapsing is `<details>`, and
//! every action is a `<form method="post">`. A script compiled into this binary
//! may make something nicer; nothing may need one.
//!
//! # Escaping
//!
//! [`esc`] is the only way a value reaches a page, and it is applied at every
//! interpolation rather than trusted at the boundary. Desk shows workspace
//! slugs, display names and audit detail - all of them written by somebody
//! else - so "this one is safe" is a judgement that has to be right every time
//! or not made at all.
//!
//! # English only
//!
//! Desk has no language switcher and no locale overlay. It is an internal tool
//! for a small team, and the sentences it writes are literals. The one
//! exception is a [`phonix_core::Message`] coming back from a service, which is
//! a key: [`message`] renders those against the built-in English catalog.

use phonix_core::Message;
use phonix_core::i18n::catalog;

/// Escape a value for HTML text or a double-quoted attribute.
///
/// The five characters that matter, and `'` as well because an attribute
/// written with single quotes is a mistake somebody will eventually make.
pub fn esc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

/// Render a service's message key as an English sentence.
pub fn message(message: &Message) -> String {
    catalog::builtin_only().render(message)
}

/// The stylesheet, inlined.
///
/// One request, no cache to bust, nothing to serve from a second path, and it
/// works with no network beyond the page itself. It is small on purpose: Desk
/// is a tool, and the product is where the design work goes.
const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #f6f7f9; --panel: #ffffff; --ink: #16191d; --muted: #5b6472;
  --line: #dfe3e8; --accent: #1f5eff; --bad: #b3261e; --good: #1f7a3d;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #14171c; --panel: #1c2027; --ink: #eef1f5; --muted: #9aa4b2;
    --line: #2c333d; --accent: #6f9bff; --bad: #f2b8b5; --good: #7ddc9c;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--bg); color: var(--ink);
  font: 15px/1.5 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
}
header.top {
  display: flex; align-items: center; gap: 1rem;
  padding: .75rem 1.25rem; background: var(--panel);
  border-bottom: 1px solid var(--line);
}
header.top .name { font-weight: 650; letter-spacing: .01em; }
header.top .env { color: var(--muted); font-size: .85em; }
header.top nav { margin-left: auto; display: flex; gap: 1rem; align-items: center; }
main { max-width: 60rem; margin: 0 auto; padding: 1.5rem 1.25rem 4rem; }
main.narrow { max-width: 26rem; }
h1 { font-size: 1.3rem; margin: 0 0 .25rem; }
p.lede { color: var(--muted); margin: 0 0 1.5rem; }
.panel {
  background: var(--panel); border: 1px solid var(--line);
  border-radius: 10px; padding: 1.25rem; margin-bottom: 1.25rem;
}
label { display: block; font-weight: 550; margin: 0 0 .3rem; }
input[type=text], input[type=email], input[type=password] {
  width: 100%; padding: .55rem .65rem; border: 1px solid var(--line);
  border-radius: 7px; background: var(--bg); color: var(--ink); font: inherit;
}
input:focus-visible, button:focus-visible, a:focus-visible {
  outline: 2px solid var(--accent); outline-offset: 2px;
}
.field { margin-bottom: 1rem; }
.hint { color: var(--muted); font-size: .85em; margin: .3rem 0 0; }
button {
  font: inherit; font-weight: 600; padding: .55rem 1rem; border-radius: 7px;
  border: 1px solid transparent; background: var(--accent); color: #fff;
  cursor: pointer;
}
button.quiet { background: transparent; color: var(--ink); border-color: var(--line); }
.notice {
  border: 1px solid var(--line); border-left: 3px solid var(--muted);
  border-radius: 7px; padding: .7rem .85rem; margin-bottom: 1rem;
}
.notice.bad { border-left-color: var(--bad); }
.notice.good { border-left-color: var(--good); }
code, .mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
.secret {
  display: block; padding: .7rem .85rem; background: var(--bg);
  border: 1px dashed var(--line); border-radius: 7px; margin: .5rem 0 1rem;
  word-break: break-all; font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
table { width: 100%; border-collapse: collapse; }
th, td { text-align: left; padding: .5rem .4rem; border-bottom: 1px solid var(--line); }
th { color: var(--muted); font-weight: 550; font-size: .85em; }
a { color: var(--accent); }
.pill {
  display: inline-block; padding: .05rem .5rem; border-radius: 999px;
  border: 1px solid var(--line); font-size: .8em; color: var(--muted);
}
"#;

/// A whole page.
///
/// `body` is already-escaped markup. The signature takes it as a `String`
/// rather than pieces because a page here is assembled by its handler and this
/// is only the frame around it.
pub struct Page {
    title: String,
    body: String,
    /// The top bar is drawn only for a signed-in desk user - there is nothing
    /// to navigate to before that, and a chrome that suggests otherwise is a
    /// sign-in page pretending to be an application.
    signed_in_as: Option<String>,
    environment: String,
}

impl Page {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            signed_in_as: None,
            environment: String::new(),
        }
    }

    pub fn signed_in_as(mut self, who: &str) -> Self {
        self.signed_in_as = Some(who.to_owned());
        self
    }

    pub fn environment(mut self, environment: &str) -> Self {
        self.environment = environment.to_owned();
        self
    }

    /// Render the document.
    pub fn render(&self) -> String {
        let chrome = match &self.signed_in_as {
            Some(who) => format!(
                r#"<header class="top">
  <span class="name">Phonix Desk</span>
  <span class="env">{env}</span>
  <nav>
    <a href="/">Workspaces</a>
    <a href="/accounts">Desk accounts</a>
    <span class="pill">{who}</span>
    <form method="post" action="/sign-out"><button class="quiet">Sign out</button></form>
  </nav>
</header>"#,
                env = esc(&self.environment),
                who = esc(who)
            ),
            None => String::new(),
        };

        let width = if self.signed_in_as.is_some() {
            ""
        } else {
            " narrow"
        };

        format!(
            r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<title>{title} - Phonix Desk</title>
<style>{style}</style>
</head>
<body>
{chrome}
<main class="page{width}">
{body}
</main>
</body>
</html>"#,
            title = esc(&self.title),
            style = STYLE,
            chrome = chrome,
            width = width,
            body = self.body,
        )
    }
}

/// A banner above a form.
pub fn notice(kind: &str, text: &str) -> String {
    format!(
        r#"<p class="notice {kind}">{text}</p>"#,
        kind = esc(kind),
        text = esc(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dangerous_character_is_escaped() {
        assert_eq!(
            esc(r#"<script>alert("x" + 'y' & z)</script>"#),
            "&lt;script&gt;alert(&quot;x&quot; + &#x27;y&#x27; &amp; z)&lt;/script&gt;"
        );
    }

    /// The one thing a page must never do is put an unescaped display name in
    /// the chrome: it is written by whoever created the account, and it is on
    /// every page they are signed in to.
    #[test]
    fn the_chrome_escapes_who_is_signed_in() {
        let page = Page::new("Workspaces", "<p>hi</p>").signed_in_as("<img onerror=x>");
        let rendered = page.render();

        assert!(!rendered.contains("<img onerror=x>"));
        assert!(rendered.contains("&lt;img onerror=x&gt;"));
    }

    #[test]
    fn a_signed_out_page_has_no_navigation() {
        let rendered = Page::new("Sign in", "<form></form>").render();

        assert!(!rendered.contains("<header"));
        assert!(!rendered.contains("Sign out"));
        assert!(rendered.contains("class=\"page narrow\""));
    }

    /// Search engines have no business here even though nginx is the real
    /// gate: a tool that suspends workspaces should not be indexable if a
    /// server block is ever mistakenly opened up.
    #[test]
    fn every_page_refuses_indexing() {
        assert!(Page::new("t", "").render().contains("noindex, nofollow"));
    }
}
