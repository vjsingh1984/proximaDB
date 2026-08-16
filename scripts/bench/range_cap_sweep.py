#!/usr/bin/env python3
"""Query-only Azure range-cap sweep over one immutable PAX bed.

The experiment holds the coalescing gap, bed, nprobe, query slice, and ground
truth fixed while changing only the maximum application-issued range. Every
``(range_cap, top_k)`` point owns a fresh server process. Application counters
are reconciled with requests observed at the Azurite HTTP boundary, because a
counter above ``object_store.get_range`` cannot reveal SDK splitting or retry.

Start the dedicated Azurite bed with ``azurite-blob --debug <path>`` and pass
that same append-only log through ``--azurite-debug-log``. Azurite is wire-shape
evidence, not production Azure latency evidence.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import threading
from pathlib import Path
from urllib.parse import unquote, urlparse

SCRIPT_ROOT = Path(__file__).resolve().parent


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load benchmark module: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ACCEPTANCE = load_module(
    "range_cap_acceptance", SCRIPT_ROOT / "sift1m_get_reduction.py"
)
NPROBE = load_module("range_cap_nprobe", SCRIPT_ROOT / "nprobe_sweep.py")
MIB = 1024 * 1024
REQUEST_LINE = re.compile(
    r"^\S+\s+(?P<request_id>\S+)\s+info:\s+BlobStorageContextMiddleware:\s+"
    r"RequestMethod=(?P<method>\S+)\s+RequestURL=(?P<url>\S+)\s+"
    r"RequestHeaders:(?P<headers>\{.*\})(?:\s+ClientIP=|$)"
)
RANGE_HEADER = re.compile(r"^bytes=(?P<start>\d+)-(?P<end>\d+)$")
COMPILER_COMMANDS = {"cargo", "cargo-clippy", "cargo-nextest", "rustc"}


def cap_mib_values(raw: str) -> list[int]:
    values = NPROBE.comma_separated_ints(raw, "--range-caps-mib")
    return values


def azure_storage_scope(storage_url: str) -> tuple[str, str]:
    parsed = urlparse(storage_url)
    if parsed.scheme != "az" or not parsed.netloc or not parsed.path.strip("/"):
        raise RuntimeError("range-cap sweep requires canonical az://container/prefix")
    return parsed.netloc, parsed.path.strip("/")


class AzuriteWireLog:
    """Observe blob GETs received by Azurite after one byte offset."""

    def __init__(self, path: Path, container: str, prefix: str):
        self.path = path
        self.scope = f"/{container}/{prefix.rstrip('/')}"

    def snapshot(self) -> int:
        if not self.path.is_file():
            raise RuntimeError(f"Azurite debug log not found: {self.path}")
        return self.path.stat().st_size

    def sample(self, offset: int) -> dict:
        size = self.snapshot()
        if size < offset:
            raise RuntimeError("Azurite debug log was truncated during one point")
        with self.path.open("rb") as source:
            source.seek(offset)
            text = source.read().decode("utf-8", errors="replace")
        requests: dict[str, dict] = {}
        for line in text.splitlines():
            match = REQUEST_LINE.match(line)
            if match is None:
                continue
            request_path = unquote(urlparse(match.group("url")).path)
            # Emulator URLs include /devstoreaccount1 before container. A
            # suffix/subpath match also accepts product-style host URLs.
            if not (
                request_path.endswith(self.scope) or f"{self.scope}/" in request_path
            ):
                continue
            try:
                headers = json.loads(match.group("headers"))
            except json.JSONDecodeError as error:
                raise RuntimeError(
                    f"invalid Azurite request headers: {line}"
                ) from error
            method = match.group("method")
            raw_range = headers.get("range") or headers.get("Range")
            range_bytes = None
            if raw_range is not None:
                range_match = RANGE_HEADER.match(raw_range)
                if range_match is None:
                    raise RuntimeError(f"unsupported Azurite Range header: {raw_range}")
                start = int(range_match.group("start"))
                end = int(range_match.group("end"))
                if end < start:
                    raise RuntimeError(f"backwards Azurite Range header: {raw_range}")
                range_bytes = end - start + 1
            requests[match.group("request_id")] = {
                "method": method,
                "range_bytes": range_bytes,
                "path": request_path,
            }
        get_requests = {
            request_id: request
            for request_id, request in requests.items()
            if request["method"] == "GET"
        }
        range_lengths = [
            request["range_bytes"]
            for request in get_requests.values()
            if request["range_bytes"] is not None
        ]
        full_gets = len(get_requests) - len(range_lengths)
        methods = {
            method: sum(request["method"] == method for request in requests.values())
            for method in sorted({request["method"] for request in requests.values()})
        }
        return {
            "observer": "azurite_server_debug_log",
            "log_path": str(self.path),
            "log_offset": [offset, size],
            "scope": self.scope,
            "http_requests": len(requests),
            "requests_by_method": methods,
            "get_requests": len(get_requests),
            "range_get_requests": len(range_lengths),
            "full_get_requests": full_gets,
            "requested_range_bytes": sum(range_lengths),
            "min_requested_range_bytes": min(range_lengths) if range_lengths else None,
            "max_requested_range_bytes": max(range_lengths) if range_lengths else None,
            "unique_request_ids": len(requests),
        }


class RssSampler:
    """Low-frequency child RSS sampler; records observer overhead explicitly."""

    def __init__(self, pid: int, interval_seconds: float = 0.25):
        self.pid = pid
        self.interval_seconds = interval_seconds
        self.samples_kib: list[int] = []
        self.stop_event = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _sample(self) -> None:
        completed = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(self.pid)],
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            return
        try:
            self.samples_kib.append(int(completed.stdout.strip()))
        except ValueError:
            return

    def _run(self) -> None:
        while not self.stop_event.is_set():
            self._sample()
            self.stop_event.wait(self.interval_seconds)

    def start(self) -> None:
        self._sample()
        self.thread.start()

    def stop(self) -> dict:
        self.stop_event.set()
        self.thread.join(timeout=max(1.0, self.interval_seconds * 4))
        self._sample()
        return {
            "observer": "ps_rss",
            "sampling_interval_seconds": self.interval_seconds,
            "samples": len(self.samples_kib),
            "baseline_bytes": self.samples_kib[0] * 1024 if self.samples_kib else None,
            "peak_bytes": max(self.samples_kib) * 1024 if self.samples_kib else None,
        }


def compiler_processes_from_ps(output: str) -> list[dict]:
    conflicts = []
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=1)
        if len(fields) != 2:
            continue
        try:
            pid = int(fields[0])
        except ValueError:
            continue
        command = Path(fields[1]).name
        if command in COMPILER_COMMANDS:
            conflicts.append({"pid": pid, "command": command})
    return conflicts


class HostContentionMonitor:
    """Fail a latency/RSS point if an external Rust build overlaps it."""

    def __init__(self, interval_seconds: float = 1.0):
        self.interval_seconds = interval_seconds
        self.conflicts: list[dict] = []
        self.samples = 0
        self.stop_event = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _sample(self) -> None:
        completed = subprocess.run(
            ["ps", "-Ao", "pid=,comm="],
            check=False,
            capture_output=True,
            text=True,
        )
        self.samples += 1
        if completed.returncode != 0:
            return
        observed = compiler_processes_from_ps(completed.stdout)
        if observed:
            known = {(item["pid"], item["command"]) for item in self.conflicts}
            self.conflicts.extend(
                item
                for item in observed
                if (item["pid"], item["command"]) not in known
            )

    def _run(self) -> None:
        while not self.stop_event.wait(self.interval_seconds):
            self._sample()

    def start(self) -> None:
        self._sample()
        self.raise_if_conflict()
        self.thread.start()

    def raise_if_conflict(self) -> None:
        if not self.conflicts:
            return
        identities = ", ".join(
            f"{item['command']} pid={item['pid']}" for item in self.conflicts
        )
        raise RuntimeError(f"host compiler contention observed: {identities}")

    def stop(self) -> dict:
        self.stop_event.set()
        if self.thread.is_alive():
            self.thread.join(timeout=max(1.0, self.interval_seconds * 2))
        self._sample()
        return {
            "observer": "ps_compiler_process_monitor",
            "sampling_interval_seconds": self.interval_seconds,
            "samples": self.samples,
            "conflicts": self.conflicts,
        }


def decision_for(candidate: dict, baseline: dict, args: argparse.Namespace) -> dict:
    wire = candidate["wire_http"]["get_requests"]
    base_wire = baseline["wire_http"]["get_requests"]
    wire_ranges = candidate["wire_http"]["range_get_requests"]
    app = candidate["physical_gets"]
    recall_delta = candidate["recall_at_k"] - baseline["recall_at_k"]
    recall_noninferior = recall_delta >= -args.max_recall_regression
    wire_reduction = 1.0 - (wire / base_wire) if base_wire else 0.0
    bytes_ratio = candidate["bytes_read"] / baseline["bytes_read"]
    p50_ratio = candidate["latency_ms"]["p50"] / baseline["latency_ms"]["p50"]
    p95_ratio = candidate["latency_ms"]["p95"] / baseline["latency_ms"]["p95"]
    candidate_peak = candidate["process_rss"]["peak_bytes"]
    baseline_peak = baseline["process_rss"]["peak_bytes"]
    rss_ratio = (
        candidate_peak / baseline_peak
        if candidate_peak is not None and baseline_peak
        else None
    )
    # The server issues application-counted byte-range reads. One additional
    # full-object GET may be control-plane/catalog work and is still billed,
    # but it is not evidence that the SDK split one ranged read. Compare the
    # range requests for the SDK-splitting gate while retaining total GETs for
    # the economic reduction calculation.
    one_wire_range_per_application_get = abs(wire_ranges - app) < 0.5
    baseline_identity = baseline.get("result_identity")
    candidate_identity = candidate.get("result_identity")
    identity_diagnostics = None
    if isinstance(baseline_identity, dict) and isinstance(candidate_identity, dict):
        identity_diagnostics = {}
        for label, key in (
            ("ordered_result", "ordered_ids_sha256_by_query"),
            ("result_set", "set_ids_sha256_by_query"),
            ("recall_hits", "recall_hits_by_query"),
        ):
            baseline_values = baseline_identity.get(key)
            candidate_values = candidate_identity.get(key)
            if not isinstance(baseline_values, list) or not isinstance(
                candidate_values, list
            ):
                raise RuntimeError(f"result identity is missing {key}")
            if len(baseline_values) != len(candidate_values):
                raise RuntimeError(f"result identity length differs for {key}")
            mismatches = [
                index
                for index, (before, after) in enumerate(
                    zip(baseline_values, candidate_values, strict=True)
                )
                if before != after
            ]
            identity_diagnostics[f"{label}_mismatch_count"] = len(mismatches)
            identity_diagnostics[f"{label}_first_mismatch_queries"] = mismatches[:20]
            if label == "recall_hits":
                deltas = [
                    after - before
                    for before, after in zip(
                        baseline_values, candidate_values, strict=True
                    )
                ]
                identity_diagnostics["recall_hit_delta_total"] = sum(deltas)
                identity_diagnostics["queries_with_fewer_recall_hits"] = sum(
                    delta < 0 for delta in deltas
                )
                identity_diagnostics["queries_with_more_recall_hits"] = sum(
                    delta > 0 for delta in deltas
                )
    checks = {
        "recall_noninferior": recall_noninferior,
        "target_recall_maintained": candidate["recall_at_k"] >= args.target_recall,
        "wire_get_reduction_material": wire_reduction >= args.min_wire_get_reduction,
        "byte_amplification_bounded": bytes_ratio <= args.max_byte_amplification,
        "p50_not_materially_regressed": p50_ratio <= args.max_latency_ratio,
        "p95_not_materially_regressed": p95_ratio <= args.max_latency_ratio,
        "rss_not_materially_regressed": (
            rss_ratio is not None and rss_ratio <= args.max_rss_ratio
        ),
        "one_wire_range_get_per_application_get": one_wire_range_per_application_get,
    }
    return {
        "baseline_cap_mib": baseline["range_cap_mib"],
        "recall_delta": recall_delta,
        "wire_get_reduction": wire_reduction,
        "bytes_ratio": bytes_ratio,
        "p50_ratio": p50_ratio,
        "p95_ratio": p95_ratio,
        "rss_ratio": rss_ratio,
        "wire_to_application_get_ratio": wire / app if app else None,
        "wire_range_to_application_get_ratio": wire_ranges / app if app else None,
        "result_identity_diagnostics": identity_diagnostics,
        "checks": checks,
        "promotion_eligible": all(checks.values()),
    }


def validate_wire_observation(label: str, point: dict) -> None:
    """Fail closed when application reads occurred but the wire observer was dead."""
    app_gets = point["physical_gets"]
    wire_gets = point["wire_http"]["get_requests"]
    if app_gets > 0 and wire_gets == 0:
        raise RuntimeError(
            f"{label}: application counted {app_gets:.0f} GETs but Azurite "
            "observed zero HTTP GETs; debug log is stale or disconnected"
        )


def checkpoint_identity(result: dict) -> dict:
    experiment = result["experiment"]
    return {
        "protocol": result["protocol"],
        "git_revision": result["git_revision"],
        "collection_id": result["collection_id"],
        "binary": result["binary"],
        "bed_config": result["bed_config"],
        "dataset": result["dataset"],
        "filesystem_profile": result["filesystem_profile"],
        "compute_profile": result["compute_profile"],
        "settled_geometry": NPROBE.stable_geometry(result["settled_geometry"]),
        "experiment_config": {
            key: experiment[key]
            for key in (
                "isolated_variable",
                "fixed_nprobe",
                "fixed_coalesce_gap_bytes",
                "range_caps_mib",
                "top_k_values",
                "fresh_process_per_point",
                "target_recall",
                "decision_thresholds",
            )
        },
    }


def validate_resume(existing: dict, expected: dict) -> set[tuple[int, int]]:
    if existing.get("status") not in {"running", "incomplete"}:
        raise RuntimeError("range-cap checkpoint is terminal; refusing resume")
    if checkpoint_identity(existing) != checkpoint_identity(expected):
        raise RuntimeError("range-cap checkpoint provenance/configuration differs")
    expected_pairs = {
        (cap, top_k)
        for cap in expected["experiment"]["range_caps_mib"]
        for top_k in expected["experiment"]["top_k_values"]
    }
    completed = set()
    for point in existing["experiment"]["points"]:
        identity = (int(point["range_cap_mib"]), int(point["top_k"]))
        if identity not in expected_pairs or identity in completed:
            raise RuntimeError(f"invalid range-cap checkpoint point: {identity}")
        completed.add(identity)
    return completed


def write_checkpoint(
    output: Path, result: dict, state: str, reason: str | None = None
) -> None:
    expected_points = len(result["experiment"]["range_caps_mib"]) * len(
        result["experiment"]["top_k_values"]
    )
    result["status"] = state
    result["checkpoint"] = {
        "state": state,
        "completed_points": len(result["experiment"]["points"]),
        "expected_points": expected_points,
        "incomplete_reason": reason,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp")
    temporary.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    temporary.replace(output)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--binary-source-revision", required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--run-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--collection-id", required=True)
    parser.add_argument("--storage-url", required=True)
    parser.add_argument("--azurite-debug-log", type=Path, required=True)
    parser.add_argument("--sift-dir", type=Path, required=True)
    parser.add_argument("--base-path", type=Path)
    parser.add_argument("--base-format", choices=("fvecs", "u8bin"), default="fvecs")
    parser.add_argument("--query-path", type=Path)
    parser.add_argument("--query-format", choices=("fvecs", "u8bin"), default="fvecs")
    parser.add_argument("--groundtruth-path", type=Path, required=True)
    parser.add_argument(
        "--groundtruth-format", choices=("ivecs", "bigann-bin"), default="ivecs"
    )
    parser.add_argument("--groundtruth-scope-rows", type=int, required=True)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--nprobe", type=int, default=12)
    parser.add_argument("--range-caps-mib", default="4,8,16,32")
    parser.add_argument("--coalesce-gap-mib", type=int, default=1)
    parser.add_argument("--top-k-values", default="10,20")
    parser.add_argument("--queries", type=int, default=1_000)
    parser.add_argument("--query-start", type=int, default=0)
    parser.add_argument("--port", type=int, default=5690)
    parser.add_argument("--max-segments", type=int, default=1)
    parser.add_argument("--required-layout-version", type=int, default=3)
    parser.add_argument("--target-recall", type=float, default=0.98)
    parser.add_argument("--max-recall-regression", type=float, default=0.0005)
    parser.add_argument("--min-wire-get-reduction", type=float, default=0.20)
    parser.add_argument("--max-byte-amplification", type=float, default=1.50)
    parser.add_argument("--max-latency-ratio", type=float, default=1.10)
    parser.add_argument("--max-rss-ratio", type=float, default=1.10)
    args = parser.parse_args()

    repository = Path(__file__).resolve().parents[2]
    binary = args.binary.resolve()
    config = args.config.resolve()
    run_root = args.run_root.resolve()
    output = args.output.resolve()
    _matrix_lock = NPROBE.acquire_matrix_lock(output)
    caps = cap_mib_values(args.range_caps_mib)
    top_k_values = NPROBE.comma_separated_ints(args.top_k_values, "--top-k-values")
    if 4 not in caps:
        raise RuntimeError("--range-caps-mib must include the 4 MiB baseline")
    if args.coalesce_gap_mib < 0 or args.nprobe <= 0:
        raise RuntimeError("coalescing gap must be non-negative and nprobe positive")
    if args.max_recall_regression < 0:
        raise RuntimeError("--max-recall-regression must be non-negative")
    if args.groundtruth_scope_rows != args.rows:
        raise RuntimeError("ground-truth scope must equal measured corpus rows")
    if not args.collection_id.isdecimal():
        raise RuntimeError("--collection-id must be a decimal catalog object id")
    NPROBE.require_config_port(config, args.port)
    current_revision, profile = NPROBE.require_release_provenance(
        repository, binary, args.binary_source_revision
    )
    container, prefix = azure_storage_scope(args.storage_url)
    wire_log = AzuriteWireLog(args.azurite_debug_log.resolve(), container, prefix)
    wire_log.snapshot()

    base_path = (
        args.base_path.resolve()
        if args.base_path is not None
        else args.sift_dir.resolve() / "sift_base.fvecs"
    )
    query_path = (
        args.query_path.resolve()
        if args.query_path is not None
        else args.sift_dir.resolve() / "sift_query.fvecs"
    )
    base_count, dimension, base_declared = ACCEPTANCE.vector_source_geometry(
        base_path, args.base_format
    )
    query_count, query_dimension, query_declared = ACCEPTANCE.vector_source_geometry(
        query_path, args.query_format
    )
    truth_count, truth_width = ACCEPTANCE.count_truth_records(
        args.groundtruth_path.resolve(), args.groundtruth_format
    )
    if args.rows > base_count or query_dimension != dimension:
        raise RuntimeError("dataset cardinality or dimension does not match the bed")
    query_end = args.query_start + args.queries
    if query_end > query_count or query_end > truth_count:
        raise RuntimeError("query slice exceeds query or ground-truth rows")
    if max(top_k_values) > truth_width:
        raise RuntimeError("top-k exceeds ground-truth width")

    run_root.mkdir(parents=True, exist_ok=True)
    geometry_reader = ACCEPTANCE.AzureCliPaxGeometry(
        args.storage_url, run_root / "pax-snapshot"
    )
    geometry = geometry_reader.materialize(geometry_reader.inventory())
    NPROBE.validate_geometry(
        geometry, args.rows, args.max_segments, args.required_layout_version
    )
    max_cells = max(segment["coarse_cells"] for segment in geometry["segments"])
    if args.nprobe > max_cells:
        raise RuntimeError(
            f"nprobe={args.nprobe} exceeds persisted max k_c={max_cells}"
        )

    result = {
        "protocol": "pax_azure_range_cap_sweep",
        "status": "running",
        "git_revision": current_revision,
        "collection_id": args.collection_id,
        "binary": {
            "path": str(binary),
            "sha256": ACCEPTANCE.sha256(binary),
            "bytes": binary.stat().st_size,
            "source_revision": args.binary_source_revision,
            "profile": profile,
        },
        "bed_config": {"path": str(config), "sha256": ACCEPTANCE.sha256(config)},
        "dataset": {
            "base": str(base_path),
            "base_format": args.base_format,
            "corpus_rows": args.rows,
            "base_available_rows": base_count,
            "base_declared_rows": base_declared,
            "dimension": dimension,
            "queries_path": str(query_path),
            "query_format": args.query_format,
            "query_available_rows": query_count,
            "query_declared_rows": query_declared,
            "groundtruth": str(args.groundtruth_path.resolve()),
            "groundtruth_format": args.groundtruth_format,
            "groundtruth_scope_rows": args.groundtruth_scope_rows,
            "groundtruth_width": truth_width,
            "query_range": [args.query_start, query_end],
        },
        "filesystem_profile": {
            "storage_url": args.storage_url,
            "backend": "azure_blob_via_azurite",
            "wire_observer": "azurite_server_debug_log",
            "persistent_local_tier": False,
            "note": (
                "Azurite proves HTTP request shape/count at the emulator boundary; "
                "it is not production Azure latency evidence."
            ),
        },
        "compute_profile": ACCEPTANCE.compute_profile(),
        "settled_geometry": geometry,
        "experiment": {
            "isolated_variable": "max_coalesced_range_bytes",
            "fixed_nprobe": args.nprobe,
            "fixed_coalesce_gap_bytes": args.coalesce_gap_mib * MIB,
            "range_caps_mib": caps,
            "top_k_values": top_k_values,
            "fresh_process_per_point": True,
            "target_recall": args.target_recall,
            "decision_thresholds": {
                "max_recall_regression": args.max_recall_regression,
                "min_wire_get_reduction": args.min_wire_get_reduction,
                "max_byte_amplification": args.max_byte_amplification,
                "max_latency_ratio": args.max_latency_ratio,
                "max_rss_ratio": args.max_rss_ratio,
            },
            "points": [],
        },
        "measurement_failures": [],
        "decisions": [],
    }

    existing = NPROBE.load_resumable_checkpoint(output)
    if existing is not None:
        completed = validate_resume(existing, result)
        result = existing
        result["checkpoint"]["incomplete_reason"] = None
    else:
        completed = set()
        write_checkpoint(output, result, "running")

    server_url = f"http://127.0.0.1:{args.port}"
    for cap_mib in caps:
        for top_k in top_k_values:
            if (cap_mib, top_k) in completed:
                continue
            label = f"range-{cap_mib}mib-top-{top_k}"
            server = ACCEPTANCE.OwnedServer(
                binary=binary,
                config=config,
                server=server_url,
                log_path=run_root / f"{label}.log",
                local_disk_path=None,
                nprobe=args.nprobe,
                azure_emulator=True,
                coalesce_gap_bytes=args.coalesce_gap_mib * MIB,
                coalesce_range_bytes=cap_mib * MIB,
            )
            sampler = None
            contention = HostContentionMonitor()
            try:
                contention.start()
                server.start()
                if server.process is None:
                    raise RuntimeError("owned server did not expose its process")
                offset = wire_log.snapshot()
                sampler = RssSampler(server.process.pid)
                sampler.start()
                point = ACCEPTANCE.run_query_sweep(
                    server_url,
                    args.collection_id,
                    query_path,
                    args.groundtruth_path.resolve(),
                    args.query_start,
                    args.queries,
                    top_k,
                    label,
                    args.query_format,
                    args.groundtruth_format,
                    contention.raise_if_conflict,
                )
                point["process_rss"] = sampler.stop()
                sampler = None
                point["host_contention"] = contention.stop()
                contention.raise_if_conflict()
                point["wire_http"] = wire_log.sample(offset)
                validate_wire_observation(label, point)
            except (Exception, KeyboardInterrupt) as error:
                write_checkpoint(
                    output,
                    result,
                    "incomplete",
                    f"{label}: {type(error).__name__}: {error}",
                )
                raise
            finally:
                if sampler is not None:
                    sampler.stop()
                contention.stop()
                server.stop()
            point["range_cap_mib"] = cap_mib
            point["coalesce_gap_mib"] = args.coalesce_gap_mib
            point["nprobe"] = args.nprobe
            point["wire_gets_per_query"] = (
                point["wire_http"]["get_requests"] / args.queries
            )
            point["wire_to_application_get_ratio"] = (
                point["wire_http"]["get_requests"] / point["physical_gets"]
                if point["physical_gets"]
                else None
            )
            point["wire_range_to_application_get_ratio"] = (
                point["wire_http"]["range_get_requests"] / point["physical_gets"]
                if point["physical_gets"]
                else None
            )
            if attribution_failure := ACCEPTANCE.ivf_byte_attribution_failure(
                label, point
            ):
                result["measurement_failures"].append(attribution_failure)
            result["experiment"]["points"].append(point)
            completed.add((cap_mib, top_k))
            write_checkpoint(output, result, "running")

    for top_k in top_k_values:
        baseline = next(
            point
            for point in result["experiment"]["points"]
            if point["range_cap_mib"] == 4 and point["top_k"] == top_k
        )
        for candidate in result["experiment"]["points"]:
            if candidate["top_k"] == top_k and candidate["range_cap_mib"] != 4:
                result["decisions"].append(
                    {
                        "top_k": top_k,
                        "candidate_cap_mib": candidate["range_cap_mib"],
                        **decision_for(candidate, baseline, args),
                    }
                )
    status = "fail" if result["measurement_failures"] else "pass"
    write_checkpoint(output, result, status)
    print(f"range-cap result: {output}", flush=True)
    return 1 if status == "fail" else 0


if __name__ == "__main__":
    raise SystemExit(main())
