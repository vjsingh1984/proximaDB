import importlib.util
import json
import math
from pathlib import Path

import pytest

SCRIPT = (
    Path(__file__).resolve().parents[2]
    / "scripts"
    / "bench"
    / "analyze_nprobe_geometry.py"
)
SPEC = importlib.util.spec_from_file_location("nprobe_geometry_analysis", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
ANALYSIS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ANALYSIS)


def test_quality_profile_prefers_first_probe_that_meets_recall():
    points = [
        {"nprobe": 1, "recall_at_k": 0.90, "gets_per_query": 1.0},
        {"nprobe": 2, "recall_at_k": 0.979, "gets_per_query": 2.0},
        {"nprobe": 4, "recall_at_k": 0.981, "gets_per_query": 4.0},
        {"nprobe": 8, "recall_at_k": 0.990, "gets_per_query": 8.0},
    ]

    profile = ANALYSIS.quality_profile(points, 0.98)

    assert profile["status"] == "attained"
    assert profile["point"]["nprobe"] == 4
    assert profile["point"]["recall_at_k"] == 0.981


def test_quality_profile_reports_unattained_without_rejecting_curve():
    points = [
        {"nprobe": 2, "recall_at_k": 0.95, "gets_per_query": 2.0},
        {"nprobe": 4, "recall_at_k": 0.97, "gets_per_query": 4.0},
        {"nprobe": 8, "recall_at_k": 0.975, "gets_per_query": 8.0},
    ]

    profile = ANALYSIS.quality_profile(points, 0.98)

    assert profile == {
        "status": "unattained",
        "target_recall": 0.98,
        "point": None,
        "max_measured_recall": 0.975,
        "max_measured_nprobe": 8,
    }


def test_curve_bend_detects_diminishing_return_before_saturation():
    points = [
        {"nprobe": 1, "recall_at_k": 0.70, "gets_per_query": 1.0},
        {"nprobe": 2, "recall_at_k": 0.85, "gets_per_query": 2.0},
        {"nprobe": 4, "recall_at_k": 0.97, "gets_per_query": 4.0},
        {"nprobe": 8, "recall_at_k": 0.98, "gets_per_query": 8.0},
        {"nprobe": 16, "recall_at_k": 0.981, "gets_per_query": 16.0},
    ]

    bend = ANALYSIS.curve_bend(points, "nprobe")

    assert bend["nprobe"] == 4


def test_curve_bend_uses_cost_frontier_and_drops_dominated_points():
    points = [
        {"nprobe": 1, "recall_at_k": 0.70, "gets_per_query": 3.0},
        {"nprobe": 2, "recall_at_k": 0.85, "gets_per_query": 2.0},
        {"nprobe": 4, "recall_at_k": 0.97, "gets_per_query": 4.0},
        {"nprobe": 8, "recall_at_k": 0.98, "gets_per_query": 8.0},
        {"nprobe": 16, "recall_at_k": 0.981, "gets_per_query": 16.0},
    ]

    frontier = ANALYSIS.pareto_frontier(points, "gets_per_query")
    bend = ANALYSIS.curve_bend(points, "gets_per_query")

    assert [point["nprobe"] for point in frontier] == [2, 4, 8, 16]
    assert bend["nprobe"] == 4


def test_power_law_fit_recovers_known_relationship_and_loocv():
    samples = [
        {"coarse_cells": k, "nprobe": 1.5 * (k**0.6)} for k in (3, 10, 30, 100, 300)
    ]

    fit = ANALYSIS.fit_power_law(samples)

    assert math.isclose(fit["coefficient"], 1.5, rel_tol=1e-12)
    assert math.isclose(fit["exponent"], 0.6, rel_tol=1e-12)
    assert math.isclose(fit["r_squared_log"], 1.0, rel_tol=1e-12)
    assert fit["loocv_mape"] < 1e-12


def test_fit_rejects_fewer_than_three_independent_scales():
    try:
        ANALYSIS.fit_power_law(
            [
                {"coarse_cells": 3, "nprobe": 2},
                {"coarse_cells": 10, "nprobe": 4},
            ]
        )
    except RuntimeError as error:
        assert "at least three" in str(error)
    else:
        raise AssertionError("fit unexpectedly accepted two points")


