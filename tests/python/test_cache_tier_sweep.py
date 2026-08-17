import importlib.util
from pathlib import Path

import pytest

SCRIPT = (
    Path(__file__).resolve().parents[2] / "scripts" / "bench" / "cache_tier_sweep.py"
)
SPEC = importlib.util.spec_from_file_location("cache_tier_sweep", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
SWEEP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SWEEP)


def test_query_slices_are_disjoint_and_measurement_is_shared_by_every_phase():
    slices = SWEEP.query_slices(
        query_start=0,
        warmup_queries=400,
        measured_queries=600,
        available_queries=1_000,
        available_truth=1_000,
    )

    assert slices == {
        "warmup": [0, 400],
        "measured": [400, 1_000],
    }


@pytest.mark.parametrize(
    ("query_start", "warmup", "measured", "available", "message"),
    [
        (0, 0, 500, 1_000, "positive"),
        (0, 500, 0, 1_000, "positive"),
        (0, 600, 500, 1_000, "exceeds"),
        (-1, 400, 500, 1_000, "non-negative"),
    ],
)
def test_query_slices_fail_closed_on_invalid_geometry(
    query_start: int,
    warmup: int,
    measured: int,
    available: int,
    message: str,
):
    with pytest.raises(RuntimeError, match=message):
        SWEEP.query_slices(
            query_start,
            warmup,
            measured,
            available,
            available,
        )


def test_disk_population_queries_measurement_before_disjoint_warmup():
    assert SWEEP.disk_population_order() == ("measured", "warmup")


def test_each_retry_uses_an_isolated_persistent_cache_directory(tmp_path):
    assert SWEEP.disk_path_for_attempt(tmp_path, 0) == (
        tmp_path / "local-disk-cache-attempt-0"
    )
    assert SWEEP.disk_path_for_attempt(tmp_path, 3) == (
        tmp_path / "local-disk-cache-attempt-3"
    )
    with pytest.raises(RuntimeError, match="non-negative"):
        SWEEP.disk_path_for_attempt(tmp_path, -1)


@pytest.mark.parametrize(
    ("hits", "misses", "expected"),
    [(0, 0, None), (1, 0, 1.0), (0, 2, 0.0), (3, 1, 0.75)],
)
def test_hit_ratio_is_explicit_about_empty_denominator(hits, misses, expected):
    assert SWEEP.hit_ratio(hits, misses) == expected


def test_add_cache_ratios_does_not_mutate_raw_counters():
    point = {
        "survivor": {"hits": 3.0, "misses": 1.0},
        "invariants": {"hits": 2.0, "misses": 0.0},
        "local_disk": {"hits": 4.0, "misses": 6.0},
    }

    enriched = SWEEP.add_cache_ratios(point)

    assert enriched is point
    assert point["survivor"] == {
        "hits": 3.0,
        "misses": 1.0,
        "hit_ratio": 0.75,
    }
    assert point["invariants"]["hit_ratio"] == 1.0
    assert point["local_disk"]["hit_ratio"] == 0.4


def test_phase_comparison_uses_same_result_identity_and_reports_economics():
    baseline = {
        "physical_gets": 10_000.0,
        "bytes_read": 20_000.0,
        "recall_at_k": 0.99,
        "latency_ms": {"p50": 100.0, "p95": 200.0},
        "result_identity": {
            "ordered_ids_sha256_by_query": ["a", "b"],
            "set_ids_sha256_by_query": ["A", "B"],
            "recall_hits_by_query": [10, 9],
        },
    }
    candidate = {
        "physical_gets": 2_000.0,
        "bytes_read": 5_000.0,
        "recall_at_k": 0.99,
        "latency_ms": {"p50": 40.0, "p95": 80.0},
        "result_identity": baseline["result_identity"].copy(),
    }

    comparison = SWEEP.compare_phase(candidate, baseline, query_count=500)

    assert comparison["get_reduction"] == pytest.approx(0.8)
    assert comparison["byte_reduction"] == pytest.approx(0.75)
    assert comparison["p50_ratio"] == pytest.approx(0.4)
    assert comparison["p95_ratio"] == pytest.approx(0.4)
    assert comparison["recall_delta"] == 0.0
    assert comparison["result_identity_equal"] is True
    assert comparison["azure_hot_read_cogs_per_million_queries_usd"] == pytest.approx(
        2.0
    )
