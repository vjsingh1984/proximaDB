"""Baseline embedded Python benchmarks for ProximaDB modalities.

This benchmark intentionally uses the public embedded Python API only. It
measures the in-process PyO3 surface for vector, relational, document, graph
entity, and observability flows without starting REST, gRPC, Arrow Flight, or
PostgreSQL wire services.
"""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any, Callable

import numpy as np

from proximadb_embedded import GraphEdge, GraphNode, ProximaDB, ProximaRecord
from proximadb_embedded import insert_arrow as embedded_insert_arrow
from proximadb_embedded import insert_proxima_records, insert_records_profiled
from proximadb_embedded import (
    normalize_document,
    normalize_graph_node,
    normalize_observability_event,
)


def timed(name: str, op_count: int, func: Callable[[], Any]) -> dict[str, Any]:
    started = time.perf_counter()
    result = func()
    elapsed = time.perf_counter() - started
    return {
        "name": name,
        "operations": op_count,
        "seconds": elapsed,
        "ops_per_second": op_count / elapsed if elapsed > 0 else None,
        "result": result,
    }


def latency_sample(
    name: str,
    op_count: int,
    func: Callable[[int], Any],
) -> dict[str, Any]:
    samples_ms: list[float] = []
    last_result: Any = None
    for i in range(op_count):
        started = time.perf_counter()
        last_result = func(i)
        samples_ms.append((time.perf_counter() - started) * 1000.0)

    samples_sorted = sorted(samples_ms)
    p95_idx = min(len(samples_sorted) - 1, int(len(samples_sorted) * 0.95))
    p99_idx = min(len(samples_sorted) - 1, int(len(samples_sorted) * 0.99))
    total_seconds = sum(samples_ms) / 1000.0
    return {
        "name": name,
        "operations": op_count,
        "seconds": total_seconds,
        "ops_per_second": op_count / total_seconds if total_seconds > 0 else None,
        "mean_ms": statistics.fmean(samples_ms),
        "p95_ms": samples_sorted[p95_idx],
        "p99_ms": samples_sorted[p99_idx],
        "result": last_result,
    }


def profiled_insert_metric(
    name: str,
    op_count: int,
    func: Callable[[], tuple[int, dict[str, Any]]],
) -> dict[str, Any]:
    started = time.perf_counter()
    result, profile = func()
    elapsed = time.perf_counter() - started
    return {
        "name": name,
        "operations": op_count,
        "seconds": elapsed,
        "ops_per_second": op_count / elapsed if elapsed > 0 else None,
        "result": result,
        "profile": profile,
    }


def failed_metric(name: str, exc: Exception) -> dict[str, Any]:
    return {
        "name": f"{name}.error",
        "operations": 0,
        "seconds": 0.0,
        "ops_per_second": None,
        "result": f"{type(exc).__name__}: {exc}",
    }


def run_suite(name: str, func: Callable[[], list[dict[str, Any]]]) -> list[dict[str, Any]]:
    try:
        return func()
    except Exception as exc:
        return [failed_metric(name, exc)]


def run_vector(db: ProximaDB, scale: int, dimension: int) -> list[dict[str, Any]]:
    rng = np.random.default_rng(42)
    count = scale * 5
    vectors = rng.random((count, dimension), dtype=np.float32)
    ids = [f"vec_{i}" for i in range(count)]
    metadata = [{"tenant": "embedded", "kind": "vector"} for _ in ids]

    db.create_collection("bench_vectors", dimension=dimension, engine="sst")
    insert = timed(
        "vector.insert_numpy",
        count,
        lambda: db.insert("bench_vectors", ids=ids, vectors=vectors, metadata=metadata),
    )
    search = latency_sample(
        "vector.search_top10",
        min(50, scale),
        lambda i: len(db.search("bench_vectors", query=vectors[i], top_k=10)),
    )
    return [insert, search]