def matrix_result(
    points: list[dict],
    *,
    status: str = "pass",
    corpus_rows: int = 100_000,
    source_revision: str = "source-revision",
    expected_nprobes: list[int] | None = None,
) -> dict:
    nprobes = expected_nprobes or sorted({point["nprobe"] for point in points})
    top_k_values = sorted({point["top_k"] for point in points})
    return {
        "protocol": "pax_nprobe_topk_matrix",
        "status": status,
        "git_revision": source_revision,
        "binary": {
            "sha256": "binary-sha",
            "source_revision": source_revision,
        },
        "bed_config": {"sha256": "config-sha"},
        "dataset": {
            "corpus_rows": corpus_rows,
            "dimension": 128,
            "groundtruth_scope_rows": corpus_rows,
            "query_range": [0, 1000],
        },
        "filesystem_profile": {"storage_url": "az://benchmarks/run-1"},
        "compute_profile": {"architecture": "arm64"},
        "settled_geometry": {
            "segment_count": 1,
            "row_count": corpus_rows,
            "segments": [
                {
                    "path": "run-1/1/data/segment.pax",
                    "blob_etag": "etag-1",
                    "bytes": 10_000,
                    "layout_version": 3,
                    "coarse_cells": 4,
                    "coarse_seed": 17,
                }
            ],
        },
        "matrix": {
            "nprobes": nprobes,
            "top_k_values": top_k_values,
            "target_recall": 0.98,
            "quality_policy": "report",
            "points": points,
        },
        "measurement_failures": [],
        "quality_outcomes": [],
        "checkpoint": {
            "state": status,
            "completed_points": len(points),
            "expected_points": len(nprobes) * len(top_k_values),
            "incomplete_reason": None if status == "pass" else "interrupted",
        },
    }


def measured_point(nprobe: int, recall: float, gets: float) -> dict:
    return {
        "nprobe": nprobe,
        "top_k": 10,
        "recall_at_k": recall,
        "gets_per_query": gets,
        "bytes_per_query": gets * 1_000_000,
        "latency_ms": {"p50": gets * 3.0, "p95": gets * 5.0},
        "ivf": {"probed_rows_per_query": nprobe * 25_000.0},
    }


def write_matrix(path: Path, result: dict) -> None:
    path.write_text(json.dumps(result))


def test_build_analysis_merges_provenance_identical_partial_checkpoints(tmp_path: Path):
    first = tmp_path / "first.json"
    second = tmp_path / "second.json"
    write_matrix(
        first,
        matrix_result(
            [measured_point(1, 0.90, 2.0), measured_point(2, 0.96, 4.0)],
            status="incomplete",
            expected_nprobes=[1, 2, 5],
        ),
    )
    write_matrix(
        second,
        matrix_result(
            [measured_point(3, 0.974, 6.0), measured_point(4, 0.975, 8.0)],
            status="incomplete",
            expected_nprobes=[3, 4, 6],
        ),
    )

    analysis = ANALYSIS.build_analysis([first, second], 0.98)

    scale = analysis["scales"][0]
    curve = scale["curves"]["10"]
    assert scale["measurement_coverage"]["status"] == "partial"
    assert scale["measurement_coverage"]["measured_point_count"] == 4
    assert scale["measurement_coverage"]["expected_point_count"] == 6
    assert scale["measurement_coverage"]["completion_fraction"] == pytest.approx(2 / 3)
    assert len(scale["sources"]) == 2
    assert [point["nprobe"] for point in scale["points"]] == [1, 2, 3, 4]
    assert curve["natural_knees"]["gets_per_query"]["status"] == "measured"
    assert curve["quality_profile"]["status"] == "unattained"
    assert analysis["fits"]["natural_knee"]["10"]["status"] == ("insufficient_samples")
    assert analysis["fits"]["quality_floor"]["10"]["status"] == ("insufficient_samples")


def test_build_analysis_rejects_same_scale_with_different_provenance(tmp_path: Path):
    first = tmp_path / "first.json"
    second = tmp_path / "second.json"
    write_matrix(first, matrix_result([measured_point(1, 0.90, 2.0)]))
    write_matrix(
        second,
        matrix_result(
            [measured_point(2, 0.96, 4.0)],
            source_revision="different-source",
        ),
    )

    with pytest.raises(RuntimeError, match="provenance"):
        ANALYSIS.build_analysis([first, second], 0.98)


