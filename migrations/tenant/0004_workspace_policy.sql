-- ---------------------------------------------------------------------------
-- 0004 - what this organization has decided for itself
--
-- Password policy and MFA enforcement move out of the code and into a row the
-- workspace's own administrators own. The defaults below are the compiled
-- system defaults; a new workspace is seeded from `[security.workspace_defaults]`
-- in the configuration, and after that this row is the only authority.
--
-- Columns rather than one JSONB blob: every setting here has a CHECK that says
-- what a valid value is, and a policy that can be saved in a state nobody could
-- satisfy is a policy that locks a workspace out of itself.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS workspace_settings (
    -- Exactly one row, for ever. The primary key can only hold TRUE, so a
    -- second INSERT collides instead of quietly creating a second policy that
    -- half the queries would read.
    id                          BOOLEAN PRIMARY KEY DEFAULT TRUE,

    -- --- password policy ---------------------------------------------------

    password_min_length         INT     NOT NULL DEFAULT 12,
    password_max_length         INT     NOT NULL DEFAULT 256,
    password_require_lowercase  BOOLEAN NOT NULL DEFAULT FALSE,
    password_require_uppercase  BOOLEAN NOT NULL DEFAULT FALSE,
    password_require_digit      BOOLEAN NOT NULL DEFAULT FALSE,
    password_require_symbol     BOOLEAN NOT NULL DEFAULT FALSE,
    password_forbid_common      BOOLEAN NOT NULL DEFAULT TRUE,
    password_forbid_personal    BOOLEAN NOT NULL DEFAULT TRUE,
    -- NULL means passwords never expire, which is the default and the
    -- recommendation: NIST withdrew routine expiry because it produces
    -- Summer2024! followed by Autumn2024!.
    password_expiry_days        INT,
    -- 0 disables the reuse check. Each remembered password costs one Argon2
    -- verification on every change, which is why the ceiling is low.
    password_history_depth      SMALLINT NOT NULL DEFAULT 0,

    -- --- multi-factor authentication ---------------------------------------

    mfa_enforcement             TEXT    NOT NULL DEFAULT 'optional',
    mfa_allow_totp              BOOLEAN NOT NULL DEFAULT TRUE,
    mfa_allow_recovery_codes    BOOLEAN NOT NULL DEFAULT TRUE,
    -- Days a user may keep signing in without a factor after enforcement is
    -- switched to 'required'. Without it, flipping the switch locks out
    -- everybody who is not at their desk with their phone.
    mfa_grace_period_days       INT     NOT NULL DEFAULT 7,
    mfa_remember_device_days    INT     NOT NULL DEFAULT 0,

    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by                  UUID REFERENCES users (id) ON DELETE SET NULL,

    CONSTRAINT workspace_settings_singleton CHECK (id),

    -- The absolute floor, restated here because the database is the last line:
    -- an organization may tighten the policy and may loosen it only this far.
    CONSTRAINT workspace_settings_password_length CHECK (
        password_min_length >= 8
        AND password_max_length <= 256
        AND password_min_length <= password_max_length
    ),
    -- Zero would expire every password the instant it was saved, including the
    -- administrator's own.
    CONSTRAINT workspace_settings_password_expiry CHECK (
        password_expiry_days IS NULL
        OR (password_expiry_days BETWEEN 1 AND 3650)
    ),
    CONSTRAINT workspace_settings_password_history CHECK (
        password_history_depth BETWEEN 0 AND 24
    ),
    CONSTRAINT workspace_settings_mfa_enforcement CHECK (
        mfa_enforcement IN ('disabled', 'optional', 'required')
    ),
    -- Requiring a factor while permitting no method to hold one locks every
    -- user out on their next sign-in.
    CONSTRAINT workspace_settings_mfa_satisfiable CHECK (
        mfa_enforcement <> 'required' OR mfa_allow_totp
    ),
    CONSTRAINT workspace_settings_mfa_windows CHECK (
        mfa_grace_period_days BETWEEN 0 AND 90
        AND mfa_remember_device_days BETWEEN 0 AND 90
    )
);

