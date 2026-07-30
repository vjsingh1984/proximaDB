"""Focused contracts for the auditable SIFT GET-reduction harness."""

from __future__ import annotations

import importlib.util
import json
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


def test_explicit_flush_waits_for_collection_wal_to_drain() -> None:
    samples = [
        'proximadb_wal_size_bytes{collection="7"} 4096\n',
        'proximadb_wal_size_bytes{collection="7"} 0\n',
    ]

    with (
        patch.object(HARNESS, "scrape_text", side_effect=samples),
        patch.object(HARNESS.time, "sleep") as sleep,
    ):
        elapsed = HARNESS.wait_for_wal_drain("http://server", "7")

    assert elapsed >= 0
    sleep.assert_called_once_with(0.25)
