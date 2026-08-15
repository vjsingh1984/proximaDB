import copy
import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest

SCRIPT = (
    Path(__file__).resolve().parents[2] / "scripts" / "bench" / "range_cap_sweep.py"
)
SPEC = importlib.util.spec_from_file_location("range_cap_sweep", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
SWEEP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SWEEP)


def test_wire_log_counts_unique_matching_get_requests_and_range_lengths(tmp_path: Path):
    log = tmp_path / "azurite-debug.log"
    log.write_text(
        "2026-08-12T00:00:00Z request-a info: "
        "BlobStorageContextMiddleware: RequestMethod=GET "
        "RequestURL=http://127.0.0.1/devstoreaccount1/bench/run-k30/a.pax "
        'RequestHeaders:{"range":"bytes=0-4194303"}\n'
        "2026-08-12T00:00:00Z request-a info: unrelated duplicate detail\n"
        "2026-08-12T00:00:01Z request-b info: "
        "BlobStorageContextMiddleware: RequestMethod=GET "
        "RequestURL=http://127.0.0.1/devstoreaccount1/bench/other/b.pax "
        'RequestHeaders:{"range":"bytes=0-8388607"}\n'
        "2026-08-12T00:00:02Z request-c info: "
        "BlobStorageContextMiddleware: RequestMethod=HEAD "
        "RequestURL=http://127.0.0.1/devstoreaccount1/bench/run-k30/a.pax "
        "RequestHeaders:{}\n"
        "2026-08-12T00:00:03Z request-d info: "
        "BlobStorageContextMiddleware: RequestMethod=GET "
        "RequestURL=http://127.0.0.1/devstoreaccount1/bench/run-k30/a.pax "
        "RequestHeaders:{}\n"
    )
    observer = SWEEP.AzuriteWireLog(log, "bench", "run-k30")

    sample = observer.sample(0)

    assert sample["get_requests"] == 2
    assert sample["http_requests"] == 3
    assert sample["requests_by_method"] == {"GET": 2, "HEAD": 1}
    assert sample["range_get_requests"] == 1
    assert sample["full_get_requests"] == 1
    assert sample["requested_range_bytes"] == 4 * 1024 * 1024
    assert sample["unique_request_ids"] == 3


def test_wire_log_snapshot_is_an_append_offset(tmp_path: Path):
    log = tmp_path / "azurite-debug.log"
    log.write_text("old line\n")
    observer = SWEEP.AzuriteWireLog(log, "bench", "run-k30")
    offset = observer.snapshot()
    with log.open("a") as output:
        output.write(
            "2026-08-12T00:00:00Z request-a info: "
            "BlobStorageContextMiddleware: RequestMethod=GET "
            "RequestURL=http://127.0.0.1/devstoreaccount1/bench/run-k30/a.pax "
            'RequestHeaders:{"range":"bytes=8-15"}\n'
        )

    sample = observer.sample(offset)

    assert sample["get_requests"] == 1
    assert sample["requested_range_bytes"] == 8


def test_storage_scope_requires_canonical_azure_url():
    assert SWEEP.azure_storage_scope("az://bench/run-k30") == ("bench", "run-k30")
    with pytest.raises(RuntimeError, match="canonical az"):
        SWEEP.azure_storage_scope("file:///tmp/bed")


def test_cap_values_are_positive_unique_mib():
    assert SWEEP.cap_mib_values("4,8,16,32") == [4, 8, 16, 32]
    with pytest.raises(RuntimeError, match="duplicates"):
        SWEEP.cap_mib_values("4,8,4")
    with pytest.raises(RuntimeError, match="positive"):
        SWEEP.cap_mib_values("0,4")


def checkpoint_result() -> dict:
    return {
        "protocol": "pax_azure_range_cap_sweep",
        "status": "running",
        "git_revision": "abc",
        "collection_id": "1",
        "binary": {"sha256": "bin"},
        "bed_config": {"sha256": "cfg"},
        "dataset": {"corpus_rows": 491655},
        "filesystem_profile": {"storage_url": "az://bench/run-k30"},
        "compute_profile": {"architecture": "arm64"},
        "settled_geometry": {
            "segment_count": 1,
            "segments": [{"path": "a.pax", "mtime_ns": 1}],
        },
        "experiment": {
            "isolated_variable": "max_coalesced_range_bytes",
            "fixed_nprobe": 12,
            "fixed_coalesce_gap_bytes": 1024 * 1024,
            "range_caps_mib": [4, 8],
            "top_k_values": [10, 20],
            "fresh_process_per_point": True,
            "target_recall": 0.98,
            "decision_thresholds": {},
            "points": [],
        },
        "measurement_failures": [],
        "decisions": [],
    }


