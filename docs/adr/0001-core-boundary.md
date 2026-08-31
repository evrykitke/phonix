# ADR 0001 — The core boundary, and the foundation entities

Status: accepted, built 2026-08-24 onwards
Date: 2026-08-24

The record was revised as it was implemented rather than rewritten afterwards:
each `### Revised:` section below is a decision the build changed, kept in place
next to the decision it replaced so the reasoning survives. One thing here is
deliberately **not** built and says so - §5's `mode` column, because a column
whose only legal value is `'strict'` says nothing, and adding it later is
additive.

The tenant database is about to stop being one flat namespace. This record draws
the line between what is *infrastructure* and what is *an app*, and specifies the
three foundation entities every commercial app needs before it can exist:
currencies, taxes, and document numbers.

---

## 1. Two layers, not one

```
phonix_tenant_acme
  ├── core.*      infrastructure. Always installed. Cannot be uninstalled.
  ├── master.*    commercial master data. Its own schema and stream like any
  │               app, but never offered in the store - see section 8.
  └── books.*     an app.
      procurement.*
```

`core` is the schema every other schema is allowed to reference. That privilege
is the whole reason to keep it small: a change to `core` is a breaking change for
every app at once, and `core` can never be uninstalled to escape a bad decision
made inside it.

### The test for core

All three must hold:

1. **Every app needs it** — not two of them.
2. **It has no opinion about a business process.** The moment `core` knows what
   an approval is, every app bends around `core`'s idea of approval.
3. **It is identity or mechanism, not meaning.**

`core` knows a **party exists**. It does not know the party is a supplier —
"supplier" is a meaning Procurement assigns. `core` knows how to allocate a
formatted counter. It does not know what an invoice is.

### Rules

- **`core` has no `requires` and may never reference an app.** Enforced by a test
  over the manifest graph.
- **No foreign keys across app schemas.** An FK from `books` into `procurement`
  means procurement can never be uninstalled. Reference by id, resolve through a
  capability port. The only permitted cross-schema FK target is `core`.
- **`core` migrations are additive.** New columns nullable or defaulted; no
  renames, no drops in the same release as the code that stops using them.
  Everything downstream is already deployed against the old shape.
- **Wait for the third.** Two apps needing the same thing is a coincidence;
  three is a pattern. The shape extracted at two is usually wrong in a way that
  can no longer be changed.
- **The database refuses; it does not act.** No triggers, no stored procedures,
  no rules. PostgreSQL enforces what the data *may be* - `NOT NULL`, `CHECK`,
  `REFERENCES`, `UNIQUE`, `DEFAULT` on insert - and those refuse a bad write
  rather than performing one of their own. Everything that decides is Rust. See
  the note below.

### Revised: the `updated_at` triggers are gone

`core` shipped with five `BEFORE UPDATE` triggers setting `updated_at = now()`,
plus one in the catalog and the plpgsql function behind them. Migration
`0017_drop_updated_at_triggers.sql` removes all of it, and every statement that
updates those rows now sets the column itself.

The argument is not performance. A trigger is behaviour that does not appear at
the call site: a repository function reads as though it writes four columns and
in fact writes five, and the only way to find out is to go and read the schema.
Behaviour invisible where it happens is behaviour that gets forgotten - and both
copies had already gone quietly wrong:

- `users.updated_at` moved on every page view, because `last_seen_at` is a
  write, and on every mistyped password, because `failed_login_count` is a
  write. "When was this user last changed?" had no answer.
- `number_sequences.updated_at` moved on every document issued, while
  `updated_by` beside it still named whoever last edited the settings. A
  timestamp and an author that disagree is not a stale record; it is a false
  one.

Setting the column at the call site makes it a decision. The rule:

> **`updated_at` follows an edit to the row's own data.** It does not follow the
> login trail - `last_seen_at`, `last_login_at`, `failed_login_count`,
> `locked_until` - and it does not follow allocating a document number.

Nothing outside `phonix-db` read any of those columns, so the change reaches no
caller. The `tenant_schema` test now asserts that a tenant database holds **no**
triggers and **no** non-extension routines, stated as an absolute rather than a
count, because this is exactly the kind of thing that comes back one table at a
time.

---

## 2. Where the existing seventeen tables land

Every table in `migrations/tenant/0001`–`0013` is infrastructure. All of it is
`core`, unchanged in shape:

| Group | Tables |
| --- | --- |
| Identity | `users`, `sessions`, `user_tokens`, `user_mfa_factors`, `password_history`, `identity_events` |
| Authorization | `roles`, `user_roles`, `role_permissions`, `user_permissions` |
| Audit | `entity_events` |
| Files | `file_uploads` |
| Settings | `workspace_settings`, `organization_profile`, `mail_settings` |
| Messaging | `outbox_events`, `processed_events` |

Nothing there needs to move out. Four tables join them: `installed_apps`,
`currencies`, `exchange_rates`, and `number_sequences`.

### `core.installed_apps`

```sql
CREATE TABLE core.installed_apps (
    app_id          TEXT PRIMARY KEY,
    schema_version  TEXT,
    state           TEXT NOT NULL DEFAULT 'installing',
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    migrated_at     TIMESTAMPTZ,
    CONSTRAINT installed_apps_app_id_format
        CHECK (app_id ~ '^[a-z][a-z0-9_]*$'),
    CONSTRAINT installed_apps_app_id_length
        CHECK (char_length(app_id) BETWEEN 2 AND 63),
    CONSTRAINT installed_apps_state_valid
        CHECK (state IN ('installing', 'active', 'read_only', 'uninstalling'))
);
```

