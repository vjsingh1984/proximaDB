#!/usr/bin/env python3
"""Reject stale repository-owner references after the anvai-labs migration."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
STALE_PATTERNS = (
    re.compile(
        r"(?:github\.com|raw\.githubusercontent\.com)/vjsingh1984/proxima?db",
        re.IGNORECASE,
    ),
    re.compile(
        r"github\.repository\s*==\s*['\"]vjsingh1984/proximadb['\"]",
        re.IGNORECASE,
    ),
    re.compile(r"owner=vjsingh1984\s+repo=proximadb", re.IGNORECASE),
)


def tracked_files() -> list[Path]:
    output = subprocess.check_output(
        ["git", "ls-files", "-z"], cwd=ROOT, text=True
    )
    return [ROOT / path for path in output.split("\0") if path]


def stale_references() -> list[str]:
    errors: list[str] = []
    for path in tracked_files():
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (UnicodeDecodeError, OSError):
            continue
        for line_number, line in enumerate(lines, start=1):
            if any(pattern.search(line) for pattern in STALE_PATTERNS):
                errors.append(f"{path.relative_to(ROOT)}:{line_number}: {line.strip()}")
    return errors


def main() -> int:
    errors = stale_references()
    if errors:
        print("Stale ProximaDB repository identity references:")
        print("\n".join(errors))
        return 1
    print("Repository identity is canonical: github.com/anvai-labs/proximaDB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
