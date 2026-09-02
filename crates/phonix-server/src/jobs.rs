//! The background work: verifying uploads, publishing events, sweeping up.
//!
//! Three loops, all of them the same shape - wake up, walk the active tenants,
//! do a bounded amount of work in each, sleep - and all three cancellable, so a
//! shutdown drains rather than severs.
//!
//! | Loop        | What it does                                              |
//! | ----------- | --------------------------------------------------------- |
//! | verifier    | Claims received uploads and turns them into stored files   |
//! | relay       | Publishes outbox rows to RabbitMQ and marks them sent      |
//! | sweeper     | Removes quarantined bytes whose job never ran              |
//!
//! # Why the verifier polls at all
//!
//! It mostly does not. An upload is dispatched the moment its bytes are down -
//! see `files::dispatch` - so the usual path is immediate and the poll finds
//! nothing. The loop exists for the case the fast path cannot cover: a process
//! that died between writing the row and running the job. Without it those rows
//! sit at `received` for ever, and the symptom is one user whose file never
//! finished while everybody else's worked.
//!
//! That is also why the interval is measured in seconds rather than
//! milliseconds. It is a safety net, not a queue.
//!
//! # Why each loop walks every tenant
//!
//! There is one database per tenant, so there is one queue per tenant, and
//! nothing in Postgres can wait on two databases at once. A `LISTEN`-based
//! design would need a connection per tenant held open permanently, which is
//! the thing the pool registry exists to avoid. Polling a handful of indexed
//! partial indexes is cheap; holding a thousand connections is not.

use std::time::Duration;

use phonix_core::TenantSlug;
use phonix_db::{PgPool, audit as audit_db, files as files_db, outbox, settings as settings_db};
use phonix_messaging::Publisher;
use phonix_services::files::verify;
use phonix_web::state::AppState;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How many outbox rows one pass publishes per tenant.
const RELAY_BATCH: usize = 64;

/// How many stale quarantine objects one pass removes per tenant.
const SWEEP_BATCH: usize = 100;

/// How often the sweeper runs. Far less often than the verifier: what it
/// collects is measured in hours by definition.
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// How often the relay runs when the broker is healthy.
const RELAY_INTERVAL: Duration = Duration::from_secs(5);

/// How many change-trail entries one prune pass deletes per tenant.
///
/// Bounded because the first pass after somebody switches retention on can have
/// a year of entries to drop, and one unbounded `DELETE` would hold a lock
/// across all of them - on the table an administrator is looking at, in the
/// middle of the working day. The pass repeats until the batch comes back short,
/// so a large backlog is cleared over a few minutes rather than in one stall.
const PRUNE_BATCH: i64 = 1_000;

/// The most batches one tenant gets in a single pass.
///
/// A ceiling on the work, not on the backlog: whatever is left is picked up by
/// the next pass an hour later. Without it, one workspace with years of history
/// would hold the loop while every other tenant waits its turn.
const PRUNE_MAX_BATCHES: usize = 20;

/// How often retention is applied.
///
/// Hourly. Retention is measured in days, so there is nothing to gain from
/// checking more often, and every pass is a query against every tenant.
const PRUNE_INTERVAL: Duration = Duration::from_secs(3_600);

/// Everything spawned at startup, so shutdown has something to wait on.
pub struct Background {
    tasks: Vec<JoinHandle<()>>,
    shutdown: CancellationToken,
}

impl Background {
    /// Ask every loop to stop, and wait for the work in flight to finish.
    pub async fn shutdown(self) {
        self.shutdown.cancel();

        for task in self.tasks {
            if let Err(err) = task.await {
                tracing::warn!(error = %err, "a background task ended abnormally");
            }
        }
    }
}

/// Start the background loops.
pub fn spawn(state: AppState) -> Background {
    let shutdown = CancellationToken::new();
    let mut tasks = Vec::new();

    if state.config.storage.jobs.enabled {
        tasks.push(tokio::spawn(verifier_loop(state.clone(), shutdown.clone())));
        tasks.push(tokio::spawn(sweeper_loop(state.clone(), shutdown.clone())));
    } else {
        // Said out loud, because the symptom - uploads that stay at "queued"
        // for ever - looks like a bug rather than a setting.
        tracing::warn!("upload verification is disabled; uploads will not be processed");
    }

    // No publisher means `rabbitmq.enabled = false`, which is a supported way
    // to run. Events still accumulate in the outbox and are published whenever
    // a process with a broker connection next runs - which is the whole reason
    // they are written to a table rather than sent directly.
    // Unconditional, unlike the others. Retention is off in every workspace
    // until somebody sets it, so the loop costs one settings read per tenant per
    // hour on a deployment that never uses it - and a deployment where it *is*
    // set but the loop was not started is one whose table grows for ever while
    // the screen says it does not.
    tasks.push(tokio::spawn(prune_loop(state.clone(), shutdown.clone())));

    if state.publisher.is_some() {
        tasks.push(tokio::spawn(relay_loop(state.clone(), shutdown.clone())));
    } else {
        tracing::warn!("no broker connection; events will be recorded but not published");
    }

    Background { tasks, shutdown }
}

