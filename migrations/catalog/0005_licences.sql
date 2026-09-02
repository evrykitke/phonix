-- ===========================================================================
-- Licences: whether a workspace is authorized to be here, and until when.
--
-- See docs/adr/0005-phonix-desk.md section 7. One question, answered in the
-- catalog and nowhere else - a licence a tenant's own administrators can reach
-- is not a licence.
--
-- This is NOT a plan, an edition, a seat count or an entitlement. Those want
-- their own record and their own table beside this one; the effective answer
-- for any future feature is the catalog's entitlement AND the tenant's own
-- switch, the narrower of two things, neither able to widen the other.
-- ===========================================================================

CREATE TABLE IF NOT EXISTS tenant_licences (
    -- One row per workspace, and the primary key says so. A workspace has one
    -- current licence; the history of what it has had is `desk_audit`, which
    -- lives where the workspace cannot edit it. A second table of superseded
    -- rows would be a second answer to "is this workspace authorized" and the
    -- two would eventually disagree.
    tenant_id       UUID PRIMARY KEY REFERENCES tenants (id) ON DELETE CASCADE,

    -- trial    - time-limited, issued by signup and by Desk
    -- licensed - paid, internal, or a demonstration
    -- revoked  - withdrawn by a person
    --
    -- There is deliberately no 'expired'. Expiry is a standing computed from
    -- the dates, never a stored state: if a date passing overwrote this column
    -- there would be no way left to tell a lapse from a withdrawal, and those
    -- are the two events the whole design exists to keep apart. A lapse is
    -- answered by extending; a withdrawal is answered by a conversation.
    state           TEXT NOT NULL,

    valid_from      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Null means no end: an internal workspace, a demonstration tenant. A
    -- licence with no end is a deliberate act by a named desk user, and the
    -- audit row saying so is the point.
    --
    -- Half-open, like every other interval in this codebase: this instant is
    -- the first one NOT covered.
    valid_until     TIMESTAMPTZ,

    -- The human reason. Free text, never parsed.
    note            TEXT,

    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The desk user's address, or 'migration' for the backfill below. Text
    -- rather than a foreign key into desk_users so that deleting an account
    -- does not erase who authorized a workspace.
    updated_by      TEXT,

    CONSTRAINT tenant_licences_state_valid CHECK (state IN ('trial', 'licensed', 'revoked')),
    CONSTRAINT tenant_licences_period_ordered CHECK (
        valid_until IS NULL OR valid_until > valid_from
    ),
    CONSTRAINT tenant_licences_note_length CHECK (char_length(note) <= 500)
);

-- Answering "what is about to run out" without a sequential scan. Partial,
-- because a licence with no end date is never the answer to that question.
CREATE INDEX IF NOT EXISTS tenant_licences_expiry_idx
    ON tenant_licences (valid_until)
    WHERE valid_until IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Backfill.
--
-- `TenantStatus::serves_traffic()` now reads "active AND currently licensed",
-- so without this the first deploy after this migration stops serving every
-- existing workspace at once.
--
-- Issued with no end date, and the note is honest about where it came from:
-- nobody licensed these workspaces, they predate the idea. A desk user
-- reviewing the list should be able to see which rows are a decision and which
-- are an inheritance.
-- ---------------------------------------------------------------------------
INSERT INTO tenant_licences (tenant_id, state, valid_from, valid_until, note, updated_by)
SELECT id,
       'licensed',
       created_at,
       NULL,
       'Issued by catalog migration 0005. This workspace predates licensing and '
           || 'nobody authorized it - review it.',
       'migration'
  FROM tenants
ON CONFLICT (tenant_id) DO NOTHING;
