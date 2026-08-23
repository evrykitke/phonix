-- What this workspace records about itself, and for how long.
--
-- The change trail is the one table nothing ever deletes from, and for a
-- workspace of ten thousand accounts it can outgrow the records it describes.
-- So it becomes a decision: which kinds are recorded, and whether entries are
-- eventually dropped.
--
-- It lives on `workspace_settings` rather than in a table of its own because it
-- is three fields, saved by the same form and read by the same person as the
-- password and MFA policies. A singleton table per setting is a join per
-- screen.
--
-- Why exclusions and not inclusions
-- ---------------------------------
-- `audit_excluded_kinds` names the kinds that are *off*. Recording the ones
-- that are on would mean that a kind added by a later release arrives switched
-- off in every existing workspace, silently, with nothing on screen to say so -
-- and a trail that quietly stops covering new things is worse than one that was
-- never switched on. See `phonix_core::audit::policy`.
--
-- The column is deliberately not constrained against a list of kind names, for
-- the same reason `entity_events.entity_type` is not: the vocabulary is
-- declared in code, and a CHECK here could only repeat it one migration behind.
-- An unknown name is kept rather than dropped, so that rolling back to a build
-- which has never heard of a kind does not silently switch it back on when the
-- workspace rolls forward again.
--
-- Defaults
-- --------
-- Recording on, retention unset. A trail somebody has to go and switch on is
-- one that was off on the day it was needed, and a default that deletes is a
-- default that loses evidence nobody agreed to lose. Both are also what every
-- existing workspace gets when this migration runs, which is the behaviour they
-- already had.

ALTER TABLE workspace_settings
    ADD COLUMN IF NOT EXISTS audit_changes_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE workspace_settings
    ADD COLUMN IF NOT EXISTS audit_excluded_kinds TEXT[] NOT NULL DEFAULT '{}';

-- NULL means "keep them forever", which is the default and an ordinary answer -
-- not a missing value. The prune job skips the workspace entirely when it is
-- null rather than treating it as zero.
ALTER TABLE workspace_settings
    ADD COLUMN IF NOT EXISTS audit_retention_days INT;

ALTER TABLE workspace_settings
    DROP CONSTRAINT IF EXISTS workspace_settings_audit_retention;

-- The floor is a week: anything shorter makes the trail useless for the thing a
-- trail is for, which is somebody noticing on Monday what went wrong on Friday.
-- The ceiling is ten years, past which "keep it for a while" has become
-- "forever" - which NULL already says, and says without a job walking the table
-- every night. Kept in step with `phonix_core::audit::policy`.
ALTER TABLE workspace_settings
    ADD CONSTRAINT workspace_settings_audit_retention CHECK (
        audit_retention_days IS NULL
        OR (audit_retention_days BETWEEN 7 AND 3650)
    );

ALTER TABLE workspace_settings
    DROP CONSTRAINT IF EXISTS workspace_settings_audit_kinds_named;

-- Not a list of which kinds are valid - that is code's to know. Only that an
-- entry is a name at all: a blank string would exclude nothing while looking
-- like it excluded something.
ALTER TABLE workspace_settings
    ADD CONSTRAINT workspace_settings_audit_kinds_named CHECK (
        NOT ('' = ANY (audit_excluded_kinds))
    );
