"""Contracts for token-only embedding corpus economics and span qrels."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import math
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
    contracts = tmp_path / "unused.toml"
    contracts.write_text(
        f"[[models]]\nmodel_id = \"test/model\"\nrevision = \"{'a' * 40}\"\n"
        "effective_context_limit = 32\n",
        encoding="utf-8",
    )
    args = Namespace(
        input=source,
        output_dir=tmp_path / "corpus",
        contracts=contracts,
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


def test_builder_rejects_duplicate_source_ids(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_script("build_token_budget_corpus")
    source = tmp_path / "source.jsonl"
    source.write_text(
        json.dumps({"id": "duplicate", "text": "one two three four five six"})
        + "\n"
        + json.dumps({"id": "duplicate", "text": "seven eight nine ten"})
        + "\n",
        encoding="utf-8",
    )
    contracts = tmp_path / "unused.toml"
    contracts.write_text(
        f"[[models]]\nmodel_id = \"test/model\"\nrevision = \"{'a' * 40}\"\n"
        "effective_context_limit = 32\n",
        encoding="utf-8",
    )
    args = Namespace(
        input=source,
        output_dir=tmp_path / "corpus",
        contracts=contracts,
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

    with pytest.raises(ValueError, match="duplicate source id 'duplicate'"):
        builder.build(args)


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

    summary = projector.project_qrels(source_qrels, _manifest, occurrences, output)

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
    projection_manifest = tmp_path / "qrels.projection.manifest.json"
    assert json.loads(projection_manifest.read_text(encoding="utf-8")) == summary


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
        projector.project_qrels(source_qrels, _manifest, occurrences, output)

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
        projector.project_qrels(source_qrels, _manifest, occurrences, output)


def test_span_qrels_reject_occurrence_manifest_drift(tmp_path: Path):
    projector = _load_script("project_span_qrels")
    manifest, _chunks, occurrences = _write_economics_fixture(tmp_path)
    source_qrels = tmp_path / "source-qrels.jsonl"
    source_qrels.write_text(
        json.dumps({"query_id": "q1", "source_id": "doc-a", "relevance": 1}) + "\n",
        encoding="utf-8",
    )
    occurrences.write_text(
        occurrences.read_text(encoding="utf-8") + "{}\n", encoding="utf-8"
    )

    with pytest.raises(ValueError, match="does not match the corpus manifest"):
        projector.project_qrels(
            source_qrels, manifest, occurrences, tmp_path / "qrels.jsonl"
        )


def _write_beir_fixture(tmp_path: Path, *, corpus_id: str = "doc-1") -> Path:
    root = tmp_path / "beir"
    (root / "qrels").mkdir(parents=True)
    (root / "corpus.jsonl").write_text(
        json.dumps({"_id": "doc-2", "title": "", "text": "second"})
        + "\n"
        + json.dumps({"_id": "doc-1", "title": "First title", "text": "first body"})
        + "\n",
        encoding="utf-8",
    )
    (root / "queries.jsonl").write_text(
        json.dumps({"_id": "q-unused", "text": "unused"})
        + "\n"
        + json.dumps({"_id": "q1", "text": "first query"})
        + "\n",
        encoding="utf-8",
    )
    (root / "qrels" / "test.tsv").write_text(
        "query-id\tcorpus-id\tscore\n" + f"q1\t{corpus_id}\t2\n",
        encoding="utf-8",
    )
    return root


def test_beir_import_preserves_document_level_judgment_semantics(tmp_path: Path):
    importer = _load_script("import_beir_dataset")
    root = _write_beir_fixture(tmp_path)
    output = tmp_path / "normalized"

    manifest = importer.import_dataset(
        root,
        split="test",
        output_dir=output,
        dataset_name="fixture",
        source_url="https://example.test/fixture.zip",
    )

    documents = [
        json.loads(line)
        for line in (output / "documents.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
    ]
    queries = [
        json.loads(line)
        for line in (output / "queries.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    source_qrels = [
        json.loads(line)
        for line in (output / "source_qrels.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
    ]
    assert documents == [
        {
            "body": "first body",
            "id": "doc-1",
            "text": "First title\n\nfirst body",
            "title": "First title",
        },
        {"body": "second", "id": "doc-2", "text": "second", "title": ""},
    ]
    assert queries == [{"id": "q1", "text": "first query"}]
    assert source_qrels == [
        {
            "judgment_granularity": "document",
            "query_id": "q1",
            "relevance": 2.0,
            "source_id": "doc-1",
        }
    ]
    assert manifest["judgment_granularity"] == "document"
    assert manifest["document_count"] == 2
    assert manifest["query_count"] == 1
    assert manifest["relation_count"] == 1
    assert manifest["license_review_required"] is True


def test_beir_import_rejects_unknown_qrel_documents(tmp_path: Path):
    importer = _load_script("import_beir_dataset")
    root = _write_beir_fixture(tmp_path, corpus_id="missing")

    with pytest.raises(ValueError, match="unknown corpus id 'missing'"):
        importer.import_dataset(
            root,
            split="test",
            output_dir=tmp_path / "normalized",
            dataset_name="fixture",
            source_url="https://example.test/fixture.zip",
        )


def _write_source_evaluation_fixture(
    tmp_path: Path, *, include_alias: bool = False
) -> tuple[Path, Path, Path]:
    occurrences = tmp_path / "occurrences.jsonl"
    occurrence_rows = [
        {
            "corpus_id": "c1",
            "source_id": "doc-a",
            "start_pos": 0,
            "end_pos": 5,
            "deduplicated_alias": False,
        },
        {
            "corpus_id": "c2",
            "source_id": "doc-a",
            "start_pos": 5,
            "end_pos": 10,
            "deduplicated_alias": False,
        },
        {
            "corpus_id": "c3",
            "source_id": "doc-b",
            "start_pos": 0,
            "end_pos": 5,
            "deduplicated_alias": False,
        },
    ]
    if include_alias:
        occurrence_rows.append(
            {
                "corpus_id": "c1",
                "source_id": "doc-alias",
                "start_pos": 0,
                "end_pos": 5,
                "deduplicated_alias": True,
            }
        )
    occurrences.write_text(
        "\n".join(json.dumps(row) for row in occurrence_rows) + "\n",
        encoding="utf-8",
    )
    run = tmp_path / "run.jsonl"
    run.write_text(
        "\n".join(
            json.dumps(row)
            for row in (
                {"query_id": "q1", "corpus_id": "c2", "score": 0.8},
                {"query_id": "q1", "corpus_id": "c1", "score": 0.9},
                {"query_id": "q1", "corpus_id": "c3", "score": 0.7},
            )
        )
        + "\n",
        encoding="utf-8",
    )
    qrels = tmp_path / "source-qrels.jsonl"
    qrels.write_text(
        "\n".join(
            json.dumps(row)
            for row in (
                {
                    "query_id": "q1",
                    "source_id": "doc-a",
                    "relevance": 2,
                    "judgment_granularity": "document",
                },
                {
                    "query_id": "q1",
                    "source_id": "doc-b",
                    "relevance": 1,
                    "judgment_granularity": "document",
                },
            )
        )
        + "\n",
        encoding="utf-8",
    )
    return run, qrels, occurrences


def test_source_retrieval_collapses_chunk_scores_before_metrics(tmp_path: Path):
    evaluator = _load_script("evaluate_source_retrieval")
    run, qrels, occurrences = _write_source_evaluation_fixture(tmp_path)
    per_query = tmp_path / "per-query.jsonl"

    result = evaluator.evaluate_source_retrieval(
        run, qrels, occurrences, k_values=(1, 2), per_query_output=per_query
    )

    assert result["metrics"]["1"] == {
        "average_precision": 1.0,
        "capped_recall": 1.0,
        "ceiling_normalized_recall": 1.0,
        "hit_rate": 1.0,
        "mrr": 1.0,
        "ndcg": 1.0,
        "perfect_recall_ceiling": 0.5,
        "precision": 1.0,
        "recall": 0.5,
    }
    assert result["metrics"]["2"] == {
        "average_precision": 1.0,
        "capped_recall": 1.0,
        "ceiling_normalized_recall": 1.0,
        "hit_rate": 1.0,
        "mrr": 1.0,
        "ndcg": 1.0,
        "perfect_recall_ceiling": 1.0,
        "precision": 1.0,
        "recall": 1.0,
    }
    assert result["relevant_documents_per_query"] == {
        "min": 2,
        "p50": 2,
        "p90": 2,
        "p99": 2,
        "max": 2,
    }
    assert result["candidate_completeness"] == {
        "1": {
            "complete": True,
            "complete_queries": 1,
            "incomplete_queries": 0,
            "required_candidates_per_query": 1,
        },
        "2": {
            "complete": True,
            "complete_queries": 1,
            "incomplete_queries": 0,
            "required_candidates_per_query": 2,
        },
    }
    assert result["source_candidates_per_query"] == {
        "min": 2,
        "p50": 2,
        "p90": 2,
        "p99": 2,
        "max": 2,
    }
    assert result["input_run_row_count"] == 3
    assert result["source_run_row_count"] == 2
    assert result["per_query_diagnostics"]["sha256"] == _sha256(per_query)
    assert json.loads(per_query.read_text(encoding="utf-8")) == {
        "candidate_count": 2,
        "metrics": {
            "1": {
                "average_precision": 1.0,
                "capped_recall": 1.0,
                "hit_rate": 1.0,
                "mrr": 1.0,
                "ndcg": 1.0,
                "perfect_recall_ceiling": 0.5,
                "precision": 1.0,
                "recall": 0.5,
            },
            "2": {
                "average_precision": 1.0,
                "capped_recall": 1.0,
                "hit_rate": 1.0,
                "mrr": 1.0,
                "ndcg": 1.0,
                "perfect_recall_ceiling": 1.0,
                "precision": 1.0,
                "recall": 1.0,
            },
        },
        "query_id": "q1",
        "relevant_document_count": 2,
    }


def test_source_retrieval_rejects_deduplicated_aliases_by_default(tmp_path: Path):
    evaluator = _load_script("evaluate_source_retrieval")
    run, qrels, occurrences = _write_source_evaluation_fixture(
        tmp_path, include_alias=True
    )

    with pytest.raises(ValueError, match="deduplicated aliases"):
        evaluator.evaluate_source_retrieval(run, qrels, occurrences, k_values=(1, 2))


def test_source_metric_golden_values_cover_rank_aware_diagnostics():
    evaluator = _load_script("evaluate_source_retrieval")

    metrics = evaluator._metrics_at_k(
        ["relevant-high", "not-relevant", "relevant-low"],
        {"relevant-high": 2.0, "relevant-low": 1.0},
        3,
    )

    assert metrics["precision"] == pytest.approx(2 / 3)
    assert metrics["recall"] == 1.0
    assert metrics["capped_recall"] == 1.0
    assert metrics["hit_rate"] == 1.0
    assert metrics["mrr"] == 1.0
    assert metrics["average_precision"] == pytest.approx((1.0 + 2 / 3) / 2)
    assert metrics["ndcg"] == pytest.approx(
        (2.0 + 1.0 / math.log2(4)) / (2.0 + 1.0 / math.log2(3))
    )


def test_source_retrieval_marks_and_can_reject_incomplete_candidate_depth(
    tmp_path: Path,
):
    evaluator = _load_script("evaluate_source_retrieval")
    run, qrels, occurrences = _write_source_evaluation_fixture(tmp_path)
    occurrences.write_text(
        occurrences.read_text(encoding="utf-8")
        + json.dumps(
            {
                "corpus_id": "c-unretrieved",
                "source_id": "doc-unretrieved",
                "start_pos": 0,
                "end_pos": 5,
                "deduplicated_alias": False,
            }
        )
        + "\n",
        encoding="utf-8",
    )

    result = evaluator.evaluate_source_retrieval(
        run, qrels, occurrences, k_values=(2, 3)
    )
    assert result["candidate_completeness"]["2"]["complete"] is True
    assert result["candidate_completeness"]["3"] == {
        "complete": False,
        "complete_queries": 0,
        "incomplete_queries": 1,
        "required_candidates_per_query": 3,
    }
    assert any("@3" in item for item in result["limitations"])

    with pytest.raises(ValueError, match="complete source candidates at k=3"):
        evaluator.evaluate_source_retrieval(
            run,
            qrels,
            occurrences,
            k_values=(2, 3),
            require_complete_k=(3,),
        )


def test_source_retrieval_accepts_source_level_runs(tmp_path: Path):
    evaluator = _load_script("evaluate_source_retrieval")
    _chunk_run, qrels, occurrences = _write_source_evaluation_fixture(tmp_path)
    source_run = tmp_path / "source-run.jsonl"
    source_run.write_text(
        "\n".join(
            json.dumps(row)
            for row in (
                {"query_id": "q1", "source_id": "doc-b", "score": 0.9},
                {"query_id": "q1", "source_id": "doc-a", "score": 0.8},
            )
        )
        + "\n",
        encoding="utf-8",
    )

    result = evaluator.evaluate_source_retrieval(
        source_run,
        qrels,
        occurrences,
        k_values=(1, 2),
        run_granularity="source",
    )

    assert result["run_granularity"] == "source"
    assert result["metrics"]["2"]["recall"] == 1.0


def test_reference_bm25_scores_normalized_jsonl_deterministically(tmp_path: Path):
    bm25 = _load_script("score_bm25_corpus")
    documents = tmp_path / "documents.jsonl"
    documents.write_text(
        "\n".join(
            json.dumps(row)
            for row in (
                {"id": "doc-b", "text": "beta beta unrelated"},
                {"id": "doc-a", "text": "alpha beta"},
                {"id": "doc-c", "text": "alpha alpha alpha"},
            )
        )
        + "\n",
        encoding="utf-8",
    )
    queries = tmp_path / "queries.jsonl"
    queries.write_text(
        json.dumps({"id": "q1", "text": "alpha"}) + "\n",
        encoding="utf-8",
    )
    output = tmp_path / "bm25.jsonl"

    manifest = bm25.score_bm25_corpus(
        documents, queries, output, top_k=3, k1=0.9, b=0.4
    )

    rows = [json.loads(line) for line in output.read_text().splitlines()]
    assert [row["source_id"] for row in rows] == ["doc-c", "doc-a", "doc-b"]
    assert rows[-1]["score"] == 0.0
    assert manifest["analyzer"] == "unicode_word_casefold_v1"
    assert manifest["scoring"] == "Okapi BM25"
    assert manifest["run_sha256"] == _sha256(output)


def test_rrf_fuses_source_runs_with_deterministic_ties(tmp_path: Path):
    fusion = _load_script("fuse_source_runs")
    dense = tmp_path / "dense.jsonl"
    lexical = tmp_path / "lexical.jsonl"
    dense.write_text(
        "\n".join(
            json.dumps(
                {
                    "query_id": "q1",
                    "source_id": source_id,
                    "rank": rank,
                    "score": 1 / rank,
                }
            )
            for rank, source_id in enumerate(("doc-a", "doc-b", "doc-c"), 1)
        )
        + "\n",
        encoding="utf-8",
    )
    lexical.write_text(
        "\n".join(
            json.dumps(
                {
                    "query_id": "q1",
                    "source_id": source_id,
                    "rank": rank,
                    "score": 1 / rank,
                }
            )
            for rank, source_id in enumerate(("doc-b", "doc-c", "doc-a"), 1)
        )
        + "\n",
        encoding="utf-8",
    )
    output = tmp_path / "rrf.jsonl"

    manifest = fusion.fuse_source_runs(
        (dense, lexical), output, labels=("dense", "lexical"), top_k=3, rrf_k=60
    )

    rows = [json.loads(line) for line in output.read_text().splitlines()]
    assert [row["source_id"] for row in rows] == ["doc-b", "doc-a", "doc-c"]
    assert manifest["fusion"] == "reciprocal_rank_fusion"
    assert manifest["run_sha256"] == _sha256(output)


def test_paired_bootstrap_comparison_is_query_aligned_and_deterministic(
    tmp_path: Path,
):
    comparison = _load_script("compare_retrieval_runs")
    baseline = tmp_path / "baseline.jsonl"
    candidate = tmp_path / "candidate.jsonl"

    def write(path: Path, values: tuple[float, float]) -> None:
        path.write_text(
            "\n".join(
                json.dumps(
                    {
                        "query_id": query_id,
                        "metrics": {"10": {"recall": value}},
                    }
                )
                for query_id, value in zip(("q1", "q2"), values, strict=True)
            )
            + "\n",
            encoding="utf-8",
        )

    write(baseline, (0.1, 0.4))
    write(candidate, (0.2, 0.3))

    first = comparison.compare_runs(
        baseline,
        candidate,
        k=10,
        metric="recall",
        bootstrap_samples=200,
        seed=7,
    )
    second = comparison.compare_runs(
        baseline,
        candidate,
        k=10,
        metric="recall",
        bootstrap_samples=200,
        seed=7,
    )

    assert first == second
    assert first["query_count"] == 2
    assert first["baseline_mean"] == pytest.approx(0.25)
    assert first["candidate_mean"] == pytest.approx(0.25)
    assert first["mean_delta"] == pytest.approx(0.0)
    assert first["paired_outcomes"] == {"candidate_wins": 1, "ties": 0, "losses": 1}
    assert first["confidence_interval"]["low"] <= 0
    assert first["confidence_interval"]["high"] >= 0


def test_paired_bootstrap_rejects_different_query_sets(tmp_path: Path):
    comparison = _load_script("compare_retrieval_runs")
    baseline = tmp_path / "baseline.jsonl"
    candidate = tmp_path / "candidate.jsonl"
    baseline.write_text(
        json.dumps({"query_id": "q1", "metrics": {"10": {"recall": 1.0}}}) + "\n",
        encoding="utf-8",
    )
    candidate.write_text(
        json.dumps({"query_id": "q2", "metrics": {"10": {"recall": 1.0}}}) + "\n",
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="query coverage differs"):
        comparison.compare_runs(
            baseline,
            candidate,
            k=10,
            metric="recall",
            bootstrap_samples=20,
            seed=1,
        )


def test_paired_bootstrap_compares_explicit_candidate_pool_depths(tmp_path: Path):
    comparison = _load_script("compare_retrieval_runs")
    baseline = tmp_path / "baseline.jsonl"
    candidate = tmp_path / "candidate.jsonl"
    baseline.write_text(
        json.dumps({"query_id": "q1", "metrics": {"100": {"recall": 0.3}}}) + "\n",
        encoding="utf-8",
    )
    candidate.write_text(
        json.dumps({"query_id": "q1", "metrics": {"200": {"recall": 0.4}}}) + "\n",
        encoding="utf-8",
    )

    result = comparison.compare_runs(
        baseline,
        candidate,
        baseline_k=100,
        candidate_k=200,
        metric="recall",
        bootstrap_samples=20,
        seed=1,
    )

    assert result["baseline_k"] == 100
    assert result["candidate_k"] == 200
    assert result["mean_delta"] == pytest.approx(0.1)
