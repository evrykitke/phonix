//! The interface kit, on a page.
//!
//! # What it is for
//!
//! [`crate::ui`] is furniture that has never heard of Phonix, and until now the
//! only way to see a piece of it was to find a screen that happened to use one.
//! That is a poor way to answer the two questions people actually have - *does
//! the kit already do this?* and *what does it look like in the state I need?* -
//! and it is how a codebase ends up with a second dropdown.
//!
//! So: one tab per component, showing it in the states it can be in, beside a
//! sentence saying what it will and will not do. A component is not finished
//! until it is on this page.
//!
//! # The specimen text is the documentation
//!
//! A showcase needs something inside each example, and filler would put a
//! paragraph of nothing into three catalogs forever. What is in the examples
//! here is instead the component's own design notes - why it is built the way
//! it is, and what it refuses to do - so the demonstration and the reference
//! are the same words.
//!
//! # There is no server function behind this page
//!
//! Nothing on it is data, so there is nothing to refuse. The permission is
//! honest all the same: [`phonix_core::permissions::UI_LIBRARY`] keeps it off the sidebar
//! of anybody who has not been granted it, and a workspace that does not want a
//! developer reference in its menu revokes it from the role. Somebody who types
//! the URL sees component specimens, which is the whole of what there is to see.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::page::{Badge, PageHeader, Panel, Tone};
use crate::icons::Icon;
use crate::l;
use crate::ui::card::CollapsibleCard;
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn ui_library_page() -> impl IntoView {
    // Ordered as the kit is built, newest last, so a tab does not move under
    // somebody's cursor when the next component lands. The roadmap stays at the
    // end for the same reason.
    let tabs = vec![
        Tab::new("cards", l!("ui_library.tab.cards"), || {
            view! { <CardsTab /> }.into_any()
        })
        .icon(Icon::LayoutGrid),
        Tab::new("roadmap", l!("ui_library.tab.roadmap"), || {
            view! { <RoadmapTab /> }.into_any()
        })
        .icon(Icon::ListTree),
    ];

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("ui_library.title")) />

        <PageHeader
            title=l!("ui_library.title")
            subtitle=l!("ui_library.subtitle")
            icon=Icon::Palette
        />

        <TabbedPanel id="ui-library" tabs=tabs />
    }
}

/// [`CollapsibleCard`], in each of the shapes it comes in.
#[component]
fn cards_tab() -> impl IntoView {
    view! {
        <Panel title=l!("ui_library.cards.title") description=l!("ui_library.cards.detail")>
            <div class="space-y-3">
                // A stack rather than one specimen: the shape of this component
                // is a list, and a single card would not show that the closed
                // ones stay scannable while one of them is open.
                <CollapsibleCard
                    title=l!("ui_library.cards.why.title")
                    detail=l!("ui_library.cards.detail")
                    icon=Icon::CircleHelp
                >
                    <Prose>{l!("ui_library.cards.why.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.cards.limits.title")
                    icon=Icon::Ban
                    meta="v1".to_owned()
                >
                    <Prose>{l!("ui_library.cards.limits.body")}</Prose>
                </CollapsibleCard>

                // No icon and no meta: the header collapses to a title and a
                // chevron, which is what the component looks like at its
                // smallest and is worth seeing beside the fuller ones.
                <CollapsibleCard title=l!("ui_library.cards.usage.title")>
                    <Prose>{l!("ui_library.cards.usage.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.cards.open.title")
                    detail=l!("ui_library.cards.open.detail")
                    icon=Icon::Eye
                    open=true
                >
                    <Prose>{l!("ui_library.cards.open.body")}</Prose>
                </CollapsibleCard>
            </div>
        </Panel>
    }
}

/// What is agreed to be built, and where each one has got to.
///
/// Drawn with the card it is the roadmap for, which is deliberate: the entry
/// for a component nobody has written yet still demonstrates the one that is
/// finished.
#[component]
fn roadmap_tab() -> impl IntoView {
    view! {
        <Panel title=l!("ui_library.roadmap.title") description=l!("ui_library.roadmap.detail")>
            <div class="space-y-3">
                <CollapsibleCard
                    title=l!("ui_library.cards.title")
                    detail=l!("ui_library.cards.detail")
                    icon=Icon::LayoutGrid
                >
                    <Status tone=Tone::Success label=l!("ui_library.status.built") />
                    <Prose>{l!("ui_library.roadmap.cards.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.roadmap.editor.title")
                    detail=l!("ui_library.roadmap.editor.detail")
                    icon=Icon::Pencil
                >
                    <Status tone=Tone::Brand label=l!("ui_library.status.next") />
                    <Prose>{l!("ui_library.roadmap.editor.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.roadmap.select.title")
                    detail=l!("ui_library.roadmap.select.detail")
                    icon=Icon::Search
                >
                    <Status tone=Tone::Neutral label=l!("ui_library.status.planned") />
                    <Prose>{l!("ui_library.roadmap.select.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.roadmap.rows.title")
                    detail=l!("ui_library.roadmap.rows.detail")
                    icon=Icon::ClipboardList
                >
                    <Status tone=Tone::Neutral label=l!("ui_library.status.planned") />
                    <Prose>{l!("ui_library.roadmap.rows.body")}</Prose>
                </CollapsibleCard>
            </div>
        </Panel>
    }
}

/// A paragraph inside a card, measured so it stays readable.
///
/// `max-w-long` because a card is as wide as the page and a line of prose
/// that wide is one the eye loses its place in. The grids and tables around it
/// want the full width; this does not.
#[component]
fn prose(children: Children) -> impl IntoView {
    view! {
        <p class="max-w-long text-sm leading-relaxed text-content-muted">{children()}</p>
    }
}

/// Where a roadmap entry has got to.
#[component]
fn status(label: String, tone: Tone) -> impl IntoView {
    view! {
        <div class="mb-2">
            <Badge label=label tone=tone />
        </div>
    }
}
