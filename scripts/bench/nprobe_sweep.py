#!/usr/bin/env python3
"""Auditable query-only nprobe/top-k sweep over one immutable PAX bed.

Ingest and clustering are intentionally outside the inner loop. Every
``(nprobe, top_k)`` point starts a fresh release server over the same settled
segments, uses the same novel query slice and exact corpus-matched ground
truth, and disables the persistent local tier. This isolates query geometry
from write geometry and avoids paying to retrain identical cells.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
from pathlib import Path

SCRIPT_ROOT = Path(__file__).resolve().parent
ACCEPTANCE_PATH = SCRIPT_ROOT / "sift1m_get_reduction.py"
SPEC = importlib.util.spec_from_file_location(
    "sift_get_reduction_acceptance", ACCEPTANCE_PATH
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load acceptance harness: {ACCEPTANCE_PATH}")
ACCEPTANCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ACCEPTANCE)


def comma_separated_ints(raw: str, label: str) -> list[int]:
    try:
        values = [int(item.strip()) for item in raw.split(",")]
    except ValueError as error:
        raise RuntimeError(f"{label} must be comma-separated integers") from error
    if not values or any(value <= 0 for value in values):
        raise RuntimeError(f"{label} values must be positive")
    if len(set(values)) != len(values):
        raise RuntimeError(f"{label} contains duplicates")
    return values


def git_output(repository: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def require_release_provenance(
    repository: Path,
    binary: Path,
    binary_source_revision: str,
) -> tuple[str, str]:
    if not binary.is_file():
        raise RuntimeError(f"release binary not found: {binary}")
    binary_text = str(binary)
    if (
        "/target/release/" not in binary_text
        and "/target/release-server/" not in binary_text
    ):
        raise RuntimeError("matrix requires an optimized release binary")
    current_revision = git_output(repository, "rev-parse", "HEAD")
    if git_output(repository, "status", "--porcelain", "--untracked-files=normal"):
        raise RuntimeError("matrix refuses a dirty worktree")
    subprocess.run(
        [
            "git",
            "merge-base",
            "--is-ancestor",
            binary_source_revision,
            current_revision,
        ],
        cwd=repository,
        check=True,
    )
    changed = git_output(
        repository,
        "diff",
        "--name-only",
        f"{binary_source_revision}..{current_revision}",
    ).splitlines()
    unsafe = [
        path
        for path in changed
        if not path.startswith(("docs/", "scripts/"))
        and path
        not in {
            "tests/python/test_sift_get_reduction_harness.py",
            "tests/python/test_bigann_prefix_groundtruth.py",
            "tests/python/test_nprobe_geometry_analysis.py",
            "tests/python/test_nprobe_sweep.py",
        }
    ]
    if unsafe:
        raise RuntimeError(
            f"binary source differs from executable source: {unsafe}; rebuild release"
        )
    return current_revision, (
        "release-server" if "/target/release-server/" in binary_text else "release"
    )


def validate_geometry(
    geometry: dict,
    rows: int,
    max_segments: int,
    layout_version: int,
) -> None:
    if geometry["row_count"] != rows:
        raise RuntimeError(
            f"settled PAX rows {geometry['row_count']} != corpus rows {rows}"
        )
    if not 0 < geometry["segment_count"] <= max_segments:
        raise RuntimeError(
            f"settled segments {geometry['segment_count']} exceed {max_segments}"
        )
    wrong_layouts = [
        segment
        for segment in geometry["segments"]
        if segment["layout_version"] != layout_version
    ]
    if wrong_layouts:
        raise RuntimeError(f"matrix requires PAX v{layout_version}: {wrong_layouts}")
    if any(not segment.get("coarse_cells") for segment in geometry["segments"]):
        raise RuntimeError("every matrix segment must persist coarse cells")


def require_config_port(config: Path, expected_port: int) -> None:
    """Fail before startup when the immutable bed config binds another port."""
    section = ""
    configured_port = None
    for raw_line in config.read_text().splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            continue
        if section != "api" or "=" not in line:
            continue
        key, value = (item.strip() for item in line.split("=", 1))
        if key == "unified_port":
            try:
                configured_port = int(value)
            except ValueError as error:
                raise RuntimeError(
                    f"invalid [api].unified_port in {config}: {value!r}"
                ) from error
            break
    if configured_port is None:
        raise RuntimeError(f"missing [api].unified_port in {config}")
    if configured_port != expected_port:
        raise RuntimeError(
            f"--port={expected_port} does not match immutable bed config "
            f"[api].unified_port={configured_port}"
        )


def checkpoint_identity(result: dict) -> dict:
    """Return the immutable provenance/configuration of a matrix run."""
    matrix = result["matrix"]
    return {
        "protocol": result["protocol"],
        "git_revision": result["git_revision"],
        "collection_id": result["collection_id"],
        "binary": result["binary"],
        "bed_config": result["bed_config"],
        "dataset": result["dataset"],
        "filesystem_profile": result["filesystem_profile"],
        "compute_profile": result["compute_profile"],
        "settled_geometry": result["settled_geometry"],
        "matrix_config": {
            "nprobes": matrix["nprobes"],
            "top_k_values": matrix["top_k_values"],
            "min_recall": matrix["min_recall"],
        },
    }


def validate_resume(existing: dict, expected: dict) -> set[tuple[int, int]]:
    """Validate a checkpoint and return its completed point identities."""
    if existing.get("status") not in {"running", "incomplete"}:
        raise RuntimeError(
            f"matrix checkpoint is terminal ({existing.get('status')!r}); "
            "refusing resume"
        )
    if checkpoint_identity(existing) != checkpoint_identity(expected):
        raise RuntimeError("matrix checkpoint provenance/configuration differs")
    expected_pairs = {
        (int(nprobe), int(top_k))
        for nprobe in expected["matrix"]["nprobes"]
        for top_k in expected["matrix"]["top_k_values"]
    }
    completed: set[tuple[int, int]] = set()
    for point in existing["matrix"].get("points", []):
        identity = (int(point["nprobe"]), int(point["top_k"]))
        if identity not in expected_pairs:
            raise RuntimeError(f"checkpoint contains unexpected point {identity}")
        if identity in completed:
            raise RuntimeError(f"checkpoint contains duplicate point {identity}")
        completed.add(identity)
    return completed


def write_checkpoint(
    output: Path,
    result: dict,
    state: str,
    incomplete_reason: str | None = None,
) -> None:
    """Atomically persist all completed points before advancing the sweep."""
    expected_points = len(result["matrix"]["nprobes"]) * len(
        result["matrix"]["top_k_values"]
    )
    result["status"] = state
    result["checkpoint"] = {
        "state": state,
        "completed_points": len(result["matrix"]["points"]),
        "expected_points": expected_points,
        "incomplete_reason": incomplete_reason,
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
    parser.add_argument("--sift-dir", type=Path, required=True)
    parser.add_argument("--base-path", type=Path)
    parser.add_argument(
        "--base-format",
        choices=("fvecs", "u8bin"),
        default="fvecs",
    )
    parser.add_argument("--query-path", type=Path)
    parser.add_argument(
        "--query-format",
        choices=("fvecs", "u8bin"),
        default="fvecs",
    )
    parser.add_argument("--groundtruth-path", type=Path, required=True)
    parser.add_argument(
        "--groundtruth-format",
        choices=("ivecs", "bigann-bin"),
        default="ivecs",
    )
    parser.add_argument("--groundtruth-scope-rows", type=int, required=True)
    parser.add_argument("--rows", type=int, required=True)
    parser.add_argument("--nprobes", required=True)
    parser.add_argument("--top-k-values", default="10,20")
    parser.add_argument("--queries", type=int, default=1_000)
    parser.add_argument("--query-start", type=int, default=0)
    parser.add_argument("--port", type=int, default=5690)
    parser.add_argument("--max-segments", type=int, default=1)
    parser.add_argument("--required-layout-version", type=int, default=3)
    parser.add_argument("--min-recall", type=float, default=0.98)
    parser.add_argument("--azurite", action="store_true")
    parser.add_argument(
        "--resume",
        action="store_true",
        help=(
            "resume an incomplete atomic checkpoint only when binary, dataset, "
            "storage geometry, query slice, and sweep configuration all match"
        ),
    )
    args = parser.parse_args()

    repository = Path(__file__).resolve().parents[2]
    binary = args.binary.resolve()
    config = args.config.resolve()
    run_root = args.run_root.resolve()
    output = args.output.resolve()
    groundtruth_path = args.groundtruth_path.resolve()
    nprobes = comma_separated_ints(args.nprobes, "--nprobes")
    top_k_values = comma_separated_ints(args.top_k_values, "--top-k-values")
    if output.exists() and not args.resume:
        raise RuntimeError(f"refusing to overwrite matrix result: {output}")
    if args.resume and not output.exists():
        raise RuntimeError(f"resume checkpoint does not exist: {output}")
    if not config.is_file():
        raise RuntimeError(f"benchmark config not found: {config}")
    require_config_port(config, args.port)
    if args.groundtruth_scope_rows != args.rows:
        raise RuntimeError("ground-truth scope must equal the measured corpus rows")
    if args.queries <= 0 or args.query_start < 0:
        raise RuntimeError("query count must be positive and start non-negative")
    if not 0.0 < args.min_recall <= 1.0:
        raise RuntimeError("--min-recall must be in (0, 1]")
    if not args.collection_id.isdecimal():
        raise RuntimeError("--collection-id must be a decimal catalog object id")

    current_revision, profile = require_release_provenance(
        repository, binary, args.binary_source_revision
    )
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
    (
        base_count,
        dimension,
        base_declared_rows,
    ) = ACCEPTANCE.vector_source_geometry(base_path, args.base_format)
    (
        query_count,
        query_dimension,
        query_declared_rows,
    ) = ACCEPTANCE.vector_source_geometry(query_path, args.query_format)
    truth_count, truth_width = ACCEPTANCE.count_truth_records(
        groundtruth_path, args.groundtruth_format
    )
    if args.rows > base_count or dimension != 128:
        raise RuntimeError(
            f"invalid base corpus: rows={base_count}, dimension={dimension}"
        )
    if query_dimension != dimension:
        raise RuntimeError("base and query dimensions differ")
    query_end = args.query_start + args.queries
    if query_end > query_count or query_end > truth_count:
        raise RuntimeError("query slice exceeds query or ground-truth rows")
    if max(top_k_values) > truth_width:
        raise RuntimeError(
            f"top_k={max(top_k_values)} exceeds truth width {truth_width}"
        )

    run_root.mkdir(parents=True, exist_ok=True)
    geometry_reader = ACCEPTANCE.AzureCliPaxGeometry(
        args.storage_url, run_root / "pax-snapshot"
    )
    geometry = geometry_reader.materialize(geometry_reader.inventory())
    validate_geometry(
        geometry,
        args.rows,
        args.max_segments,
        args.required_layout_version,
    )
    max_cells = max(segment["coarse_cells"] for segment in geometry["segments"])
    if max(nprobes) > max_cells:
        raise RuntimeError(
            f"nprobe={max(nprobes)} exceeds persisted max k_c={max_cells}"
        )

    result = {
        "protocol": "pax_nprobe_topk_matrix",
        "git_revision": current_revision,
        "collection_id": args.collection_id,
        "binary": {
            "path": str(binary),
            "sha256": ACCEPTANCE.sha256(binary),
            "bytes": binary.stat().st_size,
            "source_revision": args.binary_source_revision,
            "profile": profile,
        },
        "bed_config": {
            "path": str(config),
            "sha256": ACCEPTANCE.sha256(config),
            "port": args.port,
            "max_segments": args.max_segments,
            "required_layout_version": args.required_layout_version,
        },
        "dataset": {
            "base": str(base_path),
            "base_format": args.base_format,
            "corpus_rows": args.rows,
            "base_available_rows": base_count,
            "base_declared_rows": base_declared_rows,
            "dimension": dimension,
            "queries_path": str(query_path),
            "query_format": args.query_format,
            "query_available_rows": query_count,
            "query_declared_rows": query_declared_rows,
            "groundtruth": str(groundtruth_path),
            "groundtruth_format": args.groundtruth_format,
            "groundtruth_scope_rows": args.groundtruth_scope_rows,
            "groundtruth_width": truth_width,
            "query_range": [args.query_start, query_end],
        },
        "filesystem_profile": {
            "storage_url": args.storage_url,
            "azurite": args.azurite,
            "persistent_local_tier": False,
            "note": (
                "Fresh process and empty DRAM/result caches at each point. "
                "Queries are diverse within a point, so caches may warm across "
                "that point. Azurite exercises the production Azure HTTP "
                "backend but is not Azure WAN latency evidence. Geometry "
                "snapshot reads are out of process and excluded from counters."
            ),
        },
        "compute_profile": ACCEPTANCE.compute_profile(),
        "settled_geometry": geometry,
        "matrix": {
            "nprobes": nprobes,
            "top_k_values": top_k_values,
            "min_recall": args.min_recall,
            "points": [],
        },
        "failures": [],
    }

    if args.resume:
        existing = json.loads(output.read_text())
        completed = validate_resume(existing, result)
        result = existing
        result["checkpoint"]["incomplete_reason"] = None
        print(
            f"resuming matrix: {len(completed)}/"
            f"{len(nprobes) * len(top_k_values)} points complete",
            flush=True,
        )
    else:
        completed = set()
        write_checkpoint(output, result, "running")

    server_url = f"http://127.0.0.1:{args.port}"
    for nprobe in nprobes:
        for top_k in top_k_values:
            if (nprobe, top_k) in completed:
                print(f"checkpoint skip: nprobe={nprobe} top_k={top_k}", flush=True)
                continue
            label = f"nprobe-{nprobe}-top-{top_k}"
            server = ACCEPTANCE.OwnedServer(
                binary=binary,
                config=config,
                server=server_url,
                log_path=run_root / f"{label}.log",
                local_disk_path=None,
                nprobe=nprobe,
                azure_emulator=args.azurite,
            )
            try:
                server.start()
                point = ACCEPTANCE.run_query_sweep(
                    server_url,
                    args.collection_id,
                    query_path,
                    groundtruth_path,
                    args.query_start,
                    args.queries,
                    top_k,
                    label,
                    args.query_format,
                    args.groundtruth_format,
                )
            except (Exception, KeyboardInterrupt) as error:
                write_checkpoint(
                    output,
                    result,
                    "incomplete",
                    f"{label}: {type(error).__name__}: {error}",
                )
                raise
            finally:
                server.stop()
            expected_cells = sum(
                min(nprobe, segment["coarse_cells"]) for segment in geometry["segments"]
            )
            point["nprobe"] = nprobe
            point["normalized_nprobe_by_segment"] = [
                nprobe / segment["coarse_cells"] for segment in geometry["segments"]
            ]
            point["expected_cells_probed_per_query"] = expected_cells
            actual_cells = point["ivf"]["cells_probed_per_query"]
            if abs(actual_cells - expected_cells) > 0.01:
                result["failures"].append(
                    f"{label}: measured cells/query {actual_cells:.3f} "
                    f"!= expected {expected_cells}"
                )
            if attribution_failure := ACCEPTANCE.ivf_byte_attribution_failure(
                label, point
            ):
                result["failures"].append(attribution_failure)
            result["matrix"]["points"].append(point)
            completed.add((nprobe, top_k))
            write_checkpoint(output, result, "running")

    for top_k in top_k_values:
        recalls = [
            point["recall_at_k"]
            for point in result["matrix"]["points"]
            if point["top_k"] == top_k
        ]
        if not recalls or max(recalls) < args.min_recall:
            result["failures"].append(
                f"top_k={top_k}: no nprobe reaches recall {args.min_recall:.4f}"
            )
    final_status = "pass" if not result["failures"] else "fail"
    write_checkpoint(output, result, final_status)
    print(f"matrix result: {output}", flush=True)
    if result["failures"]:
        for failure in result["failures"]:
            print(f"FAIL: {failure}", flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
