-- ===========================================================================
-- Identity module (per tenant database)
--
-- Applied to EVERY tenant database, so every statement must be safe against a
-- tenant that already has users and safe to re-run.
--
-- Scope: accounts, credentials, sessions, second factors, one-time tokens and
-- the security audit trail. MFA and email delivery are not implemented yet -
-- their tables are created now because retrofitting columns onto a live
-- identity schema across N tenant databases is the expensive version of this
-- change, and doing it up front costs nothing.
--
-- Note the absence of a tenant_id column anywhere below. Isolation here is the
-- database boundary, so there is no filter to forget.
-- ===========================================================================

-- ---------------------------------------------------------------------------
-- users: extend the placeholder from 0001 into a real account record
-- ---------------------------------------------------------------------------

-- Names, kept apart from display_name so the app can address someone by their
-- first name without guessing where to split.
ALTER TABLE users ADD COLUMN IF NOT EXISTS first_name TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN IF NOT EXISTS last_name  TEXT NOT NULL DEFAULT '';

-- Lifecycle. Replaces the boolean is_active: "not active" needs to distinguish
-- an invitation that was never accepted from an account an admin switched off
-- from someone who has left, and a boolean cannot.
ALTER TABLE users ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'users' AND column_name = 'is_active'
    ) THEN
        UPDATE users SET status = CASE WHEN is_active THEN 'active' ELSE 'suspended' END;
        ALTER TABLE users DROP COLUMN is_active;
    END IF;
END;
$$;

-- Credentials. The hash is a full PHC string ($argon2id$v=19$m=..,t=..,p=..$salt$hash),
-- so the parameters travel with it and a future cost increase can re-hash on
-- next sign-in without a schema change or a flag day.
ALTER TABLE users ADD COLUMN IF NOT EXISTS password_algorithm    TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS password_updated_at   TIMESTAMPTZ;
-- Set when an admin resets a password, or after a forced rotation.
ALTER TABLE users ADD COLUMN IF NOT EXISTS must_change_password  BOOLEAN NOT NULL DEFAULT FALSE;

-- Email verification. Null means unverified; SMTP lands later.
ALTER TABLE users ADD COLUMN IF NOT EXISTS email_verified_at     TIMESTAMPTZ;

-- Multi-factor authentication. `mfa_enabled` is a denormalised mirror of
-- "has at least one confirmed factor", kept on the row because it is read on
-- every single sign-in and the join is not worth it.
ALTER TABLE users ADD COLUMN IF NOT EXISTS mfa_enabled           BOOLEAN NOT NULL DEFAULT FALSE;
-- Set when policy requires this user to hold a second factor, whether or not
-- they have enrolled one yet.
ALTER TABLE users ADD COLUMN IF NOT EXISTS mfa_required          BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE users ADD COLUMN IF NOT EXISTS mfa_enrolled_at       TIMESTAMPTZ;

-- Online brute-force defence. Counted per account and reset on success.
ALTER TABLE users ADD COLUMN IF NOT EXISTS failed_login_count    INT NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN IF NOT EXISTS last_failed_login_at  TIMESTAMPTZ;
ALTER TABLE users ADD COLUMN IF NOT EXISTS locked_until          TIMESTAMPTZ;

-- Sign-in trail on the row itself, for "last seen" in the UI. The full history
-- is in identity_events.
-- Addresses are stored as TEXT, not INET: the value that matters is whatever
-- the proxy forwarded, which is not always a bare address, and INET would need
-- an extra sqlx type feature to gain nothing here.
ALTER TABLE users ADD COLUMN IF NOT EXISTS last_login_ip         TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS last_seen_at          TIMESTAMPTZ;

-- Presentation preferences.
ALTER TABLE users ADD COLUMN IF NOT EXISTS avatar_url            TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS locale                TEXT NOT NULL DEFAULT 'en';
ALTER TABLE users ADD COLUMN IF NOT EXISTS timezone              TEXT NOT NULL DEFAULT 'UTC';

-- Provenance and soft deletion. Rows are never hard-deleted: outbox events,
-- audit entries and business records reference them.
ALTER TABLE users ADD COLUMN IF NOT EXISTS invited_by            UUID REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE users ADD COLUMN IF NOT EXISTS invited_at            TIMESTAMPTZ;
ALTER TABLE users ADD COLUMN IF NOT EXISTS deleted_at            TIMESTAMPTZ;

DO $$
BEGIN
    ALTER TABLE users ADD CONSTRAINT users_status_valid
        CHECK (status IN ('pending', 'active', 'suspended', 'deactivated'));
EXCEPTION
    WHEN duplicate_object THEN NULL;
END;
$$;

DO $$
BEGIN
    -- An account that can sign in must have something to sign in with. The
    -- alternative - an 'active' row with a NULL hash - is one careless
    -- `password_hash IS NULL` check away from a bypass.
    --
    -- 0001 anticipated NULL hashes for SSO-only accounts. When federated
    -- identity lands it brings its own table, and this constraint gets widened
    -- to "has a password OR has a federated identity" in that migration.
    ALTER TABLE users ADD CONSTRAINT users_password_or_pending
        CHECK (password_hash IS NOT NULL OR status IN ('pending', 'deactivated'));
