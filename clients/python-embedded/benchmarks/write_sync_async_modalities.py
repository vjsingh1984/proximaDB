"""Sync vs async write benchmarks for the embedded Python API.

Measures sync write throughput (single thread, sequential) against async
throughput (asyncio with `asyncio.to_thread` + `gather`) across every
modality the embedded surface exposes: vector, ProximaRecord, document,
graph node, observability, Arrow, and SQL DML.

The async path is the realistic "Python async app" model: the embedded
SDK itself is a blocking PyO3 call, so async clients offload each call
to a thread-pool worker and let asyncio overlap them. Concurrency is
controlled via --concurrency (default 8 workers).

Output JSON shape mirrors `baseline_modalities.py` so the comparison
script can diff numbers directly.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import platform
import statistics
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Iterable

import numpy as np

from proximadb_embedded import (
    GraphEdge,
    GraphNode,
    ProximaDB,
    ProximaRecord,
    insert_proxima_records,
    normalize_document,
    normalize_graph_node,
    normalize_observability_event,
)
from proximadb_embedded import insert_arrow as embedded_insert_arrow


# ---------------------------------------------------------------------------
# Timing helpers
# ---------------------------------------------------------------------------


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
        "mode": "sync",
    }


async def timed_async(
    name: str,
    op_count: int,
    coro_factory: Callable[[], Any],
) -> dict[str, Any]:
    started = time.perf_counter()
    result = await coro_factory()
    elapsed = time.perf_counter() - started
    return {
        "name": name,
        "operations": op_count,
        "seconds": elapsed,
        "ops_per_second": op_count / elapsed if elapsed > 0 else None,
        "result": result,
        "mode": "async",
    }


# ---------------------------------------------------------------------------
# Chunking helpers — async path splits a batch into N concurrent sub-batches
# so asyncio.gather can overlap thread-pool workers.
# ---------------------------------------------------------------------------


def chunk_indices(total: int, concurrency: int) -> list[tuple[int, int]]:
    """Return [(start, end)] half-open ranges splitting `total` into `concurrency` chunks."""
    if concurrency <= 0:
        concurrency = 1
    chunk = max(1, (total + concurrency - 1) // concurrency)
    ranges = []
    for start in range(0, total, chunk):
        end = min(total, start + chunk)
        ranges.append((start, end))
    return ranges


# ---------------------------------------------------------------------------
# Vector modality
# ---------------------------------------------------------------------------


def run_vector_sync(db: ProximaDB, count: int, dimension: int) -> dict[str, Any]:
    rng = np.random.default_rng(42)
    vectors = rng.random((count, dimension), dtype=np.float32)
    ids = [f"vec_sync_{i}" for i in range(count)]
    metadata = [{"tenant": "embedded", "kind": "vector", "seq": i} for i in range(count)]
    db.create_collection("bench_vec_sync", dimension=dimension, engine="sst")
    return timed(
        "vector.insert_numpy.sync",
        count,
        lambda: db.insert("bench_vec_sync", ids=ids, vectors=vectors, metadata=metadata),
    )


async def run_vector_async(
    db: ProximaDB, count: int, dimension: int, concurrency: int
) -> dict[str, Any]:
    rng = np.random.default_rng(43)
    vectors = rng.random((count, dimension), dtype=np.float32)
    ids = [f"vec_async_{i}" for i in range(count)]
    metadata = [{"tenant": "embedded", "kind": "vector", "seq": i} for i in range(count)]
    db.create_collection("bench_vec_async", dimension=dimension, engine="sst")

    def insert_chunk(start: int, end: int) -> int:
        return db.insert(
            "bench_vec_async",
            ids=ids[start:end],
            vectors=vectors[start:end],
            metadata=metadata[start:end],
        )

    async def coro_factory() -> int:
        tasks = [
            asyncio.to_thread(insert_chunk, start, end)
            for start, end in chunk_indices(count, concurrency)
        ]
        return sum(await asyncio.gather(*tasks))

    return await timed_async("vector.insert_numpy.async", count, coro_factory)


# ---------------------------------------------------------------------------
# ProximaRecord (canonical wire) modality
# ---------------------------------------------------------------------------


def build_proxima_records(count: int, vectors: np.ndarray, tag: str) -> list[ProximaRecord]:
    return [
        ProximaRecord(
            id=f"wire-{tag}-{i}",
            vector=vectors[i],
            props={"tenant": "embedded", "kind": "vector", "seq": i},
            source="bench-sync-async",
            schema_id="benchmark.vector.v1",
        )
        for i in range(count)
    ]


def run_record_wire_sync(db: ProximaDB, count: int, dimension: int) -> dict[str, Any]:
    rng = np.random.default_rng(101)
    vectors = rng.random((count, dimension), dtype=np.float32)
    records = build_proxima_records(count, vectors, "sync")
    db.create_collection("bench_wire_sync", dimension=dimension, engine="sst")
    return timed(
        "record_wire.vector_insert.sync",
        count,
        lambda: insert_proxima_records(db, "bench_wire_sync", records),
    )


async def run_record_wire_async(
    db: ProximaDB, count: int, dimension: int, concurrency: int
) -> dict[str, Any]:
    rng = np.random.default_rng(102)
    vectors = rng.random((count, dimension), dtype=np.float32)
    records = build_proxima_records(count, vectors, "async")
    db.create_collection("bench_wire_async", dimension=dimension, engine="sst")

    def insert_chunk(start: int, end: int) -> int:
        return insert_proxima_records(db, "bench_wire_async", records[start:end])

    async def coro_factory() -> int:
        tasks = [
            asyncio.to_thread(insert_chunk, start, end)
            for start, end in chunk_indices(count, concurrency)
        ]
        return sum(await asyncio.gather(*tasks))

    return await timed_async("record_wire.vector_insert.async", count, coro_factory)


# ---------------------------------------------------------------------------
# Document modality
# ---------------------------------------------------------------------------


def run_document_sync(db: ProximaDB, count: int) -> dict[str, Any]:
    db.create_document_collection("bench_doc_sync", indexed_paths=["$.kind", "$.tenant"])

    def insert_docs() -> int:
        for i in range(count):
            db.insert_document(
                "bench_doc_sync",
                {
                    "kind": "note" if i % 2 == 0 else "event",
                    "tenant": "embedded",
                    "score": i,
                    "payload": {"title": f"doc-{i}"},
                },
                doc_id=f"doc-sync-{i}",
            )
        return count

    return timed("document.insert.sync", count, insert_docs)


async def run_document_async(
    db: ProximaDB, count: int, concurrency: int
) -> dict[str, Any]:
    db.create_document_collection("bench_doc_async", indexed_paths=["$.kind", "$.tenant"])

    def insert_one(i: int) -> None:
        db.insert_document(
            "bench_doc_async",
            {
                "kind": "note" if i % 2 == 0 else "event",
                "tenant": "embedded",
                "score": i,
                "payload": {"title": f"doc-{i}"},
            },
            doc_id=f"doc-async-{i}",
        )

    def insert_chunk(start: int, end: int) -> int:
        for i in range(start, end):
            insert_one(i)
        return end - start

    async def coro_factory() -> int:
        tasks = [
            asyncio.to_thread(insert_chunk, start, end)
            for start, end in chunk_indices(count, concurrency)
        ]
        return sum(await asyncio.gather(*tasks))

    return await timed_async("document.insert.async", count, coro_factory)


# ---------------------------------------------------------------------------
# Graph entity modality (nodes + edges)
# ---------------------------------------------------------------------------


def make_nodes(count: int, tag: str) -> list[GraphNode]:
    return [
        GraphNode(
            f"entity-{tag}-{i}",
            labels=["Entity", "Account" if i % 2 == 0 else "Person"],
            properties={"tenant": "embedded", "seq": i},
        )
        for i in range(count)
    ]


def make_edges(count: int, tag: str) -> list[GraphEdge]:
    return [
        GraphEdge(
            f"entity-{tag}-{i}",
            f"entity-{tag}-{i + 1}",
            "RELATED_TO",
            id=f"edge-{tag}-{i}",
            weight=1.0,
            properties={"tenant": "embedded"},
        )
        for i in range(count - 1)
    ]


def run_graph_sync(db: ProximaDB, count: int) -> list[dict[str, Any]]:
    graph_id = f"bench_graph_sync_{uuid.uuid4().hex}"
    db.create_graph(graph_id)
    nodes = make_nodes(count, "sync")
    edges = make_edges(count, "sync")
    create_nodes = timed(
        "graph_entity.create_nodes.sync",
        len(nodes),
        lambda: db.create_nodes(graph_id, nodes),
    )
    create_edges = timed(
        "graph_entity.create_edges.sync",
        len(edges),
        lambda: db.create_edges(graph_id, edges),
    )
    return [create_nodes, create_edges]


async def run_graph_async(
    db: ProximaDB, count: int, concurrency: int
) -> list[dict[str, Any]]:
    graph_id = f"bench_graph_async_{uuid.uuid4().hex}"
    db.create_graph(graph_id)
    nodes = make_nodes(count, "async")
    edges = make_edges(count, "async")

    def insert_nodes_chunk(start: int, end: int) -> int:
        return db.create_nodes(graph_id, nodes[start:end])

    def insert_edges_chunk(start: int, end: int) -> int:
        return db.create_edges(graph_id, edges[start:end])

    async def nodes_coro() -> int:
        tasks = [
            asyncio.to_thread(insert_nodes_chunk, start, end)
            for start, end in chunk_indices(len(nodes), concurrency)
        ]
        return sum(await asyncio.gather(*tasks))

    async def edges_coro() -> int:
        tasks = [
            asyncio.to_thread(insert_edges_chunk, start, end)
            for start, end in chunk_indices(len(edges), concurrency)
        ]
        return sum(await asyncio.gather(*tasks))

    create_nodes = await timed_async("graph_entity.create_nodes.async", len(nodes), nodes_coro)
    create_edges = await timed_async("graph_entity.create_edges.async", len(edges), edges_coro)
    return [create_nodes, create_edges]


# ---------------------------------------------------------------------------
# Observability modality
# ---------------------------------------------------------------------------


def make_logs(count: int, base_ns: int, tag: str) -> list[dict[str, Any]]:
    return [
        {
            "timestamp_ns": base_ns + i,
            "severity": "INFO",
            "message": f"embedded {tag} log {i}",
            "source": "benchmark",
            "service": "embedded",
            "fields": {"tenant": "embedded", "seq": i},
        }
        for i in range(count)
    ]


def make_metrics(count: int, base_ns: int) -> list[dict[str, Any]]:
    return [
        {
            "metric_name": "bench_latency_ms",
            "timestamp_ns": base_ns + i,
            "value": float(i % 100),
            "labels": {"service": "embedded"},
        }
        for i in range(count)
    ]


def make_traces(count: int, base_ns: int, tag: str) -> list[dict[str, Any]]:
    return [
        {
            "trace_id": f"trace-{tag}-{i}",
            "span_id": f"span-{tag}-{i}",
            "name": "embedded_benchmark",
            "kind": "INTERNAL",
            "start_time_ns": base_ns + i,
            "end_time_ns": base_ns + i + 1_000,
            "service": "embedded",
            "status_code": "OK",
            "attributes": {"tenant": "embedded"},
        }
        for i in range(count)
    ]


def run_observability_sync(db: ProximaDB, count: int) -> list[dict[str, Any]]:
    db.create_observability_namespace("bench_obs_sync", retention_days=1)
    base_ns = 1_700_000_000_000_000_000
    logs = make_logs(count, base_ns, "sync")
    metrics = make_metrics(count, base_ns)
    traces = make_traces(count, base_ns, "sync")
    return [
        timed("observability.ingest_logs.sync", count, lambda: db.ingest_logs("bench_obs_sync", logs)),
        timed(
            "observability.ingest_metrics.sync",
            count,
            lambda: db.ingest_metrics("bench_obs_sync", metrics),
        ),
        timed(
            "observability.ingest_traces.sync",
            count,
            lambda: db.ingest_traces("bench_obs_sync", traces),
        ),
    ]


async def run_observability_async(
    db: ProximaDB, count: int, concurrency: int
) -> list[dict[str, Any]]:
    db.create_observability_namespace("bench_obs_async", retention_days=1)
    base_ns = 1_700_000_000_000_000_001
    logs = make_logs(count, base_ns, "async")
    metrics = make_metrics(count, base_ns)
    traces = make_traces(count, base_ns, "async")

    async def chunked(
        name: str, items: list[dict[str, Any]], ingest_fn: Callable[[list[dict[str, Any]]], int]
    ) -> dict[str, Any]:
        async def coro_factory() -> int:
            tasks = [
                asyncio.to_thread(ingest_fn, items[start:end])
                for start, end in chunk_indices(len(items), concurrency)
            ]
            return sum(await asyncio.gather(*tasks))

        return await timed_async(name, len(items), coro_factory)

    return [
        await chunked(
            "observability.ingest_logs.async",
            logs,
            lambda batch: db.ingest_logs("bench_obs_async", batch),
        ),
        await chunked(
            "observability.ingest_metrics.async",
            metrics,
            lambda batch: db.ingest_metrics("bench_obs_async", batch),
        ),
        await chunked(
            "observability.ingest_traces.async",
            traces,
            lambda batch: db.ingest_traces("bench_obs_async", batch),
        ),
    ]


# ---------------------------------------------------------------------------
# Arrow modality
# ---------------------------------------------------------------------------


def run_arrow_sync(db: ProximaDB, count: int, dimension: int) -> dict[str, Any] | None:
    try:
        import pyarrow as pa
    except ImportError:
        return None
    db.create_collection("bench_arrow_sync", dimension=dimension, engine="sst")
    vectors = [
        [float((row + col) % 100) / 100.0 for col in range(dimension)] for row in range(count)
    ]
    table = pa.table(
        {
            "id": [f"arrow-sync-{i}" for i in range(count)],
            "vector": pa.array(vectors, type=pa.list_(pa.float32())),
            "tenant_id": ["embedded"] * count,
            "kind": ["arrow"] * count,
        }
    )
    return timed(
        "arrow.insert_arrow.sync",
        count,
        lambda: embedded_insert_arrow(db, "bench_arrow_sync", table),
    )


async def run_arrow_async(
    db: ProximaDB, count: int, dimension: int, concurrency: int
) -> dict[str, Any] | None:
    try:
        import pyarrow as pa
    except ImportError:
        return None
    db.create_collection("bench_arrow_async", dimension=dimension, engine="sst")

    def make_table(start: int, end: int) -> "pa.Table":
        vectors = [
            [float((row + col) % 100) / 100.0 for col in range(dimension)]
            for row in range(start, end)
        ]
        return pa.table(
            {
                "id": [f"arrow-async-{i}" for i in range(start, end)],
                "vector": pa.array(vectors, type=pa.list_(pa.float32())),
                "tenant_id": ["embedded"] * (end - start),
                "kind": ["arrow"] * (end - start),
            }
        )

    def insert_chunk(start: int, end: int) -> int:
        return embedded_insert_arrow(db, "bench_arrow_async", make_table(start, end))

    async def coro_factory() -> int:
        tasks = [
            asyncio.to_thread(insert_chunk, start, end)
            for start, end in chunk_indices(count, concurrency)
        ]
        return sum(await asyncio.gather(*tasks))

    return await timed_async("arrow.insert_arrow.async", count, coro_factory)


# ---------------------------------------------------------------------------
# SQL DML modality (sync only; SQL DML batches are already a single call)
# ---------------------------------------------------------------------------


def run_sql_dml(db: ProximaDB, count: int, dimension: int) -> list[dict[str, Any]]:
    db.execute_sql(
        f"""
        CREATE TABLE IF NOT EXISTS bench_sql_batch (
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

    sql_chunk = 1000  # chunk the SQL multi-row INSERT to keep statement size sane

    def insert_batch() -> int:
        rows_inserted = 0
        for chunk_start in range(0, count, sql_chunk):
            chunk_end = min(count, chunk_start + sql_chunk)
            values = ",\n".join(
                f"('acct-batch-{i}', '{{\"tier\":\"gold\",\"seq\":{i}}}'::jsonb, '{vector_literal}')"
                for i in range(chunk_start, chunk_end)
            )
            db.execute_sql(
                f"INSERT INTO bench_sql_batch (account_id, payload, embedding) VALUES {values};"
            )
            rows_inserted += chunk_end - chunk_start
        return rows_inserted

    return [timed("relational.sql_insert_multirow_batch.sync", count, insert_batch)]


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def run_search(db: ProximaDB, count: int, dimension: int) -> list[dict[str, Any]]:
    """Reuse vector + record-wire collections inserted above and measure searches."""
    rng = np.random.default_rng(99)
    queries = rng.random((min(50, count), dimension), dtype=np.float32)
    results: list[dict[str, Any]] = []

    def latency(name: str, op_count: int, func: Callable[[int], Any]) -> dict[str, Any]:
        samples_ms: list[float] = []
        for i in range(op_count):
            started = time.perf_counter()
            func(i)
            samples_ms.append((time.perf_counter() - started) * 1000.0)
        samples_sorted = sorted(samples_ms)
        total_seconds = sum(samples_ms) / 1000.0
        return {
            "name": name,
            "operations": op_count,
            "seconds": total_seconds,
            "ops_per_second": op_count / total_seconds if total_seconds > 0 else None,
            "mean_ms": statistics.fmean(samples_ms),
            "p95_ms": samples_sorted[min(len(samples_sorted) - 1, int(len(samples_sorted) * 0.95))],
            "p99_ms": samples_sorted[min(len(samples_sorted) - 1, int(len(samples_sorted) * 0.99))],
            "mode": "sync",
        }

    # Vector search against the sync collection (legacy path)
    results.append(
        latency(
            "vector.search_top10.sync",
            len(queries),
            lambda i: len(db.search("bench_vec_sync", query=queries[i], top_k=10)),
        )
    )
    # Same against the async-inserted collection — verifies parity
    results.append(
        latency(
            "vector.search_top10.async_inserted",
            len(queries),
            lambda i: len(db.search("bench_vec_async", query=queries[i], top_k=10)),
        )
    )
    # ProximaRecord wire collection
    results.append(
        latency(
            "record_wire.vector_search_top10.sync",
            len(queries),
            lambda i: len(db.search("bench_wire_sync", query=queries[i], top_k=10)),
        )
    )
    return results


async def run_async_writes(
    db: ProximaDB, count: int, dimension: int, concurrency: int
) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    results.append(await run_vector_async(db, count, dimension, concurrency))
    results.append(await run_record_wire_async(db, count, dimension, concurrency))
    results.append(await run_document_async(db, count, concurrency))
    results.extend(await run_graph_async(db, count, concurrency))
    results.extend(await run_observability_async(db, count, concurrency))
    arrow_result = await run_arrow_async(db, count, dimension, concurrency)
    if arrow_result is not None:
        results.append(arrow_result)
    return results


def run_sync_writes(db: ProximaDB, count: int, dimension: int) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    results.append(run_vector_sync(db, count, dimension))
    results.append(run_record_wire_sync(db, count, dimension))
    results.append(run_document_sync(db, count))
    results.extend(run_graph_sync(db, count))
    results.extend(run_observability_sync(db, count))
    arrow_result = run_arrow_sync(db, count, dimension)
    if arrow_result is not None:
        results.append(arrow_result)
    results.extend(run_sql_dml(db, count, dimension))
    return results


def run(
    data_dir: Path, count: int, dimension: int, concurrency: int, include_search: bool
) -> dict[str, Any]:
    db = ProximaDB(data_dirs=str(data_dir), cache_size_mb=512, default_engine="sst")
    try:
        sync_results = run_sync_writes(db, count, dimension)
        async_results = asyncio.run(run_async_writes(db, count, dimension, concurrency))
        all_results = sync_results + async_results
        if include_search:
            all_results.extend(run_search(db, count, dimension))
        db.flush()
        return {
            "benchmark": "embedded_python_sync_vs_async_writes",
            "rows_per_modality": count,
            "dimension": dimension,
            "async_concurrency": concurrency,
            "python": platform.python_version(),
            "platform": platform.platform(),
            "data_dir": str(data_dir),
            "results": all_results,
        }
    finally:
        db.close()


def aggregate(reports: list[dict[str, Any]]) -> dict[str, Any]:
    if len(reports) == 1:
        return reports[0]
    by_name: dict[str, list[float]] = {}
    for report in reports:
        for result in report.get("results", []):
            ops = result.get("ops_per_second")
            if ops is not None:
                by_name.setdefault(result["name"], []).append(ops)
    aggregate_rows = []
    for name, values in sorted(by_name.items()):
        aggregate_rows.append(
            {
                "name": name,
                "runs": len(values),
                "ops_per_second_min": min(values),
                "ops_per_second_median": statistics.median(values),
                "ops_per_second_max": max(values),
            }
        )
    representative = reports[-1]
    return {
        "benchmark": representative["benchmark"],
        "rows_per_modality": representative["rows_per_modality"],
        "dimension": representative["dimension"],
        "async_concurrency": representative["async_concurrency"],
        "python": representative["python"],
        "platform": representative["platform"],
        "runs": len(reports),
        "aggregate_results": aggregate_rows,
        "reports": reports,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-dir", type=Path, default=None)
    parser.add_argument("--count", type=int, default=1000, help="Rows per modality per run")
    parser.add_argument("--dimension", type=int, default=64)
    parser.add_argument("--concurrency", type=int, default=8, help="Async asyncio.to_thread workers")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument(
        "--no-search",
        action="store_true",
        help="Skip the search latency block (useful when only timing writes).",
    )
    parser.add_argument("--json-out", type=Path, default=None)
    args = parser.parse_args()

    if args.count < 2:
        raise SystemExit("--count must be at least 2")
    if args.dimension < 1:
        raise SystemExit("--dimension must be at least 1")
    if args.runs < 1:
        raise SystemExit("--runs must be at least 1")
    if args.concurrency < 1:
        raise SystemExit("--concurrency must be at least 1")

    reports: list[dict[str, Any]] = []
    for run_index in range(args.runs):
        if args.data_dir is None:
            with tempfile.TemporaryDirectory(prefix=f"proximadb-bench-async-r{run_index}-") as tmp:
                reports.append(
                    run(Path(tmp), args.count, args.dimension, args.concurrency, not args.no_search)
                )
        else:
            run_dir = args.data_dir / f"run-{run_index}"
            run_dir.mkdir(parents=True, exist_ok=True)
            reports.append(
                run(run_dir, args.count, args.dimension, args.concurrency, not args.no_search)
            )

    report = aggregate(reports)
    payload = json.dumps(report, indent=2, sort_keys=True)
    print(payload)
    if args.json_out is not None:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(payload + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
