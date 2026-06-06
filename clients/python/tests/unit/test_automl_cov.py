"""Offline unit tests for proximadb_sdk.automl (pure logic, no network)."""

import random
from unittest.mock import MagicMock

import pytest

from proximadb_sdk.automl import (
    AutoML,
    EngineRecommendation,
    EngineSelector,
    HyperparameterConfig,
    HyperparameterOptimizer,
    OptimizationGoal,
    OptimizationResult,
    WorkloadCharacteristics,
    WorkloadPredictor,
    WorkloadType,
)


# --------------------------------------------------------------------------
# Enums
# --------------------------------------------------------------------------
def test_enums_values():
    assert WorkloadType.READ_HEAVY.value == "read_heavy"
    assert WorkloadType.WRITE_HEAVY == "write_heavy"
    assert OptimizationGoal.LATENCY.value == "latency"
    assert OptimizationGoal.BALANCED == "balanced"
    # ensure all members present
    assert {g.value for g in OptimizationGoal} == {
        "latency",
        "throughput",
        "memory",
        "cost",
        "recall",
        "balanced",
    }


# --------------------------------------------------------------------------
# Dataclasses
# --------------------------------------------------------------------------
def test_workload_characteristics_to_dict_defaults():
    wc = WorkloadCharacteristics()
    d = wc.to_dict()
    assert d["read_ratio"] == 0.5
    assert d["target_latency_ms"] is None
    assert set(d.keys()) == {
        "read_ratio",
        "write_ratio",
        "query_complexity",
        "vector_count",
        "vector_dimension",
        "metadata_cardinality",
        "temporal_locality",
        "spatial_locality",
        "hot_data_ratio",
        "target_latency_ms",
        "target_throughput",
        "memory_budget_mb",
    }


def test_engine_recommendation_to_dict():
    rec = EngineRecommendation(
        engine="sst",
        confidence=0.9,
        reasoning="because",
        estimated_latency_ms=5.0,
        estimated_throughput=100,
        estimated_memory_mb=50,
        config_overrides={"compression": "lz4"},
    )
    d = rec.to_dict()
    assert d["engine"] == "sst"
    assert d["config_overrides"] == {"compression": "lz4"}


def test_hyperparameter_config_and_optimization_result_defaults():
    hp = HyperparameterConfig(name="ef_search", current_value=64)
    assert hp.min_value is None
    assert hp.allowed_values is None

    res = OptimizationResult(best_config={"a": 1}, best_score=0.5, iterations=3)
    assert res.search_history == []
    assert res.improvement_ratio == 0


# --------------------------------------------------------------------------
# WorkloadPredictor
# --------------------------------------------------------------------------
def test_predict_empty_returns_defaults():
    p = WorkloadPredictor()
    wc = p.predict()
    assert wc.read_ratio == 0.5
    assert wc.write_ratio == 0.5


def test_observe_and_predict_read_heavy():
    p = WorkloadPredictor()
    for _ in range(8):
        p.observe_operation("search", latency_ms=5.0, vector_count=10)
    p.observe_operation("get", latency_ms=2.0, vector_count=1)
    p.observe_operation("insert", latency_ms=3.0, vector_count=1, metadata={"k": "v"})
    wc = p.predict()
    assert wc.read_ratio > 0.7
    assert wc.vector_count == 8 * 10 + 1 + 1
    assert 0.0 <= wc.query_complexity <= 1.0
    # multiple ops -> temporal locality path executed
    assert 0.0 <= wc.temporal_locality <= 1.0
    assert p.get_workload_type() == WorkloadType.READ_HEAVY


def test_get_workload_type_write_heavy():
    p = WorkloadPredictor()
    for _ in range(9):
        p.observe_operation("insert", latency_ms=1.0)
    p.observe_operation("update", latency_ms=1.0)
    assert p.get_workload_type() == WorkloadType.WRITE_HEAVY


