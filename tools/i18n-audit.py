#!/usr/bin/env python3
"""How much of the interface is still written in English at the point of use.

    python tools/i18n-audit.py            # a file-by-file scoreboard
    python tools/i18n-audit.py --list     # every remaining literal, with its line

The rule this measures is the one in `phonix_core::i18n`: no text that reaches a
person is written where it is used. A screen names a key and the catalog decides
the words.

This is a heuristic, not a linter. It reads `view!` blocks and reports quoted
strings that look like something a person would read. It exists to make the
remaining work countable and to stop the number going up, not to be exactly
right - it will still miss a sentence assembled by `format!` outside a view.

What it deliberately does not count, because none of it is language:

  * Tailwind class strings, including the `format!` templates that build them.
    Half of every view is `"flex items-center gap-2 {state}"`, and a scoreboard
    that counts those is a scoreboard nobody reads.
  * `"{} | Phonix"` and the bare product name. A brand is not translated, and
    the pipe is punctuation.
  * Key caps - `"Esc"`, `"Ctrl K"` - and the DOM key names an event handler
    compares against, which are `KeyboardEvent.key` values rather than words.
  * Anything inside `logging::` or `tracing::`, which is written for whoever is
    reading the server's output and not for the person using the screen.

Each of those was a real judgement made while sweeping the interface, so each
is recorded here rather than re-argued the next time the number moves.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

WEB = Path(__file__).resolve().parent.parent / "crates" / "phonix-web" / "src"

# Attributes whose values are machinery: selectors, routes, form wiring. A
# string in one of these is not read by anybody.
MACHINERY = {
    "class", "id", "href", "for_id", "name", "value", "type", "input_type",
    "autocomplete", "src", "rel", "role", "target", "icon", "size", "tone",
    "field", "key", "path", "content", "src_set", "style", "attr:class",
    "attr:id", "attr:href", "attr:type", "attr:rel", "attr:target",
}

# `l!("...")`, `msg!("...")`, `pmsg!("...")` - already keyed, and their first
# argument is a key rather than a sentence.
KEYED = re.compile(r'\b(?:l|lp|msg|pmsg)!\s*\(\s*"[^"]*"')

ATTRIBUTE = re.compile(r'([A-Za-z_][\w:]*)\s*=\s*"([^"]*)"')
EMPTY_ATTRIBUTE = re.compile(r'[A-Za-z_][\w:-]*\s*=\s*""')
LITERAL = re.compile(r'"([^"\\]{2,}?)"')

# Something a person reads: contains a space, or is a capitalised word.
READABLE = re.compile(r'^(?=.*[A-Za-z])(?:.*\s.*|[A-Z][a-z]+[.!?]?)$')

# A line whose strings are for the server's log, not for a screen.
LOGGING = re.compile(r'\b(?:logging|tracing)::')

# The vocabulary Tailwind is written in. A string built only from these is a
# class list however it was assembled - including the `format!` templates that
# splice a `{state}` into the middle of one.
TAILWIND = re.compile(
    r"^[\s{}]*(?:"
    r"(?:hover|focus|focus-visible|active|disabled|group-hover|peer-focus|"
    r"first|last|odd|even|sm|md|lg|xl|2xl|dark|rtl|ltr|print|motion-safe|"
    r"motion-reduce|aria-[a-z-]+|data-\[[^\]]+\])::?)*"
    r"-?(?:flex|grid|inline|block|hidden|absolute|relative|fixed|sticky|"
    r"items|justify|self|place|content|gap|space|w|h|min|max|size|p|px|py|pt|"
    r"pb|ps|pe|pl|pr|m|mx|my|mt|mb|ms|me|ml|mr|text|font|leading|tracking|"
    r"truncate|truncate-fade|break|whitespace|bg|border|rounded|ring|shadow|"
    r"opacity|transition|duration|ease|animate|cursor|pointer-events|select|"
    r"overflow|z|inset|top|bottom|start|end|left|right|order|col|row|divide|"
    r"outline|accent|antialiased|shrink|grow|basis|origin|translate|rotate|"
    r"scale|tabular-nums|sr-only|no-underline|underline|uppercase|lowercase|"
    r"capitalize|italic|list|aspect|object|fill|stroke|backdrop|placeholder|"
    r"caret|resize|appearance|table|align|float|clear|columns|container|"
    r"decoration|indent|line-clamp|snap|touch|will-change)"
    r"[\w./%\[\]#(),:-]*"
    r"(?:[\s{}]+|$)"
    r"(?:[\s{}]*[\w./%\[\]#(),:@-]+[\s{}]*)*$"
)

# The product's own name, and the title suffix built from it.
BRAND = re.compile(r'^(?:\{\}\s*\|\s*)?Phonix$')

# What is engraved on a key, and what the DOM calls one. Neither is a word.
KEY_CAPS = {
    "Esc", "Ctrl K", "Enter", "Escape", "Tab", "Backspace", "Delete",
    "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End",
    "PageUp", "PageDown", "Shift", "Alt", "Meta", "Control",
}


def is_language(text: str) -> bool:
    """Whether this string is words rather than machinery."""
    text = text.strip()

    if text in KEY_CAPS or BRAND.match(text):
        return False

    # `"{base} bg-brand"` is a class list whose first token is spliced in. Judge
    # what is written here, which is the rest of it.
    if TAILWIND.match(re.sub(r"^(?:\{[^}]*\}\s*)+", "", text)):
        return False

    # A template with nothing of its own to say - `"{base} {border}"` - is
    # assembling something else, and whatever it assembles is judged there.
    if not re.sub(r"\{[^}]*\}", "", text).strip(" .,:|-"):
        return False

    return bool(READABLE.match(text))


def literals(source: str) -> list[tuple[int, str]]:
    """Readable strings inside `view!` blocks, with line numbers."""
    found: list[tuple[int, str]] = []
    depth = 0
    in_view = False

    for number, line in enumerate(source.splitlines(), start=1):
        stripped = line.strip()

        if stripped.startswith("//") or LOGGING.search(line):
            continue

        if not in_view and "view!" in line:
            in_view = True
            depth = 0

        if not in_view:
            continue

        depth += line.count("{") - line.count("}")

        # Blank out the parts that are not prose before looking for prose.
        # Empty attributes go first: `alt=""` leaves two quotes that would
        # otherwise pair with the *next* attribute's opening one and report the
        # markup between them as a sentence.
        scrubbed = EMPTY_ATTRIBUTE.sub("", line)
        scrubbed = KEYED.sub("", scrubbed)
        scrubbed = ATTRIBUTE.sub(
            lambda m: "" if m.group(1) in MACHINERY else m.group(0), scrubbed
        )

        for text in LITERAL.findall(scrubbed):
            if is_language(text):
                found.append((number, text))

        if depth <= 0 and "view!" not in line:
            in_view = False

    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="show every literal")
    args = parser.parse_args()

    rows = []
    total = 0

    for path in sorted(WEB.rglob("*.rs")):
        found = literals(path.read_text(encoding="utf-8"))
        if not found:
            continue

        rows.append((len(found), path, found))
        total += len(found)

    rows.sort(key=lambda row: -row[0])

    for count, path, found in rows:
        relative = path.relative_to(WEB.parent.parent.parent)
        print(f"{count:5}  {relative}")

        if args.list:
            for number, text in found:
                print(f"         {number:5}  {text}")

    print()

    if total == 0:
        print("No user-facing text is written at the point of use.")
        return 0

    print(f"{total} literals still written at the point of use, "
          f"across {len(rows)} files")

    return 0


if __name__ == "__main__":
    sys.exit(main())
