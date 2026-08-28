//! The permission tree, with checkboxes.
//!
//! One component, two callers: the role editor and the per-account editor. They
//! differ in what they do with the answer, not in how the tree behaves, so the
//! tree does not know which one it is inside.
//!
//! # The two rules that make it usable
//!
//! **Ticking a child ticks its ancestors.** `Users.Create` without
//! `Administration` is an account that may create users and cannot open the
//! page that does it. **Unticking a parent unticks its subtree**, for the
//! mirror-image reason.
//!
//! Both are [`PermissionSet::grant`] and [`PermissionSet::revoke`] - the same
//! functions the server applies to whatever is submitted. The screen and the
//! server therefore cannot disagree about what a click meant, which is the
//! failure this component exists to avoid: a tree that lets you save a
//! selection the server then quietly rewrites.
//!
//! # Finding a permission, and hiding the ones you are not looking for
//!
//! Three controls, and they compose:
//!
//! * **the filter box** matches a label, a dotted name or a description;
//! * **only selected** hides everything not ticked;
//! * **the chevrons** shut a branch you are done with.
//!
//! The first two *suspend* the third. A branch that stayed shut while a filter
//! was running would hide the match inside it, which is a search that reports
//! nothing and is not wrong - the worst kind. So while anything is being
//! filtered the chevrons are not drawn at all, rather than drawn and ignored.
//!
//! A row survives a filter if **it** matches or **something beneath it** does.
//! Without that, searching "Delete" would show two leaves with no indication of
//! what they are the delete of.
//!
//! # An app the workspace has not switched on has no rows here
//!
//! Its permissions still exist - they are compiled in, and another workspace
//! holds them - but this one cannot. Both grant-writing services prune through
//! `for_enabled_apps` before they write, so a tick beside Books in a workspace
//! without Books is a control that is silently ignored on save. Offering it and
//! discarding it is worse than not offering it, so the tree narrows to what
//! this workspace can actually hold: `phonix_core::apps::covers` decides, and
//! [`crate::apps::InstalledApps`] supplies the list.
//!
//! The scope reaches further than the rows. A branch's `2/5` and its
//! part-ticked state count only what is in scope, or `Pages` would sit
//! permanently partial over children nobody can ever grant; and "select all"
//! selects what can be saved, or it would tick rows that vanish on submit.
//!
//! # Annotations
//!
//! A caller may pass `annotate` to label individual rows - "from role",
//! "denied for this user". The tree renders the label and stays out of the
//! meaning; deciding what a tick implies about storage is
//! [`phonix_core::authorization::grants`]'s job.

use std::collections::BTreeSet;

use leptos::prelude::*;
use phonix_core::authorization::{
    DEFINITIONS, PermissionDefinition, PermissionSet, ancestors, children, is_descendant_of,
};

use crate::apps::InstalledApps;
use crate::icons::{Icon, IconSize};
use crate::l;

/// Which permissions this workspace can hold at all.
///
/// `None` is *not resolved yet*, and it means show everything. The opposite
/// default would draw a third of the tree and then grow it, which moves rows
/// under somebody's pointer; narrowing a full tree a moment later never does.
/// In practice it is resolved before the first paint - the shell fetches it
/// with a blocking resource - and `None` is only reached by a component
/// rendered outside a shell, including every test in this file.
type Scope<'a> = Option<&'a [String]>;

/// Whether this workspace could hold `name`.
fn in_scope(scope: Scope<'_>, name: &str) -> bool {
    scope.is_none_or(|enabled| phonix_core::apps::covers(enabled, name))
}

/// Every definition this workspace could hold, in tree order.
fn definitions(scope: Scope<'_>) -> impl Iterator<Item = &'static PermissionDefinition> + use<'_> {
    DEFINITIONS
        .iter()
        .filter(move |definition| in_scope(scope, definition.name))
}

