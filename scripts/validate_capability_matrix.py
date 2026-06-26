#!/usr/bin/env python3
"""Validate docs/_internal/roadmap/CAPABILITY_MATRIX.toml.

Checks:
1. TOML parses.
2. Required top-level keys exist.
3. Capability statuses are in allowed set.
4. Evidence references point to existing files.
5. If line numbers are provided, they are within file bounds.
"""

from __future__ import annotations

import sys
from pathlib import Path
import tomllib

ALLOWED_STATUSES = {"planned", "partial", "complete"}
ROOT = Path(__file__).resolve().parents[1]
MATRIX_PATH = ROOT / "docs" / "_internal" / "roadmap" / "CAPABILITY_MATRIX.toml"

# Minimal high-signal drift checks between capability state and strategic narrative.
# These checks are intentionally narrow to avoid noisy false positives.
DRIFT_RULES = {
    # NOTE: hybrid_search_bm25_vector + framework_integrations_python drift-rules
    # removed — they checked docs/10-quality/proximadb-strategic-analysis.adoc,
    # which was relocated to anvaiops (commercial content) in the OSS
    # decontamination. The strategic-narrative drift-check is commercial-side now.
    "distributed_consensus_runtime": {
        "doc_rules": [
            {
                "path": "docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc",
                "required_substrings": [
                    "| Raft Consensus",  # Just check it exists, don't enforce exact status
                ],
            },
        ],
    },
    "mtls_identity_chain": {
        "doc_rules": [
            {
                "path": "docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc",
                "required_substrings": [
                    "| mTLS",  # Just check it exists, don't enforce exact status
                ],
            },
        ],
    },
    "graph_legacy_route_deprecation": {
        "doc_rules": [
            {
                # Relocated from docs/api/graph.adoc to docs/03-api-reference/ in the
                # 2026-05 docs taxonomy reshuffle; all deprecation/redirect content moved with it.
                "path": "docs/03-api-reference/graph.adoc",
                "required_substrings": [
                    "Legacy Endpoint Deprecation And Migration",
                    "308 Permanent Redirect",
                    "Sunset: `Sunset: 2026-06-30`",
                    "`/api/v1/graph/nodes`",
                    "`/api/v1/graph/graphs/default/nodes`",
                ],
            },
            # NOTE: the former GRAPH_ENGINES_GUIDE.adoc URL-example sub-rule was removed.
            # That guide is now a conceptual runtime guide with no API examples; the
            # multi-graph canonical-route requirement is covered by the graph.adoc rule
            # above and the source-comment rule below.
            {
                "path": "src/network/rest/v1/graph.rs",
                "required_substrings": [
                    "Legacy compatibility routes (`/api/v1/graph/nodes`, `/api/v1/graph/edges`, etc.)",
                    "return `308 Permanent Redirect`",
                    "Sunset date: `2026-06-30`",
                ],
                "forbidden_substrings": [
                    "POST   /api/v1/graph/nodes           - Create node",
                    "POST   /api/v1/graph/edges           - Create edge",
                ],
            },
        ],
    },
    "distributed_graph_engine_messaging": {
        "doc_rules": [
            {
                # PULSAR/QUASAR moved past "experimental" to fully RETIRED (TD-001,
                # Jun 2026); the guide now documents retirement, not experimentation.
                "path": "docs/06-internals/GRAPH_ENGINES_GUIDE.adoc",
                "required_substrings": [
                    "are retired graph engine names",
                    "Do not document, demo, or release-note PULSAR or QUASAR as product engines",
                ],
                "forbidden_substrings": [
                    "PULSAR is production-ready",
                    "QUASAR is production-ready",
                ],
            },
            {
                "path": "docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc",
                "required_substrings": [
                    "| PULSAR Engine",  # Just check it exists
                    "| QUASAR Engine",  # Just check it exists
                ],
                "forbidden_substrings": [
                    "| PULSAR Engine | ✅ Complete",
                    "| QUASAR Engine | ✅ Complete",
                ],
            },
        ],
    },
}


