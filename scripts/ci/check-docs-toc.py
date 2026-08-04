#!/usr/bin/env python3
"""Table-of-contents agreement check for the documentation site.

The `toc` in `docs/myst.yml` is explicit and hand-maintained, not
filesystem-discovered, so the two can disagree in either direction — and
`myst build --html --strict` catches neither:

- A page **on disk but absent from the toc** builds without a single warning
  and is simply missing from the rendered site: no sidebar entry, no nav, no
  search hit. Nothing in the toolchain mentions it.
- A toc entry **naming a file that does not exist** prints
  `⛔️ Table of contents entry does not exist: …` and then exits **0**, so the
  `docs` job stays green and the page silently vanishes from the site.

Measured against mystmd 1.10.1, both directions; neither is a build failure.
This script is what makes them one. It asserts the toc and `docs/**/*.md`
describe exactly the same set of pages, each listed once.

Usage: python3 scripts/ci/check-docs-toc.py
Requires: python3 only (standard library).
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
DOCS = REPO / "docs"
MYST = DOCS / "myst.yml"

# Directories under `docs/` that hold build output or dependencies rather than
# source pages, and so are never expected in the toc.
IGNORED_DIRS = frozenset({"_build", "node_modules"})

# `- file: architecture/zfp.md`, at any nesting depth, with an optional inline
# `# comment`. mystmd also accepts `url:` and `title:`-only branch nodes; only
# `file:` entries name a page on disk, which is all this check compares.
FILE_ENTRY = re.compile(r"^\s*-?\s*file:\s*(?P<path>[^\s#]+)\s*(?:#.*)?$")


def indent_of(line: str) -> int:
    """Number of leading spaces on `line`."""
    return len(line) - len(line.lstrip(" "))


def toc_entries() -> list[str]:
    """Every `file:` path listed under the `toc:` key of `docs/myst.yml`.

    Deliberately a bounded line scan rather than a YAML parse: the check must
    run with nothing but the standard library (the `docs` CI job installs Node,
    not Python packages), and the toc is a fixed, regular block.

    The scan is confined to the `toc:` block so a `file:` key elsewhere in the
    configuration cannot leak in, and the caller verifies the result is
    non-empty — a parser that quietly matched nothing would turn this whole
    check into a no-op that always passes.
    """
    lines = MYST.read_text(encoding="utf-8").splitlines()
    entries: list[str] = []
    toc_indent: int | None = None

    for line in lines:
        stripped = line.strip()
        if toc_indent is None:
            if re.fullmatch(r"toc:\s*(?:#.*)?", stripped):
                toc_indent = indent_of(line)
            continue

        # Blank lines and comments never end a YAML block.
        if not stripped or stripped.startswith("#"):
            continue
        # First line back at or above the `toc:` key's own indentation ends it.
        if indent_of(line) <= toc_indent:
            break

        match = FILE_ENTRY.match(line)
        if match:
            entries.append(match.group("path"))

    if toc_indent is None:
        sys.exit(f"check-docs-toc: no `toc:` key found in {MYST.relative_to(REPO)}")
    return entries


def pages_on_disk() -> set[str]:
    """Every markdown page under `docs/`, as a slash-joined relative path."""
    return {
        str(path.relative_to(DOCS).as_posix())
        for path in DOCS.rglob("*.md")
        if not IGNORED_DIRS.intersection(path.relative_to(DOCS).parts)
    }


def main() -> int:
    entries = toc_entries()
    if not entries:
        # The toc is never legitimately empty; an empty parse means the scan
        # broke against a restructured myst.yml, not that the site has no pages.
        sys.exit(
            "check-docs-toc: parsed 0 `file:` entries from the toc — the scan is "
            f"out of step with {MYST.relative_to(REPO)}, not a real result"
        )

    listed = set(entries)
    on_disk = pages_on_disk()
    problems: list[str] = []

    duplicates = sorted({e for e in entries if entries.count(e) > 1})
    for path in duplicates:
        problems.append(f"  listed {entries.count(path)}x in the toc: {path}")

    for path in sorted(on_disk - listed):
        problems.append(
            f"  on disk but missing from the toc (invisible on the site): {path}"
        )

    for path in sorted(listed - on_disk):
        problems.append(f"  in the toc but no such file under docs/: {path}")

    if problems:
        print(
            "check-docs-toc: docs/myst.yml and docs/**/*.md disagree.\n"
            + "\n".join(problems)
            + "\n\nEvery page under docs/ must appear in the myst.yml toc exactly "
            "once, and every toc entry must name a file that exists.",
            file=sys.stderr,
        )
        return 1

    print(f"check-docs-toc: OK — {len(on_disk)} pages, each listed once in the toc.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
