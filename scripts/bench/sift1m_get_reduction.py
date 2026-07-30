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

The default local backend meters the same physical read seam used by
object-store backends. Pass ``--storage-url adls://... --azurite`` to exercise
the production Azure backend over HTTP. Azurite latency is local-emulator
evidence, not a production Azure WAN-latency claim.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import signal
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path, PurePosixPath
from urllib.parse import urlparse

PAX_MAGIC = b"PAXSEG01"
PAX_HEADER_MAGIC = b"PXH1"
A0_MAGIC = b"PXA0"
AZURE_COALESCE_GAP_BYTES = 1024 * 1024
AZURE_COALESCE_RANGE_BYTES = 4 * 1024 * 1024
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
    "proximadb_ivf_cells_total",
    "proximadb_ivf_cells_probed_total",
    "proximadb_ivf_probed_rows_total",
    "proximadb_ivf_region_a_bytes_read_total",
    "proximadb_ivf_region_b_bytes_read_total",
    "proximadb_ivf_fetch_rounds_total",
    "proximadb_ivf_whole_region_fallback_total",
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


def summarize_values(values: list[int] | list[float]) -> dict:
    ordered = sorted(values)
    if not ordered:
        return {
            "min": None,
            "p50": None,
            "p95": None,
            "max": None,
            "mean": None,
        }
    return {
        "min": ordered[0],
        "p50": percentile(ordered, 0.50),
        "p95": percentile(ordered, 0.95),
        "max": ordered[-1],
        "mean": sum(ordered) / len(ordered),
    }


