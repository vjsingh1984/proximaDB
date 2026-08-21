#!/usr/bin/env python3
"""Evaluate chunk retrieval against document qrels after source collapse.

The evaluator assigns each source the maximum score of any retrieved chunk.
Quality runs should use a corpus built without exact deduplication: one stored
vector mapping to multiple source documents makes document ranking ambiguous.
Alias expansion is therefore an explicit diagnostic opt-in.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import tempfile
from collections import defaultdict
from pathlib import Path
from typing import Any


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _jsonl(path: Path):
    with path.open("r", encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            if not line.strip():
                continue
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{path} line {line_number}: expected an object")
            yield line_number, value


def _required_text(record: dict[str, Any], name: str, context: str) -> str:
    value = record.get(name)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{context}: {name} is required")
    return value


def _finite_number(record: dict[str, Any], name: str, context: str) -> float:
    value = record.get(name)
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(float(value))
    ):
        raise ValueError(f"{context}: {name} must be a finite number")
    return float(value)


def _percentile(values: list[int], fraction: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    return ordered[round((len(ordered) - 1) * fraction)]


def _histogram(values: list[int]) -> dict[str, int | None]:
    return {
        "min": min(values) if values else None,
        "p50": _percentile(values, 0.50),
        "p90": _percentile(values, 0.90),
        "p99": _percentile(values, 0.99),
        "max": max(values) if values else None,
    }


def _atomic_json(value: Any, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f"{path.name}.", suffix=".tmp", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with temporary.open("w", encoding="utf-8") as output:
            json.dump(value, output, indent=2, sort_keys=True)
            output.write("\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _load_occurrences(
    path: Path, *, allow_deduplicated_aliases: bool
) -> tuple[dict[str, tuple[str, ...]], int, int]:
    sources_by_corpus: dict[str, set[str]] = defaultdict(set)
    occurrence_count = 0
    alias_count = 0
    for line_number, record in _jsonl(path):
        context = f"occurrences line {line_number}"
        corpus_id = _required_text(record, "corpus_id", context)
        source_id = _required_text(record, "source_id", context)
        alias = record.get("deduplicated_alias")
        if not isinstance(alias, bool):
            raise ValueError(f"{context}: deduplicated_alias must be boolean")
        sources_by_corpus[corpus_id].add(source_id)
        occurrence_count += 1
        alias_count += int(alias)
    if not occurrence_count:
        raise ValueError("occurrence sidecar contains no rows")
    if alias_count and not allow_deduplicated_aliases:
        raise ValueError(
            "occurrence sidecar contains deduplicated aliases; rebuild quality "
            "corpora with deduplicate=none or explicitly allow diagnostic expansion"
        )
    return (
        {
            corpus_id: tuple(sorted(source_ids))
            for corpus_id, source_ids in sources_by_corpus.items()
        },
        occurrence_count,
        alias_count,
    )


def _load_qrels(path: Path) -> dict[str, dict[str, float]]:
    qrels: dict[str, dict[str, float]] = defaultdict(dict)
    for line_number, record in _jsonl(path):
        context = f"source qrels line {line_number}"
        if record.get("judgment_granularity") != "document":
            raise ValueError(f"{context}: judgment_granularity must be 'document'")
        query_id = _required_text(record, "query_id", context)
        source_id = _required_text(record, "source_id", context)
        relevance = _finite_number(record, "relevance", context)
        if relevance <= 0:
            raise ValueError(f"{context}: relevance must be positive")
        qrels[query_id][source_id] = max(relevance, qrels[query_id].get(source_id, 0.0))
    if not qrels:
        raise ValueError("source qrels contain no relations")
    return dict(qrels)


def _collapse_run(
    path: Path,
    sources_by_corpus: dict[str, tuple[str, ...]],
    known_queries: set[str],
) -> tuple[dict[str, dict[str, float]], int]:
    source_scores: dict[str, dict[str, float]] = defaultdict(dict)
    row_count = 0
    for line_number, record in _jsonl(path):
        context = f"run line {line_number}"
        query_id = _required_text(record, "query_id", context)
        corpus_id = _required_text(record, "corpus_id", context)
        score = _finite_number(record, "score", context)
        if query_id not in known_queries:
            raise ValueError(f"{context}: query_id {query_id!r} has no qrels")
        source_ids = sources_by_corpus.get(corpus_id)
        if source_ids is None:
            raise ValueError(f"{context}: unknown corpus_id {corpus_id!r}")
        for source_id in source_ids:
            source_scores[query_id][source_id] = max(
                score, source_scores[query_id].get(source_id, -math.inf)
            )
        row_count += 1
    if not row_count:
        raise ValueError("retrieval run contains no rows")
    return dict(source_scores), row_count


def _metrics_at_k(
    ranked_sources: list[str], qrels: dict[str, float], k: int
) -> dict[str, float]:
    retrieved = ranked_sources[:k]
    relevant = set(qrels)
    recall = len(relevant.intersection(retrieved)) / len(relevant)
    first_relevant = next(
        (rank for rank, source_id in enumerate(retrieved, 1) if source_id in relevant),
        None,
    )
    reciprocal_rank = 0.0 if first_relevant is None else 1.0 / first_relevant
    dcg = sum(
        qrels.get(source_id, 0.0) / math.log2(rank + 1)
        for rank, source_id in enumerate(retrieved, 1)
    )
    ideal = sorted(qrels.values(), reverse=True)[:k]
    idcg = sum(
        relevance / math.log2(rank + 1) for rank, relevance in enumerate(ideal, 1)
    )
    return {"mrr": reciprocal_rank, "ndcg": dcg / idcg, "recall": recall}


def evaluate_source_retrieval(
    run_path: Path,
    source_qrels_path: Path,
    occurrences_path: Path,
    *,
    k_values: tuple[int, ...] = (1, 5, 10, 20, 100),
    allow_deduplicated_aliases: bool = False,
    output_path: Path | None = None,
) -> dict[str, Any]:
    """Collapse chunk scores to sources and compute macro document metrics."""

    if not k_values or any(
        isinstance(value, bool) or not isinstance(value, int) or value <= 0
        for value in k_values
    ):
        raise ValueError("k_values must contain positive integers")
    chosen_k = tuple(sorted(set(k_values)))
    sources_by_corpus, occurrence_count, alias_count = _load_occurrences(
        occurrences_path,
        allow_deduplicated_aliases=allow_deduplicated_aliases,
    )
    qrels = _load_qrels(source_qrels_path)
    source_scores, chunk_run_row_count = _collapse_run(
        run_path, sources_by_corpus, set(qrels)
    )

    totals = {k: {"mrr": 0.0, "ndcg": 0.0, "recall": 0.0} for k in chosen_k}
    candidate_counts: list[int] = []
    source_run_row_count = 0
    for query_id in sorted(qrels):
        scores = source_scores.get(query_id, {})
        ranked = [
            source_id
            for source_id, _score in sorted(
                scores.items(), key=lambda item: (-item[1], item[0])
            )
        ]
        candidate_counts.append(len(ranked))
        source_run_row_count += len(ranked)
        for k in chosen_k:
            per_query = _metrics_at_k(ranked, qrels[query_id], k)
            for metric, value in per_query.items():
                totals[k][metric] += value

    query_count = len(qrels)
    result: dict[str, Any] = {
        "schema_version": 1,
        "producer_sha256": _sha256(Path(__file__).resolve()),
        "metric_semantics": {
            "collapse": "maximum chunk score per source",
            "ndcg_gain": "linear relevance",
            "recall": "binary positive relevance",
            "mrr": "first positive relevance",
            "aggregation": "macro mean over every qrels query",
            "tie_break": "source_id ascending",
        },
        "run_path": str(run_path.resolve()),
        "run_sha256": _sha256(run_path),
        "source_qrels_path": str(source_qrels_path.resolve()),
        "source_qrels_sha256": _sha256(source_qrels_path),
        "occurrences_path": str(occurrences_path.resolve()),
        "occurrences_sha256": _sha256(occurrences_path),
        "query_count": query_count,
        "occurrence_count": occurrence_count,
        "deduplicated_alias_count": alias_count,
        "diagnostic_alias_expansion": allow_deduplicated_aliases,
        "chunk_run_row_count": chunk_run_row_count,
        "source_run_row_count": source_run_row_count,
        "source_candidates_per_query": _histogram(candidate_counts),
        "metrics": {
            str(k): {
                metric: value / query_count
                for metric, value in sorted(totals[k].items())
            }
            for k in chosen_k
        },
        "limitations": [
            "document qrels do not measure citation-span precision",
            "latency and generator answer quality are outside this evaluator",
        ],
    }
    if allow_deduplicated_aliases and alias_count:
        result["limitations"].append(
            "diagnostic alias expansion credits every source sharing a retrieved vector"
        )
    if output_path is not None:
        result["output_path"] = str(output_path.resolve())
        _atomic_json(result, output_path)
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--source-qrels", type=Path, required=True)
    parser.add_argument("--occurrences-jsonl", type=Path, required=True)
    parser.add_argument("--k", type=int, nargs="+", default=[1, 5, 10, 20, 100])
    parser.add_argument("--allow-deduplicated-aliases", action="store_true")
    parser.add_argument("--output", type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    result = evaluate_source_retrieval(
        args.run,
        args.source_qrels,
        args.occurrences_jsonl,
        k_values=tuple(args.k),
        allow_deduplicated_aliases=args.allow_deduplicated_aliases,
        output_path=args.output,
    )
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
