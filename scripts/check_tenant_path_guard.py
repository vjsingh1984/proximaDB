#!/usr/bin/env python3
"""Guard against raw tenant-isolated object-store path construction.

CLAUDE.md / AGENTS.md mandate: every Object Storage write must be prefixed by
``DrPathBuilder``, which emits and validates ``data/{tenant_id}/{namespace_id}/
{collection_id}/``. Hand-building that prefix with string interpolation skips
the ID validation (ASCII / no ``/`` / no ``..`` / non-empty) and the tenant/pool
checks, which is a multi-tenant isolation and billing-path risk.

This is a deliberately *lightweight* guard (in the spirit of
``check_workspace_boundaries.py``): it flags string literals shaped like an
interpolated, multi-segment ``data/{...}/...`` prefix. Single-segment local
paths (``format!("data/{}", file_name)``) and literal test fixtures
(``"data/tnt_acme/ns_1/..."``) are intentionally not matched. It is a regression
fence, not a substitute for a real AST/clippy lint.

Exit status:
* 0  - no new violations (known, tracked bypasses warn but do not fail).
* 1  - a new, unlisted raw-prefix construction (ERROR), or any WARNING under
       ``--strict``.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCAN_ROOTS = ("src", "crates")

# Tenant-prefix shape: a string literal whose FIRST segment is an isolation
# root (`data/`, the legacy flat render; `accounts/`, the Phase-5 account-rooted
# render; or `_operator/`, the control-plane root) immediately followed by an
# interpolated segment and at least one more path segment. These are the
# canonical DrPathBuilder prefix shapes. `data/{}` (single segment, local fs)
# and `data/tnt_acme/{x}` (literal-first test fixture) do not match.
PREFIX_RE = re.compile(r'"(?:data|accounts|_operator)/\{[^"}]*\}/')

# Cheap pre-filter substrings — any of these must appear before we line-scan.
PREFILTER = ('"data/{', '"accounts/{', '"_operator/{')

# Files allowed to emit the canonical prefix literally (the one place it is
# constructed and validated).
ALLOWLIST: dict[str, str] = {
    "src/storage/trait_components/path_resolver.rs": (
        "DrPathBuilder canonical implementation — the single place the "
        "`data/{tenant}/{ns}/{collection}/`, account-rooted "
        "`accounts/{account}/{tenant}/{ns}/{collection}/`, and `_operator/` "
        "control-plane prefixes are built and validated."
    ),
}

# Known production bypasses, tracked as non-blocking WARNINGs (printed every run)
# until fixed. Any new, unlisted match is a hard ERROR.
KNOWN_BYPASSES: dict[str, str] = {
    "src/services/dml/mod.rs": (
        "Warehouse `materialize_table_to_parquet` keeps a manual-prefix fallback "
        "(`resolve_materialize_prefix`, legacy layout) used when the DrPath flag "
        "is off or no namespace_id is known. TD-113 is otherwise closed: the real "
        "TenantContext is threaded through the DDL TableMaterializer trait, each "
        "segment is validated via DrPathBuilder::validate_id, and the opt-in "
        "DrPathBuilder layout (PROXIMADB_WAREHOUSE_DRPATH) routes through "
        "build_from_parts. This line is the tracked fallback; do not add new "
        "direct-prefix sites."
    ),
}


@dataclass(frozen=True)
class Finding:
    severity: str  # "error" | "warning"
    location: str  # path:line
    snippet: str
    message: str


def is_test_file(rel: Path) -> bool:
    name = rel.name
    if "tests" in rel.parts or "test" in rel.parts:
        return True
    return (
        name.endswith("_test.rs")
        or name.endswith("_tests.rs")
        or name.startswith("test_")
        or name == "tests.rs"
        or name.endswith("test_utils.rs")
    )


def cfg_test_line_spans(lines: list[str]) -> set[int]:
    """Return 1-based line numbers inside `#[cfg(test)]` items (mods/fns).

    Brace-counts from the first `{` after a `#[cfg(test)]` or `#[test]`/
    `#[tokio::test]` attribute to its matching close. Lightweight: good enough
    for ordinary Rust test modules; not a full parser.
    """
    spans: set[int] = set()
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if "#[cfg(test)]" in line or "#[tokio::test]" in line or line.strip() == "#[test]":
            # Find the opening brace of the following item.
            j = i
            depth = 0
            started = False
            while j < n:
                depth += lines[j].count("{") - lines[j].count("}")
                if "{" in lines[j]:
                    started = True
                if started and depth <= 0:
                    break
                j += 1
            for k in range(i, min(j, n - 1) + 1):
                spans.add(k + 1)  # 1-based
            i = j + 1
            continue
        i += 1
    return spans


def scan() -> list[Finding]:
    findings: list[Finding] = []
    for root in SCAN_ROOTS:
        base = ROOT / root
        if not base.exists():
            continue
        for path in base.rglob("*.rs"):
            rel = path.relative_to(ROOT)
            if is_test_file(rel):
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                continue
            if not any(sub in text for sub in PREFILTER):  # cheap pre-filter
                continue
            lines = text.splitlines()
            test_spans = cfg_test_line_spans(lines)
            for idx, line in enumerate(lines, start=1):
                if idx in test_spans:
                    continue
                if not PREFIX_RE.search(line):
                    continue
                rel_str = rel.as_posix()
                if rel_str in ALLOWLIST:
                    continue
                location = f"{rel_str}:{idx}"
                snippet = line.strip()
                if rel_str in KNOWN_BYPASSES:
                    findings.append(
                        Finding("warning", location, snippet, KNOWN_BYPASSES[rel_str])
                    )
                else:
                    findings.append(
                        Finding(
                            "error",
                            location,
                            snippet,
                            "raw tenant-isolated path built without DrPathBuilder; "
                            "use DrPathBuilder::build(&namespace, &collection_id) and "
                            "DrResolvedPath::root_prefix() instead of a `data/{...}/...` "
                            "format! literal.",
                        )
                    )
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--strict",
        action="store_true",
        help="treat tracked WARNING bypasses as failures too",
    )
    args = parser.parse_args()

    findings = scan()
    print("Tenant-path guard (DrPathBuilder mandate)")
    print(f"scanned roots: {', '.join(SCAN_ROOTS)}")

    errors = [f for f in findings if f.severity == "error"]
    warnings = [f for f in findings if f.severity == "warning"]

    if not findings:
        print()
        print("No raw tenant-path construction found.")
        return 0

    print()
    for f in findings:
        print(f"{f.severity.upper()}: {f.location}")
        print(f"    {f.snippet}")
        print(f"    -> {f.message}")

    print()
    print(f"{len(errors)} error(s), {len(warnings)} tracked warning(s).")
    if errors or (args.strict and warnings):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
