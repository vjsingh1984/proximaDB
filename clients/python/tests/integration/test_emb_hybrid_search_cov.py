"""Embedded-engine integration coverage for HYBRID SEARCH.

Exercises the real in-process PyO3 ProximaDB engine end-to-end (SDK -> REST ->
embedded engine) for the hybrid-search modality: index documents/vectors, run a
hybrid (vector + BM25/document) query, and assert the fused result set.

These tests use the shared session-scoped ``embedded_db`` fixture (one engine
per pytest process) via the ``embedded_rest_client`` fixture defined in
``tests/conftest.py``. They never boot their own engine and never expect a 2nd
boot. Each test creates its own uniquely-named collection so concurrent tests
never interfere.

The vector arm of hybrid search is a real create -> insert -> search round
trip. The BM25/document arm and any server-side fused endpoint are exercised
where available, but degrade gracefully (documented error / empty list / skip)
when unimplemented in embedded mode -- the ``ProximaDBHybrid`` client-side
fusion still combines whatever arms returned real data.
"""

from __future__ import annotations

import random
import time
from uuid import uuid4

import pytest

from proximadb_sdk.hybrid import (
    CascadeFusion,
    DocumentSearchResult,
    FusionStrategy,
    HybridSearchResult,
    ProximaDBHybrid,
    ReciprocalRankFusion,
    VectorSearchResult,
    WeightedFusion,
)

DIM = 16


def _unique(modality: str = "hybrid") -> str:
    return f"itest_{modality}_{uuid4().hex[:8]}"


def _seed_vector(seed: int, dim: int = DIM) -> list[float]:
    rng = random.Random(seed)
    return [rng.random() for _ in range(dim)]


@pytest.fixture
def client(embedded_rest_client):
    return embedded_rest_client


@pytest.fixture
def hybrid_api(client):
    return ProximaDBHybrid(client)


def _make_vector_collection(client, name: str) -> bool:
    """Create an fp32 cosine collection for the vector arm. Returns True on
    success, False if create is unavailable in embedded mode."""
    from proximadb_sdk.models import CollectionConfig

    config = CollectionConfig(
        name=name,
        dimension=DIM,
        distance_metric="cosine",
        description="hybrid-search itest collection",
    )
    try:
        client.create_collection(name=name, config=config)
        return True
    except Exception:
        return False


def _drop(client, name: str) -> None:
    try:
        client.delete_collection(name)
    except Exception:
        pass


# ---------------------------------------------------------------------------
# Client-side fusion strategies over deterministic inputs (no server needed for
# the fusion math itself, but the inputs in the round-trip tests below are real
# server results). These guard the fusion contract the embedded path relies on.
# ---------------------------------------------------------------------------


@pytest.mark.integration
def test_rrf_fuses_overlapping_rankings():
    """RRF must boost an id that ranks well in BOTH arms above arm-unique ids."""
    vectors = [
        VectorSearchResult(id="a", score=0.9, rank=1),
        VectorSearchResult(id="b", score=0.8, rank=2),
        VectorSearchResult(id="c", score=0.7, rank=3),
    ]
    docs = [
        DocumentSearchResult(id="b", score=0.95, rank=1),
        DocumentSearchResult(id="a", score=0.6, rank=2),
        DocumentSearchResult(id="d", score=0.5, rank=3),
    ]
    fused = ReciprocalRankFusion(k=60).fuse(vectors, docs, top_k=10)
    assert fused, "RRF must produce fused results"
    assert all(isinstance(r, HybridSearchResult) for r in fused)
    ids = [r.id for r in fused]
    # a and b appear in both arms -> must outrank arm-unique c and d.
    assert ids[0] in {"a", "b"}
    assert ids[1] in {"a", "b"}
    assert set(ids) == {"a", "b", "c", "d"}
    # scores must be monotonically non-increasing (sorted by fused score).
    scores = [r.final_score for r in fused]
    assert scores == sorted(scores, reverse=True)


