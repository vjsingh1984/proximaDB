"""Print a side-by-side comparison of a benchmark JSON against the documented baseline.

Baseline numbers are pulled from
`docs/02-guides/api-surface-performance-guide.md` (Python 3.12, macOS arm64,
scale=200, dimension=64, 3 runs).
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

# Baseline: ops_per_second (median), measured on 2026-05-19.
# Names use the bench-script naming (so `vector.insert_numpy` matches the new
# sync key `vector.insert_numpy.sync`; we map both sync and async candidates).
BASELINE: dict[str, float] = {
    "vector.insert_numpy": 89_200.0,
    "record_wire.vector_insert": 75_100.0,
    "document.insert": 24_800.0,  # via insert_records_profiled doc batch
    "graph_entity.create_nodes": 25_300.0,  # via insert_records_profiled graph batch
    "observability.ingest_logs": 22_800.0,  # via insert_records_profiled obs batch
    "arrow_embedded.insert_arrow": 82_500.0,
    "relational.sql_insert_multirow_batch": 20_100.0,
    "vector.search_top10_profiled": 32_400.0,
    "record_wire.vector_search_top10_profiled": 26_600.0,
    "vector.search_top10": 652.0,
    "record_wire.vector_search_top10": 339.0,
    "record_wire.sql_vector_search_top10": 13_000.0,
    "record_wire.uql_vector_search_top10": 11_500.0,
    "graph_entity.cypher_match_entity_limit10": 4_100.0,
    "document.query_indexed_path": 2_250.0,
    "observability.query_logs": 79_800.0,
}


def strip_mode_suffix(name: str) -> str:
    for suffix in (".sync", ".async", ".async_inserted"):
        if name.endswith(suffix):
            return name[: -len(suffix)]
    return name


def to_baseline_key(name: str) -> str:
    """Best-effort map from candidate metric name to the baseline name."""
    base = strip_mode_suffix(name)
    # Aliases between new and old naming
    aliases = {
        "arrow.insert_arrow": "arrow_embedded.insert_arrow",
        "relational.sql_insert_multirow_batch": "relational.sql_insert_multirow_batch",
    }
    return aliases.get(base, base)


def load_metrics(path: Path) -> list[dict]:
    payload = json.loads(path.read_text())
    if "aggregate_results" in payload:
        return payload["aggregate_results"]
    return payload.get("results", [])


def get_ops(metric: dict) -> float | None:
    if "ops_per_second_median" in metric:
        return metric["ops_per_second_median"]
    return metric.get("ops_per_second")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("json_file", type=Path)
    args = parser.parse_args()

    metrics = load_metrics(args.json_file)
    rows = []
    for metric in metrics:
        name = metric["name"]
        candidate_ops = get_ops(metric)
        if candidate_ops is None:
            continue
        baseline_key = to_baseline_key(name)
        baseline_ops = BASELINE.get(baseline_key)
        if baseline_ops is None:
            ratio = None
            delta_pct = None
        else:
            ratio = candidate_ops / baseline_ops
            delta_pct = (ratio - 1.0) * 100.0
        rows.append((name, candidate_ops, baseline_key, baseline_ops, ratio, delta_pct))

    # Print as markdown table
    print("| Metric | Candidate ops/s | Baseline ops/s | Ratio | Delta % |")
    print("|---|---:|---:|---:|---:|")
    for name, cand, base_key, base, ratio, delta in rows:
        cand_str = f"{cand:,.1f}" if cand is not None else "-"
        base_str = f"{base:,.1f}" if base is not None else "(no baseline)"
        ratio_str = f"{ratio:.2f}x" if ratio is not None else "-"
        delta_str = f"{delta:+.1f}%" if delta is not None else "-"
        print(f"| {name} | {cand_str} | {base_str} | {ratio_str} | {delta_str} |")


if __name__ == "__main__":
    main()
