//! The numbering settings tab: what a document number looks like.
//!
//! # The preview is a sample, never the next number
//!
//! Showing a document the number it is *going* to get promises something the
//! save may not keep. This screen renders the *format* against a sample
//! counter, which is a different act and entirely safe - and it does it on the
//! server, because `{FY}` reads the organization's fiscal year opening and a
//! preview that guessed at that would disagree with the number actually issued.
//!
//! # The refusal is the interesting path
//!
//! Changing a format on a series that has already issued can reissue a number
//! in a shape that no longer distinguishes it from last year's. The service
//! refuses that unless "Starts at" is raised past the last number handed out,
//! and it comes back carrying that number - so the screen can say what to
//! change rather than only that something is wrong.

use leptos::prelude::*;
use phonix_core::numbering::{NumberSeries, ResetPeriod, SeriesSaved, SeriesSettings};

use crate::components::page::{GhostButton, Notice, Panel, PrimaryButton, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::numbering_fns::{preview_number_format, save_number_series};
use crate::ui::alert::{Alert, Alerts};
use crate::ui::table::DataGrid;
use crate::ui::table::config::numbering::number_series_grid;

#[component]
pub fn numbering_tab() -> impl IntoView {
    let editing: RwSignal<Option<NumberSeries>> = RwSignal::new(None);

    // Bumped after a save, which rebuilds the grid and so re-fetches it. See
    // the note on the currencies tab for why a `GridHandle` is not reachable
    // from a panel sitting beside the grid.
    let version = RwSignal::new(0_u32);

    let build = move || {
        number_series_grid(Callback::new(move |row: NumberSeries| {
            editing.set(Some(row))
        }))
    };

    view! {
        <div class="space-y-3">
            <Panel title=l!("numbering.title") description=l!("numbering.description")>
                {move || {
                    version.track();
                    view! { <DataGrid config=build() /> }
                }}
            </Panel>

            {move || {
                editing
                    .get()
                    .map(|series| {
                        view! {
                            <SeriesEditor
                                series=series
                                saved=move || version.update(|v| *v = v.wrapping_add(1))
                                close=move || editing.set(None)
                            />
                        }
                    })
            }}
        </div>
    }
}

/// One series: its format, when it resets, and where it starts.
#[component]
fn series_editor(
    series: NumberSeries,
    saved: impl Fn() + Copy + Send + Sync + 'static,
    close: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let alerts = Alerts::get();
    let title = series.key();
    let issued = series.counter;
    let scope = series.scope_key.clone();

    let settings = SeriesSettings::of(&series);
    let pattern = RwSignal::new(settings.pattern.clone());
    let reset = RwSignal::new(settings.reset_period);
    let start_at = RwSignal::new(settings.start_at);
    let is_active = RwSignal::new(settings.is_active);
    let pending = RwSignal::new(false);

    // What the format would produce, refreshed as it is typed. `Ok` is a sample
    // number; `Err` is the parser's own words about what is wrong with the
    // mask, which is more useful than "invalid format".
    let preview = Resource::new(
        move || (pattern.get(), scope.clone()),
        |(mask, scope)| async move { preview_number_format(mask, scope).await },
    );

    // The refusal, when there is one. Held apart from the alert channel because
    // it is not an error - it is an instruction about which field to change,
    // and it belongs beside that field.
    let refused = RwSignal::new(None::<String>);

    let key = (
        settings.app_id.clone(),
        settings.doc_type.clone(),
        settings.scope_key.clone(),
    );

    let save = move |()| {
        pending.set(true);
        refused.set(None);

        let submission = SeriesSettings {
            app_id: key.0.clone(),
            doc_type: key.1.clone(),
            scope_key: key.2.clone(),
            pattern: pattern.get_untracked(),
            reset_period: reset.get_untracked(),
            start_at: start_at.get_untracked(),
            is_active: is_active.get_untracked(),
        };

        leptos::task::spawn_local(async move {
            let result = save_number_series(submission).await;
            pending.set(false);

            match result {
                Ok(SeriesSaved::Saved(_)) => {
                    alerts.post(Alert::success(l!("numbering.saved")));
                    saved();
                    close();
                }
                // Not an error: an expected path through the form, and the
                // number it carries is what makes the instruction actionable.
                Ok(SeriesSaved::WouldReissue { issued }) => {
                    refused.set(Some(l!("numbering.would_reissue", issued = issued)));
                }
                Ok(SeriesSaved::BadPattern(reason)) => refused.set(Some(reason)),
                Ok(SeriesSaved::NoSuchSeries) => {
                    alerts.post(Alert::failure(l!("numbering.no_such_series")));
                }
                Err(err) => alerts.post(Alert::failure(err.to_string())),
            }
        });
    };

    view! {
        <div class="max-w-2xl">
            <Panel title=title>
                <div class="space-y-3">
                    <Notice
                        message=Signal::derive(move || refused.get())
                        tone=Tone::Warning
                    />

                    <label class="block space-y-1">
                        <span class="text-xs font-medium text-content-muted">
                            {l!("numbering.format")}
                        </span>
                        <input
                            type="text"
                            class="w-full font-mono"
                            maxlength="60"
                            prop:value=move || pattern.get()
                            on:input=move |ev| pattern.set(event_target_value(&ev))
                        />
                        <span class="block text-2xs text-content-subtle">
                            {l!("numbering.format_help")}
                        </span>
                    </label>

                    // A sample, and it says so. See the module note.
                    <Transition fallback=|| ()>
                        {move || Suspend::new(async move {
                            match preview.await {
                                Ok(Ok(sample)) => {
                                    view! {
                                        <p class="text-sm text-content-muted">
                                            {l!("numbering.preview")} ": "
                                            <code class="font-mono text-content">{sample}</code>
                                        </p>
                                    }
                                        .into_any()
                                }
                                Ok(Err(reason)) => {
                                    view! {
                                        <p class="text-sm text-warning">{reason}</p>
                                    }
                                        .into_any()
                                }
                                // A preview that could not be fetched is not
                                // worth an error: the save is what decides, and
                                // it validates the mask again.
                                Err(_) => ().into_any(),
                            }
                        })}
                    </Transition>

                    <div class="grid gap-3 sm:grid-cols-2">
                        <label class="block space-y-1">
                            <span class="text-xs font-medium text-content-muted">
                                {l!("numbering.reset")}
                            </span>
                            <select
                                class="w-full"
                                prop:value=move || reset.get().as_str().to_owned()
                                on:change=move |ev| {
                                    // Unrecognised keeps what was there: a
                                    // period silently becoming `Never` is a
                                    // series that stops resetting, and nobody
                                    // notices until January.
                                    if let Some(period) = ResetPeriod::parse(
                                        &event_target_value(&ev),
                                    ) {
                                        reset.set(period);
                                    }
                                }
                            >
                                {ResetPeriod::ALL
                                    .iter()
                                    .map(|period| {
                                        let value = period.as_str();
                                        let label = crate::i18n::t(&period.label());
                                        view! { <option value=value>{label}</option> }
                                    })
                                    .collect_view()}
                            </select>
                        </label>

                        <label class="block space-y-1">
                            <span class="text-xs font-medium text-content-muted">
                                {l!("numbering.start_at")}
                            </span>
                            <input
                                type="number"
                                class="w-full"
                                min="1"
                                prop:value=move || start_at.get()
                                on:input=move |ev| {
                                    if let Ok(value) = event_target_value(&ev).parse::<i64>() {
                                        start_at.set(value.max(1));
                                    }
                                }
                            />
                            <span class="block text-2xs text-content-subtle">
                                {l!("numbering.start_at_help")}
                            </span>
                        </label>
                    </div>

                    <p class="text-xs text-content-subtle">
                        {l!("numbering.issued")} ": "
                        {if issued > 0 {
                            issued.to_string()
                        } else {
                            l!("numbering.never_issued")
                        }}
                    </p>

                    <label class="flex items-center gap-2 text-sm text-content">
                        <input
                            type="checkbox"
                            prop:checked=move || is_active.get()
                            on:change=move |ev| is_active.set(event_target_checked(&ev))
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
