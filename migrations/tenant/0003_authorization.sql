-- ===========================================================================
-- Authorization: roles and permissions (per tenant database)
--
-- Modelled on ABP / ASP.NET Zero.
--
--   * Permission DEFINITIONS live in code (phonix_core::authorization), as a
--     dotted tree: "Pages.Administration.Users.Create". They are not stored -
--     a permission the code does not check is not a permission.
--   * Permission GRANTS live here, keyed by that name. Roles are rows, so an
--     organization can define its own alongside the two static ones.
--
-- Resolving what a user may do:
--
--     union of their roles' grants
--       + individual grants   (user_permissions, is_granted = true)
--       - individual denials  (user_permissions, is_granted = false)
--
-- A denial beats any role grant, so one person can be excluded from something
-- their role allows without inventing a near-duplicate role.
-- ===========================================================================

-- ---------------------------------------------------------------------------
-- roles
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS roles (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- The stable key used in code and in user_roles. Compared
    -- case-insensitively, so "admin" and "Admin" cannot coexist.
    name          TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    description   TEXT,

    -- Ships with the product. Cannot be renamed or deleted: removing Admin
    -- would leave a workspace nobody can administer, and renaming it would
    -- break the code that assigns it at signup.
    is_static     BOOLEAN NOT NULL DEFAULT FALSE,
    -- Assigned automatically to every new user in this workspace.
    is_default    BOOLEAN NOT NULL DEFAULT FALSE,

    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT roles_name_not_empty CHECK (char_length(trim(name)) > 0),
    CONSTRAINT roles_name_length CHECK (char_length(name) <= 64)
);

CREATE UNIQUE INDEX IF NOT EXISTS roles_name_key ON roles (lower(name));

DROP TRIGGER IF EXISTS roles_set_updated_at ON roles;
CREATE TRIGGER roles_set_updated_at
    BEFORE UPDATE ON roles
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();

-- The two roles every workspace gets. Their permission grants are written by
-- the application from the compiled definitions - see
-- `phonix_db::authorization::sync_static_roles` - so the permission tree has
-- exactly one source of truth instead of being restated in SQL and drifting.
INSERT INTO roles (name, display_name, description, is_static, is_default)
VALUES
    ('Admin', 'Admin', 'Full access to everything in this workspace.', TRUE, FALSE),
    ('User',  'User',  'The default role for everyone in this workspace.', TRUE, TRUE)
ON CONFLICT DO NOTHING;

-- ---------------------------------------------------------------------------
-- user_roles
-- ---------------------------------------------------------------------------

-- Many-to-many: someone can be an Admin and an Auditor at once, and their
-- effective permissions are the union.
CREATE TABLE IF NOT EXISTS user_roles (
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role_id     UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    granted_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    granted_by  UUID REFERENCES users (id) ON DELETE SET NULL,

    PRIMARY KEY (user_id, role_id)
);

-- "Who holds this role?", and the check that refuses to delete a role still in
-- use. The (user_id, ...) direction is already served by the primary key.
CREATE INDEX IF NOT EXISTS user_roles_role_idx ON user_roles (role_id);

-- Carry forward whatever the single `role` column from 0001 held, so an
-- existing workspace does not wake up with nobody in any role.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'users' AND column_name = 'legacy_role'
    ) THEN
        INSERT INTO user_roles (user_id, role_id)
        SELECT u.id, r.id
          FROM users u
          JOIN roles r
            -- owner and admin both became Admin; everyone else became User.
            ON lower(r.name) = CASE
                 WHEN u.legacy_role IN ('owner', 'admin') THEN 'admin'
                 ELSE 'user'
               END
         WHERE u.legacy_role IS NOT NULL
        ON CONFLICT DO NOTHING;

        -- The first account created in a workspace is its owner.
        UPDATE users SET is_owner = TRUE
         WHERE legacy_role = 'owner'
           AND NOT EXISTS (SELECT 1 FROM users WHERE is_owner);

        ALTER TABLE users DROP COLUMN legacy_role;
    END IF;
END;
$$;

-- ---------------------------------------------------------------------------
-- role_permissions
-- ---------------------------------------------------------------------------

-- One row per granted permission. Absence means "not granted": there is no
-- is_granted flag here, because a role is a positive bundle and a stored "no"
-- would be indistinguishable from a permission that was never added.
CREATE TABLE IF NOT EXISTS role_permissions (
    role_id     UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    -- Matches PermissionDefinition::name exactly. Not a foreign key, because
    -- the definitions live in code; grants for names this build does not
    -- define are pruned on load rather than rejected at write time, so a
    -- rollback to an older binary does not destroy an administrator's work.
    name        TEXT NOT NULL,
    granted_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (role_id, name),

    CONSTRAINT role_permissions_name_shape
        CHECK (name ~ '^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*$' AND char_length(name) <= 128)
);

-- ---------------------------------------------------------------------------
-- user_permissions
-- ---------------------------------------------------------------------------

-- Per-user overrides on top of their roles. Unlike role_permissions this DOES
-- carry a flag, because both answers are meaningful: `true` adds something the
-- roles do not give, `false` takes away something they do.
CREATE TABLE IF NOT EXISTS user_permissions (
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    is_granted  BOOLEAN NOT NULL,
    set_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    set_by      UUID REFERENCES users (id) ON DELETE SET NULL,

    PRIMARY KEY (user_id, name),

    CONSTRAINT user_permissions_name_shape
        CHECK (name ~ '^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*$' AND char_length(name) <= 128)
);

-- The permission resolver reads every override for one user in a single scan;
-- the primary key already covers that. This one answers the reverse question -
-- "who has been given this individually?" - which the audit screens ask.
CREATE INDEX IF NOT EXISTS user_permissions_name_idx ON user_permissions (name);
