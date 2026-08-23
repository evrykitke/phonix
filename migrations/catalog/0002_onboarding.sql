-- ===========================================================================
-- Onboarding metadata on the tenant registry.
--
-- Still no tenant business data here: an owner's email is registry-level
-- routing and support information, not the user record. The actual account -
-- with its password hash, MFA factors and sessions - lives in the tenant's own
-- database and never leaves it.
-- ===========================================================================

-- Who created the workspace. Answers "which workspaces does this address own?"
-- without opening every tenant database, which is what a future "find my
-- workspace" email needs.
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS owner_email TEXT;

-- Set when self-service onboarding completes. Distinct from created_at, which
-- is also set for tenants created by an administrator or a seeding script.
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS onboarded_at TIMESTAMPTZ;

-- How the tenant came into existence, for support and for metrics.
ALTER TABLE tenants ADD COLUMN IF NOT EXISTS created_via TEXT NOT NULL DEFAULT 'unknown';

DO $$
BEGIN
    ALTER TABLE tenants ADD CONSTRAINT tenants_created_via_valid
        CHECK (created_via IN ('signup', 'admin', 'auto_provision', 'seed', 'unknown'));
EXCEPTION
    WHEN duplicate_object THEN NULL;
END;
$$;

-- Non-unique on purpose. One person may own several workspaces, and two
-- unrelated workspaces may legitimately share an owner address.
CREATE INDEX IF NOT EXISTS tenants_owner_email_idx
    ON tenants (lower(owner_email))
    WHERE owner_email IS NOT NULL;
