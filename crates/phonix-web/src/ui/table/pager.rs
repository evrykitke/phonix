//! The strip under the table: what is showing, and how to see the rest.
//!
//! # It says the range, not just the page
//!
//! "Showing 26-50 of 213" answers the question people actually have. "Page 2 of
//! 9" only answers it after they have worked out how big a page is, and it is
//! the same width on screen.
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
    footing: Signal<Footing>,
    /// Page sizes on offer. Empty hides the control.
    choices: &'static [u32],
) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center justify-between gap-2 border-t border-edge px-3 py-2 text-xs text-content-muted">
            <div class="flex items-center gap-3">
                <span aria-live="polite">
                    {move || {
                        let f = footing.get();

                        if f.total == 0 {
                            l!("grid.no_rows")
                        } else {
                            l!(
                                "grid.showing",
                                first = f.first.to_string(),
                                last = f.last.to_string(),
                                total = f.total.to_string(),
                            )
                        }
                    }}
                </span>

                {(!choices.is_empty())
                    .then(|| view! { <PerPage state=state choices=choices /> })}
            </div>

            <div class="flex items-center gap-1">
                <Step
                    label=l!("grid.previous")
                    icon=Icon::ChevronLeft
                    disabled=Signal::derive(move || footing.get().page <= 1)
                    on_click=Callback::new(move |()| {
                        state.go_to(footing.get().page.saturating_sub(1))
                    })
                />

                <span class="px-1 sm:hidden">
                    {move || {
                        let f = footing.get();
                        format!("{} / {}", f.page, f.pages)
                    }}
                </span>

                <div class="hidden items-center gap-1 sm:flex">
                    {move || {
                        let f = footing.get();

                        window(f.page, f.pages)
                            .into_iter()
                            .map(|page| view! { <PageNumber state=state page=page current=f.page /> })
                            .collect::<Vec<_>>()
                    }}
                </div>

                <Step
                    label=l!("grid.next")
                    icon=Icon::ChevronRight
                    disabled=Signal::derive(move || {
                        let f = footing.get();
                        f.page >= f.pages
                    })
                    on_click=Callback::new(move |()| state.go_to(footing.get().page + 1))
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
fn per_page(state: GridState, choices: &'static [u32]) -> impl IntoView {
    view! {
        <label class="flex items-center gap-1">
            <span class="hidden sm:inline">{l!("grid.rows")}</span>
            <select
                // Border, background, radius and arrow come from the global
                // `select` rule in `style/main.css`. Only the size is this
                // control's own business.
                class="h-6 text-xs outline-none"
                aria-label=l!("grid.rows_per_page")
                prop:value=move || state.per_page.get().to_string()
                on:change=move |event| {
                    if let Ok(per_page) = event_target_value(&event).parse::<u32>() {
                        state.set_per_page(per_page);
                    }
                }
            >
                {choices
                    .iter()
                    .map(|choice| {
                        view! { <option value=choice.to_string()>{choice.to_string()}</option> }
                    })
                    .collect::<Vec<_>>()}
            </select>
        </label>
    }
}

#[component]
fn step(
    #[prop(into)] label: String,
    icon: Icon,
    #[prop(into)] disabled: Signal<bool>,
    on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="grid size-7 place-items-center rounded-control border border-edge text-content-muted hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
            disabled=move || disabled.get()
            aria-label=label
            on:click=move |_| on_click.run(())
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
