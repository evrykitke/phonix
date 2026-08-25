-- ---------------------------------------------------------------------------
-- 0007: this workspace's own mail relay, if it has one.
--
-- Mail is resolved as "this row if it is configured, otherwise the system
-- default from [smtp] in the configuration file". Two levels, because a single
-- global relay cannot serve the workspace that must send from its own domain,
-- and a per-tenant-only arrangement would force every workspace that does not
-- care to configure a relay before it could invite anybody.
--
-- # The password is stored sealed, not hashed
--
-- The server has to *reproduce* this secret to authenticate to the relay, the
-- same way it has to reproduce a TOTP secret to check a code - so hashing is
-- not available. It is sealed with XChaCha20-Poly1305 under the key in
-- [security.mfa].encryption_key, which lives in the environment rather than in
-- the database, so a stolen dump is not by itself a working relay credential.
--
-- BYTEA and not TEXT: the sealed value is a nonce followed by ciphertext, which
-- is bytes. Storing it base64 in a TEXT column would be an encoding nobody
-- needs and one more place to get it wrong.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS mail_settings (
    -- Exactly one row, for ever - the same singleton shape as
    -- workspace_settings, and for the same reason: a second row would be a
    -- second relay that half the queries would read.
    id              BOOLEAN PRIMARY KEY DEFAULT TRUE,

    -- The whole override is off unless this is true. A row that exists with
    -- enabled = FALSE is a workspace that configured a relay and then turned it
    -- off, which must fall back to the system default rather than send nothing.
    enabled         BOOLEAN     NOT NULL DEFAULT FALSE,

    host            TEXT        NOT NULL DEFAULT '',
    port            INT         NOT NULL DEFAULT 587,
    username        TEXT        NOT NULL DEFAULT '',

    -- Sealed. NULL means "no password has been set", which is different from
    -- an empty one: some relays authenticate on the username alone.
    password_sealed BYTEA,

    from_address    TEXT        NOT NULL DEFAULT '',
    from_name       TEXT        NOT NULL DEFAULT '',
    reply_to        TEXT,

    -- Mirrors phonix_config::SmtpEncryption. Constrained rather than free text
    -- so a typo is refused here instead of decoded into a fallback later.
    encryption      TEXT        NOT NULL DEFAULT 'start_tls',

    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by      UUID        REFERENCES users(id) ON DELETE SET NULL,

    CONSTRAINT mail_settings_singleton CHECK (id),
    CONSTRAINT mail_settings_port CHECK (port > 0 AND port <= 65535),
    CONSTRAINT mail_settings_encryption CHECK (
        encryption IN ('start_tls', 'implicit', 'none')
    ),
    -- An enabled override has to be able to send. The application says the same
    -- thing with a message naming the field; this is the floor under it, so a
    -- row written by anything other than the settings screen cannot be a relay
    -- that fails on its first message.
    CONSTRAINT mail_settings_usable CHECK (
        NOT enabled OR (length(trim(host)) > 0 AND position('@' in from_address) > 1)
    )
);

INSERT INTO mail_settings (id) VALUES (TRUE) ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Changing where a workspace's mail goes is an audited act.
--
-- It is the one setting on that screen that can silently redirect every
-- invitation and every reset link to a relay somebody else controls, which
-- makes "who changed it, and to what" exactly the question an audit asks.
--
-- `invitation_sent` and `invitation_accepted` are already valid events from
-- 0002 and need nothing here; they simply have a writer now.
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
        'mfa_policy_changed',
        'user_permissions_changed',
        'role_permissions_changed',
        'user_updated',
        'mail_settings_changed'
    )
);