/// The children of `parent` that this workspace could hold.
///
/// Used rather than [`children`] wherever the answer decides what a *row*
/// says, because a chevron over an empty branch and a tally that counts
/// ungrantable children are both the tree lying about itself.
fn scoped_children<'a>(
    scope: Scope<'a>,
    parent: &'static str,
) -> impl Iterator<Item = &'static PermissionDefinition> + use<'a> {
    children(Some(parent)).filter(move |child| in_scope(scope, child.name))
}

/// A tri-state, because a parent with some of its children ticked is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tick {
    Off,
    /// Ticked, and everything beneath it is too.
    On,
    /// Ticked, but only some of what is beneath it.
    Partial,
}

/// Whether one definition survives the filters on its own account.
///
/// The dotted name is searched as well as the label, so "Administration.Users"
/// finds the branch and "users.delete" finds the leaf - which is how the name
/// is written in code, and therefore how somebody arriving from a code review
/// will type it.
fn matches(
    definition: &PermissionDefinition,
    selected: &PermissionSet,
    query: &str,
    only_selected: bool,
) -> bool {
    if only_selected && !selected.is_granted(definition.name) {
        return false;
    }

    if query.is_empty() {
        return true;
    }

    definition.display_name.to_lowercase().contains(query)
        || definition.name.to_lowercase().contains(query)
        || definition
            .description
            .is_some_and(|description| description.to_lowercase().contains(query))
}

/// Which rows to draw, in tree order.
///
/// A pure function of the four things that decide it, so what the tree shows
/// can be asserted without rendering anything - see the tests at the foot of
/// this file.
fn visible(
    selected: &PermissionSet,
    query: &str,
    only_selected: bool,
    collapsed: &BTreeSet<&'static str>,
    scope: Scope<'_>,
) -> Vec<&'static PermissionDefinition> {
    let query = query.trim().to_lowercase();
    let filtering = !query.is_empty() || only_selected;

    definitions(scope)
        .filter(|definition| {
            // Kept when it matches, or when something beneath it does. A match
            // three levels down shown without its branch is a permission with
            // no indication of what it is the delete of.
            let wanted = matches(definition, selected, &query, only_selected)
                || definitions(scope).any(|other| {
                    is_descendant_of(other.name, definition.name)
                        && matches(other, selected, &query, only_selected)
                });

            if !wanted {
                return false;
            }

            // Collapse applies only when nothing is being filtered - see the
            // module note on why a shut branch must not swallow a match.
            filtering
                || !ancestors(definition.name)
                    .iter()
                    .any(|ancestor| collapsed.contains(*ancestor))
        })
        .collect()
}

/// How much of what lies beneath `name` is ticked.
///
/// Rendered on a branch as `2/5`, which is the one thing a checkbox cannot say:
/// a partial tick tells you *that* some of it is on, and this tells you how
/// much, without opening it.
fn tally(selected: &PermissionSet, name: &str, scope: Scope<'_>) -> (usize, usize) {
    definitions(scope)
        .filter(|definition| is_descendant_of(definition.name, name))
        .fold((0, 0), |(granted, total), definition| {
            (
                granted + usize::from(selected.is_granted(definition.name)),
                total + 1,
            )
        })
}