// ---------------------------------------------------------------------------
// Verifying uploads
// ---------------------------------------------------------------------------

/// Verify one upload, now.
///
/// Called on the fast path, straight after the bytes land. Claiming may fail -
/// the periodic loop might have got there first - and that is an ordinary
/// outcome rather than an error, which is what `SKIP LOCKED` buys.
pub async fn verify_one(state: &AppState, pool: &PgPool, tenant: &TenantSlug, file_id: Uuid) {
    let timeout = state.config.storage.jobs.claim_timeout_secs;

    match files_db::claim_one(pool, file_id, timeout).await {
        Ok(Some(row)) => run(state, pool, tenant, &row).await,
        // Somebody else has it. Nothing to do and nothing wrong.
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(file_id = %file_id, error = %err, "could not claim an upload");
        }
    }
}

/// Run the job for one claimed row, and decide what its failure means.
///
/// The three outcomes are deliberately different states, and choosing between
/// them is the only decision this function makes:
///
/// * the file was verified or refused - `verify` has already written the row;
/// * the work failed and might succeed later - back on the queue;
/// * the work failed and will not succeed - given up on, with the reason kept.
async fn run(state: &AppState, pool: &PgPool, tenant: &TenantSlug, row: &files_db::FileRow) {
    let outcome = verify::verify(pool, state.files(), tenant, row).await;

    let Err(err) = outcome else {
        return;
    };

    let attempts = row.attempts;
    let max = state.config.storage.jobs.max_attempts;
    let message = err.to_string();

    // Note which of the two questions is asked first: a failure that cannot
    // succeed on a retry is given up on immediately, however many attempts are
    // left. Burning five attempts on an unparseable key would only delay the
    // same answer.
    if verify::is_retryable(&err) && attempts < max {
        tracing::warn!(
            file_id = %row.id,
            attempt = attempts,
            max_attempts = max,
            error = %message,
            "upload verification failed; it will be tried again"
        );

        if let Err(err) = files_db::release(pool, row.id, &message).await {
            tracing::error!(file_id = %row.id, error = %err, "could not requeue an upload");
        }
        return;
    }

    tracing::error!(
        file_id = %row.id,
        attempt = attempts,
        error = %message,
        "giving up on an upload"
    );

    // `failed`, not `rejected`. The file may be perfectly fine; what went wrong
    // was ours, and telling somebody their document was refused would send them
    // off to fix something that is not broken.
    if let Err(err) = files_db::mark_failed(pool, row.id, &message).await {
        tracing::error!(file_id = %row.id, error = %err, "could not record a failed upload");
    }
}

