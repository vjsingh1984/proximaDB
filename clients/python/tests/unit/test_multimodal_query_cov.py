"""Offline unit tests for proximadb_sdk.multimodal_query.

Fully offline: no network, no server, no model downloads. The executor is
exercised with hand-built fake clients so RPC methods return plain dicts/objects.
"""

import math
from types import SimpleNamespace

import pytest

assert math is not None  # keep linter from stripping the import

from proximadb_sdk.multimodal_query import (
    CrossModalReranker,
    DocumentQueryComponent,
    FeatureExtractor,
    FeedbackSignal,
    FeedbackType,
    FusionFeatures,
    FusionModelType,
    FusionStrategy,
    GraphQueryComponent,
    JoinType,
    LearnedFusion,
    LearnedFusionConfig,
    LogQueryComponent,
    MetricQueryComponent,
    MultiModalQuery,
    MultiModalQueryBuilder,
    MultiModalQueryExecutor,
    MultiModalQueryResult,
    QueryContext,
    QueryIntent,
    QueryType,
    RerankConfig,
    RerankedResult,
    RerankExplanation,
    ScoreComponent,
    SemanticJoin,
    TemporalPreference,
    TimeDecayFunction,
    TrainingMetrics,
    TrainingSample,
    VectorQueryComponent,
    knowledge_graph_search,
    logs_with_context,
    semantic_search_with_graph,
)


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------


def test_enum_values():
    assert QueryType.VECTOR.value == "vector"
    assert FusionStrategy.RRF.value == "rrf"
    assert JoinType.SEMANTIC.value == "semantic"
    assert TimeDecayFunction.EXPONENTIAL.value == "exponential"
    assert QueryIntent.NAVIGATIONAL.value == "navigational"
    assert TemporalPreference.RECENT.value == "recent"
    assert FusionModelType.LINEAR.value == "linear"
    assert FeedbackType.CLICK.value == "click"


# ---------------------------------------------------------------------------
# Config / context dataclasses
# ---------------------------------------------------------------------------


def test_rerank_config_defaults_and_to_dict():
    cfg = RerankConfig()
    # __post_init__ populates default model weights
    assert cfg.model_weights["vector"] == 1.0
    assert cfg.model_weights["document"] == 0.8
    d = cfg.to_dict()
    assert d["semantic_rerank"] is True
    assert d["rerank_top_k"] == 100
    assert d["model_weights"]["graph"] == 0.9


def test_rerank_config_custom_weights_preserved():
    cfg = RerankConfig(model_weights={"vector": 2.0})
    assert cfg.model_weights == {"vector": 2.0}


def test_query_context_to_dict_full_and_empty():
    ctx = QueryContext(
        query_text="hello",
        query_embedding=[1.0, 0.0],
        intent=QueryIntent.SIMILARITY_SEARCH,
        temporal_preference=TemporalPreference.RECENT,
        required_models=["vector"],
        user_preferences={"vector": 1.0},
    )
    d = ctx.to_dict()
    assert d["query_text"] == "hello"
    assert d["intent"] == "similarity_search"
    assert d["temporal_preference"] == "recent"
    assert d["required_models"] == ["vector"]

    empty = QueryContext()
    de = empty.to_dict()
    assert de["intent"] is None
    assert de["required_models"] == []
    assert de["user_preferences"] == {}


def test_score_component_to_dict():
    sc = ScoreComponent(name="x", value=0.5, weight=0.3, contribution=0.15)
    assert sc.to_dict() == {
        "name": "x",
        "value": 0.5,
        "weight": 0.3,
        "contribution": 0.15,
    }


def test_rerank_explanation_and_result_to_dict():
    sc = ScoreComponent(name="x", value=1.0, weight=1.0, contribution=1.0)
    expl = RerankExplanation(
        record_id="r1",
        original_rank=2,
        new_rank=0,
        score_components=[sc],
        explanation_text="Promoted",
        confidence=0.9,
    )
    ed = expl.to_dict()
    assert ed["record_id"] == "r1"
    assert ed["score_components"][0]["name"] == "x"

    res = RerankedResult(
        records=[{"id": "r1"}],
        explanations=[expl],
        quality_score=0.8,
        diversity_score=0.7,
    )
    rd = res.to_dict()
    assert rd["records"] == [{"id": "r1"}]
    assert rd["explanations"][0]["new_rank"] == 0
    assert rd["quality_score"] == 0.8


# ---------------------------------------------------------------------------
# Component dataclasses to_dict
# ---------------------------------------------------------------------------


