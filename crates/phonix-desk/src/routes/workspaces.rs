//! The home page: the workspaces this deployment holds.
//!
//! Step 3 of the build order fills this in properly - schema version against
//! `schema_fingerprint()`, a detail page, dependency health. What is here now
//! is the honest half: the list the catalog can already answer, and a plain
//! statement of what Desk cannot do yet.
//!
//! Saying so on the page rather than leaving a convincing-looking screen is the
//! point. A console that shows a workspace and offers no way to act on it is
//! only misleading if it does not admit that is where the build stopped.

use axum::extract::State;
use axum::response::Response;
use phonix_core::TenantStatus;

use crate::html::{Page, esc};
use crate::routes::{SignedIn, html_response, internal_error};
use crate::state::DeskState;

pub async fn index(SignedIn(caller): SignedIn, State(state): State<DeskState>) -> Response {
    let tenants = match state.catalog.list().await {
        Ok(tenants) => tenants,
        Err(err) => return internal_error(err, "listing workspaces"),
    };

    let rows = tenants
        .iter()
        .map(|tenant| {
            format!(
                r#"<tr>
  <td class="mono">{slug}</td>
  <td>{name}</td>
  <td><span class="pill">{status}</span></td>
  <td class="mono">{version}</td>
  <td>{created}</td>
</tr>"#,
                slug = esc(tenant.slug.as_str()),
                name = esc(&tenant.display_name),
                status = esc(tenant.status.as_str()),
                version = esc(tenant.schema_version.as_deref().unwrap_or("-")),
                created = tenant.created_at.format("%Y-%m-%d"),
            )
        })
        .collect::<String>();

    let serving = tenants
        .iter()
        .filter(|tenant| tenant.status.serves_traffic())
        .count();
    let stuck = tenants
        .iter()
        .filter(|tenant| tenant.status == TenantStatus::Provisioning)
        .count();

    // Named because it is the reason a workspace list is worth having at all:
    // until now a workspace stuck part-way through provisioning was invisible.
    let stuck_note = if stuck > 0 {
        format!(
            r#"<p class="notice bad">{stuck} workspace(s) are still <code>provisioning</code>.
               A crash between creating the database and marking the row leaves them there,
               serving nothing. Retrying them is the next thing Desk learns to do.</p>"#
        )
    } else {
        String::new()
    };

    let body = format!(
        r#"<div class="panel">
  <h1>Workspaces</h1>
  <p class="lede">{total} in the catalog, {serving} serving traffic.</p>
  {stuck_note}
  <table>
    <thead><tr><th>Slug</th><th>Name</th><th>Status</th><th>Schema</th><th>Created</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</div>
<div class="panel">
  <h2>Not built yet</h2>
  <p class="lede">Desk can sign you in and manage its own accounts. Everything below
     is written down in <code>docs/adr/0005-phonix-desk.md</code> and is not here yet:</p>
  <ul>
    <li>Retrying a stuck <code>provisioning</code>, and migrating outdated workspaces.</li>
    <li>Suspending and resuming - the mechanism exists and nothing calls it.</li>
    <li>Licences: whether a workspace is authorized to be here, and until when.</li>
    <li>Creating a workspace, with its licence and its owner invitation.</li>
    <li>The audit trail's own screen, and the job queues.</li>
  </ul>
</div>"#,
        total = tenants.len(),
        serving = serving,
        stuck_note = stuck_note,
        rows = rows,
    );

    html_response(
        Page::new("Workspaces", body)
            .signed_in_as(&caller.user.display_name)
            .environment(state.environment())
            .render(),
    )
}
