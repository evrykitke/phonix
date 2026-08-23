-- ===========================================================================
-- Catalog database (phonix_catalog)
--
-- The single shared database. It holds the tenant registry that maps a
-- subdomain to a physical tenant database. It must contain NO tenant business
-- data - that lives in the per-tenant databases.
-- ===========================================================================

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS tenants (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Subdomain label. Mirrors the validation in phonix_core::TenantSlug:
    -- lowercase, 2-40 chars, [a-z0-9-], no leading digit or hyphen, no "--".
    slug              TEXT NOT NULL,

    display_name      TEXT NOT NULL,

    -- Physical database name, e.g. 'phonix_tenant_acme'. Stored rather than
    -- derived so a tenant can be relocated without a code change.
    database_name     TEXT NOT NULL,

    status            TEXT NOT NULL DEFAULT 'provisioning',

    -- Schema version last applied to this tenant's database, so the app knows
    -- whether to run migrations on first access.
    schema_version    TEXT,
    migrated_at       TIMESTAMPTZ,

    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT tenants_slug_format CHECK (slug ~ '^[a-z][a-z0-9]*(-[a-z0-9]+)*$'),
    CONSTRAINT tenants_slug_length CHECK (char_length(slug) BETWEEN 2 AND 40),
    CONSTRAINT tenants_status_valid CHECK (
        status IN ('active', 'provisioning', 'suspended', 'archived')
    ),
    CONSTRAINT tenants_database_name_format CHECK (database_name ~ '^[a-z_][a-z0-9_]*$'),
    CONSTRAINT tenants_database_name_length CHECK (char_length(database_name) <= 63)
);

-- Slug is the routing key looked up on (nearly) every request.
CREATE UNIQUE INDEX IF NOT EXISTS tenants_slug_key ON tenants (slug);
CREATE UNIQUE INDEX IF NOT EXISTS tenants_database_name_key ON tenants (database_name);

-- The hot path filters on status; only active tenants serve traffic.
CREATE INDEX IF NOT EXISTS tenants_status_idx ON tenants (status) WHERE status = 'active';

-- Keep updated_at honest without relying on every caller to set it.
CREATE OR REPLACE FUNCTION set_updated_at() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS tenants_set_updated_at ON tenants;
CREATE TRIGGER tenants_set_updated_at
    BEFORE UPDATE ON tenants
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();