There is no `schema_name` column: **`app_id` *is* the schema name.** Storing both
invites the two to disagree; deriving one from the other cannot. The format
check exists because that value reaches DDL as an identifier.

**`state` here is installation, not entitlement.** Whether a tenant has *paid*
for Books lives in the catalog database, because billing is cross-tenant and has
to be answerable without opening a tenant database. `read_only` is the *effect*
of a lapsed subscription, written here by the subscription service.

Install is `CREATE SCHEMA` plus that app's migration stream. Uninstall is export,
then `DROP SCHEMA … CASCADE` — which is only safe because of the
no-cross-schema-FK rule.

---

## 3. Currencies — `core`

A currency list is reference data with no business opinion, in the same class as
timezones and country codes. `organization_profile` already carries
`currency_code` and `fiscal_year_start_month`, so `core` has already committed to
knowing what a base currency is. Rates are mechanism — *policy* (which rate to
use, when to revalue) stays in the app that posts.

### Revised: the table is the selection, not the list

The first draft of this record specified `core.currencies` with `numeric_code`,
`name`, `symbol` and `minor_unit`, seeded from the full ISO 4217 list. That was
wrong, and implementing it is what showed why: **`phonix_core::locale::Currency`
already is that list**, compiled into both the server and the wasm bundle, with
the one field that matters — `minor_units`, 0 for the yen, 3 for the Kuwaiti
dinar. Copying it into a hundred and sixty rows per tenant database gives every
workspace its own answer to "how many decimal places does JPY have", and the
answer that is wrong is the one nobody ever looks at.

It is the same rule §2 applies to `installed_apps`, where `app_id` *is* the
schema name. Two places holding one fact eventually disagree; deriving cannot.

So the table holds only what is genuinely the tenant's — which codes it deals in,
and what symbol it wants printed:

```sql
CREATE TABLE core.currencies (
    code        TEXT PRIMARY KEY,
    is_enabled  BOOLEAN NOT NULL DEFAULT TRUE,
    symbol      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by  UUID REFERENCES core.users (id) ON DELETE SET NULL,
    CONSTRAINT currencies_code_format CHECK (code ~ '^[A-Z]{3}$')
);
```

`TEXT` with a `CHECK`, not `CHAR(3)` — `CHAR` is blank-padded, and a padded code
compares equal in SQL and unequal in Rust. Migration 0010 already made that call
for `organization_profile.currency_code`.

The symbol *is* per-tenant, which is why it survived the cut: `$` is at least a
dozen currencies and which one it means depends entirely on who is reading, so it
is the organization's choice rather than ISO's. The base currency is seeded from
`organization_profile`, so no screen has to handle a base currency missing from
its own picker.

**There is no delete.** A currency the workspace has stopped using is disabled.
Rates and posted documents still have to resolve, and a foreign-key error naming
`exchange_rates` is not a useful answer to somebody tidying a settings screen.

### Rates

```sql
CREATE TABLE core.exchange_rates (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    base_code   TEXT NOT NULL REFERENCES core.currencies (code),
    quote_code  TEXT NOT NULL REFERENCES core.currencies (code),
    rate        NUMERIC(20, 10) NOT NULL,
    as_of       DATE NOT NULL,
    source      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  UUID REFERENCES core.users (id) ON DELETE SET NULL,
    CONSTRAINT exchange_rates_positive CHECK (rate > 0),
    CONSTRAINT exchange_rates_distinct CHECK (base_code <> quote_code)
);

CREATE UNIQUE INDEX exchange_rates_point
    ON core.exchange_rates (base_code, quote_code, as_of, source);
CREATE INDEX exchange_rates_lookup
    ON core.exchange_rates (base_code, quote_code, as_of DESC);
```

Ten decimal places, not four. JPY to USD is around 0.0067; four places is a tenth
of a percent of error applied to every yen invoice in the ledger.

Lookup is the latest row with `as_of <=` the document date, and a date before the
earliest rate on file returns **nothing** rather than the earliest one —
extrapolating backwards is inventing a quotation. Never interpolate between two
rates either: an auditor asks which published rate was used, and "a blend of
Tuesday and Thursday" is not an answer.

`source` is part of the key, so two feeds can disagree about a day without
overwriting each other, and re-running one feed corrects its row rather than
leaving two and a question about which the document used.

**The inverse is not computed.** Real quotations have a spread, so 1/1.0925 is a
number rather than a rate. An organization needing both directions records both,
and `Money::convert` refuses a rate pointing the wrong way — which matters
because an inverted rate produces a *plausible* figure, and a plausible wrong
figure is the kind nobody catches.

### Revised: the screen, and what it may not do

`Administration → Settings → Currencies`. A grid of what the workspace has
switched on, and one panel that both adds and edits - because the service is an
upsert, and "use EUR" is a statement about the end state rather than an event.

Two absences are the design:

- **No delete.** Only enable and disable, and the action offered is whichever
  one would do something. Rates and posted documents still have to resolve.
- **No minor-units column to edit.** Decimal places come from
  `phonix_core::locale::Currency`, and the grid shows them read-only. A hundred
  and sixty editable rows per tenant database is a hundred and sixty chances
  for one workspace to disagree with ISO 4217.

Switching off the *base* currency is refused by the service, because every
amount in the workspace is expressed against it and a picker that could not
offer it would be a screen that cannot show its own totals.

Reading the list is ungated; writing is `Administration.Settings`. Every screen
with an amount on it needs the list to render a picker, so requiring a
permission to read would mean granting the administration area to anybody who
can raise a document.

### The money rule