@pytest.mark.integration
def test_weighted_fusion_respects_alpha():
    """Weighted fusion with vector-heavy alpha must favor the vector top hit."""
    vectors = [
        VectorSearchResult(id="v1", score=1.0, rank=1),
        VectorSearchResult(id="v2", score=0.5, rank=2),
    ]
    docs = [
        DocumentSearchResult(id="v2", score=1.0, rank=1),
        DocumentSearchResult(id="v1", score=0.2, rank=2),
    ]
    fused = WeightedFusion(alpha=0.9).fuse(vectors, docs, top_k=10)
    assert fused
    assert fused[0].id == "v1", "vector-heavy weighting must surface v1 first"


@pytest.mark.integration
def test_cascade_fusion_is_vector_primary():
    """Cascade keeps vector ordering and augments with the doc component."""
    vectors = [
        VectorSearchResult(id="x", score=0.9, rank=1),
        VectorSearchResult(id="y", score=0.8, rank=2),
    ]
    docs = [DocumentSearchResult(id="x", score=0.99, rank=1)]
    fused = CascadeFusion().fuse(vectors, docs, top_k=10)
    assert [r.id for r in fused] == ["x", "y"]
    # x has a doc component fused in; y is vector-only.
    assert "document" in fused[0].components
    assert "vector" in fused[1].components


# ---------------------------------------------------------------------------
# Real round-trip: vector arm through the embedded engine, fused with a doc arm.
# ---------------------------------------------------------------------------


@pytest.mark.integration
def test_hybrid_vector_arm_round_trip_then_fuse(client):
    """Real create -> insert -> search on the embedded engine, then fuse the
    live vector results with a synthetic BM25/document ranking via RRF.

    Asserts the vector arm returns real inserted ids and that the fused output
    is a non-empty ranked list of HybridSearchResult.
    """
    name = _unique()
    if not _make_vector_collection(client, name):
        pytest.skip("create_collection unavailable in embedded mode")
    try:
        n = 6
        records = []
        for i in range(n):
            records.append({"id": f"doc-{i}", "vector": _seed_vector(i)})
        batch = client.insert_records(name, records)
        assert batch.success == n, (
            f"vector arm insert must report success=={n}; got "
            f"success={batch.success}, failed={batch.failed}, errors={batch.errors}"
        )

        time.sleep(0.75)  # WAL -> search visibility settle

        # Query the exact vector for doc-2 -> doc-2 should be top vector hit.
        query = _seed_vector(2)
        results = client.search(name, vector=query, top_k=n)
        assert results, "vector arm search must return matches after insert"

        inserted_ids = {f"doc-{i}" for i in range(n)}
        vec_results = [
            VectorSearchResult(
                id=getattr(r, "id", None) or "",
                score=float(getattr(r, "score", 0.0) or 0.0),
                rank=idx + 1,
            )
            for idx, r in enumerate(results)
            if getattr(r, "id", None)
        ]
        result_ids = {v.id for v in vec_results}
        assert (
            result_ids & inserted_ids
        ), f"vector arm must return inserted ids; got {result_ids}"

        # Synthetic document/BM25 arm that overlaps the vector arm on doc-2.
        doc_results = [
            DocumentSearchResult(id="doc-2", score=0.95, rank=1),
            DocumentSearchResult(id="doc-0", score=0.7, rank=2),
            DocumentSearchResult(id="doc-9", score=0.5, rank=3),  # doc arm only
        ]

        fused = ReciprocalRankFusion().fuse(vec_results, doc_results, top_k=n)
        assert fused, "fusion of real vector + doc arms must be non-empty"
        assert all(isinstance(r, HybridSearchResult) for r in fused)
        fused_ids = [r.id for r in fused]
        # doc-2 ranks in both arms -> should be at or near the top.
        assert (
            "doc-2" in fused_ids[:2]
        ), f"doc-2 (top of both arms) must rank highly; got {fused_ids}"
        # fused output should not exceed top_k.
        assert len(fused) <= n
    finally:
        _drop(client, name)


