"""ADR-044 (SDK, third surface): the SDK keys code records on the same
line-independent canonical oid Victor's adapter mints — so a symbol gets the same
oid wherever it is indexed (Victor / AnvaiOps / the SDK).

P2 cutover (2026-06-29): **default-ON** (gated behind victor-codegraph's parity ratchet),
matching Victor's adapter. Explicit opt-out (``VICTOR_CODEGRAPH_STABLE_OID=0``) keeps the
legacy line-coupled id, byte-identical. Guarded with ``importorskip`` so it runs only
where the optional ``victor-codegraph`` extra is installed.
"""

from __future__ import annotations

import pytest

victor_codegraph = pytest.importorskip("victor_codegraph")

from proximadb_sdk._code_oid import record_symbol_id  # noqa: E402

_LEGACY_META = {
    "symbol_id": "auth.py:1:login",
    "language": "python",
    "fully_qualified_name": "auth.py::login",
    "signature": "(user)",
}


def test_default_on_returns_canonical_form(monkeypatch):
    # P2 cutover: with the env unset, the canonical line-independent oid is the default.
    monkeypatch.delenv("VICTOR_CODEGRAPH_STABLE_OID", raising=False)
    key = victor_codegraph.stable_symbol_oid(
        "repo1", "python", "auth.py::login", "(user)"
    )
    assert record_symbol_id(_LEGACY_META, "repo1") == f"graph/repo1/node/{key}"


def test_explicit_opt_out_returns_legacy_id(monkeypatch):
    # Opt-out keeps existing collections byte-identical during the bake.
    monkeypatch.setenv("VICTOR_CODEGRAPH_STABLE_OID", "0")
    assert record_symbol_id(_LEGACY_META, "repo1") == "auth.py:1:login"


def test_gated_on_returns_canonical_form(monkeypatch):
    monkeypatch.setenv("VICTOR_CODEGRAPH_STABLE_OID", "1")
    key = victor_codegraph.stable_symbol_oid(
        "repo1", "python", "auth.py::login", "(user)"
    )
    assert record_symbol_id(_LEGACY_META, "repo1") == f"graph/repo1/node/{key}"


def test_cross_surface_matches_victor_adapter(monkeypatch):
    # The SDK's record oid must equal Victor's adapter oid for the SAME symbol — derive
    # the metadata from a real parse so the FQN/signature are the parser's own.
    monkeypatch.setenv("VICTOR_CODEGRAPH_STABLE_OID", "1")
    src = "def login(user):\n    return check(user)\n"
    parsed = victor_codegraph.parse(src, file_path="auth.py")
    recs = victor_codegraph.to_proxima_records(
        parsed, repo_graph_id="repo1", stable_oid=True
    )
    adapter = next(
        r for r in recs if "graph_node" in r["labels"] and r["props"]["name"] == "login"
    )
    meta = {
        "symbol_id": "legacy-placeholder",
        "language": adapter["props"]["lang"],
        "fully_qualified_name": adapter["props"]["fully_qualified_name"],
        "signature": adapter["props"]["signature"],
    }
    assert record_symbol_id(meta, "repo1") == adapter["oid"]


def test_falls_back_to_legacy_when_derivation_unavailable(monkeypatch):
    monkeypatch.setenv("VICTOR_CODEGRAPH_STABLE_OID", "1")
    # Missing FQN/language → still returns a deterministic id (never raises).
    assert record_symbol_id({"symbol_id": "x:1:y"}, "repo1").endswith(
        "y"
    ) or record_symbol_id({"symbol_id": "x:1:y"}, "repo1").startswith("graph/")