- **Never `FLOAT` or `DOUBLE PRECISION`.** `NUMERIC(19, 4)` for amounts.
- **An amount is never one column.** The minimum is the pair
  `amount NUMERIC(19,4)` + `currency_code`.
- **Foreign-currency documents store the whole snapshot**, always together:
  currency, amount, base currency, base amount, rate, rate date. Recomputing the
  base amount later from today's rate is the classic bug, and it silently
  rewrites history.
- **One `Money` newtype in `phonix-core`.** It compiles to wasm, so the totals
  the browser shows and the totals the server posts come out of the same code.

### What shipped, and the two decisions inside it

`phonix_core::money` — `Money`, `Rate`, `ExchangeRate`, `Conversion`, `Rounding`.

**Amounts are `i128` at a fixed scale of four**, which is exactly what
`NUMERIC(19, 4)` holds, so a value that round-trips in Rust round-trips in
Postgres and neither is quietly approximating the other. `i128` and not `i64`
because the column is a digit wider than a 64-bit integer: the maximum scaled
value is 9,999,999,999,999,999,999, and `i64::MAX` is 9.2 × 10^18.

Note that **storage scale and minor units are different numbers.** Everything is
held at four places regardless of currency, because a unit price of 0.0125 is an
ordinary thing and rounding it at the line is how a thousand-unit order comes out
wrong. What the *currency* rounds to is `Currency::minor_units`, and applying it
is an explicit act — `Money::round_to_minor_unit`, the one function in the
workspace that rounds money.

**A conversion returns evidence, not a number.** `Money::convert` yields a
`Conversion` carrying both amounts, both currencies, the rate, the rate's
publication date and the rounding mode used. There is no constructor that takes a
base amount, so a snapshot whose base amount does not follow from its own rate
cannot be built — the six-column rule is enforced by the type rather than by
remembering.

Two smaller consequences worth writing down:

- **`Money` is not `Ord`, `Add` or `Sum`.** Each would have to answer what
  happens when the currencies differ, and the only honest answer is a `Result`.
- **Amounts and rates cross the wire and the driver as strings.** A JSON number
  is an IEEE double in most parsers, and `NUMERIC` has no lossless integer
  binding in sqlx — so both would undo the exactness in transit, silently,
  between a browser that got it right and a server that got it right.


## 4. Taxes — `master`

A tax code carries jurisdiction, effective dates and posting consequences. That is
business opinion, and a DICOM viewer never needs it — so taxes are the first
citizen of the `master` app, alongside parties, units of measure and payment
terms.

The design has to survive, without a schema change: EU/UK VAT, Indian GST split
into CGST/SGST/IGST, Canadian GST/PST/HST, US destination sales tax, withholding
tax, compound tax, and tax-inclusive pricing.

```sql
CREATE TABLE master.tax_codes (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code           TEXT NOT NULL,
    name           TEXT NOT NULL,
    kind           TEXT NOT NULL,
    country_code   CHAR(2),
    region_code    TEXT,
    is_compound    BOOLEAN NOT NULL DEFAULT FALSE,
    is_recoverable BOOLEAN NOT NULL DEFAULT TRUE,
    is_active      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tax_codes_kind_valid
        CHECK (kind IN ('vat', 'gst', 'sales', 'withholding', 'excise'))
);

CREATE UNIQUE INDEX tax_codes_code_key ON master.tax_codes (lower(code));
```

`is_compound` means the tax is computed on the base *plus the taxes before it in
sequence*. `is_recoverable` is what separates reclaimable input VAT from a cost.

### Rates are effective-dated, and Postgres enforces it

```sql
CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TABLE master.tax_rates (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tax_code_id UUID NOT NULL REFERENCES master.tax_codes (id) ON DELETE CASCADE,
    rate        NUMERIC(9, 6) NOT NULL CHECK (rate >= 0),
    valid_from  DATE NOT NULL,
    valid_to    DATE,
    CONSTRAINT tax_rates_no_overlap EXCLUDE USING gist (
        tax_code_id WITH =,
        daterange(valid_from, COALESCE(valid_to, 'infinity'::date), '[)') WITH &&
    )
);
```

The exclusion constraint makes two simultaneously-live rates for one code
impossible at the database level — the kind of error nobody notices until a
quarter has been filed.

### A line references a *group*, never a code

```sql
CREATE TABLE master.tax_groups (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code         TEXT NOT NULL,
    name         TEXT NOT NULL,
    country_code CHAR(2),
    is_active    BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE master.tax_group_members (
    tax_group_id UUID NOT NULL REFERENCES master.tax_groups (id) ON DELETE CASCADE,
    tax_code_id  UUID NOT NULL REFERENCES master.tax_codes (id),
    sequence     SMALLINT NOT NULL,
    PRIMARY KEY (tax_group_id, tax_code_id)
);
```

"VAT 20%" is a group with one member. "GST 18%" is a group with CGST 9% and
SGST 9%. Quebec's compound arrangement is a group with two members and
`is_compound` on the second. This is the single decision that makes the model
work in India and Canada without a migration, and `sequence` is what makes
compound ordering deterministic.

### The snapshot rule

A document line stores **resolved values**, not just the group id:

```
tax_group_id, tax_code_id, tax_code, tax_name, rate, is_compound, sequence,
taxable_amount, tax_amount
```

Rates change. A reprinted 2024 invoice must show 2024's rate and 2024's name.
This is the same discipline as the `entity_events` from/to shape — record what
was true, not a pointer to what is true now.

### Computation is a pure function

A `phonix-tax` crate with no I/O: lines in, per-line and per-tax totals out. It
compiles to wasm, so the browser previews the exact figures the server will post —
no round trip, and no possibility of the two disagreeing.

