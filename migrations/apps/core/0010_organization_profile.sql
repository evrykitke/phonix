-- ---------------------------------------------------------------------------
-- 0010: who this workspace legally is.
--
-- The catalog already holds a `display_name` per tenant. That is routing and
-- support metadata - what an operator calls this workspace in a list - and it
-- was typed into a signup box. This table is the *legal entity*: the name that
-- goes on a document, with a registration number, an address, and the currency
-- and time zone the organization actually works in. Two different facts, kept
-- apart, because otherwise the first invoice carries whatever somebody typed
-- while they were creating an account.
--
-- It lives in the tenant database and not in the catalog, because it is the
-- workspace's own data - the same rule the catalog states about itself.
--
-- # Columns, not a JSONB blob
--
-- The same argument as `workspace_settings` in 0004. Every constrained value
-- here has a CHECK that says what a valid one is, and a currency code that can
-- be saved as 'dollars' is a currency nothing can convert.
--
-- # What is deliberately not here yet
--
-- A locale (number and date formatting). Currency earns its column because it
-- carries `minor_units`, and the time zone earns its because it decides what
-- "today" means in a report. A formatting locale changes nothing until there
-- is something formatting, and a column nobody reads is a column that drifts.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS organization_profile (
    -- Exactly one row, for ever. The same singleton shape as
    -- `workspace_settings` and `mail_settings`, and for the same reason: a
    -- second row would be a second organization that half the queries read.
    id                      BOOLEAN PRIMARY KEY DEFAULT TRUE,

    -- --- who it is ---------------------------------------------------------

    -- Empty until somebody fills it in. That is a real state, not a missing
    -- row: the row is seeded below so every read finds one, and the screen
    -- shows a nudge rather than the application inventing a name.
    legal_name              TEXT NOT NULL DEFAULT '',
    trading_name            TEXT,

    -- Free text, both of them. There are as many registration and tax number
    -- formats as there are registries, and validating one shape refuses every
    -- other jurisdiction.
    registration_number     TEXT,
    tax_id                  TEXT,
    industry                TEXT,

    -- --- how to reach it ---------------------------------------------------

    -- The organization's own address, which is rarely the relay's
    -- from-address in `mail_settings`: one is where a customer replies, the
    -- other is what the server authenticates as.
    email                   TEXT,
    phone                   TEXT,
    website                 TEXT,

    -- --- where it is -------------------------------------------------------

    address_line1           TEXT,
    address_line2           TEXT,
    city                    TEXT,
    -- State, province, county or region. One column, because which level
    -- matters differs by country and asking for the wrong one is worse than
    -- asking for neither.
    region                  TEXT,
    postal_code             TEXT,
    -- ISO 3166-1 alpha-2, mirroring phonix_core::locale::Country. TEXT with a
    -- CHECK rather than CHAR(2): CHAR is blank-padded, and a padded code is a
    -- code that compares equal in SQL and unequal in Rust.
    country_code            TEXT,

    -- --- how it counts -----------------------------------------------------

    -- ISO 4217, mirroring phonix_core::locale::Currency. The reason that type
    -- exists is `minor_units` - 0 for the yen, 3 for the Kuwaiti dinar - and a
    -- code that is not in its table has no answer for that.
    currency_code           TEXT NOT NULL DEFAULT 'USD',

    -- IANA name, e.g. 'Africa/Nairobi'. Only the shape is constrained here and
    -- in the domain type; whether the name is really in the tz database is
    -- answered on the server, which is the only place that carries it.
    timezone                TEXT NOT NULL DEFAULT 'UTC',

    -- The month the financial year opens. January for most, but April, July
    -- and October are each common enough that assuming is wrong every year.
    fiscal_year_start_month SMALLINT NOT NULL DEFAULT 1,

    -- --- branding ----------------------------------------------------------

    -- The logo that goes on documents. A reference into `file_uploads`, not
    -- bytes in this row: an upload is a job here - quarantined, verified, then
    -- published - and a BYTEA column would skip all of it, so the one image
    -- this workspace puts on everything it issues would be the one image
    -- nobody scanned.
    --
    -- ON DELETE SET NULL rather than RESTRICT: deleting a file from the files
    -- screen should cost the profile its logo, not fail with a foreign-key
    -- error naming a table the person has never heard of. The screen re-reads
    -- and shows the empty state, which is recoverable by uploading again.
    logo_file_id            UUID REFERENCES file_uploads (id) ON DELETE SET NULL,

    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by              UUID REFERENCES users (id) ON DELETE SET NULL,

    CONSTRAINT organization_profile_singleton CHECK (id),

    -- An optional field is NULL or a value, never ''. The application
    -- normalises blanks away before writing; this is the floor under it, so a
    -- row written by anything else cannot make `WHERE tax_id IS NULL` miss the
    -- rows it should find. NULL <> '' is NULL, which a CHECK accepts - so this
    -- refuses the empty string and nothing else.
    CONSTRAINT organization_profile_no_blank_optionals CHECK (
        trading_name        <> ''
        AND registration_number <> ''
        AND tax_id          <> ''
        AND industry        <> ''
        AND email           <> ''
        AND phone           <> ''
        AND website         <> ''
        AND address_line1   <> ''
        AND address_line2   <> ''
        AND city            <> ''
        AND region          <> ''
        AND postal_code     <> ''
    ),

    CONSTRAINT organization_profile_name_length CHECK (
        char_length(legal_name) <= 200
        AND (trading_name IS NULL OR char_length(trading_name) <= 200)
    ),

    CONSTRAINT organization_profile_country_code CHECK (
        country_code IS NULL OR country_code ~ '^[A-Z]{2}$'
    ),
    CONSTRAINT organization_profile_currency_code CHECK (
        currency_code ~ '^[A-Z]{3}$'
    ),
    -- 64 is the longest name in the tz database with room to spare; the
    -- charset is what keeps a shell fragment out of a column that is read back
    -- and handed to a date library.
    CONSTRAINT organization_profile_timezone CHECK (
        char_length(timezone) BETWEEN 1 AND 64
        AND timezone ~ '^[A-Za-z0-9_+/-]+$'
    ),
    CONSTRAINT organization_profile_fiscal_month CHECK (
        fiscal_year_start_month BETWEEN 1 AND 12
    )
);

