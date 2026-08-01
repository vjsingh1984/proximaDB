import importlib.util
import math
from pathlib import Path

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


def test_quality_floor_prefers_first_probe_that_meets_recall():
    points = [
        {"nprobe": 1, "recall_at_k": 0.90, "gets_per_query": 1.0},
        {"nprobe": 2, "recall_at_k": 0.979, "gets_per_query": 2.0},
        {"nprobe": 4, "recall_at_k": 0.981, "gets_per_query": 4.0},
        {"nprobe": 8, "recall_at_k": 0.990, "gets_per_query": 8.0},
    ]

    knee = ANALYSIS.quality_floor(points, 0.98)

    assert knee["nprobe"] == 4
    assert knee["recall_at_k"] == 0.981


def test_curve_bend_detects_diminishing_return_before_saturation():
    points = [
        {"nprobe": 1, "recall_at_k": 0.70, "gets_per_query": 1.0},
        {"nprobe": 2, "recall_at_k": 0.85, "gets_per_query": 2.0},
        {"nprobe": 4, "recall_at_k": 0.97, "gets_per_query": 4.0},
        {"nprobe": 8, "recall_at_k": 0.98, "gets_per_query": 8.0},
        {"nprobe": 16, "recall_at_k": 0.981, "gets_per_query": 16.0},
    ]

    bend = ANALYSIS.curve_bend(points)

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


def test_dual_axis_and_pareto_svgs_encode_decision_axes(tmp_path: Path):
    point_low = {
        "nprobe": 1,
        "top_k": 10,
        "recall_at_k": 0.90,
        "gets_per_query": 2.0,
        "ivf": {"probed_rows_per_query": 25_000.0},
    }
    point_knee = {
        "nprobe": 2,
        "top_k": 10,
        "recall_at_k": 0.985,
        "gets_per_query": 6.0,
        "ivf": {"probed_rows_per_query": 50_000.0},
    }
    analysis = {
        "target_recall": 0.98,
        "scales": [
            {
                "corpus_rows": 100_000,
                "coarse_cells": 4,
                "points": [point_low, point_knee],
                "curves": {
                    "10": {
                        "quality_floor": point_knee,
                        "curve_bend": point_low,
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
    assert "rows probed=50000" in pareto_text
    assert 'class="knee"' in pareto_text
