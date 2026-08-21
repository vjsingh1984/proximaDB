#!/usr/bin/env python3
"""Fuse sealed source-level retrieval runs with deterministic weighted RRF."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import tempfile
from collections import defaultdict
from pathlib import Path
from typing import Any, Sequence


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _atomic_json(value: Any, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f"{path.name}.", suffix=".tmp", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with temporary.open("w", encoding="utf-8") as output:
            json.dump(value, output, indent=2, sort_keys=True)
            output.write("\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _atomic_jsonl(records: list[dict[str, Any]], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f"{path.name}.", suffix=".tmp", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with temporary.open("w", encoding="utf-8") as output:
            for record in records:
                output.write(json.dumps(record, sort_keys=True) + "\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _load_run(path: Path) -> dict[str, dict[str, int]]:
    rankings: dict[str, dict[str, int]] = defaultdict(dict)
    ranks_by_query: dict[str, set[int]] = defaultdict(set)
    with path.open("r", encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            if not line.strip():
                continue
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{path} line {line_number}: expected an object")
            query_id = value.get("query_id")
            source_id = value.get("source_id")
            rank = value.get("rank")
            if not isinstance(query_id, str) or not query_id.strip():
                raise ValueError(f"{path} line {line_number}: query_id is required")
            if not isinstance(source_id, str) or not source_id.strip():
                raise ValueError(f"{path} line {line_number}: source_id is required")
            if isinstance(rank, bool) or not isinstance(rank, int) or rank <= 0:
                raise ValueError(f"{path} line {line_number}: rank must be positive")
            if source_id in rankings[query_id]:
                raise ValueError(
                    f"{path} line {line_number}: duplicate source {source_id!r} "
                    f"for query {query_id!r}"
                )
            if rank in ranks_by_query[query_id]:
                raise ValueError(
                    f"{path} line {line_number}: duplicate rank {rank} "
                    f"for query {query_id!r}"
                )
            rankings[query_id][source_id] = rank
            ranks_by_query[query_id].add(rank)
    if not rankings:
        raise ValueError(f"{path} contains no run rows")
    for query_id, ranks in ranks_by_query.items():
        expected = set(range(1, len(ranks) + 1))
        if ranks != expected:
            raise ValueError(f"{path}: ranks for query {query_id!r} are not contiguous")
    return dict(rankings)


def fuse_source_runs(
    run_paths: Sequence[Path],
    output_path: Path,
    *,
    labels: Sequence[str],
    top_k: int,
    rrf_k: int = 60,
    weights: Sequence[float] | None = None,
    manifest_path: Path | None = None,
) -> dict[str, Any]:
    """Fuse runs by rank, requiring identical query coverage across every leg."""

    if len(run_paths) < 2:
        raise ValueError("at least two source runs are required")
    if len(labels) != len(run_paths) or any(not label.strip() for label in labels):
        raise ValueError("labels must contain one non-empty value per run")
    if len(set(labels)) != len(labels):
        raise ValueError("labels must be unique")
    if isinstance(top_k, bool) or not isinstance(top_k, int) or top_k <= 0:
        raise ValueError("top_k must be a positive integer")
    if isinstance(rrf_k, bool) or not isinstance(rrf_k, int) or rrf_k < 0:
        raise ValueError("rrf_k must be a non-negative integer")
    chosen_weights = tuple(weights or (1.0,) * len(run_paths))
    if len(chosen_weights) != len(run_paths) or any(
        not math.isfinite(weight) or weight <= 0 for weight in chosen_weights
    ):
        raise ValueError("weights must contain one finite positive value per run")

    runs = [_load_run(path) for path in run_paths]
    query_ids = set(runs[0])
    for label, run in zip(labels[1:], runs[1:], strict=True):
        if set(run) != query_ids:
            missing = sorted(query_ids - set(run))
            extra = sorted(set(run) - query_ids)
            raise ValueError(
                f"run {label!r} query coverage differs: missing={missing[:3]} "
                f"extra={extra[:3]}"
            )

    rows: list[dict[str, Any]] = []
    for query_id in sorted(query_ids):
        fused: dict[str, float] = defaultdict(float)
        for run, weight in zip(runs, chosen_weights, strict=True):
            for source_id, rank in run[query_id].items():
                fused[source_id] += weight / (rrf_k + rank)
        ranked = sorted(fused.items(), key=lambda item: (-item[1], item[0]))[:top_k]
        rows.extend(
            {
                "query_id": query_id,
                "rank": rank,
                "score": score,
                "source_id": source_id,
            }
            for rank, (source_id, score) in enumerate(ranked, 1)
        )

    _atomic_jsonl(rows, output_path)
    manifest_path = manifest_path or output_path.with_name(
        f"{output_path.stem}.fusion.manifest.json"
    )
    result = {
        "schema_version": 1,
        "producer_sha256": _sha256(Path(__file__).resolve()),
        "fusion": "reciprocal_rank_fusion",
        "parameters": {"rrf_k": rrf_k},
        "tie_break": "source_id ascending",
        "top_k": top_k,
        "query_count": len(query_ids),
        "inputs": [
            {
                "label": label,
                "path": str(path.resolve()),
                "sha256": _sha256(path),
                "weight": weight,
            }
            for label, path, weight in zip(
                labels, run_paths, chosen_weights, strict=True
            )
        ],
        "run_path": str(output_path.resolve()),
        "run_sha256": _sha256(output_path),
        "run_row_count": len(rows),
    }
    _atomic_json(result, manifest_path)
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, action="append", required=True)
    parser.add_argument("--label", action="append", required=True)
    parser.add_argument("--weight", type=float, action="append")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--top-k", type=int, default=100)
    parser.add_argument("--rrf-k", type=int, default=60)
    parser.add_argument("--manifest", type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    result = fuse_source_runs(
        args.run,
        args.output,
        labels=args.label,
        top_k=args.top_k,
        rrf_k=args.rrf_k,
        weights=args.weight,
        manifest_path=args.manifest,
    )
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