def fnv1a64(data: bytes) -> int:
    value = 0xCBF29CE484222325
    for byte in data:
        value ^= byte
        value = (value * 0x00000100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value


def parse_a0_geometry(a0: bytes) -> dict:
    """Parse the persisted coarse-model shape without interpreting vectors."""
    if len(a0) < 48 or a0[:4] != A0_MAGIC:
        raise RuntimeError("invalid A0 coarse directory")
    if a0[4] != 1:
        raise RuntimeError(f"unsupported A0 version {a0[4]}")
    n_comp = struct.unpack_from("<H", a0, 6)[0]
    cells = struct.unpack_from("<I", a0, 8)[0]
    dimension = struct.unpack_from("<I", a0, 12)[0]
    seed = struct.unpack_from("<Q", a0, 16)[0]
    trained_rows = struct.unpack_from("<Q", a0, 24)[0]
    covered_rows = struct.unpack_from("<Q", a0, 32)[0]
    expected_length = (
        40
        + dimension * 4
        + n_comp * dimension * 4
        + cells * n_comp * 4
        + cells * 4
        + cells * 72
        + 8
    )
    if len(a0) != expected_length:
        raise RuntimeError(
            f"A0 length {len(a0)} != expected {expected_length}"
        )
    stored_checksum = struct.unpack_from("<Q", a0, len(a0) - 8)[0]
    if fnv1a64(a0[:-8]) != stored_checksum:
        raise RuntimeError("A0 checksum mismatch")

    offset = (
        40
        + dimension * 4
        + n_comp * dimension * 4
        + cells * n_comp * 4
    )
    radii = list(struct.unpack_from(f"<{cells}f", a0, offset))
    offset += cells * 4
    cell_rows = []
    for _ in range(cells):
        row_begin, row_end = struct.unpack_from("<QQ", a0, offset)
        if row_end < row_begin:
            raise RuntimeError("A0 cell has a descending row range")
        cell_rows.append(row_end - row_begin)
        offset += 72
    if sum(cell_rows) != covered_rows:
        raise RuntimeError(
            f"A0 cell rows {sum(cell_rows)} != covered rows {covered_rows}"
        )
    nonempty = [rows for rows in cell_rows if rows]
    row_summary = summarize_values(cell_rows)
    mean_rows = row_summary["mean"] or 0.0
    return {
        "coarse_cells": cells,
        "coarse_components": n_comp,
        "coarse_dimension": dimension,
        "coarse_seed": seed,
        "coarse_trained_rows": trained_rows,
        "coarse_rows_covered": covered_rows,
        "training_rows_per_cell": (
            trained_rows / cells if cells else None
        ),
        "empty_cells": cells - len(nonempty),
        "empty_cell_fraction": (
            (cells - len(nonempty)) / cells if cells else None
        ),
        "cell_rows": cell_rows,
        "cell_row_summary": row_summary,
        "cell_row_max_to_mean": (
            max(cell_rows) / mean_rows if mean_rows else None
        ),
        "radii": radii,
        "radius_summary": summarize_values(radii),
    }


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
    coarse = {}
    if header[4] == 3:
        a0_offset, a0_length = struct.unpack_from("<QQ", header, 56)
        if a0_length < 48:
            raise RuntimeError(f"{path}: invalid A0 length {a0_length}")
        with path.open("rb") as segment:
            segment.seek(a0_offset)
            a0 = segment.read(a0_length)
        if len(a0) != a0_length:
            raise RuntimeError(f"{path}: truncated A0 directory")
        coarse = parse_a0_geometry(a0)
    return {
        "path": str(path.relative_to(root)),
        "bytes": size,
        "rows": struct.unpack_from("<Q", footer_prefix, 1)[0],
        "layout_version": header[4],
        "mtime_ns": path.stat().st_mtime_ns,
        **coarse,
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


class AzureCliPaxGeometry:
    """Read final PAX geometry from Azure/Azurite without touching server metrics.

    Query GET counters live inside the measured server process. The Azure CLI
    inventory/download calls here are out-of-process evidence reads performed
    before query sweeps. They cannot inflate or warm ProximaDB's cache counters,
    although they may warm Azurite's host page cache.
    """

    def __init__(self, storage_url: str, snapshot_root: Path):
        parsed = urlparse(storage_url)
        if parsed.scheme not in {"adls", "az", "azure"}:
            raise RuntimeError(
                f"Azure geometry requires adls:// storage, got {storage_url}"
            )
        if not parsed.netloc:
            raise RuntimeError(
                f"Azure storage URL has no container: {storage_url}"
            )
        self.container = parsed.netloc
        self.prefix = parsed.path.strip("/")
        self.snapshot_root = snapshot_root

    def _list_blobs(self) -> list[dict]:
        command = [
            "az",
            "storage",
            "blob",
            "list",
            "--container-name",
            self.container,
            "--output",
            "json",
        ]
        if self.prefix:
            command.extend(["--prefix", self.prefix])
        completed = subprocess.run(
            command,
            check=True,
            capture_output=True,
            text=True,
        )
        payload = json.loads(completed.stdout)
        if not isinstance(payload, list):
            raise RuntimeError("Azure CLI blob inventory was not a JSON list")
        return payload

    def require_empty_prefix(self) -> None:
        blobs = self._list_blobs()
        if blobs:
            names = [item.get("name", "<unnamed>") for item in blobs[:5]]
            raise RuntimeError(
                "Azure benchmark prefix is not empty: "
                f"adls://{self.container}/{self.prefix}, first_blobs={names}"
            )

    def _snapshot_target(self, blob_name: str) -> Path:
        relative = PurePosixPath(blob_name)
        if relative.is_absolute() or any(
            component in {"", ".", ".."} for component in relative.parts
        ):
            raise RuntimeError(
                f"unsafe Azure blob name in geometry snapshot: {blob_name!r}"
            )
        return self.snapshot_root.joinpath(*relative.parts)

    def inventory(self) -> dict:
        segments = []
        for blob in self._list_blobs():
            name = str(blob.get("name", ""))
            if not name.endswith(".pax"):
                continue
            properties = blob.get("properties") or {}
            size = properties.get("contentLength", blob.get("contentLength"))
            if size is None:
                raise RuntimeError(f"Azure blob has no content length: {name}")
            segments.append(
                {
                    "path": name,
                    "bytes": int(size),
                    "etag": str(properties.get("etag", blob.get("etag", ""))),
                    "last_modified": str(
                        properties.get(
                            "lastModified", blob.get("lastModified", "")
                        )
                    ),
                }
            )
        segments.sort(key=lambda item: item["path"])
        return {
            "segment_count": len(segments),
            "bytes": sum(item["bytes"] for item in segments),
            "segments": segments,
        }

    @staticmethod
    def stable_signature(inventory: dict) -> tuple:
        return tuple(
            (item["path"], item["bytes"], item["etag"])
            for item in inventory["segments"]
        )

    def materialize(self, inventory: dict) -> dict:
        self.snapshot_root.mkdir(parents=True, exist_ok=True)
        segments = []
        for blob in inventory["segments"]:
            target = self._snapshot_target(blob["path"])
            target.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(
                [
                    "az",
                    "storage",
                    "blob",
                    "download",
                    "--container-name",
                    self.container,
                    "--name",
                    blob["path"],
                    "--file",
                    str(target),
                    "--overwrite",
                    "true",
                    "--no-progress",
                    "--output",
                    "none",
                ],
                check=True,
            )
            if target.stat().st_size != blob["bytes"]:
                raise RuntimeError(
                    f"Azure snapshot size mismatch for {blob['path']}: "
                    f"{target.stat().st_size} != {blob['bytes']}"
                )
            parsed = parse_pax(target, self.snapshot_root)
            parsed["path"] = blob["path"]
            parsed["blob_etag"] = blob["etag"]
            segments.append(parsed)
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


def wait_for_materialization(
        root: Path, server: str, collection_id: str, expected_rows: int,
        max_segments: int, timeout_seconds: int, stable_seconds: int,
        azure_geometry: AzureCliPaxGeometry | None = None) -> dict:
    deadline = time.monotonic() + timeout_seconds
    stable_since = None
    prior = None
    last_report = 0.0
    last_parse_error = None
    while time.monotonic() < deadline:
        now = time.monotonic()
        try:
            geometry = (
                azure_geometry.inventory()
                if azure_geometry is not None
                else pax_geometry(root)
            )
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
        signature = (
            azure_geometry.stable_signature(geometry)
            if azure_geometry is not None
            else stable_signature(geometry)
        )
        wal_bytes = labelled_metric(
            scrape_text(server),
            "proximadb_wal_size_bytes",
            "collection",
            collection_id,
        )
        observed_rows = geometry.get("row_count")
        row_status = (
            f"{observed_rows:,}/{expected_rows:,}"
            if observed_rows is not None
            else f"pending-footer-snapshot/{expected_rows:,}"
        )
        if now - last_report >= 15:
            print(
                "settle:"
                f" rows={row_status}"
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
            (
                observed_rows == expected_rows
                if azure_geometry is None
                else True
            )
            and 0 < geometry["segment_count"] <= max_segments
            and wal_bytes == 0
        )
        if complete and signature == prior:
            stable_since = stable_since or now
            if now - stable_since >= stable_seconds:
                if azure_geometry is not None:
                    geometry = azure_geometry.materialize(geometry)
                    if geometry["row_count"] != expected_rows:
                        raise RuntimeError(
                            "quiescent Azure PAX footer row mismatch: "
                            f"{geometry['row_count']} != {expected_rows}"
                        )
                geometry["wal_unflushed_bytes"] = wal_bytes
                return geometry
        else:
            stable_since = now if complete else None
        prior = signature
        time.sleep(3)
    try:
        geometry = (
            azure_geometry.inventory()
            if azure_geometry is not None
            else pax_geometry(root)
        )
    except RuntimeError as error:
        last_parse_error = str(error)
        geometry = {"row_count": 0, "segment_count": 0}
    raise RuntimeError(
        "materialization/compaction did not quiesce: "
        f"rows={geometry.get('row_count', 'not-snapshotted')}/{expected_rows}, "
        f"segments={geometry['segment_count']} (max {max_segments}), "
        f"last_parse_error={last_parse_error!r}"
    )


def parse_prometheus(text: str) -> dict[str, float]:
    totals = dict.fromkeys(METRICS, 0.0)
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
    present_before = set(before.get("_present", []))
    present_after = set(after.get("_present", []))
    if name not in present_after and name not in present_before:
        # Prometheus collectors may register counters lazily on the first
        # increment. Absence on both sides is therefore an observed zero
        # delta, which is the expected state for a fully eliminated GET path.
        return 0.0
    if name not in present_after:
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
    ivf_cells_total = metric_delta(
        before, after, "proximadb_ivf_cells_total"
    )
    ivf_cells_probed = metric_delta(
        before, after, "proximadb_ivf_cells_probed_total"
    )
    ivf_probed_rows = metric_delta(
        before, after, "proximadb_ivf_probed_rows_total"
    )
    ivf_region_a_bytes = metric_delta(
        before, after, "proximadb_ivf_region_a_bytes_read_total"
    )
    ivf_region_b_bytes = metric_delta(
        before, after, "proximadb_ivf_region_b_bytes_read_total"
    )
    ivf_fetch_rounds = metric_delta(
        before, after, "proximadb_ivf_fetch_rounds_total"
    )
    ivf_whole_region_fallbacks = metric_delta(
        before, after, "proximadb_ivf_whole_region_fallback_total"
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
        "ivf": {
            "cells_total": ivf_cells_total,
            "cells_total_per_query": ivf_cells_total / query_count,
            "cells_probed": ivf_cells_probed,
            "cells_probed_per_query": ivf_cells_probed / query_count,
            "probed_rows": ivf_probed_rows,
            "probed_rows_per_query": ivf_probed_rows / query_count,
            "region_a_bytes": ivf_region_a_bytes,
            "region_a_bytes_per_query": (
                ivf_region_a_bytes / query_count
            ),
            "region_b_bytes": ivf_region_b_bytes,
            "region_b_bytes_per_query": (
                ivf_region_b_bytes / query_count
            ),
            "fetch_rounds": ivf_fetch_rounds,
            "fetch_rounds_per_query": ivf_fetch_rounds / query_count,
            "whole_region_fallbacks": ivf_whole_region_fallbacks,
        },
    }
    print(
        f"{phase}: recall@{top_k}={result['recall_at_k']:.4f} "
        f"GET/q={result['gets_per_query']:.2f} "
        f"bytes/q={result['bytes_per_query'] / 1_000_000:.2f}MB "
        f"p50={result['latency_ms']['p50']:.2f}ms "
        f"p95={result['latency_ms']['p95']:.2f}ms "
        f"cells/q={result['ivf']['cells_probed_per_query']:.2f} "
        f"rows/q={result['ivf']['probed_rows_per_query']:.0f} "
        f"rounds/q={result['ivf']['fetch_rounds_per_query']:.2f} "
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


def require_complete_insert(response: dict, expected_count: int) -> None:
    inserted = response.get("inserted_count")
    failed = response.get("failed_count")
    if inserted != expected_count or failed != 0:
        raise RuntimeError(
            "batch insert was not fully admitted: "
            f"expected={expected_count}, inserted_count={inserted!r}, "
            f"failed_count={failed!r}, errors={response.get('errors')!r}"
        )


def write_config(
        path: Path, root: Path, port: int, write_buffer_mb: int,
        flush_vector_threshold: int, storage_url: str) -> None:
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

[storage.optimization]
enable_mmap = false

[[storage.storage_locations]]
url = "{storage_url}"
weight = 1
tags = ["durable", "benchmark"]

[storage.wal_config]
write_buffer_directory = "file://{data / 'wal'}"
enable_wal = true
sync_mode = "PerBatch"
write_buffer_size_mb = {write_buffer_mb}
vector_count_threshold = {flush_vector_threshold}
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
                 log_path: Path, local_disk_path: Path | None,
                 ivf_k: int | None = None, nprobe: int | None = None,
                 azure_emulator: bool = False):
        self.binary = binary
        self.config = config
        self.server = server
        self.log_path = log_path
        self.local_disk_path = local_disk_path
        self.ivf_k = ivf_k
        self.nprobe = nprobe
        self.azure_emulator = azure_emulator
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
                # Keep the planner fixed across local and Azure profiles so
                # backend choice does not silently change the read geometry.
                "PROXIMADB_PAX_VECTOR_COALESCE_GAP": str(
                    AZURE_COALESCE_GAP_BYTES
                ),
                "PROXIMADB_PAX_VECTOR_COALESCE_RANGE": str(
                    AZURE_COALESCE_RANGE_BYTES
                ),
                "PROXIMADB_PAX_COALESCE_GAP": str(
                    AZURE_COALESCE_GAP_BYTES
                ),
                "PROXIMADB_PAX_COALESCE_RANGE": str(
                    AZURE_COALESCE_RANGE_BYTES
                ),
            }
        )
        # The diagnostic object-cold phase must remain cold even when the
        # invoking shell exports a persistent-cache config mirror.
        for inherited_gate in (
            "PROXIMADB_CACHE_LOCAL_DISK_PATH",
            "PROXIMADB_CACHE_LOCAL_DISK_MAX_GB",
            "PROXIMADB_CACHE_NVME_PATH",
            "PROXIMADB_CACHE_NVME_MAX_GB",
            "PROXIMADB_IVF_K",
            "PROXIMADB_PAX_READ_COARSE_NPROBE",
        ):
            environment.pop(inherited_gate, None)
        if self.ivf_k is not None:
            environment["PROXIMADB_IVF_K"] = str(self.ivf_k)
        if self.nprobe is not None:
            environment["PROXIMADB_PAX_READ_COARSE_NPROBE"] = str(self.nprobe)
        if self.azure_emulator:
            environment.update(
                {
                    "PROXIMADB_AZURE_EMULATOR": "1",
                    "AZURE_STORAGE_USE_EMULATOR": "true",
                    "AZURE_ALLOW_HTTP": "true",
                    "AZURE_STORAGE_ACCOUNT": "devstoreaccount1",
                    "AZURE_STORAGE_ACCOUNT_NAME": "devstoreaccount1",
                }
            )
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


