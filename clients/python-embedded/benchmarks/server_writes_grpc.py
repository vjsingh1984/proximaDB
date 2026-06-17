"""gRPC v1 + v2 write benchmark, sync and async, against a running server.

v1: VectorService.VectorBatch (legacy proto path)
v2: ProximaRecordService.InsertRecords (canonical ProximaRecord path)

Sync uses the blocking grpcio stub; async uses grpc.aio with the same
ProtoBuf classes. Async fan-out is N concurrent calls via asyncio.gather.

Run:
    python server_writes_grpc.py --count 10000 --dimension 64 --runs 3
"""

from __future__ import annotations

import argparse
import asyncio
import json
import platform
import statistics
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python" / "src"))

import grpc
import grpc.aio
import numpy as np
import requests

from proximadb.v1 import vector_pb2_grpc, vector_types_pb2
from proximadb.v2 import record_pb2, record_pb2_grpc

REST_URL = "http://localhost:5678"
GRPC_TARGET = "localhost:5679"


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
# Collection setup helpers via REST
# ---------------------------------------------------------------------------


def create_v1_collection(name: str, dim: int) -> None:
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
    r = requests.post(f"{REST_URL}/api/v1/collections", json=body, timeout=30)
    r.raise_for_status()


def create_v2_collection(name: str, dim: int) -> None:
    body = {
        "name": name,
        "dimension": dim,
        "distance_metric": "cosine",
        "storage_engine": "sst",
    }
    r = requests.post(f"{REST_URL}/api/v2/collections", json=body, timeout=30)
    r.raise_for_status()


# ---------------------------------------------------------------------------
# v1 — VectorBatch
# ---------------------------------------------------------------------------


def build_v1_records(count: int, dim: int, vectors: np.ndarray) -> list:
    return [
        vector_types_pb2.VectorRecord(id=f"v1-{i}", vector=vectors[i].tolist())
        for i in range(count)
    ]


def grpc_v1_sync(count: int, dim: int) -> dict[str, Any]:
    name = f"bench_grpc_v1_{uuid.uuid4().hex[:8]}"
    create_v1_collection(name, dim)
    rng = np.random.default_rng(101)
    vectors = rng.random((count, dim), dtype=np.float32)
    records = build_v1_records(count, dim, vectors)
    request = vector_types_pb2.VectorBatchRequest(collection_id=name, vectors=records)

    channel = grpc.insecure_channel(
        GRPC_TARGET,
        options=[
            ("grpc.max_send_message_length", 128 * 1024 * 1024),
            ("grpc.max_receive_message_length", 128 * 1024 * 1024),
        ],
    )
    stub = vector_pb2_grpc.VectorServiceStub(channel)

    def call() -> None:
        resp = stub.VectorBatch(request, timeout=180)
        if not resp.success:
            raise RuntimeError(f"v1 VectorBatch failed: {resp.error_message}")

    return timed_sync("grpc_v1.vector_batch.sync", count, call)


