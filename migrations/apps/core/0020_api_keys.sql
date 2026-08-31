-- ---------------------------------------------------------------------------
-- 0020: API keys, and the switch that licenses the API at all.
--
-- See docs/adr/0002-public-api.md. Two things arrive together because neither
-- is any use without the other: a credential for `/api/v1`, and the per
-- workspace flag that decides whether `/api/v1` answers this workspace.
--
-- # A key is a session that belongs to a machine
--
-- The shape deliberately echoes `core.sessions`: an opaque 32-byte token whose
-- SHA-256 digest is what is stored, two ways to stop it (expiry and
-- revocation), both checked in the same statement that looks it up. A dump of
-- this table cannot be replayed, and an expired key cannot be resurrected by a
-- code path that forgot to check.
--
-- What differs is lifetime and intent. A session is minted by a browser and
-- lives for hours; a key is minted by a person, pasted into somebody else's
-- deployment, and lives until it is revoked.
--
-- # A key can never exceed its owner
--
-- `user_id` is the account the key acts as, and `scopes` narrows it. What the
-- key may actually do is the intersection of the two, resolved on every
-- request from the user's *current* grants - so removing a permission from the
-- user, suspending them or deleting them takes it from every key they issued,
-- with nothing to update here. That is why there is no permission column: a
-- copy of the grants would be a second answer to a question `core` already
-- answers, and it would be the stale one.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS core.api_keys (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- The account this key acts as. CASCADE because a key that outlived its
    -- owner would be a credential with no permissions to intersect against,
    -- which is not a safe thing to leave lying in a table.
    user_id        UUID NOT NULL REFERENCES core.users (id) ON DELETE CASCADE,

    -- What the person who issued it called it: 'iOS app', 'nightly export'.
    -- The only way an administrator tells two keys apart on the revoke screen.
    name           TEXT NOT NULL,

    -- SHA-256 of the token, never the token. The `phx_` prefix is stripped
    -- before hashing, so these bytes have the same shape as a session's.
    token_hash     BYTEA NOT NULL,

    -- The last four characters of the token, for the screen. Enough to answer
    -- "is this the key in the config file", useless to anybody who reads it.
    token_hint     TEXT NOT NULL,

    -- Permission names from the compiled tree, e.g.
    -- 'Pages.Administration.Settings'. Empty means the key can reach only what
    -- is ungated. Names rather than an API-specific scope vocabulary: two
    -- lists of the same thing eventually disagree.
    --
    -- Not foreign-keyed to anything: the tree is compiled into the binary, not
    -- stored, and a name that leaves the build simply stops matching - which is
    -- the safe direction, since a scope that means nothing grants nothing.
    scopes         TEXT[] NOT NULL DEFAULT '{}',

    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Who issued it. Normally the same person as `user_id`; not the same
    -- question, and an administrator issuing a key for a shared account is the
    -- case where they differ. Not an FK: history keeps its names.
    created_by     UUID,

    -- Optional. A key with no expiry is the common case for a mobile app and
    -- the wrong default for a contractor's script, so the choice is the
    -- issuer's rather than this table's.
    expires_at     TIMESTAMPTZ,

    -- Written best-effort and coarsely; see the service. It exists because
    -- "is anything still using this" is the question asked before every
    -- revocation, and a precise answer would cost a write per request.
    last_used_at   TIMESTAMPTZ,

    revoked_at     TIMESTAMPTZ,
    revoked_reason TEXT,
    revoked_by     UUID,

    CONSTRAINT api_keys_token_hash_len CHECK (octet_length(token_hash) = 32),
    CONSTRAINT api_keys_token_hint_len CHECK (char_length(token_hint) = 4),
    CONSTRAINT api_keys_name_length CHECK (char_length(name) BETWEEN 1 AND 80)
);

-- The lookup on every API request.
CREATE UNIQUE INDEX IF NOT EXISTS api_keys_token_hash_key
    ON core.api_keys (token_hash);

-- The administration screen: this user's keys, live ones first.
CREATE INDEX IF NOT EXISTS api_keys_user_live_idx
    ON core.api_keys (user_id)
    WHERE revoked_at IS NULL;

-- Drives the periodic purge of keys nobody can present any more.
CREATE INDEX IF NOT EXISTS api_keys_expiry_idx
    ON core.api_keys (expires_at)
    WHERE expires_at IS NOT NULL;

COMMENT ON TABLE core.api_keys IS
    'Bearer credentials for /api/v1. Digest only; effective permissions are the owner''s current grants intersected with `scopes`.';

-- ---------------------------------------------------------------------------
-- The licence switch.
--
-- Off by default, and deliberately not a permission. A permission says what a
-- person may do inside a workspace; this says whether the workspace has the
-- feature at all. Putting it in the permission tree would make it something an
-- administrator can grant themselves.
--
-- It lives beside the security policy rather than in configuration for the
-- reason `workspace_settings` exists at all: after onboarding, the row is the
-- authority, and one workspace's licence is not a property of the deployment.
-- ---------------------------------------------------------------------------
ALTER TABLE core.workspace_settings
    ADD COLUMN IF NOT EXISTS api_enabled BOOLEAN NOT NULL DEFAULT FALSE;

COMMENT ON COLUMN core.workspace_settings.api_enabled IS
    'Whether /api/v1 answers this workspace at all. A licence, not a grant.';
