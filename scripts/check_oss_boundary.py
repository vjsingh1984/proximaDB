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
    (re.compile(r"\b(ksu|kru|kiu|keu|kou)_per_[a-z_]+\b"), "pricing unit"),
    (re.compile(r"\banvai\w*", re.IGNORECASE), "control-plane (anvaiops) reference"),
]

# Bare commercial SKU/tier names. These are flagged ONLY when they appear in an
# enum-variant context — either a fully-qualified reference (``Enum::Variant``)
# or a bare variant *declaration* (a line that is just ``Variant,`` / ``Variant {``
# / ``Variant(``). Compound identifiers (``BusinessContext``, ``BusinessRule``,
# ``EnterpriseAuthManager``, ``AzurePremium``) are NOT matched thanks to ``\b``
# word boundaries, and free-text occurrences inside string literals (log/UI copy
# in the enterprise-BI surfaces) are skipped by ``looks_like_string_literal``.
COMMERCIAL_SKU = re.compile(r"\b(Business|Enterprise|Premium|Pro)\b")
SKU_VARIANT_REF = re.compile(r"::(Business|Enterprise|Premium|Pro)\b")
SKU_VARIANT_DECL = re.compile(r"^\s*(Business|Enterprise|Premium|Pro)\s*[,{(]")

# Enum prefixes whose ``::<Variant>`` uses are architectural posture / already
# neutralized-via-alias, NOT commercial pricing (per the §3 rubric "No action"
# list). A reference qualified by one of these prefixes is exempt; so is a bare
# variant declaration belonging to one of these enums (allowlisted by the file
# that defines them, below).
SKU_ALLOWED_PREFIXES = (
    "SecurityMode::",  # architectural security posture, not pricing
    "StoragePoolClass::",  # neutralized pool class (commercial name only as alias)
    "LicenseTier::",  # license editions live behind the licensing_surface gate
)

# Files allowed to *declare* a bare commercial-named variant because the enum is
# a sanctioned architectural/neutralized type (not a pricing SKU). Path suffix →
# allowed bare variant names.
SKU_DECL_ALLOWED: dict[str, set[str]] = {
    "src/security/security_coordinator.rs": {"Enterprise"},  # SecurityMode::Enterprise
    "crates/control/proximadb-catalog/src/lib.rs": {"Premium"},  # StoragePoolClass::Premium
}

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

# TODO(oss-boundary follow-up): ``src/ai/**`` (LLM/analytics/executive-dashboard
# "Business Intelligence" surface) is an un-gated enterprise-BI surface that
# carries commercial-flavoured *string copy* (log/UI text). It is NOT yet behind
# a feature gate; that is a separate, scheduled remediation (feature-gate the
# tree, like revenue/sales/licensing/executive). This guard does not flag it
# today because its commercial terms live in string literals, not enum-variant
# identifiers — ``sku_variant_leak`` only fires on enum-variant contexts, and the
# DENY pricing/anvai patterns don't appear there. Track the gating separately.

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


def strip_string_literals(line: str) -> str:
    """Blank out the contents of ``"..."`` string literals so identifier checks
    don't fire on log/UI copy (e.g. ``"Enterprise trial created"``). Best-effort:
    a simple non-escaped double-quote pairing, which is sufficient for the
    enum-variant detection this guard does (it never needs to reason *about* the
    text inside a string)."""
    return re.sub(r'"(?:[^"\\]|\\.)*"', '""', line)


def sku_variant_leak(rel: str, line: str) -> str | None:
    """Return a reason string if ``line`` references a bare commercial SKU/tier
    enum variant outside the allowlisted (architectural / neutralized) enums, or
    ``None`` if clean. Operates on the string-literal-stripped line so free-text
    mentions in log/UI copy are ignored."""
    code = strip_string_literals(line)
    if not COMMERCIAL_SKU.search(code):
        return None
    # Fully-qualified ``Enum::Variant`` reference. The enum name is the run of
    # identifier chars immediately preceding the ``::`` of the match; allow it
    # when that ``Enum::`` qualifier is on the allowlist (architectural posture /
    # neutralized type), flag it otherwise.
    for m in SKU_VARIANT_REF.finditer(code):
        enum_name = re.search(r"([A-Za-z0-9_]+)::$", code[: m.start() + 2])
        qualifier = f"{enum_name.group(1)}::" if enum_name else ""
        if qualifier not in SKU_ALLOWED_PREFIXES:
            return f"commercial SKU/tier variant reference ({m.group(1)})"
    # Bare variant declaration (e.g. a line that is just ``Enterprise,``).
    decl = SKU_VARIANT_DECL.match(code)
    if decl:
        variant = decl.group(1)
        if variant not in SKU_DECL_ALLOWED.get(rel, set()):
            return f"commercial SKU/tier variant declaration ({variant})"
    return None


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
                # Bare commercial SKU/tier enum-variant leak (enum-context only;
                # compound identifiers + string-literal copy are not flagged).
                sku = sku_variant_leak(rel, line)
                if sku:
                    loc = f"{rel}:{i}: commercial leak ({sku}): {line.strip()[:100]}"
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
