//! The strip under the table: what is showing, and how to see the rest.
//!
//! # It says the range, not just the page
//!
//! "Showing 26-50 of 213" answers the question people actually have. "Page 2 of
//! 9" only answers it after they have worked out how big a page is, and it is
//! the same width on screen.
//!
//! # Nothing here owns a reactive value
//!
//! The strip is drawn inside the grid's `Transition`, and a transition disposes
//! the owner of what is on screen the moment a refetch starts - see the note on
//! zombie rows in [`menu`](super::menu). A `Callback` or a derived signal made
//! here would therefore be gone while the control that reads it is still on the
//! screen and still clickable, and reading one is a panic that takes the whole
//! page with it.
//!
//! Hence the shape of this file. The footing arrives as a plain value, the
//! buttons carry plain closures, and the one `Callback` a `SelectField` insists
//! on is made by the grid and handed in. All that is left for a handler to
//! touch is `state`, whose signals belong to the grid itself and outlive every
//! reload - which makes the strip correct during the window rather than merely
//! inert.
//!
//! # Numbered pages stay out of the way on a phone
//!
//! Previous and next are always there. The numbers between them are a window
//! around the current page - never the full list, because a hundred-page table
//! would otherwise render a hundred buttons - and they are hidden below `sm`,
//! where two arrows and a page count fit and eleven buttons do not.

use leptos::prelude::*;

use super::state::GridState;
use crate::icons::{Icon, IconSize};
use crate::ui::form::field::Choice;
use crate::ui::lookup::SelectField;
use crate::l;

/// What the pager is describing. Computed by the grid from the page it drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Footing {
    pub page: u32,
    pub pages: u32,
    pub total: u64,
    /// 1-based number of the first row on screen; zero when there are none.
    pub first: u64,
    pub last: u64,
}

/// How many numbered pages to show either side of the current one.
const WINDOW: u32 = 2;

#[component]
pub fn grid_pager(
    state: GridState,
    /// What is on the screen. A plain value rather than a signal: it describes
    /// the page this strip was drawn for, and the next page brings its own.
    footing: Footing,
    /// Page sizes on offer. Empty hides the control.
    choices: &'static [u32],
    /// What to do when a different page size is chosen. Handed in rather than
    /// made here - see the module note on why nothing in this strip owns a
    /// reactive value.
    on_per_page: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center justify-between gap-2 border-t border-edge px-3 py-2 text-xs text-content-muted">
            <div class="flex items-center gap-3">
                <span aria-live="polite">
                    {if footing.total == 0 {
                        l!("grid.no_rows")
                    } else {
                        l!(
                            "grid.showing",
                            first = footing.first.to_string(),
                            last = footing.last.to_string(),
                            total = footing.total.to_string(),
                        )
                    }}
                </span>

                {(!choices.is_empty())
                    .then(|| {
                        view! { <PerPage state=state choices=choices on_change=on_per_page /> }
                    })}
            </div>

            <div class="flex items-center gap-1">
                <Step
                    label=l!("grid.previous")
                    icon=Icon::ChevronLeft
                    disabled={footing.page <= 1}
                    on_click=move || state.go_to(footing.page.saturating_sub(1))
                />

                <span class="px-1 sm:hidden">{format!("{} / {}", footing.page, footing.pages)}</span>

                <div class="hidden items-center gap-1 sm:flex">
                    {window(footing.page, footing.pages)
                        .into_iter()
                        .map(|page| {
                            view! { <PageNumber state=state page=page current=footing.page /> }
                        })
                        .collect::<Vec<_>>()}
                </div>

                <Step
                    label=l!("grid.next")
                    icon=Icon::ChevronRight
                    disabled={footing.page >= footing.pages}
                    on_click=move || state.go_to(footing.page + 1)
                />
            </div>
        </div>
    }
}

/// The page numbers to draw around `page`, clamped to what exists.
///
/// Always the same width where there is room for it, so the arrows either side
/// do not shuffle as the window slides.
fn window(page: u32, pages: u32) -> Vec<u32> {
    let span = WINDOW * 2 + 1;

    if pages <= span {
        return (1..=pages.max(1)).collect();
    }

    let start = page.saturating_sub(WINDOW).max(1).min(pages - span + 1);

    (start..start + span).collect()
}

#[component]
fn per_page(
    state: GridState,
    choices: &'static [u32],
    on_change: Callback<String>,
) -> impl IntoView {
    let options = choices
        .iter()
        .map(|choice| Choice::new(choice.to_string(), choice.to_string()))
        .collect::<Vec<_>>();

    view! {
        <div class="flex items-center gap-1">
            <span class="hidden sm:inline">{l!("grid.rows")}</span>
            <SelectField
                value=Signal::derive(move || state.per_page.get().to_string())
                on_change=on_change
                options=options
                label=l!("grid.rows_per_page")
                class="h-6 w-auto min-h-0 px-1.5 text-xs"
            />
        </div>
    }
}

#[component]
fn step(
    #[prop(into)] label: String,
    icon: Icon,
    disabled: bool,
    /// A plain closure rather than a `Callback`, because a callback is an arena
    /// value and this button outlives its owner every time the table reloads.
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="grid size-7 place-items-center rounded-control border border-edge text-content-muted hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
            disabled=disabled
            aria-label=label
            on:click=move |_| on_click()
        >
            <Icon icon=icon size=IconSize::Xs />
        </button>
    }
}

#[component]
fn page_number(state: GridState, page: u32, current: u32) -> impl IntoView {
    let is_current = page == current;

    view! {
        <button
            type="button"
            class=if is_current {
                "grid size-7 place-items-center rounded-control border border-brand bg-brand-subtle font-medium text-brand"
            } else {
                "grid size-7 place-items-center rounded-control border border-transparent text-content-muted hover:bg-surface-hover hover:text-content"
            }
            aria-label=l!("grid.page", page = page.to_string())
            aria-current=if is_current { "page" } else { "false" }
            on:click=move |_| state.go_to(page)
        >
            {page.to_string()}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_list_shows_every_page() {
        assert_eq!(window(1, 3), [1, 2, 3]);
    }

    #[test]
    fn one_empty_page_still_offers_a_number_to_stand_on() {
        assert_eq!(window(1, 0), [1]);
    }

    #[test]
    fn the_window_is_centred_on_the_current_page() {
        assert_eq!(window(10, 20), [8, 9, 10, 11, 12]);
    }

    #[test]
    fn the_window_does_not_run_off_the_start() {
        assert_eq!(window(1, 20), [1, 2, 3, 4, 5]);
        assert_eq!(window(2, 20), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_window_does_not_run_off_the_end() {
        assert_eq!(window(20, 20), [16, 17, 18, 19, 20]);
        assert_eq!(window(19, 20), [16, 17, 18, 19, 20]);
    }

    #[test]
    fn the_window_keeps_its_width_wherever_it_sits() {
        for page in 1..=20 {
            assert_eq!(window(page, 20).len(), 5, "page {page}");
        }
    }
}
