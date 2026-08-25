-- ---------------------------------------------------------------------------
-- Which apps this workspace has switched on.
--
-- `state` already tracked the *schema*: installing, active, read_only. That is
-- the migration runner's business and it says nothing about subscription - by
-- the time this file runs, every app compiled into the build has had its
-- schema created and its stream applied in every tenant database, because a
-- migration under a live request is not something to arrange for later.
--
-- Enablement is the other question, and it is the one a customer answers: this
-- workspace bought Books and did not buy the CRM. It is deliberately a
-- timestamp rather than a boolean, because "when" is the fact a subscription
-- and a changelog both need, and a boolean would have to grow one anyway.
--
-- `app_version` is the app's own version at the moment it was switched on, not
-- its schema version. It is what a "what's new since you installed this" list
-- is filed against.
-- ---------------------------------------------------------------------------

ALTER TABLE core.installed_apps
    ADD COLUMN IF NOT EXISTS enabled_at  TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS enabled_by  UUID,
    ADD COLUMN IF NOT EXISTS app_version TEXT;

-- Not a foreign key to core.users, for the same reason `number_sequences`
-- holds none: deleting the administrator who installed an app must not be
-- blocked by, or quietly rewrite, the record of who installed it. It is
-- history, and history keeps its names.
COMMENT ON COLUMN core.installed_apps.enabled_by IS
    'The user who switched this app on. Not an FK: history keeps its names.';

-- `core` is on in every workspace that exists at all, and there is no moment
-- at which anybody chose it - so it is enabled as of whenever it was
-- installed, by nobody.
UPDATE core.installed_apps
   SET enabled_at = installed_at
 WHERE app_id = 'core'
   AND enabled_at IS NULL;

-- Every other app starts switched **off**, including in workspaces that
-- already have its schema. That is not a downgrade: nothing was ever offered,
-- so nothing is being taken away, and the alternative - handing every existing
-- workspace every app this build happens to contain - is exactly the
-- subscription bug this table exists to prevent.
--
-- Their data is untouched and comes straight back the moment somebody installs
-- the app, because installing has never meant creating a schema.
