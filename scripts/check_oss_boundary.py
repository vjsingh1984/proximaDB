#!/usr/bin/env python3
"""Guard the OSS-core vs enterprise/SaaS boundary in the Rust engine.

ProximaDB (this repo) is the open-source database engine — vector / OLTP / OLAP /
multimodal / compute / query — plus the *generic* multitenancy MECHANISM. The
commercial layer (pricing, billing, tenant tiers, cloud control plane) lives in
the separate ``anvaiops`` repo and integrates via sanctioned seams: feature
gates, ports+adapters (operator config + request claims), and neutral naming.
See ``docs/12-design/OSS_ENTERPRISE_BOUNDARY_2026_06_17.adoc``.

This is a deliberately *lightweight* regression fence (in the spirit of
``check_tenant_path_guard.py`` / ``check_workspace_boundaries.py``): it flags
unambiguous commercial leaks (pricing units, billing-ledger fields, commercial
pool-tier variants, control-plane refs) that appear in OSS engine code outside
doc comments / serde aliases / config. Known, not-yet-remediated leaks are
tracked bypasses (WARN, do not fail) so the guard lands before the renames; new
leaks anywhere else FAIL the build.

Exit status:
* 0 - no new violations (tracked bypasses warn only).
* 1 - a new, unlisted commercial leak (ERROR), or any WARNING under ``--strict``.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Unambiguous commercial-leak patterns (NOT generic engineering terms).
DENY = [
    (re.compile(r"\bEnterpriseDedicated\b"), "commercial pool-tier variant"),
    (re.compile(r"\bplatform_fee\b"), "pricing field"),
    (re.compile(r"\b(estimated_)?monthly_cost(_cents)?\b"), "billing-cost field"),
    (re.compile(r"\bbilling_sku\b"), "billing SKU field"),
    (re.compile(r"\bper_gb_month\b|\bper_tb_scanned\b"), "pricing rate"),
    (re.compile(r"\b(ksu|kru|kiu|keu)_per_[a-z_]+\b"), "pricing unit"),
    (re.compile(r"\banvai\w*", re.IGNORECASE), "control-plane (anvaiops) reference"),
]

# Roots to scan (the OSS engine).
SCAN_ROOTS = ["src", "crates"]

# Directories whose contents are commercial surfaces, already feature-gated OFF
# by default (Cargo [features]); exempt from the guard.
EXEMPT_DIR_PARTS = {
    "revenue",
    "sales_enablement",
    "licensing",
    "executive",
    "target",
}

# Known, not-yet-remediated leaks — tracked bypasses (WARN, do not fail). All
# remediation phases B1–B3 have LANDED (StoragePoolClass neutralized; DR billing
# fields → neutral cost_binding_ref/operator_estimate_cents; saas_billing_metrics
# → consumption_metrics), so this is empty and the guard is fully strict: any new
# commercial leak is a hard error. Add an entry only as a deliberate, scheduled
# temporary bypass.
TRACKED_TERMS_GLOBAL: set[str] = set()

# Per-file tracked bypasses (path suffix → allowed substrings).
TRACKED_BYPASS: dict[str, set[str]] = {}


def is_comment(line: str) -> bool:
    s = line.lstrip()
    return s.startswith("//") or s.startswith("*") or s.startswith("#!")


def is_serde_alias(line: str) -> bool:
    return "serde(alias" in line or "alias =" in line


def tracked(rel: str, line: str) -> bool:
    if any(term in line for term in TRACKED_TERMS_GLOBAL):
        return True
    allowed = TRACKED_BYPASS.get(rel)
    return bool(allowed) and any(term in line for term in allowed)


def scan(repo: Path):
    errors: list[str] = []
    warnings: list[str] = []
    for root in SCAN_ROOTS:
        base = repo / root
        if not base.exists():
            continue
        for path in base.rglob("*.rs"):
            parts = set(path.parts)
            if parts & EXEMPT_DIR_PARTS:
                continue
            rel = path.relative_to(repo).as_posix()
            try:
                text = path.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            for i, line in enumerate(text.splitlines(), start=1):
                if is_comment(line) or is_serde_alias(line):
                    continue
                for pat, why in DENY:
                    if pat.search(line):
                        loc = f"{rel}:{i}: commercial leak ({why}): {line.strip()[:100]}"
                        if tracked(rel, line):
                            warnings.append(loc)
                        else:
                            errors.append(loc)
    return errors, warnings


def main() -> int:
    ap = argparse.ArgumentParser(description="OSS/enterprise boundary guard")
    ap.add_argument("--repo", default=".", help="repo root")
    ap.add_argument("--strict", action="store_true", help="treat warnings as errors")
    args = ap.parse_args()

    errors, warnings = scan(Path(args.repo).resolve())

    for w in warnings:
        print(f"WARN  {w}")
    for e in errors:
        print(f"ERROR {e}")

    if errors or (args.strict and warnings):
        print(
            f"\nOSS boundary guard FAILED: {len(errors)} error(s), "
            f"{len(warnings)} tracked warning(s). "
            "Commercial concepts belong in anvaiops or behind a feature gate — see "
            "docs/12-design/OSS_ENTERPRISE_BOUNDARY_2026_06_17.adoc",
            file=sys.stderr,
        )
        return 1
    print(
        f"OSS boundary guard OK: 0 errors, {len(warnings)} tracked bypass(es) "
        "(scheduled for remediation)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