async def grpc_v1_async(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    name = f"bench_grpc_v1a_{uuid.uuid4().hex[:8]}"
    create_v1_collection(name, dim)
    rng = np.random.default_rng(102)
    vectors = rng.random((count, dim), dtype=np.float32)

    async with grpc.aio.insecure_channel(
        GRPC_TARGET,
        options=[
            ("grpc.max_send_message_length", 128 * 1024 * 1024),
            ("grpc.max_receive_message_length", 128 * 1024 * 1024),
        ],
    ) as channel:
        stub = vector_pb2_grpc.VectorServiceStub(channel)

        async def post_chunk(start: int, end: int) -> int:
            sub = [
                vector_types_pb2.VectorRecord(id=f"v1-{i}", vector=vectors[i].tolist())
                for i in range(start, end)
            ]
            req = vector_types_pb2.VectorBatchRequest(collection_id=name, vectors=sub)
            resp = await stub.VectorBatch(req, timeout=180)
            if not resp.success:
                raise RuntimeError(f"v1 async VectorBatch: {resp.error_message}")
            return end - start

        async def factory() -> None:
            tasks = [post_chunk(s_, e_) for s_, e_ in chunks(count, concurrency)]
            await asyncio.gather(*tasks)

        return await timed_async("grpc_v1.vector_batch.async", count, factory)


# ---------------------------------------------------------------------------
# v2 — InsertRecords
# ---------------------------------------------------------------------------


def build_v2_records(count: int, vectors: np.ndarray, dim: int) -> list:
    return [
        record_pb2.ProximaRecord(id=f"v2-{i}", vector=vectors[i].tolist(), vector_dimension=dim)
        for i in range(count)
    ]


def grpc_v2_sync(count: int, dim: int) -> dict[str, Any]:
    name = f"bench_grpc_v2_{uuid.uuid4().hex[:8]}"
    create_v2_collection(name, dim)
    rng = np.random.default_rng(201)
    vectors = rng.random((count, dim), dtype=np.float32)
    records = build_v2_records(count, vectors, dim)
    request = record_pb2.ProximaRecordBatch(collection_id=name, records=records)

    channel = grpc.insecure_channel(
        GRPC_TARGET,
        options=[
            ("grpc.max_send_message_length", 128 * 1024 * 1024),
            ("grpc.max_receive_message_length", 128 * 1024 * 1024),
        ],
    )
    stub = record_pb2_grpc.ProximaRecordServiceStub(channel)

    def call() -> None:
        resp = stub.InsertRecords(request, timeout=180)
        if not resp.success:
            raise RuntimeError(f"v2 InsertRecords failed: {getattr(resp, 'error_message', '')}")

    return timed_sync("grpc_v2.insert_records.sync", count, call)


async def grpc_v2_async(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    name = f"bench_grpc_v2a_{uuid.uuid4().hex[:8]}"
    create_v2_collection(name, dim)
    rng = np.random.default_rng(202)
    vectors = rng.random((count, dim), dtype=np.float32)

    async with grpc.aio.insecure_channel(
        GRPC_TARGET,
        options=[
            ("grpc.max_send_message_length", 128 * 1024 * 1024),
            ("grpc.max_receive_message_length", 128 * 1024 * 1024),
        ],
    ) as channel:
        stub = record_pb2_grpc.ProximaRecordServiceStub(channel)

        async def post_chunk(start: int, end: int) -> int:
            sub = [
                record_pb2.ProximaRecord(
                    id=f"v2-{i}", vector=vectors[i].tolist(), vector_dimension=dim
                )
                for i in range(start, end)
            ]
            req = record_pb2.ProximaRecordBatch(collection_id=name, records=sub)
            resp = await stub.InsertRecords(req, timeout=180)
            if not resp.success:
                raise RuntimeError(
                    f"v2 async InsertRecords: {getattr(resp, 'error_message', '')}"
                )
            return end - start

        async def factory() -> None:
            tasks = [post_chunk(s_, e_) for s_, e_ in chunks(count, concurrency)]
            await asyncio.gather(*tasks)

        return await timed_async("grpc_v2.insert_records.async", count, factory)


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


async def run_all(count: int, dim: int, concurrency: int) -> list[dict[str, Any]]:
    results = []
    for fn in (lambda: grpc_v1_sync(count, dim), lambda: grpc_v2_sync(count, dim)):
        try:
            results.append(fn())
        except Exception as e:
            results.append({"name": "sync.error", "error": f"{type(e).__name__}: {e}"})
    for coro in (
        grpc_v1_async(count, dim, concurrency),
        grpc_v2_async(count, dim, concurrency),
    ):
        try:
            results.append(await coro)
        except Exception as e:
            results.append({"name": "async.error", "error": f"{type(e).__name__}: {e}"})
    return results


def run_once(count: int, dim: int, concurrency: int) -> dict[str, Any]:
    return {
        "benchmark": "server_writes_grpc",
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
        "benchmark": "server_writes_grpc",
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