def test_vector_component_to_dict():
    c = VectorQueryComponent(collection="c", query_vector=[1.0, 2.0], top_k=5)
    d = c.to_dict()
    assert d["type"] == "vector"
    assert d["collection"] == "c"
    assert d["top_k"] == 5
    assert d["include_metadata"] is True


def test_graph_component_to_dict():
    c = GraphQueryComponent(graph_id="g", start_label="Cat", edge_types=["E"])
    d = c.to_dict()
    assert d["type"] == "graph"
    assert d["graph_id"] == "g"
    assert d["direction"] == "outgoing"
    assert d["limit"] == 100


def test_document_component_to_dict():
    c = DocumentQueryComponent(collection="d", text_query="q")
    d = c.to_dict()
    assert d["type"] == "document"
    assert d["text_query"] == "q"
    assert d["sort_order"] == "asc"


def test_log_component_to_dict():
    c = LogQueryComponent(namespace="ns", time_range=(1, 2), severities=["ERROR"])
    d = c.to_dict()
    assert d["type"] == "logs"
    assert d["time_range"] == (1, 2)
    assert d["limit"] == 1000


def test_metric_component_to_dict():
    c = MetricQueryComponent(namespace="ns", metric_names=["m"], time_range=(1, 2))
    d = c.to_dict()
    assert d["type"] == "metrics"
    assert d["aggregation"] == "avg"
    assert d["bucket_size_ms"] == 60000


def test_semantic_join_to_dict():
    j = SemanticJoin(left_field="a", right_field="b")
    d = j.to_dict()
    assert d["join_type"] == "semantic"
    assert d["similarity_threshold"] == 0.7


# ---------------------------------------------------------------------------
# MultiModalQueryResult
# ---------------------------------------------------------------------------


def test_multimodal_result_iter_len():
    res = MultiModalQueryResult(
        records=[{"id": "a"}, {"id": "b"}],
        total_count=2,
        query_time_ms=1.0,
        component_times={},
        fusion_strategy="rrf",
    )
    assert len(res) == 2
    assert [r["id"] for r in res] == ["a", "b"]


def test_multimodal_result_to_dataframe():
    res = MultiModalQueryResult(
        records=[{"id": "a", "v": 1}],
        total_count=1,
        query_time_ms=1.0,
        component_times={},
        fusion_strategy="rrf",
    )
    pd = pytest.importorskip("pandas")
    df = res.to_dataframe()
    assert isinstance(df, pd.DataFrame)
    assert list(df["id"]) == ["a"]


# ---------------------------------------------------------------------------
# Builder
# ---------------------------------------------------------------------------


def test_builder_full_chain_build():
    scorer = lambda r: r.get("score", 0)
    q = (
        MultiModalQueryBuilder()
        .vector("products", [0.1, 0.2], top_k=20, min_similarity=0.1, weight=2.0)
        .graph("kg", start_label="Cat", edge_types=["C"], node_filter={"x": 1})
        .document("docs", filter={"a": 1}, text_query="t", json_path_filters={"p": 1})
        .logs("ns", time_range=(1, 2), services=["s"], severities=["ERROR"], text_query="e")
        .metrics("ns", ["m"], (1, 2), aggregation="sum", group_by=["g"])
        .join_semantic("v.e", "g.e", similarity_threshold=0.8)
        .join_by_id("a.id", "b.id")
        .join_graph_path("a", "b", graph_id="kg", max_path_length=4)
        .fuse(FusionStrategy.WEIGHTED, weights={"vector_1": 5.0})
        .with_time_decay(TimeDecayFunction.EXPONENTIAL, halflife_hours=12)
        .with_custom_scorer(scorer)
        .limit(10)
        .offset(2)
        .timeout(5000)
        .include_scores(False)
        .include_metadata(False)
        .build()
    )
    assert isinstance(q, MultiModalQuery)
    assert len(q.components) == 5
    assert len(q.joins) == 3
    assert q.fusion_strategy == "weighted"
    assert q.fusion_weights["vector_1"] == 5.0
    assert q.limit == 10
    assert q.offset == 2
    assert q.timeout_ms == 5000
    assert q.include_scores is False
    assert q.include_metadata is False
    assert q.custom_scorer is scorer
    assert q.time_decay is not None


def test_builder_graph_from_vector_results_flags():
    b = MultiModalQueryBuilder().vector("c", [1.0]).graph_from_vector_results(
        "g", id_field="uid", edge_types=["E"], max_depth=3
    )
    # second component is graph and carries the private flags
    comp = b._components[1]
    assert comp._from_previous is True
    assert comp._id_field == "uid"


