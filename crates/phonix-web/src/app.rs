//! Application shell and routing.

use leptos::prelude::*;
use leptos_meta::{HashedStylesheet, Meta, MetaTags, Title, provide_meta_context};
use leptos_router::components::{ParentRoute, Route, Router, Routes};
use leptos_router::path;

use crate::components::layout::Layout;
use crate::components::user_link::{OpenCard, UserCardLayer};
use crate::i18n::{self, Locale};
use crate::pages::account::AccountPage;
use crate::pages::admin::audit_event::AuditEventPage;
use crate::pages::admin::audit_logs::AuditLogsPage;
use crate::pages::admin::entity_change::EntityChangePage;
use crate::pages::admin::roles::{RoleNewPage, RolePage, RolesPage};
use crate::pages::admin::settings::SettingsPage;
use crate::pages::admin::user_edit::UserEditPage;
use crate::pages::admin::user_invite::UserInvitePage;
use crate::pages::admin::user_permissions::UserPermissionsPage;
use crate::pages::admin::users::UsersPage;
use crate::pages::auth::{
    AcceptInvitationPage, ChallengePage, ForgotPasswordPage, SignInPage, SignUpPage,
};
use crate::pages::{dashboard::DashboardPage, not_found::NotFoundPage};
use crate::theme::{Theme, ThemePreference};
use crate::ui::alert::{AlertLayer, Alerts};

/// The HTML document the server streams.
///
/// Leptos renders the whole document (not just `<body>`), so `<head>` content
/// set by `leptos_meta` anywhere in the tree lands in the real `<head>` during
/// SSR rather than being patched in after hydration.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    // Read here, on the very first element, rather than in a component further
    // down: the theme has to be on `<html>` in the bytes the server sends, or
    // the page paints light and flips to dark once the bundle boots. That flash
    // is the whole reason the preference lives in a cookie.
    //
    // `data-theme` is deliberately absent for "follow the system" - see
    // `ThemeMode::attribute`.
    let appearance = ThemePreference::from_request();

    // Resolved here, on `<html>`, for the same reason and at the same moment as
    // the theme: `lang` is what a screen reader picks a voice from and what
    // `:lang()` rules key off, and `dir` decides which way the whole page runs.
    // Both have to be in the bytes the server sends.
    //
    // It is also the language the browser half reads back during hydration, so
    // this attribute is not decoration - it is the handover.
    let catalog = i18n::current_catalog();
    let language = catalog.language();
    let overlay = i18n::overlay_json(&catalog);

    // Stamped rather than recomputed in the browser, for the same reason the
    // language is. Coverage is a fraction of `BUILTIN`, and `BUILTIN` lives in
    // two separately compiled binaries - the server's and the bundle's. They
    // are the same in a release and routinely differ for a few seconds under
    // `cargo leptos watch`, which is enough for the two halves to disagree
    // about whether the "partly translated" note exists. A node that is there
    // on one side and not the other is the fatal kind of hydration mismatch.
    let coverage = catalog.coverage();

    view! {
        <!DOCTYPE html>
        <html
            lang=language.code()
            dir=language.direction().attribute()
            class="h-full"
            data-theme=appearance.mode.attribute()
            data-accent=appearance.accent.key()
            data-i18n-coverage=coverage.to_string()
        >
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                // Injects the hashed CSS filename so a deploy cannot serve a
                // stale stylesheet from cache.
                <HashedStylesheet options=options.clone() id="leptos" />
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() />
                <MetaTags />

                // The words this document was rendered with, for the bundle to
                // hydrate against. `{}` when the page is in English, because
                // the English catalog is compiled into the bundle already.
                //
                // Before `HydrationScripts` would be tidier; it is here instead
                // because it must not delay them, and a parser reaching this
                // has already started fetching the wasm.
                <script type="application/json" id=i18n::CATALOG_ELEMENT_ID inner_html=overlay />
            </head>
            <body class="h-full bg-surface text-content antialiased">
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn app() -> impl IntoView {
    provide_meta_context();

    // Seeded from the same cookie the document was rendered with, so the
    // appearance menu opens showing what is actually on screen.
    Theme::provide(ThemePreference::from_request());

    // Above everything, because everything says something. On the server this
    // resolves from the request; in the browser it reads back what the server
    // decided, so the two halves cannot render different words.
    Locale::provide(i18n::current_catalog());

    // Above the router, so an alert raised by a save that then navigates is not
    // unmounted by its own success. See `ui::alert::host`.
    Alerts::provide();
    // Beside the alerts, and mounted at the root for the same reason: the card
    // is opened from inside a scrolling table and must not be drawn there.
    OpenCard::provide();

    view! {
        <Title text="Phonix" />
        <Meta name="description" content="Phonix" />
        <Meta name="color-scheme" content="light dark" />

        <AlertLayer />
        <UserCardLayer />

        <Router>
            <Routes fallback=NotFoundPage>
                // Every screen hangs off one parent route rather than being
                // wrapped in `<Layout>`: see `components::layout` for what that
                // wrapping costs. The path is empty, so it adds no segment.
                <ParentRoute path=path!("") view=Layout>
                    // Signed out, "/" is the sign-in screen. Once a session
                    // exists the dashboard is at "/dashboard" and the handoff
                    // endpoint sends the browser straight there.
                    <Route path=path!("/") view=SignInPage />
                    <Route path=path!("/signup") view=SignUpPage />
                    // Public, like the two above: an invitation is followed by
                    // somebody who has no session yet.
                    <Route path=path!("/invitations/accept") view=AcceptInvitationPage />
                    // Public for the plainest reason of the four: whoever is
                    // here has forgotten the password a session is made from.
                    // `landing` has to agree - see PASSWORD_RESET_PATH.
                    <Route path=path!("/forgot-password") view=ForgotPasswordPage />

                    // Half-authenticated: the password was accepted and the
                    // second factor has not been. `LoginResult::next_path`
                    // sends the browser here, so this route has to exist
                    // before anybody is allowed to enrol a factor.
                    <Route path=path!("/auth/challenge") view=ChallengePage />

                    <Route path=path!("/dashboard") view=DashboardPage />
                    <Route path=path!("/account") view=AccountPage />

                    // Administration. Each of these is named by a node in
                    // `navigation::tree` and gated on the matching permission
                    // there; the screens themselves state their own.
                    <Route path=path!("/admin/users") view=UsersPage />
                    <Route path=path!("/admin/users/invite") view=UserInvitePage />
                    <Route path=path!("/admin/users/:id/edit") view=UserEditPage />
                    <Route path=path!("/admin/users/:id/permissions") view=UserPermissionsPage />
                    <Route path=path!("/admin/roles") view=RolesPage />
                    // Before the parameter, so "new" is a screen rather than a
                    // role id that fails to parse.
                    <Route path=path!("/admin/roles/new") view=RoleNewPage />
                    <Route path=path!("/admin/roles/:id") view=RolePage />
                    <Route path=path!("/admin/settings") view=SettingsPage />
                    <Route path=path!("/admin/audit-logs") view=AuditLogsPage />
                    <Route path=path!("/admin/audit-logs/:id") view=AuditEventPage />
                    <Route path=path!("/admin/changes/:id") view=EntityChangePage />
                </ParentRoute>
            </Routes>
        </Router>
    }
}
