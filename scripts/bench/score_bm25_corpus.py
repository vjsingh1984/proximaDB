#!/usr/bin/env python3
"""Score normalized JSONL documents with a sealed reference BM25 control.

This is an evaluation control, not ProximaDB's production lexical engine.  Its
small, dependency-free analyzer is deliberately recorded in the manifest so a
result cannot be confused with Lucene/Tantivy or an official BEIR baseline.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import tempfile
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

_WORD = re.compile(r"\w+", re.UNICODE)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _tokens(text: str) -> tuple[str, ...]:
    return tuple(match.group(0).casefold() for match in _WORD.finditer(text))


def _load_records(path: Path, *, kind: str) -> list[tuple[str, str]]:
    records: list[tuple[str, str]] = []
    seen: set[str] = set()
    with path.open("r", encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            if not line.strip():
                continue
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{kind} line {line_number}: expected an object")
            record_id = value.get("id")
            text = value.get("text")
            if not isinstance(record_id, str) or not record_id.strip():
                raise ValueError(f"{kind} line {line_number}: id is required")
            if record_id in seen:
                raise ValueError(
                    f"{kind} line {line_number}: duplicate id {record_id!r}"
                )
            if not isinstance(text, str) or not text.strip():
                raise ValueError(f"{kind} line {line_number}: text is required")
            seen.add(record_id)
            records.append((record_id, text))
    if not records:
        raise ValueError(f"{kind} contains no records")
    return records


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


def _atomic_jsonl(records: list[dict[str, Any]], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f"{path.name}.", suffix=".tmp", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with temporary.open("w", encoding="utf-8") as output:
            for record in records:
                output.write(json.dumps(record, sort_keys=True) + "\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def score_bm25_corpus(
    documents_path: Path,
    queries_path: Path,
    output_path: Path,
    *,
    top_k: int,
    k1: float = 0.9,
    b: float = 0.4,
    manifest_path: Path | None = None,
) -> dict[str, Any]:
    """Build an in-memory inverted index and emit a deterministic source run."""

    if isinstance(top_k, bool) or not isinstance(top_k, int) or top_k <= 0:
        raise ValueError("top_k must be a positive integer")
    if not math.isfinite(k1) or k1 <= 0:
        raise ValueError("k1 must be finite and positive")
    if not math.isfinite(b) or not 0 <= b <= 1:
        raise ValueError("b must be finite and between zero and one")

    documents = _load_records(documents_path, kind="documents")
    queries = _load_records(queries_path, kind="queries")
    chosen_k = min(top_k, len(documents))
    doc_ids = tuple(record_id for record_id, _text in documents)
    document_tokens = [_tokens(text) for _record_id, text in documents]
    lengths = [len(tokens) for tokens in document_tokens]
    if any(length == 0 for length in lengths):
        raise ValueError("documents must contain at least one analyzer token")
    average_length = sum(lengths) / len(lengths)

    postings: dict[str, list[tuple[int, int]]] = defaultdict(list)
    for index, tokens in enumerate(document_tokens):
        for term, frequency in Counter(tokens).items():
            postings[term].append((index, frequency))

    rows: list[dict[str, Any]] = []
    document_count = len(documents)
    for query_id, query_text in queries:
        scores = [0.0] * document_count
        for term, query_frequency in Counter(_tokens(query_text)).items():
            term_postings = postings.get(term, ())
            document_frequency = len(term_postings)
            if not document_frequency:
                continue
            inverse_document_frequency = math.log(
                1.0
                + (document_count - document_frequency + 0.5)
                / (document_frequency + 0.5)
            )
            for document_index, term_frequency in term_postings:
                length_normalization = (
                    1.0 - b + b * (lengths[document_index] / average_length)
                )
                scores[document_index] += query_frequency * (
                    inverse_document_frequency
                    * term_frequency
                    * (k1 + 1.0)
                    / (term_frequency + k1 * length_normalization)
                )
        ranked = sorted(
            zip(scores, doc_ids, strict=True), key=lambda item: (-item[0], item[1])
        )[:chosen_k]
        rows.extend(
            {
                "query_id": query_id,
                "rank": rank,
                "score": score,
                "source_id": source_id,
            }
            for rank, (score, source_id) in enumerate(ranked, 1)
        )

    _atomic_jsonl(rows, output_path)
    manifest_path = manifest_path or output_path.with_name(
        f"{output_path.stem}.scoring.manifest.json"
    )
    result = {
        "schema_version": 1,
        "producer_sha256": _sha256(Path(__file__).resolve()),
        "scoring": "Okapi BM25",
        "analyzer": "unicode_word_casefold_v1",
        "parameters": {"b": b, "k1": k1},
        "tie_break": "source_id ascending",
        "top_k_requested": top_k,
        "top_k_emitted": chosen_k,
        "document_count": document_count,
        "query_count": len(queries),
        "documents_path": str(documents_path.resolve()),
        "documents_sha256": _sha256(documents_path),
        "queries_path": str(queries_path.resolve()),
        "queries_sha256": _sha256(queries_path),
        "run_path": str(output_path.resolve()),
        "run_sha256": _sha256(output_path),
        "run_row_count": len(rows),
        "limitations": [
            "reference analyzer is not Lucene or Tantivy",
            "scores are an evaluation control, not production serving evidence",
        ],
    }
    _atomic_json(result, manifest_path)
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--documents", type=Path, required=True)
    parser.add_argument("--queries", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--top-k", type=int, default=100)
    parser.add_argument("--k1", type=float, default=0.9)
    parser.add_argument("--b", type=float, default=0.4)
    parser.add_argument("--manifest", type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    result = score_bm25_corpus(
        args.documents,
        args.queries,
        args.output,
        top_k=args.top_k,
        k1=args.k1,
        b=args.b,
        manifest_path=args.manifest,
    )
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