def run_arrow_batch(db: ProximaDB, scale: int, dimension: int) -> list[dict[str, Any]]:
    try:
        import pyarrow as pa
    except ImportError:
        return [
            {
                "name": "arrow_embedded.insert_arrow",
                "operations": 0,
                "seconds": 0.0,
                "ops_per_second": None,
                "result": "skipped: pyarrow is not installed",
            }
        ]

    count = scale * 5
    db.create_collection("bench_arrow_vectors", dimension=dimension, engine="sst")
    vectors = [
        [float((row + col) % 100) / 100.0 for col in range(dimension)]
        for row in range(count)
    ]
    table = pa.table(
        {
            "id": [f"arrow_vec_{i}" for i in range(count)],
            "vector": pa.array(vectors, type=pa.list_(pa.float32())),
            "tenant_id": ["embedded"] * count,
            "kind": ["arrow"] * count,
        }
    )

    insert = timed(
        "arrow_embedded.insert_arrow",
        count,
        lambda: embedded_insert_arrow(db, "bench_arrow_vectors", table),
    )
    get = latency_sample(
        "arrow_embedded.get_backing_vector",
        min(50, scale),
        lambda i: db.get_vector("bench_arrow_vectors", f"arrow_vec_{i}") is not None,
    )
    return [insert, get]


def run_relational(db: ProximaDB, scale: int, dimension: int) -> list[dict[str, Any]]:
    db.execute_sql(
        f"""
        CREATE TABLE IF NOT EXISTS bench_accounts (
            account_id TEXT NOT NULL,
            payload JSONB NOT NULL DEFAULT '{{}}'::jsonb,
            embedding VECTOR({dimension}),
            PRIMARY KEY (account_id)
        ) WITH (
            storage_engine = 'SST',
            layout = 'hybrid',
            schema_kind = 'relational_entity'
        );
        """
    )
    db.execute_sql(
        f"""
        CREATE TABLE IF NOT EXISTS bench_accounts_batch (
            account_id TEXT NOT NULL,
            payload JSONB NOT NULL DEFAULT '{{}}'::jsonb,
            embedding VECTOR({dimension}),
            PRIMARY KEY (account_id)
        ) WITH (
            storage_engine = 'SST',
            layout = 'hybrid',
            schema_kind = 'relational_entity'
        );
        """
    )
    vector_literal = "[" + ", ".join("0.1" for _ in range(dimension)) + "]"

    def insert_rows() -> int:
        for i in range(scale):
            db.execute_sql(
                f"""
                INSERT INTO bench_accounts (account_id, payload, embedding)
                VALUES (
                    'acct-{i}',
                    '{{"tier":"gold","seq":{i}}}'::jsonb,
                    '{vector_literal}'
                );
                """
        )
        return scale

    def insert_rows_batch() -> int:
        values = ",\n".join(
            f"('acct-batch-{i}', '{{\"tier\":\"gold\",\"seq\":{i}}}'::jsonb, "
            f"'{vector_literal}')"
            for i in range(scale)
        )
        db.execute_sql(
            f"""
            INSERT INTO bench_accounts_batch (account_id, payload, embedding)
            VALUES {values};
            """
        )
        return scale

    insert_single = timed("relational.sql_insert_single_row_loop", scale, insert_rows)
    insert_batch = timed("relational.sql_insert_multirow_batch", scale, insert_rows_batch)
    get = latency_sample(
        "relational.get_backing_vector",
        min(50, scale),
        lambda i: db.get_vector("bench_accounts_batch", f"acct-batch-{i}") is not None,
    )
    return [insert_single, insert_batch, get]


def run_documents(db: ProximaDB, scale: int) -> list[dict[str, Any]]:
    db.create_document_collection("bench_docs", indexed_paths=["$.kind", "$.tenant"])

    def insert_docs() -> int:
        for i in range(scale):
            db.insert_document(
                "bench_docs",
                {
                    "kind": "note" if i % 2 == 0 else "event",
                    "tenant": "embedded",
                    "score": i,
                    "payload": {"title": f"doc-{i}"},
                },
                doc_id=f"doc-{i}",
            )
        return scale

    insert = timed("document.insert", scale, insert_docs)
    query = latency_sample(
        "document.query_indexed_path",
        min(50, scale),
        lambda _i: len(db.query_documents("bench_docs", "$.kind = 'note'", limit=10)),
    )
    return [insert, query]


