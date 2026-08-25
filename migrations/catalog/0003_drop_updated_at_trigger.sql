-- The catalog's copy of the same trigger, removed for the same reason.
--
-- See `migrations/apps/core/0017_drop_updated_at_triggers.sql` for the full
-- argument. In short: a trigger is behaviour that does not appear at the call
-- site, so `Catalog::mark_active` and its siblings now set `updated_at`
-- themselves.
--
-- The catalog has exactly one such table.

DROP TRIGGER IF EXISTS tenants_set_updated_at ON tenants;
DROP FUNCTION IF EXISTS set_updated_at();
