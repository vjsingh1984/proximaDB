#!/usr/bin/env python3
"""Run the ProximaDB adapter for context-corridor-v1.

This runner deliberately emits a local baseline, not a competitor claim. The
scenario contract forbids cross-system publication until every adapter is
runnable under the same dataset and measurement protocol.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import random
import statistics
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


def request(base: str, method: str, path: str, body: dict | None, timeout: float) -> tuple[Any, float]:
    headers = {"Accept": "application/json"}
    payload = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        payload = json.dumps(body, separators=(",", ":")).encode()
    req = urllib.request.Request(base.rstrip("/") + path, data=payload, headers=headers, method=method)
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            status = response.status
            raw = response.read()
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"{method} {path}: HTTP {exc.code}: {exc.read()[:1000]!r}") from exc
    latency_ms = (time.perf_counter() - started) * 1000.0
    if status not in (200, 201, 202, 204):
        raise RuntimeError(f"{method} {path}: unexpected HTTP {status}")
    return (json.loads(raw) if raw.strip() else None), latency_ms


def percentile(values: list[float], p: int) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = min(len(ordered) - 1, math.ceil((p / 100.0) * len(ordered)) - 1)
    return ordered[index]


def ids(payload: Any) -> list[str]:
    if not isinstance(payload, dict):
        return []
    values = payload.get("matches") or payload.get("results") or payload.get("records") or []
    found: list[str] = []
    for value in values if isinstance(values, list) else []:
        if not isinstance(value, dict):
            continue
        record_id = value.get("id") or value.get("record_id")
        if isinstance(record_id, str):
            found.append(record_id)
        nested = value.get("record")
        if isinstance(nested, dict) and isinstance(nested.get("id"), str):
            found.append(nested["id"])
    return found


def git_sha(root: Path) -> str:
    try:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:5678")
    parser.add_argument("--records", type=int, default=1000)
    parser.add_argument("--dimension", type=int, default=32)
    parser.add_argument("--queries", type=int, default=200)
    parser.add_argument("--warmup", type=int, default=20)
    parser.add_argument("--batch-size", type=int, default=100)
    parser.add_argument("--seed", type=int, default=20260803)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if min(args.records, args.dimension, args.queries, args.batch_size) <= 0:
        parser.error("records, dimension, queries, and batch-size must be positive")

    root = Path(__file__).resolve().parents[2]
    rng = random.Random(args.seed)
    collection = f"context_bench_{int(time.time())}_{rng.randrange(100000):05d}"
    records: list[dict] = []
    for index in range(args.records):
        vector = [rng.uniform(-1.0, 1.0) for _ in range(args.dimension)]
        records.append(
            {
                "id": f"rec-{index}",
                "vector": vector,
                "props": {"partition": f"p{index % 8}", "ordinal": index},
            }
        )
    dataset_bytes = json.dumps(records, sort_keys=True, separators=(",", ":")).encode()
    dataset_hash = hashlib.sha256(dataset_bytes).hexdigest()

    request(
        args.base_url,
        "POST",
        "/api/v2/collections",
        {
            "name": collection,
            "dimension": args.dimension,
            "engine": "sst",
            "distance_metric": "cosine",
            "enable_proxima_record": True,
            "schema": {
                "columns": [
                    {"name": "partition", "data_type": "text", "indexed": True, "filterable": True},
                    {"name": "ordinal", "data_type": "integer", "indexed": False, "filterable": True},
                ],
                "enforcement": "hybrid",
                "allow_additional_fields": True,
            },
        },
        args.timeout,
    )
    ingest_started = time.perf_counter()
    for offset in range(0, len(records), args.batch_size):
        request(
            args.base_url,
            "POST",
            f"/api/v2/collections/{collection}/records/batch",
            {"records": records[offset : offset + args.batch_size], "upsert": True},
            args.timeout,
        )
    ingest_seconds = time.perf_counter() - ingest_started
    time.sleep(0.75)

    query_indexes = [rng.randrange(args.records) for _ in range(args.warmup + args.queries)]
    latencies: list[float] = []
    correct = 0
    for ordinal, record_index in enumerate(query_indexes):
        payload, latency = request(
            args.base_url,
            "POST",
            f"/api/v2/collections/{collection}/search",
            {
                "vector": records[record_index]["vector"],
                "top_k": 10,
                "filters": [
                    {
                        "field": "partition",
                        "op": "eq",
                        "value": f"p{record_index % 8}",
                    }
                ],
            },
            args.timeout,
        )
        if ordinal >= args.warmup:
            latencies.append(latency)
            correct += int(f"rec-{record_index}" in ids(payload)[:10])

    route_health, _ = request(
        args.base_url,
        "GET",
        f"/api/v2/_diagnostics/collections/{collection}/route-health",
        None,
        args.timeout,
    )
    report = {
        "schema_version": 1,
        "scenario_id": "context-corridor-v1",
        "system": "proximadb",
        "status": "measured-local-baseline",
        "commit_sha": git_sha(root),
        "dataset": {
            "records": args.records,
            "dimension": args.dimension,
            "seed": args.seed,
            "sha256": dataset_hash,
        },
        "environment": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "base_url": args.base_url,
        },
        "metrics": {
            "ingest_records_per_second": round(args.records / ingest_seconds, 3),
            "filtered_recall_at_10": round(correct / args.queries, 6),
            "latency_p50_ms": round(statistics.median(latencies), 3),
            "latency_p95_ms": round(percentile(latencies, 95), 3),
            "latency_p99_ms": round(percentile(latencies, 99), 3),
        },
        "query_count": args.queries,
        "warmup_count": args.warmup,
        "server_signals": route_health,
        "publication_eligible": False,
        "publication_blocker": "competitor adapters, five trials, >=1M records, and four backends are required",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