def run_graph_entity(db: ProximaDB, scale: int) -> list[dict[str, Any]]:
    graph_id = f"bench_entity_graph_{uuid.uuid4().hex}"
    db.create_graph(graph_id)
    nodes = [
        GraphNode(
            f"entity-{i}",
            labels=["Entity", "Account" if i % 2 == 0 else "Person"],
            properties={"tenant": "embedded", "seq": i},
        )
        for i in range(scale)
    ]
    edges = [
        GraphEdge(
            f"entity-{i}",
            f"entity-{i + 1}",
            "RELATED_TO",
            id=f"edge-{i}",
            weight=1.0,
            properties={"tenant": "embedded"},
        )
        for i in range(scale - 1)
    ]
    insert_nodes = timed(
        "graph_entity.create_nodes",
        len(nodes),
        lambda: db.create_nodes(graph_id, nodes),
    )
    insert_edges = timed(
        "graph_entity.create_edges",
        len(edges),
        lambda: db.create_edges(graph_id, edges),
    )
    traverse = latency_sample(
        "graph_entity.traverse_depth2",
        min(50, scale),
        lambda i: len(
            db.traverse_graph(
                graph_id,
                f"entity-{i}",
                max_depth=2,
            )["nodes"]
        ),
    )
    return [insert_nodes, insert_edges, traverse]


def run_observability(db: ProximaDB, scale: int) -> list[dict[str, Any]]:
    db.create_observability_namespace("bench_obs", retention_days=1)
    base_ns = 1_700_000_000_000_000_000
    logs = [
        {
            "timestamp_ns": base_ns + i,
            "severity": "INFO",
            "message": f"embedded benchmark log {i}",
            "source": "benchmark",
            "service": "embedded",
            "fields": {"tenant": "embedded", "seq": i},
        }
        for i in range(scale)
    ]
    metrics = [
        {
            "metric_name": "bench_latency_ms",
            "timestamp_ns": base_ns + i,
            "value": float(i % 100),
            "labels": {"service": "embedded"},
        }
        for i in range(scale)
    ]
    spans = [
        {
            "trace_id": f"trace-{i}",
            "span_id": f"span-{i}",
            "name": "embedded_benchmark",
            "kind": "INTERNAL",
            "start_time_ns": base_ns + i,
            "end_time_ns": base_ns + i + 1_000,
            "service": "embedded",
            "status_code": "OK",
            "attributes": {"tenant": "embedded"},
        }
        for i in range(scale)
    ]

    ingest_logs = timed(
        "observability.ingest_logs",
        scale,
        lambda: db.ingest_logs("bench_obs", logs),
    )
    ingest_metrics = timed(
        "observability.ingest_metrics",
        scale,
        lambda: db.ingest_metrics("bench_obs", metrics),
    )
    ingest_traces = timed(
        "observability.ingest_traces",
        scale,
        lambda: db.ingest_traces("bench_obs", spans),
    )
    query_logs = latency_sample(
        "observability.query_logs",
        min(50, scale),
        lambda _i: len(
            db.query_logs(
                "bench_obs",
                base_ns - 1,
                base_ns + scale + 1_000,
                query="benchmark",
                limit=10,
            )
        ),
    )
    query_metrics = latency_sample(
        "observability.aggregate_metrics",
        min(50, scale),
        lambda _i: len(
            db.aggregate_metrics(
                "bench_obs",
                "bench_latency_ms",
                aggregation="avg",
                start_time=None,
                end_time=None,
                step_seconds=60,
            )
        ),
    )
    query_traces = latency_sample(
        "observability.query_traces",
        min(50, scale),
        lambda i: len(
            db.query_traces(
                "bench_obs",
                base_ns - 1,
                base_ns + scale + 1_000,
                trace_id=f"trace-{i}",
                service="embedded",
                operation=None,
                min_duration_ns=None,
                status=None,
                limit=10,
            )
        ),
    )
    return [
        ingest_logs,
        ingest_metrics,
        ingest_traces,
        query_logs,
        query_metrics,
        query_traces,
    ]


def _comparison(
    baseline: dict[str, Any] | None,
    candidate: dict[str, Any] | None,
    *,
    name: str,
) -> dict[str, Any]:
    baseline_ops = baseline.get("ops_per_second") if baseline else None
    candidate_ops = candidate.get("ops_per_second") if candidate else None
    ratio = None
    delta_percent = None
    if baseline_ops and candidate_ops is not None:
        ratio = candidate_ops / baseline_ops
        delta_percent = (ratio - 1.0) * 100.0
    return {
        "name": name,
        "baseline": baseline["name"] if baseline else None,
        "candidate": candidate["name"] if candidate else None,
        "baseline_ops_per_second": baseline_ops,
        "candidate_ops_per_second": candidate_ops,
        "candidate_to_baseline_ratio": ratio,
        "delta_percent": delta_percent,
    }


