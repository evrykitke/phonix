//! The front page of an app: what is in it, and where to go next.
//!
//! # Why an app needs one at all
//!
//! The launcher and the store both send somebody *to an app*, and until now
//! there was nowhere to send them - `/sales` answered nothing, and the only
//! address that existed was one screen deep. That is fine while an app has one
//! screen and stops being fine at two: "Sales" then means the invoice list,
//! and the second thing anybody adds has to be discovered through the sidebar.
//!
//! # What belongs on one
//!
//! A handful of numbers and the way in to each screen. Not a report: a page
//! somebody passes through on the way somewhere else has a second or two of
//! attention, and a chart nobody has asked a question of yet is decoration.
//!
//! # Numbers that do not depend on today
//!
//! Every figure here renders twice - once on the server, once at hydration -
//! and a count that reads the clock can differ between the two. Near midnight
//! that is a hydration mismatch, and a wasm panic takes the whole application
//! with it; see [`crate::recovery`]. So the counts are of *states*, which are
//! facts about a row, and never of periods. "Posted this month" belongs on a
//! report that is allowed to be a client-only render.
//!
//! Money is left off for a different reason: a workspace can invoice in several
//! currencies, and one total across them is either wrong or needs a rate for a
//! date - which is the date problem again, wearing a hat.

use leptos::prelude::*;
use leptos_router::components::A;
use phonix_core::apps::AppDescriptor;
use phonix_core::i18n::Message;

use crate::apps::icon_of;
use crate::i18n::t;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::viewer::Viewer;

/// One number, with the word for what it counts.
#[derive(Clone)]
pub struct Stat {
    pub label: String,
    pub value: String,
}

impl Stat {
    pub fn new(label: impl Into<String>, value: impl ToString) -> Self {
        Self {
            label: label.into(),
            value: value.to_string(),
        }
    }
}

/// A way in to one of the app's screens.
#[derive(Clone)]
pub struct Shortcut {
    pub label: String,
    pub detail: String,
    pub href: String,
    pub icon: Icon,
    /// Hidden without it. Cosmetic, like every gate in the interface - the
    /// screen behind it refuses on its own account.
    pub permission: Option<&'static str>,
    /// Drawn as the one thing to do, rather than one of several places to go.
    pub primary: bool,
}

impl Shortcut {
    pub fn new(
        label: impl Into<String>,
        detail: impl Into<String>,
        href: impl Into<String>,
        icon: Icon,
    ) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            href: href.into(),
            icon,
            permission: None,
            primary: false,
        }
    }

    #[must_use]
    pub fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    #[must_use]
    pub const fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
}

/// An app's front page.
///
/// The app supplies its own numbers and its own ways in; everything about the
/// *shape* - the heading, the stat strip, the tile grid - is here, so a second
/// app's home cannot come out looking like a different product.
#[component]
pub fn app_home(
    app: &'static AppDescriptor,
    /// Resolved by the caller, because only the app knows what is worth
    /// counting. Empty is fine and draws nothing.
    #[prop(optional, into)]
    stats: Signal<Vec<Stat>>,
    shortcuts: Vec<Shortcut>,
) -> impl IntoView {
    let viewer = Viewer::get();
    let name = t(&Message::new(app.name));
    let summary = t(&Message::new(app.summary));
    let version = app.version;

    view! {
        <div class="space-y-6">
            <header class="flex flex-wrap items-start gap-3">
                <span class="grid size-11 shrink-0 place-items-center rounded-card bg-brand-subtle text-brand">
                    <Icon icon=icon_of(app) size=IconSize::Lg />
                </span>
                <div class="min-w-0 flex-1">
                    <div class="flex flex-wrap items-baseline gap-2">
                        <h1 class="text-xl font-semibold tracking-tight text-content">{name}</h1>
                        <span class="font-mono text-2xs text-content-subtle">
                            {t(&Message::new("apps.version").arg("version", version))}
                        </span>
                    </div>
                    <p class="mt-0.5 max-w-measure text-sm text-content-muted">{summary}</p>
                </div>
            </header>

            {move || {
                let stats = stats.get();

                (!stats.is_empty())
                    .then(|| {
                        view! {
                            <dl class="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
                                {stats
                                    .into_iter()
                                    .map(|stat| {
                                        view! {
                                            <div class="rounded-card border border-edge bg-surface-raised px-4 py-3">
                                                <dt class="text-xs text-content-subtle">
                                                    {stat.label}
                                                </dt>
                                                <dd class="mt-1 text-2xl font-semibold tabular-nums text-content">
                                                    {stat.value}
                                                </dd>
                                            </div>
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                            </dl>
                        }
                    })
            }}

            <section class="space-y-2">
                <h2 class="text-xs font-medium uppercase tracking-wide text-content-subtle">
                    {l!("apps.home.go_to")}
                </h2>
                <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                    {shortcuts
                        .into_iter()
                        .map(|shortcut| {
                            let permission = shortcut.permission;
                            let allowed = move || {
                                permission.is_none_or(|permission| {
                                    viewer.get().is_some_and(|user| user.can(permission))
                                })
                            };

                            view! {
                                <Show when=allowed fallback=|| ()>
                                    <ShortcutTile shortcut=shortcut.clone() />
                                </Show>
                            }
                        })
                        .collect::<Vec<_>>()}
                </div>
            </section>
        </div>
    }
}

/// One way in, as a card-sized link.
///
/// A whole card rather than a line of links: these are the first thing on the
/// page and the reason somebody opened it, and a row of underlined text is the
/// shape of a footer.
#[component]
fn shortcut_tile(shortcut: Shortcut) -> impl IntoView {
    let class = if shortcut.primary {
        "group flex items-start gap-3 rounded-card border border-brand bg-brand-subtle p-4 hover:bg-brand-subtle-hover"
    } else {
        "group flex items-start gap-3 rounded-card border border-edge bg-surface-raised p-4 hover:bg-surface-hover"
    };
    let icon_class = if shortcut.primary {
        "grid size-9 shrink-0 place-items-center rounded-control bg-brand text-on-brand"
    } else {
        "grid size-9 shrink-0 place-items-center rounded-control bg-surface-sunken text-content-muted"
    };

    view! {
        <A href=shortcut.href attr:class=class>
            <span class=icon_class>
                <Icon icon=shortcut.icon size=IconSize::Sm />
            </span>
            <span class="min-w-0 flex-1">
                <span class="flex items-center gap-1 text-sm font-medium text-content">
                    {shortcut.label}
                    <span class="text-content-subtle transition-transform group-hover:translate-x-0.5">
                        <Icon icon=Icon::ArrowRight size=IconSize::Xs />
                    </span>
                </span>
                <span class="mt-0.5 block text-xs leading-relaxed text-content-muted">
                    {shortcut.detail}
                </span>
            </span>
        </A>
    }
}
