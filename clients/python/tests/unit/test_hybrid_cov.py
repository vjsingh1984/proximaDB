"""Offline unit tests for proximadb_sdk.hybrid.

Covers fusion strategies (RRF / weighted / cascade), result dataclasses,
the high-level ProximaDBHybrid request shaping + fusion parsing, repository
search/cache behavior, and factory functions. Fully offline: the only
backend is a MagicMock / hand fake.
"""

from __future__ import annotations

import warnings
from datetime import datetime
from unittest.mock import MagicMock

import pytest

from proximadb_sdk.hybrid import (
    CascadeFusion,
    DocumentSearchResult,
    FusionStrategy,
    GraphSearchResult,
    HybridQueryRepository,
    HybridSearchResult,
    JoinType,
    ProximaDBHybrid,
    QueryModel,
    ReciprocalRankFusion,
    TimeSeriesResult,
    VectorSearchResult,
    WeightedFusion,
    _merge_component_metadata,
    _normalize_fusion_inputs,
    _result_id,
    create_fusion_strategy,
    create_hybrid_api,
)


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------


def test_enums_values():
    assert FusionStrategy.RRF.value == "rrf"
    assert FusionStrategy.PROJECTION.value == "projection"
    assert JoinType.SEMANTIC.value == "semantic"
    assert QueryModel.VECTOR.value == "vector"
    assert QueryModel.TIMESERIES.value == "timeseries"


# ---------------------------------------------------------------------------
# Dataclass to_dict / properties
# ---------------------------------------------------------------------------


def test_vector_result_to_dict():
    r = VectorSearchResult(id="a", score=0.9, distance=0.1, rank=2,
                           metadata={"k": "v"}, collection="c")
    d = r.to_dict()
    assert d["id"] == "a"
    assert d["score"] == 0.9
    assert d["model"] == "vector"
    assert d["collection"] == "c"


def test_graph_result_to_dict():
    r = GraphSearchResult(node_id="n1", score=0.5, path=["a", "b"],
                          properties={"p": 1}, labels=["L"], edges=[{"e": 1}],
                          collection="g")
    d = r.to_dict()
    assert d["node_id"] == "n1"
    assert d["id"] == "n1"
    assert d["model"] == "graph"
    assert d["labels"] == ["L"]


def test_document_result_to_dict():
    r = DocumentSearchResult(id="d1", score=0.7, rank=1, highlight=["h"],
                             document={"content": "x"}, metadata={"m": 1})
    d = r.to_dict()
    assert d["id"] == "d1"
    assert d["model"] == "document"
    assert d["highlight"] == ["h"]


def test_timeseries_result_to_dict_with_and_without_ts():
    ts = datetime(2024, 1, 2, 3, 4, 5)
    r = TimeSeriesResult(id="m", score=0.3, timestamp=ts,
                         values={"v": 1}, tags={"t": "x"}, collection="tsc")
    d = r.to_dict()
    assert d["timestamp"] == ts.isoformat()
    assert d["model"] == "timeseries"

    r2 = TimeSeriesResult(id="m2", score=0.1)
    assert r2.to_dict()["timestamp"] is None


def test_hybrid_result_properties_and_to_dict():
    vec = VectorSearchResult(id="x", score=0.8)
    doc = DocumentSearchResult(id="x", score=0.6)
    hr = HybridSearchResult(
        id="x",
        final_score=1.4,
        components={QueryModel.VECTOR.value: vec, QueryModel.DOCUMENT.value: doc},
        rank=1,
        explanation="why",
    )
    assert hr.fused_score == 1.4
    assert hr.vector_score == 0.8
    assert hr.bm25_score == 0.6
    d = hr.to_dict()
    assert d["score"] == 1.4
    assert d["fused_score"] == 1.4
    # nested components were dataclasses -> dict via to_dict
    assert d["components"]["vector"]["id"] == "x"


def test_hybrid_result_properties_dict_components():
    # components stored as plain dicts -> exercise the .get fallback path
    hr = HybridSearchResult(
        id="y",
        final_score=2.0,
        components={
            QueryModel.VECTOR.value: {"score": 0.4},
            QueryModel.DOCUMENT.value: {"score": 0.2},
        },
    )
    assert hr.vector_score == 0.4
    assert hr.bm25_score == 0.2
    # to_dict passes through non-to_dict components unchanged
    d = hr.to_dict()
    assert d["components"]["vector"] == {"score": 0.4}


def test_hybrid_result_properties_missing_components():
    hr = HybridSearchResult(id="z", final_score=0.0)
    assert hr.vector_score == 0.0
    assert hr.bm25_score == 0.0