# Maturity-contract reconciliation guard. Unlike DRIFT_RULES, these are NOT tied to a
# capability id -- they always run. They assert the doc-drift reconciliation between the
# product support contract and the technical-debt register / PRD / VISION stays in place,
# so a future edit cannot silently re-introduce "production-ready" claims that contradict
# SUPPORTED_SURFACE. See docs/SUPPORTED_SURFACE.adoc "Authority Hierarchy".
MATURITY_CONTRACT_RULES = [
    {
        "path": "docs/SUPPORTED_SURFACE.adoc",
        "required_substrings": [
            "== Authority Hierarchy",
        ],
    },
    {
        "path": "docs/10-quality/TECHNICAL_DEBT.adoc",
        "required_substrings": [
            # Banner: register tracks implementation state, not support level.
            "This register tracks _implementation state_, not _product-support level_.",
            # Relabeled contradiction cells, each cross-linked to its support tier.
            "_Experimental in v0.2 (see SUPPORTED_SURFACE)_",  # external catalogs (TD-002)
            "_Beta in v0.2 (see SUPPORTED_SURFACE)_",          # mTLS / TST / event (TD-006/009/010)
            "Not a v0.2 supported surface (see SUPPORTED_SURFACE)",  # framework integrations (TD-011)
        ],
    },
    {
        "path": "docs/00-product/PRD.adoc",
        "required_substrings": [
            "shippable/supported surface for v0.2 is defined by",  # precedence banner
            "v0.2 status: Beta",                  # TST + Event P0 notes
            "v0.2 status: not a Supported surface",  # Framework integrations P0 note
        ],
    },
    {
        "path": "docs/00-product/VISION.adoc",
        "required_substrings": [
            "This document states the *broad, durable vision*",
        ],
    },
    # REST /api/v3 is a 308 alias to /api/v2/collections/{id}/documents.
    # These checks ensure the alias relationship and redirect behaviour remain
    # in place and that v3 is not re-promoted to an independent API surface.
    {
        "path": "src/network/rest/v3/mod.rs",
        "required_substrings": [
            # v3 returns a 308 to the canonical v2 path.
            "308 Permanent Redirect",
            # Redirect target comment must reference the canonical v2 path template.
            "/api/v2/collections/{id}/documents",
            # Sunset header constant must be referenced (not hard-coded inline).
            "V3_DOCUMENTS_SUNSET_DATE",
        ],
        "forbidden_substrings": [
            # v3 must not be re-promoted to a stable independent surface.
            "v3 is a stable API surface",
            "v3 production-ready",
        ],
    },
    # gRPC v1 compatibility services must remain OFF by default and must carry
    # Sunset documentation noting post-sunset hard-deletion (see TD-121).
    {
        "path": "crates/platform/proximadb-runtime/src/bootstrap_config.rs",
        "required_substrings": [
            # Default value of the env-var helper must be false (opt-in, not opt-out).
            "unwrap_or(false)",
            # Env-var override name is documented in the field comment.
            "PROXIMADB_GRPC_V1_COMPAT",
            # Hard-deletion intent after Sunset must be stated in the struct field doc.
            "Post-sunset these v1 services are removed entirely",
        ],
        "forbidden_substrings": [
            # v1 compat must not default to enabled.
            "unwrap_or(true)",
        ],
    },
    {
        "path": "src/network/multi_server.rs",
        "required_substrings": [
            # Runtime registration comment must note post-sunset removal.
            "Post-sunset these service impls are removed entirely",
            # v1 services must be behind the flag check, not unconditionally registered.
            "enable_grpc_v1_compat",
        ],
    },
]


def parse_reference(ref: str) -> tuple[Path, int | None]:
    if ":" in ref:
        path_part, line_part = ref.rsplit(":", 1)
        if line_part.isdigit():
            return ROOT / path_part, int(line_part)
    return ROOT / ref, None


