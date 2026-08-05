#!/usr/bin/env python3
"""Run a context-corridor-v1 benchmark adapter.

The runner is intentionally publication-conservative. It preserves failed runs,
emits every metric in the scenario contract, and never marks a single local run
as publication eligible. Dataset generation is streaming so the declared 1M
record scale does not require materializing the corpus in Python memory.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import random
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

try:
    import resource
except ImportError:  # pragma: no cover - exercised on Windows runners.
    resource = None  # type: ignore[assignment]

SCENARIO_ID = "context-corridor-v1"
TOP_K = 10
REQUIRED_METRICS = (
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
)


def request(
    base: str, method: str, path: str, body: dict | None, timeout: float
) -> tuple[Any, float]:
    headers = {"Accept": "application/json"}
    payload = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        payload = json.dumps(body, separators=(",", ":")).encode()
    req = urllib.request.Request(
        base.rstrip("/") + path,
        data=payload,
        headers=headers,
        method=method,
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            status = response.status
            raw = response.read()
    except urllib.error.HTTPError as exc:
        raise RuntimeError(
            f"{method} {path}: HTTP {exc.code}: {exc.read()[:1000]!r}"
        ) from exc
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
    values = (
        payload.get("matches") or payload.get("results") or payload.get("records") or []
    )
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
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def peak_rss_bytes() -> int | None:
    if resource is None:
        return None
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(peak if sys.platform == "darwin" else peak * 1024)


def sanitized_error(error: Exception, dsn: str) -> str:
    message = str(error).replace(dsn, "<redacted-dsn>")
    return re.sub(r"(?i)(password\s*=\s*)\S+", r"\1<redacted>", message)


def vector_literal(vector: list[float]) -> str:
    return "[" + ",".join(format(value, ".9g") for value in vector) + "]"


def make_record(index: int, dimension: int, rng: random.Random) -> dict[str, Any]:
    return {
        "id": f"rec-{index}",
        "vector": [rng.uniform(-1.0, 1.0) for _ in range(dimension)],
        "props": {"partition": f"p{index % 8}", "ordinal": index},
    }


@dataclass(frozen=True)
class DatasetManifest:
    sha256: str
    query_records: list[dict[str, Any]]


class Adapter(Protocol):
    system_id: str

    def prepare(self, dimension: int) -> None: ...

    def insert_batch(self, records: list[dict[str, Any]]) -> None: ...

    def finish_ingest(self) -> None: ...

    def search(
        self, record: dict[str, Any], *, filtered: bool
    ) -> tuple[list[str], float]: ...

    def signals(self) -> dict[str, Any]: ...

    def environment(self) -> dict[str, Any]: ...

    def close(self, *, keep_data: bool) -> None: ...


class ProximaAdapter:
    system_id = "proximadb"

    def __init__(self, base_url: str, timeout: float, collection: str) -> None:
        self.base_url = base_url
        self.timeout = timeout
        self.collection = collection

    def prepare(self, dimension: int) -> None:
        request(
            self.base_url,
            "POST",
            "/api/v2/collections",
            {
                "name": self.collection,
                "dimension": dimension,
                "engine": "sst",
                "distance_metric": "cosine",
                "enable_proxima_record": True,
                "schema": {
                    "columns": [
                        {
                            "name": "partition",
                            "data_type": "text",
                            "indexed": True,
                            "filterable": True,
                        },
                        {
                            "name": "ordinal",
                            "data_type": "integer",
                            "indexed": False,
                            "filterable": True,
                        },
                    ],
                    "enforcement": "hybrid",
                    "allow_additional_fields": True,
                },
            },
            self.timeout,
        )

    def insert_batch(self, records: list[dict[str, Any]]) -> None:
        request(
            self.base_url,
            "POST",
            f"/api/v2/collections/{self.collection}/records/batch",
            {"records": records, "upsert": True},
            self.timeout,
        )

    def finish_ingest(self) -> None:
        # The current supported corridor does not expose a write-fence endpoint.
        # Keep the historical bounded settle interval explicit in the report.
        time.sleep(0.75)

    def search(
        self, record: dict[str, Any], *, filtered: bool
    ) -> tuple[list[str], float]:
        body: dict[str, Any] = {"vector": record["vector"], "top_k": TOP_K}
        if filtered:
            body["filters"] = [
                {
                    "field": "partition",
                    "op": "eq",
                    "value": record["props"]["partition"],
                }
            ]
        payload, latency = request(
            self.base_url,
            "POST",
            f"/api/v2/collections/{self.collection}/search",
            body,
            self.timeout,
        )
        return ids(payload)[:TOP_K], latency

    def signals(self) -> dict[str, Any]:
        payload, _ = request(
            self.base_url,
            "GET",
            f"/api/v2/_diagnostics/collections/{self.collection}/route-health",
            None,
            self.timeout,
        )
        return payload if isinstance(payload, dict) else {}

    def environment(self) -> dict[str, Any]:
        return {
            "base_url": self.base_url,
            "engine": "sst",
            "distance": "cosine",
            "settle_seconds": 0.75,
        }

    def close(self, *, keep_data: bool) -> None:
        if not keep_data:
            request(
                self.base_url,
                "DELETE",
                f"/api/v2/collections/{self.collection}",
                None,
                self.timeout,
            )


class PgvectorAdapter:
    system_id = "pgvector"

    def __init__(self, dsn: str, table: str) -> None:
        if not table.replace("_", "").isalnum():
            raise ValueError("generated pgvector table name is not a safe identifier")
        self.dsn = dsn
        self.table = table
        self.quoted_table = f'"{table}"'
        self.quoted_partition_index = f'"{table}_partition_idx"'
        self.quoted_embedding_index = f'"{table}_embedding_hnsw_idx"'
        self.connection: Any = None
        self.driver = "unknown"
        self.server_version = "unknown"
        self.extension_version = "unknown"

    def _connect(self) -> None:
        try:
            import psycopg

            self.connection = psycopg.connect(self.dsn)
            self.driver = "psycopg3"
            return
        except ImportError:
            pass
        try:
            import psycopg2

            self.connection = psycopg2.connect(self.dsn)
            self.driver = "psycopg2"
            return
        except ImportError as exc:
            raise RuntimeError(
                "pgvector adapter requires 'psycopg[binary]' or 'psycopg2-binary'"
            ) from exc

    def prepare(self, dimension: int) -> None:
        self._connect()
        with self.connection.cursor() as cursor:
            cursor.execute("CREATE EXTENSION IF NOT EXISTS vector")
            cursor.execute(f"""
                CREATE TABLE {self.quoted_table} (
                    id TEXT PRIMARY KEY,
                    properties JSONB NOT NULL,
                    embedding vector({dimension}) NOT NULL
                )
                """)
            cursor.execute(
                f"CREATE INDEX {self.quoted_partition_index} "
                f"ON {self.quoted_table} ((properties ->> 'partition'))"
            )
            cursor.execute(
                f"CREATE INDEX {self.quoted_embedding_index} "
                f"ON {self.quoted_table} USING hnsw (embedding vector_cosine_ops)"
            )
            cursor.execute("SELECT version()")
            self.server_version = str(cursor.fetchone()[0])
            cursor.execute(
                "SELECT extversion FROM pg_extension WHERE extname = 'vector'"
            )
            self.extension_version = str(cursor.fetchone()[0])
        self.connection.commit()

    def insert_batch(self, records: list[dict[str, Any]]) -> None:
        rows = [
            (
                record["id"],
                json.dumps(record["props"], separators=(",", ":")),
                vector_literal(record["vector"]),
            )
            for record in records
        ]
        statement = (
            f"INSERT INTO {self.quoted_table} "
            "(id, properties, embedding) VALUES (%s, %s::jsonb, %s::vector)"
        )
        with self.connection.cursor() as cursor:
            if self.driver == "psycopg2":
                from psycopg2.extras import execute_values

                execute_values(
                    cursor,
                    statement.replace(
                        "VALUES (%s, %s::jsonb, %s::vector)", "VALUES %s"
                    ),
                    rows,
                    template="(%s, %s::jsonb, %s::vector)",
                    page_size=len(rows),
                )
            else:
                cursor.executemany(statement, rows)
        self.connection.commit()

    def finish_ingest(self) -> None:
        with self.connection.cursor() as cursor:
            cursor.execute(f"ANALYZE {self.quoted_table}")
            cursor.execute("SET hnsw.ef_search = 40")
            # pgvector applies WHERE predicates after an approximate HNSW scan.
            # Iterative scanning lets the access method continue until it has
            # enough predicate-matching rows instead of silently under-returning
            # filtered top-k results. strict_order preserves exact distance order
            # for a like-for-like accuracy comparison.
            cursor.execute("SET hnsw.iterative_scan = strict_order")
        self.connection.commit()

    def search(
        self, record: dict[str, Any], *, filtered: bool
    ) -> tuple[list[str], float]:
        where = "WHERE properties ->> 'partition' = %s" if filtered else ""
        parameters: tuple[Any, ...]
        if filtered:
            parameters = (
                record["props"]["partition"],
                vector_literal(record["vector"]),
                TOP_K,
            )
        else:
            parameters = (vector_literal(record["vector"]), TOP_K)
        statement = (
            f"SELECT id FROM {self.quoted_table} {where} "
            "ORDER BY embedding <=> %s::vector LIMIT %s"
        )
        started = time.perf_counter()
        with self.connection.cursor() as cursor:
            cursor.execute(statement, parameters)
            found = [str(row[0]) for row in cursor.fetchall()]
        return found, (time.perf_counter() - started) * 1000.0

    def signals(self) -> dict[str, Any]:
        with self.connection.cursor() as cursor:
            cursor.execute(
                "SELECT blks_read, blks_hit, tup_returned, tup_fetched "
                "FROM pg_stat_database WHERE datname = current_database()"
            )
            row = cursor.fetchone()
        if not row:
            return {}
        reads, hits, returned, fetched = (int(value) for value in row)
        return {
            "pg_stat_database": {
                "blocks_read": reads,
                "blocks_hit": hits,
                "tuples_returned": returned,
                "tuples_fetched": fetched,
            }
        }

    def environment(self) -> dict[str, Any]:
        return {
            "driver": self.driver,
            "server_version": self.server_version,
            "pgvector_version": self.extension_version,
            "index": "hnsw(vector_cosine_ops)",
            "hnsw_ef_search": 40,
            "hnsw_iterative_scan": "strict_order",
            "filter_index": "btree((properties ->> 'partition'))",
            "distance": "cosine",
        }

    def close(self, *, keep_data: bool) -> None:
        if self.connection is None:
            return
        try:
            if not keep_data:
                with self.connection.cursor() as cursor:
                    cursor.execute(f"DROP TABLE IF EXISTS {self.quoted_table}")
                self.connection.commit()
        finally:
            self.connection.close()


def ingest_stream(
    adapter: Adapter,
    *,
    record_count: int,
    dimension: int,
    batch_size: int,
    seed: int,
    query_indexes: list[int],
) -> tuple[DatasetManifest, float]:
    selected = set(query_indexes)
    query_records: dict[int, dict[str, Any]] = {}
    hasher = hashlib.sha256()
    hasher.update(b"[")
    rng = random.Random(seed)
    adapter.prepare(dimension)
    started = time.perf_counter()
    batch: list[dict[str, Any]] = []
    for index in range(record_count):
        record = make_record(index, dimension, rng)
        if index:
            hasher.update(b",")
        hasher.update(
            json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
        )
        if index in selected:
            query_records[index] = record
        batch.append(record)
        if len(batch) == batch_size:
            adapter.insert_batch(batch)
            batch = []
    if batch:
        adapter.insert_batch(batch)
    adapter.finish_ingest()
    ingest_seconds = time.perf_counter() - started
    hasher.update(b"]")
    missing = selected - query_records.keys()
    if missing:
        raise RuntimeError(f"query records were not generated: {sorted(missing)[:10]}")
    return (
        DatasetManifest(
            sha256=hasher.hexdigest(),
            query_records=[query_records[index] for index in query_indexes],
        ),
        ingest_seconds,
    )


def run_queries(
    adapter: Adapter,
    records: list[dict[str, Any]],
    *,
    warmup_count: int,
) -> tuple[dict[str, float], list[float]]:
    unfiltered_hits = 0
    filtered_hits = 0
    measured = 0
    filtered_latencies: list[float] = []
    for ordinal, record in enumerate(records):
        unfiltered_ids, _ = adapter.search(record, filtered=False)
        filtered_ids, filtered_latency = adapter.search(record, filtered=True)
        if ordinal < warmup_count:
            continue
        measured += 1
        filtered_latencies.append(filtered_latency)
        unfiltered_hits += int(record["id"] in unfiltered_ids[:TOP_K])
        filtered_hits += int(record["id"] in filtered_ids[:TOP_K])
    if measured == 0:
        raise RuntimeError("benchmark produced no measured queries")
    return (
        {
            "recall_at_10": round(unfiltered_hits / measured, 6),
            "filtered_recall_at_10": round(filtered_hits / measured, 6),
        },
        filtered_latencies,
    )


def unavailable_physical_metrics() -> dict[str, Any]:
    return {
        "object_gets": None,
        "object_puts": None,
        "bytes_read": None,
        "bytes_written": None,
        "cache_hit_ratio": None,
        "estimated_cost_per_million_queries": None,
    }


def build_report(
    adapter: Adapter,
    *,
    root: Path,
    manifest: DatasetManifest,
    record_count: int,
    dimension: int,
    seed: int,
    query_count: int,
    warmup_count: int,
    ingest_seconds: float,
    accuracy: dict[str, float],
    latencies: list[float],
) -> dict[str, Any]:
    metrics: dict[str, Any] = {
        **accuracy,
        "ingest_records_per_second": round(record_count / ingest_seconds, 3),
        "latency_p50_ms": round(percentile(latencies, 50), 3),
        "latency_p95_ms": round(percentile(latencies, 95), 3),
        "latency_p99_ms": round(percentile(latencies, 99), 3),
        "peak_rss_bytes": peak_rss_bytes(),
        **unavailable_physical_metrics(),
    }
    assert set(metrics) == set(REQUIRED_METRICS)
    return {
        "schema_version": 1,
        "scenario_id": SCENARIO_ID,
        "system": adapter.system_id,
        "status": "measured-local-baseline",
        "commit_sha": git_sha(root),
        "dataset": {
            "records": record_count,
            "dimension": dimension,
            "seed": seed,
            "sha256": manifest.sha256,
        },
        "environment": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            **adapter.environment(),
        },
        "metrics": metrics,
        "metric_scope": {
            "recall_at_10": "inserted query-record target present in top 10",
            "filtered_recall_at_10": "inserted query-record target present in filtered top 10",
            "latency_percentiles": "filtered search client-observed latency",
            "peak_rss_bytes": "benchmark runner peak RSS; not server RSS",
            "unavailable": [key for key, value in metrics.items() if value is None],
        },
        "query_count": query_count,
        "warmup_count": warmup_count,
        "server_signals": adapter.signals(),
        "publication_eligible": False,
        "publication_blocker": (
            "all adapters, five trials, >=1M records, four backends, and complete "
            "server-side physical-cost metrics are required"
        ),
    }


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def make_adapter(args: argparse.Namespace, run_name: str) -> Adapter:
    if args.system == "proximadb":
        return ProximaAdapter(args.base_url, args.timeout, run_name)
    return PgvectorAdapter(args.pg_dsn, run_name)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--system", choices=("proximadb", "pgvector"), default="proximadb"
    )
    parser.add_argument("--base-url", default="http://127.0.0.1:5678")
    parser.add_argument(
        "--pg-dsn",
        default="postgresql://postgres:postgres@127.0.0.1:5432/postgres",
        help="pgvector PostgreSQL DSN; never written to the report",
    )
    parser.add_argument("--records", type=int, default=1000)
    parser.add_argument("--dimension", type=int, default=32)
    parser.add_argument("--queries", type=int, default=200)
    parser.add_argument("--warmup", type=int, default=20)
    parser.add_argument("--batch-size", type=int, default=100)
    parser.add_argument("--seed", type=int, default=20260803)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--keep-data", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if min(args.records, args.dimension, args.queries, args.batch_size) <= 0:
        parser.error("records, dimension, queries, and batch-size must be positive")
    if args.warmup < 0:
        parser.error("warmup must be non-negative")

    root = Path(__file__).resolve().parents[2]
    run_rng = random.Random(args.seed ^ 0xC0DEC0DE)
    query_indexes = [
        run_rng.randrange(args.records) for _ in range(args.warmup + args.queries)
    ]
    run_name = f"context_bench_{int(time.time())}_{run_rng.randrange(100000):05d}"
    adapter = make_adapter(args, run_name)
    try:
        manifest, ingest_seconds = ingest_stream(
            adapter,
            record_count=args.records,
            dimension=args.dimension,
            batch_size=args.batch_size,
            seed=args.seed,
            query_indexes=query_indexes,
        )
        accuracy, latencies = run_queries(
            adapter,
            manifest.query_records,
            warmup_count=args.warmup,
        )
        report = build_report(
            adapter,
            root=root,
            manifest=manifest,
            record_count=args.records,
            dimension=args.dimension,
            seed=args.seed,
            query_count=args.queries,
            warmup_count=args.warmup,
            ingest_seconds=ingest_seconds,
            accuracy=accuracy,
            latencies=latencies,
        )
        write_report(args.output, report)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    except Exception as exc:  # Failed runs are evidence and must be retained.
        failure = {
            "schema_version": 1,
            "scenario_id": SCENARIO_ID,
            "system": args.system,
            "status": "failed",
            "commit_sha": git_sha(root),
            "error_type": type(exc).__name__,
            "error": sanitized_error(exc, args.pg_dsn),
            "publication_eligible": False,
        }
        write_report(args.output, failure)
        print(f"context corridor FAILED: {exc}", file=sys.stderr)
        return 1
    finally:
        try:
            adapter.close(keep_data=args.keep_data)
        except Exception as cleanup_error:
            print(f"context corridor cleanup warning: {cleanup_error}", file=sys.stderr)


if __name__ == "__main__":
    raise SystemExit(main())
