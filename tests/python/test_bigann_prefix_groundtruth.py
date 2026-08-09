"""Contracts for exact BIGANN prefix ground-truth derivation."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import struct
import sys

import numpy as np

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPOSITORY_ROOT / "scripts" / "bench" / "bigann_prefix_groundtruth.py"
SPEC = importlib.util.spec_from_file_location("bigann_prefix_groundtruth", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
GROUNDTRUTH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GROUNDTRUTH)


def test_exact_l2_accumulator_is_fail_closed_by_dimension() -> None:
    assert GROUNDTRUTH.exact_l2_compute_dtype(128) is np.float32
    assert GROUNDTRUTH.exact_l2_compute_dtype(129) is np.float32
    assert GROUNDTRUTH.exact_l2_compute_dtype(130) is np.float64
    with np.testing.assert_raises_regex(RuntimeError, "positive"):
        GROUNDTRUTH.exact_l2_compute_dtype(0)


def test_superset_filter_only_falls_back_for_uncovered_queries() -> None:
    superset_ids = np.asarray(
        [
            [0, 1, 3],
            [4, 2, 3],
        ],
        dtype="<i4",
    )

    neighbors, uncovered = GROUNDTRUTH.derive_prefix_neighbors(
        superset_ids,
        prefix_rows=3,
        top_k=2,
    )

    assert neighbors.tolist() == [[0, 1], [2, -1]]
    assert uncovered == [1]


def test_exact_fallback_uses_prefix_and_deterministic_id_ties() -> None:
    base = np.asarray(
        [
            [0, 0],
            [1, 0],
            [10, 0],
            [20, 0],
            [30, 0],
        ],
        dtype=np.uint8,
    )
    queries = np.asarray([[0, 0], [9, 0]], dtype=np.uint8)

    neighbors = GROUNDTRUTH.exact_neighbors_for_queries(
        base,
        queries,
        query_indices=[1],
        prefix_rows=3,
        top_k=2,
        query_batch=1,
        base_block=2,
    )
    tied_distances = np.asarray([[1.0, 1.0]], dtype="<f4")
    tied_ids = np.asarray([[7, 3]], dtype="<i4")
    _, tied_neighbors = GROUNDTRUTH.lexicographic_top_k(
        tied_distances,
        tied_ids,
        top_k=1,
    )

    assert neighbors.tolist() == [[2, 1]]
    assert tied_neighbors.tolist() == [[3]]


def test_main_exact_only_mode_records_complete_provenance(
    tmp_path: Path, monkeypatch
) -> None:
    """Omitting the superset must brute-force every query and say so in evidence."""
    base_path = tmp_path / "base.u8bin"
    queries_path = tmp_path / "queries.u8bin"
    output_path = tmp_path / "truth.ivecs"

    def write_u8bin(path: Path, rows: np.ndarray) -> None:
        with path.open("wb") as destination:
            destination.write(struct.pack("<II", rows.shape[0], rows.shape[1]))
            rows.tofile(destination)

    write_u8bin(
        base_path,
        np.asarray([[0, 0], [1, 0], [10, 0], [20, 0]], dtype=np.uint8),
    )
    write_u8bin(queries_path, np.asarray([[0, 0], [9, 0]], dtype=np.uint8))
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bigann_prefix_groundtruth.py",
            "--base",
            str(base_path),
            "--queries-path",
            str(queries_path),
            "--output",
            str(output_path),
            "--prefix-rows",
            "3",
            "--queries",
            "2",
            "--top-k",
            "2",
            "--query-batch",
            "1",
            "--base-block",
            "2",
        ],
    )

    assert GROUNDTRUTH.main() == 0
    encoded = np.fromfile(output_path, dtype="<i4").reshape(2, 3)
    manifest = json.loads(output_path.with_suffix(".ivecs.json").read_text())
    assert encoded.tolist() == [[2, 0, 1], [2, 2, 1]]
    assert manifest["mode"] == "exact_full"
    assert manifest["superset_groundtruth"] is None
    assert manifest["derived_from_superset_rows"] == 0
    assert manifest["exact_fallback_rows"] == 2
    assert manifest["exact_fallback_query_indices"] == [0, 1]