def evaluate_doc_rule(cap_id: str, rule: dict, errors: list[str]) -> None:
    doc_path_str = rule.get("path")
    if not isinstance(doc_path_str, str):
        errors.append(f"[{cap_id}] drift-check rule missing valid path")
        return

    doc_path = ROOT / doc_path_str
    if not doc_path.exists():
        errors.append(f"[{cap_id}] drift-check document missing: {doc_path_str}")
        return

    try:
        content = doc_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        errors.append(f"[{cap_id}] drift-check document not UTF-8: {doc_path_str}")
        return

    for snippet in rule.get("forbidden_substrings", []):
        if snippet in content:
            errors.append(
                f"[{cap_id}] drift-check failed: found forbidden text '{snippet}' in {doc_path_str}"
            )

    for snippet in rule.get("required_substrings", []):
        if snippet not in content:
            errors.append(
                f"[{cap_id}] drift-check failed: missing required text '{snippet}' in {doc_path_str}"
            )


def main() -> int:
    if not MATRIX_PATH.exists():
        print(f"ERROR: Missing matrix file: {MATRIX_PATH}")
        return 1

    try:
        data = tomllib.loads(MATRIX_PATH.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        print(f"ERROR: Invalid TOML in {MATRIX_PATH}: {exc}")
        return 1

    errors: list[str] = []
    if "as_of_date" not in data:
        errors.append("Missing top-level key: as_of_date")
    if "capabilities" not in data or not isinstance(data["capabilities"], list):
        errors.append("Missing or invalid top-level key: capabilities")

    for idx, cap in enumerate(data.get("capabilities", []), start=1):
        cap_id = cap.get("id", f"<missing-id-{idx}>")
        status = cap.get("status")
        if status not in ALLOWED_STATUSES:
            errors.append(
                f"[{cap_id}] invalid status '{status}' (allowed: {sorted(ALLOWED_STATUSES)})"
            )

        evidence = cap.get("evidence", [])
        if not isinstance(evidence, list) or not evidence:
            errors.append(f"[{cap_id}] evidence must be a non-empty list")
            continue

        for ref in evidence:
            if not isinstance(ref, str):
                errors.append(f"[{cap_id}] evidence reference must be string: {ref!r}")
                continue
            file_path, line_no = parse_reference(ref)
            if not file_path.exists():
                errors.append(f"[{cap_id}] evidence file missing: {ref}")
                continue
            if line_no is not None:
                try:
                    line_count = len(file_path.read_text(encoding="utf-8").splitlines())
                except UnicodeDecodeError:
                    errors.append(f"[{cap_id}] evidence file not UTF-8 text: {ref}")
                    continue
                if line_no < 1 or line_no > line_count:
                    errors.append(
                        f"[{cap_id}] evidence line out of range: {ref} (max {line_count})"
                    )

    # Cross-doc drift checks
    capabilities_by_id = {
        cap.get("id"): cap for cap in data.get("capabilities", []) if isinstance(cap, dict)
    }
    for cap_id, rule in DRIFT_RULES.items():
        cap = capabilities_by_id.get(cap_id)
        if not cap:
            continue
        if cap.get("status") == "planned":
            continue

        if "doc_rules" in rule:
            doc_rules = rule.get("doc_rules")
            if not isinstance(doc_rules, list) or not doc_rules:
                errors.append(f"[{cap_id}] drift-check has invalid doc_rules definition")
                continue
            for doc_rule in doc_rules:
                if not isinstance(doc_rule, dict):
                    errors.append(f"[{cap_id}] drift-check doc_rule must be object: {doc_rule!r}")
                    continue
                evaluate_doc_rule(cap_id, doc_rule, errors)
            continue

        evaluate_doc_rule(cap_id, rule, errors)

    # Maturity-contract reconciliation guard (always evaluated).
    for rule in MATURITY_CONTRACT_RULES:
        evaluate_doc_rule("maturity_contract", rule, errors)

    if errors:
        print("Capability matrix validation failed:")
        for err in errors:
            print(f"- {err}")
        return 1

    print("Capability matrix validation passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