-- The row exists from this migration onwards, so every read finds a profile and
-- no code path has to invent one.
INSERT INTO organization_profile (id) VALUES (TRUE) ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Changing who the organization says it is, is an audited act.
--
-- The legal name, the registration number and the tax id are what appear on
-- anything this workspace issues. "Who changed the entity name, and to what" is
-- exactly the question asked after a document goes out wrong, and the from/to
-- shape is what earns it a diff on the audit detail page rather than a sentence
-- saying something changed.
-- ---------------------------------------------------------------------------

ALTER TABLE identity_events DROP CONSTRAINT IF EXISTS identity_events_event_valid;

ALTER TABLE identity_events ADD CONSTRAINT identity_events_event_valid CHECK (
    event IN (
        'signup',
        'login',
        'logout',
        'password_change',
        'password_reset_requested',
        'password_reset_completed',
        'email_verification_sent',
        'email_verified',
        'mfa_enrolled',
        'mfa_challenge',
        'mfa_removed',
        'mfa_recovery_used',
        'mfa_recovery_generated',
        'account_locked',
        'account_unlocked',
        'role_changed',
        'session_revoked',
        'invitation_sent',
        'invitation_accepted',
        'password_policy_changed',
        'mfa_policy_changed',
        'user_permissions_changed',
        'role_permissions_changed',
        'user_updated',
        'mail_settings_changed',
        'role_created',
        'role_updated',
        'role_deleted',
        'organization_profile_changed',
        'organization_logo_changed'
    )
);
