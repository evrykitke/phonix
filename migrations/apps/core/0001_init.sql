-- ===========================================================================
-- Tenant database template (phonix_tenant_<slug>)
--
-- Applied to EVERY tenant database. Because each tenant is physically
-- separate, there is no tenant_id column anywhere below - isolation is the
-- database boundary itself.
--
-- Every migration here must be safe to run against an existing tenant.
-- ===========================================================================

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- Tenant-local copy of identity, so joins stay inside one database.
CREATE TABLE IF NOT EXISTS users (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email          TEXT NOT NULL,
    display_name   TEXT NOT NULL,

    -- Argon2id PHC string. Null for SSO-only accounts.
    password_hash  TEXT,

    role           TEXT NOT NULL DEFAULT 'member',
    is_active      BOOLEAN NOT NULL DEFAULT TRUE,
    last_login_at  TIMESTAMPTZ,

    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT users_role_valid CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    CONSTRAINT users_email_shape CHECK (position('@' IN email) > 1)
);

-- Case-insensitive uniqueness: 'Ada@x.com' and 'ada@x.com' are one account.
CREATE UNIQUE INDEX IF NOT EXISTS users_email_key ON users (lower(email));

-- Outbox: events are written in the SAME transaction as the state change that
-- produced them, then relayed to RabbitMQ. This is what makes "state changed
-- but event lost" impossible after a broker outage.
CREATE TABLE IF NOT EXISTS outbox_events (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id        UUID NOT NULL DEFAULT gen_random_uuid(),

    -- Becomes the AMQP routing key suffix: tenant.<slug>.<routing_key>
    routing_key     TEXT NOT NULL,
    payload         JSONB NOT NULL,
    headers         JSONB NOT NULL DEFAULT '{}'::jsonb,

    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at    TIMESTAMPTZ,
    attempts        INT NOT NULL DEFAULT 0,
    last_error      TEXT,

    CONSTRAINT outbox_routing_key_not_empty CHECK (char_length(routing_key) > 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS outbox_events_event_id_key ON outbox_events (event_id);

-- The relay polls this: unpublished rows, oldest first. Partial index keeps it
-- small no matter how much history accumulates.
CREATE INDEX IF NOT EXISTS outbox_events_unpublished_idx
    ON outbox_events (occurred_at)
    WHERE published_at IS NULL;

-- Consumer-side idempotency. RabbitMQ guarantees at-least-once delivery, so a
-- handler must be able to recognise a message it has already processed.
CREATE TABLE IF NOT EXISTS processed_events (
    event_id      UUID PRIMARY KEY,
    consumer      TEXT NOT NULL,
    processed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS processed_events_processed_at_idx
    ON processed_events (processed_at);

CREATE OR REPLACE FUNCTION set_updated_at() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS users_set_updated_at ON users;
CREATE TRIGGER users_set_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();
