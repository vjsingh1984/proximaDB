"""Cross-protocol write benchmarks against a running proximadb-server.

Mirrors the embedded sync/async write benchmark but routes through:
 - REST (HTTP POST /api/v1/vectors/batch)
 - pgwire (psycopg2 INSERT)
 - Arrow Flight (DoPut with Arrow IPC stream)

Goal: characterize wire-protocol overhead vs the embedded path, and
confirm whether the server-mediated path scales the same way under
batch-size pressure.

Run:
    python server_writes_multi_protocol.py --count 10000 --dimension 64 \
        --runs 3 --json-out artifacts/server_writes_10k.json
"""

from __future__ import annotations

import argparse
import asyncio
import json
import platform
import statistics
import time
import uuid
from pathlib import Path
from typing import Any, Callable

import numpy as np

DEFAULT_REST = "http://localhost:5678"
DEFAULT_PGWIRE = "host=localhost port=5433 user=postgres dbname=postgres"
DEFAULT_FLIGHT = "grpc+tcp://localhost:5680"


def timed(name: str, ops: int, fn: Callable[[], Any]) -> dict[str, Any]:
    t0 = time.perf_counter()
    res = fn()
    elapsed = time.perf_counter() - t0
    return {
        "name": name,
        "operations": ops,
        "seconds": elapsed,
        "ops_per_second": ops / elapsed if elapsed > 0 else None,
        "result": res,
    }


# ---------------------------------------------------------------------------
# REST
# ---------------------------------------------------------------------------


def make_rest_session(base_url: str):
    import requests

    s = requests.Session()
    s.headers["Content-Type"] = "application/json"
    s.base_url = base_url  # type: ignore[attr-defined]
    return s


def rest_create_collection(s, name: str, dim: int) -> None:
    body = {
        "operation": 1,
        "collection_id": name,
        "collection_config": {
            "name": name,
            "dimension": dim,
            "distance_metric": 1,
            "storage_engine": 0,
        },
    }
    r = s.post(f"{s.base_url}/api/v1/collections", json=body, timeout=30)
    r.raise_for_status()


def rest_vector_batch(s, collection: str, ids: list[str], vectors: np.ndarray) -> int:
    body = {
        "collection_id": collection,
        "vectors": [
            {"id": ids[i], "vector": vectors[i].tolist(), "metadata": {}}
            for i in range(len(ids))
        ],
    }
    r = s.post(f"{s.base_url}/api/v1/vectors/batch", json=body, timeout=120)
    r.raise_for_status()
    return len(ids)


def run_rest(base_url: str, count: int, dim: int) -> list[dict[str, Any]]:
    s = make_rest_session(base_url)
    name = f"bench_rest_vec_{uuid.uuid4().hex[:8]}"
    rest_create_collection(s, name, dim)

    rng = np.random.default_rng(7)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"r-{i}" for i in range(count)]

    sync_one = timed(
        "rest.vector_batch.sync_one_call",
        count,
        lambda: rest_vector_batch(s, name, ids, vectors),
    )

    # Chunked sync (server-side batches of 1000 to mirror client-side chunking)
    chunk = 1000
    name2 = f"bench_rest_vec2_{uuid.uuid4().hex[:8]}"
    rest_create_collection(s, name2, dim)

    def chunked_call() -> int:
        total = 0
        for start in range(0, count, chunk):
            end = min(count, start + chunk)
            total += rest_vector_batch(s, name2, ids[start:end], vectors[start:end])
        return total

    sync_chunked = timed(
        f"rest.vector_batch.sync_chunked_{chunk}",
        count,
        chunked_call,
    )
    return [sync_one, sync_chunked]


# ---------------------------------------------------------------------------
# Arrow Flight
# ---------------------------------------------------------------------------