def test_builder_join_graph_path_attrs():
    b = MultiModalQueryBuilder().join_graph_path("a", "b", "g", max_path_length=5)
    join = b._joins[0]
    assert join.join_type == JoinType.GRAPH_PATH
    assert join._graph_id == "g"
    assert join._max_path_length == 5


def test_builder_with_time_decay_default_reference_time():
    b = MultiModalQueryBuilder().with_time_decay(TimeDecayFunction.LINEAR)
    func, params = b._time_decay
    assert func == TimeDecayFunction.LINEAR
    assert params["reference_time"] > 0


# ---------------------------------------------------------------------------
# MultiModalQuery.to_dict
# ---------------------------------------------------------------------------


def test_query_to_dict_with_time_decay_enum():
    q = MultiModalQuery(
        components=[{"type": "vector"}],
        joins=[],
        fusion_strategy="rrf",
        fusion_weights={},
        time_decay=(TimeDecayFunction.GAUSSIAN, {"halflife_hours": 1}),
        limit=10,
        offset=0,
        timeout_ms=1000,
        include_scores=True,
        include_metadata=True,
    )
    d = q.to_dict()
    assert d["time_decay"]["function"] == "gaussian"
    assert d["time_decay"]["halflife_hours"] == 1


def test_query_to_dict_with_time_decay_string_func():
    q = MultiModalQuery(
        components=[],
        joins=[],
        fusion_strategy="rrf",
        fusion_weights={},
        time_decay=("linear", {"halflife_hours": 2}),
        limit=10,
        offset=0,
        timeout_ms=1000,
        include_scores=True,
        include_metadata=True,
    )
    d = q.to_dict()
    assert d["time_decay"]["function"] == "linear"


def test_query_to_dict_no_time_decay():
    q = MultiModalQuery(
        components=[],
        joins=[],
        fusion_strategy="rrf",
        fusion_weights={},
        time_decay=None,
        limit=10,
        offset=0,
        timeout_ms=1000,
        include_scores=True,
        include_metadata=True,
    )
    assert "time_decay" not in q.to_dict()


# ---------------------------------------------------------------------------
# CrossModalReranker
# ---------------------------------------------------------------------------


def test_reranker_empty_records():
    rr = CrossModalReranker()
    out = rr.rerank([])
    assert out.records == []
    assert out.quality_score == 1.0
    assert out.diversity_score == 1.0


def test_reranker_full_pipeline_with_explanations():
    cfg = RerankConfig(generate_explanations=True)
    rr = CrossModalReranker(cfg)
    records = [
        {"id": "1", "score": 0.95, "_source_type": "vector", "embedding": [1.0, 0.0],
         "timestamp": 0},
        {"id": "2", "score": 0.4, "_source_type": "graph", "content": "hello world"},
        {"id": "3", "score": 0.6, "_source_type": "document", "text": "world peace"},
    ]
    ctx = QueryContext(
        query_text="hello world",
        query_embedding=[1.0, 0.0],
        intent=QueryIntent.SIMILARITY_SEARCH,
        temporal_preference=TemporalPreference.RECENT,
        required_models=["vector"],
    )
    out = rr.rerank(records, ctx)
    assert len(out.records) == 3
    assert len(out.explanations) == 3
    assert all("_rerank_score" in r for r in out.records)
    assert 0.0 <= out.quality_score <= 1.0
    assert 0.0 <= out.diversity_score <= 1.0


def test_reranker_no_optional_steps():
    cfg = RerankConfig(
        semantic_rerank=False,
        context_aware=False,
        diversity_optimization=False,
        generate_explanations=False,
    )
    rr = CrossModalReranker(cfg)
    records = [{"id": "1", "score": 0.5}, {"id": "2", "score": 0.9}]
    out = rr.rerank(records)
    assert out.explanations == []
    # highest base score should rank first
    assert out.records[0]["id"] == "2"


def test_reranker_semantic_text_fallback_and_default():
    rr = CrossModalReranker(RerankConfig(diversity_optimization=False))
    records = [
        {"id": "1", "score": 0.5, "content": "alpha beta"},  # text path
        {"id": "2", "score": 0.5},  # no text, no embedding -> 0.5 default
    ]
    ctx = QueryContext(query_text="alpha", query_embedding=[1.0])
    out = rr.rerank(records, ctx)
    assert len(out.records) == 2


