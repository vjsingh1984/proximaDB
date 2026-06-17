#!/usr/bin/env python3
"""Benchmark the embedded ProximaDB shape needed by Victor/codingagent.

This benchmark focuses on the actual embedded integration profile:

- Vector ingest/search for 10K, 100K, and 1M 384D BGE-shaped vectors
- Graph ingest/query/traversal for code/CCG-style nodes and edges
- Document ingest/query for code documents
- SQL query latency over embedded collections
- Logs, metrics, and traces for JSONL-style observability data

The vector baseline uses the native embedded module directly with NumPy batch
inserts to measure the release-performance ceiling. The multimodel benchmark
uses the public Python client plus its exposed native handle, which mirrors the
way Victor's ProximaDB provider integrates today.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
import time
from collections.abc import Callable, Iterable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import numpy as np

REPO_ROOT = Path(__file__).resolve().parents[3]
PYTHON_SRC = Path(__file__).resolve().parents[1] / "src"
if str(PYTHON_SRC) not in sys.path:
    sys.path.insert(0, str(PYTHON_SRC))

try:
    import proximadb_embedded as proximadb
except ImportError:
    try:
        import proximadb  # type: ignore[no-redef]
    except ImportError as exc:  # pragma: no cover - benchmark import guard
        raise SystemExit(
            "Embedded ProximaDB module is not available. Build/install "
            "`proximadb-embedded` first."
        ) from exc


DIMENSION = 384
VECTOR_COLLECTION = "code_vectors"
DOCUMENT_COLLECTION = "code_documents"
GRAPH_COLLECTION = "code_graph"
OBS_NAMESPACE = "code_observability"


@dataclass
class MetricResult:
    name: str
    count: int
    total_ms: float
    throughput: float
    avg_ms: float | None = None
    p50_ms: float | None = None
    p95_ms: float | None = None
    max_ms: float | None = None
    notes: str | None = None


def _normalized_random_vectors(count: int, dimension: int, seed: int) -> np.ndarray:
    rng = np.random.default_rng(seed)
    vectors = rng.standard_normal((count, dimension)).astype(np.float32)
    norms = np.linalg.norm(vectors, axis=1, keepdims=True)
    norms[norms == 0.0] = 1.0
    return vectors / norms


def _run_samples(iterations: int, fn: Callable[[], Any]) -> tuple[list[float], Any]:
    latencies_ms: list[float] = []
    last_result = None
    for _ in range(iterations):
        start = time.perf_counter()
        last_result = fn()
        latencies_ms.append((time.perf_counter() - start) * 1000.0)
    return latencies_ms, last_result


def _run_query_samples(
    queries: Iterable[np.ndarray],
    fn: Callable[[np.ndarray], Any],
) -> tuple[list[float], Any]:
    latencies_ms: list[float] = []
    last_result = None
    for query in queries:
        start = time.perf_counter()
        last_result = fn(query)
        latencies_ms.append((time.perf_counter() - start) * 1000.0)
    return latencies_ms, last_result


def _format_vector_literal(vector: np.ndarray) -> str:
    return "[" + ",".join(f"{float(value):.6f}" for value in vector.tolist()) + "]"


def _wait_for_embedded_axis_ready(
    base_dir: str,
    timeout_s: float,
    poll_interval_s: float = 0.5,
) -> dict[str, Any]:
    if timeout_s <= 0:
        return {
            "enabled": False,
            "ready": False,
            "wait_ms": 0.0,
            "tracked_files": 0,
        }

    queue_state_paths = list(Path(base_dir).glob("queue/queue/*/state.json"))
    if not queue_state_paths:
        return {
            "enabled": True,
            "ready": False,
            "wait_ms": 0.0,
            "tracked_files": 0,
            "reason": "queue_state_missing",
        }

    start = time.perf_counter()
    deadline = start + timeout_s
    last_state: dict[str, Any] = {}

    while time.perf_counter() < deadline:
        with queue_state_paths[0].open("r", encoding="utf-8") as handle:
            last_state = json.load(handle)
        file_status = last_state.get("file_status", [])
        if file_status and all(
            status.get("ready_for_compaction", False) for status in file_status
        ):
            return {
                "enabled": True,
                "ready": True,
                "wait_ms": (time.perf_counter() - start) * 1000.0,
                "tracked_files": len(file_status),
            }
        time.sleep(poll_interval_s)

    file_status = last_state.get("file_status", [])
    return {
        "enabled": True,
        "ready": False,
        "wait_ms": (time.perf_counter() - start) * 1000.0,
        "tracked_files": len(file_status),
    }


def _build_vector_metadata(start: int, count: int) -> list[dict[str, Any]]:
    return [
        {
            "file_path": f"src/module_{(start + i) % 100}.py",
            "symbol_name": f"symbol_{start + i}",
            "language": "python",
            "kind": "function" if (start + i) % 2 == 0 else "class",
            "line_number": start + i,
        }
        for i in range(count)
    ]


def _vector_insert_baseline(
    native_db: Any,
    collection: str,
    total_vectors: int,
    batch_size: int,
    dimension: int,
    seed: int,
    with_metadata: bool = False,
) -> tuple[MetricResult, np.ndarray, list[np.ndarray]]:
    start = time.perf_counter()
    query_vector: np.ndarray | None = None
    query_vectors: list[np.ndarray] = []

    for offset in range(0, total_vectors, batch_size):
        current = min(batch_size, total_vectors - offset)
        vectors = _normalized_random_vectors(current, dimension, seed + offset)
        ids = [f"vec_{offset + i}" for i in range(current)]
        metadata = _build_vector_metadata(offset, current) if with_metadata else None

        if hasattr(native_db, "insert_numpy"):
            native_db.insert_numpy(collection, ids, vectors, metadata)
        else:
            native_db.insert(collection, ids, vectors.tolist(), metadata)

        if query_vector is None:
            query_vector = vectors[0].copy()
        remaining = 25 - len(query_vectors)
        if remaining > 0:
            query_vectors.extend(vectors[:remaining].copy())

    total_ms = (time.perf_counter() - start) * 1000.0
    throughput = total_vectors / (total_ms / 1000.0)
    assert query_vector is not None

    return (
        MetricResult(
            name=f"{collection}_insert",
            count=total_vectors,
            total_ms=total_ms,
            throughput=throughput,
        ),
        query_vector,
        query_vectors,
    )


def benchmark_native_vector_surface(
    vector_counts: Iterable[int],
    batch_size: int,
    dimension: int,
    engine: str,
    search_mode: str | None,
    wait_for_axis_ready_s: float,
) -> list[MetricResult]:
    results: list[MetricResult] = []

    for total_vectors in vector_counts:
        temp_dir = tempfile.mkdtemp(prefix=f"proximadb_vec_{total_vectors}_")
        try:
            native = proximadb.ProximaDB(
                data_dirs=temp_dir,
                default_engine=engine,
                cache_size_mb=1024,
                enable_wal=True,
            )
            native.create_collection(VECTOR_COLLECTION, dimension, engine)

            insert_result, query_vector, query_vectors = _vector_insert_baseline(
                native_db=native,
                collection=VECTOR_COLLECTION,
                total_vectors=total_vectors,
                batch_size=batch_size,
                dimension=dimension,
                seed=11,
            )
            results.append(insert_result)
            _flush_if_supported(native)
            axis_wait = _wait_for_embedded_axis_ready(temp_dir, wait_for_axis_ready_s)

            def run_search(query: np.ndarray) -> Any:
                if hasattr(native, "search_numpy"):
                    if search_mode:
                        return native.search_numpy(
                            VECTOR_COLLECTION, query, 10, None, search_mode
                        )
                    return native.search_numpy(VECTOR_COLLECTION, query, 10)
                return native.search(VECTOR_COLLECTION, query.tolist(), 10)

            latencies_ms, last_results = _run_query_samples(query_vectors, run_search)
            results.append(
                MetricResult(
                    name=f"{VECTOR_COLLECTION}_search",
                    count=len(latencies_ms),
                    total_ms=float(sum(latencies_ms)),
                    throughput=1000.0 / float(np.mean(latencies_ms)),
                    avg_ms=float(np.mean(latencies_ms)),
                    p50_ms=float(np.percentile(latencies_ms, 50)),
                    p95_ms=float(np.percentile(latencies_ms, 95)),
                    max_ms=float(np.max(latencies_ms)),
                    notes=(
                        f"top_k=10 results={len(last_results or [])} "
                        f"query_mode=unique flushed=true search_mode={search_mode or 'exact'} "
                        f"axis_wait_enabled={axis_wait['enabled']} axis_ready={axis_wait['ready']} "
                        f"axis_wait_ms={axis_wait['wait_ms']:.2f}"
                    ),
                )
            )

            native.close()
        finally:
            shutil.rmtree(temp_dir, ignore_errors=True)

    return results


def _make_graph_nodes(count: int) -> list[Any]:
    nodes = []
    for index in range(count):
        node_kind = "function" if index % 2 == 0 else "class"
        node = proximadb.GraphNode(
            f"node_{index}",
            labels=["symbol", node_kind],
            properties={
                "kind": node_kind,
                "qualified_name": f"pkg.module.symbol_{index}",
                "file_path": f"src/module_{index % 64}.py",
                "language": "python",
            },
        )
        nodes.append(node)
    return nodes


def _make_graph_edges(node_count: int) -> list[Any]:
    edges = []
    for index in range(node_count):
        next_index = (index + 1) % node_count
        edges.append(
            proximadb.GraphEdge(
                f"node_{index}",
                f"node_{next_index}",
                "CALLS",
                id=f"edge_{index}",
                weight=1.0,
                properties={"kind": "call", "count": str((index % 5) + 1)},
            )
        )
    return edges


def _benchmark_documents(
    db: Any,
    document_count: int,
) -> list[MetricResult]:
    results: list[MetricResult] = []
    try:
        db.create_document_collection(DOCUMENT_COLLECTION, config={})
    except TypeError:
        try:
            db.create_document_collection(DOCUMENT_COLLECTION)
        except Exception as exc:
            if "already exists" not in str(exc).lower():
                raise
    except Exception as exc:
        if "already exists" not in str(exc).lower():
            raise

    start = time.perf_counter()
    for index in range(document_count):
        document = {
            "path": f"src/module_{index % 64}.py",
            "language": "python",
            "kind": "source_file",
            "content": f"def symbol_{index}(): return {index}",
            "updated_at_ns": 1_700_000_000_000_000_000 + index,
        }
        try:
            db.insert_document(DOCUMENT_COLLECTION, document, id=f"doc_{index}")
        except TypeError:
            db.insert_document(DOCUMENT_COLLECTION, document, doc_id=f"doc_{index}")
    total_ms = (time.perf_counter() - start) * 1000.0
    results.append(
        MetricResult(
            name="documents_insert",
            count=document_count,
            total_ms=total_ms,
            throughput=document_count / (total_ms / 1000.0),
        )
    )

    latencies_ms, query_result = _run_samples(
        20,
        lambda: db.query_documents(
            DOCUMENT_COLLECTION, filter={"language": "python"}, limit=25
        ),
    )
    returned_docs = (
        len(query_result.get("documents", []))
        if isinstance(query_result, dict)
        else len(query_result or [])
    )
    results.append(
        MetricResult(
            name="documents_query",
            count=20,
            total_ms=float(sum(latencies_ms)),
            throughput=1000.0 / float(np.mean(latencies_ms)),
            avg_ms=float(np.mean(latencies_ms)),
            p50_ms=float(np.percentile(latencies_ms, 50)),
            p95_ms=float(np.percentile(latencies_ms, 95)),
            notes=f"returned={returned_docs}",
        )
    )
    return results


def _benchmark_graph(native: Any, node_count: int) -> list[MetricResult]:
    results: list[MetricResult] = []
    try:
        native.create_graph(GRAPH_COLLECTION, "orion")
    except Exception as exc:
        if "already exists" not in str(exc).lower():
            raise

    nodes = _make_graph_nodes(node_count)
    edges = _make_graph_edges(node_count)

    start = time.perf_counter()
    inserted_nodes = native.create_nodes(GRAPH_COLLECTION, nodes)
    total_ms = (time.perf_counter() - start) * 1000.0
    results.append(
        MetricResult(
            name="graph_insert_nodes",
            count=inserted_nodes,
            total_ms=total_ms,
            throughput=inserted_nodes / (total_ms / 1000.0),
        )
    )

    start = time.perf_counter()
    inserted_edges = native.create_edges(GRAPH_COLLECTION, edges)
    total_ms = (time.perf_counter() - start) * 1000.0
    results.append(
        MetricResult(
            name="graph_insert_edges",
            count=inserted_edges,
            total_ms=total_ms,
            throughput=inserted_edges / (total_ms / 1000.0),
        )
    )

    query_latencies_ms, query_result = _run_samples(
        20,
        lambda: native.query_nodes(
            GRAPH_COLLECTION,
            labels=["symbol"],
            properties={"language": "python"},
            limit=25,
            offset=0,
        ),
    )
    results.append(
        MetricResult(
            name="graph_query_nodes",
            count=20,
            total_ms=float(sum(query_latencies_ms)),
            throughput=1000.0 / float(np.mean(query_latencies_ms)),
            avg_ms=float(np.mean(query_latencies_ms)),
            p50_ms=float(np.percentile(query_latencies_ms, 50)),
            p95_ms=float(np.percentile(query_latencies_ms, 95)),
            notes=f"returned={len(query_result or [])}",
        )
    )

    traverse_latencies_ms, traversal = _run_samples(
        20,
        lambda: native.traverse_graph(
            GRAPH_COLLECTION,
            start_node_id="node_0",
            max_depth=2,
            edge_types=["CALLS"],
            limit=200,
        ),
    )
    results.append(
        MetricResult(
            name="graph_traverse",
            count=20,
            total_ms=float(sum(traverse_latencies_ms)),
            throughput=1000.0 / float(np.mean(traverse_latencies_ms)),
            avg_ms=float(np.mean(traverse_latencies_ms)),
            p50_ms=float(np.percentile(traverse_latencies_ms, 50)),
            p95_ms=float(np.percentile(traverse_latencies_ms, 95)),
            notes=f"nodes={len(traversal.get('nodes', []))}",
        )
    )
    return results


def _benchmark_sql_and_unified(db: Any, query_vector: np.ndarray) -> list[MetricResult]:
    results: list[MetricResult] = []
    vector_literal = _format_vector_literal(query_vector)
    vector_search_sql = (
        f"SELECT id, score FROM VECTOR_SEARCH('{VECTOR_COLLECTION}', "
        f"'{vector_literal}', 25) ORDER BY score DESC LIMIT 25"
    )

    sql_latencies_ms, sql_result = _run_samples(
        20,
        lambda: db.execute_sql(vector_search_sql, collection=VECTOR_COLLECTION),
    )
    results.append(
        MetricResult(
            name="sql_query",
            count=20,
            total_ms=float(sum(sql_latencies_ms)),
            throughput=1000.0 / float(np.mean(sql_latencies_ms)),
            avg_ms=float(np.mean(sql_latencies_ms)),
            p50_ms=float(np.percentile(sql_latencies_ms, 50)),
            p95_ms=float(np.percentile(sql_latencies_ms, 95)),
            notes=f"rows={sql_result.get('row_count', 0)}",
        )
    )

    unified_latencies_ms, unified_result = _run_samples(
        20,
        lambda: db.execute_unified_query(vector_search_sql),
    )
    results.append(
        MetricResult(
            name="unified_query",
            count=20,
            total_ms=float(sum(unified_latencies_ms)),
            throughput=1000.0 / float(np.mean(unified_latencies_ms)),
            avg_ms=float(np.mean(unified_latencies_ms)),
            p50_ms=float(np.percentile(unified_latencies_ms, 50)),
            p95_ms=float(np.percentile(unified_latencies_ms, 95)),
            notes=f"rows={len(unified_result or [])}",
        )
    )
    return results


def _flush_if_supported(target: Any) -> None:
    flush = getattr(target, "flush", None)
    if callable(flush):
        flush()


def _benchmark_observability(
    db: Any,
    log_count: int,
    trace_count: int,
) -> list[MetricResult]:
    results: list[MetricResult] = []
    try:
        db.create_observability_namespace(OBS_NAMESPACE, retention_days=7)
    except Exception as exc:
        if "already exists" not in str(exc).lower():
            raise

    base_ns = 1_700_000_000_000_000_000
    logs = [
        {
            "timestamp_ns": base_ns + index * 1_000,
            "severity": "ERROR" if index % 10 == 0 else "INFO",
            "message": f"log event {index}",
            "service": "victor-agent",
            "source": "codingagent",
            "fields": {"file_path": f"src/module_{index % 64}.py", "index": index},
        }
        for index in range(log_count)
    ]
    start = time.perf_counter()
    ingested_logs = db.ingest_logs(OBS_NAMESPACE, logs)
    total_ms = (time.perf_counter() - start) * 1000.0
    results.append(
        MetricResult(
            name="logs_ingest",
            count=ingested_logs,
            total_ms=total_ms,
            throughput=ingested_logs / (total_ms / 1000.0),
        )
    )

    log_latencies_ms, log_query_result = _run_samples(
        20,
        lambda: db.query_logs(
            OBS_NAMESPACE,
            start_time_ns=base_ns,
            end_time_ns=base_ns + log_count * 2_000,
            query="ERROR",
            limit=25,
        ),
    )
    results.append(
        MetricResult(
            name="logs_query",
            count=20,
            total_ms=float(sum(log_latencies_ms)),
            throughput=1000.0 / float(np.mean(log_latencies_ms)),
            avg_ms=float(np.mean(log_latencies_ms)),
            p50_ms=float(np.percentile(log_latencies_ms, 50)),
            p95_ms=float(np.percentile(log_latencies_ms, 95)),
            notes=f"returned={len(log_query_result or [])}",
        )
    )

    metrics = [
        {
            "metric_name": "rl_reward",
            "timestamp_ns": base_ns + index * 5_000,
            "value": float(index % 100) / 100.0,
            "labels": {"service": "victor-agent", "mode": "embedded"},
        }
        for index in range(log_count)
    ]
    start = time.perf_counter()
    ingested_metrics = db.ingest_metrics(OBS_NAMESPACE, metrics)
    total_ms = (time.perf_counter() - start) * 1000.0
    results.append(
        MetricResult(
            name="metrics_ingest",
            count=ingested_metrics,
            total_ms=total_ms,
            throughput=ingested_metrics / (total_ms / 1000.0),
        )
    )

    metric_latencies_ms, metric_result = _run_samples(
        20,
        lambda: db.aggregate_metrics(
            OBS_NAMESPACE,
            metric_name="rl_reward",
            aggregation="avg",
            step_seconds=60,
        ),
    )
    results.append(
        MetricResult(
            name="metrics_aggregate",
            count=20,
            total_ms=float(sum(metric_latencies_ms)),
            throughput=1000.0 / float(np.mean(metric_latencies_ms)),
            avg_ms=float(np.mean(metric_latencies_ms)),
            p50_ms=float(np.percentile(metric_latencies_ms, 50)),
            p95_ms=float(np.percentile(metric_latencies_ms, 95)),
            notes=f"points={len(metric_result or [])}",
        )
    )

    traces = []
    for index in range(trace_count):
        traces.append(
            {
                "trace_id": f"trace_{index // 3}",
                "span_id": f"span_{index}",
                "parent_span_id": f"span_{index - 1}" if index % 3 else None,
                "name": "tool.execute" if index % 2 == 0 else "retrieval.search",
                "kind": "INTERNAL",
                "service": "victor-agent",
                "start_time_ns": base_ns + index * 10_000,
                "end_time_ns": base_ns + index * 10_000 + 5_000,
                "status_code": "OK",
                "attributes": {
                    "workspace": "codingagent",
                    "file_path": f"src/module_{index % 64}.py",
                },
            }
        )

    start = time.perf_counter()
    ingested_traces = db.ingest_traces(OBS_NAMESPACE, traces)
    total_ms = (time.perf_counter() - start) * 1000.0
    results.append(
        MetricResult(
            name="traces_ingest",
            count=ingested_traces,
            total_ms=total_ms,
            throughput=ingested_traces / (total_ms / 1000.0),
        )
    )

    trace_latencies_ms, trace_query_result = _run_samples(
        20,
        lambda: db.query_traces(
            OBS_NAMESPACE,
            start_time_ns=base_ns,
            end_time_ns=base_ns + trace_count * 20_000,
            service="victor-agent",
            limit=25,
        ),
    )
    results.append(
        MetricResult(
            name="traces_query",
            count=20,
            total_ms=float(sum(trace_latencies_ms)),
            throughput=1000.0 / float(np.mean(trace_latencies_ms)),
            avg_ms=float(np.mean(trace_latencies_ms)),
            p50_ms=float(np.percentile(trace_latencies_ms, 50)),
            p95_ms=float(np.percentile(trace_latencies_ms, 95)),
            notes=f"returned={len(trace_query_result or [])}",
        )
    )

    trace_detail_latencies_ms, trace_detail_result = _run_samples(
        20,
        lambda: db.get_trace(OBS_NAMESPACE, "trace_0"),
    )
    results.append(
        MetricResult(
            name="trace_get",
            count=20,
            total_ms=float(sum(trace_detail_latencies_ms)),
            throughput=1000.0 / float(np.mean(trace_detail_latencies_ms)),
            avg_ms=float(np.mean(trace_detail_latencies_ms)),
            p50_ms=float(np.percentile(trace_detail_latencies_ms, 50)),
            p95_ms=float(np.percentile(trace_detail_latencies_ms, 95)),
            notes=f"spans={len(trace_detail_result.get('spans', []))}",
        )
    )

    return results


def benchmark_codingagent_shape(
    vector_count: int,
    batch_size: int,
    graph_nodes: int,
    document_count: int,
    log_count: int,
    trace_count: int,
    dimension: int,
    engine: str,
    search_mode: str | None,
    wait_for_axis_ready_s: float,
) -> dict[str, Any]:
    temp_dir = tempfile.mkdtemp(prefix="proximadb_victor_shape_")
    try:
        api_surface = "raw_native"
        client_like: Any = None
        native = None

        try:
            from proximadb_sdk.unified_client import ProximaDBClient

            client_like = ProximaDBClient(
                protocol="embedded",
                data_dir=temp_dir,
                default_engine=engine,
                cache_size_mb=1024,
            )
            native = getattr(client_like, "_client", None)
            if native is None:
                raise RuntimeError("Embedded client did not expose a native handle")
            api_surface = "sdk_embedded"
        except Exception as exc:
            native = proximadb.ProximaDB(
                data_dirs=temp_dir,
                default_engine=engine,
                cache_size_mb=1024,
                enable_wal=True,
            )
            client_like = native
            api_surface = f"raw_native_fallback:{exc.__class__.__name__}"

        native.create_collection(VECTOR_COLLECTION, dimension, engine)
        vector_insert, query_vector, query_vectors = _vector_insert_baseline(
            native_db=native,
            collection=VECTOR_COLLECTION,
            total_vectors=vector_count,
            batch_size=batch_size,
            dimension=dimension,
            seed=29,
            with_metadata=True,
        )
        _flush_if_supported(native)
        axis_wait = _wait_for_embedded_axis_ready(temp_dir, wait_for_axis_ready_s)

        vector_latencies_ms, vector_search_result = _run_query_samples(
            query_vectors,
            lambda query: (
                native.search_numpy(
                    VECTOR_COLLECTION,
                    query,
                    10,
                    None,
                    search_mode,
                )
                if hasattr(native, "search_numpy") and search_mode
                else (
                    native.search_numpy(VECTOR_COLLECTION, query, 10)
                    if hasattr(native, "search_numpy")
                    else native.search(VECTOR_COLLECTION, query.tolist(), 10)
                )
            ),
        )

        multimodel_results = [
            vector_insert,
            MetricResult(
                name="vectors_search",
                count=len(vector_latencies_ms),
                total_ms=float(sum(vector_latencies_ms)),
                throughput=1000.0 / float(np.mean(vector_latencies_ms)),
                avg_ms=float(np.mean(vector_latencies_ms)),
                p50_ms=float(np.percentile(vector_latencies_ms, 50)),
                p95_ms=float(np.percentile(vector_latencies_ms, 95)),
                max_ms=float(np.max(vector_latencies_ms)),
                notes=(
                    f"results={len(vector_search_result or [])} "
                    f"query_mode=unique flushed=true search_mode={search_mode or 'exact'} "
                    f"axis_wait_enabled={axis_wait['enabled']} axis_ready={axis_wait['ready']} "
                    f"axis_wait_ms={axis_wait['wait_ms']:.2f}"
                ),
            ),
        ]
        multimodel_results.extend(_benchmark_graph(native, graph_nodes))
        multimodel_results.extend(_benchmark_documents(client_like, document_count))
        _flush_if_supported(native)
        multimodel_results.extend(_benchmark_sql_and_unified(client_like, query_vector))
        multimodel_results.extend(
            _benchmark_observability(client_like, log_count, trace_count)
        )

        if hasattr(client_like, "close"):
            client_like.close()

        return {
            "api_surface": api_surface,
            "native_surface": f"{type(native).__module__}.{type(native).__name__}",
            "results": [asdict(item) for item in multimodel_results],
        }
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def _print_metric(metric: dict[str, Any]) -> None:
    avg = f" avg={metric['avg_ms']:.2f}ms" if metric.get("avg_ms") is not None else ""
    p50 = f" p50={metric['p50_ms']:.2f}ms" if metric.get("p50_ms") is not None else ""
    p95 = f" p95={metric['p95_ms']:.2f}ms" if metric.get("p95_ms") is not None else ""
    max_ms = (
        f" max={metric['max_ms']:.2f}ms" if metric.get("max_ms") is not None else ""
    )
    notes = f" notes={metric['notes']}" if metric.get("notes") else ""
    print(
        f"{metric['name']}: count={metric['count']} total={metric['total_ms']:.2f}ms "
        f"throughput={metric['throughput']:.2f}/s{avg}{p50}{p95}{max_ms}{notes}"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--vector-counts",
        nargs="+",
        type=int,
        default=[10_000, 100_000, 1_000_000],
        help="Vector scales for the native embedded baseline",
    )
    parser.add_argument(
        "--integrated-vector-count",
        type=int,
        default=10_000,
        help="Vector count for the multimodel codingagent-shaped benchmark",
    )
    parser.add_argument("--graph-nodes", type=int, default=2_000)
    parser.add_argument("--documents", type=int, default=2_000)
    parser.add_argument("--logs", type=int, default=5_000)
    parser.add_argument("--traces", type=int, default=3_000)
    parser.add_argument("--dimension", type=int, default=DIMENSION)
    parser.add_argument("--batch-size", type=int, default=10_000)
    parser.add_argument("--engine", default="sst")
    parser.add_argument(
        "--search-mode",
        default="adaptive",
        help="Vector search mode for embedded retrieval benchmarks (exact, approximate, adaptive)",
    )
    parser.add_argument(
        "--wait-for-axis-ready-seconds",
        type=float,
        default=0.0,
        help=(
            "Optional time budget to wait for embedded background AXIS indexing "
            "before measuring vector search latency"
        ),
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    native_results = [
        asdict(result)
        for result in benchmark_native_vector_surface(
            vector_counts=args.vector_counts,
            batch_size=args.batch_size,
            dimension=args.dimension,
            engine=args.engine,
            search_mode=args.search_mode,
            wait_for_axis_ready_s=args.wait_for_axis_ready_seconds,
        )
    ]
    multimodel = benchmark_codingagent_shape(
        vector_count=args.integrated_vector_count,
        batch_size=args.batch_size,
        graph_nodes=args.graph_nodes,
        document_count=args.documents,
        log_count=args.logs,
        trace_count=args.traces,
        dimension=args.dimension,
        engine=args.engine,
        search_mode=args.search_mode,
        wait_for_axis_ready_s=args.wait_for_axis_ready_seconds,
    )

    summary = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "repo_root": str(REPO_ROOT),
        "dimension": args.dimension,
        "engine": args.engine,
        "search_mode": args.search_mode,
        "wait_for_axis_ready_seconds": args.wait_for_axis_ready_seconds,
        "native_vector_baseline": native_results,
        "codingagent_shape": multimodel,
    }

    print("Native embedded vector baseline")
    for metric in native_results:
        _print_metric(metric)

    print("\nCodingagent multimodel shape")
    print(f"native_surface: {multimodel['native_surface']}")
    for metric in multimodel["results"]:
        _print_metric(metric)

    if args.output:
        args.output.write_text(json.dumps(summary, indent=2), encoding="utf-8")
        print(f"\nWrote {args.output}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
