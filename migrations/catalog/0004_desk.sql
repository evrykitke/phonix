-- ===========================================================================
-- Phonix Desk: the people who run the platform, and what they did.
--
-- See docs/adr/0005-phonix-desk.md. Three tables, all in the catalog and none
-- of them reachable from a tenant database:
--
--   desk_users     who may sign in to Desk
--   desk_sessions  a signed-in browser, by digest, never by token
--   desk_audit     what a desk user did, where a workspace cannot edit it
--
-- A desk user is NOT a `core.users` row. `Caller` is tenant-scoped and every
-- gate in phonix-services is written against it; a desk user has no workspace
-- and the catalog has no permissions. Section 4 of the ADR is why these are
-- separate tables rather than a synthetic tenant.
-- ===========================================================================

-- ---------------------------------------------------------------------------
-- desk_users
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS desk_users (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Lowercased at the call site. A citext column would need the extension in
    -- the catalog for one table, and the application already normalises every
    -- address it stores.
    email               TEXT NOT NULL,
    display_name        TEXT NOT NULL,

    -- Argon2id, same parameters as a workspace user - `[security.password]`.
    -- Null until the setup link below is used, which is the whole of the
    -- "nobody sets somebody else's password" rule as it applies here.
    password_hash       TEXT,

    -- TOTP shared secret, sealed by `crypto::vault` before it arrives, exactly
    -- like `core.user_mfa_factors.secret_encrypted`. A database dump must not
    -- be equivalent to holding everybody's authenticator app.
    --
    -- TOTP is not optional for Desk (ADR section 4), so a row with no
    -- `totp_confirmed_at` cannot complete a sign-in - it can only finish
    -- enrolling.
    totp_secret         BYTEA,
    totp_confirmed_at   TIMESTAMPTZ,

    -- pending  - created, has not yet used its setup link
    -- active   - has a password and a confirmed authenticator
    -- disabled - kept for the audit trail, cannot sign in
    status              TEXT NOT NULL DEFAULT 'pending',

    -- The single-use link that carries a new desk user from "created by
    -- somebody else" to "has a password only they know". SHA-256 of the token,
    -- never the token: same rule as every other credential in this codebase.
    --
    -- On the row rather than in a fourth table because a desk user has at most
    -- one outstanding setup link, and a table with a uniqueness constraint of
    -- one row per user is a column.
    setup_token_hash    BYTEA,
    setup_expires_at    TIMESTAMPTZ,

    -- Sign-in lockout. Counted here rather than in Redis because Desk must
    -- work when Redis does not, and because a lockout that forgets itself on
    -- restart is not one.
    failed_attempts     INTEGER NOT NULL DEFAULT 0,
    locked_until        TIMESTAMPTZ,

    last_signed_in_at   TIMESTAMPTZ,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at         TIMESTAMPTZ,

    CONSTRAINT desk_users_status_valid CHECK (
        status IN ('pending', 'active', 'disabled')
    ),
    CONSTRAINT desk_users_email_length CHECK (char_length(email) BETWEEN 3 AND 254),
    -- An active desk user has both halves. Enforced here and not only in Rust
    -- because this is the invariant the whole surface rests on: no password
    -- means no sign-in, and no confirmed authenticator means no second factor
    -- to demand.
    CONSTRAINT desk_users_active_is_complete CHECK (
        status <> 'active'
        OR (password_hash IS NOT NULL AND totp_secret IS NOT NULL AND totp_confirmed_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS desk_users_email_key ON desk_users (email);

-- Looking a setup link up by its digest. Partial, because the column is null
-- for every user who has finished setting up - which is all of them, normally.
CREATE UNIQUE INDEX IF NOT EXISTS desk_users_setup_token_key
    ON desk_users (setup_token_hash)
    WHERE setup_token_hash IS NOT NULL;

-- ---------------------------------------------------------------------------
-- desk_sessions
-- ---------------------------------------------------------------------------
--
-- Shaped like `core.sessions` and for the same reasons: the cookie holds the
-- token, this table holds its SHA-256 digest, and both deadlines live in the
-- WHERE clause so an expired session cannot be resurrected by a code path that
-- forgot to check.
--
-- `mfa_satisfied` is false between the password and the code. A session in
-- that state may reach the MFA screen and nothing else.
CREATE TABLE IF NOT EXISTS desk_sessions (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    desk_user_id        UUID NOT NULL REFERENCES desk_users (id) ON DELETE CASCADE,

    token_hash          BYTEA NOT NULL,

    mfa_satisfied       BOOLEAN NOT NULL DEFAULT false,
    mfa_attempts        INTEGER NOT NULL DEFAULT 0,

    ip                  TEXT,
    user_agent          TEXT,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Slides forward with activity.
    expires_at          TIMESTAMPTZ NOT NULL,
    -- Never moves. This is when a real sign-in becomes necessary.
    absolute_expires_at TIMESTAMPTZ NOT NULL,

    revoked_at          TIMESTAMPTZ,
    revoked_reason      TEXT,

    CONSTRAINT desk_sessions_user_agent_length CHECK (char_length(user_agent) <= 256)
);

CREATE UNIQUE INDEX IF NOT EXISTS desk_sessions_token_key ON desk_sessions (token_hash);
CREATE INDEX IF NOT EXISTS desk_sessions_user_idx ON desk_sessions (desk_user_id);
-- The sweeper's query, and only live rows are worth an index.
CREATE INDEX IF NOT EXISTS desk_sessions_expiry_idx
    ON desk_sessions (absolute_expires_at)
    WHERE revoked_at IS NULL;

-- ---------------------------------------------------------------------------
-- desk_audit
-- ---------------------------------------------------------------------------
--
-- Deliberately not `core.entity_events` in a tenant: "who suspended this
-- workspace" must not be a row that workspace's own administrators can read,
-- edit, or lose when the database is archived.
--
-- `before`/`after` follow the shape the entity trail already uses, because
-- that shape is what earns a diff on a detail page. An action with no
-- before-state - a migration sweep, a retry - leaves `before` null and puts
-- what happened in `after`.
CREATE TABLE IF NOT EXISTS desk_audit (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Nullable, and that is not laziness: a failed sign-in is worth recording
    -- even when the address matched nobody, and the whole point of this table
    -- is that it keeps rows the actor would rather it did not.
    desk_user_id        UUID REFERENCES desk_users (id) ON DELETE SET NULL,
    -- Kept as text as well as by id, so deleting an account does not erase who
    -- did the thing.
    actor_email         TEXT,

    action              TEXT NOT NULL,
    -- The workspace an action was about, by slug rather than by id: a slug is
    -- what a person recognises, and this row must still read correctly after
    -- the tenant row is gone.
    tenant_slug         TEXT,

    outcome             TEXT NOT NULL DEFAULT 'ok',
    detail              TEXT,

    before_state        JSONB,
    after_state         JSONB,

    ip                  TEXT,
    occurred_at         TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT desk_audit_outcome_valid CHECK (outcome IN ('ok', 'refused', 'failed'))
);

CREATE INDEX IF NOT EXISTS desk_audit_occurred_idx ON desk_audit (occurred_at DESC);
CREATE INDEX IF NOT EXISTS desk_audit_actor_idx ON desk_audit (desk_user_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS desk_audit_tenant_idx
    ON desk_audit (tenant_slug, occurred_at DESC)
    WHERE tenant_slug IS NOT NULL;