/// Every branch in the tree - what "collapse all" shuts.
fn branches(scope: Scope<'_>) -> BTreeSet<&'static str> {
    definitions(scope)
        .filter(|definition| scoped_children(scope, definition.name).next().is_some())
        .map(|definition| definition.name)
        .collect()
}

/// The permission tree as a list of checkboxes.
///
/// `selection` is read *and written*: this component owns the interaction, and
/// the page around it owns saving. That split is what lets the same tree serve
/// a form that saves on submit and one that saves per click.
#[component]
pub fn permission_tree(
    /// What is currently ticked. Mutated in place as boxes are clicked.
    selection: RwSignal<PermissionSet>,
    /// Whether anything may be clicked at all.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// An optional note for one permission - where it came from, why it is off.
    #[prop(optional)]
    annotate: Option<Callback<&'static str, Option<String>>>,
) -> impl IntoView {
    let scope = InstalledApps::get();
    let query = RwSignal::new(String::new());
    let only_selected = RwSignal::new(false);
    // Opens fully expanded. The tree is short enough to read whole, and a
    // screen that opens with everything shut hides the thing it is for.
    let collapsed = RwSignal::new(BTreeSet::<&'static str>::new());

    let filtering = move || !query.get().trim().is_empty() || only_selected.get();

    // The denominator on both counters below, and what "select all" selects.
    // `DEFINITIONS.len()` would be counting permissions this workspace cannot
    // hold, so "12 of 40" would never reach 40.
    let in_reach = move || {
        let enabled = scope.get();
        definitions(enabled.as_deref()).count()
    };

    let rows = move || {
        let query = query.get();
        let only = only_selected.get();
        let collapsed = collapsed.get();

        // Read untracked unless the selection is itself part of the filter.
        // Otherwise every tick rebuilds every row, and a list that redraws
        // under the pointer is a list that loses the row you were about to
        // click next.
        let enabled = scope.get();
        let enabled = enabled.as_deref();

        if only {
            selection.with(|selected| visible(selected, &query, true, &collapsed, enabled))
        } else {
            selection
                .with_untracked(|selected| visible(selected, &query, false, &collapsed, enabled))
        }
    };

    view! {
        <div class="divide-y divide-edge rounded-card border border-edge bg-surface-raised">
            <div class="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
                <div class="flex items-center gap-2 text-xs font-medium uppercase tracking-wide text-content-subtle">
                    <Icon icon=Icon::ListTree size=IconSize::Xs />
                    {l!("permissions.title")}
                </div>

                <div class="flex flex-wrap items-center gap-1">
                    <Show when=move || !filtering() fallback=|| ()>
                        <BulkButton
                            label=l!("permissions.expand_all")
                            on_click=Callback::new(move |()| collapsed.set(BTreeSet::new()))
                        />
                        <BulkButton
                            label=l!("permissions.collapse_all")
                            on_click=Callback::new(move |()| {
                                let enabled = scope.get();
                                collapsed.set(branches(enabled.as_deref()));
                            })
                        />
                        <span class="mx-1 h-4 w-px bg-edge" aria-hidden="true"></span>
                    </Show>
                    <BulkButton
                        label=l!("permissions.select_all")
                        disabled=disabled
                        on_click=Callback::new(move |()| {
                            // Scoped, or the tree would tick rows that the
                            // service prunes on the way in and the screen
                            // would report a selection nobody saved.
                            let all = match scope.get() {
                                Some(enabled) => PermissionSet::all().for_enabled_apps(&enabled),
                                None => PermissionSet::all(),
                            };
                            selection.set(all);
                        })
                    />
                    <BulkButton
                        label=l!("permissions.clear")
                        disabled=disabled
                        on_click=Callback::new(move |()| selection.set(PermissionSet::new()))
                    />
                </div>
            </div>

            <div class="flex flex-wrap items-center gap-2 px-3 py-2">
                <div class="flex h-8 min-w-[12rem] flex-1 items-center gap-2 rounded-control border border-edge bg-surface px-2 sm:max-w-sm">
                    <Icon icon=Icon::Search size=IconSize::Xs class="shrink-0 text-content-subtle" />
                    <input
                        type="search"
                        // `control-bare`: the border belongs to the box around
                        // this input, not to the input.
                        class="control-bare w-full bg-transparent text-sm text-content outline-none"
                        placeholder=l!("permissions.search")
                        aria-label=l!("permissions.search_label")
                        prop:value=move || query.get()
                        on:input=move |event| query.set(event_target_value(&event))
                    />
                    <Show when=move || !query.get().is_empty() fallback=|| ()>
                        <button
                            type="button"
                            class="shrink-0 text-content-subtle hover:text-content"
                            aria-label=l!("permissions.clear_filter")
                            on:click=move |_| query.set(String::new())
                        >
                            <Icon icon=Icon::X size=IconSize::Xs />
                        </button>
                    </Show>
                </div>

                <button
                    type="button"
                    class=move || {
                        let state = if only_selected.get() {
                            "border-brand bg-brand-subtle text-brand"
                        } else {
                            "border-edge text-content-muted hover:bg-surface-hover hover:text-content"
                        };
                        format!(
                            "flex h-8 shrink-0 items-center gap-1.5 rounded-control border px-2 text-xs {state}",
                        )
                    }
                    aria-pressed=move || if only_selected.get() { "true" } else { "false" }
                    on:click=move |_| only_selected.update(|only| *only = !*only)
                >
                    <Icon icon=Icon::Filter size=IconSize::Xs />
                    {l!("permissions.only_selected")}
                </button>
            </div>

            <ul class="p-1">
                // One boundary for the list and for everything a row draws inside it.
                // The installed apps decide which definitions exist at all, and within a
                // row they decide whether there is a chevron or a spacer and whether the
                // tally badge is there - all of it a different shape on the server,
                // where the blocking resource has not resolved during this pass, than in
                // the browser, where it has. Nothing under here reads anything else, so
                // the boundary costs one wait that the blocking resource was making the
                // document take anyway.
                <Suspense fallback=|| ()>
                    {move || {
                        let shown = rows();

                        if shown.is_empty() {
                            return view! {
                                <li class="px-3 py-6 text-center text-sm text-content-subtle">
                                    {l!("permissions.no_match")}
                                </li>
                            }
                                .into_any();
                        }

                        shown
                            .into_iter()
                            .map(|definition| {
                                view! {
                                    <PermissionRow
                                        definition=definition
                                        selection=selection
                                        disabled=disabled
                                        collapsed=collapsed
                                        filtering=Signal::derive(filtering)
                                        annotate=annotate
                                    />
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </Suspense>
            </ul>

            <div class="flex flex-wrap items-center justify-between gap-2 px-3 py-2 text-xs text-content-subtle">
                // The counters read the installed apps too, and only to produce text,
                // which corrects itself rather than killing the page. Behind the
                // boundary anyway: the alternative is shipping a document that counts
                // against every permission in the build, including apps this workspace
                // has never installed, and then quietly rewriting the number.
                <Suspense fallback=|| ()>
                    <span>
                        {move || {
                            let count = selection.with(PermissionSet::len);
                            l!(
                                "permissions.selected_of",
                                count = count.to_string(),
                                total = in_reach().to_string(),
                            )
                        }}
                    </span>
                    <Show when=move || filtering() fallback=|| ()>
                        <span>
                            {move || {
                                let shown = rows().len();
                                l!(
                                    "permissions.shown_of",
                                    shown = shown.to_string(),
                                    total = in_reach().to_string(),
                                )
                            }}
                        </span>
                    </Show>
                </Suspense>
            </div>
        </div>
    }
}

/// One permission, indented to its depth in the tree.
///
/// Rendered as a flat list rather than nested `<ul>`s. The tree is declared
/// depth-first, so indentation alone reproduces its shape - and a flat list
/// survives filtering, where a nested one would have to rebuild its nesting
/// every keystroke around whichever rows happened to survive.
///
/// The row is two buttons rather than one: the chevron opens the branch, the
/// rest ticks the box. They were one control before this, which meant a branch
/// could not be shut without granting it.
#[component]
fn permission_row(
    definition: &'static PermissionDefinition,
    selection: RwSignal<PermissionSet>,
    #[prop(into)] disabled: Signal<bool>,
    collapsed: RwSignal<BTreeSet<&'static str>>,
    /// Whether a filter is running, in which case no chevron is drawn.
    #[prop(into)]
    filtering: Signal<bool>,
    // Not `#[prop(optional)]`: the parent already holds an `Option` and passes
    // it straight through, which an optional prop would want unwrapped first.
    annotate: Option<Callback<&'static str, Option<String>>>,
) -> impl IntoView {
    let scope = InstalledApps::get();
    let name = definition.name;
    let depth = definition.depth();
    // Scoped: a branch whose only children belong to an app this workspace has
    // not switched on is a leaf here, and drawing a chevron on it would open
    // nothing.
    let has_children = move || {
        let enabled = scope.get();
        scoped_children(enabled.as_deref(), name).next().is_some()
    };

    let tick = move || {
        let enabled = scope.get();
        let enabled = enabled.as_deref();

        selection.with(|selected| {
            if !selected.is_granted(name) {
                return Tick::Off;
            }
            if has_children()
                && !scoped_children(enabled, name).all(|child| selected.is_granted(child.name))
            {
                // Ticked itself, but not everything under it. Worth showing,
                // because "Users" alone means read-only and "Users" with its
                // children means read-write, and the checkbox cannot say that.
                return Tick::Partial;
            }
            Tick::On
        })
    };

    let toggle = move |_| {
        if disabled.get() {
            return;
        }
        selection.update(|selected| {
            if selected.is_granted(name) {
                // Takes the subtree with it: leaving `Users.Delete` behind
                // after dropping `Users` is an orphaned grant.
                selected.revoke(name);
            } else {
                // Brings the ancestors with it, for the mirror reason.
                selected.grant(name);
            }
        });
    };

    let is_shut = move || collapsed.with(|collapsed| collapsed.contains(name));
    let note = move || annotate.and_then(|annotate| annotate.run(name));

    view! {
        <li>
            <div
                class="flex items-start gap-1 rounded-control hover:bg-surface-hover"
                style=format!("padding-left:{}rem", depth as f32 * 1.25)
            >
                {move || {
                    if has_children() && !filtering.get() {
                        view! {
                            <button
                                type="button"
                                class="mt-1 grid size-5 shrink-0 place-items-center rounded text-content-subtle hover:bg-surface-active hover:text-content"
                                aria-expanded=move || if is_shut() { "false" } else { "true" }
                                aria-label=move || {
                                    if is_shut() {
                                        l!(
                                            "permissions.open_group",
                                            group = definition.display_name
                                        )
                                    } else {
                                        l!(
                                            "permissions.close_group",
                                            group = definition.display_name
                                        )
                                    }
                                }
                                on:click=move |_| {
                                    collapsed
                                        .update(|collapsed| {
                                            if !collapsed.remove(name) {
                                                collapsed.insert(name);
                                            }
                                        });
                                }
                            >
                                {move || {
                                    if is_shut() {
                                        view! { <Icon icon=Icon::ChevronRight size=IconSize::Xs /> }
                                            .into_any()
                                    } else {
                                        view! { <Icon icon=Icon::ChevronDown size=IconSize::Xs /> }
                                            .into_any()
                                    }
                                }}
                            </button>
                        }
                            .into_any()
                    } else {
                        // A spacer, so a leaf's checkbox lines up under its
                        // siblings' rather than under their chevrons.
                        view! { <span class="size-5 shrink-0" aria-hidden="true"></span> }
                            .into_any()
                    }
                }}

                <button
                    type="button"
                    class=move || {
                        let state = if tick() == Tick::Off {
                            "text-content-muted"
                        } else {
                            "text-content"
                        };
                        let weight = if depth == 0 { "font-semibold" } else { "font-medium" };
                        format!(
                            "flex min-w-0 flex-1 items-start gap-2 rounded-control px-1 py-1 \
                             text-left disabled:cursor-not-allowed disabled:opacity-60 \
                             {state} {weight}",
                        )
                    }
                    disabled=move || disabled.get()
                    aria-pressed=move || if tick() == Tick::Off { "false" } else { "true" }
                    on:click=toggle
                >
                    <Checkbox tick=Signal::derive(tick) />

                    <span class="min-w-0 flex-1">
                        <span class="flex flex-wrap items-baseline gap-x-2">
                            <span class="text-sm">{definition.display_name}</span>
                            <code class="break-all font-mono text-2xs font-normal text-content-subtle">
                                {name}
                            </code>
                            {move || {
                                note()
                                    .map(|note| {
                                        view! {
                                            <span class="rounded-full bg-surface-sunken px-1.5 py-0.5 text-2xs font-normal text-content-subtle">
                                                {note}
                                            </span>
                                        }
                                    })
                            }}
                        </span>
                        {definition
                            .description
                            .map(|description| {
                                view! {
                                    <span class="block text-xs font-normal text-content-subtle">
                                        {description}
                                    </span>
                                }
                            })}
                    </span>
                </button>

                {move || {
                    has_children()
                        .then(|| {
                            view! {
                                <span class="mt-1 shrink-0 rounded-full bg-surface-sunken px-1.5 py-0.5 font-mono text-2xs text-content-subtle">
                                    {move || {
                                        let enabled = scope.get();
                                        let (granted, total) = selection
                                            .with(|selected| {
                                                tally(selected, name, enabled.as_deref())
                                            });
                                        format!("{granted}/{total}")
                                    }}
                                </span>
                            }
                        })
                }}
            </div>
        </li>
    }
}

/// The box itself. A styled `<span>`, not an `<input>`: the whole row is the
/// control, and a real checkbox inside a button is a nested interactive element
/// that screen readers and the browser both handle badly.
#[component]
fn checkbox(tick: Signal<Tick>) -> impl IntoView {
    view! {
        <span
            class=move || {
                let state = match tick.get() {
                    Tick::Off => "border-edge-strong",
                    Tick::On | Tick::Partial => "border-brand bg-brand text-on-brand",
                };
                format!(
                    "mt-0.5 grid size-4 shrink-0 place-items-center rounded border transition-colors {state}",
                )
            }
            aria-hidden="true"
        >
            {move || match tick.get() {
                Tick::Off => ().into_any(),
                Tick::On => view! { <Icon icon=Icon::Check size=IconSize::Xs /> }.into_any(),
                // A dash, the universal "some of this".
                Tick::Partial => view! { <Icon icon=Icon::Minus size=IconSize::Xs /> }.into_any(),
            }}
        </span>
    }
}

#[component]
fn bulk_button(
    #[prop(into)] label: String,
    /// Left off by the view controls: expanding a branch is not a change to
    /// anything, so it stays available on a tree nobody may edit.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="rounded-control px-2 py-1 text-xs text-content-muted hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50"
            disabled=move || disabled.get()
            on:click=move |_| on_click.run(())
        >
            {label}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::names as perms;

    use super::*;

    /// Unscoped: a workspace with every app, which is what these tests are
    /// about. [`scoped_to`] is the one that narrows.
    fn shown(
        selected: &PermissionSet,
        query: &str,
        only_selected: bool,
        collapsed: &[&'static str],
    ) -> Vec<&'static str> {
        let collapsed: BTreeSet<&'static str> = collapsed.iter().copied().collect();

        visible(selected, query, only_selected, &collapsed, None)
            .into_iter()
            .map(|definition| definition.name)
            .collect()
    }

    /// What the tree draws for a workspace holding exactly these apps.
    fn scoped_to(enabled: &[&str]) -> Vec<&'static str> {
        let enabled: Vec<String> = enabled.iter().map(|id| (*id).to_owned()).collect();

        visible(
            &PermissionSet::new(),
            "",
            false,
            &BTreeSet::new(),
            Some(&enabled),
        )
        .into_iter()
        .map(|definition| definition.name)
        .collect()
    }

    #[test]
    fn with_nothing_typed_the_whole_tree_is_there() {
        assert_eq!(
            shown(&PermissionSet::new(), "", false, &[]).len(),
            DEFINITIONS.len()
        );
    }

    #[test]
    fn a_match_keeps_the_branch_it_hangs_from() {
        // Otherwise "Delete" is two identical rows and no way to tell which
        // one deletes users and which one deletes roles.
        let rows = shown(&PermissionSet::new(), "Delete", false, &[]);

        assert!(rows.contains(&perms::USERS_DELETE));
        assert!(rows.contains(&perms::USERS), "the branch went missing");
        assert!(rows.contains(&perms::ADMINISTRATION));
        assert!(rows.contains(&perms::PAGES));
        // ...and nothing else did.
        assert!(!rows.contains(&perms::USERS_CREATE));
    }

    #[test]
    fn a_branch_that_matches_brings_everything_under_it() {
        let rows = shown(&PermissionSet::new(), "Roles", false, &[]);

        assert!(rows.contains(&perms::ROLES));
        assert!(rows.contains(&perms::ROLES_CREATE));
        assert!(rows.contains(&perms::ROLES_CHANGE_PERMISSIONS));
    }

    #[test]
    fn the_dotted_name_is_searched_as_well_as_the_label() {
        // It is how the permission is written in code, so it is how somebody
        // arriving from a code review will type it.
        let rows = shown(
            &PermissionSet::new(),
            "administration.users.create",
            false,
            &[],
        );

        assert!(rows.contains(&perms::USERS_CREATE));
    }

    #[test]
    fn the_filter_ignores_case_and_surrounding_space() {
        assert_eq!(
            shown(&PermissionSet::new(), "  DELETE  ", false, &[]),
            shown(&PermissionSet::new(), "delete", false, &[]),
        );
    }

    #[test]
    fn a_shut_branch_hides_what_is_under_it_and_stays_visible_itself() {
        let rows = shown(&PermissionSet::new(), "", false, &[perms::USERS]);

        assert!(rows.contains(&perms::USERS));
        assert!(!rows.contains(&perms::USERS_CREATE));
        // A sibling branch is untouched.
        assert!(rows.contains(&perms::ROLES_CREATE));
    }

    #[test]
    fn a_filter_reaches_into_a_shut_branch() {
        // The failure this prevents: a search that reports nothing, is not
        // wrong, and gives no hint that the answer is behind a chevron.
        let rows = shown(&PermissionSet::new(), "Create", false, &[perms::USERS]);

        assert!(rows.contains(&perms::USERS_CREATE));
    }

    #[test]
    fn only_selected_keeps_the_branch_above_a_ticked_leaf() {
        let mut selected = PermissionSet::new();
        selected.grant(perms::AUDIT_LOGS);

        let rows = shown(&selected, "", true, &[]);

        assert!(rows.contains(&perms::AUDIT_LOGS));
        // `grant` pulled these in, so they are ticked and would show anyway -
        // the point is that the tree is not a flat list of leaves.
        assert!(rows.contains(&perms::ADMINISTRATION));
        assert!(!rows.contains(&perms::USERS_DELETE));
    }

    #[test]
    fn only_selected_on_an_empty_selection_shows_nothing() {
        assert!(shown(&PermissionSet::new(), "", true, &[]).is_empty());
    }

    #[test]
    fn the_two_filters_narrow_together_rather_than_either_winning() {
        let mut selected = PermissionSet::new();
        selected.grant(perms::USERS_CREATE);

        let rows = shown(&selected, "Delete", true, &[]);

        // Ticked but does not match, matches but is not ticked: neither
        // survives both.
        assert!(!rows.contains(&perms::USERS_CREATE));
        assert!(!rows.contains(&perms::USERS_DELETE));
    }

    #[test]
    fn a_branch_counts_only_what_lies_beneath_it() {
        let mut selected = PermissionSet::new();
        selected.grant(perms::USERS_CREATE);
        selected.grant(perms::USERS_EDIT);

        let (granted, total) = tally(&selected, perms::USERS, None);

        assert_eq!(granted, 2);
        assert_eq!(total, children(Some(perms::USERS)).count());
        // The branch itself is ticked - `grant` pulled it in - and is not part
        // of its own count, or every branch would read one better than it is.
        assert!(selected.is_granted(perms::USERS));
    }

    #[test]
    fn a_leaf_has_nothing_to_count() {
        assert_eq!(
            tally(&PermissionSet::all(), perms::USERS_DELETE, None),
            (0, 0)
        );
    }

    #[test]
    fn collapse_all_shuts_every_branch_and_no_leaves() {
        let branches = branches(None);

        assert!(branches.contains(perms::PAGES));
        assert!(branches.contains(perms::USERS));
        assert!(!branches.contains(perms::USERS_DELETE));
        assert!(!branches.contains(perms::DASHBOARD));
    }

    #[test]
    fn collapsing_the_root_leaves_exactly_the_root() {
        let rows = shown(
            &PermissionSet::new(),
            "",
            false,
            &branches(None).into_iter().collect::<Vec<_>>(),
        );

        assert_eq!(rows, vec![perms::PAGES]);
    }

    #[test]
    fn an_app_the_workspace_has_not_installed_has_no_rows() {
        // A tick beside it would be ignored on save: both grant-writing
        // services prune through `for_enabled_apps` before they write. Offering
        // a control and then discarding what it says is worse than not
        // offering it.
        let rows = scoped_to(&[]);

        assert!(!rows.iter().any(|name| name.starts_with("Pages.Sales")));
        // The always-on apps are there whatever the list says.
        assert!(rows.contains(&perms::PARTIES));
        assert!(rows.contains(&perms::USERS));
        assert!(rows.contains(&perms::PAGES));
    }

    #[test]
    fn a_workspace_that_has_subscribed_to_nothing_still_has_the_always_on_apps() {
        let rows = scoped_to(&[]);

        assert!(rows.contains(&perms::AUDIT_LOGS));
        assert!(rows.contains(&perms::APPS_INSTALL));
        // Master data is part of every workspace - nobody subscribes to it and
        // nobody can switch it off - so its permissions are always grantable.
        assert!(rows.contains(&perms::PARTIES));
        // Books is the one thing that is actually optional.
        assert!(!rows.iter().any(|name| name.starts_with("Pages.Sales")));
    }

    #[test]
    fn a_branch_counts_only_the_children_this_workspace_could_hold() {
        // `Pages` would otherwise sit permanently part-ticked over children
        // nobody in this workspace can ever be granted.
        let books_off: Vec<String> = Vec::new();

        let (_, total) = tally(&PermissionSet::new(), perms::PAGES, Some(&books_off));
        let (_, everything) = tally(&PermissionSet::new(), perms::PAGES, None);

        assert!(
            total < everything,
            "the tally has to shrink when an app is off",
        );
        assert_eq!(
            everything - total,
            DEFINITIONS
                .iter()
                .filter(|d| d.name.starts_with("Pages.Sales"))
                .count(),
            "and shrink by exactly what Books owns",
        );
    }

    #[test]
    fn select_all_in_a_narrowed_tree_is_what_the_service_would_keep() {
        // The screen and the server must not disagree about what a click meant
        // - the same rule the grant and revoke helpers exist for.
        let enabled: Vec<String> = Vec::new();
        let picked = PermissionSet::all().for_enabled_apps(&enabled);

        assert!(picked.is_granted(perms::PARTIES));
        assert!(picked.is_granted(perms::USERS));
        assert!(!picked.is_granted(perms::INVOICES));
        assert_eq!(picked.len(), scoped_to(&[]).len());
    }
}