async fn verifier_loop(state: AppState, shutdown: CancellationToken) {
    let jobs = &state.config.storage.jobs;
    let interval = Duration::from_secs(jobs.poll_interval_secs);

    tracing::info!(
        every_secs = jobs.poll_interval_secs,
        concurrency = jobs.concurrency,
        "upload verifier started"
    );

    loop {
        if !wait(&shutdown, interval).await {
            tracing::info!("upload verifier stopping");
            return;
        }

        for (tenant, pool) in active_tenants(&state).await {
            let claimed = files_db::claim_batch(
                &pool,
                state.config.storage.jobs.concurrency,
                state.config.storage.jobs.claim_timeout_secs,
            )
            .await;

            match claimed {
                Ok(rows) => {
                    for row in rows {
                        // Sequential within a tenant. Verification is a read of
                        // the whole file plus a hash - disk-bound, not
                        // CPU-bound - so running the batch in parallel would
                        // contend for the same spindle rather than finish
                        // sooner. `concurrency` bounds the batch, not the
                        // number of things happening at once.
                        run(&state, &pool, &tenant, &row).await;
                    }
                }
                Err(err) => {
                    tracing::warn!(tenant = %tenant, error = %err, "could not claim uploads");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Publishing what happened
// ---------------------------------------------------------------------------

async fn relay_loop(state: AppState, shutdown: CancellationToken) {
    let Some(publisher) = state.publisher.clone() else {
        return;
    };

    tracing::info!(
        every_secs = RELAY_INTERVAL.as_secs(),
        "outbox relay started"
    );

    loop {
        if !wait(&shutdown, RELAY_INTERVAL).await {
            tracing::info!("outbox relay stopping");
            return;
        }

        for (tenant, pool) in active_tenants(&state).await {
            if let Err(err) = drain(&publisher, &tenant, &pool).await {
                tracing::warn!(tenant = %tenant, error = %err, "outbox relay pass failed");
            }
        }
    }
}

/// Publish one tenant's unpublished events.
///
/// Published first, marked second. The other order loses an event whenever the
/// publish fails, and this order at worst sends one twice - which is why every
/// message carries a `message_id` and why consumers are required to be
/// idempotent. See `phonix_db::outbox`.
async fn drain(
    publisher: &Publisher,
    tenant: &TenantSlug,
    pool: &PgPool,
) -> Result<(), phonix_db::DbError> {
    let events = outbox::claim_unpublished(pool, RELAY_BATCH).await?;

    for event in events {
        let message = phonix_messaging::OutgoingMessage {
            routing_suffix: event.routing_key.clone(),
            payload: serde_json::to_vec(&event.payload).unwrap_or_default(),
            content_type: "application/json",
            // The row's own id, not a fresh one. A republished event has to
            // look like the same event to a consumer deduplicating on it.
            event_id: event.event_id,
        };

        match publisher.publish(tenant, message).await {
            Ok(()) => {
                outbox::mark_published(pool, event.id).await?;

                tracing::debug!(
                    tenant = %tenant,
                    routing_key = %event.routing_key,
                    event_id = %event.event_id,
                    "event published"
                );
            }
            Err(err) => {
                // The row stays unpublished, which is the point: a broker that
                // is down means events queue up here and go out when it
                // returns, rather than being lost while it was away.
                outbox::record_failure(pool, event.id, &err.to_string()).await?;

                tracing::warn!(
                    tenant = %tenant,
                    event_id = %event.event_id,
                    error = %err,
                    "could not publish an event; it stays in the outbox"
                );

                // Stop this tenant's pass. The next event would fail the same
                // way, and hammering an unreachable broker sixty-four times is
                // not better than trying again in five seconds.
                break;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Sweeping up
// ---------------------------------------------------------------------------

async fn sweeper_loop(state: AppState, shutdown: CancellationToken) {
    let ttl = state.config.storage.quarantine_ttl_mins;

    tracing::info!(quarantine_ttl_mins = ttl, "quarantine sweeper started");

    loop {
        if !wait(&shutdown, SWEEP_INTERVAL).await {
            tracing::info!("quarantine sweeper stopping");
            return;
        }

        for (tenant, pool) in active_tenants(&state).await {
            let stale = match files_db::expired_quarantine(&pool, ttl, SWEEP_BATCH).await {
                Ok(rows) => rows,
                Err(err) => {
                    tracing::warn!(tenant = %tenant, error = %err, "could not read stale uploads");
                    continue;
                }
            };

            for row in stale {
                sweep(&state, &pool, &tenant, &row).await;
            }
        }
    }
}

/// Remove one abandoned upload's bytes, and the row that named them.
///
/// Reached only when a job never ran at all - a process killed between the
/// write and the row, or a deployment with the worker turned off. The bytes go
/// first, so a crash in between leaves an orphan object rather than a row
/// pointing at one that is gone.
async fn sweep(state: &AppState, pool: &PgPool, tenant: &TenantSlug, row: &files_db::FileRow) {
    let Some(raw) = row.quarantine_key.as_deref() else {
        return;
    };

    match phonix_storage::StorageKey::parse_for(tenant, raw) {
        Ok(key) => {
            if let Err(err) = state.storage.delete(&key).await {
                tracing::warn!(key = %key, error = %err, "could not sweep a stale upload");
                return;
            }
        }
        Err(err) => {
            // A row naming another tenant's area. Left strictly alone: the one
            // thing worse than a stale file is a sweeper that deletes across a
            // tenant boundary because a row told it to.
            tracing::error!(
                file_id = %row.id,
                error = %err,
                "a stale upload names an object this tenant may not touch; leaving it"
            );
            return;
        }
    }

    if let Err(err) = files_db::delete(pool, row.id).await {
        tracing::warn!(file_id = %row.id, error = %err, "could not remove a swept upload row");
        return;
    }

    tracing::info!(file_id = %row.id, tenant = %tenant, "swept an abandoned upload");
}

// ---------------------------------------------------------------------------
// Applying change-trail retention
// ---------------------------------------------------------------------------

/// Delete change-trail entries older than each workspace asked to keep.
///
/// Per tenant, because the retention is a per-workspace setting: one may keep
/// ninety days and its neighbour for ever, and both are right.
async fn prune_loop(state: AppState, shutdown: CancellationToken) {
    tracing::info!("change-trail retention started");

    loop {
        if !wait(&shutdown, PRUNE_INTERVAL).await {
            tracing::info!("change-trail retention stopping");
            return;
        }

        for (tenant, pool) in active_tenants(&state).await {
            prune_tenant(&tenant, &pool).await;
        }
    }
}

/// Apply one workspace's retention, in batches.
async fn prune_tenant(tenant: &TenantSlug, pool: &PgPool) {
    let settings = match settings_db::load(pool).await {
        Ok(settings) => settings,
        Err(err) => {
            // Skipped, not defaulted. A workspace whose settings cannot be read
            // is one whose retention is unknown, and guessing here deletes
            // history nobody agreed to lose.
            tracing::warn!(tenant = %tenant, error = %err, "could not read the audit policy");
            return;
        }
    };

    // `None` is the default and an ordinary answer: keep everything.
    let Some(days) = settings.audit.retention_days else {
        return;
    };

    let mut deleted = 0_u64;

    for _ in 0..PRUNE_MAX_BATCHES {
        match audit_db::prune(pool, days, PRUNE_BATCH).await {
            Ok(0) => break,
            Ok(batch) => {
                deleted += batch;

                // Short batch means the backlog is gone. Asking again would be
                // one more full scan for nothing.
                if batch < PRUNE_BATCH.unsigned_abs() {
                    break;
                }
            }
            Err(err) => {
                tracing::warn!(
                    tenant = %tenant,
                    error = %err,
                    "could not apply change-trail retention",
                );
                break;
            }
        }
    }

    if deleted > 0 {
        // Said out loud. Entries disappearing from an audit screen is alarming
        // unless there is a line somewhere saying it was policy.
        tracing::info!(
            tenant = %tenant,
            deleted,
            retention_days = days,
            "applied change-trail retention",
        );
    }
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Every active tenant, with a pool open on it.
///
/// A tenant whose pool cannot be opened is skipped with a warning rather than
/// ending the pass: one unreachable database must not stop the other ninety-nine
/// from having their work done.
///
/// The rows read here are carried into `resolve_record` rather than being
/// reduced back to slugs. Handing back the slug would have the registry read
/// each row a second time, so a pass would cost one query per tenant on top of
/// the single list - growing with the catalog, on a loop that runs whether or
/// not there is any work to do.
async fn active_tenants(state: &AppState) -> Vec<(TenantSlug, PgPool)> {
    let records = match state.catalog.list().await {
        Ok(records) => records,
        Err(err) => {
            tracing::warn!(error = %err, "could not read the tenant catalog");
            return Vec::new();
        }
    };

    let mut open = Vec::with_capacity(records.len());

    for record in records {
        // A suspended, archived or unlicensed workspace is not served, and its
        // background work is not run either: verifying an upload for a tenant
        // nobody may reach would be work done on behalf of an account that has
        // been switched off - or one that is no longer authorized to be here.
        if !record.serves_traffic() {
            continue;
        }

        // Kept back for the log line and the caller, because the record itself
        // is handed over.
        let slug = record.slug.clone();

        match state.tenants.resolve_record(record).await {
            Ok(handle) => open.push((slug, handle.pool)),
            Err(err) => {
                tracing::warn!(tenant = %slug, error = %err, "skipping a tenant this pass");
            }
        }
    }

    open
}

/// Sleep, unless we are being shut down.
///
/// Returns `false` when the loop should stop. `biased` so a pending
/// cancellation wins over an elapsed timer: at shutdown the point is to leave,
/// not to squeeze in one more pass.
async fn wait(shutdown: &CancellationToken, interval: Duration) -> bool {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => false,
        () = tokio::time::sleep(interval) => true,
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::files::UploadResult;

    use super::*;

    #[tokio::test]
    async fn waiting_stops_immediately_when_cancelled() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        // `biased` is what makes this deterministic: without it a timer that
        // has already elapsed could win the race and the loop would do one more
        // pass after being told to stop.
        assert!(!wait(&shutdown, Duration::from_secs(3600)).await);
    }

    #[tokio::test]
    async fn waiting_returns_true_when_the_interval_elapses() {
        let shutdown = CancellationToken::new();
        assert!(wait(&shutdown, Duration::from_millis(1)).await);
    }

    #[test]
    fn the_routing_key_a_consumer_would_bind_to_is_the_one_written() {
        // The relay republishes whatever the outbox row says, so this is the
        // string a queue binding has to match. Pinned here because changing it
        // is a change to a public contract, not an edit.
        assert_eq!(UploadResult::ROUTING_KEY, "file.upload.completed");
    }
}
