#!/usr/bin/env python3
"""Reject resolved TDs whose body still declares open work."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
TD_DIR = ROOT / "docs" / "10-quality" / "td"
RESOLVED = re.compile(
    r"(?:\|\s*Status\s*\||\*\*Status:\*\*)\s*(?:\*\*)?Resolved\b", re.I
)
OPEN_HEADING = re.compile(r"^==+\s+.*\b(?:open|remaining|pending)\b", re.I)
OPEN_MILESTONE = re.compile(
    r"^\*\*[^\n]*(?:\bopen\b|\bremaining\b|\bpending\b)[^\n]*\*\*:?\s*$", re.I
)


def main() -> int:
    violations: list[str] = []
    for path in sorted(TD_DIR.glob("TD-*.adoc")):
        lines = path.read_text(encoding="utf-8").splitlines()
        if not any(RESOLVED.search(line) for line in lines[:30]):
            continue
        for line_no, line in enumerate(lines, 1):
            if OPEN_HEADING.search(line) or OPEN_MILESTONE.search(line):
                violations.append(
                    f"{path.relative_to(ROOT)}:{line_no}: resolved TD declares open work: {line.strip()}"
                )

    if violations:
        print("TD status consistency check failed:", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        return 1
    print("TD status consistency check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