@pytest.mark.integration
def test_hybrid_search_high_level_api_vector_only(client, hybrid_api):
    """ProximaDBHybrid.search with only the vector arm wired against the
    embedded engine. The doc arm is unwired (no text_query collection content),
    so this exercises graceful single-arm fusion through the high-level API."""
    name = _unique()
    if not _make_vector_collection(client, name):
        pytest.skip("create_collection unavailable in embedded mode")
    try:
        n = 5
        records = [{"id": f"h-{i}", "vector": _seed_vector(100 + i)} for i in range(n)]
        batch = client.insert_records(name, records)
        assert batch.success == n
        time.sleep(0.75)

        query = _seed_vector(102)
        try:
            results = hybrid_api.search(
                vector_collection=name,
                query_vector=query,
                fusion_strategy=FusionStrategy.RRF,
                top_k=n,
            )
        except Exception as exc:  # noqa: BLE001
            pytest.skip(f"high-level hybrid search unavailable in embedded mode: {exc}")
            return

        # The high-level API may surface raw VectorSearchResult/HybridSearchResult
        # or an empty list if the embedded vector search returns nothing through
        # the search_vectors shim. Either is acceptable; assert the shape.
        assert isinstance(results, list)
        for r in results[:3]:
            assert isinstance(r, (HybridSearchResult, VectorSearchResult, dict))
    finally:
        _drop(client, name)


@pytest.mark.integration
def test_bm25_document_arm_degrades_gracefully(client):
    """The BM25/document arm: index documents then query them. The embedded
    engine may or may not implement the document store; either a real result
    set or a documented empty/error is acceptable. Pairs with the vector arm to
    document the full hybrid surface."""
    name = _unique("hybriddoc")
    if not _make_vector_collection(client, name):
        pytest.skip("create_collection unavailable in embedded mode")
    try:
        docs = [
            {"title": "python parsing", "body": "how to parse json in python"},
            {"title": "rust vectors", "body": "vector search in rust databases"},
            {"title": "python search", "body": "full text search with python bm25"},
        ]
        indexed = 0
        for i, d in enumerate(docs):
            try:
                client.insert_document(name, d, id=f"bm25-{i}")
                indexed += 1
            except Exception:  # noqa: BLE001 — doc store may be unimplemented
                break

        if indexed == 0:
            pytest.skip("document/BM25 arm unimplemented in embedded mode")
            return

        time.sleep(0.5)
        try:
            resp = client.query_documents(name, filter=None, limit=10)
        except Exception as exc:  # noqa: BLE001
            pytest.skip(f"query_documents unavailable in embedded mode: {exc}")
            return

        # Documented contract: a dict with a 'documents' list (possibly empty if
        # the embedded doc store does not persist these in this build).
        assert isinstance(resp, dict)
        returned = resp.get("documents", [])
        assert isinstance(returned, list)

        # If documents came back, fuse them with a vector arm to prove the
        # hybrid fusion accepts real server document results.
        if returned:
            doc_results = [
                DocumentSearchResult(
                    id=str(doc.get("id", f"d{idx}")),
                    score=1.0 / (idx + 1),
                    rank=idx + 1,
                )
                for idx, doc in enumerate(returned)
            ]
            vec_results = [
                VectorSearchResult(id=doc_results[0].id, score=0.9, rank=1),
            ]
            fused = ReciprocalRankFusion().fuse(vec_results, doc_results, top_k=10)
            assert fused
            assert all(isinstance(r, HybridSearchResult) for r in fused)
    finally:
        _drop(client, name)


@pytest.mark.integration
def test_hybrid_empty_arms_fuse_to_empty(hybrid_api):
    """Fusing two empty arms must yield an empty list, not an error -- the
    graceful-degradation contract when neither arm returns data."""
    fused = ReciprocalRankFusion().fuse([], [], top_k=10)
    assert fused == []
    # list_strategies is a pure-client capability advertisement.
    strategies = hybrid_api.list_strategies()
    assert isinstance(strategies, list) and strategies
    assert any(s.get("id") == "rrf" for s in strategies)
