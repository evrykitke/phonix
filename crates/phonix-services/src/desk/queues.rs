//! How far behind the background work is, workspace by workspace.
//!
//! The four loops in `phonix-server`'s `jobs.rs` - verifier, relay, sweeper,
//! prune - run in that process, in memory, with no shared state anything else
//! can read. So this is deliberately **not** a screen over the loops. It is a
//! screen over the tables they work on, which is the part that survives a
//! restart and is the part worth watching:
//!
//! | Question | Answered by |
//! | --- | --- |
//! | Is the verifier keeping up? | uploads still `received`, and how old |
//! | Did a worker die holding a job? | uploads stuck at `verifying` |
//! | Is the broker reachable? | unpublished outbox rows, and how old |
//!
//! A loop that has stopped shows up here within a minute as a number that
//! stops going down - which is the symptom anybody would actually notice, and
//! it needs no instrumentation in the server at all.
//!
//! # Counts and timestamps, and nothing else
//!
//! Desk may not read a workspace's business data (ADR 0005 section 6), and an
//! upload's file name or an event's payload is exactly that. Both reads here
//! are aggregates by construction, in the repository layer, so this module has
//! no opportunity to widen them by accident.
//!
//! # It opens one pool per workspace, per page load
//!
//! There is one database per tenant, so there is no single query that can
//! answer this. The pools are opened lazily, used for two aggregate statements
//! and closed. That is why this is its own page behind its own navigation
//! entry rather than a panel on the home page: it costs a connection per
//! workspace, and the workspace list must stay cheap enough to reload without
//! thinking about it.

use phonix_config::DatabaseConfig;
use phonix_core::TenantSlug;
use phonix_db::files::{self, QueueDepth};
use phonix_db::outbox::{self, Backlog};
use phonix_db::tenancy::catalog::Catalog;

use crate::error::ServiceResult;

/// One workspace's outstanding background work.
pub struct WorkspaceQueues {
    pub slug: TenantSlug,
    pub display_name: String,
    /// `None` when the workspace's database could not be read - suspended
    /// workspaces are skipped before this, so a `None` here is a real problem
    /// and is shown as one rather than as a zero.
    pub uploads: Option<QueueDepth>,
    pub outbox: Option<Backlog>,
    pub error: Option<String>,
}

impl WorkspaceQueues {
    /// Whether anything here is waiting. Drives the ordering, so the workspace
    /// with a problem is not on the second screen.
    pub fn outstanding(&self) -> u64 {
        let uploads = self
            .uploads
            .as_ref()
            .map(|q| q.waiting + q.in_flight + q.failed)
            .unwrap_or(0);
        let outbox = self.outbox.as_ref().map(|b| b.unpublished).unwrap_or(0);

        uploads + outbox
    }
}

/// Read every serving workspace's queues.
///
/// Suspended, archived, unlicensed and still-provisioning workspaces are
/// skipped: `jobs.rs` does not run their background work either, so a backlog
/// there is expected rather than a fault, and listing it would bury the ones
/// that matter.
pub async fn survey(
    catalog: &Catalog,
    database: &DatabaseConfig,
) -> ServiceResult<Vec<WorkspaceQueues>> {
    let mut out = Vec::new();

    for tenant in catalog.list().await? {
        if !tenant.serves_traffic() {
            continue;
        }

        let pool = phonix_db::tenant_pool(database, &tenant.database_name);

        let uploads = files::queue_depth(&pool).await;
        let backlog = outbox::backlog(&pool).await;

        // Opened for these two statements only. The registry in the server
        // process keeps its own; this one must not outlive the page.
        pool.close().await;

        // One workspace being unreadable is not a reason to answer nothing for
        // the rest - the same rule the migration sweep follows, and for the
        // same reason.
        let error = uploads
            .as_ref()
            .err()
            .or(backlog.as_ref().err())
            .map(ToString::to_string);

        if let Some(error) = &error {
            tracing::warn!(tenant = %tenant.slug, error, "could not read a workspace's queues");
        }

        out.push(WorkspaceQueues {
            slug: tenant.slug.clone(),
            display_name: tenant.display_name.clone(),
            uploads: uploads.ok(),
            outbox: backlog.ok(),
            error,
        });
    }

    // Busiest first. A page sorted by slug puts the workspace with a thousand
    // stuck uploads wherever the alphabet happens to place it.
    out.sort_by(|a, b| {
        b.outstanding()
            .cmp(&a.outstanding())
            .then_with(|| a.slug.as_str().cmp(b.slug.as_str()))
    });

    Ok(out)
}