EXCEPTION
    WHEN duplicate_object THEN NULL;
END;
$$;

-- Who created the workspace. Not a role: roles are data an administrator can
-- edit, and "the person who must never be locked out" cannot be. The owner
-- keeps the Admin role like anyone else - this flag is what stops that role
-- from being taken away from them.
ALTER TABLE users ADD COLUMN IF NOT EXISTS is_owner BOOLEAN NOT NULL DEFAULT FALSE;

-- Exactly one owner per workspace. Enforced in the schema rather than in
-- application code because "who can be locked out" is not a rule worth
-- trusting to a code path.
CREATE UNIQUE INDEX IF NOT EXISTS users_single_owner_idx
    ON users ((is_owner))
    WHERE is_owner AND deleted_at IS NULL;

-- The `role` column from 0001 is superseded by the roles / user_roles tables
-- in 0003. A single text column cannot express "this person is an Admin and an
-- Auditor", nor a role an organization invented for itself.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'users' AND column_name = 'role'
    ) THEN
        -- Carried forward so 0003 can convert it into user_roles rows.
        ALTER TABLE users RENAME COLUMN role TO legacy_role;
        ALTER TABLE users ALTER COLUMN legacy_role DROP NOT NULL;
        ALTER TABLE users ALTER COLUMN legacy_role DROP DEFAULT;
        ALTER TABLE users DROP CONSTRAINT IF EXISTS users_role_valid;
    END IF;
END;
$$;

-- The sign-in lookup. Partial on deleted_at so a soft-deleted account frees
-- its address for reuse while the row stays for referential integrity.
DROP INDEX IF EXISTS users_email_key;
CREATE UNIQUE INDEX IF NOT EXISTS users_active_email_key
    ON users (lower(email))
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS users_status_idx ON users (status) WHERE deleted_at IS NULL;

-- ---------------------------------------------------------------------------
-- sessions
-- ---------------------------------------------------------------------------

-- Server-side sessions rather than a self-contained signed token. The cost is
-- one indexed lookup per request; what it buys is instant revocation - sign out
-- everywhere, suspend an account, respond to a stolen laptop - which a JWT
-- cannot do without exactly this table anyway.
CREATE TABLE IF NOT EXISTS sessions (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,

    -- SHA-256 of the opaque cookie value, never the value itself. A dump of
    -- this table therefore cannot be replayed as a set of live sessions.
    -- Plain SHA-256 rather than Argon2: the input is 32 bytes of CSPRNG
    -- output, so there is no low-entropy space to grind, and this is read on
    -- every request.
    token_hash          BYTEA NOT NULL,

    -- Whether this session cleared the second factor. A session may exist
    -- before MFA is satisfied so the challenge page has something to attach to.
    mfa_satisfied       BOOLEAN NOT NULL DEFAULT FALSE,

    ip                  TEXT,
    user_agent          TEXT,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Two independent deadlines. `expires_at` slides forward with activity;
    -- `absolute_expires_at` never moves, so a session that is kept warm by a
    -- background tab still dies on schedule.
    expires_at          TIMESTAMPTZ NOT NULL,
    absolute_expires_at TIMESTAMPTZ NOT NULL,

    revoked_at          TIMESTAMPTZ,
    revoked_reason      TEXT,

    CONSTRAINT sessions_token_hash_len CHECK (octet_length(token_hash) = 32),
    CONSTRAINT sessions_deadlines_ordered CHECK (absolute_expires_at >= expires_at)
);

CREATE UNIQUE INDEX IF NOT EXISTS sessions_token_hash_key ON sessions (token_hash);

-- "Sign out everywhere", and the sign-in check that revokes other sessions.
CREATE INDEX IF NOT EXISTS sessions_user_active_idx
    ON sessions (user_id)
    WHERE revoked_at IS NULL;

-- Drives the periodic purge.
CREATE INDEX IF NOT EXISTS sessions_expiry_idx ON sessions (absolute_expires_at);

-- ---------------------------------------------------------------------------
-- user_mfa_factors
-- ---------------------------------------------------------------------------

