#!/usr/bin/env python3
"""Guard against duplicate TD and ADR identifiers across the repo.

Concurrent agent sessions repeatedly collided on TD/ADR *numbers* — two sessions
off an old base both pick the same "next" number, discovered only at merge (or
silently coexisting, e.g. two `ADR-031-*.adoc`). Sequential numbering in a shared
space has no atomic allocator, so this is a CI **backstop**: it fails the PR the
moment a duplicate id appears in the tree. Because a rebased PR re-runs CI, branch
B's `ADR-037` collides with develop's `ADR-037` on rebase and is caught *before*
merge.

What it checks (all hard errors):
  1. No two ADR files share a number — `docs/12-design/adr/ADR-<NNN>-<slug>.adoc`.
  2. No two TDs share an id — across BOTH the legacy `TECHNICAL_DEBT.adoc` table
     rows (`| TD-… |`) AND the per-file TDs (`docs/10-quality/td/TD-<id>-…`), so a
     new per-file TD can't reuse a legacy id. Non-numeric topic-scoped ids
     (`TD-CAT-1`, `TD-SC-2`) and variants (`TD-162a-2`) compare as full tokens.

What it warns (does not fail — the README index drift is a tracked backlog):
  3. An ADR file not listed in the ADR README index.

Exit 0 = unique; 1 = a duplicate (or, with --strict, any warning).

Filing convention: see `docs/12-design/HOW_TO_FILE_TD_AND_ADR.adoc`.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ADR_DIR = ROOT / "docs" / "12-design" / "adr"
ADR_README = ADR_DIR / "README.adoc"
TD_TABLE = ROOT / "docs" / "10-quality" / "TECHNICAL_DEBT.adoc"
TD_DIR = ROOT / "docs" / "10-quality" / "td"

# ADR filename: ADR-<digits><optional letter>-<slug>.adoc  → capture the number.
ADR_FILE_RE = re.compile(r"^ADR-(\d+[a-z]?)-.+\.adoc$")
# A TD id token: TD- followed by an alnum/hyphen run (TD-167, TD-162a-2, TD-CAT-1).
TD_ID_RE = re.compile(r"\bTD-[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*\b")
# A TD id at the START of an AsciiDoc table cell: `| TD-... |` (the id column).
TD_ROW_RE = re.compile(r"^\|\s*(TD-[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*)\b")


def adr_numbers() -> dict[str, list[str]]:
    """Map ADR number -> list of filenames declaring it (>1 ⇒ collision)."""
    by_num: dict[str, list[str]] = defaultdict(list)
    if not ADR_DIR.is_dir():
        return by_num
    for p in sorted(ADR_DIR.glob("ADR-*.adoc")):
        m = ADR_FILE_RE.match(p.name)
        if m:
            by_num[m.group(1)].append(p.name)
    return by_num


def td_ids() -> dict[str, list[str]]:
    """Map TD id -> list of sources declaring it (legacy table rows + per-file)."""
    by_id: dict[str, list[str]] = defaultdict(list)
    # Legacy monolithic table: one declaration per id-column row.
    if TD_TABLE.is_file():
        for line in TD_TABLE.read_text(encoding="utf-8").splitlines():
            m = TD_ROW_RE.match(line)
            if m:
                by_id[m.group(1)].append(f"{TD_TABLE.name} (table row)")
    # Per-file TDs: the id is read from the file's first `= TD-<id>: ...` heading
    # (NOT the filename — a slug uses hyphens too, so `TD-167-foo` is ambiguous;
    # in the heading the id is delimited by `:`/space, matching how the legacy
    # table row and ADR titles declare their id). Filename is cosmetic.
    if TD_DIR.is_dir():
        for p in sorted(TD_DIR.glob("TD-*.adoc")):
            if p.name.lower() == "readme.adoc":
                continue
            heading = next(
                (
                    line
                    for line in p.read_text(encoding="utf-8").splitlines()
                    if line.startswith("= ")
                ),
                "",
            )
            m = TD_ID_RE.search(heading)
            if m:
                by_id[m.group(0)].append(p.name)
            else:
                # A per-file TD with no `= TD-…:` heading can't be uniqueness-checked.
                by_id[f"<no-id-heading:{p.name}>"].append(p.name)
    return by_id


def indexed_adrs() -> set[str]:
    """ADR filenames linked from the ADR README index."""
    if not ADR_README.is_file():
        return set()
    text = ADR_README.read_text(encoding="utf-8")
    return set(re.findall(r"ADR-\d+[a-z]?-[A-Za-z0-9._-]+\.adoc", text))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--strict",
        action="store_true",
        help="treat warnings (unindexed ADRs) as failures too",
    )
    args = ap.parse_args()

    errors: list[str] = []
    warnings: list[str] = []

    for num, files in sorted(adr_numbers().items()):
        if len(files) > 1:
            errors.append(
                f"ADR number {num} is used by {len(files)} files: {', '.join(files)} "
                f"— renumber to the next free ADR (scan {ADR_DIR.relative_to(ROOT)}/)."
            )

    for tid, sources in sorted(td_ids().items()):
        if len(sources) > 1:
            errors.append(
                f"TD id {tid} is declared {len(sources)}×: {', '.join(sources)} "
                f"— a TD id must be unique across the legacy table and the per-file dir."
            )

    indexed = indexed_adrs()
    if indexed:  # only warn when an index exists to compare against
        for p in sorted(ADR_DIR.glob("ADR-*.adoc")):
            if p.name not in indexed:
                warnings.append(f"ADR file not listed in the README index: {p.name}")

    for w in warnings:
        print(f"WARNING: {w}", file=sys.stderr)
    for e in errors:
        print(f"ERROR: {e}", file=sys.stderr)

    n_adr = sum(len(v) for v in adr_numbers().values())
    n_td = len(td_ids())
    print(
        f"checked {n_adr} ADR files, {n_td} TD ids — "
        f"{len(errors)} error(s), {len(warnings)} warning(s)."
    )

    if errors or (args.strict and warnings):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
