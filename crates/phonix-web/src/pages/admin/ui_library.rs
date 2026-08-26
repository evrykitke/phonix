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

use phonix_core::i18n::Message;

use crate::components::page::{Badge, PageHeader, Panel, Tone};
use crate::i18n::t;
use crate::icons::Icon;
use crate::l;
use crate::ui::card::CollapsibleCard;
use crate::ui::editor::{EDITOR_GZIP_BYTES, RichText};
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
        Tab::new("editor", l!("ui_library.tab.editor"), || {
            view! { <EditorTab /> }.into_any()
        })
        .icon(Icon::Pencil),
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
                    <Status tone=Tone::Success label=l!("ui_library.status.built") />
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

/// [`RichText`], writable and read-only, with what it is holding underneath.
///
/// Two editors on one page on purpose: the bundle is fetched once and both
/// mount from it, which is the arrangement `ui::editor::browser` exists to
/// guarantee and the only way to see that it does.
#[component]
fn editor_tab() -> impl IntoView {
    // Seeded rather than empty. An editor with nothing in it demonstrates a
    // border; what wants looking at is how a heading, a list and a table are
    // set, and whether the read-only copy sets them the same way.
    let document = RwSignal::new(SPECIMEN.to_owned());

    // Compiled in by the build script rather than guessed at, and gzipped
    // rather than raw: the server compresses what it serves, so that is the
    // number somebody deciding whether to add an extension has to weigh.
    let weight = t(&Message::new("ui_library.editor.weight")
        .arg("size", (EDITOR_GZIP_BYTES / 1024).to_string()));

    view! {
        <div class="space-y-3">
            <Panel title=l!("ui_library.editor.title") description=l!("ui_library.editor.detail")>
                <div class="space-y-3">
                    <p class="text-xs text-content-subtle">{weight}</p>
                    <RichText value=document label=l!("ui_library.editor.title") />
                </div>
            </Panel>

            // Beside the first rather than inside a collapsible card, for a
            // reason worth knowing: ProseMirror measures the width of a table's
            // columns when it mounts, and a card that arrives closed mounts it
            // into a subtree the browser is not laying out. It recovers on the
            // first interaction, but a demonstration that has to be poked
            // before it looks right is not one.
            <Panel
                title=l!("ui_library.editor.disabled")
                description=l!("ui_library.editor.disabled_detail")
            >
                // The same signal as above: typing in the first changes what
                // this one shows, which is also how a form pushes a reset or a
                // reloaded draft into a field.
                <RichText value=document disabled=true label=l!("ui_library.editor.disabled") />
            </Panel>

            <CollapsibleCard
                title=l!("ui_library.editor.source")
                detail=l!("ui_library.editor.stored_detail")
                icon=Icon::FileText
            >
                <pre class="max-h-64 overflow-auto rounded-control bg-surface-sunken p-3 font-mono text-2xs leading-relaxed text-content-muted">
                    {move || document.get()}
                </pre>
            </CollapsibleCard>
        </div>
    }
}

/// What the editor opens holding.
///
/// Markup rather than a catalog key: it is a specimen of *structure* - a
/// heading, a list, a link, a table - and translating the words inside it
/// would not make the structure any clearer to somebody reading it in German.
const SPECIMEN: &str = concat!(
    "<h2>Payment terms</h2>",
    "<p>Net <strong>30 days</strong> from the date of invoice. ",
    "Late payment carries interest at the statutory rate.</p>",
    "<ul><li>Bank transfer preferred</li><li>Reference the invoice number</li></ul>",
    "<table><tbody>",
    "<tr><th>Method</th><th>Settles in</th></tr>",
    "<tr><td>Transfer</td><td>1-2 days</td></tr>",
    "<tr><td>Card</td><td>Same day</td></tr>",
    "</tbody></table>",
    "<p>See <a href=\"https://example.com/terms\">the full terms</a>.</p>",
);

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
