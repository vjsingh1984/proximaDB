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


def test_binary_neutral_paths_follow_language_boundary_not_filename_allowlist():
    assert SWEEP.is_binary_neutral_path("docs/12-design/adr/example.adoc")
    assert SWEEP.is_binary_neutral_path("scripts/bench/new_experiment.py")
    assert SWEEP.is_binary_neutral_path("tests/python/test_new_experiment.py")
    assert not SWEEP.is_binary_neutral_path("src/storage/engines/sst/search/mod.rs")
    assert not SWEEP.is_binary_neutral_path("Cargo.toml")


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
            "coalesce_gap_bytes": 1024 * 1024,
            "coalesce_range_bytes": 4 * 1024 * 1024,
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


def test_checkpoint_loading_auto_resumes_only_non_terminal_states(tmp_path: Path):
    output = tmp_path / "matrix.json"
    assert SWEEP.load_resumable_checkpoint(output) is None

    for state in ("running", "incomplete"):
        checkpoint = expected_result()
        checkpoint["status"] = state
        output.write_text(json.dumps(checkpoint))
        assert SWEEP.load_resumable_checkpoint(output) == checkpoint

    for state in ("pass", "fail", "unknown"):
        checkpoint = expected_result()
        checkpoint["status"] = state
        output.write_text(json.dumps(checkpoint))
        with pytest.raises(RuntimeError, match="terminal or invalid"):
            SWEEP.load_resumable_checkpoint(output)


def test_checkpoint_loading_rejects_malformed_json(tmp_path: Path):
    output = tmp_path / "matrix.json"
    output.write_text("{not-json")

    with pytest.raises(RuntimeError, match="cannot read matrix checkpoint"):
        SWEEP.load_resumable_checkpoint(output)


def test_matrix_lock_is_single_writer_and_recovers_after_close(tmp_path: Path):
    output = tmp_path / "matrix.json"
    first = SWEEP.acquire_matrix_lock(output)
    try:
        with pytest.raises(RuntimeError, match="already owns checkpoint"):
            SWEEP.acquire_matrix_lock(output)
    finally:
        first.close()

    recovered = SWEEP.acquire_matrix_lock(output)
    recovered.close()


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


def test_resume_ignores_snapshot_mtime_but_still_detects_segment_change():
    """A re-materialised snapshot must not defeat resume, but a real change must.

    `materialize()` re-downloads the segment into the run's pax-snapshot dir on
    every invocation, so the local copy's `mtime_ns` differs each time. Treating
    that as immutable provenance made `--resume` impossible for a byte-identical
    segment. Segment identity is pinned by blob_etag + bytes + path instead.
    """
    geometry = {
        "segment_count": 1,
        "row_count": 100_000,
        "segments": [
            {
                "path": "run-1/1/data/L3.pax",
                "bytes": 2757638547,
                "blob_etag": "0xETAG",
                "mtime_ns": 1785995948231058365,
            }
        ],
    }
    existing = expected_result()
    existing["settled_geometry"] = geometry
    existing["status"] = "running"
    existing["matrix"] = dict(existing["matrix"], points=[])

    # Same segment, re-materialised: only the local mtime moved.
    remateralised = copy.deepcopy(existing)
    remateralised["settled_geometry"]["segments"][0]["mtime_ns"] = 1785998237107866341
    assert SWEEP.checkpoint_identity(existing) == SWEEP.checkpoint_identity(
        remateralised
    )
    assert SWEEP.validate_resume(existing, remateralised) == set()

    # A genuinely different segment must still be rejected.
    for field, value in (("blob_etag", "0xOTHER"), ("bytes", 42), ("path", "other")):
        changed = copy.deepcopy(existing)
        changed["settled_geometry"]["segments"][0][field] = value
        assert SWEEP.checkpoint_identity(existing) != SWEEP.checkpoint_identity(changed)
        with pytest.raises(RuntimeError, match="provenance/configuration differs"):
            SWEEP.validate_resume(existing, changed)


def test_read_planner_is_part_of_the_resume_identity():
    """A matrix must never mix read planners across its points.

    Every recall-floor nprobe published to date was measured at the Azure 4 MiB
    planner because the sweep could not set it (TD-IVF-3 / TD-SEARCH-3). Now
    that it can, resuming a 4 MiB matrix under a 24 MiB planner would silently
    splice two geometries into one curve that no single configuration ever
    exhibited.
    """
    existing = expected_result()
    existing["status"] = "running"

    same = copy.deepcopy(existing)
    assert SWEEP.checkpoint_identity(existing) == SWEEP.checkpoint_identity(same)
    assert SWEEP.validate_resume(existing, same) == set()

    for field, value in (
        ("coalesce_range_bytes", 24 * 1024 * 1024),
        ("coalesce_gap_bytes", 4 * 1024 * 1024),
    ):
        changed = copy.deepcopy(existing)
        changed["matrix"][field] = value
        assert SWEEP.checkpoint_identity(existing) != SWEEP.checkpoint_identity(changed)
        with pytest.raises(RuntimeError, match="provenance/configuration differs"):
            SWEEP.validate_resume(existing, changed)


def test_planner_defaults_reproduce_the_previous_pinned_behaviour():
    """An unset invocation must be byte-identical to the pre-change sweep."""
    parser_defaults = {
        action.dest: action.default
        for action in SWEEP.build_parser()._actions
        if action.dest in {"coalesce_gap_mib", "coalesce_range_mib"}
    }
    assert parser_defaults["coalesce_gap_mib"] * SWEEP.MIB == (
        SWEEP.ACCEPTANCE.AZURE_COALESCE_GAP_BYTES
    )
    assert parser_defaults["coalesce_range_mib"] * SWEEP.MIB == (
        SWEEP.ACCEPTANCE.AZURE_COALESCE_RANGE_BYTES
    )
