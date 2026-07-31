#!/usr/bin/env python3
"""Build exact ground truth for a BIGANN corpus prefix.

BIGANN publishes exact top-100 neighbors for selected corpus sizes. For a
smaller prefix, filtering a superset's ordered top-100 IDs is exact whenever
at least ``top_k`` IDs remain: every omitted vector is below the superset's
top-100 boundary. Only uncovered queries require brute-force distance work.

The output uses classic ``ivecs`` so existing recall tooling can consume it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path

import numpy as np


def inspect_u8bin(path: Path) -> tuple[np.memmap, int, int, int]:
    with path.open("rb") as source:
        header = source.read(8)
    if len(header) != 8:
        raise RuntimeError(f"{path}: missing u8bin header")
    declared_rows, dimension = struct.unpack("<II", header)
    if declared_rows <= 0 or dimension <= 0:
        raise RuntimeError(
            f"{path}: invalid u8bin shape ({declared_rows}, {dimension})"
        )
    payload_bytes = path.stat().st_size - 8
    if payload_bytes < 0 or payload_bytes % dimension:
        raise RuntimeError(f"{path}: partial dense u8bin row")
    physical_rows = payload_bytes // dimension
    if physical_rows <= 0 or physical_rows > declared_rows:
        raise RuntimeError(
            f"{path}: physical rows {physical_rows} are outside declared "
            f"range 1..{declared_rows}"
        )
    vectors = np.memmap(
        path,
        mode="r",
        dtype=np.uint8,
        offset=8,
        shape=(physical_rows, dimension),
    )
    return vectors, physical_rows, dimension, declared_rows


def read_bigann_groundtruth_ids(path: Path) -> np.memmap:
    with path.open("rb") as source:
        header = source.read(8)
    if len(header) != 8:
        raise RuntimeError(f"{path}: missing BIGANN ground-truth header")
    rows, width = struct.unpack("<II", header)
    if rows <= 0 or width <= 0:
        raise RuntimeError(f"{path}: invalid ground-truth shape ({rows}, {width})")
    expected_bytes = 8 + rows * width * 8
    if path.stat().st_size != expected_bytes:
        raise RuntimeError(
            f"{path}: expected {expected_bytes} bytes for IDs + distances"
        )
    return np.memmap(
        path,
        mode="r",
        dtype="<i4",
        offset=8,
        shape=(rows, width),
    )


def derive_prefix_neighbors(
    superset_ids: np.ndarray,
    prefix_rows: int,
    top_k: int,
) -> tuple[np.ndarray, list[int]]:
    if prefix_rows <= 0:
        raise RuntimeError("prefix_rows must be positive")
    if top_k <= 0 or top_k > superset_ids.shape[1]:
        raise RuntimeError(
            f"top_k={top_k} exceeds superset width {superset_ids.shape[1]}"
        )
    output = np.full((superset_ids.shape[0], top_k), -1, dtype="<i4")
    uncovered = []
    for query_index, row in enumerate(superset_ids):
        prefix_ids = row[(row >= 0) & (row < prefix_rows)]
        available = min(len(prefix_ids), top_k)
        output[query_index, :available] = prefix_ids[:available]
        if available < top_k:
            uncovered.append(query_index)
    return output, uncovered


def lexicographic_top_k(
    distances: np.ndarray,
    ids: np.ndarray,
    top_k: int,
) -> tuple[np.ndarray, np.ndarray]:
    selected_distances = np.empty((distances.shape[0], top_k), dtype="<f4")
    selected_ids = np.empty((distances.shape[0], top_k), dtype="<i4")
    for row_index in range(distances.shape[0]):
        row_distances = distances[row_index]
        row_ids = ids[row_index]
        boundary = np.partition(row_distances, top_k - 1)[top_k - 1]
        strict = np.flatnonzero(row_distances < boundary)
        equal = np.flatnonzero(row_distances == boundary)
        remaining = top_k - len(strict)
        equal_order = np.argsort(row_ids[equal], kind="stable")[:remaining]
        chosen = np.concatenate((strict, equal[equal_order]))
        order = np.lexsort((row_ids[chosen], row_distances[chosen]))
        chosen = chosen[order]
        selected_distances[row_index] = row_distances[chosen]
        selected_ids[row_index] = row_ids[chosen]
    return selected_distances, selected_ids


def exact_neighbors_for_queries(
    base: np.ndarray,
    queries: np.ndarray,
    query_indices: list[int],
    prefix_rows: int,
    top_k: int,
    query_batch: int,
    base_block: int,
) -> np.ndarray:
    if not query_indices:
        return np.empty((0, top_k), dtype="<i4")
    if not 0 < prefix_rows <= base.shape[0]:
        raise RuntimeError(
            f"prefix_rows={prefix_rows} exceeds {base.shape[0]} base rows"
        )
    if top_k <= 0 or top_k > prefix_rows:
        raise RuntimeError(f"top_k={top_k} is invalid for the prefix")
    if query_batch <= 0 or base_block <= 0:
        raise RuntimeError("query/base block sizes must be positive")
    selected_queries = np.asarray(queries[query_indices], dtype="<f4")
    base_norms = np.empty(prefix_rows, dtype="<f4")
    for base_start in range(0, prefix_rows, base_block):
        base_end = min(base_start + base_block, prefix_rows)
        block = np.asarray(base[base_start:base_end], dtype="<f4")
        base_norms[base_start:base_end] = np.einsum("ij,ij->i", block, block)

    output = np.empty((len(query_indices), top_k), dtype="<i4")
    for query_start in range(0, len(query_indices), query_batch):
        query_end = min(query_start + query_batch, len(query_indices))
        query = selected_queries[query_start:query_end]
        query_norms = np.einsum("ij,ij->i", query, query)[:, None]
        best_distances = np.full((len(query), top_k), np.inf, dtype="<f4")
        best_ids = np.full((len(query), top_k), -1, dtype="<i4")
        for base_start in range(0, prefix_rows, base_block):
            base_end = min(base_start + base_block, prefix_rows)
            block = np.asarray(base[base_start:base_end], dtype="<f4")
            distances = (
                query_norms
                + base_norms[base_start:base_end][None, :]
                - 2.0 * (query @ block.T)
            )
            np.maximum(distances, 0.0, out=distances)
            ids = np.broadcast_to(
                np.arange(base_start, base_end, dtype="<i4"),
                distances.shape,
            )
            combined_distances = np.concatenate((best_distances, distances), axis=1)
            combined_ids = np.concatenate((best_ids, ids), axis=1)
            best_distances, best_ids = lexicographic_top_k(
                combined_distances, combined_ids, top_k
            )
        output[query_start:query_end] = best_ids
        print(
            f"exact fallback: {query_end:,}/{len(query_indices):,} queries",
            flush=True,
        )
    return output


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--queries-path", type=Path, required=True)
    parser.add_argument("--superset-groundtruth", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--prefix-rows", type=int, required=True)
    parser.add_argument("--queries", type=int, default=10_000)
    parser.add_argument("--top-k", type=int, default=20)
    parser.add_argument("--query-batch", type=int, default=8)
    parser.add_argument("--base-block", type=int, default=100_000)
    args = parser.parse_args()

    if args.output.exists():
        raise RuntimeError(f"refusing to overwrite {args.output}")
    base, base_rows, dimension, base_declared_rows = inspect_u8bin(args.base)
    queries, query_rows, query_dimension, query_declared_rows = inspect_u8bin(
        args.queries_path
    )
    if dimension != query_dimension:
        raise RuntimeError("base/query dimensions differ")
    superset_ids = read_bigann_groundtruth_ids(args.superset_groundtruth)
    if not 0 < args.queries <= min(query_rows, superset_ids.shape[0]):
        raise RuntimeError(
            f"requested {args.queries} queries but only "
            f"{min(query_rows, superset_ids.shape[0])} are available"
        )
    if not 0 < args.prefix_rows <= base_rows:
        raise RuntimeError(f"requested prefix {args.prefix_rows} of {base_rows} rows")

    neighbors, uncovered = derive_prefix_neighbors(
        superset_ids[: args.queries],
        args.prefix_rows,
        args.top_k,
    )
    if uncovered:
        neighbors[uncovered] = exact_neighbors_for_queries(
            base,
            queries,
            uncovered,
            args.prefix_rows,
            args.top_k,
            args.query_batch,
            args.base_block,
        )
    if np.any(neighbors < 0) or np.any(neighbors >= args.prefix_rows):
        raise RuntimeError("generated ground truth contains out-of-scope IDs")

    encoded = np.empty((neighbors.shape[0], neighbors.shape[1] + 1), dtype="<i4")
    encoded[:, 0] = neighbors.shape[1]
    encoded[:, 1:] = neighbors
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("xb") as destination:
        encoded.tofile(destination)
    manifest = {
        "protocol": "bigann_prefix_groundtruth_v1",
        "base": str(args.base.resolve()),
        "base_sha256": sha256(args.base),
        "base_physical_rows": base_rows,
        "base_declared_rows": base_declared_rows,
        "queries_path": str(args.queries_path.resolve()),
        "queries_sha256": sha256(args.queries_path),
        "query_rows": args.queries,
        "query_declared_rows": query_declared_rows,
        "superset_groundtruth": str(args.superset_groundtruth.resolve()),
        "superset_groundtruth_sha256": sha256(args.superset_groundtruth),
        "prefix_rows": args.prefix_rows,
        "dimension": dimension,
        "top_k": args.top_k,
        "derived_from_superset_rows": args.queries - len(uncovered),
        "exact_fallback_rows": len(uncovered),
        "exact_fallback_query_indices": uncovered,
        "tie_break": "distance_ascending_then_vector_id_ascending",
        "output": str(args.output.resolve()),
        "output_format": "ivecs",
        "output_sha256": sha256(args.output),
    }
    manifest_path = args.output.with_suffix(args.output.suffix + ".json")
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"manifest: {manifest_path}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
