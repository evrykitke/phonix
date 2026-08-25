# Document number series

One file per app, named after its `app_id`: `books.toml` declares what the
`books` app numbers. An app with no numbered documents needs no file, and a
missing file is not an error.

These are **defaults**, not settings. Installing an app inserts the rows into
`core.number_sequences`; from that moment the tenant owns them, and a redeploy
never puts back a format they changed. A default is what a workspace starts
with, not what it is held to — which is exactly why this is a file that can be
reviewed in a pull request rather than a table somebody edits in production.

```toml
[[series]]
doc_type = "sales_invoice"          # required
label    = "books.doc_type.invoice" # optional, an i18n key
mask     = "INV-{YYYY}-#####"       # required
reset    = "fiscal_year"            # optional, default "never"
start_at = 1                        # optional, default 1
scope    = ""                       # optional, default "" — see below
```

## The mask

`#` is one counter digit. Every `#` in a mask belongs to the **same** counter,
filled right to left and zero-padded, with whatever lies between the groups kept
verbatim:

| Mask               | Counter 42        | Counter 123 456   |
| ------------------ | ----------------- | ----------------- |
| `#####`            | `00042`           | `123456`          |
| `INV-#####`        | `INV-00042`       | `INV-123456`      |
| `#-#####-####`     | `0-00000-0042`    | `0-00012-3456`    |
| `INV-{YYYY}-#####` | `INV-2026-00042`  | `INV-2026-123456` |

A counter past its padding **widens** the number rather than being refused or
truncated — truncating would issue a duplicate, and refusing would stop the
business invoicing. In a grouped mask the leftmost run is the one that widens.

Other placeholders go in curly brackets, upper case exactly as written:

| Token     | Renders                                                     |
| --------- | ----------------------------------------------------------- |
| `{YYYY}`  | Calendar year, four digits                                  |
| `{YY}`    | Calendar year, two digits                                   |
| `{MM}`    | Month, zero-padded                                          |
| `{DD}`    | Day, zero-padded                                            |
| `{FY}`    | Financial year, named by the calendar year it **opens**     |
| `{SCOPE}` | This sequence's `scope`                                     |
| `{N...}`  | Counter digits — the older spelling of `#`, one `N` per slot |

`{N...}` and `#` mean the same thing, and a mask may use one or the other but
never both. `INV #{NNNNN}` reads as a hash followed by a five-digit counter and
would render a six-digit one, so it is refused rather than guessed at.

Everything here is validated when the file is read, at startup. A format typo
should stop a deployment, where somebody is watching — not the first invoice, in
front of a customer.

## `reset`

When the counter goes back to `start_at`. One of:

`never` · `daily` · `monthly` · `yearly` · `fiscal_year`

The reset happens as part of the allocation itself, by comparing the period the
sequence last issued into against the period the document falls in. Nothing runs
at midnight, and a year boundary cannot interleave with a document being posted.

`fiscal_year` follows `organization_profile.fiscal_year_start_month`, which each
workspace sets for itself. A year is named by the calendar year it opens, so a
year running April 2026 to March 2027 is `2026`.

## `scope`

A branch, a till, a warehouse: a short code (letters, digits, `-`, `_`) that
gives that location its own counter. Leave it empty — the usual case — for one
sequence across the whole workspace.

Declaring the same `doc_type` twice under different scopes is how an app ships
per-location numbering. Declaring it twice under the *same* scope is refused,
because the second would be silently dropped on install.

## What is not here

`core` itself issues no numbered documents, so there is no `core.toml`. The
first file in this directory will arrive with the first app.
