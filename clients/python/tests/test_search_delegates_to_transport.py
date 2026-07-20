"""Regression: SDK search() must hit the server, not silently fall back to a
client-side local store (TD-SDK-1 S2b / ADR-068 D5).

Symptom (2026-07-20 local Azurite round-trip): ``client.search(...)`` returned
``[]`` though the server returned correct results (curl + server logs confirm).
Root cause: ``adapters/rest_adapter.RestAdapter.search`` called the REST transport
(``protocols.rest_sync.ProximaDBClient.search``) with the WRONG kwarg names —
``query_vector`` / ``metadata_filters`` instead of ``vector`` / ``metadata_filter``
— raising ``TypeError``, which ``unified_client.search_single`` catches and
converts into a sticky *local fallback* (``_activate_local_fallback``), so the
read never reaches the server.

This is exactly the facade bug the codegen-drift gate cannot see; per ADR-068 D5
it is owned by a behavioral/differential check. Here we drive the full
facade -> adapter -> transport chain with the transport mocked to a recorded
response shape (fixture-replay) and assert (a) the call reaches the transport,
(b) the kwargs flow correctly, and (c) the results pass through — i.e. NO local
fallback.
"""

from __future__ import annotations

import pytest

from proximadb_sdk import connect_rest
from proximadb_sdk.models import SearchResult


def _fresh_client():
    # Fresh client => _prefer_local_fallback is False, so search_single tries
    # the adapter (transport) path rather than the local store.
    return connect_rest("http://127.0.0.1:5678")


def test_search_delegates_to_transport_and_returns_results(monkeypatch) -> None:
    client = _fresh_client()
    adapter = client._adapter  # the instance search_single uses (NOT _get_rest_adapter(), a different one)
    assert adapter is not None, "REST adapter must be wired for connect_rest"

    expected = [
        SearchResult(id="r1", score=0.987),
        SearchResult(id="r2", score=0.523),
    ]

    captured: dict[str, object] = {}

    def fake_transport_search(
        collection_id: str,
        vector: list[float],
        top_k: int = 10,
        metadata_filter: dict | None = None,
        **kwargs: object,
    ) -> list[SearchResult]:
        # Signature matches protocols.rest_sync.ProximaDBClient.search. If the
        # adapter passes the OLD wrong kwargs (query_vector / metadata_filters),
        # this is never reached — the TypeError trips local fallback first.
        captured.update(
            collection_id=collection_id,
            vector=vector,
            top_k=top_k,
            metadata_filter=metadata_filter,
        )
        return expected

    # Replace the leaf REST transport's search (NOT the adapter, NOT the facade).
    adapter._client.search = fake_transport_search  # type: ignore[assignment]

    results = client.search("testcol", [0.1, 0.2, 0.3, 0.4], top_k=5)

    # (c) results pass through — no silent [] / local fallback.
    assert [r.id for r in results] == ["r1", "r2"]
    # (a)+(b) the transport was reached with the correctly-named kwargs.
    assert captured.get("collection_id") == "testcol"
    assert captured.get("vector") == [0.1, 0.2, 0.3, 0.4]
    assert captured.get("top_k") == 5


def test_search_metadata_filter_flows_to_transport(monkeypatch) -> None:
    client = _fresh_client()
    adapter = client._adapter  # the instance search_single uses (NOT _get_rest_adapter(), a different one)
    captured: dict[str, object] = {}

    def fake_transport_search(
        collection_id: str,
        vector: list[float],
        top_k: int = 10,
        metadata_filter: dict | None = None,
        **kwargs: object,
    ) -> list[SearchResult]:
        captured["metadata_filter"] = metadata_filter
        return []

    adapter._client.search = fake_transport_search  # type: ignore[assignment]
    client.search("c", [0.1, 0.2, 0.3, 0.4], top_k=3, metadata_filter={"t": "x"})
    assert captured.get("metadata_filter") == {"t": "x"}
