-- ===========================================================================
-- 0011: the change trail (`entity_events`).
--
-- `identity_events` is the *security* trail: who signed in, who was locked
-- out, who spent a recovery code. Record edits were folded into it one event
-- at a time - `user_updated`, `role_created`, `organization_profile_changed` -
-- and each addition cost a migration that restated a CHECK constraint across
-- every tenant database (see 0005, 0006, 0008). That is the wrong shape twice
-- over:
--
--   * it cannot answer "what has ever happened to *this* record", because
--     nothing in the row says which record it was;
--   * it makes auditing a new entity a schema change, so the cheapest thing to
--     do when adding one is not to audit it.
--
-- This table is the other half. One row per change to one record, keyed by
-- what it was and which one, so a detail screen can show that record's own
-- history and the trail as a whole is still one list.
--
-- # What goes where, from here on
--
--   identity_events   something *happened* - a sign-in, a lockout, a challenge
--   entity_events     something *changed* - a record was created, edited, gone
--
-- Existing rows are left exactly where they are. A trail whose past is rewritten
-- by a deployment is not a trail, and the security screen still renders the old
-- CRUD rows perfectly well; they simply stop being added to.
-- ===========================================================================

CREATE TABLE IF NOT EXISTS entity_events (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    -- Which kind of record, e.g. 'organization_profile', 'role', 'user'.
    --
    -- Deliberately NOT check-constrained. The vocabulary is declared in
    -- `phonix_core::audit::EntityKind`, which is the crate that also knows what
    -- to call each kind on screen and where its detail page lives - and a
    -- constraint here would mean that auditing a new entity costs a migration
    -- applied to every tenant database, which is the cost this table exists to
    -- remove. An unknown value read by an older build renders as itself rather
    -- than as nothing, which is the failure mode worth having.
    entity_type  TEXT NOT NULL,

    -- Which record, as text rather than as a UUID.
    --
    -- Most entities are keyed by one, but not all: the organization profile is
    -- a single row with nothing to point at, and an entity keyed by a pair
    -- would not fit either. A UUID column would force those to invent one.
    entity_id    TEXT NOT NULL,

    -- Closed set, and constrained, unlike entity_type: these are the three
    -- things that can happen to a record. A fourth verb is a change to what
    -- this table means, not an addition to a list, and it should not be
    -- possible to write one by accident.
    action       TEXT NOT NULL,

    -- What the record was called when this happened.
    --
    -- Stored rather than joined, because the point of a trail is to survive the
    -- record: after a role is deleted there is nothing left to join to, and
    -- "Role 8f2c... deleted" answers nobody's question.
    label        TEXT,

    -- Who did it. Both, for the same reason identity_events keeps both: the
    -- row must still name somebody after the account is gone.
    actor_id     UUID REFERENCES users (id) ON DELETE SET NULL,
    actor_email  TEXT,

    -- `{"from": {...}, "to": {...}}` - the shape `phonix_services::audit::diff`
    -- turns into a diff. A creation has no `from` and a deletion has no `to`,
    -- and both keys are still written, holding JSON null, so that every row
    -- diffs by the same rule: a row carrying only one side is read as a fact
    -- rather than as a change, which would lose the detail page for every
    -- creation.
    detail       JSONB NOT NULL DEFAULT '{}'::jsonb,

    ip           TEXT,
    user_agent   TEXT,
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT entity_events_action_valid
        CHECK (action IN ('created', 'updated', 'deleted')),
    -- An empty string is not an identifier, and a row carrying one is a row no
    -- history section will ever find.
    CONSTRAINT entity_events_identified
        CHECK (length(entity_type) > 0 AND length(entity_id) > 0)
);

-- The history section on a record's own page: this thing, newest first.
CREATE INDEX IF NOT EXISTS entity_events_record_idx
    ON entity_events (entity_type, entity_id, occurred_at DESC);

-- The trail as a whole, which is how the admin screen opens.
CREATE INDEX IF NOT EXISTS entity_events_recent_idx
    ON entity_events (occurred_at DESC);

-- "Everything this person changed", which is the question asked about an
-- account after it turns out to have been compromised.
CREATE INDEX IF NOT EXISTS entity_events_actor_idx
    ON entity_events (actor_id, occurred_at DESC);

-- "Every role that was deleted", without scanning the trail for one kind.
CREATE INDEX IF NOT EXISTS entity_events_kind_idx
    ON entity_events (entity_type, occurred_at DESC);