Two policies belong on the document, stored, not inferred:

- **Rounding level** — per line, or per document total.
- **Rounding mode** — half-up or half-even.

Reconciliation disputes come from these two being implicit. Tax-inclusive pricing
is a third flag on the document: net is derived as `gross / (1 + Σ rates)` with the
compound ordering respected.

### Revised: what `master` turned out to be

Step 5 is built. Four things about it were decided while implementing and are
not in the design above.

**Parties are one table, and a role is a row.** The first draft said "parties"
and left it there. The question implementing it asks is: what makes a party a
*customer*? Not a column - a company that buys from you and also delivers for
you is one organization, and two tables would mean two addresses to keep in
step, two tax registrations, and a document that cannot say the two are one
party. Not a `kind` either, because that forces a choice which is not exclusive
in real trade.

So `master.party_roles` is one row per claim, with an **open** vocabulary:
Books marks a party `customer`, Procurement marks the same party `supplier`,
and neither has to know the other exists. A pair of booleans would have needed
a migration in `master` every time an app started using parties - and `master`
cannot depend on the apps above it, which is the whole point of the layering.
The column carries a shape `CHECK` and no list, because a list would be exactly
that migration.

`PartyKind` stays, and is organization or person. That is a fact about the
party rather than a claim about it: it decides whether "legal name" means
anything and which name a document prints.

**An address on a document is a copy.** `PostalAddress` is a value with no id;
`PartyAddress` is the record a screen edits. A customer who moves next year
must not silently rewrite last year's invoices - the same rule the tax snapshot
follows, and the same rule `entity_events` follows.

**At most one primary address per purpose is kept by the service, not by a
partial unique index.** An index would refuse the save the moment somebody
ticked the new one before unticking the old, which is the order everybody does
it in.

**`TEXT` with a `CHECK`, never `CHAR(n)`.** The tax design above wrote
`country_code CHAR(2)`. That is not what shipped, and section 3 says why:
`CHAR` is blank-padded, and a padded value compares equal in SQL and unequal in
Rust. Migration 0010 had already made the same call for `currency_code`.

### The tax model, and the three things Postgres enforces

Built as designed - codes, effective-dated rates, groups, and the compound
`sequence` - with three constraints carrying weight that no Rust test can:

1. **`tax_rates_no_overlap`**, a GiST exclusion constraint over
   `(tax_code_id, daterange(valid_from, valid_to, '[)'))`. Two simultaneously
   live rates for one code become impossible at the database level. The
   repository maps the violation to `DbError::TaxRateOverlap` rather than
   checking first: a check-then-insert is a race, and the race is two
   administrators filing the same rate change on the same afternoon.
2. **The window is half-open.** The day a rate changes belongs to exactly one
   row. A closed range is how somebody eventually writes yesterday's end date
   as today's start.
3. **`tax_group_members.tax_code_id` has no cascade.** A tax inside a group
   cannot be deleted out from under it, because a group that lost a member
   silently would change what every document using it comes to. Retiring a tax
   is `is_active = false`.

`btree_gist` is pinned `WITH SCHEMA public`: with `master` first on the
migration search path it would otherwise be created inside `master`, and
uninstalling the app would take the extension with it.

### `phonix-tax`: what the arithmetic decided

The design said "lines in, per-line and per-tax totals out". Four decisions the
implementation had to make that the design did not:

- **Compound is a factor chain, and the factors do not depend on the amount.**
  Each tax contributes `base × rate`, where `base` is `1` normally and
  `1 + everything accumulated so far` when compound. That is what makes
  tax-inclusive pricing a *division* rather than a search: the net is
  `gross / (1 + accumulated)`, with the compound ordering already inside the
  accumulation.
- **Document-level rounding uses a running total, not proportional
  allocation.** Each line gets the difference between the correctly rounded
  running total and what has already been given out. It preserves the document
  total by construction and does the right thing on a document that mixes
  positive lines with a negative discount line - where proportional weights do
  not. Whichever level is used, **the lines add up to the total exactly**; a
  document whose lines do not sum to its own footer is one nobody can check.
- **Under inclusive pricing the residual goes to the net, never to the tax.**
  The gross is the given - it is the price that was quoted - so something has to
  absorb the rounding. The tax is what gets remitted and filed, and moving a
  cent into it to make a subtraction work is moving a cent of somebody else's
  money.
- **A rate is a proportion; only one function takes a percentage.**
  `TaxRate::parse_percent` is the single door the word *percent* comes through,
  because the two readings of "18" differ by a factor of a hundred and a caller
  that has to remember which one it holds will eventually forget. A percentage
  carries four meaningful decimal places, not six: dividing by a hundred moves
  the point twice.

`Money::scale_by` was added to `phonix-core` for this: multiply by a ratio of
two integers, rounding **once**. It is the primitive behind applying a rate, and
`Money::convert` had been doing the same thing by hand.

### The one place the two layers meet

`phonix_services::master::tax::treatment_on` turns a group and a date into the
snapshot a document keeps. It does no arithmetic; `phonix_tax::compute` does no
I/O. That is the whole seam, and it is what lets the browser preview the figures
the server will post.

It is deliberately **ungated**. It is not a screen - it is what an app calls
while pricing a document, and the app has already checked that the caller may
raise one. Requiring `Master.Taxes` here would mean everybody who can write an
invoice must also be allowed to edit the tax tables.

---

## 5. Document numbers — `core`

Requirements, in order of how much trouble they cause when missed:

- **Gap-free** where the law requires it — Italy, Spain, Portugal, Poland, India
  GST, most of Latin America. A missing invoice number is an audit finding.
