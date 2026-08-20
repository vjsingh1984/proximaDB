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


def test_adaptive_policy_is_explicit_and_fixed_only_remains_available():
    assert SWEEP.range_policies(False) == ("fixed",)
    assert SWEEP.range_policies(True) == ("fixed", "adaptive")


def test_each_retry_uses_an_isolated_persistent_cache_directory(tmp_path):
    assert SWEEP.disk_path_for_attempt(tmp_path, "fixed", 0) == (
        tmp_path / "local-disk-cache-fixed-attempt-0"
    )
    assert SWEEP.disk_path_for_attempt(tmp_path, "adaptive", 3) == (
        tmp_path / "local-disk-cache-adaptive-attempt-3"
    )
    with pytest.raises(RuntimeError, match="non-negative"):
        SWEEP.disk_path_for_attempt(tmp_path, "fixed", -1)
    with pytest.raises(RuntimeError, match="unsupported"):
        SWEEP.disk_path_for_attempt(tmp_path, "other", 0)


def test_rejected_attempt_cleanup_is_scoped_to_owned_policy_directories(tmp_path):
    paths = {
        policy: SWEEP.disk_path_for_attempt(tmp_path, policy, 2)
        for policy in SWEEP.range_policies(True)
    }
    for path in paths.values():
        path.mkdir()
        (path / "cache-entry").write_text("scratch")

    SWEEP.remove_discarded_cache_paths(tmp_path, paths)

    assert all(not path.exists() for path in paths.values())
    outside = tmp_path.parent / "outside-cache"
    with pytest.raises(RuntimeError, match="unowned"):
        SWEEP.remove_discarded_cache_paths(tmp_path, {"fixed": outside})


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


def point(
    *,
    physical_gets: float,
    wire_gets: int,
    bytes_read: float,
    recall: float = 0.99,
    p50: float = 100.0,
    p95: float = 200.0,
    peak_rss: int = 100,
    qps: float = 10.0,
    concurrency: int = 4,
    disk_hits: float = 0.0,
    identity_suffix: str = "",
    wire_range_gets: int | None = None,
):
    if wire_range_gets is None:
        wire_range_gets = int(physical_gets)
    return {
        "physical_gets": physical_gets,
        "wire_http": {
            "get_requests": wire_gets,
            "range_get_requests": wire_range_gets,
        },
        "wire_range_to_application_get_ratio": (
            wire_range_gets / physical_gets if physical_gets else None
        ),
        "bytes_read": bytes_read,
        "recall_at_k": recall,
        "latency_ms": {"p50": p50, "p95": p95},
        "process_rss": {"peak_bytes": peak_rss},
        "load": {
            "qps": qps,
            "peak_in_flight": concurrency,
            "configured_concurrency": concurrency,
        },
        "local_disk": {"hits": disk_hits, "misses": 0.0, "resident_bytes": 0.0},
        "result_identity": {
            "ordered_ids_sha256_by_query": [f"a{identity_suffix}", "b"],
            "set_ids_sha256_by_query": [f"A{identity_suffix}", "B"],
            "recall_hits_by_query": [10, 9],
        },
    }


def test_point_comparison_separates_wire_and_application_economics():
    baseline = {
        **point(
            physical_gets=10_000.0,
            wire_gets=12_000,
            bytes_read=20_000.0,
            peak_rss=100,
            qps=10.0,
        )
    }
    candidate = point(
        physical_gets=2_000.0,
        wire_gets=3_000,
        bytes_read=5_000.0,
        p50=40.0,
        p95=80.0,
        peak_rss=110,
        qps=20.0,
    )

    comparison = SWEEP.compare_points(candidate, baseline, query_count=500)

    assert comparison["application_get_reduction"] == pytest.approx(0.8)
    assert comparison["wire_get_reduction"] == pytest.approx(0.75)
    assert comparison["bytes_ratio"] == pytest.approx(0.25)
    assert comparison["p50_ratio"] == pytest.approx(0.4)
    assert comparison["p95_ratio"] == pytest.approx(0.4)
    assert comparison["rss_ratio"] == pytest.approx(1.1)
    assert comparison["qps_ratio"] == pytest.approx(2.0)
    assert comparison["recall_delta"] == 0.0
    assert comparison["result_identity_diagnostics"]["result_set_mismatch_count"] == 0
    assert comparison["azure_hot_read_cogs_per_million_queries_usd"] == pytest.approx(
        3.0
    )


def test_equal_zero_io_is_equal_and_range_reconciled():
    baseline = point(
        physical_gets=0,
        wire_gets=1,
        wire_range_gets=0,
        bytes_read=0,
    )
    candidate = point(
        physical_gets=0,
        wire_gets=1,
        wire_range_gets=0,
        bytes_read=0,
    )

    comparison = SWEEP.compare_points(candidate, baseline, query_count=5)

    assert comparison["application_get_reduction"] == 0.0
    assert comparison["bytes_ratio"] == 1.0
    assert SWEEP.wire_ranges_reconciled(candidate) is True


