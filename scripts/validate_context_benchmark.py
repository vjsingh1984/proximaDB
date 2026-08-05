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
    if runnable != REQUIRED_SYSTEMS:
        errors.append(
            "all six adapters (ProximaDB, pgvector, Qdrant, Milvus, "
            "Elasticsearch, SurrealDB) must be marked runnable"
        )
    qdrant = next(
        (item for item in data.get("system", []) if item.get("id") == "qdrant"),
        {},
    )
    qdrant_adapter = qdrant.get("adapter", {})
    expected_qdrant_adapter = {
        "runner_system": "qdrant",
        "distance": "cosine",
        "vector_index": "hnsw(m=16,ef_construct=100)",
        "metadata_index": "keyword(partition)",
        "hnsw_ef_search": 40,
        "write_visibility": "wait=true per batch",
        "readiness_fence": "green status and empty update queue",
        "query_endpoint": "/points/query",
    }
    if qdrant_adapter != expected_qdrant_adapter:
        errors.append(
            "Qdrant adapter contract must disclose the pinned filter, ANN, "
            "visibility, and readiness settings"
        )
    milvus = next(
        (item for item in data.get("system", []) if item.get("id") == "milvus"),
        {},
    )
    expected_milvus_adapter = {
        "runner_system": "milvus",
        "distance": "cosine",
        "vector_index": "hnsw(M=16,efConstruction=100)",
        "metadata_index": "bitmap(partition)",
        "hnsw_ef_search": 40,
        "consistency_level": "Strong",
        "readiness_fence": "flush + loaded + vector/scalar indexes finished",
        "query_endpoint": "/v2/vectordb/entities/search",
    }
    if milvus.get("adapter", {}) != expected_milvus_adapter:
        errors.append(
            "Milvus adapter contract must disclose the pinned scalar/vector "
            "indexes, consistency, and readiness settings"
        )
    elasticsearch = next(
        (item for item in data.get("system", []) if item.get("id") == "elasticsearch"),
        {},
    )
    expected_elasticsearch_adapter = {
        "runner_system": "elasticsearch",
        "distance": "cosine",
        "vector_index": "dense_vector hnsw(m=16,ef_construction=100)",
        "metadata_index": "keyword(partition)",
        "hnsw_num_candidates": 40,
        "write_visibility": "forced _refresh per ingest",
        "readiness_fence": "green status and matching document count",
        "query_endpoint": "/_search (knn)",
    }
    if elasticsearch.get("adapter", {}) != expected_elasticsearch_adapter:
        errors.append(
            "Elasticsearch adapter contract must disclose the pinned "
            "dense_vector ANN, keyword filter, visibility, and readiness settings"
        )
    surrealdb = next(
        (item for item in data.get("system", []) if item.get("id") == "surrealdb"),
        {},
    )
    expected_surrealdb_adapter = {
        "runner_system": "surrealdb",
        "distance": "cosine",
        "vector_index": "hnsw(M=16,EFC=100)",
        "metadata_index": "index(partition)",
        "hnsw_ef_search": 40,
        "readiness_fence": "synchronous index + full document count",
        "query_endpoint": "/sql (<|K,EF|> KNN)",
    }
    if surrealdb.get("adapter", {}) != expected_surrealdb_adapter:
        errors.append(
            "SurrealDB adapter contract must disclose the pinned HNSW ANN, "
            "scalar filter index, and readiness settings"
        )
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
        "five-trial/1M publication floor; all six adapters (ProximaDB, "
        "pgvector, Qdrant, Milvus, Elasticsearch, SurrealDB) are runnable."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