- Formats with date tokens and periodic resets.
- Per-scope numbering — branch, location, fiscal year.
- Concurrency-safe under real load.

```sql
CREATE TABLE core.number_sequences (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id        TEXT NOT NULL,
    doc_type      TEXT NOT NULL,
    scope_key     TEXT NOT NULL DEFAULT '',
    pattern       TEXT NOT NULL,
    reset_period  TEXT NOT NULL DEFAULT 'never',
    period_key    TEXT NOT NULL DEFAULT '',
    counter       BIGINT NOT NULL DEFAULT 0,
    start_at      BIGINT NOT NULL DEFAULT 1,
    is_active     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by    UUID REFERENCES core.users (id) ON DELETE SET NULL,
    CONSTRAINT number_sequences_reset_valid
        CHECK (reset_period IN ('never', 'daily', 'monthly', 'yearly', 'fiscal_year')),
    CONSTRAINT number_sequences_pattern_shape
        CHECK (char_length(pattern) BETWEEN 1 AND 60 AND pattern ~ '[{]N+[}]')
);

CREATE UNIQUE INDEX number_sequences_key
    ON core.number_sequences (app_id, doc_type, scope_key);
```

`core` does not know what a `doc_type` is. Each app declares its document types
and default patterns; installing the app inserts the rows; the tenant edits them
on a `core` numbering settings screen — the screen is about counters, not about
invoices. The table therefore ships **empty**: there are no apps yet, and a
seeded row for a document type nothing issues is a row that drifts.

`app_id` is deliberately **not** a foreign key to `installed_apps`. Uninstalling
an app must not cascade away the numbering history of documents it already
issued; those numbers still have to be explicable afterwards.

`scope_key` is `''` and not `NULL` for "one sequence for the workspace", because
it is part of a unique index and `NULL` does not compare equal to itself — two
unscoped sequences for one `doc_type` would both be allowed.

### Revised: `mode` is not built

The first draft carried a `mode` column with `'strict'` and `'fast'`, where
`'fast'` used a real sequence for documents whose gaps are harmless. It is not
built, because it needs a Postgres `SEQUENCE` created per row at runtime, and a
column whose only legal value is `'strict'` is a column that says nothing.
Adding it later is `ALTER TABLE ... ADD COLUMN ... DEFAULT`, which is additive
and cheap. A value nothing implements is not.

### Allocation is one statement

```sql
UPDATE core.number_sequences
   SET counter    = CASE
                        WHEN period_key = $4 AND counter >= start_at
                        THEN counter + 1
                        ELSE start_at
                    END,
       period_key = $4
 WHERE app_id = $1 AND doc_type = $2 AND scope_key = $3 AND is_active
RETURNING counter, pattern;
```

The period reset is inside the same `CASE`, so a year boundary can never
interleave with an allocation, and nothing has to run at midnight for the first
document of January to be number one.

**Revised: the `counter >= start_at` arm.** The first draft had two branches —
`counter + 1` or `start_at` — and that is wrong in two ways that only show up in
use. A fresh row has `counter = 0` and, under `reset_period = 'never'`, a
`period_key` that already matches; a sequence configured to begin at 5000 would
therefore issue 1. And raising `start_at` past numbers already issued, which is
the *supported* way to move a sequence on, would do nothing at all. The third
arm handles both, and both are covered by tests.

Note which direction is dangerous. **Raising** `start_at` moves the sequence on.
**Lowering** it changes nothing while a period is running, because the counter is
already ahead — it only decides where the next period opens.

**Why not a Postgres `SEQUENCE`.** `nextval()` is deliberately non-transactional:
a rollback burns the number. That is exactly right for surrogate keys and
unlawful for invoices. The `UPDATE` takes a row lock held until commit, so a
rollback *returns* the number and concurrent allocators queue behind it.

**Gap-free is bought with serialization on that one row.** Every document of one
type queues through one row. That is the trade, and for an invoice it is the
correct one: the law wants an unbroken sequence, and an unbroken sequence is
inherently serial.

### Four rules that matter more than the schema

1. **Allocate inside the document's own transaction.** Same transaction as the
   `INSERT`. A failed save then returns the number automatically, and a retry
   cannot burn one. This is enforced as far as a signature can: `allocate` takes
   `&mut PgConnection`, not the `PgExecutor` every other repository takes, so it
   cannot be handed a pool — where each statement is its own transaction and the
   lock would be released the moment the `UPDATE` returned.
2. **Never number a draft.** Allocate at confirm/post, never at create. A
   discarded draft would otherwise leave a permanent gap — precisely what an
   auditor asks about. Drafts display their UUID, or the word Draft.
3. **Never show a document the number it is *going* to get.** Promising before
   commit promises something that may not be kept. Previewing a *pattern*
   against a sample counter is a different act and entirely safe —
   `Pattern::preview`, which is why it is a separate function.
4. **Belt and braces.** A unique index on `(doc_type, number)` in the app's own
   table. It cannot live in `core`, which holds no documents. Administrators edit
   sequences; that index is what stops a duplicate reaching the ledger.

### Pattern tokens

```
{YYYY}  {YY}  {MM}  {DD}  {FY}  {SCOPE}  {NNNN…}  #
```

The run of `N`s is the zero-padded counter width: `INV-{YYYY}-{NNNNNN}` renders
`INV-2026-000042`. `{FY}` reads `organization_profile.fiscal_year_start_month`,
which already exists. Upper case exactly as written — `{nnnn}` is refused rather
than treated as a counter, because the alternative is a pattern that looks right
in a settings box and prints `{nnnn}` on an invoice.

