import copy
import importlib.util
import json
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "bench" / "nprobe_sweep.py"
SPEC = importlib.util.spec_from_file_location("nprobe_sweep", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
SWEEP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SWEEP)


def expected_result() -> dict:
    return {
        "protocol": "pax_nprobe_topk_matrix",
        "git_revision": "abc123",
        "collection_id": "1",
        "binary": {"sha256": "binary-sha", "source_revision": "source"},
        "bed_config": {"sha256": "config-sha", "port": 5690},
        "dataset": {"corpus_rows": 100_000, "query_range": [0, 1000]},
        "filesystem_profile": {"storage_url": "az://benchmarks/run-1"},
        "compute_profile": {"architecture": "arm64"},
        "settled_geometry": {"segment_count": 1, "row_count": 100_000},
        "matrix": {
            "nprobes": [1, 2],
            "top_k_values": [10, 20],
            "target_recall": 0.98,
            "quality_policy": "require",
            "points": [],
        },
        "measurement_failures": [],
        "quality_outcomes": [],
    }


def test_atomic_checkpoint_records_completed_and_expected_points(tmp_path: Path):
    output = tmp_path / "matrix.json"
    result = expected_result()
    result["matrix"]["points"].append({"nprobe": 1, "top_k": 10, "recall_at_k": 0.9})

    SWEEP.write_checkpoint(output, result, "running")

    persisted = json.loads(output.read_text())
    assert persisted["status"] == "running"
    assert persisted["checkpoint"] == {
        "completed_points": 1,
        "expected_points": 4,
        "incomplete_reason": None,
        "state": "running",
    }
    assert not (tmp_path / ".matrix.json.tmp").exists()


def test_resume_accepts_only_matching_unique_completed_points():
    expected = expected_result()
    existing = copy.deepcopy(expected)
    existing["status"] = "incomplete"
    existing["matrix"]["points"] = [{"nprobe": 1, "top_k": 10, "recall_at_k": 0.9}]

    assert SWEEP.validate_resume(existing, expected) == {(1, 10)}

    wrong_binary = copy.deepcopy(existing)
    wrong_binary["binary"]["sha256"] = "different"
    with pytest.raises(RuntimeError, match="provenance/configuration"):
        SWEEP.validate_resume(wrong_binary, expected)

    wrong_config = copy.deepcopy(existing)
    wrong_config["bed_config"]["sha256"] = "different"
    with pytest.raises(RuntimeError, match="provenance/configuration"):
        SWEEP.validate_resume(wrong_config, expected)

    duplicate = copy.deepcopy(existing)
    duplicate["matrix"]["points"].append(copy.deepcopy(existing["matrix"]["points"][0]))
    with pytest.raises(RuntimeError, match="duplicate point"):
        SWEEP.validate_resume(duplicate, expected)


def test_resume_rejects_terminal_checkpoint():
    expected = expected_result()
    existing = copy.deepcopy(expected)
    existing["status"] = "pass"

    with pytest.raises(RuntimeError, match="terminal"):
        SWEEP.validate_resume(existing, expected)


def test_quality_policy_reports_unattained_without_invalidating_measurement():
    points = [
        {"top_k": 10, "recall_at_k": 0.975},
        {"top_k": 20, "recall_at_k": 0.981},
    ]

    outcomes = SWEEP.evaluate_quality(points, [10, 20], 0.98)

    assert outcomes == [
        {
            "top_k": 10,
            "target_recall": 0.98,
            "status": "unattained",
            "max_measured_recall": 0.975,
        },
        {
            "top_k": 20,
            "target_recall": 0.98,
            "status": "attained",
            "max_measured_recall": 0.981,
        },
    ]
    assert SWEEP.final_status([], outcomes, "report") == "pass"
    assert SWEEP.final_status([], outcomes, "require") == "fail"


def test_measurement_failure_always_fails_regardless_of_quality_policy():
    outcomes = [
        {
            "top_k": 10,
            "target_recall": 0.98,
            "status": "attained",
            "max_measured_recall": 0.99,
        }
    ]

    assert SWEEP.final_status(["cell attribution mismatch"], outcomes, "report") == (
        "fail"
    )