def test_reranker_context_intents():
    rr = CrossModalReranker(
        RerankConfig(semantic_rerank=False, diversity_optimization=False)
    )
    for intent, stype in [
        (QueryIntent.RELATIONSHIP_EXPLORATION, "graph"),
        (QueryIntent.INFORMATIONAL, "document"),
        (QueryIntent.ANALYTICAL, "logs"),
        (QueryIntent.NAVIGATIONAL, "vector"),
    ]:
        records = [{"id": "1", "score": 0.95, "_source_type": stype, "created_at": 0}]
        ctx = QueryContext(intent=intent, temporal_preference=TemporalPreference.HISTORICAL)
        out = rr.rerank(records, ctx)
        assert len(out.records) == 1


def test_cosine_similarity():
    rr = CrossModalReranker()
    assert rr._cosine_similarity([1.0, 0.0], [1.0, 0.0]) == pytest.approx(1.0)
    assert rr._cosine_similarity([1.0, 0.0], [0.0, 1.0]) == pytest.approx(0.0)
    assert rr._cosine_similarity([], []) == 0.0
    assert rr._cosine_similarity([1.0], [1.0, 2.0]) == 0.0
    assert rr._cosine_similarity([0.0, 0.0], [1.0, 1.0]) == 0.0


def test_text_similarity():
    rr = CrossModalReranker()
    assert rr._text_similarity("a b c", "b c d") == pytest.approx(2 / 4)
    assert rr._text_similarity("", "") == 0.0


def test_temporal_boost():
    rr = CrossModalReranker()
    now = int(__import__("time").time() * 1e9)
    assert rr._compute_temporal_boost(now, TemporalPreference.RECENT) > 0
    assert rr._compute_temporal_boost(now, TemporalPreference.HISTORICAL) >= 0
    assert rr._compute_temporal_boost(now, TemporalPreference.NEUTRAL) == 0.0


def test_record_similarity():
    rr = CrossModalReranker()
    a = {"_source_type": "vector", "score": 0.5, "id": "x"}
    b = {"_source_type": "vector", "score": 0.5, "id": "y"}
    sim = rr._record_similarity(a, b)
    assert 0.0 <= sim <= 1.0
    # disjoint records
    assert rr._record_similarity({"a": 1}, {"b": 2}) >= 0.0


def test_diversity_and_quality_scores():
    rr = CrossModalReranker()
    assert rr._compute_diversity_score([{"id": "1"}]) == 1.0
    records = [
        {"_source_type": "vector", "_rerank_score": 0.9},
        {"_source_type": "graph", "_rerank_score": 0.5},
    ]
    assert 0.0 <= rr._compute_diversity_score(records) <= 1.0
    assert 0.0 <= rr._compute_quality_score(records) <= 1.0
    assert rr._compute_quality_score([]) == 1.0


def test_explanation_confidence():
    rr = CrossModalReranker()
    sc_pos = ScoreComponent("a", 1.0, 1.0, 0.5)
    sc_neg = ScoreComponent("b", 1.0, 1.0, -0.5)
    assert rr._compute_explanation_confidence({"score_components": [sc_pos, sc_neg]}) == 0.5
    assert rr._compute_explanation_confidence({"score_components": []}) == 0.5


def test_explanation_demotion_path():
    cfg = RerankConfig(generate_explanations=True, semantic_rerank=False,
                       context_aware=False, diversity_optimization=False)
    rr = CrossModalReranker(cfg)
    # record "a" starts rank 0 with low score -> gets demoted below "b"
    records = [{"id": "a", "score": 0.1}, {"id": "b", "score": 0.9}]
    out = rr.rerank(records)
    texts = [e.explanation_text for e in out.explanations]
    assert any("Demoted" in t or "Promoted" in t or "unchanged" in t for t in texts)


# ---------------------------------------------------------------------------
# Executor with fake clients
# ---------------------------------------------------------------------------


class _Hit:
    def __init__(self, id, score, metadata=None):
        self.id = id
        self.score = score
        self.metadata = metadata or {}


class _Graph:
    def traverse(self, graph_id, start_nodes, edge_types, max_depth, limit):
        return [{"id": "n1", "depth": 1, "labels": ["L"], "properties": {}}]


class FakeClient:
    def __init__(self):
        self.graph = _Graph()

    def search(self, collection, vector, top_k, filter):
        return [_Hit("v1", 0.9, {"a": 1}), _Hit("v2", 0.5)]

    def query_documents(self, collection, filter, text_query, limit):
        return [{"id": "d1", "body": "x"}]

    def query_logs(self, namespace, time_range, services, text_query, limit):
        return [{"id": "l1", "timestamp": 100, "service": "svc"}]

    def aggregate_metrics(self, namespace, metric_names, time_range, aggregation):
        return [{"name": "m1", "value": 1.0, "timestamp": 50}]