def test_checkpoint_resume_ignores_materialized_mtime_and_rejects_cap_change():
    existing = checkpoint_result()
    existing["experiment"]["points"] = [{"range_cap_mib": 4, "top_k": 10}]
    expected = copy.deepcopy(existing)
    expected["experiment"]["points"] = []
    expected["settled_geometry"]["segments"][0]["mtime_ns"] = 2

    assert SWEEP.validate_resume(existing, expected) == {(4, 10)}

    changed = copy.deepcopy(expected)
    changed["experiment"]["range_caps_mib"] = [4, 16]
    with pytest.raises(RuntimeError, match="provenance/configuration"):
        SWEEP.validate_resume(existing, changed)


def test_checkpoint_write_is_atomic_and_records_progress(tmp_path: Path):
    output = tmp_path / "range-cap.json"
    result = checkpoint_result()
    result["experiment"]["points"] = [{"range_cap_mib": 4, "top_k": 10}]

    SWEEP.write_checkpoint(output, result, "running")

    persisted = SWEEP.json.loads(output.read_text())
    assert persisted["checkpoint"] == {
        "state": "running",
        "completed_points": 1,
        "expected_points": 4,
        "incomplete_reason": None,
    }
    assert not (tmp_path / ".range-cap.json.tmp").exists()


def test_wire_validation_rejects_dead_observer_when_application_read_storage():
    point = {
        "physical_gets": 12.0,
        "wire_http": {"get_requests": 0, "range_get_requests": 0},
    }

    with pytest.raises(RuntimeError, match="observed zero HTTP GETs"):
        SWEEP.validate_wire_observation("range-16mib-top-10", point)


def test_decision_matches_application_reads_to_ranged_gets_not_control_gets():
    args = SimpleNamespace(
        recall_tolerance=1e-12,
        target_recall=0.98,
        min_wire_get_reduction=0.2,
        max_byte_amplification=1.5,
        max_latency_ratio=1.1,
        max_rss_ratio=1.1,
    )
    baseline = {
        "range_cap_mib": 4,
        "recall_at_k": 0.99,
        "physical_gets": 439.0,
        "bytes_read": 100.0,
        "latency_ms": {"p50": 10.0, "p95": 20.0},
        "process_rss": {"peak_bytes": 1_000},
        "wire_http": {
            "get_requests": 440,
            "range_get_requests": 439,
            "full_get_requests": 1,
        },
    }
    candidate = copy.deepcopy(baseline)
    candidate.update(
        {
            "range_cap_mib": 16,
            "physical_gets": 158.0,
            "bytes_read": 102.0,
            "latency_ms": {"p50": 8.0, "p95": 16.0},
            "process_rss": {"peak_bytes": 1_050},
            "wire_http": {
                "get_requests": 159,
                "range_get_requests": 158,
                "full_get_requests": 1,
            },
        }
    )

    decision = SWEEP.decision_for(candidate, baseline, args)

    assert decision["checks"]["one_wire_range_get_per_application_get"] is True
    assert decision["wire_range_to_application_get_ratio"] == 1.0
    assert decision["promotion_eligible"] is True


def test_decision_localizes_result_set_and_order_mismatches_without_relaxing_gate():
    args = SimpleNamespace(
        recall_tolerance=1e-12,
        target_recall=0.98,
        min_wire_get_reduction=0.2,
        max_byte_amplification=1.5,
        max_latency_ratio=1.1,
        max_rss_ratio=1.1,
    )
    baseline = {
        "range_cap_mib": 4,
        "recall_at_k": 0.99,
        "physical_gets": 100.0,
        "bytes_read": 100.0,
        "latency_ms": {"p50": 10.0, "p95": 20.0},
        "process_rss": {"peak_bytes": 1_000},
        "wire_http": {"get_requests": 101, "range_get_requests": 100},
        "result_identity": {
            "ordered_ids_sha256_by_query": ["order-a", "order-b"],
            "set_ids_sha256_by_query": ["set-a", "set-b"],
            "recall_hits_by_query": [10, 9],
        },
    }
    candidate = copy.deepcopy(baseline)
    candidate.update(
        {
            "range_cap_mib": 24,
            "physical_gets": 60.0,
            "bytes_read": 103.0,
            "latency_ms": {"p50": 8.0, "p95": 16.0},
            "process_rss": {"peak_bytes": 1_050},
            "wire_http": {"get_requests": 61, "range_get_requests": 60},
            "result_identity": {
                "ordered_ids_sha256_by_query": ["order-a", "order-c"],
                "set_ids_sha256_by_query": ["set-a", "set-b"],
                "recall_hits_by_query": [10, 8],
            },
        }
    )

    decision = SWEEP.decision_for(candidate, baseline, args)

    assert decision["result_identity_diagnostics"] == {
        "ordered_result_mismatch_count": 1,
        "ordered_result_first_mismatch_queries": [1],
        "result_set_mismatch_count": 0,
        "result_set_first_mismatch_queries": [],
        "recall_hits_mismatch_count": 1,
        "recall_hits_first_mismatch_queries": [1],
    }
    assert decision["promotion_eligible"] is True
