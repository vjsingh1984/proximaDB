#!/usr/bin/env python3
"""Validate the reproducible context-corridor competitor benchmark contract."""

from __future__ import annotations

import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
SCENARIO = ROOT / "benches/context-corridor/scenario.toml"
REQUIRED_SYSTEMS = {"proximadb", "pgvector", "qdrant", "milvus", "elasticsearch", "surrealdb"}
REQUIRED_METRICS = {
    "recall_at_10",
    "filtered_recall_at_10",
    "ingest_records_per_second",
    "latency_p50_ms",
    "latency_p95_ms",
    "latency_p99_ms",
    "peak_rss_bytes",
    "object_gets",
    "object_puts",
    "bytes_read",
    "bytes_written",
    "cache_hit_ratio",
    "estimated_cost_per_million_queries",
}


def main() -> int:
    try:
        data = tomllib.loads(SCENARIO.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        print(f"ERROR: context benchmark scenario is invalid: {exc}", file=sys.stderr)
        return 1
    errors: list[str] = []
    if data.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    runner = data.get("runner")
    if not isinstance(runner, str) or not (ROOT / runner).is_file():
        errors.append(f"runner does not exist: {runner!r}")
    systems = {item.get("id") for item in data.get("system", [])}
    if systems != REQUIRED_SYSTEMS:
        errors.append(f"systems must be {sorted(REQUIRED_SYSTEMS)}, found {sorted(systems)}")
    runnable = {
        item.get("id") for item in data.get("system", []) if item.get("adapter_status") == "runnable"
    }
    if runnable != {"proximadb"}:
        errors.append("only the implemented ProximaDB adapter may be marked runnable")
    protocol = data.get("protocol", {})
    metrics = set(protocol.get("required_metrics", []))
    missing_metrics = REQUIRED_METRICS - metrics
    if missing_metrics:
        errors.append(f"required metrics missing: {sorted(missing_metrics)}")
    dataset = data.get("dataset", {})
    scales = dataset.get("record_scales", [])
    minimum = dataset.get("publish_minimum_records")
    if not scales or minimum not in scales or minimum < 1_000_000:
        errors.append("publication scale must be a declared record scale of at least 1M")
    if protocol.get("trials", 0) < 5:
        errors.append("publication requires at least five trials")
    if set(protocol.get("required_backends", [])) != {"local", "s3", "azure", "gcs"}:
        errors.append("local, S3, Azure, and GCS backends are required")
    publication = data.get("publication", {})
    for gate in (
        "require_raw_results",
        "require_commit_sha",
        "require_server_config",
        "require_hardware_manifest",
        "require_dataset_hash",
        "require_failed_runs",
        "forbid_cross_system_claims_until_all_adapters_runnable",
    ):
        if publication.get(gate) is not True:
            errors.append(f"publication.{gate} must be true")
    if errors:
        print("ERROR: context benchmark contract drift:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print(
        "Context benchmark contract OK: 6 systems, 13 metrics, 4 backends, "
        "five-trial/1M publication floor; only ProximaDB adapter is runnable."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
