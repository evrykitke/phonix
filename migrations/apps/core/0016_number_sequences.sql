-- ---------------------------------------------------------------------------
-- 0016: gap-free document numbers.
--
-- Italy, Spain, Portugal, Poland, India under GST and most of Latin America
-- require an unbroken sequence on a tax document. A missing number is an audit
-- finding, and "the save failed" is not a defence.
--
-- # Why this is a table and not a SEQUENCE
--
-- `nextval()` is deliberately non-transactional: a rollback burns the number.
-- That is exactly right for a surrogate key and unlawful for an invoice. A row
-- with a counter, updated inside the document's own transaction, takes a row
-- lock held until commit - so a rollback *returns* the number and concurrent
-- allocators queue behind it rather than skipping past.
--
-- Gap-free is bought with serialization on one row. That is the trade, and for
-- an invoice it is the correct one.
--
-- # core does not know what a doc_type is
--
-- It knows how to allocate a formatted counter. What an `invoice` is belongs to
-- whichever app posts one - see the boundary in docs/adr/0001-core-boundary.md.
-- Each app declares its document types and default patterns; installing the app
-- inserts the rows; the tenant edits them on a numbering settings screen that
-- lives in core, because the screen is about counters rather than invoices.
--
-- So this table ships **empty**. There are no apps yet, and a seeded row for a
-- document type nothing issues is a row that drifts.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS core.number_sequences (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Which app owns the document type. Same vocabulary as
    -- `core.installed_apps.app_id`, deliberately *not* a foreign key to it:
    -- uninstalling an app must not cascade away the numbering history of
    -- documents it already issued, and those numbers still have to be
    -- explicable afterwards.
    app_id        TEXT NOT NULL,

    -- 'invoice', 'credit_note', 'purchase_order'. Opaque here.
    doc_type      TEXT NOT NULL,

    -- Per-branch, per-till, per-warehouse numbering. Empty string rather than
    -- NULL for "one sequence for the whole workspace", because it is part of a
    -- unique index and NULLs do not compare equal to each other - two unscoped
    -- sequences for one doc_type would both be allowed.
    scope_key     TEXT NOT NULL DEFAULT '',

    -- The format. Mirrors phonix_core::numbering::Pattern, which is what
    -- validates it properly; the CHECK below is the floor under that.
    pattern       TEXT NOT NULL,

    reset_period  TEXT NOT NULL DEFAULT 'never',

    -- Which period the counter is currently running in - '2026-08', 'FY2026'.
    -- Compared for equality by the allocation and never parsed. The reset is
    -- not a scheduled job: it happens because this stops matching, inside the
    -- same statement that hands out the number, so a year boundary cannot
    -- interleave with an allocation and nothing has to run at midnight.
    period_key    TEXT NOT NULL DEFAULT '',

    -- The last number issued in `period_key`. 0 means none yet, which is why
    -- the allocation starts from `start_at` rather than from `counter + 1`
    -- whenever `counter < start_at`.
    counter       BIGINT NOT NULL DEFAULT 0,

    -- Where each period begins. Raising this above the current counter is how
    -- an administrator moves a sequence past numbers already issued elsewhere -
    -- the allocation picks it up on the next document.
    start_at      BIGINT NOT NULL DEFAULT 1,

    -- An inactive sequence refuses to allocate rather than allocating quietly.
    -- The allocation matches on this, so switching it off stops the documents
    -- rather than only hiding the row.
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,

    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by    UUID REFERENCES core.users (id) ON DELETE SET NULL,

    -- Both of these reach code as identifiers and reach a document as text.
    CONSTRAINT number_sequences_app_id_format CHECK (app_id ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT number_sequences_doc_type_format CHECK (doc_type ~ '^[a-z][a-z0-9_]*$'),

    -- A scope is rendered straight into the number, so it has to be something
    -- somebody can type into a search box or read down a phone.
    CONSTRAINT number_sequences_scope_format CHECK (
        scope_key ~ '^[A-Za-z0-9_-]*$' AND char_length(scope_key) <= 40
    ),

    -- The floor under `Pattern::parse`: a pattern with no counter would give
    -- every document in the workspace the same number, and that is worth
    -- refusing in the one place no code path can go around. Bracket classes
    -- rather than backslashes because braces are quantifier syntax in a POSIX
    -- regular expression.
    CONSTRAINT number_sequences_pattern_shape CHECK (
        char_length(pattern) BETWEEN 1 AND 60 AND pattern ~ '[{]N+[}]'
    ),

    CONSTRAINT number_sequences_reset_valid CHECK (
        reset_period IN ('never', 'daily', 'monthly', 'yearly', 'fiscal_year')
    ),

    CONSTRAINT number_sequences_counter_range CHECK (counter >= 0),
    CONSTRAINT number_sequences_start_range CHECK (start_at >= 1)
);

-- One sequence per document type per scope. This index is what the allocation
-- locks a single row through, so it is not only a uniqueness rule - it is the
-- reason the allocation is one statement and not a read followed by a write.
CREATE UNIQUE INDEX IF NOT EXISTS number_sequences_key
    ON core.number_sequences (app_id, doc_type, scope_key);

DROP TRIGGER IF EXISTS number_sequences_set_updated_at ON core.number_sequences;
CREATE TRIGGER number_sequences_set_updated_at
    BEFORE UPDATE ON core.number_sequences
    FOR EACH ROW
    EXECUTE FUNCTION core.set_updated_at();

-- ---------------------------------------------------------------------------
-- What is deliberately not here
--
-- **No `mode` column.** The design allows for a 'fast' mode - a real Postgres
-- SEQUENCE for documents where gaps are harmless, skipping the row contention
-- entirely. It is not built, because it needs a sequence created per row at
-- runtime, and a column whose only legal value is 'strict' is a column that
-- says nothing. Adding it later is `ALTER TABLE ... ADD COLUMN ... DEFAULT`,
-- which is additive and cheap; a value nothing implements is not.
--
-- **No unique index on the numbers themselves.** It cannot live here: core does
-- not hold documents. Each app puts a unique index on `(doc_type, number)` in
-- its own table, and that index is the belt to this table's braces -
-- administrators edit `start_at`, and it is what stops a duplicate reaching the
-- ledger when somebody edits it wrongly.
-- ---------------------------------------------------------------------------