def test_get_workload_type_mixed():
    p = WorkloadPredictor()
    for _ in range(5):
        p.observe_operation("search", latency_ms=1.0)
    for _ in range(5):
        p.observe_operation("insert", latency_ms=1.0)
    assert p.get_workload_type() == WorkloadType.MIXED


def test_predict_single_op_temporal_locality_branch():
    p = WorkloadPredictor()
    p.observe_operation("search", latency_ms=200.0, vector_count=5)
    wc = p.predict()
    # single observation -> temporal_locality default 0.5 branch
    assert wc.temporal_locality == 0.5
    # high latency clamps query_complexity to 1.0
    assert wc.query_complexity == 1.0


def test_window_size_eviction():
    p = WorkloadPredictor(window_size=3)
    for i in range(5):
        p.observe_operation("search", latency_ms=float(i))
    assert len(p._operations) == 3
    # counts are not evicted
    assert p._operation_counts["search"] == 5


# --------------------------------------------------------------------------
# EngineSelector
# --------------------------------------------------------------------------
def test_recommend_basic_returns_recommendation():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(read_ratio=0.8, write_ratio=0.2, vector_count=100000)
    rec = sel.recommend(wc, OptimizationGoal.LATENCY)
    assert isinstance(rec, EngineRecommendation)
    assert rec.engine in EngineSelector.ENGINE_PROFILES
    assert rec.estimated_throughput >= 1000
    assert rec.estimated_memory_mb >= 10
    assert 0.0 <= rec.confidence <= 1.0


@pytest.mark.parametrize(
    "goal",
    [
        OptimizationGoal.LATENCY,
        OptimizationGoal.THROUGHPUT,
        OptimizationGoal.MEMORY,
        OptimizationGoal.RECALL,
        OptimizationGoal.BALANCED,
        OptimizationGoal.COST,
    ],
)
def test_recommend_all_goals(goal):
    sel = EngineSelector()
    wc = WorkloadCharacteristics(vector_count=5000, vector_dimension=768)
    rec = sel.recommend(wc, goal)
    assert rec.engine in EngineSelector.ENGINE_PROFILES


def test_recommend_skips_swift_for_large_dataset():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(vector_count=10_000_000, read_ratio=0.95)
    recs = sel.compare_engines(wc)
    engines = {r.engine for r in recs}
    assert "swift" not in engines


def test_recommend_swift_eligible_small_dataset():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(vector_count=500, read_ratio=0.95, write_ratio=0.05)
    recs = sel.compare_engines(wc)
    engines = {r.engine for r in recs}
    assert "swift" in engines


def test_target_latency_penalty_branch():
    sel = EngineSelector()
    # viper latency_base=90, target 10 -> penalized
    wc = WorkloadCharacteristics(vector_count=1000, target_latency_ms=10.0)
    profile = EngineSelector.ENGINE_PROFILES["viper"]
    score = sel._calculate_score("viper", profile, wc, OptimizationGoal.BALANCED)
    assert 0.0 <= score <= 1.0


def test_target_throughput_penalty_branch():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(vector_count=1000, target_throughput=999999)
    profile = EngineSelector.ENGINE_PROFILES["viper"]
    score = sel._calculate_score("viper", profile, wc, OptimizationGoal.THROUGHPUT)
    assert 0.0 <= score <= 1.0


def test_infer_workload_type_all_branches():
    sel = EngineSelector()
    assert (
        sel._infer_workload_type(WorkloadCharacteristics(write_ratio=0.8))
        == "write_heavy"
    )
    assert (
        sel._infer_workload_type(
            WorkloadCharacteristics(write_ratio=0.1, read_ratio=0.8)
        )
        == "read_heavy"
    )
    assert (
        sel._infer_workload_type(
            WorkloadCharacteristics(
                write_ratio=0.1, read_ratio=0.1, query_complexity=0.8
            )
        )
        == "analytics"
    )
    assert (
        sel._infer_workload_type(
            WorkloadCharacteristics(
                write_ratio=0.1,
                read_ratio=0.1,
                query_complexity=0.1,
                temporal_locality=0.9,
            )
        )
        == "streaming"
    )
    assert (
        sel._infer_workload_type(
            WorkloadCharacteristics(
                write_ratio=0.1,
                read_ratio=0.1,
                query_complexity=0.1,
                temporal_locality=0.1,
            )
        )
        == "mixed"
    )


