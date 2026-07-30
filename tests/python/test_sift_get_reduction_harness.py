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

    HARNESS.write_config(config_path, tmp_path, 5690, 128, storage_url)

    assert f'url = "{storage_url}"' in config_path.read_text()
