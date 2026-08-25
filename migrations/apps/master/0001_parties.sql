-- master 0001: parties.
--
-- The first table of the first app. Everything here lands in the `master`
-- schema because this stream runs on a search path rooted there - see
-- `phonix_db::tenancy::apps`. References to core are written out in full;
-- `core` is deliberately absent from this path so that an unqualified
-- reference fails loudly instead of resolving by luck.
--
-- WHY ONE TABLE AND NOT TWO
--
-- A customer, a supplier, a carrier and an agent are the same row wearing
-- different hats, and in real trade they are routinely the same organization -
-- a company that buys from you and also delivers for you. Two tables would mean
-- two addresses to keep in step, two tax registrations, and no way for a
-- document to say the two are one party. So there is one table, and
-- `party_roles` is what an app claims about a row.
--
-- WHY THE ROLE IS A ROW AND NOT A COLUMN
--
-- A pair of booleans would need a migration in `master` every time an app
-- started using parties, and `master` cannot depend on the apps above it. A row
-- per claim costs a join and buys an app that ships without touching this file.

CREATE TABLE parties (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Assigned by the workspace, not generated. An accounts department that has
    -- called them ACME01 for twenty years is not going to stop.
    code          TEXT NOT NULL,
    kind          TEXT NOT NULL DEFAULT 'organization',
    name          TEXT NOT NULL,
    -- The registered name, when it differs from what they are called. An
    -- invoice is a legal instrument and names the entity, not its trading style.
    legal_name    TEXT,
    tax_id        TEXT,
    -- Where they are for tax purposes, which is not the same as any of their
    -- addresses: a party can be registered in one country and shipped to in
    -- another.
    country_code  TEXT,
    email         TEXT,
    phone         TEXT,
    website       TEXT,
    -- What they are normally invoiced in. NULL means the workspace's own.
    currency_code TEXT REFERENCES core.currencies (code),
    -- The tax treatment a new document reaches for. The foreign key is added in
    -- 0002, once tax_groups exists; the column is here so that a party created
    -- between the two migrations is still shaped correctly.
    tax_group_id  UUID,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by    UUID REFERENCES core.users (id) ON DELETE SET NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by    UUID REFERENCES core.users (id) ON DELETE SET NULL,

    -- TEXT with a CHECK rather than CHAR(n) throughout this file. CHAR is
    -- blank-padded, and a padded value compares equal in SQL and unequal in
    -- Rust - migration 0010 in core made the same call for currency_code.
    CONSTRAINT parties_code_shape
        CHECK (code ~ '^[A-Za-z0-9_-]{1,30}$'),
    CONSTRAINT parties_kind_valid
        CHECK (kind IN ('organization', 'person')),
    CONSTRAINT parties_name_present
        CHECK (char_length(btrim(name)) BETWEEN 1 AND 160),
    CONSTRAINT parties_legal_name_length
        CHECK (legal_name IS NULL OR char_length(legal_name) <= 160),
    CONSTRAINT parties_country_format
        CHECK (country_code IS NULL OR country_code ~ '^[A-Z]{2}$'),
    CONSTRAINT parties_currency_format
        CHECK (currency_code IS NULL OR currency_code ~ '^[A-Z]{3}$')
);

-- Case-insensitively unique: `acme01` and `ACME01` are the same customer typed
-- by two people, and letting both exist is how a statement comes out halved.
CREATE UNIQUE INDEX parties_code_key ON parties (lower(code));
CREATE INDEX parties_name_lookup ON parties (lower(name));
CREATE INDEX parties_active ON parties (is_active, lower(name));

-- What an app claims about a party.
--
-- No foreign key to anything app-shaped, and no CHECK listing the known roles:
-- the vocabulary is open on purpose, and a constraint here would have to be
-- migrated every time an app was added. The shape check is what stops a role
-- that cannot be searched for.
CREATE TABLE party_roles (
    party_id   UUID NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    role       TEXT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (party_id, role),
    CONSTRAINT party_roles_shape
        CHECK (role ~ '^[a-z][a-z0-9_]{0,31}$')
);

-- "Every customer" is the query a sales screen opens with, so the role leads.
CREATE INDEX party_roles_by_role ON party_roles (role, party_id);

-- Where to bill them, and where to send the goods.
--
-- Several per purpose are allowed - a group with two registered offices has
-- two billing addresses - so `purpose` narrows a picker rather than keying a
-- row. `is_primary` is the default among them, kept at most one per purpose by
-- the service rather than by a partial unique index: an index would refuse the
-- moment somebody ticked the new one before unticking the old.
CREATE TABLE party_addresses (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    party_id     UUID NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    purpose      TEXT NOT NULL DEFAULT 'billing',
    label        TEXT,
    line1        TEXT,
    line2        TEXT,
    city         TEXT,
    region       TEXT,
    postal_code  TEXT,
    country_code TEXT,
    is_primary   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by   UUID REFERENCES core.users (id) ON DELETE SET NULL,

    CONSTRAINT party_addresses_purpose_valid
        CHECK (purpose IN ('billing', 'shipping', 'other')),
    CONSTRAINT party_addresses_country_format
        CHECK (country_code IS NULL OR country_code ~ '^[A-Z]{2}$'),
    -- An address with nothing in it prints as a blank block on a document.
    CONSTRAINT party_addresses_not_empty
        CHECK (COALESCE(line1, line2, city, region, postal_code, country_code) IS NOT NULL),
    CONSTRAINT party_addresses_line_lengths
        CHECK (
            COALESCE(char_length(line1), 0) <= 120
            AND COALESCE(char_length(line2), 0) <= 120
            AND COALESCE(char_length(city), 0) <= 120
            AND COALESCE(char_length(region), 0) <= 120
            AND COALESCE(char_length(postal_code), 0) <= 120
            AND COALESCE(char_length(label), 0) <= 120
        )
);

CREATE INDEX party_addresses_by_party ON party_addresses (party_id, purpose);

-- Who at that organization to actually write to.
--
-- "Accounts payable" and "the person who signs the order" are two different
-- addresses at one company, and sending a payment reminder to the second is how
-- a reminder gets ignored. The party's own `email` stays: it is the front door,
-- and these are the people behind it.
CREATE TABLE party_contacts (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    party_id   UUID NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    job_title  TEXT,
    email      TEXT,
    phone      TEXT,
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by UUID REFERENCES core.users (id) ON DELETE SET NULL,

    CONSTRAINT party_contacts_name_present
        CHECK (char_length(btrim(name)) BETWEEN 1 AND 120),
    CONSTRAINT party_contacts_job_title_length
        CHECK (job_title IS NULL OR char_length(job_title) <= 120)
);

CREATE INDEX party_contacts_by_party ON party_contacts (party_id);