def storage_action_via_flight(
        host: str, port: int, collection_id: str, action_type: str,
        expected_operation: str, flight_module=None) -> dict:
    if flight_module is None:
        try:
            import pyarrow.flight as flight_module
        except ImportError as error:
            raise RuntimeError(
                "--explicit-flush-every-rows requires PyArrow Flight; "
                "run the harness with the repository Python environment"
            ) from error

    location = flight_module.Location.for_grpc_tcp(host, port)
    client = flight_module.FlightClient(location)
    try:
        action = flight_module.Action(
            action_type,
            json.dumps({"collection_id": collection_id}).encode(),
        )
        responses = list(client.do_action(action))
    finally:
        client.close()
    if len(responses) != 1:
        raise RuntimeError(
            f"Flight {action_type} returned an unexpected response count: "
            f"{len(responses)}"
        )
    payload = json.loads(bytes(responses[0].body))
    if (
        payload.get("success") is not True
        or payload.get("operation") != expected_operation
    ):
        raise RuntimeError(
            f"Flight {action_type} did not succeed as expected: {payload}"
        )
    return payload


def force_flush_via_flight(
        host: str, port: int, collection_id: str,
        flight_module=None) -> dict:
    return storage_action_via_flight(
        host,
        port,
        collection_id,
        "flush_collection",
        "flush",
        flight_module,
    )


