-- Let a pattern spell its counter with `#`.
--
-- `0016` accepted one spelling: `{NNNN}`. A `#` is the spelling every other
-- system in this space uses for a digit slot, and it is what makes a grouped
-- reference number readable in a settings box: `#-#####-####` says what it is
-- at a glance, where `{NNNNNNNNNN}` says only that it is ten of something.
--
-- Both spellings mean the same thing - one counter, filled right to left across
-- every slot in the pattern, with the literals between the groups kept. The
-- renderer is `phonix_core::numbering::Pattern`, which is where the real
-- validation lives; this constraint is the floor under it.
--
-- The `<>` is exclusive-or on two booleans: a pattern must use one spelling or
-- the other and never both. Mixing them is refused because `INV #{NNNNN}` reads
-- as a hash followed by a five-digit counter and would render a six-digit one,
-- which is a pattern that looks right in a settings box and prints something
-- else on the document.

ALTER TABLE core.number_sequences
    DROP CONSTRAINT IF EXISTS number_sequences_pattern_shape;

ALTER TABLE core.number_sequences
    ADD CONSTRAINT number_sequences_pattern_shape CHECK (
        char_length(pattern) BETWEEN 1 AND 60
        AND (pattern ~ '[{]N+[}]') <> (pattern ~ '#')
    );