# ---------------------------------------------------------------------------
# Helper functions
# ---------------------------------------------------------------------------


def test_result_id_helpers():
    assert _result_id(VectorSearchResult(id="a", score=1.0)) == "a"
    assert _result_id({"id": "b"}) == "b"


def test_normalize_fusion_inputs_dict_passthrough():
    d = {"vector": [1], "graph": [2]}
    assert _normalize_fusion_inputs(d) is d


def test_normalize_fusion_inputs_lists():
    out = _normalize_fusion_inputs([1, 2], [3, 4])
    assert out == {"vector": [1, 2], "document": [3, 4]}
    out2 = _normalize_fusion_inputs([1])
    assert out2 == {"vector": [1]}


def test_merge_component_metadata():
    vec = VectorSearchResult(id="a", score=1.0, metadata={"x": 1})
    dct = {"vector": vec, "doc": {"metadata": {"y": 2}}, "none": {"metadata": None}}
    merged = _merge_component_metadata(dct)
    assert merged == {"x": 1, "y": 2}


# ---------------------------------------------------------------------------
# RRF fusion
# ---------------------------------------------------------------------------


def _vecs(ids, scores):
    return [VectorSearchResult(id=i, score=s) for i, s in zip(ids, scores)]


def _docs(ids, scores):
    return [DocumentSearchResult(id=i, score=s) for i, s in zip(ids, scores)]


def test_rrf_fuse_basic():
    rrf = ReciprocalRankFusion(k=10)
    vector = _vecs(["a", "b", "c"], [0.9, 0.8, 0.7])
    document = _docs(["b", "a", "d"], [0.5, 0.4, 0.3])
    fused = rrf.fuse(vector, document)
    ids = [r.id for r in fused]
    assert set(ids) == {"a", "b", "c", "d"}
    assert all(isinstance(r, HybridSearchResult) for r in fused)
    assert fused[0].explanation.startswith("RRF score")
    assert fused[0].components


def test_rrf_fuse_dict_with_weights_and_topk():
    rrf = ReciprocalRankFusion()
    results = {
        "vector": _vecs(["a", "b"], [1.0, 0.5]),
        "document": _docs(["a", "c"], [0.9, 0.4]),
    }
    fused = rrf.fuse(results, weights={"vector": 2.0, "document": 1.0}, top_k=2)
    assert len(fused) == 2
    assert fused[0].id == "a"  # boosted by both lists + vector weight


def test_rrf_empty():
    assert ReciprocalRankFusion().fuse({}) == []


# ---------------------------------------------------------------------------
# Weighted fusion
# ---------------------------------------------------------------------------


def test_weighted_default_weights_via_alpha():
    wf = WeightedFusion(alpha=0.7)
    assert wf.default_weights["vector"] == 0.7
    assert wf.default_weights["document"] == pytest.approx(0.3)


def test_weighted_explicit_weights():
    wf = WeightedFusion(weights={"vector": 0.6, "document": 0.4})
    assert wf.default_weights == {"vector": 0.6, "document": 0.4}


def test_weighted_fuse_normalizes_and_ranks():
    wf = WeightedFusion(weights={"vector": 1.0, "document": 1.0})
    vector = _vecs(["a", "b"], [10.0, 5.0])
    document = _docs(["a", "b"], [2.0, 1.0])
    fused = wf.fuse(vector, document, top_k=2)
    assert fused[0].id == "a"
    assert fused[0].explanation.startswith("Weighted score")
    assert fused[0].final_score == pytest.approx(2.0)


def test_weighted_fuse_dict_input_with_zero_max_and_empty_model():
    wf = WeightedFusion()
    results = {
        "vector": _vecs(["a"], [0.0]),  # max_score == 0 -> set to 1.0
        "document": [],  # empty -> skipped
    }
    fused = wf.fuse(results)
    assert [r.id for r in fused] == ["a"]


def test_weighted_fuse_dict_score_via_get():
    wf = WeightedFusion(weights={"vector": 1.0})
    results = {"vector": [{"id": "a", "score": 4.0}, {"id": "b", "score": 2.0}]}
    fused = wf.fuse(results)
    assert fused[0].id == "a"
    assert fused[0].final_score == pytest.approx(1.0)


# ---------------------------------------------------------------------------
# Cascade fusion
# ---------------------------------------------------------------------------