def compact_via_flight(
        host: str, port: int, collection_id: str,
        flight_module=None) -> dict:
    return storage_action_via_flight(
        host,
        port,
        collection_id,
        "compact_collection",
        "compact",
        flight_module,
    )


def wait_for_pax_epoch(
        server: str, collection_id: str, geometry: AzureCliPaxGeometry,
        before_inventory: dict, timeout_seconds: int = 300,
) -> tuple[float, dict, float | None]:
    started = time.monotonic()
    deadline = started + timeout_seconds
    before_signature = geometry.stable_signature(before_inventory)
    prior_signature = None
    stable_observations = 0
    while time.monotonic() < deadline:
        inventory = geometry.inventory()
        signature = geometry.stable_signature(inventory)
        if signature != before_signature:
            stable_observations = (
                stable_observations + 1
                if signature == prior_signature
                else 1
            )
        else:
            stable_observations = 0
        prior_signature = signature
        wal_bytes = labelled_metric(
            scrape_text(server),
            "proximadb_wal_size_bytes",
            "collection",
            collection_id,
        )
        if (
            stable_observations >= 2
            and (wal_bytes is None or wal_bytes == 0)
        ):
            return time.monotonic() - started, inventory, wal_bytes
        time.sleep(0.5)
    raise RuntimeError(
        "storage action did not publish a stable PAX epoch within "
        f"{timeout_seconds}s: collection={collection_id}"
    )


