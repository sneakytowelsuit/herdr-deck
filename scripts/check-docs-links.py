#!/usr/bin/env python3
"""Check that the docs have no dead ends.

Two failure modes that mdBook itself is happy to build:

* a SUMMARY entry pointing at a file that does not exist — the page 404s for readers;
* a relative link between pages pointing at a file that does not exist.

Both are easy to introduce while moving pages around, and neither shows up until someone
clicks. Run from the repository root.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parent.parent / "docs" / "src"

SUMMARY_LINK = re.compile(r"\]\(\./([^)]+)\)")
RELATIVE_LINK = re.compile(r"\]\((\.\.?/[^)#]+)(?:#[^)]*)?\)")


def main() -> int:
    summary = (SRC / "SUMMARY.md").read_text()
    listed = set(SUMMARY_LINK.findall(summary))
    on_disk = {
        str(path.relative_to(SRC))
        for path in SRC.rglob("*.md")
        if path.name != "SUMMARY.md"
    }

    problems: list[str] = []

    for missing in sorted(listed - on_disk):
        problems.append(f"SUMMARY.md lists {missing}, but no such file exists")

    # An orphan is not broken, but it is unreachable, which is nearly as bad.
    for orphan in sorted(on_disk - listed):
        problems.append(f"{orphan} exists but is not in SUMMARY.md, so nothing links to it")

    for page in sorted(SRC.rglob("*.md")):
        for link in RELATIVE_LINK.findall(page.read_text()):
            if not (page.parent / link).resolve().exists():
                problems.append(f"{page.relative_to(SRC)} links to {link}, which does not exist")

    if problems:
        print("Documentation problems:\n")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    print(f"Docs OK: {len(on_disk)} pages, all reachable, all links resolve.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
