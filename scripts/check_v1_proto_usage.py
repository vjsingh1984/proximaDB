#!/usr/bin/env python3
"""Track proximadb.v1 proto usage with a staged ratchet (TD-123).

The v1 proto package (`proto/proximadb/v1/`, the Rust `proximadb_v1` module, the
Python `proximadb.v1` package) is the legacy internal domain model that TD-123
migrates to v2. This script measures how much v1 proto the source still
REFERENCES and ratchets that count DOWN: `no-regression` mode fails if the total
has increased vs the committed baseline, so the migration can only move in one
direction. It does not delete or migrate anything — it is the measurement gate
TD-123 names as a prerequisite to the hard v1 removal (TD-121, which is gated on
2026-07-01 + TD-122 + TD-123).

Mirrors `scripts/count_panic_patterns.sh` (report / no-regression modes,
baseline JSON under `docs/_internal/roadmap/`, `--format text|json`, `--write`).

Usage:
  scripts/check_v1_proto_usage.py                            # report (exit 0)
  scripts/check_v1_proto_usage.py --mode no-regression       # fail if total rose
  scripts/check_v1_proto_usage.py --write docs/_internal/roadmap/V1_PROTO_USAGE_BASELINE.json
  scripts/check_v1_proto_usage.py --format json
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
DEFAULT_BASELINE = REPO_ROOT / "docs/_internal/roadmap/V1_PROTO_USAGE_BASELINE.json"

# Top-level trees scanned for v1 proto references.
ROOTS = ("src", "crates", "clients", "tests")

# File extensions that can reference v1 proto symbols.
EXTENSIONS = {".rs", ".py"}

# Path fragments whose files are NOT counted. These are generated artifacts or
# the v1 proto definitions themselves: counting them would measure generator
# output size, not migration progress, and would make the metric noisy.
EXCLUDE_SUBSTRINGS = (
    "/target/",
    "/.cargo/",
    "proximadb-proto/src/proto/",   # generated rust stubs (incl. proximadb.v1.rs)
    "_pb2.py",                       # generated python stubs
    "_pb2_grpc.py",
    "_pb2.pyi",
    "/v1_pb2/",
    "check_v1_proto_usage.py",       # this script (its own patterns mention the symbols)
)

# Regexes identifying a v1 proto reference. `proximadb_v1` covers Rust module
# paths (`crate::proto::proximadb_v1::...`, `proximadb_proto::proximadb_v1`) and
# generated-name imports; `proximadb\.v1` covers the Python package path;
# `proximadb/v1/` covers build/glue references to the proto source path.
PATTERNS = [
    re.compile(r"proximadb_v1"),
    re.compile(r"proximadb\.v1"),
    re.compile(r"proximadb/v1/"),
]

SCHEMA_VERSION = 1


def _root_of(path: Path) -> str | None:
    s = str(path)
    for r in ROOTS:
        # Absolute prefix match on the top-level tree.
        if s.startswith(str((REPO_ROOT / r)) + "/") or s == str(REPO_ROOT / r):
            return r
    return None


def iter_files():
    for root in ROOTS:
        base = REPO_ROOT / root
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in EXTENSIONS:
                continue
            s = str(path)
            if any(ex in s for ex in EXCLUDE_SUBSTRINGS):
                continue
            yield path


def count_file(path: Path) -> int:
    try:
        text = path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return 0
    return sum(len(p.findall(text)) for p in PATTERNS)


def measure() -> dict:
    per_root = {r: 0 for r in ROOTS}
    file_count = 0
    total = 0
    for path in iter_files():
        n = count_file(path)
        if not n:
            continue
        root = _root_of(path)
        if root is None:
            continue
        file_count += 1
        total += n
        per_root[root] += n
    return {
        "schema_version": SCHEMA_VERSION,
        "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "patterns": [p.pattern for p in PATTERNS],
        "roots": list(ROOTS),
        "excludes": list(EXCLUDE_SUBSTRINGS),
        "total": total,
        "file_count": file_count,
        "per_root": per_root,
    }


def render_text(metrics: dict, baseline_total: int | None) -> str:
    lines = [
        "=== ProximaDB v1 Proto Usage Ratchet (TD-123) ===",
        f"Generated (UTC): {metrics['generated_at']}",
        f"Mode: {metrics['mode']}",
        "",
        "Totals:",
        f"  references: {metrics['total']}",
        f"  files:      {metrics['file_count']}",
        "",
        "Per root (references):",
    ]
    for r in ROOTS:
        lines.append(f"  {r:<10} {metrics['per_root'].get(r, 0)}")
    if baseline_total is not None:
        delta = metrics["total"] - baseline_total
        lines += ["", f"Baseline references: {baseline_total}",
                  f"Delta vs baseline:   {delta:+d}"]
    return "\n".join(lines)


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description="TD-123 v1-proto usage ratchet gate.")
    p.add_argument("--mode", choices=["report", "no-regression"], default="report")
    p.add_argument("--baseline", default=str(DEFAULT_BASELINE))
    p.add_argument("--format", choices=["text", "json"], default="text")
    p.add_argument("--write", default=None, help="write metrics JSON to this path")
    args = p.parse_args(argv)

    metrics = measure()
    metrics["mode"] = args.mode

    baseline_total = None
    if args.mode == "no-regression":
        bpath = Path(args.baseline)
        if not bpath.exists():
            print(f"v1-proto-usage: baseline not found at {bpath}; report-only.",
                  file=sys.stderr)
            metrics["mode"] = "report (baseline missing)"
        else:
            try:
                baseline_total = int(json.loads(bpath.read_text())["total"])
            except (json.JSONDecodeError, KeyError, ValueError, OSError) as e:
                print(f"v1-proto-usage: invalid baseline {bpath}: {e}", file=sys.stderr)
                return 2

    if args.write:
        out = Path(args.write)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(metrics, indent=2, sort_keys=True) + "\n")

    if args.format == "json":
        print(json.dumps(metrics, indent=2, sort_keys=True))
    else:
        print(render_text(metrics, baseline_total))

    if args.mode == "no-regression" and baseline_total is not None:
        if metrics["total"] > baseline_total:
            print(
                f"\n::error::v1 proto usage increased: {metrics['total']} > baseline "
                f"{baseline_total}. v1 proto is being retired (TD-123) — do not add new "
                f"v1 references; migrate to v2 message types. If an increase is "
                f"intentional, regenerate the baseline: "
                f"scripts/check_v1_proto_usage.py --write {args.baseline}",
                file=sys.stderr,
            )
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