def run_arrow_flight(uri: str, count: int, dim: int) -> list[dict[str, Any]]:
    try:
        import pyarrow as pa
        import pyarrow.flight as flight
    except ImportError:
        return [{"name": "flight.vector_batch", "skipped": "pyarrow.flight missing"}]

    name = f"bench_flight_vec_{uuid.uuid4().hex[:8]}"
    # Create collection via REST (Flight has no DDL surface in this build).
    s = make_rest_session(DEFAULT_REST)
    rest_create_collection(s, name, dim)

    rng = np.random.default_rng(11)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"f-{i}" for i in range(count)]

    table = pa.table(
        {
            "id": ids,
            "vector": pa.array(vectors.tolist(), type=pa.list_(pa.float32())),
        }
    )

    client = flight.FlightClient(uri)
    descriptor = flight.FlightDescriptor.for_path(name.encode("utf-8"))

    def do_put_call() -> int:
        writer, _meta_reader = client.do_put(descriptor, table.schema)
        try:
            writer.write_table(table)
        finally:
            writer.close()
        return count

    return [timed("flight.do_put.one_call", count, do_put_call)]


# ---------------------------------------------------------------------------
# pgwire
# ---------------------------------------------------------------------------


def run_pgwire(conninfo: str, count: int, dim: int) -> list[dict[str, Any]]:
    try:
        import psycopg2
    except ImportError:
        return [{"name": "pgwire.sql_insert", "skipped": "psycopg2 missing"}]

    try:
        conn = psycopg2.connect(conninfo)
    except Exception as e:
        return [{"name": "pgwire.sql_insert", "skipped": f"connect: {e}"}]
    conn.autocommit = True
    cur = conn.cursor()
    table = f"bench_pg_vec_{uuid.uuid4().hex[:8]}"
    try:
        cur.execute(
            f"""
            CREATE TABLE {table} (
                id TEXT NOT NULL,
                embedding VECTOR({dim}),
                PRIMARY KEY (id)
            ) WITH (storage_engine='SST', layout='hybrid', schema_kind='relational_entity');
            """
        )
    except Exception as e:
        return [{"name": "pgwire.sql_insert", "skipped": f"create: {e}"}]

    vector_literal = "[" + ",".join(["0.1"] * dim) + "]"

    def insert_chunked(chunk: int) -> int:
        for start in range(0, count, chunk):
            end = min(count, start + chunk)
            values = ",".join(
                f"('p-{i}', '{vector_literal}')" for i in range(start, end)
            )
            cur.execute(f"INSERT INTO {table} (id, embedding) VALUES {values};")
        return count

    sync_chunked = timed(
        "pgwire.sql_insert.chunked_1000",
        count,
        lambda: insert_chunked(1000),
    )
    cur.close()
    conn.close()
    return [sync_chunked]


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def run(count: int, dim: int) -> dict[str, Any]:
    results = []
    for name, fn in (
        ("rest", lambda: run_rest(DEFAULT_REST, count, dim)),
        ("pgwire", lambda: run_pgwire(DEFAULT_PGWIRE, count, dim)),
        ("flight", lambda: run_arrow_flight(DEFAULT_FLIGHT, count, dim)),
    ):
        try:
            results.extend(fn())
        except Exception as e:
            results.append({"name": f"{name}.error", "error": f"{type(e).__name__}: {e}"})
    return {
        "benchmark": "server_writes_multi_protocol",
        "rows": count,
        "dimension": dim,
        "python": platform.python_version(),
        "platform": platform.platform(),
        "results": results,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=10000)
    parser.add_argument("--dimension", type=int, default=64)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--json-out", type=Path, default=None)
    args = parser.parse_args()

    reports = [run(args.count, args.dimension) for _ in range(args.runs)]
    by_name: dict[str, list[float]] = {}
    for rep in reports:
        for r in rep["results"]:
            ops = r.get("ops_per_second")
            if ops is not None:
                by_name.setdefault(r["name"], []).append(ops)
    agg = [
        {
            "name": name,
            "runs": len(values),
            "ops_per_second_min": min(values),
            "ops_per_second_median": statistics.median(values),
            "ops_per_second_max": max(values),
        }
        for name, values in sorted(by_name.items())
    ]
    payload = {
        "benchmark": "server_writes_multi_protocol",
        "rows": args.count,
        "dimension": args.dimension,
        "runs": args.runs,
        "aggregate_results": agg,
        "reports": reports,
    }
    out = json.dumps(payload, indent=2, sort_keys=True)
    print(out)
    if args.json_out:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        args.json_out.write_text(out + "\n")


if __name__ == "__main__":
    main()