def compare_wire_format(results: list[dict[str, Any]]) -> list[dict[str, Any]]:
    by_name = {result["name"]: result for result in results}
    return [
        _comparison(
            by_name.get("vector.insert_numpy"),
            by_name.get("record_wire.vector_insert"),
            name="vector_legacy_vs_proximarecord_wire",
        ),
        _comparison(
            by_name.get("document.insert"),
            by_name.get("record_wire.document_insert"),
            name="document_facade_vs_proximarecord_wire",
        ),
        _comparison(
            by_name.get("graph_entity.create_nodes"),
            by_name.get("record_wire.graph_node_insert"),
            name="graph_node_facade_vs_proximarecord_wire",
        ),
        _comparison(
            by_name.get("observability.ingest_logs"),
            by_name.get("record_wire.observability_event_insert"),
            name="observability_log_facade_vs_proximarecord_wire",
        ),
    ]


def run_record_wire(db: ProximaDB, scale: int, dimension: int) -> list[dict[str, Any]]:
    rng = np.random.default_rng(84)
    vector_count = scale * 5
    vectors = rng.random((vector_count, dimension), dtype=np.float32)

    db.create_collection("bench_record_wire_vectors", dimension=dimension, engine="sst")
    vector_records = [
        ProximaRecord(
            id=f"wire-vec-{i}",
            vector=vectors[i],
            props={"tenant": "embedded", "kind": "vector", "seq": i},
            source="python-embedded-benchmark",
            schema_id="benchmark.vector.v1",
        )
        for i in range(vector_count)
    ]
    vector_insert = timed(
        "record_wire.vector_insert",
        vector_count,
        lambda: insert_proxima_records(
            db,
            "bench_record_wire_vectors",
            vector_records,
        ),
    )
    db.create_collection("bench_record_wire_vectors_profiled", dimension=dimension, engine="sst")
    vector_insert_profiled = profiled_insert_metric(
        "record_wire.vector_insert_profiled",
        vector_count,
        lambda: insert_records_profiled(
            db,
            "bench_record_wire_vectors_profiled",
            vector_records,
        ),
    )
    dict_records = [
        {
            "id": f"wire-dict-vec-{i}",
            "vector": vectors[i],
            "props": {"tenant": "embedded", "kind": "vector", "seq": i},
            "source": "python-embedded-benchmark",
            "schema_id": "benchmark.vector.v1",
        }
        for i in range(vector_count)
    ]
    db.create_collection("bench_record_wire_vectors_dict_profiled", dimension=dimension, engine="sst")
    vector_dict_insert_profiled = profiled_insert_metric(
        "record_wire.vector_dict_insert_profiled",
        vector_count,
        lambda: insert_records_profiled(
            db,
            "bench_record_wire_vectors_dict_profiled",
            dict_records,
        ),
    )
    vector_search = latency_sample(
        "record_wire.vector_search_top10",
        min(50, scale),
        lambda i: len(
            db.search("bench_record_wire_vectors", query=vectors[i], top_k=10)
        ),
    )

    zero_vector = [0.0] * dimension
    db.create_collection("bench_record_wire_docs", dimension=dimension, engine="sst")
    document_records = [
        normalize_document(
            f"wire-doc-{i}",
            {
                "kind": "note" if i % 2 == 0 else "event",
                "tenant": "embedded",
                "score": i,
                "payload": {"title": f"doc-{i}"},
            },
            zero_vector,
            text_columns=["kind"],
        )
        for i in range(scale)
    ]
    document_insert = timed(
        "record_wire.document_insert",
        scale,
        lambda: insert_proxima_records(
            db,
            "bench_record_wire_docs",
            document_records,
        ),
    )
    db.create_collection("bench_record_wire_docs_profiled", dimension=dimension, engine="sst")
    document_insert_profiled = profiled_insert_metric(
        "record_wire.document_insert_profiled",
        scale,
        lambda: insert_records_profiled(
            db,
            "bench_record_wire_docs_profiled",
            document_records,
        ),
    )

    db.create_collection("bench_record_wire_graph_nodes", dimension=dimension, engine="sst")
    graph_records = [
        normalize_graph_node(
            f"wire-entity-{i}",
            ["Entity", "Account" if i % 2 == 0 else "Person"],
            {"tenant": "embedded", "seq": i},
            zero_vector,
        )
        for i in range(scale)
    ]
    graph_insert = timed(
        "record_wire.graph_node_insert",
        scale,
        lambda: insert_proxima_records(
            db,
            "bench_record_wire_graph_nodes",
            graph_records,
        ),
    )
    db.create_collection("bench_record_wire_graph_nodes_profiled", dimension=dimension, engine="sst")
    graph_insert_profiled = profiled_insert_metric(
        "record_wire.graph_node_insert_profiled",
        scale,
        lambda: insert_records_profiled(
            db,
            "bench_record_wire_graph_nodes_profiled",
            graph_records,
        ),
    )

    db.create_collection("bench_record_wire_observability", dimension=dimension, engine="sst")
    base_ns = 1_700_000_000_000_000_000
    observability_records = [
        normalize_observability_event(
            f"wire-log-{i}",
            {
                "timestamp_ns": base_ns + i,
                "severity": "INFO",
                "message": f"embedded benchmark log {i}",
                "source": "benchmark",
                "service": "embedded",
                "fields": {"tenant": "embedded", "seq": i},
            },
            zero_vector,
            event_type="log",
        )
        for i in range(scale)
    ]
    observability_insert = timed(
        "record_wire.observability_event_insert",
        scale,
        lambda: insert_proxima_records(
            db,
            "bench_record_wire_observability",
            observability_records,
        ),
    )
    db.create_collection("bench_record_wire_observability_profiled", dimension=dimension, engine="sst")
    observability_insert_profiled = profiled_insert_metric(
        "record_wire.observability_event_insert_profiled",
        scale,
        lambda: insert_records_profiled(
            db,
            "bench_record_wire_observability_profiled",
            observability_records,
        ),
    )

    return [
        vector_insert,
        vector_insert_profiled,
        vector_dict_insert_profiled,
        vector_search,
        document_insert,
        document_insert_profiled,
        graph_insert,
        graph_insert_profiled,
        observability_insert,
        observability_insert_profiled,
    ]


