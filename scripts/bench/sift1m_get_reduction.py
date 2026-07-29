#!/usr/bin/env python3
"""Auditable SIFT1M GET-reduction acceptance harness.

This harness owns the server processes it measures and refuses to publish a
result unless:

* the binary comes from an optimized Cargo profile;
* the SIFT files have the requested cardinality and dimension;
* live PAX footer row counts sum to the full corpus;
* segment paths/sizes/row counts are stable after async compaction;
* the collection's unflushed-WAL byte gauge is zero;
* the physical object-store GET/byte counters are present; and
* recall is computed against full-corpus SIFT ground truth.

It measures three distinct states with fresh result/DRAM caches:

1. post-write: cache_on_write=all, before restart;
2. local_disk_warm: restart with the persistent local-disk tier; and
3. object_cold: restart without the local-disk tier (diagnostic baseline).

The local backend is intentional: CountingFileSystem meters the same physical
read seam used by object-store backends, without injecting WAN latency into a
GET-count correctness benchmark. Latency results therefore describe this
machine/filesystem profile and are not Azure network-latency claims.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request


PAX_MAGIC = b"PAXSEG01"
PAX_HEADER_MAGIC = b"PXH1"
METRICS = (
    "proximadb_object_store_gets_total",
    "proximadb_object_store_bytes_read_total",
    "proximadb_survivor_cache_hits",
    "proximadb_survivor_cache_misses",
    "proximadb_segment_invariants_cache_hits_total",
    "proximadb_segment_invariants_cache_misses_total",
    "proximadb_cache_local_disk_hits_total",
    "proximadb_cache_local_disk_misses_total",
    "proximadb_cache_local_disk_bytes",
    "proximadb_compactions_total",
    "proximadb_wal_size_bytes",
)


def request_json(url: str, method: str = "GET", body: object | None = None,
                 timeout: int = 180) -> dict:
    data = None if body is None else json.dumps(body).encode()
    headers = {} if data is None else {"Content-Type": "application/json"}
    request = urllib.request.Request(
        url, data=data, headers=headers, method=method
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        payload = response.read()
    return json.loads(payload) if payload else {}


def count_fixed_records(path: Path, scalar_bytes: int) -> tuple[int, int]:
    with path.open("rb") as source:
        header = source.read(4)
    if len(header) != 4:
        raise RuntimeError(f"{path}: missing vector header")
    dimension = struct.unpack("<i", header)[0]
    if dimension <= 0:
        raise RuntimeError(f"{path}: invalid dimension {dimension}")
    record_bytes = 4 + dimension * scalar_bytes
    size = path.stat().st_size
    if size % record_bytes:
        raise RuntimeError(
            f"{path}: {size} bytes is not a multiple of {record_bytes}"
        )
    return size // record_bytes, dimension


def read_fvecs(path: Path, start: int, count: int) -> list[list[float]]:
    total, dimension = count_fixed_records(path, 4)
    if start < 0 or count <= 0 or start + count > total:
        raise RuntimeError(
            f"{path}: requested [{start}, {start + count}) of {total}"
        )
    record_bytes = 4 + 4 * dimension
    vectors: list[list[float]] = []
    with path.open("rb") as source:
        source.seek(start * record_bytes)
        for _ in range(count):
            record = source.read(record_bytes)
            encoded_dimension = struct.unpack_from("<i", record, 0)[0]
            if encoded_dimension != dimension:
                raise RuntimeError(f"{path}: variable dimension encountered")
            vectors.append(
                list(struct.unpack_from(f"<{dimension}f", record, 4))
            )
    return vectors


def read_ivecs(path: Path, start: int, count: int) -> list[list[int]]:
    total, dimension = count_fixed_records(path, 4)
    if start < 0 or count <= 0 or start + count > total:
        raise RuntimeError(
            f"{path}: requested [{start}, {start + count}) of {total}"
        )
    record_bytes = 4 + 4 * dimension
    vectors: list[list[int]] = []
    with path.open("rb") as source:
        source.seek(start * record_bytes)
        for _ in range(count):
            record = source.read(record_bytes)
            encoded_dimension = struct.unpack_from("<i", record, 0)[0]
            if encoded_dimension != dimension:
                raise RuntimeError(f"{path}: variable dimension encountered")
            vectors.append(
                list(struct.unpack_from(f"<{dimension}i", record, 4))
            )
    return vectors


def iter_fvec_batches(path: Path, count: int, batch_size: int):
    total, dimension = count_fixed_records(path, 4)
    if count > total:
        raise RuntimeError(f"{path}: requested {count} of {total} vectors")
    record_bytes = 4 + 4 * dimension
    with path.open("rb") as source:
        next_id = 0
        while next_id < count:
            batch = []
            for _ in range(min(batch_size, count - next_id)):
                record = source.read(record_bytes)
                encoded_dimension = struct.unpack_from("<i", record, 0)[0]
                if encoded_dimension != dimension:
                    raise RuntimeError(f"{path}: variable dimension encountered")
                batch.append(
                    {
                        "id": f"v{next_id}",
                        "vector": list(
                            struct.unpack_from(f"<{dimension}f", record, 4)
                        ),
                    }
                )
                next_id += 1
            yield batch


def parse_pax(path: Path, root: Path) -> dict:
    size = path.stat().st_size
    if size < 25:
        raise RuntimeError(f"{path}: too short for a coalesced PAX segment")
    with path.open("rb") as segment:
        header = segment.read(72)
        segment.seek(-16, os.SEEK_END)
        tail = segment.read(16)
        if tail[8:] != PAX_MAGIC:
            raise RuntimeError(f"{path}: missing coalesced PAX tail")
        footer_len = struct.unpack_from("<Q", tail, 0)[0]
        if footer_len < 9 or footer_len + 16 > size:
            raise RuntimeError(f"{path}: invalid footer length {footer_len}")
        segment.seek(-(16 + footer_len), os.SEEK_END)
        footer_prefix = segment.read(9)
    if footer_prefix[0] != 1:
        raise RuntimeError(
            f"{path}: unsupported footer version {footer_prefix[0]}"
        )
    if header[:4] != PAX_HEADER_MAGIC:
        raise RuntimeError(f"{path}: legacy PAX layout cannot prove row count")
    return {
        "path": str(path.relative_to(root)),
        "bytes": size,
        "rows": struct.unpack_from("<Q", footer_prefix, 1)[0],
        "layout_version": header[4],
        "mtime_ns": path.stat().st_mtime_ns,
    }


def pax_geometry(root: Path) -> dict:
    segments = [
        parse_pax(path, root)
        for path in sorted(root.rglob("*.pax"))
        if path.is_file()
    ]
    return {
        "segment_count": len(segments),
        "row_count": sum(item["rows"] for item in segments),
        "bytes": sum(item["bytes"] for item in segments),
        "segments": segments,
    }


def stable_signature(geometry: dict) -> tuple:
    return tuple(
        (item["path"], item["bytes"], item["rows"], item["mtime_ns"])
        for item in geometry["segments"]
    )


def wait_for_materialization(root: Path, server: str, collection_id: str,
                             expected_rows: int, max_segments: int,
                             timeout_seconds: int,
                             stable_seconds: int) -> dict:
    deadline = time.monotonic() + timeout_seconds
    stable_since = None
    prior = None
    last_report = 0.0
    last_parse_error = None
    while time.monotonic() < deadline:
        now = time.monotonic()
        try:
            geometry = pax_geometry(root)
            last_parse_error = None
        except RuntimeError as error:
            # Compaction output is visible while it is still being streamed.
            # A missing tail is therefore transient, but it can never satisfy
            # the stable/quiescent gate below.
            last_parse_error = str(error)
            stable_since = None
            prior = None
            if now - last_report >= 15:
                print(f"settle: waiting for valid PAX tail: {error}", flush=True)
                last_report = now
            time.sleep(3)
            continue
        signature = stable_signature(geometry)
        wal_bytes = labelled_metric(
            scrape_text(server),
            "proximadb_wal_size_bytes",
            "collection",
            collection_id,
        )
        if now - last_report >= 15:
            print(
                "settle:"
                f" rows={geometry['row_count']:,}/{expected_rows:,}"
                f" segments={geometry['segment_count']}"
                f" bytes={geometry['bytes']:,}"
                f" wal_unflushed={wal_bytes!r}",
                flush=True,
            )
            last_report = now
        # Compaction publishes its output before deleting inputs, so a transient
        # row sum above `expected_rows` is legal. It must disappear before the
        # stable window; a persistent duplicate/stale segment fails by timeout.
        complete = (
            geometry["row_count"] == expected_rows
            and 0 < geometry["segment_count"] <= max_segments
            and wal_bytes == 0
        )
        if complete and signature == prior:
            stable_since = stable_since or now
            if now - stable_since >= stable_seconds:
                geometry["wal_unflushed_bytes"] = wal_bytes
                return geometry
        else:
            stable_since = now if complete else None
        prior = signature
        time.sleep(3)
    try:
        geometry = pax_geometry(root)
    except RuntimeError as error:
        last_parse_error = str(error)
        geometry = {"row_count": 0, "segment_count": 0}
    raise RuntimeError(
        "materialization/compaction did not quiesce: "
        f"rows={geometry['row_count']}/{expected_rows}, "
        f"segments={geometry['segment_count']} (max {max_segments}), "
        f"last_parse_error={last_parse_error!r}"
    )


def parse_prometheus(text: str) -> dict[str, float]:
    totals = {name: 0.0 for name in METRICS}
    present: set[str] = set()
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        token, _, raw_value = line.partition(" ")
        name = token.split("{", 1)[0]
        if name not in totals:
            continue
        try:
            value = float(raw_value.strip().split()[0])
        except (ValueError, IndexError):
            continue
        totals[name] += value
        present.add(name)
    totals["_present"] = sorted(present)  # type: ignore[assignment]
    return totals


def scrape_text(server: str) -> str:
    with urllib.request.urlopen(
        server + "/metrics/prometheus", timeout=30
    ) as response:
        return response.read().decode()


def scrape(server: str) -> dict[str, float]:
    return parse_prometheus(scrape_text(server))


def labelled_metric(text: str, name: str, label: str,
                    expected_value: str) -> float | None:
    """Return one exact labelled Prometheus sample without summing tenants."""
    label_pattern = re.compile(
        rf'(?:^|,){re.escape(label)}="([^"\\]*(?:\\.[^"\\]*)*)"'
    )
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line.startswith(name + "{"):
            continue
        token, _, raw_value = line.partition(" ")
        labels = token[len(name) + 1:-1]
        match = label_pattern.search(labels)
        if match is None or match.group(1) != expected_value:
            continue
        try:
            return float(raw_value.strip().split()[0])
        except (ValueError, IndexError):
            return None
    return None


def metric_delta(before: dict, after: dict, name: str) -> float:
    present = set(after.get("_present", []))
    if name not in present:
        raise RuntimeError(f"required metric {name} is absent after the sweep")
    delta = after[name] - before.get(name, 0.0)
    if delta < 0:
        raise RuntimeError(f"metric {name} moved backwards across one process")
    return delta


def percentile(sorted_values: list[float], quantile: float) -> float:
    rank = max(0, math.ceil(quantile * len(sorted_values)) - 1)
    return sorted_values[rank]


def run_query_sweep(server: str, collection_id: str, query_path: Path,
                    groundtruth_path: Path, query_start: int,
                    query_count: int, top_k: int, phase: str) -> dict:
    queries = read_fvecs(query_path, query_start, query_count)
    groundtruth = read_ivecs(groundtruth_path, query_start, query_count)
    before = scrape(server)
    latencies = []
    recalls = []
    for offset, query in enumerate(queries):
        started = time.perf_counter()
        response = request_json(
            f"{server}/api/v2/collections/{collection_id}/search",
            method="POST",
            body={"vector": query, "top_k": top_k},
            timeout=300,
        )
        latencies.append((time.perf_counter() - started) * 1000)
        returned = {item.get("id") for item in response.get("results", [])}
        expected = {
            f"v{row}" for row in groundtruth[offset][:top_k]
        }
        recalls.append(len(returned & expected) / top_k)
    after = scrape(server)
    latencies.sort()
    gets = metric_delta(
        before, after, "proximadb_object_store_gets_total"
    )
    bytes_read = metric_delta(
        before, after, "proximadb_object_store_bytes_read_total"
    )
    survivor_hits = metric_delta(
        before, after, "proximadb_survivor_cache_hits"
    )
    survivor_misses = metric_delta(
        before, after, "proximadb_survivor_cache_misses"
    )
    invariant_hits = metric_delta(
        before, after, "proximadb_segment_invariants_cache_hits_total"
    )
    invariant_misses = metric_delta(
        before, after, "proximadb_segment_invariants_cache_misses_total"
    )
    local_hits = metric_delta(
        before, after, "proximadb_cache_local_disk_hits_total"
    )
    local_misses = metric_delta(
        before, after, "proximadb_cache_local_disk_misses_total"
    )
    survivor_total = survivor_hits + survivor_misses
    invariant_total = invariant_hits + invariant_misses
    result = {
        "phase": phase,
        "query_start": query_start,
        "query_count": query_count,
        "top_k": top_k,
        "recall_at_k": sum(recalls) / len(recalls),
        "latency_ms": {
            "p50": percentile(latencies, 0.50),
            "p95": percentile(latencies, 0.95),
            "mean": sum(latencies) / len(latencies),
        },
        "physical_gets": gets,
        "gets_per_query": gets / query_count,
        "bytes_read": bytes_read,
        "bytes_per_query": bytes_read / query_count,
        "survivor": {
            "hits": survivor_hits,
            "misses": survivor_misses,
            "hit_ratio": (
                survivor_hits / survivor_total if survivor_total else None
            ),
        },
        "invariants": {
            "hits": invariant_hits,
            "misses": invariant_misses,
            "hit_ratio": (
                invariant_hits / invariant_total if invariant_total else None
            ),
        },
        "local_disk": {
            "hits": local_hits,
            "misses": local_misses,
            "resident_bytes": after[
                "proximadb_cache_local_disk_bytes"
            ],
        },
    }
    print(
        f"{phase}: recall@{top_k}={result['recall_at_k']:.4f} "
        f"GET/q={result['gets_per_query']:.2f} "
        f"bytes/q={result['bytes_per_query'] / 1_000_000:.2f}MB "
        f"p50={result['latency_ms']['p50']:.2f}ms "
        f"p95={result['latency_ms']['p95']:.2f}ms "
        f"local_hits={local_hits:.0f}",
        flush=True,
    )
    return result


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_config(path: Path, root: Path, port: int) -> None:
    data = root / "data"
    config = f"""[server]
