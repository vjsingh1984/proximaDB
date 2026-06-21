#!/usr/bin/env python3
"""Coverage ratchet gate for the qa branch.

Enforces a NO-REGRESSION policy with an 80% target across components (Rust crates
and the Python SDK):

  - FAIL if any component's line coverage drops more than TOLERANCE below its
    committed baseline (a regression).
  - FAIL if a NEW component lands below the 80% target.
  - WARN (non-fatal) for existing components still under 80% — they ratchet up
    over time; regressions are what block.

Usage:
  # compute a normalized {component: pct} map from raw tool output:
  coverage_ratchet.py normalize --rust llvm-cov.json --sdk sdk-cov.json -o current.json

  # check current against the committed baseline:
  coverage_ratchet.py check --baseline docs/_internal/roadmap/COVERAGE_BASELINE.json \
      --current current.json

  # (re)write the baseline from current (after an intentional change):
  coverage_ratchet.py write --current current.json \
      --baseline docs/_internal/roadmap/COVERAGE_BASELINE.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

TARGET = 80.0  # coverage goal (%)
TOLERANCE = 0.5  # allowed downward drift before it's a regression (%)


def _pct(covered: int, total: int) -> float:
    return 100.0 * covered / total if total else 100.0


def normalize_rust(path: Path) -> dict[str, float]:
    """Parse `cargo llvm-cov report --json` into {crate: line_pct}.

    llvm-cov json: {"data":[{"files":[{"filename":..., "summary":{"lines":{"count","covered"}}}]}]}
    We group files by workspace crate (the path segment after `crates/<tier>/<crate>/`
    or the root crate `src/`).
    """
    doc = json.loads(path.read_text())
    agg: dict[str, list[int]] = {}  # crate -> [covered, total]
    for block in doc.get("data", []):
        for f in block.get("files", []):
            name = f.get("filename", "")
            lines = f.get("summary", {}).get("lines", {})
            covered = int(lines.get("covered", 0))
            total = int(lines.get("count", 0))
            crate = _crate_of(name)
            if crate is None:
                continue
            slot = agg.setdefault(crate, [0, 0])
            slot[0] += covered
            slot[1] += total
    return {f"rust:{k}": round(_pct(v[0], v[1]), 2) for k, v in agg.items() if v[1]}


def _crate_of(path: str) -> str | None:
    parts = path.replace("\\", "/").split("/")
    if "crates" in parts:
        i = parts.index("crates")
        # crates/<tier>/<crate>/src/...
        if len(parts) > i + 2:
            return parts[i + 2]
    # root crate: .../<repo>/src/...  -> "proximadb"
    if "src" in parts:
        return "proximadb"
    return None


def normalize_sdk(path: Path) -> dict[str, float]:
    """Parse `coverage json` (pytest-cov) into {sdk: line_pct}."""
    doc = json.loads(path.read_text())
    totals = doc.get("totals", {})
    pct = totals.get("percent_covered")
    if pct is None:
        covered = totals.get("covered_lines", 0)
        total = covered + totals.get("missing_lines", 0)
        pct = _pct(covered, total)
    return {"sdk:proximadb-python": round(float(pct), 2)}


def cmd_normalize(args) -> int:
    current: dict[str, float] = {}
    if args.rust:
        current.update(normalize_rust(Path(args.rust)))
    if args.sdk:
        current.update(normalize_sdk(Path(args.sdk)))
    Path(args.out).write_text(json.dumps(dict(sorted(current.items())), indent=2) + "\n")
    print(f"wrote {len(current)} components -> {args.out}")
    return 0


def cmd_write(args) -> int:
    current = json.loads(Path(args.current).read_text())
    Path(args.baseline).write_text(json.dumps(dict(sorted(current.items())), indent=2) + "\n")
    print(f"baseline written: {len(current)} components -> {args.baseline}")
    return 0


def cmd_check(args) -> int:
    baseline = json.loads(Path(args.baseline).read_text())
    current = json.loads(Path(args.current).read_text())
    regressions, new_below, warnings = [], [], []

    for comp, cur in sorted(current.items()):
        base = baseline.get(comp)
        if base is None:
            if cur + 1e-9 < TARGET:
                new_below.append((comp, cur))
            continue
        if cur + TOLERANCE < base:
            regressions.append((comp, base, cur))
        elif cur + 1e-9 < TARGET:
            warnings.append((comp, cur))

    missing = sorted(set(baseline) - set(current))

    print(f"=== Coverage ratchet (target {TARGET}%, tolerance {TOLERANCE}%) ===")
    print(f"  components: {len(current)} current / {len(baseline)} baseline")
    if warnings:
        print(f"  below-target (warn, {len(warnings)}):")
        for c, p in warnings[:40]:
            print(f"    - {c}: {p}%")
    if missing:
        print(f"  WARNING: {len(missing)} baseline component(s) absent from current: {missing[:10]}")
    if new_below:
        print(f"  NEW component(s) below {TARGET}% (FAIL):")
        for c, p in new_below:
            print(f"    - {c}: {p}% (< {TARGET}%)")
    if regressions:
        print(f"  REGRESSIONS (FAIL):")
        for c, b, cur in regressions:
            print(f"    - {c}: {b}% -> {cur}% (drop {round(b - cur, 2)}%)")

    if regressions or new_below:
        print("\nqa coverage gate: FAIL")
        return 1
    print("\nqa coverage gate: PASS")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    n = sub.add_parser("normalize", help="normalize raw tool output -> {component: pct}")
    n.add_argument("--rust", help="cargo llvm-cov report --json output")
    n.add_argument("--sdk", help="pytest coverage json output")
    n.add_argument("-o", "--out", required=True)
    n.set_defaults(func=cmd_normalize)

    c = sub.add_parser("check", help="check current vs baseline (ratchet)")
    c.add_argument("--baseline", required=True)
    c.add_argument("--current", required=True)
    c.set_defaults(func=cmd_check)

    w = sub.add_parser("write", help="write baseline from current")
    w.add_argument("--current", required=True)
    w.add_argument("--baseline", required=True)
    w.set_defaults(func=cmd_write)

    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