-- The row exists from the first migration onwards, so every read finds a policy
-- and no code path has to invent one.
INSERT INTO workspace_settings (id) VALUES (TRUE) ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- password_history
--
-- Only for workspaces that set `password_history_depth > 0`. Rows are hashes,
-- never passwords, and are pruned to the configured depth on every change - an
-- unbounded history is a growing pile of hashes of passwords the user may still
-- be using elsewhere.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS password_history (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    password_hash   TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS password_history_user_idx
    ON password_history (user_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- users: when the password was last set
--
-- 0002 added `password_updated_at`, which is NULL for accounts created before
-- it existed. Expiry needs a date it can compare, so accounts with no recorded
-- change are treated as having changed it when they were created.
-- ---------------------------------------------------------------------------

UPDATE users SET password_updated_at = created_at WHERE password_updated_at IS NULL;

-- ---------------------------------------------------------------------------
-- sessions: the MFA challenge
--
-- A session exists before the second factor is satisfied, so the challenge
-- screen has something to attach to. These two columns bound how long that
-- half-authenticated state may last and how many wrong codes it survives.
-- ---------------------------------------------------------------------------

ALTER TABLE sessions ADD COLUMN IF NOT EXISTS mfa_attempts INT NOT NULL DEFAULT 0;

-- When the challenge stops being answerable. NULL for a session that never
-- needed one. Deliberately separate from `expires_at`: a challenge lives for
-- minutes while a session lives for hours, and reusing the session deadline
-- would leave a proven password waiting at a code box all day.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS mfa_challenge_expires_at TIMESTAMPTZ;

-- ---------------------------------------------------------------------------
-- user_mfa_factors: rotation and one-shot recovery codes
-- ---------------------------------------------------------------------------

-- Which key encrypted `secret_encrypted`. The ciphertext itself carries a
-- version byte; this column exists so a rotation can find the rows still on the
-- old key without decrypting every one of them.
ALTER TABLE user_mfa_factors ADD COLUMN IF NOT EXISTS key_version SMALLINT NOT NULL DEFAULT 1;

-- Recovery codes are deleted when spent rather than flagged, so a used code
-- leaves nothing to compare against. This records that a set was issued, for
-- the "you have N codes left, generated on D" line on the security screen.
ALTER TABLE user_mfa_factors ADD COLUMN IF NOT EXISTS batch_id UUID;

CREATE INDEX IF NOT EXISTS user_mfa_factors_confirmed_idx
    ON user_mfa_factors (user_id, kind)
    WHERE confirmed_at IS NOT NULL;

-- A user gets one authenticator app at a time. Enrolling a second without
-- removing the first is how people end up with a secret in an app they threw
-- away and no idea which entry is live.
CREATE UNIQUE INDEX IF NOT EXISTS user_mfa_factors_one_totp_idx
    ON user_mfa_factors (user_id)
    WHERE kind = 'totp' AND confirmed_at IS NOT NULL;

-- ---------------------------------------------------------------------------
-- identity_events: the events this migration makes possible
--
-- Recovery-code use is not `mfa_challenge`: it is the escape hatch, and a
-- workspace should be able to see how often it is being taken without reading
-- the detail column of every challenge. Policy changes are recorded because
-- "who relaxed the password rules, and when" is exactly the question an audit
-- asks after the fact.
-- ---------------------------------------------------------------------------

ALTER TABLE identity_events DROP CONSTRAINT IF EXISTS identity_events_event_valid;

ALTER TABLE identity_events ADD CONSTRAINT identity_events_event_valid CHECK (
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
        'mfa_recovery_used',
        'mfa_recovery_generated',
        'account_locked',
        'account_unlocked',
        'role_changed',
        'session_revoked',
        'invitation_sent',
        'invitation_accepted',
        'password_policy_changed',
        'mfa_policy_changed'
    )
);