-- Schema only for now; enrolment and challenge logic come with the MFA work.
-- One table for every factor kind rather than one per kind: the columns differ
-- but the lifecycle - enrol, confirm, use, revoke - does not.
CREATE TABLE IF NOT EXISTS user_mfa_factors (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,

    kind           TEXT NOT NULL,
    -- What the user called it: "iPhone", "YubiKey (blue)".
    label          TEXT NOT NULL,

    -- TOTP shared secret, encrypted with an application key before it gets
    -- here. Storing it in clear would make a database dump equivalent to
    -- having every user's authenticator app.
    secret_encrypted   BYTEA,
    -- WebAuthn.
    credential_id      BYTEA,
    public_key         BYTEA,
    sign_count         BIGINT NOT NULL DEFAULT 0,
    transports         TEXT[],

    -- Null until the user proves they can produce a code from it. An
    -- unconfirmed factor must never satisfy a challenge.
    confirmed_at   TIMESTAMPTZ,
    last_used_at   TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT user_mfa_factors_kind_valid
        CHECK (kind IN ('totp', 'webauthn', 'recovery_code')),
    -- Each kind must actually carry the material it needs.
    CONSTRAINT user_mfa_factors_material_present CHECK (
        (kind = 'totp'          AND secret_encrypted IS NOT NULL)
     OR (kind = 'webauthn'      AND credential_id IS NOT NULL AND public_key IS NOT NULL)
     OR (kind = 'recovery_code' AND secret_encrypted IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS user_mfa_factors_user_idx ON user_mfa_factors (user_id);

-- A WebAuthn credential id is unique across the whole relying party.
CREATE UNIQUE INDEX IF NOT EXISTS user_mfa_factors_credential_key
    ON user_mfa_factors (credential_id)
    WHERE credential_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- user_tokens
-- ---------------------------------------------------------------------------

-- One table for every single-use secret handed to a user: the columns are
-- identical and so is the danger, so they get one implementation of "issue,
-- expire, consume exactly once" rather than four.
CREATE TABLE IF NOT EXISTS user_tokens (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,

    purpose      TEXT NOT NULL,
    -- SHA-256 again, for the same reason as sessions.token_hash.
    token_hash   BYTEA NOT NULL,

    expires_at   TIMESTAMPTZ NOT NULL,
    -- Set the moment the token is redeemed. The row is kept so a second
    -- attempt is answered "already used" rather than "no such token", and so
    -- replay attempts are visible.
    consumed_at  TIMESTAMPTZ,

    created_ip   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT user_tokens_purpose_valid CHECK (
        purpose IN (
            'email_verification',
            'password_reset',
            'invitation',
            -- Trades a just-created account on the signup host for a session
            -- cookie on the workspace's own host. Lives for seconds.
            'session_handoff'
        )
    ),
    CONSTRAINT user_tokens_hash_len CHECK (octet_length(token_hash) = 32)
);

CREATE UNIQUE INDEX IF NOT EXISTS user_tokens_hash_key ON user_tokens (token_hash);

-- Finds the outstanding token when a new one is requested, so issuing a second
-- password reset can invalidate the first.
CREATE INDEX IF NOT EXISTS user_tokens_user_purpose_idx
    ON user_tokens (user_id, purpose)
    WHERE consumed_at IS NULL;

CREATE INDEX IF NOT EXISTS user_tokens_expiry_idx ON user_tokens (expires_at);

-- ---------------------------------------------------------------------------
-- identity_events
-- ---------------------------------------------------------------------------

-- The security audit trail: append-only, and the one place where the reason a
-- sign-in failed is written down. The login form itself must stay vague to
-- avoid confirming which addresses have accounts; this is where the truth goes.
CREATE TABLE IF NOT EXISTS identity_events (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    -- Nullable, and SET NULL on delete: a failed sign-in for an address that
    -- does not exist has no user to point at, and that attempt is exactly the
    -- kind of thing worth keeping.
    user_id      UUID REFERENCES users (id) ON DELETE SET NULL,
    -- Recorded alongside user_id so the trail survives the account.
    email        TEXT,

    event        TEXT NOT NULL,
    succeeded    BOOLEAN NOT NULL,
    -- Free-form context: the failure reason, the factor kind, the old role.
    detail       JSONB NOT NULL DEFAULT '{}'::jsonb,

    ip           TEXT,
    user_agent   TEXT,
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT identity_events_event_valid CHECK (
        event IN (
            'signup',
            'login',
            'logout',
            'password_change',
            'password_reset_requested',
            'password_reset_completed',
            'email_verification_sent',
            'email_verified',
            'mfa_enrolled',
            'mfa_challenge',
            'mfa_removed',
            'account_locked',
            'account_unlocked',
            'role_changed',
            'session_revoked',
            'invitation_sent',
            'invitation_accepted'
        )
    )
);

CREATE INDEX IF NOT EXISTS identity_events_user_idx ON identity_events (user_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS identity_events_recent_idx ON identity_events (occurred_at DESC);

-- Finds a burst of failures from one address across many accounts, which is
-- what credential stuffing looks like from the inside.
CREATE INDEX IF NOT EXISTS identity_events_failures_idx
    ON identity_events (ip, occurred_at DESC)
    WHERE succeeded = FALSE;

-- ---------------------------------------------------------------------------
-- triggers
-- ---------------------------------------------------------------------------

-- set_updated_at() is defined in 0001; users already carries the trigger.
-- Nothing added here needs it: sessions track last_seen_at explicitly, and the
-- other three tables are append-only or single-transition.