def test_quality_gated_failure_is_complete_measurement_evidence(tmp_path: Path):
    path = tmp_path / "quality-fail.json"
    result = matrix_result(
        [
            measured_point(1, 0.90, 2.0),
            measured_point(2, 0.96, 4.0),
            measured_point(3, 0.975, 6.0),
        ],
        status="fail",
    )
    result["matrix"]["quality_policy"] = "require"
    result["quality_outcomes"] = [
        {
            "top_k": 10,
            "target_recall": 0.98,
            "status": "unattained",
            "max_measured_recall": 0.975,
        }
    ]
    write_matrix(path, result)

    analysis = ANALYSIS.build_analysis([path], 0.98)

    assert analysis["scales"][0]["measurement_coverage"]["status"] == "complete"
    assert analysis["scales"][0]["curves"]["10"]["quality_profile"]["status"] == (
        "unattained"
    )


def test_load_matrix_rejects_active_checkpoint(tmp_path: Path):
    path = tmp_path / "running.json"
    write_matrix(
        path,
        matrix_result([measured_point(1, 0.90, 2.0)], status="running"),
    )

    with pytest.raises(RuntimeError, match="still running"):
        ANALYSIS.load_matrix(path)


def test_load_matrix_rejects_terminal_checkpoint_missing_points(tmp_path: Path):
    path = tmp_path / "truncated.json"
    result = matrix_result([measured_point(1, 0.90, 2.0)])
    result["checkpoint"]["expected_points"] = 2
    write_matrix(path, result)

    with pytest.raises(RuntimeError, match="missing measured points"):
        ANALYSIS.load_matrix(path)


def test_load_matrix_accepts_complete_pre_checkpoint_evidence(tmp_path: Path):
    path = tmp_path / "legacy-complete.json"
    result = matrix_result([measured_point(1, 0.90, 2.0)])
    result.pop("checkpoint")
    write_matrix(path, result)

    assert ANALYSIS.load_matrix(path)["status"] == "pass"


def test_fit_marks_partial_evidence_as_provisional():
    samples = [
        {
            "corpus_rows": rows,
            "coarse_cells": cells,
            "nprobe": probe,
            "measurement_coverage": coverage,
        }
        for rows, cells, probe, coverage in (
            (100_000, 3, 2, "complete"),
            (1_000_000, 30, 7, "complete"),
            (10_000_000, 305, 30, "partial"),
        )
    ]

    fit = ANALYSIS._fit_series(samples)

    assert fit["status"] == "fit"
    assert fit["evidence_status"] == "provisional_partial_input"


def test_dual_axis_and_pareto_svgs_encode_decision_axes(tmp_path: Path):
    point_low = {
        "nprobe": 1,
        "top_k": 10,
        "recall_at_k": 0.90,
        "gets_per_query": 2.0,
        "ivf": {"probed_rows_per_query": 25_000.0},
    }
    point_middle = {
        "nprobe": 2,
        "top_k": 10,
        "recall_at_k": 0.970,
        "gets_per_query": 6.0,
        "ivf": {"probed_rows_per_query": 50_000.0},
    }
    point_quality = {
        "nprobe": 4,
        "top_k": 10,
        "recall_at_k": 0.985,
        "gets_per_query": 12.0,
        "ivf": {"probed_rows_per_query": 75_000.0},
    }
    analysis = {
        "target_recall": 0.98,
        "scales": [
            {
                "corpus_rows": 100_000,
                "coarse_cells": 4,
                "points": [point_low, point_middle, point_quality],
                "curves": {
                    "10": {
                        "economy_profile": {
                            "objective": "gets_per_query",
                            "status": "measured",
                            "point": point_middle,
                        },
                        "quality_profile": {
                            "status": "attained",
                            "target_recall": 0.98,
                            "point": point_quality,
                        },
                    }
                },
            }
        ],
    }
    dual = tmp_path / "dual.svg"
    pareto = tmp_path / "pareto.svg"

    ANALYSIS.write_dual_axis_svg(analysis, dual)
    ANALYSIS.write_pareto_svg(analysis, pareto)

    dual_text = dual.read_text()
    pareto_text = pareto.read_text()
    assert "Normalized probe fraction (nprobe / k_c)" in dual_text
    assert "Physical GET/query" in dual_text
    assert "Axis crossings are not optima" in dual_text
    assert "N=100,000; k_c=4; top_k=10; nprobe=2" in dual_text
    assert "Recall → GET/query Pareto frontier" in pareto_text
    assert "Bubble area scales with actual rows probed / corpus rows" in pareto_text
    assert "rows probed=75000" in pareto_text
    assert 'class="natural-knee"' in pareto_text
    assert 'class="quality-floor"' in pareto_text
