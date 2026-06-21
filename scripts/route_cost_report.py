#!/usr/bin/env python3
"""Route-cost evidence report — decide PROXIMADB_ROUTE_COST_OVERRIDE on data, not faith.

The trace-driven cost model (`src/query/route_cost_model.rs`) runs in **observe
mode** by default: every routed query logs an advisory in its decision reason —
either it *concurs* with the static route or it *would prefer* a cheaper backend
(with the ranked neutral scores). This script summarizes those already-emitted
advisories from a log so a solo operator can see, with real traffic, whether
flipping the live-override flag would actually help — the co-design "measure
before you optimize" discipline (P11) as a tiny report, not an RL system.

It reads structured/plain log lines (a file arg, or stdin) and tallies, per
backend, how often the model diverged from the static choice and by how much
(neutral score advantage). No new instrumentation; pure read of existing output.

Usage:
    python scripts/route_cost_report.py path/to/proximadb.log
    journalctl -u proximadb | python scripts/route_cost_report.py

Exit: always 0 (a report, not a gate).
"""

from __future__ import annotations

import argparse
import re
import statistics
import sys

# route_select_advised emits one of these into the decision reason:
#   "cost-model concurs (min-cost over N sample(s): Native(Volcano) [Native(Volcano)=12.0, ...])"
#   "cost-model would prefer DataFusionLocal (min-cost over N sample(s): ... [Native(Volcano)=200.0, DataFusionLocal=120.0]) — observe-mode, route unchanged"
#   "cost-model OVERRIDE Native(Volcano)→DataFusionLocal (...)"   "cost-model EXPLORE ..."
_PREFER = re.compile(r"cost-model would prefer (?P<backend>[^\s(]+)")
_CONCUR = re.compile(r"cost-model concurs")
_OVERRIDE = re.compile(r"cost-model OVERRIDE \S+→(?P<backend>[^\s(]+)")
_SAMPLES = re.compile(r"min-cost over (?P<n>\d+) sample")
_RANKED = re.compile(r"\[(?P<ranked>[^\]]*)\]")  # "Native(Volcano)=200.0, DataFusionLocal=120.0"
_SCORE = re.compile(r"=(?P<score>[0-9]+(?:\.[0-9]+)?)")

# Override fires only above this neutral advantage (mirrors RouteCostModel::min_advantage).
MIN_ADVANTAGE = 0.15
# Need at least this many diverging observations before a recommendation is trustworthy.
MIN_OBSERVATIONS = 20


def advantage(ranked: str) -> float | None:
    """Relative advantage of the cheapest over the next-cheapest in a ranked list:
    (second - first) / second, in [0,1). Order-agnostic (sorts the parsed scores),
    so it is robust to log formatting. None if < 2 usable scores."""
    scores = sorted(float(m.group("score")) for m in _SCORE.finditer(ranked))
    if len(scores) < 2 or scores[1] <= 0:
        return None
    return (scores[1] - scores[0]) / scores[1]


def main() -> int:
    ap = argparse.ArgumentParser(description="ProximaDB route-cost observe-mode report")
    ap.add_argument("log", nargs="?", help="log file (default: stdin)")
    args = ap.parse_args()
    src = open(args.log, encoding="utf-8", errors="ignore") if args.log else sys.stdin

    concur = 0
    overrides = 0
    prefer_by_backend: dict[str, int] = {}
    advantages: list[float] = []
    max_samples = 0

    with src:
        for line in src:
            if _CONCUR.search(line):
                concur += 1
                continue
            if (mo := _OVERRIDE.search(line)) is not None:
                overrides += 1
                prefer_by_backend[mo.group("backend")] = (
                    prefer_by_backend.get(mo.group("backend"), 0) + 1
                )
            elif (mp := _PREFER.search(line)) is not None:
                prefer_by_backend[mp.group("backend")] = (
                    prefer_by_backend.get(mp.group("backend"), 0) + 1
                )
            else:
                continue
            if (ms := _SAMPLES.search(line)) is not None:
                max_samples = max(max_samples, int(ms.group("n")))
            if (mr := _RANKED.search(line)) is not None and (adv := advantage(mr.group("ranked"))):
                advantages.append(adv)

    diverged = sum(prefer_by_backend.values())
    total = concur + diverged + overrides
    print("=== ProximaDB route-cost observe-mode report ===")
    if total == 0:
        print(
            "No cost-model advisories found. Run routed SELECTs with the cost "
            "observer installed (default), routing logs captured, then re-run."
        )
        return 0
    print(f"advisories: {total}  (concur={concur}, would-prefer={diverged}, override-fired={overrides})")
    print(f"max warmed samples per shape-class: {max_samples}")
    if prefer_by_backend:
        print("would-prefer by backend:")
        for b, n in sorted(prefer_by_backend.items(), key=lambda kv: -kv[1]):
            print(f"  {b}: {n}")
    median_adv = statistics.median(advantages) if advantages else 0.0
    print(f"median neutral advantage when diverging: {median_adv * 100:.1f}% (override gate: {MIN_ADVANTAGE * 100:.0f}%)")

    # Verdict — enable override only with enough evidence AND a consistent,
    # above-gate advantage; otherwise keep observing (don't flip on noise).
    diverge_frac = diverged / total if total else 0.0
    print("\nverdict:")
    if diverged < MIN_OBSERVATIONS:
        print(
            f"  KEEP OBSERVING — only {diverged} diverging samples (< {MIN_OBSERVATIONS}). "
            "Not enough evidence to flip PROXIMADB_ROUTE_COST_OVERRIDE."
        )
    elif median_adv >= MIN_ADVANTAGE and diverge_frac >= 0.2:
        print(
            f"  CONSIDER ENABLING PROXIMADB_ROUTE_COST_OVERRIDE — the model would "
            f"re-route {diverge_frac * 100:.0f}% of queries to a backend that is "
            f"{median_adv * 100:.0f}% cheaper (above the {MIN_ADVANTAGE * 100:.0f}% gate). "
            "Enable in staging first; the override is freshness-safe + reversible."
        )
    else:
        print(
            f"  KEEP OBSERVING — divergence ({diverge_frac * 100:.0f}%) / advantage "
            f"({median_adv * 100:.0f}%) below the bar; the static routes are close to optimal."
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
