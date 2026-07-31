"""Contracts for exact BIGANN prefix ground-truth derivation."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPOSITORY_ROOT / "scripts" / "bench" / "bigann_prefix_groundtruth.py"
SPEC = importlib.util.spec_from_file_location("bigann_prefix_groundtruth", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
GROUNDTRUTH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GROUNDTRUTH)


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
