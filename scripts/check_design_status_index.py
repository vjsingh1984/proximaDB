#!/usr/bin/env python3
"""Validate the ADR/TD status corpus as a complete, parseable status-of-record.

This is deliberately a structural guard, not a claim that prose is true. It makes
the truth surface finite and reviewable:

* every ADR and per-file TD must expose a status in one of the repository's four
  historical forms (``:status:``, a ``Status`` table row, ``**Status:**``, or
  ``== Status``);
* every legacy TD row in ``TECHNICAL_DEBT.adoc`` must use its controlled status
  vocabulary;
* every ADR file must be linked exactly once from the ADR README index;
* the README status class must agree with the ADR's own status class.

Claim freshness is handled separately by ``check_status_as_of.py``. Together the
two checks prevent a newly filed decision from disappearing from the index and
force periodic code-backed review of the index's current-state claims.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ADR_DIR = ROOT / "docs" / "12-design" / "adr"
ADR_README = ADR_DIR / "README.adoc"
TD_DIR = ROOT / "docs" / "10-quality" / "td"
LEGACY_TD_REGISTER = ROOT / "docs" / "10-quality" / "TECHNICAL_DEBT.adoc"

ATTR_STATUS_RE = re.compile(r"^:status:\s*(.+?)\s*$", re.I | re.M)
TABLE_STATUS_RE = re.compile(r"^\|\s*Status\s*\|\s*(.+?)\s*$", re.I | re.M)
LABEL_STATUS_RE = re.compile(r"^\*\*Status:\*\*\s*(.+?)\s*$", re.I | re.M)
SECTION_STATUS_RE = re.compile(r"^==\s+Status\s*$", re.I)
ADR_CLASSES = ("accepted", "proposed", "superseded", "rejected", "deprecated")
LEGACY_TD_CLASSES = ("Open", "In Progress", "Partial", "Resolved", "Won't Do")


def clean_status(value: str) -> str:
    """Strip lightweight AsciiDoc emphasis without flattening the status prose."""
    return value.strip().replace("**", "").replace("`", "")


def section_status(lines: list[str]) -> str | None:
    for index, line in enumerate(lines):
        if not SECTION_STATUS_RE.match(line):
            continue
        for candidate in lines[index + 1 :]:
            candidate = candidate.strip()
            if not candidate or candidate.startswith("//"):
                continue
            return clean_status(candidate)
    return None


def statuses(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    found = [clean_status(match) for match in ATTR_STATUS_RE.findall(text)]
    found.extend(clean_status(match) for match in TABLE_STATUS_RE.findall(text))
    found.extend(clean_status(match) for match in LABEL_STATUS_RE.findall(text))
    section = section_status(text.splitlines())
    if section:
        found.append(section)
    return found


def status_class(value: str) -> str | None:
    lowered = value.lower()
    matches = [
        (match.start(), candidate)
        for candidate in ADR_CLASSES
        if (match := re.search(rf"\b{candidate}\b", lowered))
    ]
    return min(matches)[1] if matches else None


def readme_statuses() -> dict[str, list[str]]:
    """Parse the four-row ADR index records and return filename -> status cells."""
    lines = ADR_README.read_text(encoding="utf-8").splitlines()
    result: dict[str, list[str]] = {}
    for index, line in enumerate(lines):
        match = re.match(r"^\|\s+link:(ADR-[^.]+\.adoc)\[ADR-\d+[a-z]?\]\s*$", line)
        if not match:
            continue
        filename = match.group(1)
        # Record layout is link, title, status, governs. Skip blank lines while
        # retaining only table-cell lines.
        cells: list[str] = []
        for candidate in lines[index + 1 :]:
            if candidate.startswith("| "):
                cells.append(candidate[2:].strip())
                if len(cells) == 3:
                    break
        if len(cells) >= 2:
            result.setdefault(filename, []).append(clean_status(cells[1]))
    return result


def legacy_td_rows() -> list[tuple[int, str, str]]:
    """Return (line, id, status) for rows in the pre-per-file live TD register."""
    rows: list[tuple[int, str, str]] = []
    for line_number, line in enumerate(
        LEGACY_TD_REGISTER.read_text(encoding="utf-8").splitlines(), start=1
    ):
        if not re.match(r"^\|\s*TD-", line):
            continue
        cells = [cell.strip() for cell in line.split("|")[1:]]
        if len(cells) >= 4:
            rows.append((line_number, cells[0], cells[3]))
        else:
            rows.append((line_number, cells[0] if cells else "TD-?", ""))
    return rows


def valid_legacy_td_status(value: str) -> bool:
    return any(
        value == candidate or value.startswith(f"{candidate} (")
        for candidate in LEGACY_TD_CLASSES
    )


def main() -> int:
    errors: list[str] = []

    adr_files = sorted(ADR_DIR.glob("ADR-*.adoc"))
    td_files = sorted(TD_DIR.glob("TD-*.adoc"))
    for path in [*adr_files, *td_files]:
        if not statuses(path):
            errors.append(f"{path.relative_to(ROOT)}: no parseable status")

    legacy_rows = legacy_td_rows()
    if not legacy_rows:
        errors.append(f"{LEGACY_TD_REGISTER.relative_to(ROOT)}: no legacy TD rows found")
    for line_number, td_id, status in legacy_rows:
        if not valid_legacy_td_status(status):
            errors.append(
                f"{LEGACY_TD_REGISTER.relative_to(ROOT)}:{line_number}: "
                f"{td_id} uses uncontrolled status {status!r}"
            )

    indexed = readme_statuses()
    for path in adr_files:
        entries = indexed.get(path.name, [])
        if not entries:
            errors.append(f"{path.relative_to(ROOT)}: missing from ADR README index")
            continue
        if len(entries) != 1:
            errors.append(
                f"{path.relative_to(ROOT)}: indexed {len(entries)} times (want exactly once)"
            )
            continue
        own_classes = {status_class(value) for value in statuses(path)} - {None}
        index_class = status_class(entries[0])
        if not own_classes:
            errors.append(
                f"{path.relative_to(ROOT)}: status has no ADR lifecycle class "
                f"({', '.join(ADR_CLASSES)})"
            )
        elif index_class not in own_classes:
            errors.append(
                f"{path.relative_to(ROOT)}: README class {index_class!r} disagrees "
                f"with file class(es) {sorted(own_classes)!r}"
            )

    unknown = sorted(set(indexed) - {path.name for path in adr_files})
    for filename in unknown:
        errors.append(f"{ADR_README.relative_to(ROOT)}: links missing ADR file {filename}")

    if errors:
        print("design status index check failed:", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1

    print(
        f"design status index check passed: {len(adr_files)} ADRs, "
        f"{len(td_files)} per-file TDs, {len(legacy_rows)} legacy TD rows"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
