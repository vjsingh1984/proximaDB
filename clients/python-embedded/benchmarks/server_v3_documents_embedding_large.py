"""v3 documents bench for the bge-LARGE (1024-dim) variant.

Same shape as `server_v3_documents_embedding.py` but creates the
collection with `dimension=1024` to match the BGE-large output. The
server must be started with `PROXIMADB_EMBED_VARIANT=large` so the
embedding service loads `bge-large-en-v1.5.onnx` from
`PROXIMADB_EMBED_MODEL_DIR`.
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

REST_URL = "http://localhost:5678"

TEXTS = [
    "the quick brown fox jumps over the lazy dog repeatedly",
    "embedding models project text into a dense vector space",
    "vector databases support similarity search using cosine distance",
    "ProximaDB unifies vector relational document graph and observability storage",
    "asyncio overlap is most useful when per-request work is significant",
    "tokio runtime block_on serializes work from multiple OS threads",
    "the write-ahead log guarantees durability before storage layer commit",
    "PAX columnar layouts trade off scan throughput against random access",
    "schema validation runs at the boundary not after storage commit",
    "arrow flight do_put streams columnar batches over HTTP/2",
]


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


def create_collection(name: str, dim: int = 1024) -> None:
    import requests

    body = {
        "name": name,
        "dimension": dim,
        "distance_metric": "cosine",
        "storage_engine": "sst",
    }
    r = requests.post(f"{REST_URL}/api/v2/collections", json=body, timeout=30)
    r.raise_for_status()


def make_records(count: int) -> list[dict]:
    return [
        {
            "id": f"d-{i}",
            "text": TEXTS[i % len(TEXTS)] + f" — doc number {i}",
            "metadata": {"tenant": "embedded", "seq": i},
        }
        for i in range(count)
    ]


def v3_sync_one_at_a_time(count: int) -> dict[str, Any]:
    import requests

    name = f"v3l_sync1_{uuid.uuid4().hex[:8]}"
    create_collection(name)
    records = make_records(count)
    s = requests.Session()
    url = f"{REST_URL}/api/v3/collections/{name}/documents"

    def call() -> None:
        for r in records:
            resp = s.post(url, json={"records": [r]}, timeout=60)
            resp.raise_for_status()

    return timed_sync("v3.embed.sync_one_per_call", count, call)


def v3_sync_one_big_call(count: int) -> dict[str, Any]:
    import requests

    name = f"v3l_sync_big_{uuid.uuid4().hex[:8]}"
    create_collection(name)
    records = make_records(count)
    s = requests.Session()
    url = f"{REST_URL}/api/v3/collections/{name}/documents"

    def call() -> None:
        resp = s.post(url, json={"records": records}, timeout=300)
        resp.raise_for_status()

    return timed_sync("v3.embed.sync_one_big_call", count, call)


async def v3_async_one_per_call(count: int, concurrency: int) -> dict[str, Any]:
    import aiohttp

    name = f"v3l_async1_{uuid.uuid4().hex[:8]}"
    create_collection(name)
    records = make_records(count)
    url = f"{REST_URL}/api/v3/collections/{name}/documents"

    sem = asyncio.Semaphore(concurrency)

    async with aiohttp.ClientSession() as session:

        async def post_one(rec: dict) -> None:
            async with sem:
                async with session.post(
                    url, json={"records": [rec]}, timeout=aiohttp.ClientTimeout(total=60)
                ) as r:
                    r.raise_for_status()
                    await r.read()

        async def factory() -> None:
            await asyncio.gather(*(post_one(r) for r in records))

        return await timed_async(
            f"v3.embed.async_one_per_call_conc{concurrency}", count, factory
        )


async def v3_async_chunked(count: int, concurrency: int) -> dict[str, Any]:
    import aiohttp

    name = f"v3l_async_chunk_{uuid.uuid4().hex[:8]}"
    create_collection(name)
    records = make_records(count)
    url = f"{REST_URL}/api/v3/collections/{name}/documents"

    async with aiohttp.ClientSession() as session:

        async def post_chunk(start: int, end: int) -> None:
            async with session.post(
                url,
                json={"records": records[start:end]},
                timeout=aiohttp.ClientTimeout(total=300),
            ) as r:
                r.raise_for_status()
                await r.read()

        async def factory() -> None:
            await asyncio.gather(*(post_chunk(s_, e_) for s_, e_ in chunks(count, concurrency)))

        return await timed_async(f"v3.embed.async_chunked_conc{concurrency}", count, factory)


async def run_all(count: int, concurrency: int) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    for fn in (lambda: v3_sync_one_at_a_time(count), lambda: v3_sync_one_big_call(count)):
        try:
            results.append(fn())
        except Exception as e:
            results.append({"name": "sync.error", "error": f"{type(e).__name__}: {e}"})
    for coro in (
        v3_async_one_per_call(count, concurrency),
        v3_async_chunked(count, concurrency),
    ):
        try:
            results.append(await coro)
        except Exception as e:
            results.append({"name": "async.error", "error": f"{type(e).__name__}: {e}"})
    return results


def run_once(count: int, concurrency: int) -> dict[str, Any]:
    return {
        "benchmark": "server_v3_documents_embedding_large",
        "rows": count,
        "concurrency": concurrency,
        "python": platform.python_version(),
        "platform": platform.platform(),
        "results": asyncio.run(run_all(count, concurrency)),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=100)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--json-out", type=Path, default=None)
    args = parser.parse_args()

    reports = [run_once(args.count, args.concurrency) for _ in range(args.runs)]
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
        "benchmark": "server_v3_documents_embedding_large",
        "rows": args.count,
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