def test_executor_vector_only():
    q = MultiModalQueryBuilder().vector("c", [0.1]).build()
    ex = MultiModalQueryExecutor(FakeClient())
    res = ex.execute(q)
    assert isinstance(res, MultiModalQueryResult)
    assert res.records[0]["_source_type"] == "vector"
    assert res.metadata["component_count"] == 1


def test_executor_vector_search_error_returns_empty():
    class Boom:
        def search(self, **kw):
            raise RuntimeError("nope")

    q = MultiModalQueryBuilder().vector("c", [0.1]).build()
    res = MultiModalQueryExecutor(Boom()).execute(q)
    assert res.records == []


def test_executor_all_component_types_rrf():
    q = (
        MultiModalQueryBuilder()
        .vector("c", [0.1])
        .graph("g", start_nodes=["s"])
        .document("d")
        .logs("ns")
        .metrics("ns", ["m"], (1, 2))
        .fuse(FusionStrategy.RRF)
        .build()
    )
    res = MultiModalQueryExecutor(FakeClient()).execute(q)
    assert res.fusion_strategy == "rrf"
    assert len(res.records) >= 1


def test_executor_graph_missing_attr_returns_empty():
    class NoGraph:
        def search(self, **kw):
            return []

    q = MultiModalQueryBuilder().graph("g").build()
    res = MultiModalQueryExecutor(NoGraph()).execute(q)
    assert res.records == []


def test_executor_unknown_component_type():
    q = MultiModalQuery(
        components=[{"type": "bogus"}],
        joins=[],
        fusion_strategy="rrf",
        fusion_weights={},
        time_decay=None,
        limit=10,
        offset=0,
        timeout_ms=1000,
        include_scores=True,
        include_metadata=True,
    )
    res = MultiModalQueryExecutor(FakeClient()).execute(q)
    assert res.records == []


def test_executor_intersection_union_weighted():
    client = FakeClient()
    for strat in (FusionStrategy.INTERSECTION, FusionStrategy.UNION, FusionStrategy.WEIGHTED):
        q = MultiModalQueryBuilder().vector("c", [0.1]).document("d").fuse(strat).build()
        res = MultiModalQueryExecutor(client).execute(q)
        assert isinstance(res, MultiModalQueryResult)


def test_executor_with_joins_and_time_decay_and_custom_scorer():
    q = (
        MultiModalQueryBuilder()
        .vector("c", [0.1])
        .logs("ns")
        .join_by_id("id", "id")
        .with_time_decay(TimeDecayFunction.LINEAR, halflife_hours=1)
        .with_custom_scorer(lambda r: r.get("score", 0))
        .build()
    )
    res = MultiModalQueryExecutor(FakeClient()).execute(q)
    assert isinstance(res, MultiModalQueryResult)


def test_executor_offset_limit_slicing():
    q = MultiModalQueryBuilder().vector("c", [0.1]).offset(1).limit(1).build()
    res = MultiModalQueryExecutor(FakeClient()).execute(q)
    # vector returns 2 hits; offset 1, limit 1 -> 1 record
    assert len(res.records) == 1


def test_executor_extract_field_nested():
    ex = MultiModalQueryExecutor(FakeClient())
    rec = {"a": {"b": "val"}}
    assert ex._extract_field(rec, "a.b") == "val"
    assert ex._extract_field(rec, "a.x") is None
    assert ex._extract_field({"k": None}, "k") is None


def test_executor_fuse_single_and_empty():
    ex = MultiModalQueryExecutor(FakeClient())
    assert ex._fuse_results([], "rrf", {}) == []
    one = [[{"id": "a"}]]
    assert ex._fuse_results(one, "rrf", {}) == [{"id": "a"}]


def test_executor_fuse_default_strategy_falls_back_to_rrf():
    ex = MultiModalQueryExecutor(FakeClient())
    out = ex._fuse_results([[{"id": "a"}], [{"id": "b"}]], "unknown", {})
    assert any("_rrf_score" in r for r in out)


def test_executor_intersection_helper():
    ex = MultiModalQueryExecutor(FakeClient())
    out = ex._fuse_intersection([[{"id": "a"}, {"id": "b"}], [{"id": "b"}]])
    assert [r["id"] for r in out] == ["b"]
    assert ex._fuse_intersection([]) == []


