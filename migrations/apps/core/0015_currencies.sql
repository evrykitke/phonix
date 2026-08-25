-- ---------------------------------------------------------------------------
-- 0015: the currencies this workspace transacts in, and the rates between them.
--
-- # This table is not the ISO 4217 list
--
-- The list is already in the binary. `phonix_core::locale::Currency` carries
-- every active code with its name and - the field that matters - its minor
-- units: 0 for the yen, 2 for the dollar, 3 for the Kuwaiti dinar. Copying that
-- into a hundred and sixty rows per tenant database would give every workspace
-- its own answer to "how many decimal places does JPY have", and the answer
-- that is wrong would be the one nobody ever looks at.
--
-- So this table holds only the part that is genuinely the tenant's: **which
-- currencies it uses**, and what symbol it wants printed. Name and minor units
-- are looked up in the compiled table, exactly as `organization_profile`
-- already does with `currency_code`. It is the same rule 0014 applies to
-- `installed_apps`, where `app_id` *is* the schema name: two places holding one
-- fact eventually disagree, and deriving cannot.
--
-- # Why a table at all, then
--
-- Because `exchange_rates` needs something to point at, and because a picker
-- offering a hundred and sixty currencies to an organization that invoices in
-- two is a picker nobody wants. A row here means "we deal in this".
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS core.currencies (
    -- ISO 4217, upper case. TEXT with a CHECK rather than CHAR(3) for the
    -- reason 0010 gives: CHAR is blank-padded, and a padded code compares equal
    -- in SQL and unequal in Rust.
    code        TEXT PRIMARY KEY,

    -- A row that has stopped being used, rather than a row that is gone. The
    -- rates and the documents that reference it still have to resolve, so
    -- switching a currency off hides it from pickers and nothing more.
    is_enabled  BOOLEAN NOT NULL DEFAULT TRUE,

    -- What to print on a document, when the organization has an opinion. KSh
    -- rather than KES, or a bare dollar sign for a business that only ever
    -- trades in one dollar.
    --
    -- Deliberately absent from the compiled table and present here, because a
    -- symbol is not a fact about the currency: '$' is at least a dozen of them,
    -- and which one it means depends entirely on who is reading. That makes it
    -- the tenant's choice rather than ISO's.
    symbol      TEXT,

    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by  UUID REFERENCES core.users (id) ON DELETE SET NULL,

    CONSTRAINT currencies_code_format CHECK (code ~ '^[A-Z]{3}$'),
    -- An optional field is NULL or a value, never ''. Same floor as 0010.
    CONSTRAINT currencies_symbol_shape CHECK (
        symbol IS NULL OR char_length(symbol) BETWEEN 1 AND 8
    )
);

DROP TRIGGER IF EXISTS currencies_set_updated_at ON core.currencies;
CREATE TRIGGER currencies_set_updated_at
    BEFORE UPDATE ON core.currencies
    FOR EACH ROW
    EXECUTE FUNCTION core.set_updated_at();

-- The organization already has a currency - 0010 gave it one, defaulting to
-- USD - and every amount in the workspace is denominated in it. Seeding it here
-- means no screen has to handle the case where the base currency is not in the
-- list it is choosing from.
INSERT INTO core.currencies (code)
SELECT currency_code FROM core.organization_profile
ON CONFLICT (code) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Exchange rates.
--
-- Mechanism, not policy. This table records what a rate *was*, on a day,
-- according to somebody. Which rate a document should use - the invoice date,
-- the receipt date, a monthly average, a fixed budget rate - is a decision the
-- posting app makes, and different apps in the same workspace legitimately make
-- it differently.
--
-- # Never interpolate
--
-- A lookup is "the latest row at or before the document date". Not a blend of
-- the two nearest, however tempting that looks on a chart: an auditor asks
-- which published rate was used, and "somewhere between Tuesday and Thursday"
-- is not an answer anyone accepts.
--
-- # Both directions are rows
--
-- A rate says how many `quote` one `base` buys. The inverse is not stored and
-- is not computed: real quotations have a spread, so 1/1.0925 is a number
-- rather than a rate. An organization that needs EUR->USD and USD->EUR records
-- both, and `phonix_core::money` refuses to apply one the wrong way round -
-- which matters because an inverted rate produces a plausible figure, and a
-- plausible wrong figure is the kind nobody catches.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS core.exchange_rates (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    base_code   TEXT NOT NULL REFERENCES core.currencies (code),
    quote_code  TEXT NOT NULL REFERENCES core.currencies (code),

    -- NUMERIC(20, 10), and the ten decimal places are not generosity. JPY to
    -- USD is around 0.0067; at four places that rounds to a tenth of a percent
    -- of error, applied to every yen invoice in the ledger. Mirrors
    -- `phonix_core::money::RATE_SCALE`.
    rate        NUMERIC(20, 10) NOT NULL,

    -- The day the rate was published, not the day it was fetched. DATE and not
    -- TIMESTAMPTZ: central banks publish a daily fixing, and giving it a time
    -- invents a precision the source does not have.
    as_of       DATE NOT NULL,

    -- Where it came from: 'ecb', 'manual', a provider name. Free text, because
    -- the set of sources an organization uses is not ours to enumerate, and it
    -- is part of the key so two feeds can disagree without overwriting.
    source      TEXT NOT NULL,

    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  UUID REFERENCES core.users (id) ON DELETE SET NULL,

    CONSTRAINT exchange_rates_positive CHECK (rate > 0),
    -- A currency does not have a rate against itself. It is 1, it is not
    -- quoted, and a row saying otherwise is an import that went wrong.
    CONSTRAINT exchange_rates_distinct CHECK (base_code <> quote_code),
    CONSTRAINT exchange_rates_source_shape CHECK (
        char_length(source) BETWEEN 1 AND 40
    )
);

-- One rate per pair, per day, per source. Re-fetching the same day from the
-- same feed updates the row rather than adding a second one, which is what
-- makes "the rate on that date" a question with one answer.
CREATE UNIQUE INDEX IF NOT EXISTS exchange_rates_point
    ON core.exchange_rates (base_code, quote_code, as_of, source);

-- The lookup this table exists for: walk back from a document date to the most
-- recent published rate. DESC on `as_of` so that walk is the index order rather
-- than a sort afterwards.
CREATE INDEX IF NOT EXISTS exchange_rates_lookup
    ON core.exchange_rates (base_code, quote_code, as_of DESC);
