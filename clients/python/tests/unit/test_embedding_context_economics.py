"""Contracts for token-only embedding corpus economics and span qrels."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import re
from argparse import Namespace
from pathlib import Path

import pytest

from proximadb_sdk.chunking_strategies import (
    CompositeInputContract,
    InputRenderer,
    ResolvedInputContract,
)


def _load_script(name: str):
    repo = Path(__file__).resolve().parents[4]
    path = repo / "scripts" / "bench" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class WordCounter:
    name = "test/word-counter"
    fingerprint = "word-counter-v1"
    advertised_limit = 32

    @staticmethod
    def count(text: str) -> int:
        return len(list(re.finditer(r"\S+", text))) + 2

    @staticmethod
    def content_offsets(text: str):
        return tuple(match.span() for match in re.finditer(r"\S+", text))


def _contract() -> CompositeInputContract:
    return CompositeInputContract(
        (
            ResolvedInputContract(
                model_id="test/model",
                model_revision="a" * 40,
                counter=WordCounter(),
                effective_context_limit=32,
                renderer=InputRenderer(),
                native_dimension=8,
                output_dimension=8,
            ),
        )
    )


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_exact_dedup_preserves_every_source_occurrence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_script("build_token_budget_corpus")
    source = tmp_path / "source.jsonl"
    source.write_text(
        "\n".join(
            json.dumps(
                {
                    "id": source_id,
                    "text": "one two three four five six seven eight nine",
                }
            )
            for source_id in ("doc-a", "doc-b")
        )
        + "\n",
        encoding="utf-8",
    )
    args = Namespace(
        input=source,
        output_dir=tmp_path / "corpus",
        contracts=tmp_path / "unused.toml",
        text_field="text",
        id_field="id",
        strategy="fixed_size",
        target_tokens=7,
        overlap_tokens=1,
        min_content_tokens=1,
        boundary_char_size=10_000,
        overflow_policy="split",
        short_chunk_policy="keep",
        max_chunks=None,
        allow_partial_corpus=False,
        deduplicate="exact",
    )
    monkeypatch.setattr(builder, "_load_contracts", lambda _path: _contract())

    manifest = builder.build(args)

    occurrences_path = args.output_dir / "chunk_occurrences.jsonl"
    occurrences = [
        json.loads(line)
        for line in occurrences_path.read_text(encoding="utf-8").splitlines()
    ]
    assert manifest["schema_version"] == 2
    assert manifest["chunk_count"] == 2
    assert manifest["chunk_occurrence_count"] == 4
    assert manifest["duplicate_chunk_count"] == 2
    assert manifest["chunk_occurrences_sha256"] == _sha256(occurrences_path)
    assert manifest["boundary_char_size"] == 10_000
    assert manifest["source_fields"] == {"id": "id", "text": "text"}
    assert [item["source_id"] for item in occurrences] == [
        "doc-a",
        "doc-a",
        "doc-b",
        "doc-b",
    ]
    assert occurrences[0]["corpus_id"] == occurrences[2]["corpus_id"]
    assert occurrences[1]["corpus_id"] == occurrences[3]["corpus_id"]
    assert [item["deduplicated_alias"] for item in occurrences] == [
        False,
        False,
        True,
        True,
    ]


def _write_economics_fixture(tmp_path: Path) -> tuple[Path, Path, Path]:
    chunks = tmp_path / "chunks.jsonl"
    chunks.write_text(
        "\n".join(
            (
                json.dumps(
                    {
                        "chunk_id": "c1",
                        "source_id": "doc-a",
                        "start_pos": 0,
                        "end_pos": 4,
                        "token_counts": {"test/model": 4},
                    }
                ),
                json.dumps(
                    {
                        "chunk_id": "c2",
                        "source_id": "doc-a",
                        "start_pos": 4,
                        "end_pos": 8,
                        "token_counts": {"test/model": 6},
                    }
                ),
            )
        )
        + "\n",
        encoding="utf-8",
    )
    occurrences = tmp_path / "chunk_occurrences.jsonl"
    occurrences.write_text(
        "\n".join(
            json.dumps(item)
            for item in (
                {
                    "corpus_id": "c1",
                    "source_id": "doc-a",
                    "start_pos": 0,
                    "end_pos": 4,
                    "deduplicated_alias": False,
                },
                {
                    "corpus_id": "c2",
                    "source_id": "doc-a",
                    "start_pos": 4,
                    "end_pos": 8,
                    "deduplicated_alias": False,
                },
                {
                    "corpus_id": "c1",
                    "source_id": "doc-b",
                    "start_pos": 0,
                    "end_pos": 4,
                    "deduplicated_alias": True,
                },
            )
        )
        + "\n",
        encoding="utf-8",
    )
    manifest = tmp_path / "corpus.manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "corpus_scope": "complete_source",
                "zero_truncation_asserted": True,
                "source_count": 2,
                "chunk_count": 2,
                "chunk_occurrence_count": 3,
                "duplicate_chunk_count": 1,
                "deduplication_policy": "exact",
                "chunks_sha256": _sha256(chunks),
                "chunk_occurrences_sha256": _sha256(occurrences),
                "token_budget": {"target_tokens": 8, "overlap_tokens": 1},
                "input_contract": {
                    "contracts": [
                        {
                            "model_id": "test/model",
                            "output_dimension": 8,
                            "effective_context_limit": 32,
                        }
                    ]
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    return manifest, chunks, occurrences


def test_economics_separates_canonical_rows_from_source_occurrences(tmp_path: Path):
    economics = _load_script("embedding_context_economics")
    manifest, chunks, occurrences = _write_economics_fixture(tmp_path)

    result = economics.analyze_corpus(
        manifest,
        chunks_path=chunks,
        occurrences_path=occurrences,
        dimensions=(8,),
        fixed_bytes_per_row=12,
    )

    assert result["row_economics"] == {
        "canonical_rows": 2,
        "source_occurrence_rows": 3,
        "deduplicated_alias_rows": 1,
        "source_count": 2,
        "sources_with_occurrences": 2,
        "sources_without_occurrences": 0,
        "occurrences_per_source": {
            "min": 1,
            "p50": 1,
            "p90": 2,
            "p99": 2,
            "max": 2,
        },
    }
    assert result["tokens_by_model"]["test/model"] == {
        "rendered_tokens": 10,
        "attention_quadratic_proxy": 52,
        "histogram": {"min": 4, "p50": 4, "p90": 6, "p99": 6, "max": 6},
    }
    dimension = result["storage_by_dimension"]["8"]
    assert dimension["vector_payload_bytes"] == {
        "float32": 64,
        "float16": 32,
        "sq8": 16,
    }
    assert dimension["fixed_row_overhead_bytes"] == 24
    assert dimension["one_update_per_source_vector_bytes"] == {
        "float32": 96,
        "float16": 48,
        "sq8": 24,
    }


def test_span_qrels_project_through_deduplicated_occurrences(tmp_path: Path):
    projector = _load_script("project_span_qrels")
    _manifest, _chunks, occurrences = _write_economics_fixture(tmp_path)
    source_qrels = tmp_path / "source-qrels.jsonl"
    source_qrels.write_text(
        "\n".join(
            (
                json.dumps(
                    {
                        "query_id": "q1",
                        "source_id": "doc-a",
                        "start_pos": 3,
                        "end_pos": 5,
                        "relevance": 2,
                    }
                ),
                json.dumps({"query_id": "q2", "source_id": "doc-b", "relevance": 1}),
            )
        )
        + "\n",
        encoding="utf-8",
    )
    output = tmp_path / "qrels.jsonl"

    summary = projector.project_qrels(source_qrels, occurrences, output)

    projected = [
        json.loads(line) for line in output.read_text(encoding="utf-8").splitlines()
    ]
    assert projected == [
        {"corpus_id": "c1", "query_id": "q1", "relevance": 2.0},
        {"corpus_id": "c2", "query_id": "q1", "relevance": 2.0},
        {"corpus_id": "c1", "query_id": "q2", "relevance": 1.0},
    ]
    assert summary["source_relation_count"] == 2
    assert summary["projected_relation_count"] == 3
    assert summary["query_count"] == 2


def test_span_qrels_reject_invalid_or_unmatched_spans(tmp_path: Path):
    projector = _load_script("project_span_qrels")
    _manifest, _chunks, occurrences = _write_economics_fixture(tmp_path)
    source_qrels = tmp_path / "source-qrels.jsonl"
    output = tmp_path / "qrels.jsonl"
    source_qrels.write_text(
        json.dumps(
            {
                "query_id": "q1",
                "source_id": "doc-a",
                "start_pos": 8,
                "end_pos": 8,
                "relevance": 1,
            }
        )
        + "\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="end_pos must be greater"):
        projector.project_qrels(source_qrels, occurrences, output)

    source_qrels.write_text(
        json.dumps(
            {
                "query_id": "q1",
                "source_id": "missing",
                "relevance": 1,
            }
        )
        + "\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="matched no corpus occurrences"):
        projector.project_qrels(source_qrels, occurrences, output)
