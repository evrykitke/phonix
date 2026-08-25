//! The currencies settings tab: what this workspace deals in.
//!
//! # One form for adding and for editing
//!
//! Because the service is an upsert, and "use EUR" is a statement about the end
//! state rather than an event. Two forms would be two ways to say the same
//! thing, and the second one is always the one that forgets a field.
//!
//! # The picker offers every ISO code, and the grid shows only the chosen ones
//!
//! Those are two different questions. Adding a currency is choosing from the
//! world; the list is what this workspace has decided about.

use leptos::prelude::*;
use phonix_core::locale::Currency;
use phonix_core::money::WorkspaceCurrency;

use crate::components::page::{GhostButton, Panel, PrimaryButton};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::currency_fns::save_currency;
use crate::ui::alert::{Alert, Alerts};
use crate::ui::table::DataGrid;
use crate::ui::table::config::currencies::currencies_grid;

#[component]
pub fn currencies_tab() -> impl IntoView {
    // `None` means the panel is closed. `Some` is the currency being added or
    // changed - the same shape either way, because the save is an upsert.
    let editing: RwSignal<Option<WorkspaceCurrency>> = RwSignal::new(None);

    // Bumped after a save, which rebuilds the grid and so re-fetches it.
    //
    // A `GridHandle` would be tidier and is not reachable from here: a handle
    // is handed to a *row action*, and this panel is a sibling of the grid
    // rather than something inside it. Rebuilding costs the sort and the
    // filter, which on a settings tab somebody has just finished editing is a
    // fair price for not inventing a second refresh mechanism.
    let version = RwSignal::new(0_u32);

    let build = move || {
        currencies_grid(
            Callback::new(move |row: WorkspaceCurrency| editing.set(Some(row))),
            Callback::new(move |()| {
                editing.set(Some(WorkspaceCurrency {
                    // The default is a placeholder the picker replaces, not a
                    // suggestion: a form that opened on the workspace's own base
                    // currency would invite somebody to overwrite its symbol while
                    // trying to add a second one.
                    currency: Currency::default(),
                    is_enabled: true,
                    symbol: None,
                }));
            }),
        )
    };

    view! {
        <div class="space-y-3">
            <Panel title=l!("currencies.title") description=l!("currencies.description")>
                {move || {
                    version.track();
                    view! { <DataGrid config=build() /> }
                }}
            </Panel>

            // Created fresh each time `editing` changes, which is what re-seeds
            // the controls: they read their opening value once.
            {move || {
                editing
                    .get()
                    .map(|row| {
                        view! {
                            <CurrencyEditor
                                row=row
                                saved=move || version.update(|v| *v = v.wrapping_add(1))
                                close=move || editing.set(None)
                            />
                        }
                    })
            }}
        </div>
    }
}

/// Add a currency, or change how one is shown.
#[component]
fn currency_editor(
    row: WorkspaceCurrency,
    /// Re-read the list. Called only on success, so a failed save leaves the
    /// grid showing what is actually stored.
    saved: impl Fn() + Copy + Send + Sync + 'static,
    close: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let alerts = Alerts::get();

    let code = RwSignal::new(row.currency.code().to_owned());
    let symbol = RwSignal::new(row.symbol.clone().unwrap_or_default());
    let is_enabled = RwSignal::new(row.is_enabled);
    let pending = RwSignal::new(false);

    let save = move |()| {
        pending.set(true);
        let code = code.get_untracked();
        let symbol = symbol.get_untracked();
        let enabled = is_enabled.get_untracked();

        leptos::task::spawn_local(async move {
            let result = save_currency(code, enabled, Some(symbol)).await;
            pending.set(false);

            match result {
                Ok(_) => {
                    alerts.post(Alert::success(l!("currencies.saved")));
                    saved();
                    close();
                }
                // The server's own words: it knows whether this was the base
                // currency, an unknown code, or a permission.
                Err(err) => alerts.post(Alert::failure(err.to_string())),
            }
        });
    };

    view! {
        <div class="max-w-2xl">
            <Panel title=l!("currencies.add")>
                <div class="space-y-3">
                    <div class="grid gap-3 sm:grid-cols-2">
                        <label class="block space-y-1">
                            <span class="text-xs font-medium text-content-muted">
                                {l!("field.currency")}
                            </span>
                            <select
                                class="w-full"
                                prop:value=move || code.get()
                                on:change=move |ev| code.set(event_target_value(&ev))
                            >
                                {Currency::all()
                                    .iter()
                                    .map(|currency| {
                                        let value = currency.code().to_owned();
                                        let label = currency.label();
                                        view! { <option value=value>{label}</option> }
                                    })
                                    .collect_view()}
                            </select>
                        </label>

                        <label class="block space-y-1">
                            <span class="text-xs font-medium text-content-muted">
                                {l!("field.symbol")}
                            </span>
                            <input
                                type="text"
                                class="w-full"
                                maxlength="8"
                                prop:value=move || symbol.get()
                                on:input=move |ev| symbol.set(event_target_value(&ev))
                            />
                            <span class="block text-2xs text-content-subtle">
                                {l!("currencies.symbol_help")}
                            </span>
                        </label>
                    </div>

                    <label class="flex items-center gap-2 text-sm text-content">
                        <input
                            type="checkbox"
                            prop:checked=move || is_enabled.get()
                            on:change=move |ev| is_enabled.set(event_target_checked(&ev))
                        />
                        {l!("common.active")}
                    </label>

                    <div class="flex items-center justify-end gap-2">
                        <GhostButton
                            label=l!("common.cancel")
                            on_click=Callback::new(move |()| close())
                        />
                        <PrimaryButton
                            label=l!("common.save")
                            icon=Icon::Save
                            pending=Signal::derive(move || pending.get())
                            on_click=Callback::new(save)
                        />
                    </div>
                </div>
            </Panel>
        </div>
    }
}
