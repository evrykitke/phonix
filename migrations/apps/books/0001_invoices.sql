-- books 0001: sales invoices.
--
-- The first real app, and the first schema that holds a *document* rather than
-- a setting or a piece of master data. Everything below lands in the `books`
-- schema, because this stream runs on a search path rooted there.
--
-- WHAT IS AND IS NOT A FOREIGN KEY
--
-- `core.currencies` and `core.users` are referenced properly: core is the one
-- schema every app is allowed to point at, and that privilege is the whole
-- reason it is kept small.
--
-- `master.parties` and `master.tax_codes` are referenced **by id only, with no
-- foreign key**. That is the rule that makes an app uninstallable: an FK from
-- `books` into `master` would mean master could never be dropped, and an FK
-- into another app would mean neither could. The cost is that this schema
-- cannot ask the database whether a party still exists - which is exactly why
-- the columns beside the id are a snapshot rather than a join.
--
-- WHY THE SNAPSHOT
--
-- A customer who moves, a tax that changes and a rate that drifts must not
-- rewrite an invoice that was already sent. So the party's name and address,
-- and every tax code's name and rate, are copied onto the document when it is
-- posted and never resolved again. Same discipline as `entity_events`: record
-- what was true, not a pointer to what is true now.

CREATE TABLE invoices (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- NULL while it is a draft. Taken from core.number_sequences at post, in
    -- the same transaction as the write - see the note on `allocate`.
    number             TEXT,
    status             TEXT NOT NULL DEFAULT 'draft',

    -- The customer, by id and as a copy. No foreign key: see above.
    party_id           UUID NOT NULL,
    party_code         TEXT NOT NULL,
    -- The *registered* name where the party has one. An invoice is a legal
    -- instrument and names the entity, not its trading style.
    party_name         TEXT NOT NULL,
    party_tax_id       TEXT,
    party_line1        TEXT,
    party_line2        TEXT,
    party_city         TEXT,
    party_region       TEXT,
    party_postal_code  TEXT,
    party_country_code TEXT,

    issued_on          DATE NOT NULL,
    due_on             DATE,

    currency_code      TEXT NOT NULL REFERENCES core.currencies (code),

    -- The six-column conversion snapshot, together or not at all. Recomputing
    -- a base amount later from today's rate is the classic bug, and it
    -- silently rewrites history.
    --
    -- All NULL when the invoice is already in the workspace's own currency:
    -- there is nothing to convert, and a rate of one is not evidence of a
    -- quotation somebody published.
    base_currency_code TEXT REFERENCES core.currencies (code),
    exchange_rate      NUMERIC(20, 10),
    rate_date          DATE,
    base_gross_amount  NUMERIC(19, 4),

    -- The two policies that decide what the document comes to. Stored, not
    -- inferred: reconciliation disputes come from these being implicit, and a
    -- workspace that changes its default must not change what a document
    -- already says.
    pricing            TEXT NOT NULL DEFAULT 'exclusive',
    rounding_level     TEXT NOT NULL DEFAULT 'line',
    rounding           TEXT NOT NULL DEFAULT 'half_up',

    -- Stored rather than recomputed on read. The arithmetic is deterministic,
    -- so recomputing would agree today - and a corrected tax rate would
    -- silently disagree next year.
    net_amount         NUMERIC(19, 4) NOT NULL DEFAULT 0,
    tax_amount         NUMERIC(19, 4) NOT NULL DEFAULT 0,
    gross_amount       NUMERIC(19, 4) NOT NULL DEFAULT 0,

    notes              TEXT,

    posted_at          TIMESTAMPTZ,
    posted_by          UUID REFERENCES core.users (id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by         UUID REFERENCES core.users (id) ON DELETE SET NULL,
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by         UUID REFERENCES core.users (id) ON DELETE SET NULL,

    CONSTRAINT invoices_status_valid
        CHECK (status IN ('draft', 'posted', 'voided')),
    CONSTRAINT invoices_pricing_valid
        CHECK (pricing IN ('exclusive', 'inclusive')),
    CONSTRAINT invoices_rounding_level_valid
        CHECK (rounding_level IN ('line', 'document')),
    CONSTRAINT invoices_rounding_valid
        CHECK (rounding IN ('half_up', 'half_even')),
    CONSTRAINT invoices_currency_format
        CHECK (currency_code ~ '^[A-Z]{3}$'),
    CONSTRAINT invoices_country_format
        CHECK (party_country_code IS NULL OR party_country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT invoices_due_after_issued
        CHECK (due_on IS NULL OR due_on >= issued_on),
    CONSTRAINT invoices_party_name_present
        CHECK (char_length(btrim(party_name)) BETWEEN 1 AND 160),

    -- A draft has no number and a posted document has one. Both halves are
    -- checked, because a posted invoice without a number is a document that
    -- cannot be referred to and a numbered draft is a number burned on
    -- something that may never be sent.
    CONSTRAINT invoices_number_follows_status
        CHECK ((status = 'draft') = (number IS NULL)),

    -- A posted invoice records who posted it and when. A draft has not been.
    CONSTRAINT invoices_posted_recorded
        CHECK ((status = 'draft') = (posted_at IS NULL)),

    -- The conversion snapshot is all six columns or none of them.
    CONSTRAINT invoices_rate_snapshot_whole
        CHECK (
            (base_currency_code IS NULL
             AND exchange_rate IS NULL
             AND rate_date IS NULL
             AND base_gross_amount IS NULL)
            OR (base_currency_code IS NOT NULL
                AND exchange_rate IS NOT NULL
                AND rate_date IS NOT NULL
                AND base_gross_amount IS NOT NULL)
        ),
    CONSTRAINT invoices_rate_positive
        CHECK (exchange_rate IS NULL OR exchange_rate > 0)
);

-- Belt and braces.
--
-- The sequence in `core` is what makes numbering gap-free; this is what stops a
-- duplicate reaching the ledger if somebody edits a series by hand. It cannot
-- live in core, which holds no documents.
--
-- Partial, because every draft has a NULL number and NULLs do not compare equal
-- - a plain unique index would allow two drafts and then be useless.
CREATE UNIQUE INDEX invoices_number_key ON invoices (number) WHERE number IS NOT NULL;

-- "Everything for this customer" and "what is owed" are the two questions a
-- sales screen opens with.
CREATE INDEX invoices_by_party ON invoices (party_id, issued_on DESC);
CREATE INDEX invoices_by_status ON invoices (status, issued_on DESC);
CREATE INDEX invoices_due ON invoices (due_on) WHERE status = 'posted';

CREATE TABLE invoice_lines (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    invoice_id     UUID NOT NULL REFERENCES invoices (id) ON DELETE CASCADE,
    -- Position on the document, from one. What the reader sees, and what makes
    -- "line 3" mean something to both of us.
    line_no        SMALLINT NOT NULL,

    description    TEXT NOT NULL,
    -- Four places, matching the amount scale, so quantity times price is a
    -- ratio of two integers with a single rounding.
    quantity       NUMERIC(19, 4) NOT NULL,
    unit_price     NUMERIC(19, 4) NOT NULL,

    net_amount     NUMERIC(19, 4) NOT NULL,
    tax_amount     NUMERIC(19, 4) NOT NULL,
    gross_amount   NUMERIC(19, 4) NOT NULL,

    -- Where the treatment came from. By id, with no foreign key, and kept for
    -- tracing only - the taxes below are what the document actually charged.
    tax_group_id   UUID,
    tax_group_code TEXT NOT NULL DEFAULT '',

    CONSTRAINT invoice_lines_description_present
        CHECK (char_length(btrim(description)) BETWEEN 1 AND 500),
    -- A line of nothing prints and charges nothing.
    CONSTRAINT invoice_lines_quantity_not_zero
        CHECK (quantity <> 0),
    CONSTRAINT invoice_lines_line_no_positive
        CHECK (line_no >= 1)
);

CREATE UNIQUE INDEX invoice_lines_order ON invoice_lines (invoice_id, line_no);

-- What each line was actually charged, in the order it applied.
--
-- This is the snapshot that makes a 2030 reprint of a 2026 invoice show 2026's
-- rate and 2026's name, after the code has been renamed and the rate changed
-- twice. `tax_code_id` has no foreign key for the usual reason, and would be
-- the wrong thing to read on a reprint even if it did.
CREATE TABLE invoice_line_taxes (
    line_id        UUID NOT NULL REFERENCES invoice_lines (id) ON DELETE CASCADE,
    sequence       SMALLINT NOT NULL,

    tax_code_id    UUID NOT NULL,
    tax_code       TEXT NOT NULL,
    tax_name       TEXT NOT NULL,
    tax_kind       TEXT NOT NULL,
    rate           NUMERIC(9, 6) NOT NULL,
    is_compound    BOOLEAN NOT NULL DEFAULT FALSE,
    is_recoverable BOOLEAN NOT NULL DEFAULT TRUE,

    -- What this tax was charged on: the net, or the net plus the taxes before
    -- it when compound. Stored, because it is what a reader checks the rate
    -- against and deriving it later would need the whole chain again.
    taxable_amount NUMERIC(19, 4) NOT NULL,
    tax_amount     NUMERIC(19, 4) NOT NULL,

    PRIMARY KEY (line_id, sequence),
    CONSTRAINT invoice_line_taxes_kind_valid
        CHECK (tax_kind IN ('vat', 'gst', 'sales', 'withholding', 'excise')),
    CONSTRAINT invoice_line_taxes_rate_not_negative
        CHECK (rate >= 0)
);