def test_cascade_fuse_vector_primary():
    cf = CascadeFusion()
    vector = _vecs(["a", "b"], [0.9, 0.7])
    document = _docs(["a", "z"], [0.5, 0.4])
    fused = cf.fuse(vector, document)
    assert [r.id for r in fused] == ["a", "b"]
    assert fused[0].final_score == 0.9
    assert "document" in fused[0].components
    assert fused[0].explanation.startswith("Cascade")


def test_cascade_fuse_no_vector_returns_empty():
    cf = CascadeFusion()
    assert cf.fuse({"document": _docs(["a"], [0.5])}) == []


def test_cascade_topk():
    cf = CascadeFusion()
    fused = cf.fuse(_vecs(["a", "b", "c"], [3, 2, 1]), top_k=1)
    assert len(fused) == 1


# ---------------------------------------------------------------------------
# HybridQueryRepository
# ---------------------------------------------------------------------------


def test_repository_cache_key():
    repo = HybridQueryRepository(client=MagicMock())
    key = repo._build_cache_key(
        [0.1, 0.2],
        "MATCH (n)",
        {"language": "python"},
        (datetime(2024, 1, 1), datetime(2024, 1, 2)),
        FusionStrategy.RRF,
    )
    assert key.startswith("v:")
    assert ":g:" in key
    assert ":d:" in key
    assert ":t:" in key
    assert key.endswith("f:rrf")


def test_repository_cache_key_minimal():
    repo = HybridQueryRepository(client=MagicMock())
    key = repo._build_cache_key(None, None, None, None, FusionStrategy.WEIGHTED)
    assert key == "f:weighted"


def test_repository_search_empty_when_no_tasks():
    repo = HybridQueryRepository(client=MagicMock())
    out = repo.search()
    assert out == []


def test_repository_search_caches_results():
    repo = HybridQueryRepository(client=MagicMock(), cache_ttl=1000)
    out1 = repo.search(fusion_strategy=FusionStrategy.RRF)
    out2 = repo.search(fusion_strategy=FusionStrategy.RRF)
    assert out1 == out2 == []
    assert repo._cache


def test_repository_sql_returns_empty():
    repo = HybridQueryRepository(client=MagicMock())
    assert repo.sql("SELECT 1") == []


def test_repository_private_searches_return_empty():
    import asyncio

    repo = HybridQueryRepository(client=MagicMock())
    loop = asyncio.new_event_loop()
    try:
        assert loop.run_until_complete(repo._vector_search("c", [0.1], 5)) == []
        assert loop.run_until_complete(repo._graph_search("c", "q")) == []
        assert loop.run_until_complete(repo._document_search("c", {})) == []
    finally:
        loop.close()


# ---------------------------------------------------------------------------
# ProximaDBHybrid high-level
# ---------------------------------------------------------------------------


def test_hybrid_resolve_fusion_variants():
    h = ProximaDBHybrid(client=MagicMock())
    assert isinstance(h._resolve_fusion(FusionStrategy.RRF), ReciprocalRankFusion)
    assert isinstance(h._resolve_fusion(FusionStrategy.WEIGHTED), WeightedFusion)
    assert isinstance(h._resolve_fusion(FusionStrategy.CASCADE), CascadeFusion)
    assert isinstance(h._resolve_fusion(FusionStrategy.PROJECTION), ReciprocalRankFusion)
    inst = WeightedFusion()
    assert h._resolve_fusion(inst) is inst
    assert isinstance(h._resolve_fusion(None), ReciprocalRankFusion)


def test_hybrid_mock_result_helpers():
    h = ProximaDBHybrid(client=MagicMock())
    vecs = h._mock_vector_results("c", top_k=3, filters={"lang": "py"})
    assert len(vecs) == 3
    assert vecs[0].metadata["lang"] == "py"
    docs = h._mock_document_results("hello", top_k=2, filters={"x": 1})
    assert len(docs) == 2
    assert docs[0].document["content"] == "hello"


def test_hybrid_search_vector_path_shapes_request():
    client = MagicMock()
    client.search_vectors.return_value = {
        "results": [
            {"id": "v1", "score": 0.9, "metadata": {"m": 1}},
            {"id": "v2", "score": 0.8, "metadata": {}},
        ]
    }
    h = ProximaDBHybrid(client=client)
    out = h.search(
        vector_query=[0.1, 0.2],
        vector_collection="vecs",
        top_k=5,
        document_filter={"lang": "py"},
    )
    client.search_vectors.assert_called_once_with(
        collection="vecs",
        query_vector=[0.1, 0.2],
        top_k=5,
        filters={"lang": "py"},
    )
    assert all(isinstance(r, HybridSearchResult) for r in out)
    assert {r.id for r in out} == {"v1", "v2"}


