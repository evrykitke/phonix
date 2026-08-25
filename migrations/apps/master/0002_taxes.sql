-- master 0002: tax codes, rates, groups.
--
-- The design has to survive, without a schema change: EU and UK VAT, Indian GST
-- split into CGST/SGST/IGST, Canadian GST/PST/HST, US destination sales tax,
-- withholding tax, compound tax, and tax-inclusive pricing. It does that with
-- one indirection - a document line references a *group*, never a code - and
-- one exclusion constraint.

-- btree_gist is what lets a plain equality column sit inside a GiST exclusion
-- constraint beside a range. Pinned to `public` rather than left to the search
-- path: with `master` first it would otherwise be created inside `master`, and
-- uninstalling the app would take the extension with it.
CREATE EXTENSION IF NOT EXISTS btree_gist WITH SCHEMA public;

CREATE TABLE tax_codes (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code           TEXT NOT NULL,
    name           TEXT NOT NULL,
    kind           TEXT NOT NULL,
    country_code   TEXT,
    -- A state, province or district. Free text, because there is no list that
    -- covers every country below the country.
    region_code    TEXT,
    -- Computed on the base PLUS the taxes before it in sequence. Quebec's QST
    -- on top of GST is the canonical case.
    is_compound    BOOLEAN NOT NULL DEFAULT FALSE,
    -- What separates reclaimable input VAT from a cost. A posting consequence,
    -- not a label.
    is_recoverable BOOLEAN NOT NULL DEFAULT TRUE,
    is_active      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by     UUID REFERENCES core.users (id) ON DELETE SET NULL,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by     UUID REFERENCES core.users (id) ON DELETE SET NULL,

    -- Five kinds, and this one IS check-constrained, unlike the open role
    -- vocabulary next door: the kind decides whether an amount posts as a
    -- liability, a recoverable asset or a deduction from a payment, and a sixth
    -- would be a change to what this table means rather than an addition to a
    -- list.
    CONSTRAINT tax_codes_kind_valid
        CHECK (kind IN ('vat', 'gst', 'sales', 'withholding', 'excise')),
    CONSTRAINT tax_codes_code_shape
        CHECK (code ~ '^[A-Za-z0-9_-]{1,20}$'),
    CONSTRAINT tax_codes_name_present
        CHECK (char_length(btrim(name)) BETWEEN 1 AND 120),
    CONSTRAINT tax_codes_country_format
        CHECK (country_code IS NULL OR country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT tax_codes_region_length
        CHECK (region_code IS NULL OR char_length(region_code) <= 20),
    -- A region without a country is a district of nowhere, and a rate keyed on
    -- the region alone would match a same-named region elsewhere.
    CONSTRAINT tax_codes_region_needs_country
        CHECK (region_code IS NULL OR country_code IS NOT NULL)
);

CREATE UNIQUE INDEX tax_codes_code_key ON tax_codes (lower(code));
CREATE INDEX tax_codes_active ON tax_codes (is_active, lower(code));

-- Rates are effective-dated, and Postgres enforces it.
--
-- A code outlives its rates: UK VAT has been 15%, 17.5% and 20% under one name,
-- and a reprinted 2024 invoice has to show 2024's. So the rate is a row with a
-- window rather than a column on the code.
--
-- The window is half-open - `[valid_from, valid_to)` - so the day a rate
-- changes belongs to exactly one row. A closed range is how somebody eventually
-- writes yesterday's end date as today's start and two rates are live at once.
CREATE TABLE tax_rates (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tax_code_id UUID NOT NULL REFERENCES tax_codes (id) ON DELETE CASCADE,
    -- NUMERIC(9, 6): six places because several published district and cess
    -- rates are quoted to five, and three digits before the point because
    -- withholding arrangements above 100% exist.
    rate        NUMERIC(9, 6) NOT NULL,
    valid_from  DATE NOT NULL,
    -- Exclusive. NULL is open-ended.
    valid_to    DATE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  UUID REFERENCES core.users (id) ON DELETE SET NULL,

    -- Zero is a rate: a zero-rated supply is not an exempt one, and the
    -- difference is whether input tax can be recovered.
    CONSTRAINT tax_rates_not_negative CHECK (rate >= 0),
    CONSTRAINT tax_rates_forwards CHECK (valid_to IS NULL OR valid_to > valid_from),

    -- The constraint this table exists for. Two simultaneously-live rates for
    -- one code become impossible at the database level, rather than a mistake
    -- nobody notices until a quarter has been filed.
    CONSTRAINT tax_rates_no_overlap EXCLUDE USING gist (
        tax_code_id WITH =,
        daterange(valid_from, COALESCE(valid_to, 'infinity'::date), '[)') WITH &&
    )
);

CREATE INDEX tax_rates_lookup ON tax_rates (tax_code_id, valid_from DESC);

-- A line references a group, never a code.
--
-- "VAT 20%" is a group with one member. "GST 18%" is a group with CGST 9% and
-- SGST 9%. Quebec is a group with two members and is_compound on the second.
-- This single decision is what makes the model work in India and Canada without
-- a migration.
CREATE TABLE tax_groups (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code         TEXT NOT NULL,
    name         TEXT NOT NULL,
    country_code TEXT,
    is_active    BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by   UUID REFERENCES core.users (id) ON DELETE SET NULL,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by   UUID REFERENCES core.users (id) ON DELETE SET NULL,

    CONSTRAINT tax_groups_code_shape
        CHECK (code ~ '^[A-Za-z0-9_-]{1,20}$'),
    CONSTRAINT tax_groups_name_present
        CHECK (char_length(btrim(name)) BETWEEN 1 AND 120),
    CONSTRAINT tax_groups_country_format
        CHECK (country_code IS NULL OR country_code ~ '^[A-Z]{2}$')
);

CREATE UNIQUE INDEX tax_groups_code_key ON tax_groups (lower(code));
CREATE INDEX tax_groups_active ON tax_groups (is_active, lower(code));

CREATE TABLE tax_group_members (
    tax_group_id UUID NOT NULL REFERENCES tax_groups (id) ON DELETE CASCADE,
    -- No cascade: a tax code that is in a group cannot be deleted out from
    -- under it. Deactivating one is the supported way to retire it, and the
    -- documents that used it still have to resolve.
    tax_code_id  UUID NOT NULL REFERENCES tax_codes (id),
    -- Position in the compound order, ascending. What makes "the taxes before
    -- it" mean something rather than depending on how a query happened to sort.
    sequence     SMALLINT NOT NULL,
    PRIMARY KEY (tax_group_id, tax_code_id),
    CONSTRAINT tax_group_members_sequence_sane CHECK (sequence BETWEEN 0 AND 32)
);

-- The order a group is read in, and the only order the arithmetic accepts.
CREATE UNIQUE INDEX tax_group_members_order
    ON tax_group_members (tax_group_id, sequence);

-- Now that tax_groups exists, close the loop left open in 0001. NOT VALID is
-- not needed: the column is empty, because parties and groups arrive in the
-- same release.
ALTER TABLE parties
    ADD CONSTRAINT parties_tax_group_fk
    FOREIGN KEY (tax_group_id) REFERENCES tax_groups (id) ON DELETE SET NULL;
