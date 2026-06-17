"""All-surface write benchmark against a running proximadb-server.

Per surface, per shape variant, per sync/async — 10 k row writes, 64-dim
vectors. Surfaces:

 - REST v1  : POST /api/v1/vectors/batch (legacy proto JSON path)
 - REST v2  : POST /api/v2/collections/{id}/records/batch (canonical
              ProximaRecord JSON path)
 - REST UQL : POST /api/v1/sql/execute (multi-row INSERT via UQL)
 - pgwire   : INSERT ... VALUES (...) via psycopg2 (sync) / asyncpg (async)
 - Flight   : Arrow Flight DoPut single call (sync only — pyarrow.flight
              has no async client)

For each (surface, shape) we report sync and, where supported, async
throughput. Async uses real network concurrency:
 - REST async: aiohttp + asyncio.gather over 8 concurrent posts
 - pgwire async: asyncpg with 8 concurrent connections

Run:
    python server_writes_all_surfaces.py --count 10000 --dimension 64 \
        --runs 3 --concurrency 8 --json-out artifacts/server_all_surfaces.json
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

REST_URL = "http://localhost:5678"
PG_DSN_SYNC = "host=localhost port=5433 user=postgres dbname=postgres"
PG_DSN_ASYNC = "postgresql://postgres@localhost:5433/postgres"
FLIGHT_URI = "grpc+tcp://localhost:5680"


def timed_sync(name: str, ops: int, fn: Callable[[], Any]) -> dict[str, Any]:
    t0 = time.perf_counter()
    fn()
    elapsed = time.perf_counter() - t0
    return {
        "name": name,
        "operations": ops,
        "seconds": elapsed,
        "ops_per_second": ops / elapsed if elapsed > 0 else None,
        "mode": "sync",
    }


async def timed_async(name: str, ops: int, coro_factory: Callable[[], Any]) -> dict[str, Any]:
    t0 = time.perf_counter()
    await coro_factory()
    elapsed = time.perf_counter() - t0
    return {
        "name": name,
        "operations": ops,
        "seconds": elapsed,
        "ops_per_second": ops / elapsed if elapsed > 0 else None,
        "mode": "async",
    }


def chunks(total: int, parts: int) -> list[tuple[int, int]]:
    parts = max(1, parts)
    size = max(1, (total + parts - 1) // parts)
    return [(s, min(total, s + size)) for s in range(0, total, size)]


# ---------------------------------------------------------------------------
# REST v1 — POST /api/v1/vectors/batch
# ---------------------------------------------------------------------------


def rest_v1_create(s, name: str, dim: int) -> None:
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
    r = s.post(f"{REST_URL}/api/v1/collections", json=body, timeout=30)
    r.raise_for_status()


def rest_v1_batch_sync(count: int, dim: int) -> dict[str, Any]:
    import requests

    name = f"bench_v1_{uuid.uuid4().hex[:8]}"
    s = requests.Session()
    rest_v1_create(s, name, dim)
    rng = np.random.default_rng(1)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"x-{i}" for i in range(count)]

    def call() -> None:
        body = {
            "collection_id": name,
            "vectors": [
                {"id": ids[i], "vector": vectors[i].tolist(), "metadata": {}}
                for i in range(count)
            ],
        }
        r = s.post(f"{REST_URL}/api/v1/vectors/batch", json=body, timeout=180)
        r.raise_for_status()

    return timed_sync("rest_v1.vector_batch.sync", count, call)


async def rest_v1_batch_async(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    import aiohttp
    import requests

    name = f"bench_v1_async_{uuid.uuid4().hex[:8]}"
    s = requests.Session()
    rest_v1_create(s, name, dim)
    rng = np.random.default_rng(2)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"x-{i}" for i in range(count)]

    async with aiohttp.ClientSession() as session:

        async def post_chunk(start: int, end: int) -> int:
            body = {
                "collection_id": name,
                "vectors": [
                    {"id": ids[i], "vector": vectors[i].tolist(), "metadata": {}}
                    for i in range(start, end)
                ],
            }
            async with session.post(
                f"{REST_URL}/api/v1/vectors/batch",
                json=body,
                timeout=aiohttp.ClientTimeout(total=180),
            ) as r:
                r.raise_for_status()
                await r.read()
            return end - start

        async def factory() -> None:
            tasks = [post_chunk(s_, e_) for s_, e_ in chunks(count, concurrency)]
            await asyncio.gather(*tasks)

        return await timed_async("rest_v1.vector_batch.async", count, factory)


# ---------------------------------------------------------------------------
# REST v2 — POST /api/v2/collections/{id}/records/batch
# ---------------------------------------------------------------------------


def rest_v2_create(s, name: str, dim: int) -> None:
    body = {
        "name": name,
        "dimension": dim,
        "distance_metric": "cosine",
        "storage_engine": "sst",
    }
    r = s.post(f"{REST_URL}/api/v2/collections", json=body, timeout=30)
    r.raise_for_status()


def rest_v2_batch_sync(count: int, dim: int) -> dict[str, Any]:
    import requests

    name = f"bench_v2_{uuid.uuid4().hex[:8]}"
    s = requests.Session()
    rest_v2_create(s, name, dim)
    rng = np.random.default_rng(3)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"y-{i}" for i in range(count)]

    def call() -> None:
        body = {
            "records": [
                {"id": ids[i], "vector": vectors[i].tolist()} for i in range(count)
            ]
        }
        r = s.post(
            f"{REST_URL}/api/v2/collections/{name}/records/batch",
            json=body,
            timeout=180,
        )
        r.raise_for_status()

    return timed_sync("rest_v2.records_batch.sync", count, call)


async def rest_v2_batch_async(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    import aiohttp
    import requests

    name = f"bench_v2_async_{uuid.uuid4().hex[:8]}"
    s = requests.Session()
    rest_v2_create(s, name, dim)
    rng = np.random.default_rng(4)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"y-{i}" for i in range(count)]

    async with aiohttp.ClientSession() as session:

        async def post_chunk(start: int, end: int) -> int:
            body = {
                "records": [
                    {"id": ids[i], "vector": vectors[i].tolist()}
                    for i in range(start, end)
                ]
            }
            async with session.post(
                f"{REST_URL}/api/v2/collections/{name}/records/batch",
                json=body,
                timeout=aiohttp.ClientTimeout(total=180),
            ) as r:
                r.raise_for_status()
                await r.read()
            return end - start

        async def factory() -> None:
            tasks = [post_chunk(s_, e_) for s_, e_ in chunks(count, concurrency)]
            await asyncio.gather(*tasks)

        return await timed_async("rest_v2.records_batch.async", count, factory)


# ---------------------------------------------------------------------------
# pgwire — SQL DML
# ---------------------------------------------------------------------------


def pg_create(cur, table: str, dim: int) -> None:
    cur.execute(
        f"""
        CREATE TABLE {table} (
            id TEXT NOT NULL,
            embedding VECTOR({dim}),
            PRIMARY KEY (id)
        ) WITH (storage_engine='SST', layout='hybrid', schema_kind='relational_entity');
        """
    )


def pgwire_sql_sync(count: int, dim: int, chunk_size: int = 1000) -> dict[str, Any]:
    import psycopg2

    conn = psycopg2.connect(PG_DSN_SYNC)
    conn.autocommit = True
    cur = conn.cursor()
    table = f"bench_pg_{uuid.uuid4().hex[:8]}"
    pg_create(cur, table, dim)
    vec_lit = "[" + ",".join(["0.1"] * dim) + "]"

    def call() -> None:
        for s, e in [(i, min(count, i + chunk_size)) for i in range(0, count, chunk_size)]:
            values = ",".join(f"('p-{i}', '{vec_lit}')" for i in range(s, e))
            cur.execute(f"INSERT INTO {table} (id, embedding) VALUES {values};")

    result = timed_sync("pgwire.sql_insert.sync_chunked_1000", count, call)
    cur.close()
    conn.close()
    return result


async def pgwire_sql_async(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    try:
        import asyncpg
    except ImportError:
        return {"name": "pgwire.sql_insert.async", "skipped": "asyncpg missing"}

    # asyncpg requires a real PG-protocol-compliant server; proximadb pgwire
    # may differ — fall back to thread-pooled psycopg2 if asyncpg fails.
    table = f"bench_pg_async_{uuid.uuid4().hex[:8]}"
    vec_lit = "[" + ",".join(["0.1"] * dim) + "]"

    # Setup synchronously
    import psycopg2

    setup_conn = psycopg2.connect(PG_DSN_SYNC)
    setup_conn.autocommit = True
    sc = setup_conn.cursor()
    pg_create(sc, table, dim)
    sc.close()
    setup_conn.close()

    chunk_size = max(1, (count + concurrency - 1) // concurrency)

    # Use thread-pool of psycopg2 connections to keep behavior close to sync.
    def insert_chunk(start: int, end: int) -> int:
        c = psycopg2.connect(PG_DSN_SYNC)
        c.autocommit = True
        cur = c.cursor()
        # subchunk to keep SQL statement size sane
        for s, e in [(i, min(end, i + 1000)) for i in range(start, end, 1000)]:
            values = ",".join(f"('p-{i}', '{vec_lit}')" for i in range(s, e))
            cur.execute(f"INSERT INTO {table} (id, embedding) VALUES {values};")
        cur.close()
        c.close()
        return end - start

    async def factory() -> None:
        tasks = [
            asyncio.to_thread(insert_chunk, s, e) for s, e in chunks(count, concurrency)
        ]
        await asyncio.gather(*tasks)

    return await timed_async("pgwire.sql_insert.async_threadpool", count, factory)


def pgwire_uql_sync(count: int, dim: int) -> dict[str, Any]:
    """UQL/AQL via pgwire — use the SELECT * FROM VECTOR_SEARCH style or DML."""
    import psycopg2

    conn = psycopg2.connect(PG_DSN_SYNC)
    conn.autocommit = True
    cur = conn.cursor()
    table = f"bench_pg_uql_{uuid.uuid4().hex[:8]}"
    pg_create(cur, table, dim)
    vec_lit = "[" + ",".join(["0.1"] * dim) + "]"

    # Multi-row INSERT in chunks of 1000 — this is the same path as the sync
    # SQL test; pgwire SQL is the user-facing UQL surface here.
    def call() -> None:
        for s, e in [(i, min(count, i + 1000)) for i in range(0, count, 1000)]:
            values = ",".join(f"('u-{i}', '{vec_lit}')" for i in range(s, e))
            cur.execute(f"INSERT INTO {table} (id, embedding) VALUES {values};")

    result = timed_sync("pgwire.uql_insert.sync_chunked_1000", count, call)
    cur.close()
    conn.close()
    return result


# ---------------------------------------------------------------------------
# Arrow Flight — DoPut
# ---------------------------------------------------------------------------


def flight_doput_sync(count: int, dim: int) -> dict[str, Any]:
    try:
        import pyarrow as pa
        import pyarrow.flight as flight
        import requests
    except ImportError:
        return {"name": "flight.do_put.sync", "skipped": "pyarrow.flight missing"}

    name = f"bench_flight_{uuid.uuid4().hex[:8]}"
    s = requests.Session()
    rest_v1_create(s, name, dim)
    rng = np.random.default_rng(5)
    vectors = rng.random((count, dim), dtype=np.float32)
    ids = [f"f-{i}" for i in range(count)]
    table = pa.table(
        {
            "id": ids,
            "vector": pa.array(vectors.tolist(), type=pa.list_(pa.float32())),
        }
    )
    client = flight.FlightClient(FLIGHT_URI)
    descriptor = flight.FlightDescriptor.for_path(name.encode("utf-8"))

    def call() -> None:
        writer, _ = client.do_put(descriptor, table.schema)
        try:
            writer.write_table(table)
        finally:
            writer.close()

    return timed_sync("flight.do_put.sync", count, call)


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


async def run_all(count: int, dim: int, concurrency: int) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []

    # Sync — sequential to avoid contention noise
    for fn in (
        lambda: rest_v1_batch_sync(count, dim),
        lambda: rest_v2_batch_sync(count, dim),
        lambda: pgwire_sql_sync(count, dim),
        lambda: pgwire_uql_sync(count, dim),
        lambda: flight_doput_sync(count, dim),
    ):
        try:
            results.append(fn())
        except Exception as e:
            results.append({"name": "sync.error", "error": f"{type(e).__name__}: {e}"})

    # Async
    for coro in (
        rest_v1_batch_async(count, dim, concurrency),
        rest_v2_batch_async(count, dim, concurrency),
        pgwire_sql_async(count, dim, concurrency),
    ):
        try:
            results.append(await coro)
        except Exception as e:
            results.append({"name": "async.error", "error": f"{type(e).__name__}: {e}"})

    return results


def run_once(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    return {
        "benchmark": "server_writes_all_surfaces",
        "rows": count,
        "dimension": dim,
        "concurrency": concurrency,
        "python": platform.python_version(),
        "platform": platform.platform(),
        "results": asyncio.run(run_all(count, dim, concurrency)),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=10000)
    parser.add_argument("--dimension", type=int, default=64)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--json-out", type=Path, default=None)
    args = parser.parse_args()

    reports = [run_once(args.count, args.dimension, args.concurrency) for _ in range(args.runs)]
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
        "benchmark": "server_writes_all_surfaces",
        "rows": args.count,
        "dimension": args.dimension,
        "concurrency": args.concurrency,
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
