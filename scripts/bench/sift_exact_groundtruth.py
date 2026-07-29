#!/usr/bin/env python3
"""Generate exact top-k ivecs for a fixed SIFT corpus prefix.

Distance evaluation is blocked over queries and base rows. Peak temporary
memory is therefore O(query_batch * base_block), not O(queries * corpus).
The output is suitable for sift1m_get_reduction.py when accompanied by
--groundtruth-scope-rows equal to --rows.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import struct

import numpy as np


def fixed_fvecs(path: Path) -> tuple[np.memmap, int, int]:
    with path.open("rb") as source:
        encoded = source.read(4)
    if len(encoded) != 4:
        raise RuntimeError(f"{path}: missing fvec dimension")
    dimension = struct.unpack("<i", encoded)[0]
    if dimension <= 0:
        raise RuntimeError(f"{path}: invalid dimension {dimension}")
    record_scalars = dimension + 1
    if path.stat().st_size % (record_scalars * 4):
        raise RuntimeError(f"{path}: partial fvec record")
    count = path.stat().st_size // (record_scalars * 4)
    raw = np.memmap(
        path,
        mode="r",
        dtype="<f4",
        shape=(count, record_scalars),
    )
    headers = raw.view("<i4")[:, 0]
    if not np.all(headers == dimension):
        raise RuntimeError(f"{path}: variable fvec dimensions")
    return raw, count, dimension


def exact_neighbors(
    base: np.ndarray,
    queries: np.ndarray,
    top_k: int,
    query_batch: int,
    base_block: int,
) -> np.ndarray:
    if top_k <= 0 or top_k > base.shape[0]:
        raise RuntimeError(f"top_k={top_k} is invalid for {base.shape[0]} rows")
    output = np.empty((queries.shape[0], top_k), dtype="<i4")
    base_norms = np.einsum("ij,ij->i", base, base)
    for query_start in range(0, queries.shape[0], query_batch):
        query_end = min(query_start + query_batch, queries.shape[0])
        query = np.asarray(queries[query_start:query_end], dtype="<f4")
        query_norms = np.einsum("ij,ij->i", query, query)[:, None]
        best_distances = np.full((len(query), top_k), np.inf, dtype="<f4")
        best_ids = np.full((len(query), top_k), -1, dtype="<i4")
        for base_start in range(0, base.shape[0], base_block):
            base_end = min(base_start + base_block, base.shape[0])
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
            combined_distances = np.concatenate(
                (best_distances, distances), axis=1
            )
            combined_ids = np.concatenate((best_ids, ids), axis=1)
            chosen = np.argpartition(
                combined_distances, top_k - 1, axis=1
            )[:, :top_k]
            best_distances = np.take_along_axis(
                combined_distances, chosen, axis=1
            )
            best_ids = np.take_along_axis(combined_ids, chosen, axis=1)
        order = np.argsort(best_distances, axis=1, kind="stable")
        output[query_start:query_end] = np.take_along_axis(
            best_ids, order, axis=1
        )
        print(
            f"ground truth: {query_end:,}/{queries.shape[0]:,} queries",
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
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--queries", type=int, default=10_000)
    parser.add_argument("--top-k", type=int, default=100)
    parser.add_argument("--query-batch", type=int, default=32)
    parser.add_argument("--base-block", type=int, default=100_000)
    args = parser.parse_args()

    if args.output.exists():
        raise RuntimeError(f"refusing to overwrite {args.output}")
    base_raw, base_count, dimension = fixed_fvecs(args.base)
    query_raw, query_count, query_dimension = fixed_fvecs(args.queries_path)
    if dimension != query_dimension:
        raise RuntimeError("base/query dimensions differ")
    if not 0 < args.rows <= base_count:
        raise RuntimeError(f"requested {args.rows} of {base_count} base rows")
    if not 0 < args.queries <= query_count:
        raise RuntimeError(
            f"requested {args.queries} of {query_count} query rows"
        )
    if args.query_batch <= 0 or args.base_block <= 0:
        raise RuntimeError("query/base block sizes must be positive")

    neighbors = exact_neighbors(
        base_raw[:args.rows, 1:],
        query_raw[:args.queries, 1:],
        args.top_k,
        args.query_batch,
        args.base_block,
    )
    encoded = np.empty(
        (neighbors.shape[0], neighbors.shape[1] + 1), dtype="<i4"
    )
    encoded[:, 0] = neighbors.shape[1]
    encoded[:, 1:] = neighbors
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("xb") as destination:
        encoded.tofile(destination)
    manifest = {
        "protocol": "sift_exact_groundtruth_v1",
        "base": str(args.base.resolve()),
        "queries_path": str(args.queries_path.resolve()),
        "corpus_rows": args.rows,
        "query_rows": args.queries,
        "dimension": dimension,
        "top_k": args.top_k,
        "output": str(args.output.resolve()),
        "output_sha256": sha256(args.output),
    }
    manifest_path = args.output.with_suffix(args.output.suffix + ".json")
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"manifest: {manifest_path}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
