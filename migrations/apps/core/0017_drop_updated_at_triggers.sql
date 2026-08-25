-- Take the last of the application logic out of the database.
--
-- Five tables carried a `BEFORE UPDATE` trigger that set `updated_at = now()`,
-- and one plpgsql function behind them. They are removed here, and every
-- statement that updates those rows now sets the column itself.
--
-- The reason is not that triggers are slow. It is that a trigger is behaviour
-- that does not appear at the call site: a repository function reads as though
-- it writes four columns and in fact writes five, and the only way to know is
-- to go and read the schema. Behaviour that is invisible where it happens is
-- behaviour that gets forgotten -- and this one had already gone wrong. The
-- trigger fired on *every* write, so `users.updated_at` moved on each page
-- view (`last_seen_at`) and on each mistyped password (`failed_login_count`),
-- which made "when was this user last changed?" unanswerable. Worse, on
-- `number_sequences` it moved `updated_at` on every document allocated while
-- leaving `updated_by` pointing at whoever last edited the settings -- a
-- timestamp and an author that disagree, which is not a stale record but a
-- false one.
--
-- Setting the column at the call site makes that a decision instead of an
-- accident. The rule the repositories now follow:
--
--   `updated_at` follows an edit to the row's own data. It does not follow the
--   login trail (`last_seen_at`, `last_login_at`, `failed_login_count`,
--   `locked_until`) and it does not follow issuing a number.
--
-- Nothing outside `phonix-db` read any of these columns, so no caller changes.
--
-- What stays in the database is the part that is *not* logic: `NOT NULL`,
-- `CHECK`, `REFERENCES`, `UNIQUE`, and `DEFAULT now()` on insert. Those are
-- constraints on what the data may be, enforced where the data lives, and they
-- refuse a write rather than performing one. A trigger performs one.

DROP TRIGGER IF EXISTS users_set_updated_at ON core.users;
DROP TRIGGER IF EXISTS roles_set_updated_at ON core.roles;
DROP TRIGGER IF EXISTS file_uploads_set_updated_at ON core.file_uploads;
DROP TRIGGER IF EXISTS currencies_set_updated_at ON core.currencies;
DROP TRIGGER IF EXISTS number_sequences_set_updated_at ON core.number_sequences;

-- 0014 moved the function into `core` along with everything else. The `public`
-- drop covers a database that somehow never took that path; both are
-- `IF EXISTS`, so the pair is safe in either order and on a fresh build.
DROP FUNCTION IF EXISTS core.set_updated_at();
DROP FUNCTION IF EXISTS public.set_updated_at();
