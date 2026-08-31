-- ---------------------------------------------------------------------------
-- 0021: telling a phone's session apart from a browser's.
--
-- See docs/adr/0003-mobile-authentication.md. One column, and the argument for
-- it is mostly an argument about what is *not* being added.
--
-- # A mobile session is a session
--
-- The tempting shape for signing somebody in from a phone is a subsystem of
-- its own: an access token, a refresh token beside it, a rotation scheme, an
-- expiry policy, a revocation path. Every one of those already exists here,
-- once, and 0002 already argued the case against the self-contained signed
-- token that would let us skip the lookup:
--
--     what it buys is instant revocation - sign out everywhere, suspend an
--     account, respond to a lost laptop - which a JWT cannot do without
--     exactly this table anyway.
--
-- A phone is more likely to be lost than a laptop, not less. So the phone gets
-- the same row, the same two deadlines, the same `revoked_at`, the same
-- `mfa_satisfied`. The only thing that differs is the envelope the token
-- travels in - a bearer header rather than a cookie - and an envelope is not a
-- schema change.
--
-- # Then why a column at all
--
-- Two reasons, and the second outlives the first.
--
-- Lifetimes. `[security.session]` is tuned for a browser: 12 hours idle, 7
-- days absolute. An application people keep on their phone and are signed out
-- of every week is an application they stop opening, so a mobile session reads
-- its deadlines from `[security.session.mobile]` instead. Something has to say
-- which set a row was opened under, and has to keep saying it, because `touch`
-- slides the idle deadline on every request long after the sign-in is over.
--
-- The device list. "Chrome on Windows, this browser" and "the phone app, last
-- seen an hour ago" are different facts about who is holding your account, and
-- a screen that renders both as a user-agent string is one nobody reads. That
-- screen is not built yet; the data it needs starts being recorded now, because
-- a column added later is a column that is null for every session that mattered.
--
-- # Why TEXT and a CHECK rather than an enum type
--
-- Same reason as everywhere else in this schema: adding a value to a Postgres
-- enum is a migration that cannot run inside a transaction on every version we
-- support, and the set here is small, closed, and read by Rust that already has
-- to match on it exhaustively.
-- ---------------------------------------------------------------------------

ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'browser';

-- `ADD CONSTRAINT IF NOT EXISTS` does not exist, and this migration has to be
-- safe to re-run: the tenant sweep applies every stream to every database on
-- boot, and a half-applied schema is how one workspace ends up different from
-- the others.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sessions_kind_known'
           AND conrelid = 'sessions'::regclass
    ) THEN
        ALTER TABLE sessions
            ADD CONSTRAINT sessions_kind_known CHECK (kind IN ('browser', 'mobile'));
    END IF;
END
$$;

-- Every session that existed before this migration was opened by a browser,
-- which is what the default already says. Stated here so the intent survives
-- the day somebody wonders whether the backfill was forgotten.
COMMENT ON COLUMN sessions.kind IS
    'browser | mobile. Which set of deadlines this session lives by, and what a device list calls it.';
