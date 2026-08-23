//! Turning a stored security-trail row into something a screen may read.
//!
//! Two things are needed and only one of them is about identity. The diff -
//! recognising `{from, to}`, flattening it into dotted paths, comparing lists
//! as sets - is [`crate::audit::diff`], shared with the change trail. What is
//! left here is the mapping from an `identity_events` row to the
//! [`AuditEvent`] a browser is allowed to see.
//!
//! The two differ in one deliberate way: the stored row carries a free-form
//! `detail` object, and this carries a rendered summary of it. Shipping that
//! object verbatim would be shipping an unversioned internal structure to a
//! client that would then depend on it.

use phonix_core::identity::{AuditEvent, AuditEventDetail};
use phonix_db::identity::audit::AuditRecord;

use crate::audit::diff;

/// One stored row as a line of the list.
pub fn listing(record: AuditRecord) -> AuditEvent {
    AuditEvent {
        id: record.id,
        event: record.event,
        succeeded: record.succeeded,
        user_id: record.user_id,
        email: record.email,
        ip: record.ip,
        summary: diff::summarise(&record.detail),
        occurred_at: record.occurred_at,
    }
}

/// One stored row, opened.
pub fn described(record: AuditRecord) -> AuditEventDetail {
    let changes = diff::changes(&record.detail);
    let facts = diff::facts(&record.detail);
    let user_agent = record.user_agent.clone();

    AuditEventDetail {
        event: listing(record),
        user_agent,
        changes,
        facts,
    }
}