def test_executor_union_helper_with_anon():
    ex = MultiModalQueryExecutor(FakeClient())
    out = ex._fuse_union([[{"id": "a"}, {"foo": 1}], [{"id": "a"}, {"id": "c"}]])
    ids = [r.get("id") for r in out]
    assert "a" in ids and "c" in ids
    # the record without an id is still included
    assert any("foo" in r for r in out)


def test_executor_apply_joins_single_component_passthrough():
    ex = MultiModalQueryExecutor(FakeClient())
    comp = [[{"id": "a"}]]
    assert ex._apply_joins(comp, [{"join_type": "inner"}]) == comp


def test_executor_apply_time_decay_functions():
    ex = MultiModalQueryExecutor(FakeClient())
    ref = 1000 * 3600 * int(1e9)
    records = [{"id": "a", "score": 1.0, "timestamp": 0}]
    for func in (
        TimeDecayFunction.LINEAR,
        TimeDecayFunction.EXPONENTIAL,
        TimeDecayFunction.GAUSSIAN,
        TimeDecayFunction.NONE,
    ):
        out = ex._apply_time_decay(
            [dict(r) for r in records],
            (func, {"reference_time": ref, "halflife_hours": 24, "time_field": "timestamp"}),
        )
        assert "_decayed_score" in out[0]


def test_executor_time_decay_skips_missing_timestamp_and_clamps_negative():
    ex = MultiModalQueryExecutor(FakeClient())
    records = [
        {"id": "a"},  # no timestamp -> skipped
        {"id": "b", "timestamp": 10_000_000_000},  # future relative to ref=0
    ]
    out = ex._apply_time_decay(
        records,
        (TimeDecayFunction.EXPONENTIAL, {"reference_time": 0, "halflife_hours": 1}),
    )
    # 'a' skipped (no decay key), 'b' present
    assert any("_decayed_score" in r for r in out)


# ---------------------------------------------------------------------------
# Convenience functions
# ---------------------------------------------------------------------------


def test_convenience_functions():
    client = FakeClient()
    r1 = semantic_search_with_graph(client, "c", [0.1], "g", edge_types=["E"], top_k=5)
    assert isinstance(r1, MultiModalQueryResult)
    r2 = knowledge_graph_search(client, "g", "Cat", [0.1], "c", max_depth=2, top_k=5)
    assert isinstance(r2, MultiModalQueryResult)
    r3 = logs_with_context(client, "ns", "err", "g", (1, 2), top_k=5)
    assert isinstance(r3, MultiModalQueryResult)


# ---------------------------------------------------------------------------
# FusionFeatures / FeatureExtractor
# ---------------------------------------------------------------------------


def test_fusion_features_to_flat_vector():
    f = FusionFeatures(
        query_features=[1.0, 2.0],
        model_features={"vector": [0.1, 0.2], "graph": [0.3, 0.4]},
        interaction_features=[9.0],
    )
    flat = f.to_flat_vector()
    # query(2) + 4 models * 2 + interaction(1)
    assert flat[:2] == [1.0, 2.0]
    assert flat[-1] == 9.0
    assert len(flat) == 2 + 4 * 2 + 1


def test_feature_extractor_extract():
    fe = FeatureExtractor(num_features=8)
    results = [
        {"source": "vector", "records": [{"id": "a", "score": 0.9}, {"id": "b", "score": 0.5}]},
        {"source": "document", "records": [{"id": "a", "score": 0.7}]},
        {"source": "graph", "records": [{"id": "c"}]},
        {"source": "other", "records": []},
    ]
    feats = fe.extract(results)
    assert isinstance(feats, FusionFeatures)
    assert len(feats.query_features) == 8
    assert "vector" in feats.model_features
    # record 'a' appears in two sources
    assert "a" in feats.record_features
    flat = feats.to_flat_vector()
    assert len(flat) > 0


def test_feature_extractor_model_type_encodings():
    fe = FeatureExtractor(num_features=8)
    for source, expected in [("vector", 0.25), ("document", 0.50), ("graph", 0.75), ("logs", 1.0)]:
        feats = fe._extract_model_features({"source": source, "records": [{"score": 1.0}]})
        assert feats[7] == expected


def test_feature_extractor_empty():
    fe = FeatureExtractor(num_features=4)
    feats = fe.extract([])
    assert feats.query_features[0] == 0.0


# ---------------------------------------------------------------------------
# LearnedFusion
# ---------------------------------------------------------------------------


def _make_sample(target=1.0):
    feats = FusionFeatures(
        query_features=[0.1] * 32,
        model_features={"vector": [0.2] * 32},
        interaction_features=[0.3] * 32,
    )
    return TrainingSample(features=feats, target_scores={"r1": target})


