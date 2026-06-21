#!/usr/bin/env python3
"""Validate docs/_internal/roadmap/BENCHMARK_EVIDENCE.toml.

The evidence ledger ties each externally-facing performance/capability claim to
its evidence. This guard keeps it honest and machine-checkable:

1. TOML parses; required top-level keys exist.
2. Each claim has the required fields and a known `status`.
3. `asserted_in` references point at existing files / in-range lines (the claim
   must actually appear where it is cited).
4. `bench_source` / `artifact` paths, when given, exist.
5. Status invariants are enforced:
     measured     -> bench_source, bench_command, artifact, hardware, date,
                     version all non-empty (you cannot claim 'measured' without
                     reproducible provenance).
     unverified   -> bench_source non-empty (a bench exists; just no artifact).
     aspirational -> bench_source empty (if a bench exists it is at least
                     unverified, never aspirational).
6. Claim ids are unique.
"""

from __future__ import annotations

import sys
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[1]
LEDGER_PATH = ROOT / "docs" / "_internal" / "roadmap" / "BENCHMARK_EVIDENCE.toml"

ALLOWED_STATUSES = {"measured", "unverified", "aspirational"}
REQUIRED_FIELDS = ("id", "claim", "metric", "status", "asserted_in")
# Fields that must be non-empty when status == "measured".
MEASURED_REQUIRED = ("bench_source", "bench_command", "artifact", "hardware", "date", "version")


def parse_reference(ref: str) -> tuple[Path, int | None]:
    if ":" in ref:
        path_part, line_part = ref.rsplit(":", 1)
        if line_part.isdigit():
            return ROOT / path_part, int(line_part)
    return ROOT / ref, None


def check_path_in_range(ref: str, errors: list[str], cap_id: str, field: str) -> None:
    file_path, line_no = parse_reference(ref)
    if not file_path.exists():
        errors.append(f"[{cap_id}] {field} file missing: {ref}")
        return
    if line_no is not None:
        try:
            line_count = len(file_path.read_text(encoding="utf-8").splitlines())
        except UnicodeDecodeError:
            errors.append(f"[{cap_id}] {field} file not UTF-8 text: {ref}")
            return
        if line_no < 1 or line_no > line_count:
            errors.append(f"[{cap_id}] {field} line out of range: {ref} (max {line_count})")


def main() -> int:
    if not LEDGER_PATH.exists():
        print(f"ERROR: Missing evidence ledger: {LEDGER_PATH}")
        return 1

    try:
        data = tomllib.loads(LEDGER_PATH.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        print(f"ERROR: Invalid TOML in {LEDGER_PATH}: {exc}")
        return 1

    errors: list[str] = []
    if "as_of_date" not in data:
        errors.append("Missing top-level key: as_of_date")
    claims = data.get("claims")
    if not isinstance(claims, list) or not claims:
        errors.append("Missing or empty top-level key: claims")
        claims = []

    seen_ids: set[str] = set()
    for idx, claim in enumerate(claims, start=1):
        cap_id = claim.get("id", f"<missing-id-{idx}>")

        for field in REQUIRED_FIELDS:
            if not claim.get(field):
                errors.append(f"[{cap_id}] missing required field: {field}")

        if cap_id in seen_ids:
            errors.append(f"[{cap_id}] duplicate claim id")
        seen_ids.add(cap_id)

        status = claim.get("status")
        if status not in ALLOWED_STATUSES:
            errors.append(
                f"[{cap_id}] invalid status {status!r} (allowed: {sorted(ALLOWED_STATUSES)})"
            )

        asserted_in = claim.get("asserted_in", [])
        if not isinstance(asserted_in, list) or not asserted_in:
            errors.append(f"[{cap_id}] asserted_in must be a non-empty list")
        else:
            for ref in asserted_in:
                if not isinstance(ref, str):
                    errors.append(f"[{cap_id}] asserted_in entry must be string: {ref!r}")
                    continue
                check_path_in_range(ref, errors, cap_id, "asserted_in")

        bench_source = claim.get("bench_source", "")
        if bench_source:
            check_path_in_range(bench_source, errors, cap_id, "bench_source")
        artifact = claim.get("artifact", "")
        if artifact:
            check_path_in_range(artifact, errors, cap_id, "artifact")

        # Status invariants.
        if status == "measured":
            for field in MEASURED_REQUIRED:
                if not claim.get(field):
                    errors.append(
                        f"[{cap_id}] status=measured requires non-empty {field}"
                    )
        elif status == "unverified":
            if not bench_source:
                errors.append(
                    f"[{cap_id}] status=unverified requires a bench_source "
                    "(a bench that could substantiate the claim)"
                )
        elif status == "aspirational":
            if bench_source:
                errors.append(
                    f"[{cap_id}] status=aspirational must have empty bench_source "
                    "(if a bench exists it is at least 'unverified')"
                )

    if errors:
        print("Benchmark evidence ledger validation failed:")
        for err in errors:
            print(f"- {err}")
        return 1

    measured = sum(1 for c in claims if c.get("status") == "measured")
    unverified = sum(1 for c in claims if c.get("status") == "unverified")
    aspirational = sum(1 for c in claims if c.get("status") == "aspirational")
    print(
        f"Benchmark evidence ledger validation passed: {len(claims)} claims "
        f"({measured} measured, {unverified} unverified, {aspirational} aspirational)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