Rendering is a pure function in `phonix_core::numbering`, so it compiles to wasm
and the settings screen previews `INV-2026-000042` with no round trip.

Three decisions the renderer had to make that the design did not:

- **A fiscal year is named by the calendar year it opens.** April 2026 to March
  2027 is "2026/27" in the UK, "FY2027" to the US federal government and
  "FY 2026-27" in India — so it is a choice, not a fact. Naming it by the opening
  year is the one convention that degrades gracefully: the default profile opens
  in January, and for a January year `{FY}` is then exactly `{YYYY}`. Naming it
  by the closing year would hand a January-start organization next year's number
  on every document, which is indistinguishable from a bug.
- **A fiscal-year period key carries an `FY` prefix.** Without it a January
  fiscal year produces the same key as a calendar one, and an administrator
  switching between the two mid-year would leave a stored key that accidentally
  matches — so the counter would carry on instead of resetting.
- **A counter past its padding widens the number** rather than being refused or
  truncated. `INV-{NNNN}` issues `INV-10000` on the ten-thousandth document.
  Ugly; truncating would issue a duplicate and refusing would stop the business
  invoicing, and neither of those is better than ugly.

**Guard the edit.** Changing `pattern` or `reset_period` mid-period can collide
with numbers already issued — changing the period alone opens a new period, which
restarts the counter at `start_at`. The settings service must either force
`start_at` past the highest issued value, or refuse. The repository does as it is
told; that judgement is the service's.

### Revised: the screen, and the one refusal worth rendering

`Administration → Settings → Numbering`. A grid of every series with the counter
it has reached, and a panel that edits the format, the reset period and
`start_at`.

Three things it deliberately does not offer:

- **No "new series" button.** A series belongs to a document type, and document
  types come from `config/numbering/<app_id>.toml`. One created by hand is one
  no app ever asks for.
- **No delete.** A number already issued has to stay explicable afterwards.
- **No editable counter.** It is the series' own record of what it has handed
  out, and a box somebody can type into is a number issued twice. Moving a
  series on is `start_at`, which the allocation honours the next time it runs.

The preview is rendered **on the server**, through `NumberGenerator`, for the
reason the generator carries `preview` at all: `{FY}` reads the organization's
fiscal year opening, and a browser guessing at that would show a format that is
right and a number that is wrong.

`SettingsOutcome::WouldReissue` reaches the screen as
`SeriesSaved::WouldReissue { issued }` and is drawn as an inline notice rather
than an error, carrying the number. That is the whole reason the outcome holds
it: "this series has already issued 41, so raise Starts at above that" is an
instruction, and "the format cannot be changed" is not.

---


### Revised: `#` is the mask, and the defaults live in a file

Two changes to the above, both from the same request: numbers should come out of
a *generator*, and how a series looks should be *configuration*.

**`#` is one counter digit.** `{NNNN}` still works and means the same thing, but
`#` is the spelling every other system in this space uses, and it is what makes
a grouped reference number readable in a settings box: `#-#####-####` says what
it is at a glance where `{NNNNNNNNNN}` says only that it is ten of something.

Every slot in a mask belongs to the **same** counter, filled right to left and
zero-padded, with the literals between the groups kept:

```
#-#####-####      counter        42  ->  0-00000-0042
                  counter   123_456  ->  0-00012-3456
INV-{YYYY}-#####  counter        42  ->  INV-2026-00042
```

That has three consequences worth writing down.

- **`ManyCounters` is gone.** The first draft refused `{NNN}-{NNN}` because "the
  number would contain itself twice". Under one shared field it has a reading,
  and it is the same reading `###-###` has, so it is now allowed.
- **Mixing the two spellings is refused** — a new `MixedCounters`. Not because
  it could not be given a meaning, but because `INV #{NNNNN}` reads as a hash
  followed by a five-digit counter and would render a *six*-digit one. A pattern
  that looks right in a settings box and prints something else on the document
  is the exact failure this type exists to prevent. Migration `0018` puts the
  same rule in the column, as an exclusive-or over the two spellings.
- **Overflow widens the leftmost group.** `#-#####-####` at ten billion is
  `10-00000-0000`, not a smear across all three. The right-hand groups are the
  ones a reader has learned the shape of.

**An app declares its series in `config/numbering/<app_id>.toml`.**

```toml
[[series]]
doc_type = "sales_invoice"
label    = "books.doc_type.invoice"   # an i18n key, not a phrase
mask     = "INV-{YYYY}-#####"
reset    = "fiscal_year"
start_at = 1
```

A file, not a table, because of who owns what. The app owns the *question* —
which documents it issues and what they should look like out of the box. The
tenant owns the *answer*: once installed, the rows are theirs, and `install`
uses `ON CONFLICT DO NOTHING` so a redeploy never puts back a format they
changed. A default is what a workspace starts with, not what it is held to — and
a default in a file gets reviewed in a pull request.

Everything in it is validated at load: the mask parses, `doc_type` fits the
column, `label` is a key the catalog actually holds, `start_at >= 1`, and no
document type is declared twice for one scope (the second would be dropped
silently by `DO NOTHING`). Unknown keys are refused too, so `rest = "yearly"` is
a typo that stops a deployment rather than a reset period that silently never
happens. A format error should stop a deploy, where somebody is watching — not
the first invoice, in front of a customer.

The table therefore still ships **empty**, and `config/numbering/` holds only a
README: `core` issues no numbered documents, and the first file arrives with the
first app.

### The generator

`phonix_services::numbering::NumberGenerator` is the third piece.

It exists because every number needs the organization's fiscal year opening,
which lives in `organization_profile` and changes about once. Reading it per
document is a query per document to answer a question whose answer is the same
all day. `NumberGenerator::open` reads it once and is held for a request, a
batch, or a worker's lifetime.

