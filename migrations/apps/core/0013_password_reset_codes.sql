-- Counting the guesses at a one-time code.
--
-- `user_tokens` was built for secrets with 256 bits of entropy in them: a
-- session handoff, an invitation link. Nothing guesses those, so nothing needed
-- counting, and the table has no notion of a failed attempt.
--
-- A password-reset code is the opposite kind of secret. It is six digits,
-- because it is read off a phone and typed into a different window, and six
-- digits is a million guesses - a number a script gets through in minutes
-- against an endpoint that will answer indefinitely. Entropy is not what makes
-- it safe. What makes it safe is that the answering stops.
--
-- So the attempt count lives on the row rather than in memory or in Redis. It
-- has to survive a restart and be the same number for every process that might
-- serve the next guess, and it has to be read and written in the same
-- transaction that decides whether the guess was right - otherwise two
-- simultaneous guesses each see "4 attempts used" and the limit is a
-- suggestion. See `phonix_db::identity::one_time_token::redeem_code`.
--
-- Why on the shared table and not a table of its own
-- --------------------------------------------------
-- The column is dead weight for the other three purposes, and that is cheaper
-- than the alternative. A separate `password_reset_codes` table would be a
-- second implementation of "issue, expire, consume exactly once" - the exact
-- duplication the one-table design was chosen to avoid - and the third step is
-- the one that is easy to get wrong twice.
--
-- Existing rows get 0, which is what they have always effectively had.

ALTER TABLE user_tokens
    ADD COLUMN IF NOT EXISTS attempts SMALLINT NOT NULL DEFAULT 0;

-- Guards against a bug writing a negative count and turning the ceiling into a
-- floor. There is no upper bound here on purpose: the maximum is configuration,
-- an operator is allowed to change it, and a CHECK against a number in a TOML
-- file could only ever be that number one deploy out of date.
ALTER TABLE user_tokens
    DROP CONSTRAINT IF EXISTS user_tokens_attempts_sane;

ALTER TABLE user_tokens
    ADD CONSTRAINT user_tokens_attempts_sane CHECK (attempts >= 0);
