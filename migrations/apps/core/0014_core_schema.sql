-- ===========================================================================
-- The `core` schema, and the app registry.
--
-- Everything built so far is infrastructure: identity, authorization, audit,
-- files, settings, messaging. None of it holds an opinion about a business
-- process, so all of it belongs to `core` - the one schema every app is
-- allowed to reference, and the one no tenant may uninstall.
--
-- See docs/adr/0001-core-boundary.md for where the line is drawn and why.
--
-- Idempotent by construction, and it has to be: this migration exists to
-- relocate tables that a database provisioned *after* it will never have had
-- in `public` to begin with. The runner creates `core` before the first
-- migration runs and puts it first on the search path, so on a fresh database
-- 0001-0013 build everything in the right place and every move below finds
-- nothing to do.
--
-- `public` stays on the search path behind `core`, permanently: pgcrypto is
-- installed there, and `gen_random_uuid()` has to keep resolving.
-- ===========================================================================

CREATE SCHEMA IF NOT EXISTS core;

-- ---------------------------------------------------------------------------
-- Relocate the tables.
--
-- `ALTER TABLE ... SET SCHEMA` carries indexes, constraints and column-owned
-- sequences along with the table, and leaves the OID alone - so the three
-- `set_updated_at` triggers keep firing and prepared statements keep resolving
-- through the move.
--
-- `_sqlx_migrations` is in the list. Moving the migrator's own bookkeeping
-- table from under it mid-run is safe for the same reason: the INSERT that
-- records this migration is parsed after the ALTER has run, in the same
-- transaction, and resolves through the search path to its new home.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    -- Declaration order is the order of migrations 0001-0013.
    relocatable CONSTANT TEXT[] := ARRAY[
        'users',
        'sessions',
        'user_tokens',
        'user_mfa_factors',
        'password_history',
        'identity_events',
        'roles',
        'user_roles',
        'role_permissions',
        'user_permissions',
        'entity_events',
        'file_uploads',
        'workspace_settings',
        'organization_profile',
        'mail_settings',
        'outbox_events',
        'processed_events',
        '_sqlx_migrations'
    ];
    relation TEXT;
BEGIN
    FOREACH relation IN ARRAY relocatable LOOP
        IF EXISTS (
            SELECT 1 FROM pg_tables
             WHERE schemaname = 'public' AND tablename = relation
        ) THEN
            EXECUTE format('ALTER TABLE public.%I SET SCHEMA core', relation);
        END IF;
    END LOOP;
END $$;

-- The trigger function behind `users_set_updated_at` and its two siblings.
-- Triggers bind to it by OID, so moving it is invisible to them; what it buys
-- is that a future `CREATE TRIGGER ... EXECUTE FUNCTION set_updated_at()` in a
-- core migration resolves to core rather than leaving a dependency on public.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM pg_proc p
          JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'public' AND p.proname = 'set_updated_at'
    ) THEN
        ALTER FUNCTION public.set_updated_at() SET SCHEMA core;
    END IF;
END $$;

-- ---------------------------------------------------------------------------
-- The app registry.
--
-- One row per app whose schema exists in this database. `app_id` *is* the
-- schema name - deriving one from the other rather than storing both is what
-- stops the two disagreeing, and makes `DROP SCHEMA` on uninstall unambiguous.
--
-- `state` is installation, not entitlement. Whether this tenant has paid for
-- an app lives in the catalog database, because billing is cross-tenant and
-- has to be answerable without opening a tenant database. `read_only` here is
-- the *effect* of a lapsed subscription, written by the subscription service.
--
-- `core` is always present and always active. It is listed like any other app
-- so the migration runner has one uniform place to record what it applied.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS core.installed_apps (
    -- Stable, lowercase, and identical to the Postgres schema it owns.
    app_id          TEXT PRIMARY KEY,

    -- Highest migration version applied to this app's schema, zero-padded.
    -- NULL until the runner has finished a pass.
    schema_version  TEXT,

    state           TEXT NOT NULL DEFAULT 'installing',

    installed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    migrated_at     TIMESTAMPTZ,

    -- Mirrors the identifier rule in the runner: an app id reaches DDL as a
    -- schema name, so it may only ever be a bare Postgres identifier.
    CONSTRAINT installed_apps_app_id_format
        CHECK (app_id ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT installed_apps_app_id_length
        CHECK (char_length(app_id) BETWEEN 2 AND 63),
    CONSTRAINT installed_apps_state_valid
        CHECK (state IN ('installing', 'active', 'read_only', 'uninstalling'))
);

INSERT INTO core.installed_apps (app_id, state)
VALUES ('core', 'active')
ON CONFLICT (app_id) DO NOTHING;
