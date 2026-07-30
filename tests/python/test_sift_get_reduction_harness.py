"""Focused contracts for the auditable SIFT GET-reduction harness."""

from __future__ import annotations

import importlib.util
import json
import struct
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import pytest

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
HARNESS_PATH = REPOSITORY_ROOT / "scripts" / "bench" / "sift1m_get_reduction.py"
SPEC = importlib.util.spec_from_file_location(
    "sift1m_get_reduction_harness", HARNESS_PATH
)
assert SPEC is not None and SPEC.loader is not None
HARNESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HARNESS)
MATRIX_PATH = REPOSITORY_ROOT / "scripts" / "bench" / "nprobe_sweep.py"
MATRIX_SPEC = importlib.util.spec_from_file_location(
    "sift_nprobe_matrix", MATRIX_PATH
)
assert MATRIX_SPEC is not None and MATRIX_SPEC.loader is not None
MATRIX = importlib.util.module_from_spec(MATRIX_SPEC)
MATRIX_SPEC.loader.exec_module(MATRIX)


def test_matrix_contract_rejects_duplicate_probes_and_wrong_geometry() -> None:
    assert MATRIX.comma_separated_ints("1,2,4", "--nprobes") == [1, 2, 4]
    with pytest.raises(RuntimeError, match="duplicates"):
        MATRIX.comma_separated_ints("1,2,1", "--nprobes")
    with pytest.raises(RuntimeError, match="rows"):
        MATRIX.validate_geometry(
            {
                "row_count": 99,
                "segment_count": 1,
                "segments": [
                    {"layout_version": 3, "coarse_cells": 4}
                ],
            },
            rows=100,
            max_segments=1,
            layout_version=3,
        )


def test_a0_geometry_reports_cell_shape_and_verifies_checksum() -> None:
    dimension = 2
    components = 1
    rows = [3, 0]
    radii = [1.5, 0.0]
    encoded = bytearray(b"PXA0")
    encoded.extend(bytes([1, 0]))
    encoded.extend(struct.pack("<HIIQQQ", components, len(rows), dimension, 7, 5, sum(rows)))
    encoded.extend(struct.pack("<2f", 0.0, 0.0))
    encoded.extend(struct.pack("<2f", 1.0, 0.0))
    encoded.extend(struct.pack("<2f", 0.0, 2.0))
    encoded.extend(struct.pack("<2f", *radii))
    row_begin = 0
    for row_count in rows:
        row_end = row_begin + row_count
        encoded.extend(
            struct.pack(
                "<QQQQQQQQII",
                row_begin,
                row_end,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            )
        )
        row_begin = row_end
    encoded.extend(struct.pack("<Q", HARNESS.fnv1a64(encoded)))

    geometry = HARNESS.parse_a0_geometry(bytes(encoded))

    assert geometry["coarse_cells"] == 2
    assert geometry["coarse_trained_rows"] == 5
    assert geometry["cell_rows"] == rows
    assert geometry["empty_cell_fraction"] == 0.5
    assert geometry["cell_row_max_to_mean"] == 2.0
    assert geometry["radii"] == radii

    encoded[-1] ^= 1
    with pytest.raises(RuntimeError, match="checksum"):
        HARNESS.parse_a0_geometry(bytes(encoded))


def test_azure_inventory_scopes_prefix_and_records_stable_identity(
    tmp_path: Path,
) -> None:
    payload = [
        {
            "name": "run-1/collections/7/segment.pax",
            "properties": {
                "contentLength": 1234,
                "etag": "etag-1",
                "lastModified": "2026-07-30T00:00:00Z",
            },
        },
        {
            "name": "run-1/collections/7/manifest.json",
            "properties": {"contentLength": 50, "etag": "etag-2"},
        },
    ]
    completed = SimpleNamespace(stdout=json.dumps(payload))
    geometry = HARNESS.AzureCliPaxGeometry(
        "adls://benchmarks/run-1", tmp_path
    )

    with patch.object(HARNESS.subprocess, "run", return_value=completed) as run:
        inventory = geometry.inventory()

    command = run.call_args.args[0]
    assert command[command.index("--container-name") + 1] == "benchmarks"
    assert command[command.index("--prefix") + 1] == "run-1"
    assert inventory == {
        "segment_count": 1,
        "bytes": 1234,
        "segments": [
            {
                "path": "run-1/collections/7/segment.pax",
                "bytes": 1234,
                "etag": "etag-1",
                "last_modified": "2026-07-30T00:00:00Z",
            }
        ],
    }
    assert geometry.stable_signature(inventory) == (
        ("run-1/collections/7/segment.pax", 1234, "etag-1"),
    )