def test_generate_config_sst_write_heavy():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(write_ratio=0.8)
    cfg = sel._generate_config("sst", EngineSelector.ENGINE_PROFILES["sst"], wc)
    assert cfg["compression"] == "lz4"
    assert cfg["flush_threshold_mb"] == 128


def test_generate_config_sst_read_heavy():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(write_ratio=0.2)
    cfg = sel._generate_config("sst", EngineSelector.ENGINE_PROFILES["sst"], wc)
    assert cfg["compression"] == "zstd"
    assert cfg["bloom_filter_fpp"] == 0.001


def test_generate_config_helix_high_dim_and_spatial():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(vector_dimension=1024, spatial_locality=0.9)
    cfg = sel._generate_config("helix", EngineSelector.ENGINE_PROFILES["helix"], wc)
    assert cfg["pca_dimensions"] == min(128, 1024 // 4)
    assert cfg["hilbert_bits"] == 16


def test_generate_config_helix_low_dim_low_spatial():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(vector_dimension=128, spatial_locality=0.1)
    cfg = sel._generate_config("helix", EngineSelector.ENGINE_PROFILES["helix"], wc)
    assert "pca_dimensions" not in cfg
    assert cfg["hilbert_bits"] == 12


def test_generate_config_viper_large_and_small():
    sel = EngineSelector()
    big = sel._generate_config(
        "viper",
        EngineSelector.ENGINE_PROFILES["viper"],
        WorkloadCharacteristics(vector_count=200000),
    )
    assert big["row_group_size"] == 100000
    small = sel._generate_config(
        "viper",
        EngineSelector.ENGINE_PROFILES["viper"],
        WorkloadCharacteristics(vector_count=50),
    )
    assert small["row_group_size"] == 10000
    assert small["enable_statistics"] is True


def test_generate_config_swift_exact_search_branches():
    sel = EngineSelector()
    no_target = sel._generate_config(
        "swift",
        EngineSelector.ENGINE_PROFILES["swift"],
        WorkloadCharacteristics(target_latency_ms=None),
    )
    assert no_target["in_memory"] is True
    assert no_target["exact_search"] is True
    tight = sel._generate_config(
        "swift",
        EngineSelector.ENGINE_PROFILES["swift"],
        WorkloadCharacteristics(target_latency_ms=2.0),
    )
    assert tight["exact_search"] is False


def test_generate_config_raptor_and_nova_unknown():
    sel = EngineSelector()
    raptor = sel._generate_config(
        "raptor",
        EngineSelector.ENGINE_PROFILES["raptor"],
        WorkloadCharacteristics(hot_data_ratio=0.5),
    )
    assert raptor["adaptive_pruning"] is True
    assert raptor["cache_hot_blocks"] is True
    # nova has no special config -> empty dict
    nova = sel._generate_config(
        "nova",
        EngineSelector.ENGINE_PROFILES["nova"],
        WorkloadCharacteristics(),
    )
    assert nova == {}


def test_generate_reasoning_all_engines():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(
        write_ratio=0.8,
        spatial_locality=0.9,
        query_complexity=0.9,
        hot_data_ratio=0.5,
    )
    for engine in EngineSelector.ENGINE_PROFILES:
        reasoning = sel._generate_reasoning(engine, wc, OptimizationGoal.LATENCY)
        assert isinstance(reasoning, str)
        assert "Optimized for latency" in reasoning


def test_generate_reasoning_balanced_no_goal_clause():
    sel = EngineSelector()
    reasoning = sel._generate_reasoning(
        "nova", WorkloadCharacteristics(), OptimizationGoal.BALANCED
    )
    assert "Optimized for" not in reasoning


def test_compare_engines_sorted_descending():
    sel = EngineSelector()
    wc = WorkloadCharacteristics(read_ratio=0.7, write_ratio=0.3, vector_count=5000)
    recs = sel.compare_engines(wc)
    assert len(recs) >= 1
    confidences = [r.confidence for r in recs]
    assert confidences == sorted(confidences, reverse=True)


def test_get_engine_recommendation_unknown_returns_none():
    sel = EngineSelector()
    assert sel._get_engine_recommendation("nonexistent", WorkloadCharacteristics()) is None


def test_get_engine_recommendation_swift_too_large_none():
    sel = EngineSelector()
    rec = sel._get_engine_recommendation(
        "swift", WorkloadCharacteristics(vector_count=10_000_000)
    )
    assert rec is None


# --------------------------------------------------------------------------
# HyperparameterOptimizer
# --------------------------------------------------------------------------
def test_get_searchable_params():
    opt = HyperparameterOptimizer()
    params = opt._get_searchable_params()
    names = {p.name for p in params}
    assert {"ef_search", "ef_construction", "m", "bloom_filter_fpp", "block_size"} == names


def test_optimize_no_client_heuristic():
    opt = HyperparameterOptimizer()
    res = opt.optimize("col", goal=OptimizationGoal.LATENCY, max_iterations=5)
    assert isinstance(res, OptimizationResult)
    assert res.iterations == 5
    assert len(res.search_history) == 5
    assert res.best_score != float("-inf")


def test_optimize_recall_goal():
    opt = HyperparameterOptimizer()
    res = opt.optimize("col", goal=OptimizationGoal.RECALL, max_iterations=3)
    assert res.iterations == 3


def test_generate_candidate_full_exploration(monkeypatch):
    opt = HyperparameterOptimizer()
    params = opt._get_searchable_params()
    # force exploration (random.random() < 1.0 always)
    monkeypatch.setattr(random, "random", lambda: 0.0)
    monkeypatch.setattr(random, "choice", lambda seq: seq[0])
    monkeypatch.setattr(random, "randint", lambda a, b: 0)
    monkeypatch.setattr(random, "uniform", lambda a, b: a)
    cfg = opt._generate_candidate(params, iteration=0, max_iterations=10)
    # allowed_values param chooses first; min/max+step param uses min; fpp uniform=min
    assert cfg["block_size"] == 16384
    assert cfg["ef_search"] == 16
    assert cfg["bloom_filter_fpp"] == 0.001


def test_generate_candidate_full_exploitation(monkeypatch):
    opt = HyperparameterOptimizer()
    params = opt._get_searchable_params()
    # force exploit (random.random() >= exploration_rate)
    monkeypatch.setattr(random, "random", lambda: 0.99)
    cfg = opt._generate_candidate(params, iteration=9, max_iterations=10)
    assert cfg["ef_search"] == 64  # current_value
    assert cfg["m"] == 16


def test_generate_candidate_param_without_bounds(monkeypatch):
    opt = HyperparameterOptimizer()
    # param with no allowed_values and no min/max -> falls to current_value
    param = HyperparameterConfig(name="lonely", current_value=42)
    monkeypatch.setattr(random, "random", lambda: 0.0)
    cfg = opt._generate_candidate([param], iteration=0, max_iterations=10)
    assert cfg["lonely"] == 42


def test_evaluate_config_heuristic_no_client():
    opt = HyperparameterOptimizer()
    score = opt._evaluate_config(
        "col", {"ef_search": 256, "m": 32}, OptimizationGoal.RECALL, None
    )
    assert score > 0.5


def test_heuristic_score_goal_branches():
    opt = HyperparameterOptimizer()
    cfg = {"ef_search": 512, "m": 64}
    recall = opt._heuristic_score(cfg, OptimizationGoal.RECALL)
    latency = opt._heuristic_score(cfg, OptimizationGoal.LATENCY)
    memory = opt._heuristic_score(cfg, OptimizationGoal.MEMORY)
    base = opt._heuristic_score({}, OptimizationGoal.BALANCED)
    assert recall > base
    assert latency >= 0.5
    assert memory >= 0.5
    assert base == 0.5


def test_evaluate_config_with_client_and_queries():
    client = MagicMock()
    client.search.return_value = {"results": []}
    opt = HyperparameterOptimizer(client=client)
    score = opt._evaluate_config(
        "col",
        {"ef_search": 64},
        OptimizationGoal.LATENCY,
        test_queries=[[0.1, 0.2], [0.3, 0.4]],
    )
    assert score > 0
    assert client.search.called


def test_evaluate_config_with_client_throughput():
    client = MagicMock()
    client.search.return_value = {}
    opt = HyperparameterOptimizer(client=client)
    score = opt._evaluate_config(
        "col", {}, OptimizationGoal.THROUGHPUT, test_queries=[[0.1]]
    )
    assert score > 0


def test_evaluate_config_with_client_balanced_else():
    client = MagicMock()
    client.search.return_value = {}
    opt = HyperparameterOptimizer(client=client)
    score = opt._evaluate_config(
        "col", {}, OptimizationGoal.BALANCED, test_queries=[[0.1]]
    )
    assert score > 0


def test_evaluate_config_with_client_search_error():
    client = MagicMock()
    client.search.side_effect = RuntimeError("boom")
    opt = HyperparameterOptimizer(client=client)
    score = opt._evaluate_config(
        "col", {}, OptimizationGoal.LATENCY, test_queries=[[0.1]]
    )
    # error path -> penalty latency 1000 -> low but positive score
    assert score > 0


def test_optimize_with_client_and_test_queries():
    client = MagicMock()
    client.search.return_value = {}
    opt = HyperparameterOptimizer(client=client)
    res = opt.optimize(
        "col",
        goal=OptimizationGoal.LATENCY,
        max_iterations=2,
        test_queries=[[0.1, 0.2]],
    )
    assert res.iterations == 2
    assert isinstance(res.improvement_ratio, float) or res.improvement_ratio == 0


# --------------------------------------------------------------------------
# AutoML
# --------------------------------------------------------------------------
def test_automl_init_components():
    am = AutoML()
    assert isinstance(am.predictor, WorkloadPredictor)
    assert isinstance(am.selector, EngineSelector)
    assert isinstance(am.optimizer, HyperparameterOptimizer)


def test_automl_recommend_engine_default_write_ratio():
    am = AutoML()
    rec = am.recommend_engine(
        vector_count=1000,
        vector_dimension=768,
        read_ratio=0.7,
        goal=OptimizationGoal.LATENCY,
    )
    assert isinstance(rec, EngineRecommendation)
    assert rec.engine in EngineSelector.ENGINE_PROFILES


def test_automl_recommend_engine_explicit_write_ratio():
    am = AutoML()
    rec = am.recommend_engine(
        vector_count=2000,
        read_ratio=0.6,
        write_ratio=0.4,
        target_latency_ms=20.0,
        target_throughput=1000,
        memory_budget_mb=512,
    )
    assert isinstance(rec, EngineRecommendation)


def test_automl_auto_configure():
    am = AutoML()
    cfg = am.auto_configure("col", goal=OptimizationGoal.MEMORY, max_iterations=3)
    assert isinstance(cfg, dict)
    assert "ef_search" in cfg


def test_automl_observe_and_analyze():
    am = AutoML()
    am.observe("search", latency_ms=5.0, vector_count=3)
    am.observe("insert", latency_ms=2.0)
    wc = am.analyze_workload()
    assert isinstance(wc, WorkloadCharacteristics)
    assert wc.vector_count == 4


def test_automl_passes_client_to_subcomponents():
    client = MagicMock()
    am = AutoML(client=client)
    assert am.selector._client is client
    assert am.optimizer._client is client