It also carries `preview`, and that is the point of putting both on one type: a
settings screen that resolved the fiscal year differently from the posting path
would show a format that is right and a number that is wrong. Sharing the
generator makes them provably agree, and a unit test asserts it across an April
year boundary.

`next` takes `&mut PgConnection` for the reason `allocate` does, and says so.

`save_settings` is the guard the repository documentation asks for, and it lives
here because it is a judgement. A workspace that has posted `INV-2026-000041`
and then narrows its pattern to `INV-####` will reissue `0042` in a shape that
no longer distinguishes it from last year's — two documents, one number, and no
constraint in `core` that can see it, because `core` holds no documents. So a
format change on a sequence that has already issued is refused unless `start_at`
is raised past the last counter handed out. It comes back as
`SettingsOutcome::WouldReissue { issued }` rather than an error, the same way a
wrong password does: it is an expected path through a form, and the screen needs
the number in order to offer the fix.

---

## 6. Getting there from `public`

Migration streams live one directory per app, core included:

```
migrations/
  catalog/            the shared registry
  apps/core/          0001 … 0014
  apps/books/         later
```

`migrations/apps/core/0014_core_schema.sql` creates the schema and relocates the
seventeen tables, the `set_updated_at` trigger function, and sqlx's own
`_sqlx_migrations`. `SET SCHEMA` carries indexes, constraints and column-owned
sequences along and leaves the OID alone, so triggers keep firing across the
move. It is idempotent: the runner creates `core` *before* the first migration
runs, so a database provisioned today builds straight into `core` and 0014 finds
nothing to relocate.

### The search path — revised

The first draft of this record said *fully qualify every query, do not lean on
`search_path`*. That was wrong, for two reasons found while implementing it:

1. **pgcrypto is installed in `public`.** `public` has to stay on the path
   regardless, or `gen_random_uuid()` stops resolving. The choice was never
   between a search path and none.
2. **sqlx names its bookkeeping table unqualified.** Running each app's stream
   on a path rooted at that app's schema is what puts `books._sqlx_migrations`
   inside `books` — which is how the streams become independent without sqlx
   needing to know that apps exist. Qualification cannot buy that.

So: **tenant connections open on `core,public`**, and the ~130 existing
infrastructure queries stay as they are. The original worry — a query silently
reading the wrong table — needed two schemas on the path holding the same table
name, and after 0014 `public` holds no tables at all.

**App schemas are deliberately absent from the path, and that absence is the
rule.** An app's tables are always written `books.invoices`. A schema that is not
on the path cannot be reached by a query that forgot which app it meant, so the
mistake is a loud error rather than a quiet wrong answer. The same goes for an
app migration referencing core: `core.users`, never `users`.

### Version marking

The catalog's `schema_version` now holds a fingerprint across every app —
`core:0014`, later `core:0014,books:0003` — because the boot sweep skips any
tenant whose marker already matches. A marker that tracked core alone would let
an app gain a migration and never reach the tenants that need it.

---

## 7. Order of work

1. ~~`0014_core_schema.sql` — move the seventeen tables.~~ **Done.**
2. ~~`core.installed_apps` + per-app migration streams in the runner.~~ **Done.**
3. ~~`core.currencies`, `core.exchange_rates`, the `Money` newtype.~~ **Done.**
4. ~~`core.number_sequences` + the pattern renderer.~~ **Done.**
   The `#` mask, `config/numbering/<app>.toml` and `NumberGenerator` followed;
   see the two revisions in section 5.
5. ~~The `master` app: parties, tax codes, rates, groups, and `phonix-tax`.~~
   **Done.** Two crates - `phonix-tax` for the arithmetic, `phonix-master` for
   the vocabulary - plus `migrations/apps/master/0001`-`0002`, the repositories,
   the services, and the screens under `/master`. See the revisions in section 4.
6. ~~The first real app.~~ **Done, on `app/books`.** Sales invoices:
   `crates/app-books` for the vocabulary and the pricing,
   `migrations/apps/books/0001`, `config/numbering/books.toml`, the
   repository, the service, and the screens under `/sales`. See section 8.

Steps 1 and 2 got more expensive with every tenant added, which is why they went
first.

The live tests are `#[ignore]`d, because they need a reachable server. Run them
deliberately:

```
cargo test -p phonix-db --test tenant_schema -- --ignored --test-threads=1
cargo test -p phonix-db --test currency      -- --ignored --test-threads=1
cargo test -p phonix-db --test numbering     -- --ignored --test-threads=1
cargo test -p phonix-db --test master        -- --ignored --test-threads=1
cargo test -p phonix-db --test books         -- --ignored --test-threads=1
```

`tenant_schema` covers both paths through step 1 — a database built today and one
built before 0014. `currency` covers the part of step 3 no unit test can reach:
that a rate crosses `NUMERIC(20, 10)` and comes back with all ten decimal places
still on it. The arithmetic is proved in `phonix-core`; the column is proved
here.

`master` covers the three things only Postgres can prove: that two rates for
one tax can never be live at once, that six decimal places survive
`NUMERIC(9, 6)` in both directions, and that a tax inside a group cannot be
deleted out from under it. It also proves the thing the app mechanism is for -
that `master` needed no special case in the runner.

`numbering` covers the only thing that actually makes a sequence gap-free, and
it is not in any schema: the row lock, and that Postgres holds it until the
transaction ends. Two tests carry the weight — one rolls a transaction back and
finds the number still available, and one runs two allocations at once and
watches the second wait for the first to commit.

