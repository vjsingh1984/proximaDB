"""Tests for the DualUseStore integration helper (TD-051).

The store wraps a ``ProximaDBClient`` with a paper-compatible model
(arXiv:2604.14403 Killingback et al. 2026) so callers can use
embeddings as both retrieval keys AND compressed context, never
storing raw text on the database side. The model is the contract;
the SDK just makes it cheap to use.

These tests use a stub ``DualUseModel`` so we can exercise the
end-to-end shape without a real model and without a real
ProximaDB server.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional

import pytest

from proximadb_sdk.integrations.dual_use_store import (  # noqa: E402
    DualUseModel,
    DualUseRetrievalResult,
    DualUseStore,
)
from proximadb_sdk.models import SearchResult, VectorRecord


# ---- stub model ----------------------------------------------------

class StubDualUseModel:
    """Toy ``DualUseModel`` that maps text → length-3 vector and back.

    The "decompression" round-trip is lossy by construction (we only
    store the length of the original text), but that's enough to pin
    the contract: retrieval surfaces the decompressed text, not raw
    metadata storage.
    """

    def __init__(self) -> None:
        self.embed_calls: list[str] = []
        self.decompress_calls: list[tuple[float, ...]] = []
        self._reverse_index: dict[tuple[float, ...], str] = {}

    def embed(self, text: str) -> list[float]:
        self.embed_calls.append(text)
        # Encode three crude features so distinct strings map to
        # distinct vectors. Keeps the stub deterministic and lets us
        # round-trip via the reverse index.
        vec = [
            float(len(text)),
            float(sum(ord(c) for c in text) % 100),
            float(text.count(" ")),
        ]
        self._reverse_index[tuple(vec)] = text
        return vec

    def decompress(self, embedding: list[float]) -> str:
        self.decompress_calls.append(tuple(embedding))
        # Look up the original text. In a real model this is the
        # decoder running over the embedding; here we just round-trip
        # through what we stored at embed() time.
        return self._reverse_index.get(
            tuple(embedding), f"<unknown:{embedding}>"
        )


# ---- stub client ---------------------------------------------------

@dataclass
class _Insert:
    collection_id: str
    records: list[VectorRecord]


@dataclass
class _Search:
    collection_id: str
    vector: list[float]
    top_k: int
    include_vectors: bool
    include_metadata: bool


@dataclass
class StubClient:
    """Minimal stand-in for ProximaDBClient. Records every call so
    tests can assert on behavior, and lets each test seed the search
    response."""

    inserts: list[_Insert] = field(default_factory=list)
    searches: list[_Search] = field(default_factory=list)
    deletes: list[tuple[str, list[str]]] = field(default_factory=list)
    next_search_results: list[SearchResult] = field(default_factory=list)

    def insert_vectors(
        self, collection_id: str, records=None, **kwargs: Any
    ) -> dict[str, Any]:
        records = records or kwargs.get("records") or []
        self.inserts.append(
            _Insert(collection_id=collection_id, records=list(records))
        )
        return {"success": True, "count": len(list(records))}

    def search(
        self,
        collection_id: str,
        vector,
        top_k: int = 10,
        include_metadata: bool = True,
        include_vectors: bool = False,
        **kwargs: Any,
    ) -> list[SearchResult]:
        self.searches.append(
            _Search(
                collection_id=collection_id,
                vector=list(vector),
                top_k=top_k,
                include_vectors=include_vectors,
                include_metadata=include_metadata,
            )
        )
        return list(self.next_search_results)

    def delete_vectors(
        self, collection_id: str, vector_ids: list[str]
    ) -> dict[str, Any]:
        self.deletes.append((collection_id, list(vector_ids)))
        return {"success": True}


# ---- helpers -------------------------------------------------------

def make_store() -> tuple[DualUseStore, StubClient, StubDualUseModel]:
    client = StubClient()
    model = StubDualUseModel()
    store = DualUseStore(client=client, collection_id="docs", model=model)
    return store, client, model


# ---- protocol contract ---------------------------------------------

def test_stub_model_satisfies_dual_use_model_protocol() -> None:
    """Anyone implementing ``embed`` + ``decompress`` should be
    accepted as a ``DualUseModel``. Pin the structural-typing contract."""
    model = StubDualUseModel()
    # Direct isinstance check via runtime_checkable Protocol.
    assert isinstance(model, DualUseModel)


# ---- add() ---------------------------------------------------------

def test_add_stores_only_embedding_no_raw_text() -> None:
    """The whole point of dual-use embeddings: raw text MUST NOT
    appear in the stored metadata. Pin this aggressively because a
    regression here defeats the storage-saving claim."""
    store, client, _model = make_store()
    store.add("hello world", doc_id="d1")

    assert len(client.inserts) == 1
    inserted = client.inserts[0].records[0]
    assert inserted.id == "d1"
    assert inserted.vector == [11.0, float(sum(ord(c) for c in "hello world") % 100), 1.0]

    # The metadata MUST NOT carry the original text -- if a future
    # change accidentally stuffs it back in, this assertion catches it.
    md = inserted.metadata or {}
    assert "text" not in md
    assert "source" not in md
    assert "raw" not in md
    assert "content" not in md


def test_add_returns_assigned_doc_id() -> None:
    store, _client, _ = make_store()
    assert store.add("alpha", doc_id="d1") == "d1"


def test_add_generates_doc_id_when_not_supplied() -> None:
    store, client, _ = make_store()
    returned_id = store.add("alpha")
    assert returned_id, "must return a non-empty id"
    inserted = client.inserts[0].records[0]
    assert inserted.id == returned_id


def test_add_calls_embed_exactly_once() -> None:
    """Embedding is the expensive call. Make sure we never embed
    twice for one add() (e.g., once for storage, once for ID
    derivation)."""
    store, _client, model = make_store()
    store.add("alpha")
    assert model.embed_calls == ["alpha"]


# ---- add_many() ----------------------------------------------------

def test_add_many_stores_each_text_with_no_raw_text() -> None:
    store, client, model = make_store()
    ids = store.add_many(["a", "bb", "ccc"])

    assert len(ids) == 3
    assert len(set(ids)) == 3, "ids must be unique"
    assert model.embed_calls == ["a", "bb", "ccc"]

    # All inserts batched into one call -- a contract for performance.
    assert len(client.inserts) == 1
    records = client.inserts[0].records
    assert len(records) == 3
    for rec in records:
        md = rec.metadata or {}
        assert "text" not in md and "source" not in md


def test_add_many_uses_supplied_ids_when_provided() -> None:
    store, client, _ = make_store()
    ids = store.add_many(["x", "y"], ids=["i1", "i2"])
    assert ids == ["i1", "i2"]
    inserted_ids = [r.id for r in client.inserts[0].records]
    assert inserted_ids == ["i1", "i2"]


def test_add_many_rejects_mismatched_ids_length() -> None:
    store, _client, _ = make_store()
    with pytest.raises(ValueError, match="ids length"):
        store.add_many(["x", "y"], ids=["only-one"])


def test_add_many_with_empty_input_does_not_call_client() -> None:
    """Robustness: an empty batch must not cause an empty insert
    that the server might reject."""
    store, client, model = make_store()
    ids = store.add_many([])
    assert ids == []
    assert client.inserts == []
    assert model.embed_calls == []


# ---- retrieve() ----------------------------------------------------

def test_retrieve_includes_vectors_so_decompress_can_run() -> None:
    """The critical invariant: the search call MUST request vectors.
    Without them, the model has nothing to decompress and the whole
    pattern is broken."""
    store, client, model = make_store()
    # Seed: model embeds "alpha" first, so that vector reconstructs.
    embedded = model.embed("alpha")
    client.next_search_results = [
        SearchResult(id="d1", score=0.9, vector=embedded)
    ]

    results = store.retrieve("alpha")

    assert len(client.searches) == 1
    assert client.searches[0].include_vectors is True, (
        "DualUseStore must request vectors so decompress() has input"
    )
    assert len(results) == 1
    assert results[0].text == "alpha"


def test_retrieve_decompresses_each_result() -> None:
    store, client, model = make_store()
    v_alpha = model.embed("alpha")
    v_beta = model.embed("beta")
    v_gamma = model.embed("gamma three words here")
    client.next_search_results = [
        SearchResult(id="d1", score=0.9, vector=v_alpha),
        SearchResult(id="d2", score=0.7, vector=v_beta),
        SearchResult(id="d3", score=0.5, vector=v_gamma),
    ]

    results = store.retrieve("query text")
    texts = [r.text for r in results]
    assert texts == ["alpha", "beta", "gamma three words here"]


def test_retrieve_preserves_score_order() -> None:
    """The store must NOT reorder results -- the server's ranking is
    authoritative. We just decompress in place."""
    store, client, model = make_store()
    v1 = model.embed("low")
    v2 = model.embed("high")
    # Server returns in order of decreasing score.
    client.next_search_results = [
        SearchResult(id="d2", score=0.9, vector=v2),
        SearchResult(id="d1", score=0.3, vector=v1),
    ]
    results = store.retrieve("query")
    assert [r.id for r in results] == ["d2", "d1"]
    assert [r.score for r in results] == [0.9, 0.3]


def test_retrieve_skips_results_missing_vector() -> None:
    """Server might return a result without a vector if vectors were
    pruned for size. Decompress can't run on those, so the store
    should skip them rather than yield malformed text. Robustness."""
    store, client, model = make_store()
    v = model.embed("alpha")
    client.next_search_results = [
        SearchResult(id="d1", score=0.9, vector=v),
        SearchResult(id="d2", score=0.7, vector=None),  # no vector
    ]
    results = store.retrieve("query")
    assert [r.id for r in results] == ["d1"], (
        "results without vectors must be silently skipped"
    )


def test_retrieve_empty_when_no_results() -> None:
    store, client, _ = make_store()
    client.next_search_results = []
    assert store.retrieve("query") == []


def test_retrieve_passes_top_k() -> None:
    store, client, _ = make_store()
    client.next_search_results = []
    store.retrieve("query", top_k=42)
    assert client.searches[0].top_k == 42


# ---- delete() ------------------------------------------------------

def test_delete_forwards_ids_to_client() -> None:
    store, client, _ = make_store()
    store.delete(["d1", "d2"])
    assert client.deletes == [("docs", ["d1", "d2"])]


def test_delete_with_empty_list_is_noop() -> None:
    """Empty delete should not call the client (mirrors add_many)."""
    store, client, _ = make_store()
    store.delete([])
    assert client.deletes == []


# ---- shape of DualUseRetrievalResult -------------------------------

def test_retrieval_result_carries_id_score_and_text() -> None:
    store, client, model = make_store()
    v = model.embed("alpha")
    client.next_search_results = [SearchResult(id="d1", score=0.9, vector=v)]
    results = store.retrieve("alpha")
    assert isinstance(results[0], DualUseRetrievalResult)
    assert results[0].id == "d1"
    assert results[0].score == 0.9
    assert results[0].text == "alpha"