def post_flush_compaction_observation(
        explicit_flush_every_rows: int | None) -> dict | None:
    if explicit_flush_every_rows is None:
        return None
    return {
        "requested": False,
        "reason": (
            "automatic threshold compaction is observed by the stable "
            "materialization gate"
        ),
    }


def ingest(
        server: str, base_path: Path, expected_rows: int, batch_size: int,
        flight_host: str, flight_port: int,
        explicit_flush_every_rows: int | None,
        explicit_geometry: AzureCliPaxGeometry | None,
) -> tuple[str, float, dict[int, int], list[dict], dict | None]:
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
    retry_status_counts: dict[int, int] = {}
    explicit_flushes = []
    for batch in iter_fvec_batches(base_path, expected_rows, batch_size):
        for attempt in range(8):
            try:
                response = request_json(
                    f"{server}/api/v2/collections/"
                    f"{collection_id}/records/batch",
                    method="POST",
                    body={"records": batch},
                    timeout=300,
                )
                require_complete_insert(response, len(batch))
                break
            except urllib.error.HTTPError as error:
                if error.code not in (429, 503) or attempt == 7:
                    raise
                retry_status_counts[error.code] = (
                    retry_status_counts.get(error.code, 0) + 1
                )
                time.sleep(min(10, 0.25 * (2 ** attempt)))
        inserted += len(batch)
        if (
            explicit_flush_every_rows is not None
            and inserted % explicit_flush_every_rows == 0
        ):
            if explicit_geometry is None:
                raise RuntimeError(
                    "explicit flush epochs require Azure PAX inventory evidence"
                )
            inventory_before = explicit_geometry.inventory()
            wal_before = labelled_metric(
                scrape_text(server),
                "proximadb_wal_size_bytes",
                "collection",
                collection_id,
            )
            flush_started = time.perf_counter()
            response = force_flush_via_flight(
                flight_host,
                flight_port,
                collection_id,
            )
            action_seconds = time.perf_counter() - flush_started
            (
                publish_seconds,
                inventory_after,
                wal_after,
            ) = wait_for_pax_epoch(
                server,
                collection_id,
                explicit_geometry,
                inventory_before,
            )
            explicit_flushes.append(
                {
                    "after_rows": inserted,
                    "wal_bytes_before": wal_before,
                    "wal_bytes_after": wal_after,
                    "action_seconds": action_seconds,
                    "publish_seconds": publish_seconds,
                    "segments_before": inventory_before["segments"],
                    "segments_after": inventory_after["segments"],
                    "response": response,
                }
            )
            print(
                f"explicit flush: rows={inserted:,} "
                f"action={action_seconds:.3f}s "
                f"publish={publish_seconds:.3f}s "
                f"segments={inventory_after['segment_count']}",
                flush=True,
            )
        if inserted % 100_000 == 0:
            elapsed = time.perf_counter() - started
            print(
                f"ingest: {inserted:,}/{expected_rows:,} "
                f"({inserted / elapsed:,.0f} vectors/s)",
                flush=True,
            )
    if inserted != expected_rows:
        raise RuntimeError(
            f"ingest iterator admitted {inserted} rows, expected {expected_rows}"
        )
    # Do not issue a second compaction after the final explicit flush. The
    # fifth L0 epoch arms automatic compaction at the configured threshold,
    # and wait_for_materialization below is the authoritative quiescence gate.
    # If automatic compaction already finished, the operator action is a valid
    # no-op and therefore cannot produce the "new PAX epoch" this harness used
    # to wait for. Requiring one caused a false 300-second timeout; racing the
    # automatic morsel also risked measuring redundant work rather than the
    # settled write path.
    explicit_compaction = post_flush_compaction_observation(
        explicit_flush_every_rows
    )
    elapsed = time.perf_counter() - started
    return (
        collection_id,
        elapsed,
        retry_status_counts,
        explicit_flushes,
        explicit_compaction,
    )