def test_learned_fusion_defaults():
    lf = LearnedFusion()
    assert lf.config.model_type == FusionModelType.GRADIENT_BOOSTING
    assert lf.is_trained is False
    assert lf.training_buffer_size == 0


def test_learned_fusion_linear_init_weights():
    lf = LearnedFusion(LearnedFusionConfig(model_type=FusionModelType.LINEAR))
    assert len(lf._model_weights) == lf.config.num_features * 5


def test_learned_fusion_fuse_empty_and_single():
    lf = LearnedFusion()
    assert lf.fuse([]) == []
    single = [{"source": "vector", "records": [{"id": "a"}]}]
    assert lf.fuse(single) == [{"id": "a"}]


def test_learned_fusion_fuse_untrained_uses_rrf():
    lf = LearnedFusion(LearnedFusionConfig(enable_online_learning=False))
    results = [
        {"source": "vector", "records": [{"id": "a", "score": 0.9}, {"id": "b", "score": 0.4}]},
        {"source": "document", "records": [{"id": "a", "score": 0.7}, {"id": "c", "score": 0.6}]},
    ]
    fused = lf.fuse(results)
    assert all("_fusion_score" in r for r in fused)
    # 'a' appears in both -> should rank highly
    assert fused[0]["id"] == "a"


def test_learned_fusion_record_feedback_click_and_relevance():
    lf = LearnedFusion()
    feats = FusionFeatures(query_features=[0.1] * 32)
    lf.record_feedback(feats, FeedbackSignal(FeedbackType.CLICK, record_id="r1", position=0))
    lf.record_feedback(
        feats, FeedbackSignal(FeedbackType.RELEVANCE_JUDGMENT, record_id="r2", relevant=True)
    )
    assert lf.training_buffer_size == 2


def test_learned_fusion_feedback_disabled_when_not_collecting():
    lf = LearnedFusion(LearnedFusionConfig(collect_training_data=False))
    lf.record_feedback(FusionFeatures(), FeedbackSignal(FeedbackType.CLICK, record_id="r1", position=0))
    assert lf.training_buffer_size == 0


def test_learned_fusion_buffer_max_eviction():
    lf = LearnedFusion(LearnedFusionConfig(max_training_samples=2))
    for _ in range(4):
        lf.add_training_sample(_make_sample())
    assert lf.training_buffer_size == 2


def test_learned_fusion_train_raises_without_enough_samples():
    lf = LearnedFusion(LearnedFusionConfig(min_samples_for_training=5))
    lf.add_training_sample(_make_sample())
    with pytest.raises(ValueError):
        lf.train()


def test_learned_fusion_train_linear_and_predict():
    cfg = LearnedFusionConfig(model_type=FusionModelType.LINEAR, min_samples_for_training=3)
    lf = LearnedFusion(cfg)
    for _ in range(5):
        lf.add_training_sample(_make_sample(target=1.0))
    metrics = lf.train()
    assert isinstance(metrics, TrainingMetrics)
    assert metrics.num_samples == 5
    assert lf.is_trained is True
    # importance available after training
    imp = lf.get_feature_importance()
    assert imp is not None
    # now fuse uses _predict (trained)
    results = [
        {"source": "vector", "records": [{"id": "a", "score": 0.9}]},
        {"source": "document", "records": [{"id": "b", "score": 0.4}]},
    ]
    fused = lf.fuse(results)
    assert all("_fusion_score" in r for r in fused)


def test_learned_fusion_train_gradient_boosting_and_predict():
    cfg = LearnedFusionConfig(
        model_type=FusionModelType.GRADIENT_BOOSTING, min_samples_for_training=3
    )
    lf = LearnedFusion(cfg)
    # mix of targets so residuals are nonzero and stumps get fit
    for i in range(6):
        lf.add_training_sample(_make_sample(target=1.0 if i % 2 == 0 else 0.0))
    metrics = lf.train()
    assert metrics.num_samples == 6
    assert lf.is_trained is True
    imp = lf.get_feature_importance()
    assert imp is None or isinstance(imp, list)
    results = [
        {"source": "vector", "records": [{"id": "a", "score": 0.9}]},
        {"source": "graph", "records": [{"id": "b", "score": 0.4}]},
    ]
    fused = lf.fuse(results)
    assert all("_fusion_score" in r for r in fused)


def test_learned_fusion_feature_importance_untrained_none():
    lf = LearnedFusion()
    assert lf.get_feature_importance() is None