---

## 8. The first app, and what it proved

`books` is one schema, one migration, one crate and one config file. Nothing in
the runner knows it exists beyond an entry in `APPS`, which is the result the
first seven sections were for.

### The prefix says which side of the boundary a crate is on

```
phonix-*   infrastructure. Every app may depend on it. Cannot be uninstalled.
app-*      a product. Installs into a workspace; a build can leave it out.
```

`app` and not some other word because it is the one the rest of the codebase
already uses for this - `migrations/apps/`, `core.installed_apps`, `app_id`,
`AppMigrations`. Two words for one idea eventually disagree, which is the same
argument section 2 makes about `app_id` being the schema name.

### What `books` may and may not reference

`core.currencies` and `core.users` are proper foreign keys. `master.parties`
and `master.tax_codes` are referenced **by id, with none** - the rule from
section 1, and the first time it has cost anything. The cost is that `books`
cannot ask the database whether a party still exists, and the payment is that
`DROP SCHEMA books CASCADE` remains a safe thing to do.

That absence is also why the columns beside those ids are a *snapshot*. A
document stores the party's registered name and address, and every line stores
the code, name and rate of each tax that applied. None of it is re-resolved at
print time, so a customer who moves and a rate that changes cannot rewrite an
invoice that was already sent.

### Posting is the act the whole numbering design was for

A draft carries no number. Posting allocates one **in the same transaction as
the write**, which is what makes a failed post *return* the number rather than
burn it - proved by a live test that rolls one back and finds the same number
still waiting. A voided invoice keeps its number, because a number that
disappears is a gap and a gap is what an auditor asks about.

The belt-and-braces index from section 5 is real now: `invoices_number_key`,
partial on `number IS NOT NULL`, because every draft has a NULL and NULLs do
not compare equal.

### The browser prices the document

`app_books::pricing` compiles to wasm. The editor re-prices the whole invoice
locally on every keystroke - compound chains, inclusive pricing,
document-level rounding - and the server runs *the same function* over the same
treatments when it saves. There is no "calculate totals" endpoint, deliberately:
that would be a second implementation of the arithmetic living in the network,
and the first thing to disagree with the document.

The one thing the browser cannot know is which taxes apply on the document's
date, because that needs a rate table. It fetches those once, and re-fetches
only when the date changes.

### Two things the app decided that the boundary did not

- **A `Quantity` lives in `app-books`, not in `core`.** Only one app needs it.
  Section 1 says to wait for the third, and promoting it later is a re-export.
- **Books claims the party as a customer through the repository**, not through
  `master::party::claim_role`, which requires `Parties.Edit`. Being allowed to
  raise an invoice against somebody *is* the authority to mark them a customer;
  requiring the master-data permission as well would mean nobody in sales could
  invoice anyone. It is also the only way `master` can learn the party is in
  use, since it has no foreign key into `books`.


## 8. Installing an app, and the one that turned out not to be one

Step 6 raised a question the design above does not answer: what does it mean
for a workspace to *have* an app? Every app compiled into the build had its
schema migrated into every tenant database, and `installed_apps.state` was read
by nothing. A boundary that lets you leave an app out of a build but not out of
a workspace is only half a boundary, and the missing half is the one with a
subscription attached to it.

**Installing is enabling, and enabling is a permission grant.** The schema is
already there - migrating under a live request is a fault, not a feature - so
an install writes `installed_apps.enabled_at` and re-syncs the static roles.
Every gate downstream already answers to permissions: the menu, the command
palette, the grids, `Caller::require` in every service. Granting the subtree
beneath an app's permission root switches all of them on at once, and there is
deliberately no second mechanism to keep in step with that one.

The rule it imposes: **an app owns a whole permission subtree and nothing
outside it.** Revocation is a prefix match on a dotted boundary, so two apps
sharing a parent would take each other down. Tests refuse a catalog that
overlaps, and refuse a permission no app owns.

### `master` is not an app you can decline

The registry above called it "an ordinary app, always installed for commercial
products, absent from a pure clinical one". Implementing the store showed that
sentence to be two arguments wearing one coat.

Whether a *build* contains `phonix-master` is a composition question, and the
answer there stands: a clinical product would not compile it in, and nothing in
`core` would notice. Whether a *workspace of a running deployment* subscribes to
its own customer list is a different question, and the answer is no - it is not
a thing anybody would sell.

The technical reason is the one that settles it. Master data is what the other
apps *reference*: an invoice names a party and a tax group, a purchase order
would name a supplier, a CRM would name both, and none of them holds a foreign
key to say so. Make it switchable and every app that reads it has to answer
"what if the thing I point at is off" - which in practice means every app
declaring `requires: ["master"]`, which is always-on again with a dependency
graph to maintain on top.

So `master` is `always_on`, alongside `core`, and keeps everything else: its own
schema, its own migration stream, no foreign key reaching out of it. The
boundary is worth keeping whether or not anybody can switch the thing off - it
is what lets an app be *added*, which is the direction that actually happens.

### Two lists that cannot be one

`phonix_db::tenancy::apps::APPS` holds `sqlx::Migrator` and can never compile to
wasm. `phonix_core::apps::CATALOG` holds the name, summary, icon, version and
home, and must, because the browser draws the store. A test asserts they name
the same apps in the same order; the drift is quiet and expensive both ways -
an app in the registry alone gets a schema nobody can switch on, and one in the
catalog alone is offered, installed, and fails on its first query.

An app declares its **home** rather than deriving it from its permission root.
That was derived once and produced `/pages` for core, because core's root *is*
the tree's root. A test now checks the two agree instead, which keeps the
property without the failure.
