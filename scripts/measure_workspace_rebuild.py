#!/usr/bin/env python3
"""Measure targeted Cargo check times for workspace isolation work.

The workspace refactor is only valuable if targeted package checks get cheaper.
This script records repeatable package-level `cargo check` timings without
requiring a full root rebuild.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TARGET_DIR = Path("/private/tmp/proximadb-workspace-baseline")
DEFAULT_OUTPUT = ROOT / "artifacts" / "workspace_build_baseline.json"
DEFAULT_PACKAGES = (
    "proximadb-kernel",
    "proximadb-proto",
    "proximadb-data-model",
    "proximadb-records",
    "proximadb-telemetry",
    "proximadb-runtime-common",
    "proximadb-runtime",
    "proximadb-storage-common",
    "proximadb-graph-query",
    "proximadb-graph",
    "proximadb-multimodel-plan",
    "proximadb-query",
    "proximadb",
)


@dataclass(frozen=True)
class Measurement:
    package: str
    command: list[str]
    elapsed_seconds: float | None
    returncode: int | None
    status: str
    stdout_tail: str
    stderr_tail: str


def tail(text: str, max_chars: int = 4000) -> str:
    return text[-max_chars:]


def build_command(cargo: str, package: str, target_dir: Path, features: str | None) -> list[str]:
    command = [
        cargo,
        "check",
        "-p",
        package,
        "--lib",
        "--target-dir",
        str(target_dir),
    ]
    if features:
        command.extend(["--features", features])
    return command


def measure_package(
    cargo: str,
    package: str,
    target_dir: Path,
    features: str | None,
    dry_run: bool,
) -> Measurement:
    command = build_command(cargo, package, target_dir, features)
    if dry_run:
        return Measurement(package, command, None, None, "dry-run", "", "")

    started = time.monotonic()
    completed = subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        env=os.environ.copy(),
        text=True,
    )
    elapsed = time.monotonic() - started
    return Measurement(
        package=package,
        command=command,
        elapsed_seconds=round(elapsed, 3),
        returncode=completed.returncode,
        status="ok" if completed.returncode == 0 else "failed",
        stdout_tail=tail(completed.stdout),
        stderr_tail=tail(completed.stderr),
    )


def write_report(path: Path, measurements: list[Measurement], target_dir: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    report = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "host": {
            "platform": platform.platform(),
            "python": sys.version.split()[0],
        },
        "workspace": str(ROOT),
        "target_dir": str(target_dir),
        "measurements": [asdict(measurement) for measurement in measurements],
    }
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def print_list(packages: tuple[str, ...]) -> None:
    print("Workspace rebuild baseline packages")
    for package in packages:
        print(package)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "-p",
        "--package",
        action="append",
        dest="packages",
        help="package to measure; may be supplied multiple times",
    )
    parser.add_argument(
        "--features",
        help="optional feature set passed to every cargo check command",
    )
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=DEFAULT_TARGET_DIR,
        help=f"Cargo target directory, default: {DEFAULT_TARGET_DIR}",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help=f"JSON report path, default: {DEFAULT_OUTPUT.relative_to(ROOT)}",
    )
    parser.add_argument(
        "--cargo",
        default="cargo",
        help="cargo executable, default: cargo",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="list default packages and exit",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print commands and write a report without executing cargo",
    )
    parser.add_argument(
        "--keep-going",
        action="store_true",
        help="continue measuring packages after a cargo check failure",
    )
    args = parser.parse_args()

    packages = tuple(args.packages or DEFAULT_PACKAGES)
    if args.list:
        print_list(packages)
        return 0

    measurements: list[Measurement] = []
    for package in packages:
        measurement = measure_package(
            cargo=args.cargo,
            package=package,
            target_dir=args.target_dir,
            features=args.features,
            dry_run=args.dry_run,
        )
        measurements.append(measurement)
        elapsed = (
            "not-run"
            if measurement.elapsed_seconds is None
            else f"{measurement.elapsed_seconds:.3f}s"
        )
        print(f"{measurement.status}: {package}: {elapsed}")
        if measurement.status == "failed" and not args.keep_going:
            break

    write_report(args.output, measurements, args.target_dir)
    failed = [measurement.package for measurement in measurements if measurement.status == "failed"]
    if failed:
        print(f"wrote report: {args.output}")
        print(f"failed packages: {', '.join(failed)}")
        return 1

    print(f"wrote report: {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