@pytest.mark.parametrize("blob_name", ["/absolute.pax", "../escape.pax"])
def test_azure_snapshot_rejects_paths_outside_evidence_root(
    tmp_path: Path, blob_name: str
) -> None:
    geometry = HARNESS.AzureCliPaxGeometry(
        "adls://benchmarks/run-1", tmp_path
    )

    with pytest.raises(RuntimeError, match="unsafe Azure blob name"):
        geometry._snapshot_target(blob_name)


def test_config_preserves_object_store_url(tmp_path: Path) -> None:
    config_path = tmp_path / "benchmark.toml"
    storage_url = "adls://benchmarks/run-1"

    HARNESS.write_config(config_path, tmp_path, 5690, 128, 20_000, storage_url)

    config = config_path.read_text()
    assert f'url = "{storage_url}"' in config
    assert "[storage.optimization]\nenable_mmap = false" in config
    assert "vector_count_threshold = 20000" in config


def test_explicit_flush_uses_supported_flight_action() -> None:
    calls = {}

    class FakeLocation:
        @staticmethod
        def for_grpc_tcp(host: str, port: int) -> tuple[str, int]:
            return host, port

    class FakeClient:
        def __init__(self, location: tuple[str, int]):
            calls["location"] = location

        def do_action(self, action):
            calls["action"] = action
            return [
                SimpleNamespace(
                    body=json.dumps(
                        {
                            "success": True,
                            "collection_id": "7",
                            "operation": "flush",
                        }
                    ).encode()
                )
            ]

        def close(self) -> None:
            calls["closed"] = True

    class FakeAction:
        def __init__(self, action_type: str, body: bytes):
            self.type = action_type
            self.body = body

    flight = SimpleNamespace(
        Location=FakeLocation,
        FlightClient=FakeClient,
        Action=FakeAction,
    )

    response = HARNESS.force_flush_via_flight(
        "127.0.0.1", 5692, "7", flight
    )

    assert response["success"] is True
    assert calls["location"] == ("127.0.0.1", 5692)
    assert calls["action"].type == "flush_collection"
    assert json.loads(calls["action"].body) == {"collection_id": "7"}
    assert calls["closed"] is True


def test_explicit_compaction_uses_supported_flight_action() -> None:
    calls = {}

    class FakeLocation:
        @staticmethod
        def for_grpc_tcp(host: str, port: int) -> tuple[str, int]:
            return host, port

    class FakeClient:
        def __init__(self, location: tuple[str, int]):
            calls["location"] = location

        def do_action(self, action):
            calls["action"] = action
            return [
                SimpleNamespace(
                    body=json.dumps(
                        {
                            "success": True,
                            "collection_id": "7",
                            "operation": "compact",
                        }
                    ).encode()
                )
            ]

        def close(self) -> None:
            calls["closed"] = True

    class FakeAction:
        def __init__(self, action_type: str, body: bytes):
            self.type = action_type
            self.body = body

    flight = SimpleNamespace(
        Location=FakeLocation,
        FlightClient=FakeClient,
        Action=FakeAction,
    )

    response = HARNESS.compact_via_flight(
        "127.0.0.1", 5690, "7", flight
    )

    assert response["success"] is True
    assert calls["location"] == ("127.0.0.1", 5690)
    assert calls["action"].type == "compact_collection"
    assert json.loads(calls["action"].body) == {"collection_id": "7"}
    assert calls["closed"] is True


def test_explicit_flush_bed_relies_on_automatic_compaction_quiescence() -> None:
    assert HARNESS.post_flush_compaction_observation(None) is None
    observation = HARNESS.post_flush_compaction_observation(20_000)

    assert observation is not None
    assert observation["requested"] is False
    assert "materialization gate" in observation["reason"]


def test_explicit_flush_waits_for_stable_new_pax_epoch() -> None:
    before = {"segments": [], "segment_count": 0, "bytes": 0}
    after = {
        "segments": [{"path": "7/data/L0.pax", "bytes": 4096, "etag": "e1"}],
        "segment_count": 1,
        "bytes": 4096,
    }

    class FakeGeometry:
        def __init__(self):
            self.calls = 0

        def inventory(self) -> dict:
            self.calls += 1
            return after

        @staticmethod
        def stable_signature(inventory: dict) -> tuple:
            return tuple(
                (item["path"], item["bytes"], item["etag"])
                for item in inventory["segments"]
            )

    with (
        patch.object(HARNESS, "scrape_text", return_value=""),
        patch.object(HARNESS.time, "sleep") as sleep,
    ):
        geometry = FakeGeometry()
        elapsed, observed, wal_bytes = HARNESS.wait_for_pax_epoch(
            "http://server", "7", geometry, before
        )

    assert elapsed >= 0
    assert observed == after
    assert wal_bytes is None
    assert geometry.calls == 2
    sleep.assert_called_once_with(0.5)
