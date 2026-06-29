"""ADR-044: the SDK's code-record symbol oid.

A code symbol's record id is the **line-independent canonical oid** minted by
victor-codegraph — derived from the same ``(repo, language, fully_qualified_name,
signature)`` coordinates Victor's adapter and the AnvaiOps connector use, so the same
symbol gets the same oid wherever it is indexed.

P2 cutover (2026-06-29): **default-ON**, matching Victor's adapter (gated behind its
parity ratchet — no collisions / completeness / line-shift stability). Opt out per-process
with ``VICTOR_CODEGRAPH_STABLE_OID=0`` to keep the legacy line-coupled ``path:line:name``
id, byte-identical, during the mixed-read bake.

Pure + dependency-light (only ``os`` + a lazy ``victor_codegraph`` import) so it is
unit-testable without the SDK's gRPC client surface.
"""

from __future__ import annotations

import os

_STABLE_OID_ENV = "VICTOR_CODEGRAPH_STABLE_OID"


def _stable_oid_enabled() -> bool:
    val = os.getenv(_STABLE_OID_ENV)
    if val is None or val.strip() == "":
        return True  # ADR-044 P2: canonical is the default (parity-ratchet-gated)
    return val.strip().lower() in ("1", "true", "yes", "on")


def record_symbol_id(metadata: dict, repo_graph_id: str) -> str:
    """The record id for a code symbol (ADR-044).

    Returns Victor's canonical adapter form ``graph/{repo}/node/{key}`` when the gate is
    on and victor-codegraph supplies the derivation; otherwise the legacy ``symbol_id``
    from ``metadata`` (and on any failure — never raises).
    """
    legacy = metadata.get("symbol_id") or ""
    if not _stable_oid_enabled():
        return legacy
    try:
        import victor_codegraph as _vcg

        key = _vcg.stable_symbol_oid(
            repo_graph_id,
            metadata.get("language") or "",
            metadata.get("fully_qualified_name") or "",
            metadata.get("signature"),
        )
    except Exception:
        return legacy
    return f"graph/{repo_graph_id}/node/{key}"