node_id = "sift1m-get-reduction"
bind_address = "127.0.0.1"
port = {port}
data_dir = "{data}"

[server.tenant]
mode = "single_tenant"
default_tenant = "default"

[storage]
metadata_url = "file://{data / 'metadata'}"
mmap_enabled = false

[[storage.storage_locations]]
url = "file://{data / 'sst'}"
weight = 1
tags = ["durable", "benchmark"]

[storage.wal_config]
write_buffer_directory = "file://{data / 'wal'}"
enable_wal = true
sync_mode = "PerBatch"
write_buffer_size_mb = 4096
flush_interval_secs = 12

[storage.sst_config]
data_directory = "{data / 'sst'}"
mmap_enabled = false
segment_invariants_cache_mb = 256
survivor_cache_mb = 1024
cache_on_write = "all"

[api]
unified_mode = true
unified_port = {port}
rest_port = {port}
grpc_port = {port + 1}
arrow_flight_port = {port + 2}
pg_port = {port + 3}
internal_mux_port = {port + 10001}

[monitoring]
metrics_enabled = true
log_level = "info"
"""
    path.write_text(config)


class OwnedServer:
    def __init__(self, binary: Path, config: Path, server: str,
                 log_path: Path, local_disk_path: Path | None):
        self.binary = binary
        self.config = config
        self.server = server
        self.log_path = log_path
        self.local_disk_path = local_disk_path
        self.process: subprocess.Popen | None = None
        self.log_file = None

    def start(self) -> None:
        environment = os.environ.copy()
        environment.update(
            {
                "PROXIMADB_COUNT_FS_IO": "1",
                "PROXIMADB_CACHE_PREFILL": "0",
                "PROXIMADB_CACHE_ON_WRITE": "all",
                "PROXIMADB_L0_COMPACTION_ENABLED": "1",
            }
        )
        # The diagnostic object-cold phase must remain cold even when the
        # invoking shell exports a persistent-cache config mirror.
        for inherited_gate in (
            "PROXIMADB_CACHE_LOCAL_DISK_PATH",
            "PROXIMADB_CACHE_LOCAL_DISK_MAX_GB",
            "PROXIMADB_CACHE_NVME_PATH",
            "PROXIMADB_CACHE_NVME_MAX_GB",
        ):
            environment.pop(inherited_gate, None)
        if self.local_disk_path is not None:
            environment["PROXIMADB_CACHE_LOCAL_DISK_PATH"] = str(
                self.local_disk_path
            )
            environment["PROXIMADB_CACHE_LOCAL_DISK_MAX_GB"] = "10"
        self.log_file = self.log_path.open("wb")
        self.process = subprocess.Popen(
            [str(self.binary), "-c", str(self.config)],
            cwd=self.log_path.parent,
            stdout=self.log_file,
            stderr=subprocess.STDOUT,
            env=environment,
            start_new_session=True,
        )
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"server exited with {self.process.returncode}; "
                    f"see {self.log_path}"
                )
            try:
                request_json(self.server + "/health/live", timeout=5)
                return
            except (OSError, urllib.error.URLError, json.JSONDecodeError):
                time.sleep(1)
        raise RuntimeError(f"server did not become live; see {self.log_path}")

    def stop(self) -> None:
        if self.process is None:
            return
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        if self.log_file is not None:
            self.log_file.close()
        self.process = None


def ingest(server: str, base_path: Path, expected_rows: int,
           batch_size: int) -> tuple[str, float]:
    _, dimension = count_fixed_records(base_path, 4)
    response = request_json(
        server + "/api/v2/collections",
        method="POST",
        body={
            "name": "sift1m_get_reduction",
            "dimension": dimension,
            "engine": "sst",
            "enable_proxima_record": True,
            "distance_metric": "euclidean",
        },
    )
    collection_id = str(
        response.get("collection_id") or response.get("id") or ""
    )
    if not collection_id:
        raise RuntimeError(f"create response has no collection id: {response}")
    try:
        int(collection_id)
    except ValueError as error:
        raise RuntimeError(
            f"v2 create returned non-numeric catalog object id {collection_id!r}"
        ) from error
    started = time.perf_counter()
    inserted = 0
    for batch in iter_fvec_batches(base_path, expected_rows, batch_size):
        for attempt in range(8):
            try:
                request_json(
                    f"{server}/api/v2/collections/"
                    f"{collection_id}/records/batch",
                    method="POST",
                    body={"records": batch},
                    timeout=300,
                )
                break
            except urllib.error.HTTPError as error:
                if error.code not in (429, 503) or attempt == 7:
                    raise
                time.sleep(min(10, 0.25 * (2 ** attempt)))
        inserted += len(batch)
        if inserted % 100_000 == 0:
            elapsed = time.perf_counter() - started
            print(
                f"ingest: {inserted:,}/{expected_rows:,} "
                f"({inserted / elapsed:,.0f} vectors/s)",
                flush=True,
            )
    elapsed = time.perf_counter() - started
    return collection_id, elapsed


def require_empty_directory(path: Path) -> None:
    if path.exists() and any(path.iterdir()):
        raise RuntimeError(
            f"{path} is not empty; use a fresh benchmark root"
        )
    path.mkdir(parents=True, exist_ok=True)


def gate_failures(phase: str, result: dict, max_gets: float | None,
                  min_recall: float, max_p50_ms: float,
                  require_local_hit: bool) -> list[str]:
    failures = []
    if max_gets is not None and result["gets_per_query"] > max_gets:
        failures.append(
            f"{phase}: GET/q {result['gets_per_query']:.2f} > {max_gets:.2f}"
        )
    if result["recall_at_k"] < min_recall:
        failures.append(
            f"{phase}: recall {result['recall_at_k']:.4f} < {min_recall:.4f}"
        )
    if result["latency_ms"]["p50"] > max_p50_ms:
        failures.append(
            f"{phase}: p50 {result['latency_ms']['p50']:.2f}ms "
            f"> {max_p50_ms:.2f}ms"
        )
    if require_local_hit and result["local_disk"]["hits"] <= 0:
        failures.append(f"{phase}: local-disk phase recorded zero local hits")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release-server/proximadb-server"),
    )
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument(
        "--sift-dir",
        type=Path,
        default=Path(os.environ.get("SIFT_DIR", "/Users/vijaysingh/sift1m")),
    )
    parser.add_argument("--port", type=int, default=5690)
    parser.add_argument("--rows", type=int, default=1_000_000)
    parser.add_argument("--batch-size", type=int, default=2_000)
    parser.add_argument("--queries", type=int, default=200)
    parser.add_argument("--top-k", type=int, default=10)
    parser.add_argument("--settle-timeout-secs", type=int, default=1_200)
    parser.add_argument("--stable-secs", type=int, default=30)
    parser.add_argument("--max-segments", type=int, default=2)
    parser.add_argument("--post-write-max-gets", type=float, default=5.0)
    parser.add_argument("--local-warm-max-gets", type=float, default=10.0)
    parser.add_argument("--min-recall", type=float, default=0.98)
    parser.add_argument("--max-p50-ms", type=float, default=50.0)
    args = parser.parse_args()

    binary = args.binary.resolve()
    if not binary.is_file():
        raise RuntimeError(f"release binary not found: {binary}")
    binary_text = str(binary)
    if "/target/release/" not in binary_text and "/target/release-server/" not in binary_text:
        raise RuntimeError(
            "benchmark binary must come from target/release or "
            "target/release-server"
        )
    git_revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    tracked_status = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=normal"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if tracked_status:
        raise RuntimeError(
            "benchmark refuses a dirty worktree; commit the exact "
            "source state before building and measuring"
        )

    base_path = args.sift_dir / "sift_base.fvecs"
    query_path = args.sift_dir / "sift_query.fvecs"
    groundtruth_path = args.sift_dir / "sift_groundtruth.ivecs"
    base_count, base_dimension = count_fixed_records(base_path, 4)
    query_count, query_dimension = count_fixed_records(query_path, 4)
    gt_count, _ = count_fixed_records(groundtruth_path, 4)
    if args.rows > base_count or base_dimension != 128:
        raise RuntimeError(
            f"invalid SIFT base: rows={base_count}, dim={base_dimension}"
        )
    measured_queries = args.queries * 3
    if measured_queries > query_count or measured_queries > gt_count:
        raise RuntimeError(
            "not enough SIFT queries/ground-truth rows for three disjoint "
            f"{args.queries}-query phases"
        )
    if query_dimension != base_dimension:
        raise RuntimeError("base/query dimensions differ")

    root = args.root.resolve()
    require_empty_directory(root)
    config = root / "benchmark.toml"
    write_config(config, root, args.port)
    server_url = f"http://127.0.0.1:{args.port}"
    local_disk = root / "local-disk-cache"
    result = {
        "protocol": "sift1m_get_reduction_v2",
        "git_revision": git_revision,
        "binary": {
            "path": str(binary),
            "sha256": sha256(binary),
            "bytes": binary.stat().st_size,
            "profile": (
                "release-server"
                if "/target/release-server/" in binary_text
                else "release"
            ),
        },
        "dataset": {
            "base": str(base_path),
            "available_rows": base_count,
            "measured_rows": args.rows,
            "dimension": base_dimension,
            "query_count": args.queries,
            "phase_query_ranges": {
                "post_write": [0, args.queries],
                "local_disk_warm": [args.queries, args.queries * 2],
                "object_cold": [args.queries * 2, args.queries * 3],
            },
            "groundtruth_scope": base_count,
        },
        "filesystem_profile": {
            "segment_backend": "file",
            "local_disk_path": str(local_disk),
            "note": (
                "GET count is physical-I/O-seam evidence; latency is local "
                "filesystem evidence, not Azure WAN evidence"
            ),
        },
        "thresholds": {
            "post_write_max_gets_per_query": args.post_write_max_gets,
            "local_disk_warm_max_gets_per_query": args.local_warm_max_gets,
            "min_recall_at_k": args.min_recall,
            "max_p50_ms": args.max_p50_ms,
            "max_segments": args.max_segments,
        },
        "phases": {},
    }

    active: OwnedServer | None = None
    failures: list[str] = []
    try:
        active = OwnedServer(
            binary, config, server_url, root / "server-ingest.log", local_disk
        )
        active.start()
        collection_id, ingest_seconds = ingest(
            server_url, base_path, args.rows, args.batch_size
        )
        result["collection_id"] = collection_id
        result["ingest"] = {
            "seconds": ingest_seconds,
            "vectors_per_second": args.rows / ingest_seconds,
        }
        geometry = wait_for_materialization(
            root / "data" / "sst",
            server_url,
            collection_id,
            args.rows,
            args.max_segments,
            args.settle_timeout_secs,
            args.stable_secs,
        )
        result["settled_geometry"] = geometry
        post_write = run_query_sweep(
            server_url,
            collection_id,
            query_path,
            groundtruth_path,
            0,
            args.queries,
            args.top_k,
            "post_write",
        )
        result["phases"]["post_write"] = post_write
        failures.extend(gate_failures(
            "post_write",
            post_write,
            args.post_write_max_gets,
            args.min_recall,
            args.max_p50_ms,
            require_local_hit=False,
        ))
        active.stop()

        active = OwnedServer(
            binary,
            config,
            server_url,
            root / "server-local-disk-warm.log",
            local_disk,
        )
        active.start()
        local_warm = run_query_sweep(
            server_url,
            collection_id,
            query_path,
            groundtruth_path,
            args.queries,
            args.queries,
            args.top_k,
            "local_disk_warm",
        )
        result["phases"]["local_disk_warm"] = local_warm
        failures.extend(gate_failures(
            "local_disk_warm",
            local_warm,
            args.local_warm_max_gets,
            args.min_recall,
            args.max_p50_ms,
            require_local_hit=True,
        ))
        active.stop()

        active = OwnedServer(
            binary,
            config,
            server_url,
            root / "server-object-cold.log",
            None,
        )
        active.start()
        object_cold = run_query_sweep(
            server_url,
            collection_id,
            query_path,
            groundtruth_path,
            args.queries * 2,
            args.queries,
            args.top_k,
            "object_cold",
        )
        result["phases"]["object_cold"] = object_cold
        failures.extend(gate_failures(
            "object_cold",
            object_cold,
            None,
            args.min_recall,
            args.max_p50_ms,
            require_local_hit=False,
        ))
        if failures:
            result["gate_failures"] = failures
            raise RuntimeError("; ".join(failures))
        result["status"] = "pass"
    except Exception as error:
        result["status"] = "fail"
        result["error"] = str(error)
        raise
    finally:
        if active is not None:
            active.stop()
        output = root / "result.json"
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print(f"result: {output}", flush=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"ERROR: {error}", file=sys.stderr, flush=True)
        raise
