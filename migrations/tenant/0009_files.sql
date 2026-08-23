-- ---------------------------------------------------------------------------
-- 0009: uploaded files.
--
-- One table, and it is both the job and the file.
--
-- An upload is not an insert; it is work. Bytes arrive, and then something has
-- to decide what they are, hash them, move them out of quarantine and one day
-- scan them - none of which may depend on the connection that carried them
-- staying open. So the row is created the moment the bytes land, in state
-- 'received', and a worker walks it to a terminal state afterwards.
--
--   received ──► verifying ──► stored      the file is real and in place
--                    │
--                    ├───────► rejected    the file is not acceptable
--                    └───────► failed      we could not finish the work
--
-- # Why one table and not two
--
-- A separate `upload_jobs` and `stored_files` would need the id handed from one
-- to the other, which means a link given out while the job is running points at
-- a row that will not be the file. Here the id is the same throughout: the row
-- that says "somebody is uploading this" becomes the row that says "this file
-- exists", and every reference made in between stays valid.
--
-- # Why this is a queue and RabbitMQ is not
--
-- The broker carries the *result* outward - see the outbox insert in
-- phonix_services::files - but the work itself is claimed here, with
-- FOR UPDATE SKIP LOCKED against the partial index below. Three reasons:
--
--   * The row has to be written anyway, so the queue is free.
--   * A claim and a state change are then one transaction. With a broker they
--     are two, and the gap between them is where duplicated and lost work live.
--   * Uploads still work with rabbitmq.enabled = false, which is a supported
--     configuration and would otherwise mean files that never leave quarantine.
--
-- # What is deliberately not a foreign key
--
-- `storage_key` names an object in a filesystem or a bucket, which no database
-- constraint can vouch for. What the constraints below do enforce is that a row
-- claiming to be 'stored' has a key, a type and a digest - so "stored" can
-- never mean "we lost track of it".
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS file_uploads (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    status                TEXT        NOT NULL DEFAULT 'received',

    -- Which policy applies: 'avatars', 'attachments', 'imports'. Declared in
    -- code (phonix_core::files::bucket), not constrained here - adding a bucket
    -- would otherwise be a migration, and the value is written only by code
    -- that has already looked it up.
    bucket                TEXT        NOT NULL,

    -- The name the caller's file had, already sanitised. Kept so a list can
    -- show it and a download can offer it back; never used to build a path.
    original_name         TEXT        NOT NULL,
    -- The name it is actually stored under, which resembles the original in no
    -- way at all. NULL until it has been stored.
    stored_name           TEXT,

    -- What the browser claimed. Recorded so a rejection can quote it, and used
    -- for nothing else - see the module docs in phonix_core::files::catalog.
    declared_content_type TEXT,
    -- What the bytes turned out to be. This is the one that decides anything,
    -- and it is what a download is served as.
    content_type          TEXT,
    category              TEXT,

    byte_size             BIGINT      NOT NULL,
    -- Lowercase hex SHA-256, computed while the bytes were being written.
    checksum_sha256       TEXT,

    -- Where the object lives now, and where it lived before it was verified.
    -- Exactly one of them is set at any point in a healthy row's life.
    storage_key           TEXT,
    quarantine_key        TEXT,

    -- Terminal state 'rejected' only. The code is the stable identifier; the
    -- detail is the serialised phonix_core::files::Rejection, so a screen can
    -- render the same sentence the uploader saw at the time.
    rejection             TEXT,
    rejection_detail      JSONB,

    -- The job's own bookkeeping.
    attempts              INT         NOT NULL DEFAULT 0,
    claimed_at            TIMESTAMPTZ,
    last_error            TEXT,

    uploaded_by           UUID        REFERENCES users(id) ON DELETE SET NULL,

    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    verified_at           TIMESTAMPTZ,

    CONSTRAINT file_uploads_status_valid CHECK (
        status IN ('received', 'verifying', 'stored', 'rejected', 'failed')
    ),
    CONSTRAINT file_uploads_size_sane CHECK (byte_size >= 0),
    CONSTRAINT file_uploads_bucket_not_empty CHECK (char_length(bucket) > 0),
    CONSTRAINT file_uploads_name_not_empty CHECK (char_length(original_name) > 0),

    -- The floor under "stored". A row in this state is one the download route
    -- will serve, so it has to have somewhere to read from, something to send
    -- as a content type, and a digest to prove the bytes with.
    CONSTRAINT file_uploads_stored_is_complete CHECK (
        status <> 'stored'
        OR (
            storage_key     IS NOT NULL
            AND stored_name IS NOT NULL
            AND content_type IS NOT NULL
            AND checksum_sha256 IS NOT NULL
        )
    ),

    -- A refusal that does not say why is not a refusal, it is a mystery.
    CONSTRAINT file_uploads_rejected_has_a_reason CHECK (
        status <> 'rejected' OR rejection IS NOT NULL
    ),

    -- A digest is 64 hex characters or it is not a SHA-256.
    CONSTRAINT file_uploads_checksum_shape CHECK (
        checksum_sha256 IS NULL OR checksum_sha256 ~ '^[0-9a-f]{64}$'
    )
);

-- The queue. Partial, so it holds only the work outstanding however much
-- history accumulates: a workspace with a million stored files has an index
-- here with as many rows as it has uploads in flight, which is normally none.
CREATE INDEX IF NOT EXISTS file_uploads_pending_idx
    ON file_uploads (created_at)
    WHERE status IN ('received', 'verifying');

-- What the files list is sorted by.
CREATE INDEX IF NOT EXISTS file_uploads_bucket_created_idx
    ON file_uploads (bucket, created_at DESC);

-- Finding an identical file that is already here. Partial on 'stored' because
-- a duplicate of something still being checked is not yet a duplicate of
-- anything.
CREATE INDEX IF NOT EXISTS file_uploads_checksum_idx
    ON file_uploads (checksum_sha256)
    WHERE status = 'stored' AND checksum_sha256 IS NOT NULL;

-- Two rows must never claim the same object. Without this a bug in naming
-- would show up as one file quietly overwriting another, which is the worst
-- possible way to find out about it.
CREATE UNIQUE INDEX IF NOT EXISTS file_uploads_storage_key_key
    ON file_uploads (storage_key)
    WHERE storage_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS file_uploads_uploaded_by_idx
    ON file_uploads (uploaded_by)
    WHERE uploaded_by IS NOT NULL;

DROP TRIGGER IF EXISTS file_uploads_set_updated_at ON file_uploads;
CREATE TRIGGER file_uploads_set_updated_at
    BEFORE UPDATE ON file_uploads
    FOR EACH ROW
    EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- A profile picture is a stored file, not a URL.
--
-- `avatar_url` has been on this table since 0002 and holds an address - a
-- Gravatar, an identity provider's picture, something this application does not
-- host. It stays, because that is still a thing an account can have.
--
-- This is the other case: a picture somebody uploaded here, which is a row in
-- the table above and therefore has a size, a type decided from its bytes, and
-- a verification state. ON DELETE SET NULL rather than CASCADE - removing a
-- picture must not remove the person.
-- ---------------------------------------------------------------------------

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS avatar_file_id UUID
    REFERENCES file_uploads(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS users_avatar_file_idx
    ON users (avatar_file_id)
    WHERE avatar_file_id IS NOT NULL;