def test_zero_application_gets_fail_reconciliation_if_a_range_reaches_wire():
    candidate = point(
        physical_gets=0,
        wire_gets=1,
        wire_range_gets=1,
        bytes_read=0,
    )

    assert SWEEP.wire_ranges_reconciled(candidate) is False


def passing_policy_results():
    return {
        "fixed": {
            "phases": {
                "object_cold": point(
                    physical_gets=100, wire_gets=100, bytes_read=1_000
                ),
                "dram_warm": point(
                    physical_gets=80,
                    wire_gets=80,
                    bytes_read=900,
                    p50=80,
                    p95=160,
                    peak_rss=105,
                    qps=12,
                ),
                "disk_warm": point(
                    physical_gets=60,
                    wire_gets=60,
                    bytes_read=800,
                    p50=90,
                    p95=180,
                    peak_rss=105,
                    qps=11,
                    disk_hits=10,
                ),
            }
        },
        "adaptive": {
            "phases": {
                "object_cold": point(
                    physical_gets=75,
                    wire_gets=75,
                    bytes_read=1_100,
                    p50=80,
                    p95=160,
                    peak_rss=105,
                    qps=12,
                ),
                "dram_warm": point(
                    physical_gets=60,
                    wire_gets=60,
                    bytes_read=950,
                    p50=70,
                    p95=140,
                    peak_rss=108,
                    qps=14,
                ),
                "disk_warm": point(
                    physical_gets=45,
                    wire_gets=45,
                    bytes_read=850,
                    p50=80,
                    p95=160,
                    peak_rss=108,
                    qps=13,
                    disk_hits=12,
                ),
            }
        },
    }


def evaluate(policy_results):
    return SWEEP.evaluate_promotion(
        policy_results,
        query_count=500,
        concurrency=4,
        target_recall=0.98,
        max_recall_regression=0.0005,
        min_disk_get_reduction=0.20,
        min_adaptive_cold_get_reduction=0.10,
        max_adaptive_warm_get_ratio=1.05,
        max_byte_amplification=1.25,
        max_latency_ratio=1.10,
        max_rss_ratio=1.10,
        min_qps_ratio=0.95,
    )


def test_paired_promotion_gates_cache_benefit_and_adaptive_safety_by_tier():
    evaluation = evaluate(passing_policy_results())

    assert evaluation["promotion_eligible"] is True
    assert evaluation["gate_failures"] == []
    assert evaluation["paired_comparisons"]["object_cold"][
        "wire_get_reduction"
    ] == pytest.approx(0.25)
    assert evaluation["cache_comparisons"]["adaptive"]["disk_warm"][
        "wire_get_reduction"
    ] == pytest.approx(0.40)


def test_paired_promotion_accepts_equal_zero_object_io_in_disk_tier():
    policy_results = passing_policy_results()
    for policy_result in policy_results.values():
        disk = policy_result["phases"]["disk_warm"]
        disk["physical_gets"] = 0
        disk["wire_http"] = {"get_requests": 1, "range_get_requests": 0}
        disk["wire_range_to_application_get_ratio"] = None
        disk["bytes_read"] = 0

    evaluation = evaluate(policy_results)

    assert evaluation["promotion_eligible"] is True
    assert evaluation["paired_comparisons"]["disk_warm"]["bytes_ratio"] == 1.0
    assert not any(
        "wire_range_reconciled" in item for item in evaluation["gate_failures"]
    )


def test_fixed_only_diagnostic_can_be_valid_without_authorizing_promotion():
    policy_results = passing_policy_results()

    evaluation = evaluate({"fixed": policy_results["fixed"]})

    assert evaluation["measurement_valid"] is True
    assert evaluation["promotion_eligible"] is False
    assert evaluation["gate_failures"] == []


def test_result_identity_is_diagnostic_but_warm_get_regression_fails_closed():
    policy_results = passing_policy_results()
    adaptive_disk = policy_results["adaptive"]["phases"]["disk_warm"]
    adaptive_disk["wire_http"]["get_requests"] = 70
    adaptive_disk["physical_gets"] = 70
    adaptive_disk["result_identity"] = point(
        physical_gets=1,
        wire_gets=1,
        bytes_read=1,
        identity_suffix="-tie",
    )["result_identity"]

    evaluation = evaluate(policy_results)

    assert evaluation["promotion_eligible"] is False
    assert "paired.disk_warm.get_not_regressed" in evaluation["gate_failures"]
    assert (
        evaluation["paired_comparisons"]["disk_warm"]["result_identity_diagnostics"][
            "result_set_mismatch_count"
        ]
        == 1
    )
    assert not any("identity" in failure for failure in evaluation["gate_failures"])
