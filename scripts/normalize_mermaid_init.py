#!/usr/bin/env python3
"""
Normalize Mermaid init blocks across AsciiDoc files.

Fixes common typo patterns like "}}}%%" -> "}}%%" and ensures a closing %% exists.

Scope:
- Walks docs/ recursively
- Skips docs/09-roadmap/**
- Applies to all *.adoc files
"""

from __future__ import annotations

import os
import re
from pathlib import Path

DOCS_DIR = Path(__file__).resolve().parents[1] / "docs"
SKIP_PREFIX = DOCS_DIR / "09-roadmap"


def should_skip(path: Path) -> bool:
    try:
        return SKIP_PREFIX in path.parents or str(path).startswith(str(SKIP_PREFIX))
    except Exception:
        return False


def normalize_mermaid(text: str) -> tuple[str, int]:
    """Return normalized text and number of replacements."""
    count = 0

    # 1) Replace the triple brace variant '}}}%%' with '}}%%'
    def repl_triple(m: re.Match) -> str:
        nonlocal count
        count += 1
        return m.group(0).replace("}}}%%", "}}%%")

    text = re.sub(r"%%\{init:[^\n]*\}\}\}%%", repl_triple, text)

    # 2) Ensure lines with '%%{init: ...}' end with '%%' (some may miss it)
    #    We append %% if a line starts with '%%{init:' and doesn't end with '%%'
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if line.strip().startswith("%%{init:") and not line.strip().endswith("%%"):
            lines[i] = line.rstrip() + "%%"
            count += 1
    return ("\n".join(lines) + ("\n" if text.endswith("\n") else ""), count)


def main() -> int:
    total_files = 0
    total_changes = 0
    for root, _, files in os.walk(DOCS_DIR):
        for fname in files:
            if not fname.endswith(".adoc"):
                continue
            path = Path(root) / fname
            if should_skip(path):
                continue
            try:
                original = path.read_text(encoding="utf-8")
            except Exception:
                continue
            updated, changes = normalize_mermaid(original)
            if changes:
                path.write_text(updated, encoding="utf-8")
                total_files += 1
                total_changes += changes
                print(f"Normalized {changes} mermaid lines in {path}")

    print(f"Done. Files updated: {total_files}, total changes: {total_changes}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