def test_hybrid_search_text_path_shapes_request():
    client = MagicMock()
    client.query_documents.return_value = {
        "documents": [
            {"id": "d1", "data": {"content": "x"}, "metadata": {"k": 1}},
            {"id": "d2"},
        ]
    }
    h = ProximaDBHybrid(client=client)
    out = h.search(
        text_query="find me",
        document_collection="docs",
        document_filter={"lang": "py"},
        top_k=4,
        fusion_strategy=FusionStrategy.WEIGHTED,
    )
    client.query_documents.assert_called_once_with(
        collection_name="docs",
        filter={"lang": "py"},
        limit=4,
    )
    assert {r.id for r in out} == {"d1", "d2"}


def test_hybrid_search_text_path_default_collection():
    client = MagicMock()
    client.query_documents.return_value = {"documents": [{"id": "d1"}]}
    h = ProximaDBHybrid(client=client)
    h.search(text_query="q")
    assert client.query_documents.call_args.kwargs["collection_name"] == "hybrid_collection"


def test_hybrid_search_combined_vector_and_text():
    client = MagicMock()
    client.search_vectors.return_value = {"results": [{"id": "shared", "score": 0.9}]}
    client.query_documents.return_value = {"documents": [{"id": "shared"}]}
    h = ProximaDBHybrid(client=client)
    out = h.search(
        vector_query=[0.1],
        vector_collection="v",
        text_query="t",
        document_collection="d",
    )
    assert any(r.id == "shared" for r in out)


def test_hybrid_search_vector_error_warns_and_continues():
    client = MagicMock()
    client.search_vectors.side_effect = RuntimeError("boom")
    h = ProximaDBHybrid(client=client)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        out = h.search(vector_query=[0.1], vector_collection="v")
    assert out == []
    assert any("Vector search failed" in str(w.message) for w in caught)


def test_hybrid_search_text_error_warns_and_continues():
    client = MagicMock()
    client.query_documents.side_effect = RuntimeError("nope")
    h = ProximaDBHybrid(client=client)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        out = h.search(text_query="t")
    assert out == []
    assert any("Document search failed" in str(w.message) for w in caught)


def test_hybrid_search_query_vector_alias_and_filters_alias():
    client = MagicMock()
    client.search_vectors.return_value = {"results": [{"id": "v1", "score": 0.5}]}
    h = ProximaDBHybrid(client=client)
    h.search(query_vector=[0.3], vector_collection="v", filters={"a": 1})
    assert client.search_vectors.call_args.kwargs["filters"] == {"a": 1}


def test_hybrid_search_delegates_to_repository_for_graph_only():
    # No vector_query and no text_query -> repository path
    client = MagicMock()
    h = ProximaDBHybrid(client=client)
    out = h.search(graph_query="MATCH (n)", graph_collection="g")
    assert out == []


def test_hybrid_sql_delegates():
    h = ProximaDBHybrid(client=MagicMock())
    assert h.sql("SELECT 1", params=[1]) == []


def test_hybrid_clear_cache():
    h = ProximaDBHybrid(client=MagicMock())
    h._repository._cache["k"] = ([], 0.0)
    h.clear_cache()
    assert h._repository._cache == {}


def test_hybrid_list_strategies():
    h = ProximaDBHybrid(client=MagicMock())
    strategies = h.list_strategies()
    ids = {s["id"] for s in strategies}
    assert "rrf" in ids
    assert "borda_count" in ids


# ---------------------------------------------------------------------------
# Factory functions
# ---------------------------------------------------------------------------


def test_create_hybrid_api():
    api = create_hybrid_api(MagicMock(), cache_ttl=10, default_fusion=FusionStrategy.WEIGHTED)
    assert isinstance(api, ProximaDBHybrid)
    assert api._default_fusion == FusionStrategy.WEIGHTED


def test_create_fusion_strategy_all():
    assert isinstance(create_fusion_strategy(FusionStrategy.RRF, k=5), ReciprocalRankFusion)
    assert create_fusion_strategy(FusionStrategy.RRF, k=5).k == 5
    wf = create_fusion_strategy(FusionStrategy.WEIGHTED, weights={"vector": 1.0})
    assert isinstance(wf, WeightedFusion)
    assert isinstance(create_fusion_strategy(FusionStrategy.CASCADE), CascadeFusion)


def test_create_fusion_strategy_unknown_raises():
    with pytest.raises(ValueError):
        create_fusion_strategy(FusionStrategy.LEARNED)
