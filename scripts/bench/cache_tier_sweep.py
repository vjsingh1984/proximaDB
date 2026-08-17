#!/usr/bin/env python3
"""Measure object-cold, DRAM-warm, and persistent-disk-warm PAX reads.

Every measured phase searches the same query slice.  DRAM is warmed with a
disjoint slice so a process-local query-result cache cannot impersonate a PAX
range-cache hit.  The persistent tier is populated in measured-then-warmup
order: the second, disjoint slice exerts eviction pressure on the first slice,
spilling its ranges before the server restarts with empty DRAM.

Azurite's append-only debug log is reconciled with application counters for
each measured phase.  It proves HTTP request shape, not production Azure
latency.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path

SCRIPT_ROOT = Path(__file__).resolve().parent
MIB = 1024 * 1024
AZURE_HOT_READ_USD_PER_10K = 0.005


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load benchmark module: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ACCEPTANCE = load_module(
    "cache_tier_acceptance", SCRIPT_ROOT / "sift1m_get_reduction.py"
)
NPROBE = load_module("cache_tier_nprobe", SCRIPT_ROOT / "nprobe_sweep.py")
RANGE = load_module("cache_tier_range", SCRIPT_ROOT / "range_cap_sweep.py")


def query_slices(
    query_start: int,
    warmup_queries: int,
    measured_queries: int,
    available_queries: int,
    available_truth: int,
) -> dict[str, list[int]]:
    if query_start < 0:
        raise RuntimeError("query start must be non-negative")
    if warmup_queries <= 0 or measured_queries <= 0:
        raise RuntimeError("warmup and measured query counts must be positive")
    warmup_end = query_start + warmup_queries
    measured_end = warmup_end + measured_queries
    if measured_end > available_queries or measured_end > available_truth:
        raise RuntimeError("warmup plus measured query slice exceeds available rows")
    return {
        "warmup": [query_start, warmup_end],
        "measured": [warmup_end, measured_end],
    }


def disk_population_order() -> tuple[str, str]:
    """Order that makes measured ranges eligible for L1-to-disk spill."""
    return ("measured", "warmup")


def disk_path_for_attempt(run_root: Path, attempt: int) -> Path:
    if attempt < 0:
        raise RuntimeError("attempt must be non-negative")
    return run_root / f"local-disk-cache-attempt-{attempt}"


def hit_ratio(hits: float, misses: float) -> float | None:
    total = hits + misses
    return hits / total if total else None


def add_cache_ratios(point: dict) -> dict:
    for tier in ("survivor", "invariants", "local_disk"):
        counters = point[tier]
        counters["hit_ratio"] = hit_ratio(counters["hits"], counters["misses"])
    return point


def compare_phase(candidate: dict, baseline: dict, query_count: int) -> dict:
    if query_count <= 0:
        raise RuntimeError("query count must be positive")
    candidate_gets = candidate["physical_gets"]
    baseline_gets = baseline["physical_gets"]
    candidate_bytes = candidate["bytes_read"]
    baseline_bytes = baseline["bytes_read"]
    candidate_gets_per_query = candidate_gets / query_count
    return {
        "get_reduction": (
            1.0 - candidate_gets / baseline_gets if baseline_gets else None
        ),
        "byte_reduction": (
            1.0 - candidate_bytes / baseline_bytes if baseline_bytes else None
        ),
        "p50_ratio": (candidate["latency_ms"]["p50"] / baseline["latency_ms"]["p50"]),
        "p95_ratio": (candidate["latency_ms"]["p95"] / baseline["latency_ms"]["p95"]),
        "recall_delta": candidate["recall_at_k"] - baseline["recall_at_k"],
        "result_identity_equal": (
            candidate["result_identity"] == baseline["result_identity"]
        ),
        "azure_hot_read_cogs_per_million_queries_usd": (
            candidate_gets_per_query * 1_000_000 / 10_000 * AZURE_HOT_READ_USD_PER_10K
        ),
    }


def require_new_run(output: Path, run_root: Path) -> None:
    if output.exists():
        raise RuntimeError(f"output already exists: {output}")
    if run_root.exists() and any(run_root.iterdir()):
        raise RuntimeError(f"run root must be empty: {run_root}")
    run_root.mkdir(parents=True, exist_ok=True)


def write_result(output: Path, result: dict) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp")
    temporary.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    temporary.replace(output)


def run_measured_phase(
    *,
    label: str,
    server,
    server_url: str,
    collection_id: str,
    query_path: Path,
    groundtruth_path: Path,
    query_format: str,
    groundtruth_format: str,
    query_start: int,
    query_count: int,
    top_k: int,
    wire_log,
) -> dict:
    if server.process is None:
        raise RuntimeError("owned server did not expose its process")
    wire_offset = wire_log.snapshot()
    contention = RANGE.HostContentionMonitor()
    contention.start()
    sampler = RANGE.RssSampler(server.process.pid)
    sampler.start()
    try:
        point = ACCEPTANCE.run_query_sweep(
            server_url,
            collection_id,
            query_path,
            groundtruth_path,
            query_start,
            query_count,
            top_k,
            label,
            query_format,
            groundtruth_format,
            contention.raise_if_conflict,
        )
    finally:
        rss = sampler.stop()
        contention_observation = contention.stop()
    contention.raise_if_conflict()
    point["process_rss"] = rss
    point["host_contention"] = contention_observation
    point["wire_http"] = wire_log.sample(wire_offset)
    RANGE.validate_wire_observation(label, point)
    point["wire_gets_per_query"] = point["wire_http"]["get_requests"] / query_count
    point["wire_range_to_application_get_ratio"] = (
        point["wire_http"]["range_get_requests"] / point["physical_gets"]
        if point["physical_gets"]
        else None
    )
    return add_cache_ratios(point)


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
    parser.add_argument("--base-path", type=Path, required=True)
    parser.add_argument("--base-format", choices=("fvecs", "u8bin"), default="fvecs")
    parser.add_argument("--query-path", type=Path, required=True)
    parser.add_argument("--query-format", choices=("fvecs", "u8bin"), default="fvecs")
    parser.add_argument("--groundtruth-path", type=Path, required=True)
    parser.add_argument(
        "--groundtruth-format", choices=("ivecs", "bigann-bin"), default="ivecs"
    )
    parser.add_argument("--groundtruth-scope-rows", type=int, required=True)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--nprobe", type=int, required=True)
    parser.add_argument("--range-cap-mib", type=int, required=True)
    parser.add_argument("--coalesce-gap-mib", type=int, default=1)
    parser.add_argument("--top-k", type=int, default=10)
    parser.add_argument("--warmup-queries", type=int, default=500)
    parser.add_argument("--measured-queries", type=int, default=500)
    parser.add_argument("--query-start", type=int, default=0)
    parser.add_argument("--port", type=int, default=5790)
    parser.add_argument("--max-segments", type=int, default=1)
    parser.add_argument("--required-layout-version", type=int, default=3)
    parser.add_argument("--target-recall", type=float, default=0.98)
    parser.add_argument("--min-disk-get-reduction", type=float, default=0.20)
    parser.add_argument("--max-cache-latency-ratio", type=float, default=1.10)
    parser.add_argument("--host-quiet-window-secs", type=float, default=0)
    parser.add_argument("--host-quiet-timeout-secs", type=float, default=3600)
    parser.add_argument("--max-contention-retries", type=int, default=0)
    args = parser.parse_args()

    repository = Path(__file__).resolve().parents[2]
    binary = args.binary.resolve()
    config = args.config.resolve()
    run_root = args.run_root.resolve()
    output = args.output.resolve()
    require_new_run(output, run_root)
    _matrix_lock = NPROBE.acquire_matrix_lock(output)
    if not args.collection_id.isdecimal():
        raise RuntimeError("--collection-id must be a decimal catalog object id")
    if args.range_cap_mib <= 0 or args.nprobe <= 0:
        raise RuntimeError("range cap and nprobe must be positive")
    if args.coalesce_gap_mib < 0:
        raise RuntimeError("coalescing gap must be non-negative")
    if args.max_contention_retries < 0:
        raise RuntimeError("contention retries must be non-negative")
    if args.groundtruth_scope_rows != args.rows:
        raise RuntimeError("ground-truth scope must equal measured corpus rows")
    NPROBE.require_config_port(config, args.port)
    current_revision, profile = NPROBE.require_release_provenance(
        repository, binary, args.binary_source_revision
    )

    base_path = args.base_path.resolve()
    query_path = args.query_path.resolve()
    groundtruth_path = args.groundtruth_path.resolve()
    base_count, dimension, base_declared = ACCEPTANCE.vector_source_geometry(
        base_path, args.base_format
    )
    query_count, query_dimension, query_declared = ACCEPTANCE.vector_source_geometry(
        query_path, args.query_format
    )
    truth_count, truth_width = ACCEPTANCE.count_truth_records(
        groundtruth_path, args.groundtruth_format
    )
    if args.rows > base_count or query_dimension != dimension:
        raise RuntimeError("dataset cardinality or dimension does not match the bed")
    if args.top_k > truth_width:
        raise RuntimeError("top-k exceeds ground-truth width")
    slices = query_slices(
        args.query_start,
        args.warmup_queries,
        args.measured_queries,
        query_count,
        truth_count,
    )

    container, prefix = RANGE.azure_storage_scope(args.storage_url)
    wire_log = RANGE.AzuriteWireLog(args.azurite_debug_log.resolve(), container, prefix)
    wire_log.snapshot()
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
        "protocol": "pax_cache_tier_sweep",
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
            "available_rows": base_count,
            "base_declared_rows": base_declared,
            "measured_rows": args.rows,
            "dimension": dimension,
            "queries": str(query_path),
            "query_format": args.query_format,
            "query_available_rows": query_count,
            "query_declared_rows": query_declared,
            "groundtruth": str(groundtruth_path),
            "groundtruth_format": args.groundtruth_format,
            "groundtruth_scope_rows": args.groundtruth_scope_rows,
            "groundtruth_width": truth_width,
            "query_slices": slices,
        },
        "settled_geometry": geometry,
        "compute_profile": ACCEPTANCE.compute_profile(),
        "experiment": {
            "fixed_nprobe": args.nprobe,
            "fixed_range_cap_mib": args.range_cap_mib,
            "fixed_coalesce_gap_mib": args.coalesce_gap_mib,
            "top_k": args.top_k,
            "disk_population_order": list(disk_population_order()),
            "disk_population_reason": (
                "measured ranges are touched before a disjoint slice exerts "
                "eviction pressure; restart then clears DRAM and result cache"
            ),
            # OwnedServer deliberately fixes the isolated benchmark tier at
            # 10 GiB.  Report the actual control instead of exposing a CLI
            # knob that the shared launcher cannot honor.
            "local_disk_max_gb": 10,
            "azure_hot_read_usd_per_10k": AZURE_HOT_READ_USD_PER_10K,
            "host_quiet_window_seconds": args.host_quiet_window_secs,
            "host_quiet_timeout_seconds": args.host_quiet_timeout_secs,
            "max_contention_retries": args.max_contention_retries,
        },
        "phases": {},
        "comparisons": {},
        "gate_failures": [],
        "rejected_attempts": [],
    }
    write_result(output, result)

    server_url = f"http://127.0.0.1:{args.port}"
    def new_server(label: str, disk_path: Path | None):
        return ACCEPTANCE.OwnedServer(
            binary=binary,
            config=config,
            server=server_url,
            log_path=run_root / f"{label}.log",
            local_disk_path=disk_path,
            nprobe=args.nprobe,
            azure_emulator=True,
            coalesce_gap_bytes=args.coalesce_gap_mib * MIB,
            coalesce_range_bytes=args.range_cap_mib * MIB,
        )

    def query(label: str, server, bounds: list[int]) -> dict:
        return run_measured_phase(
            label=label,
            server=server,
            server_url=server_url,
            collection_id=args.collection_id,
            query_path=query_path,
            groundtruth_path=groundtruth_path,
            query_format=args.query_format,
            groundtruth_format=args.groundtruth_format,
            query_start=bounds[0],
            query_count=bounds[1] - bounds[0],
            top_k=args.top_k,
            wire_log=wire_log,
        )

    active = None
    try:
        attempt = 0
        while True:
            attempt_phases = {}
            local_disk = disk_path_for_attempt(run_root, attempt)
            try:
                RANGE.wait_for_host_quiet(
                    args.host_quiet_window_secs,
                    args.host_quiet_timeout_secs,
                )
                active = new_server(f"object-cold-attempt-{attempt}", None)
                active.start()
                attempt_phases["object_cold"] = query(
                    "object_cold", active, slices["measured"]
                )
                active.stop()
                active = None

                RANGE.wait_for_host_quiet(
                    args.host_quiet_window_secs,
                    args.host_quiet_timeout_secs,
                )
                active = new_server(f"dram-warm-attempt-{attempt}", None)
                active.start()
                attempt_phases["dram_warmup"] = query(
                    "dram_warmup", active, slices["warmup"]
                )
                attempt_phases["dram_warm"] = query(
                    "dram_warm", active, slices["measured"]
                )
                active.stop()
                active = None

                RANGE.wait_for_host_quiet(
                    args.host_quiet_window_secs,
                    args.host_quiet_timeout_secs,
                )
                active = new_server(
                    f"disk-populate-attempt-{attempt}", local_disk
                )
                active.start()
                for slice_name in disk_population_order():
                    attempt_phases[f"disk_population_{slice_name}"] = query(
                        f"disk_population_{slice_name}",
                        active,
                        slices[slice_name],
                    )
                active.stop()
                active = None

                RANGE.wait_for_host_quiet(
                    args.host_quiet_window_secs,
                    args.host_quiet_timeout_secs,
                )
                active = new_server(f"disk-warm-attempt-{attempt}", local_disk)
                active.start()
                attempt_phases["disk_warm"] = query(
                    "disk_warm", active, slices["measured"]
                )
                active.stop()
                active = None
            except (Exception, KeyboardInterrupt) as error:
                if active is not None:
                    active.stop()
                    active = None
                if RANGE.is_host_contention_error(error) and (
                    attempt < args.max_contention_retries
                ):
                    result["rejected_attempts"].append(
                        {
                            "attempt": attempt,
                            "reason": f"{type(error).__name__}: {error}",
                            "discarded_phases": sorted(attempt_phases),
                            "discarded_local_disk_path": str(local_disk),
                        }
                    )
                    write_result(output, result)
                    attempt += 1
                    continue
                raise
            result["phases"] = attempt_phases
            result["experiment"]["accepted_attempt"] = attempt
            result["experiment"]["accepted_local_disk_path"] = str(local_disk)
            write_result(output, result)
            break

        baseline = result["phases"]["object_cold"]
        for phase in ("dram_warm", "disk_warm"):
            result["comparisons"][phase] = compare_phase(
                result["phases"][phase], baseline, args.measured_queries
            )
        measured = [
            baseline,
            result["phases"]["dram_warm"],
            result["phases"]["disk_warm"],
        ]
        if any(point["recall_at_k"] < args.target_recall for point in measured):
            result["gate_failures"].append("a measured phase fell below recall ratchet")
        for phase in ("dram_warm", "disk_warm"):
            comparison = result["comparisons"][phase]
            if not comparison["result_identity_equal"]:
                result["gate_failures"].append(
                    f"{phase} changed result identity relative to object cold"
                )
            if comparison["p50_ratio"] > args.max_cache_latency_ratio:
                result["gate_failures"].append(f"{phase} materially regressed p50")
            if comparison["p95_ratio"] > args.max_cache_latency_ratio:
                result["gate_failures"].append(f"{phase} materially regressed p95")
        disk = result["phases"]["disk_warm"]
        if disk["local_disk"]["hits"] <= 0:
            result["gate_failures"].append(
                "disk-warm phase recorded zero local-disk hits"
            )
        disk_reduction = result["comparisons"]["disk_warm"]["get_reduction"]
        if disk_reduction is None or disk_reduction < args.min_disk_get_reduction:
            result["gate_failures"].append(
                "disk-warm GET reduction was below threshold"
            )
        result["status"] = "pass" if not result["gate_failures"] else "fail"
    except (Exception, KeyboardInterrupt) as error:
        result["status"] = "incomplete"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if active is not None:
            active.stop()
        write_result(output, result)

    print(f"cache-tier result: {output}", flush=True)
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