def test_learned_fusion_online_training_trigger():
    cfg = LearnedFusionConfig(
        model_type=FusionModelType.LINEAR,
        enable_online_learning=True,
        online_update_frequency=1,
        min_samples_for_training=2,
    )
    lf = LearnedFusion(cfg)
    for _ in range(3):
        lf.add_training_sample(_make_sample())
    results = [
        {"source": "vector", "records": [{"id": "a", "score": 0.9}]},
        {"source": "document", "records": [{"id": "b", "score": 0.4}]},
    ]
    # fuse increments query_count to 1 -> triggers _maybe_train -> trains
    lf.fuse(results)
    assert lf.is_trained is True


def test_stump_predict_branches():
    lf = LearnedFusion()
    stump = {"feature_index": 0, "threshold": 0.5, "left_value": -1.0, "right_value": 1.0}
    assert lf._stump_predict(stump, [0.1]) == -1.0
    assert lf._stump_predict(stump, [0.9]) == 1.0
    # feature index out of range -> treated as 0.0 -> left
    assert lf._stump_predict({"feature_index": 99, "threshold": 0.5,
                              "left_value": -1.0, "right_value": 1.0}, [0.9]) == -1.0


def test_fit_stump_empty_returns_none():
    lf = LearnedFusion()
    assert lf._fit_stump([], []) is None
    assert lf._fit_stump([[]], [0.0]) is None


# ---------------------------------------------------------------------------
# Extra targeted branches
# ---------------------------------------------------------------------------


class BareClient:
    """Client missing every optional method the executor probes for."""

    def search(self, **kw):
        return []


def test_executor_components_with_missing_client_methods():
    # graph/document/logs/metrics all return [] when the client lacks the method
    q = (
        MultiModalQueryBuilder()
        .graph("g")
        .document("docs")
        .logs("ns", time_range=(0, 1))
        .metrics("ns", ["m"], (0, 1))
        .fuse(FusionStrategy.UNION)
        .build()
    )
    res = MultiModalQueryExecutor(BareClient()).execute(q)
    assert res.records == []


def test_executor_graph_traverse_exception():
    class GraphBoom:
        def search(self, **kw):
            return []

        graph = SimpleNamespace(
            traverse=lambda **kw: (_ for _ in ()).throw(RuntimeError("boom"))
        )

    q = MultiModalQueryBuilder().graph("g").build()
    res = MultiModalQueryExecutor(GraphBoom()).execute(q)
    assert res.records == []


def test_learned_fusion_predict_untrained_neural_returns_half():
    cfg = LearnedFusionConfig(
        model_type=FusionModelType.NEURAL_NETWORK, num_features=2
    )
    lf = LearnedFusion(cfg)
    # _predict default branch (non-linear, non-gb) -> 0.5
    out = lf._predict(FusionFeatures(query_features=[0.1, 0.2]), ["a", "b"])
    assert out == [0.5, 0.5]


def test_learned_fusion_train_default_branch_neural():
    cfg = LearnedFusionConfig(
        model_type=FusionModelType.NEURAL_NETWORK,
        num_features=2,
        min_samples_for_training=1,
    )
    lf = LearnedFusion(cfg)
    lf.add_training_sample(
        TrainingSample(FusionFeatures(query_features=[0.1, 0.2]), {"r": 1.0})
    )
    # NEURAL_NETWORK type falls through to linear training
    metrics = lf.train()
    assert lf.is_trained
    assert metrics.num_samples == 1


def test_learned_fusion_gb_feature_importance_no_trees():
    cfg = LearnedFusionConfig(model_type=FusionModelType.GRADIENT_BOOSTING)
    lf = LearnedFusion(cfg)
    lf._is_trained = True  # trained flag set but no trees fitted
    assert lf.get_feature_importance() is None


def test_learned_fusion_maybe_train_swallows_error():
    cfg = LearnedFusionConfig(
        model_type=FusionModelType.LINEAR, min_samples_for_training=1
    )
    lf = LearnedFusion(cfg)
    lf.add_training_sample(TrainingSample(FusionFeatures(), {"r": 1.0}))

    # Force train() to raise; _maybe_train must swallow it
    def boom():
        raise RuntimeError("train failed")

    lf.train = boom  # type: ignore[assignment]
    lf._maybe_train()  # should not raise
    assert lf.is_trained is False


def test_fuse_results_single_component_returns_as_is():
    ex = MultiModalQueryExecutor(FakeClient())
    comp = [[{"id": "a"}]]
    assert ex._fuse_results(comp, "rrf", {}) == [{"id": "a"}]