def require_empty_directory(path: Path) -> None:
    if path.exists() and any(path.iterdir()):
        raise RuntimeError(
            f"{path} is not empty; use a fresh benchmark root"
        )
    path.mkdir(parents=True, exist_ok=True)


def require_groundtruth_scope(rows: int, groundtruth_scope: int) -> None:
    if groundtruth_scope != rows:
        raise RuntimeError(
            "ground-truth corpus mismatch: "
            f"--rows={rows}, groundtruth_scope={groundtruth_scope}; "
            "subset runs require an ivecs file generated against exactly the "
            "same corpus prefix"
        )


def gate_failures(phase: str, result: dict, max_gets: float | None,
                  min_recall: float, max_p50_ms: float | None,
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
    if (
        max_p50_ms is not None
        and result["latency_ms"]["p50"] > max_p50_ms
    ):
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
    parser.add_argument(
        "--groundtruth-path",
        type=Path,
        help=(
            "ivecs ground truth for exactly --rows corpus rows; defaults to "
            "<sift-dir>/sift_groundtruth.ivecs (the full base corpus)"
        ),
    )
    parser.add_argument(
        "--groundtruth-scope-rows",
        type=int,
        help=(
            "corpus cardinality used to generate --groundtruth-path; required "
            "with a custom path and required to equal --rows"
        ),
    )
    parser.add_argument("--port", type=int, default=5690)
    parser.add_argument("--rows", type=int, default=1_000_000)
    parser.add_argument("--batch-size", type=int, default=2_000)
    parser.add_argument("--write-buffer-mb", type=int, default=4096)
    parser.add_argument(
        "--flush-vector-threshold",
        type=int,
        default=100_000,
        help=(
            "WAL vector-count threshold; the predicted-byte floor may defer "
            "its size flush, so this does not guarantee segment cardinality"
        ),
    )
    parser.add_argument(
        "--explicit-flush-every-rows",
        type=int,
        help=(
            "issue the supported Arrow Flight flush action after each exact "
            "row interval; intended for controlled small-corpus geometry"
        ),
    )
    parser.add_argument("--queries", type=int, default=1_000)
    parser.add_argument("--top-k", type=int, default=10)
    parser.add_argument("--settle-timeout-secs", type=int, default=1_200)
    parser.add_argument("--stable-secs", type=int, default=30)
    parser.add_argument("--max-segments", type=int, default=2)
    parser.add_argument("--require-layout-version", type=int, default=3)
    parser.add_argument("--post-write-max-gets", type=float, default=5.0)
    parser.add_argument("--local-warm-max-gets", type=float, default=10.0)
    parser.add_argument("--object-cold-max-gets", type=float, default=20.0)
    parser.add_argument("--min-recall", type=float, default=0.98)
    parser.add_argument("--max-p50-ms", type=float, default=50.0)
    parser.add_argument(
        "--storage-url",
        help=(
            "durable segment base URL; defaults to a file:// directory under "
            "--root. adls:// requires Azure CLI for footer geometry."
        ),
    )
    parser.add_argument(
        "--azurite",
        action="store_true",
        help=(
            "run the production Azure backend against local Azurite; requires "
            "--storage-url adls://... and AZURE_STORAGE_CONNECTION_STRING for "
            "out-of-process geometry snapshots"
        ),
    )
    parser.add_argument(
        "--local-warm-max-p50-ms",
        type=float,
        help=(
            "optional local-tier latency gate; unset by default because the "
            "acceptance latency target is object-cold and local file:// "
            "latency is not an Azure WAN proxy"
        ),
    )
    parser.add_argument(
        "--binary-source-revision",
        help=(
            "Git revision used to build --binary; defaults to current HEAD. "
            "Use only when later commits touch docs, benchmark harnesses, or "
            "the focused Python contract for this harness."
        ),
    )
    parser.add_argument(
        "--ivf-k",
        type=int,
        help="force compaction-time coarse cell count (scale experiments only)",
    )
    parser.add_argument(
        "--nprobe",
        type=int,
        help="force query-time coarse cells (scale experiments only)",
    )
    args = parser.parse_args()
    if args.write_buffer_mb <= 0:
        raise RuntimeError("--write-buffer-mb must be positive")
    if args.flush_vector_threshold <= 0:
        raise RuntimeError("--flush-vector-threshold must be positive")
    if args.explicit_flush_every_rows is not None:
        if args.explicit_flush_every_rows <= 0:
            raise RuntimeError("--explicit-flush-every-rows must be positive")
        if args.explicit_flush_every_rows % args.batch_size:
            raise RuntimeError(
                "--explicit-flush-every-rows must be a multiple of --batch-size"
            )
        if args.rows % args.explicit_flush_every_rows:
            raise RuntimeError(
                "--rows must be a multiple of --explicit-flush-every-rows"
            )
    if args.ivf_k is not None and args.ivf_k <= 0:
        raise RuntimeError("--ivf-k must be positive")
    if args.nprobe is not None and args.nprobe <= 0:
        raise RuntimeError("--nprobe must be positive")
    if args.azurite and not (
        args.storage_url
        and urlparse(args.storage_url).scheme in {"adls", "az", "azure"}
    ):
        raise RuntimeError("--azurite requires --storage-url adls://...")
    if args.azurite and not os.environ.get("AZURE_STORAGE_CONNECTION_STRING"):
        raise RuntimeError(
            "--azurite requires AZURE_STORAGE_CONNECTION_STRING for geometry"
        )

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
    binary_source_revision = args.binary_source_revision or git_revision
    subprocess.run(
        [
            "git",
            "merge-base",
            "--is-ancestor",
            binary_source_revision,
            git_revision,
        ],
        check=True,
    )
    if binary_source_revision != git_revision:
        changed_since_build = subprocess.run(
            [
                "git",
                "diff",
                "--name-only",
                f"{binary_source_revision}..{git_revision}",
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        unsafe_changes = [
            path for path in changed_since_build
            if not path.startswith(("docs/", "scripts/"))
            and path != "tests/python/test_sift_get_reduction_harness.py"
        ]
        if unsafe_changes:
            raise RuntimeError(
                "binary source revision differs from executable source: "
                f"{unsafe_changes}; rebuild the release binary"
            )

    base_path = args.sift_dir / "sift_base.fvecs"
    query_path = args.sift_dir / "sift_query.fvecs"
    base_count, base_dimension = count_fixed_records(base_path, 4)
    groundtruth_path = (
        args.groundtruth_path.resolve()
        if args.groundtruth_path is not None
        else args.sift_dir / "sift_groundtruth.ivecs"
    )
    if args.groundtruth_path is not None and args.groundtruth_scope_rows is None:
        raise RuntimeError(
            "--groundtruth-scope-rows is required with --groundtruth-path"
        )
    groundtruth_scope = (
        args.groundtruth_scope_rows
        if args.groundtruth_scope_rows is not None
        else base_count
    )
    require_groundtruth_scope(args.rows, groundtruth_scope)
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
    storage_url = args.storage_url or f"file://{root / 'data' / 'sst'}"
    storage_scheme = urlparse(storage_url).scheme
    if storage_scheme not in {"file", "adls", "az", "azure"}:
        raise RuntimeError(
            f"unsupported benchmark storage URL scheme: {storage_scheme}"
        )
    azure_geometry = (
        AzureCliPaxGeometry(storage_url, root / "azure-pax-snapshot")
        if storage_scheme in {"adls", "az", "azure"}
        else None
    )
    if azure_geometry is not None:
        azure_geometry.require_empty_prefix()
    if (
        args.explicit_flush_every_rows is not None
        and azure_geometry is None
    ):
        raise RuntimeError(
            "--explicit-flush-every-rows requires Azure/Azurite storage so "
            "each durable PAX epoch can be proven by blob identity"
        )
    config = root / "benchmark.toml"
    write_config(
        config,
        root,
        args.port,
        args.write_buffer_mb,
        args.flush_vector_threshold,
        storage_url,
    )
    server_url = f"http://127.0.0.1:{args.port}"
    local_disk = root / "local-disk-cache"
    backend_profile = (
        "azure_blob_azurite"
        if args.azurite
        else (
            "azure_blob"
            if azure_geometry is not None
            else "local_file"
        )
    )
    filesystem_note = (
        "Azure backend and HTTP request-path evidence against Azurite. "
        "GET counters are emitted by the measured ProximaDB process. "
        "Out-of-process Azure CLI inventory/footer reads are excluded from "
        "those counters but may warm the emulator host page cache; latency is "
        "local-emulator evidence, not production Azure WAN evidence."
        if args.azurite
        else (
            "GET count is physical-I/O-seam evidence. Latency is local "
            "filesystem evidence, not Azure WAN evidence."
            if azure_geometry is None
            else (
                "Azure backend evidence. Out-of-process Azure CLI inventory/"
                "footer reads are excluded from the measured server counters."
            )
        )
    )
    result = {
        "protocol": "sift1m_get_reduction_v2",
        "git_revision": git_revision,
        "binary": {
            "path": str(binary),
            "sha256": sha256(binary),
            "bytes": binary.stat().st_size,
            "source_revision": binary_source_revision,
            "profile": (
                "release-server"
                if "/target/release-server/" in binary_text
                else "release"
            ),
        },
        "harness": {
            "git_revision": git_revision,
            "path": str(Path(__file__).resolve()),
            "sha256": sha256(Path(__file__).resolve()),
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
            "groundtruth": str(groundtruth_path),
            "groundtruth_scope": groundtruth_scope,
        },
        "filesystem_profile": {
            "segment_backend": backend_profile,
            "storage_url": storage_url,
            "azurite": args.azurite,
            "geometry_evidence": (
                "azure_cli_inventory_and_footer_snapshot"
                if azure_geometry is not None
                else "local_pax_footer"
            ),
            "local_disk_path": str(local_disk),
            "range_planner": {
                "profile": "azure",
                "max_gap_bytes": AZURE_COALESCE_GAP_BYTES,
                "max_range_bytes": AZURE_COALESCE_RANGE_BYTES,
            },
            "note": filesystem_note,
        },
        "probe_policy": {
            "ivf_k_override": args.ivf_k,
            "nprobe_override": args.nprobe,
            "required_layout_version": args.require_layout_version,
        },
        "ingest_config": {
            "batch_size": args.batch_size,
            "write_buffer_mb": args.write_buffer_mb,
            "flush_vector_threshold": args.flush_vector_threshold,
            "explicit_flush_every_rows": args.explicit_flush_every_rows,
            "explicit_flush_flight_port": (
                args.port if args.explicit_flush_every_rows is not None else None
            ),
        },
        "thresholds": {
            "post_write_max_gets_per_query": args.post_write_max_gets,
            "local_disk_warm_max_gets_per_query": args.local_warm_max_gets,
            "object_cold_max_gets_per_query": args.object_cold_max_gets,
            "min_recall_at_k": args.min_recall,
            "post_write_max_p50_ms": args.max_p50_ms,
            "local_disk_warm_max_p50_ms": args.local_warm_max_p50_ms,
            "object_cold_max_p50_ms": args.max_p50_ms,
            "max_segments": args.max_segments,
        },
        "phases": {},
    }

    active: OwnedServer | None = None
    failures: list[str] = []
    try:
        active = OwnedServer(
            binary,
            config,
            server_url,
            root / "server-ingest.log",
            local_disk,
            args.ivf_k,
            args.nprobe,
            args.azurite,
        )
        active.start()
        (
            collection_id,
            ingest_seconds,
            retry_status_counts,
            explicit_flushes,
            explicit_compaction,
        ) = ingest(
            server_url,
            base_path,
            args.rows,
            args.batch_size,
            "127.0.0.1",
            args.port,
            args.explicit_flush_every_rows,
            azure_geometry,
        )
        result["collection_id"] = collection_id
        result["ingest"] = {
            "seconds": ingest_seconds,
            "vectors_per_second": args.rows / ingest_seconds,
            "retry_status_counts": retry_status_counts,
            "explicit_flushes": explicit_flushes,
            "explicit_compaction": explicit_compaction,
        }
        geometry = wait_for_materialization(
            root / "data" / "sst",
            server_url,
            collection_id,
            args.rows,
            args.max_segments,
            args.settle_timeout_secs,
            args.stable_secs,
            azure_geometry,
        )
        wrong_layouts = [
            segment for segment in geometry["segments"]
            if segment["layout_version"] != args.require_layout_version
        ]
        if wrong_layouts:
            raise RuntimeError(
                "settled segment layout mismatch: "
                f"required v{args.require_layout_version}, got {wrong_layouts}"
            )
        if args.ivf_k is not None:
            wrong_cells = [
                segment for segment in geometry["segments"]
                if segment.get("coarse_cells") != args.ivf_k
            ]
            if wrong_cells:
                raise RuntimeError(
                    f"forced ivf_k={args.ivf_k} was not persisted: "
                    f"{wrong_cells}"
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
            args.ivf_k,
            args.nprobe,
            args.azurite,
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
            args.local_warm_max_p50_ms,
            require_local_hit=True,
        ))
        active.stop()

        active = OwnedServer(
            binary,
            config,
            server_url,
            root / "server-object-cold.log",
            None,
            args.ivf_k,
            args.nprobe,
            args.azurite,
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
            args.object_cold_max_gets,
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