def run(data_dir: Path, scale: int, dimension: int) -> dict[str, Any]:
    db = ProximaDB(data_dirs=str(data_dir), cache_size_mb=512, default_engine="sst")
    results = []
    results.extend(run_suite("vector", lambda: run_vector(db, scale, dimension)))
    results.extend(run_suite("arrow_embedded", lambda: run_arrow_batch(db, scale, dimension)))
    results.extend(run_suite("relational", lambda: run_relational(db, scale, dimension)))
    results.extend(run_suite("document", lambda: run_documents(db, scale)))
    results.extend(run_suite("graph_entity", lambda: run_graph_entity(db, scale)))
    results.extend(run_suite("observability", lambda: run_observability(db, scale)))
    results.extend(run_suite("record_wire", lambda: run_record_wire(db, scale, dimension)))
    db.flush()

    return {
        "benchmark": "embedded_python_modalities_record_wire_comparison",
        "scale": scale,
        "dimension": dimension,
        "python": platform.python_version(),
        "platform": platform.platform(),
        "data_dir": str(data_dir),
        "results": results,
        "wire_format_comparison": compare_wire_format(results),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-dir", type=Path, default=None)
    parser.add_argument("--scale", type=int, default=200)
    parser.add_argument("--dimension", type=int, default=64)
    parser.add_argument("--json-out", type=Path, default=None)
    args = parser.parse_args()

    if args.scale < 2:
        raise SystemExit("--scale must be at least 2")
    if args.dimension < 1:
        raise SystemExit("--dimension must be at least 1")

    if args.data_dir is None:
        with tempfile.TemporaryDirectory(prefix="proximadb-embedded-bench-") as tmp:
            report = run(Path(tmp), args.scale, args.dimension)
    else:
        args.data_dir.mkdir(parents=True, exist_ok=True)
        report = run(args.data_dir, args.scale, args.dimension)

    payload = json.dumps(report, indent=2, sort_keys=True)
    print(payload)
    if args.json_out is not None:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(payload + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
