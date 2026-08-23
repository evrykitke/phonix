#!/usr/bin/env python3
"""Add keys to the English catalog and its translations in one step.

The three files have to move together - `en.json` is the source of truth, and a
test refuses the build if `locales/fr.json` or `locales/de.json` is missing a key
it defines. Editing them by hand is three chances to sort something wrongly or
forget one; this does the sorting, keeps the `_comment` header on top, and
groups the output by area so the files stay readable.

Import it from a one-off script:

    from i18n_keys import add
    add({
        "common.back": {"en": "Back", "fr": "Retour", "de": "Zurueck"},
    })

It refuses to overwrite a key that already exists with different words, so a
sweep cannot quietly restate somebody else's sentence.
"""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ENGLISH = ROOT / "crates" / "phonix-core" / "i18n" / "en.json"
LOCALES = ROOT / "locales"

# The languages `Language::ALL` offers besides English. Kept here rather than
# discovered from the directory so that a stray file cannot silently become a
# language, and a missing one is an error rather than a no-op.
TRANSLATIONS = ("de", "fr")


def _load(path: Path) -> dict[str, str]:
    return json.loads(path.read_text(encoding="utf-8"))


def _write(path: Path, catalog: dict[str, str]) -> None:
    """Sorted, with a blank line between areas so the file stays scannable."""
    comment = catalog.get("_comment")
    keys = sorted(key for key in catalog if not key.startswith("_"))

    lines = ["{"]
    if comment is not None:
        lines.append(f"  {json.dumps('_comment')}: {json.dumps(comment, ensure_ascii=False)},")
        lines.append("")

    previous_area = None
    for index, key in enumerate(keys):
        area = key.split(".", 1)[0]
        if previous_area is not None and area != previous_area:
            lines.append("")
        previous_area = area

        comma = "" if index == len(keys) - 1 else ","
        lines.append(
            f"  {json.dumps(key)}: {json.dumps(catalog[key], ensure_ascii=False)}{comma}"
        )

    lines.append("}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def add(entries: dict[str, dict[str, str]]) -> None:
    """`{key: {"en": ..., "fr": ..., "de": ...}}`, every language required."""
    catalogs = {"en": _load(ENGLISH)}
    for code in TRANSLATIONS:
        catalogs[code] = _load(LOCALES / f"{code}.json")

    for key, words in entries.items():
        for code in ("en", *TRANSLATIONS):
            if code not in words:
                raise SystemExit(f"{key} has no {code}")

            existing = catalogs[code].get(key)
            if existing is not None and existing != words[code]:
                raise SystemExit(
                    f"{key} already says {existing!r} in {code}, not {words[code]!r}"
                )

            catalogs[code][key] = words[code]

    _write(ENGLISH, catalogs["en"])
    for code in TRANSLATIONS:
        _write(LOCALES / f"{code}.json", catalogs[code])

    print(f"{len(entries)} keys into en + {' + '.join(TRANSLATIONS)}")


def check() -> int:
    """What the Rust test checks, without waiting for a compile."""
    english = {k: v for k, v in _load(ENGLISH).items() if not k.startswith("_")}
    problems = 0

    for code in TRANSLATIONS:
        catalog = {
            k: v for k, v in _load(LOCALES / f"{code}.json").items()
            if not k.startswith("_")
        }

        for key in sorted(set(english) - set(catalog)):
            print(f"{code}: missing {key}")
            problems += 1
        for key in sorted(set(catalog) - set(english)):
            print(f"{code}: unknown {key}")
            problems += 1
        for key in sorted(set(catalog) & set(english)):
            if _blanks(catalog[key]) != _blanks(english[key]):
                print(f"{code}: {key} fills {_blanks(catalog[key])}, English supplies {_blanks(english[key])}")
                problems += 1

    print(f"{len(english)} keys, {problems} problems")
    return problems


def _blanks(template: str) -> list[str]:
    found, rest = [], template
    while "{" in rest:
        rest = rest[rest.index("{") + 1:]
        if "}" not in rest:
            break
        found.append(rest[:rest.index("}")])
        rest = rest[rest.index("}") + 1:]
    return sorted(found)


if __name__ == "__main__":
    raise SystemExit(1 if check() else 0)
