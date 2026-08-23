//! Ctrl+K: the mega menu and the search box, which are the same thing.
//!
//! Opened empty it is a menu - every place this user may go, grouped by
//! section, which is the "mega menu" a top bar would otherwise have to lay out
//! in columns. Typed into, the same list ranks itself and becomes a search.
//! There is no second index: both read
//! [`navigation::reachable`](crate::navigation::reachable), so a screen that is
//! in the menu is findable here the moment it is added, and a screen the viewer
//! may not open is in neither.
//!
//! The shortcut itself is bound on the window in [`super::AppShell`], because it
//! has to work when this component is not on screen.

use leptos::html;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use super::Shell;
use crate::i18n::Locale;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::navigation::{Destination, MENU, reachable, search};

#[component]
pub fn command_palette() -> impl IntoView {
    let shell = Shell::get();
    let query = RwSignal::new(String::new());
    let selected = RwSignal::new(0usize);
    let input_ref: NodeRef<html::Input> = NodeRef::new();

    // The session, read once. `Option` because the palette is built before it
    // has necessarily arrived - which shows an empty list for a moment rather
    // than every entry regardless of permission.
    let user = AsyncDerived::new(move || async move { shell.user().await });

    // Held rather than read per result: the palette searches the *translated*
    // label, so every scoring pass needs the words, and it re-runs on every
    // keystroke.
    let catalog = Locale::get().shared();

    let results = Memo::new(move |_| {
        let query = query.get();
        let query = query.trim();

        let Some(Some(user)) = user.get() else {
            return Vec::new();
        };

        if query.is_empty() {
            // Menu order, not relevance: with nothing typed there is no
            // relevance to sort by, and tree order is what makes the grouping
            // below read like the sidebar.
            reachable(MENU, Some(&user), &catalog)
        } else {
            search(MENU, Some(&user), &catalog, query)
        }
    });

    // Opening puts the caret in the box and starts from the top. Reopening with
    // last week's query still in it would be a small, constant annoyance.
    Effect::new(move |_| {
        if shell.palette_open.get() {
            query.set(String::new());
            selected.set(0);

            if let Some(input) = input_ref.get() {
                let _ = input.focus();
            }
        }
    });

    let go = move |href: &str| {
        shell.close_palette();
        use_navigate()(href, NavigateOptions::default());
    };

    let move_selection = move |delta: isize| {
        let count = results.with(Vec::len);
        if count == 0 {
            return;
        }

        selected.update(|index| {
            let next = *index as isize + delta;
            // Wraps, because a list you can walk off the end of makes people
            // hunt for the edge instead of holding the key down.
            *index = next.rem_euclid(count as isize) as usize;
        });
    };

    view! {
        <Show when=move || shell.palette_open.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-50 flex items-start justify-center bg-overlay px-4 pt-[12vh]"
                role="dialog"
                aria-modal="true"
                aria-label=l!("palette.title")
                on:click=move |_| shell.close_palette()
            >
                <div
                    class="w-full max-w-xl overflow-hidden rounded-pop border border-edge bg-surface-raised shadow-pop"
                    // The backdrop closes the palette; a click inside it must
                    // not bubble up and do the same.
                    on:click=|event| event.stop_propagation()
                >
                    <div class="flex items-center gap-2 border-b border-edge px-3">
                        <Icon
                            icon=Icon::Search
                            size=IconSize::Sm
                            class="shrink-0 text-content-subtle"
                        />
                        <input
                            node_ref=input_ref
                            type="text"
                            class="control-bare h-10 w-full bg-transparent text-sm text-content outline-none placeholder:text-content-subtle"
                            placeholder=l!("palette.placeholder")
                            autocomplete="off"
                            spellcheck="false"
                            prop:value=move || query.get()
                            on:input=move |event| {
                                query.set(event_target_value(&event));
                                selected.set(0);
                            }
                            on:keydown=move |event| {
                                match event.key().as_str() {
                                    "ArrowDown" => {
                                        event.prevent_default();
                                        move_selection(1);
                                    }
                                    "ArrowUp" => {
                                        event.prevent_default();
                                        move_selection(-1);
                                    }
                                    "Enter" => {
                                        event.prevent_default();
                                        let target = results
                                            .with(|list| {
                                                list.get(selected.get_untracked()).map(|item| item.href)
                                            });
                                        if let Some(href) = target {
                                            go(href);
                                        }
                                    }
                                    "Escape" => shell.close_palette(),
                                    _ => {}
                                }
                            }
                        />
                        // The engraving on the key, not a word for it.
                        <kbd class="shrink-0 rounded border border-edge bg-surface-sunken px-1 font-mono text-2xs text-content-subtle">
                            "Esc"
                        </kbd>
                    </div>

                    <div class="max-h-[50vh] overflow-y-auto p-1">
                        {move || {
                            let items = results.get();

                            if items.is_empty() {
                                return view! {
                                    <p class="px-3 py-6 text-center text-sm text-content-subtle">
                                        {l!("palette.no_match")}
                                    </p>
                                }
                                    .into_any();
                            }

                            // Section headings only with an empty query. Once
                            // results are ranked they are no longer grouped, and
                            // a heading over a re-ordered list would lie about
                            // what is under it.
                            let grouped = query.with(|query| query.trim().is_empty());
                            let mut previous: Option<String> = None;

                            items
                                .into_iter()
                                .enumerate()
                                .map(|(index, item)| {
                                    let heading = (grouped && item.section != previous)
                                        .then(|| item.section.clone())
                                        .flatten();
                                    previous = item.section.clone();

                                    view! {
                                        <Show
                                            when={
                                                let heading = heading.clone();
                                                move || heading.is_some()
                                            }
                                            fallback=|| ()
                                        >
                                            <div class="px-2 pb-1 pt-2 text-2xs font-semibold uppercase tracking-wide text-content-subtle">
                                                {heading.clone().unwrap_or_default()}
                                            </div>
                                        </Show>
                                        <Row item=item.clone() index=index selected=selected go=go />
                                    }
                                })
                                .collect::<Vec<_>>()
                                .into_any()
                        }}
                    </div>

                    <div class="flex items-center gap-3 border-t border-edge px-3 py-1.5 text-2xs text-content-subtle">
                        // The key caps are the keys themselves, not words.
                        <Hint keys="↑↓" label=l!("palette.hint.navigate") />
                        <Hint keys="↵" label=l!("palette.hint.open") />
                        <Hint keys="Ctrl K" label=l!("palette.hint.toggle") />
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// One result.
#[component]
fn row(
    item: Destination,
    index: usize,
    selected: RwSignal<usize>,
    go: impl Fn(&str) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let href = item.href;
    let label = item.label.clone();
    let breadcrumb = item.breadcrumb.join(" / ");
    let has_breadcrumb = !breadcrumb.is_empty();

    view! {
        <button
            type="button"
            class=move || {
                let state = if selected.get() == index {
                    "bg-brand-subtle text-brand"
                } else {
                    "text-content-muted hover:bg-surface-hover hover:text-content"
                };
                format!("flex w-full items-center gap-2 rounded-control px-2 py-1.5 text-left text-sm {state}")
            }
            // Hovering moves the selection so the keyboard and the mouse never
            // disagree about which row Enter would open.
            on:mousemove=move |_| selected.set(index)
            on:click=move |_| go(href)
        >
            <span class="grid size-6 shrink-0 place-items-center">
                {match item.icon {
                    Some(icon) => view! { <Icon icon=icon size=IconSize::Sm /> }.into_any(),
                    None => view! { <span class="size-1.5 rounded-full bg-current opacity-40" /> }
                        .into_any(),
                }}
            </span>

            <span class="min-w-0 flex-1 truncate-fade">{label}</span>

            <Show when=move || has_breadcrumb fallback=|| ()>
                <span class="shrink-0 text-2xs text-content-subtle">{breadcrumb.clone()}</span>
            </Show>
        </button>
    }
}

#[component]
fn hint(keys: &'static str, #[prop(into)] label: String) -> impl IntoView {
    view! {
        <span class="flex items-center gap-1">
            <kbd class="rounded border border-edge bg-surface-sunken px-1 font-mono">{keys}</kbd>
            {label}
        </span>
    }
}
