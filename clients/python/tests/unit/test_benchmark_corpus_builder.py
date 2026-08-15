"""Offline contract tests for the tracked benchmark corpus transport."""

from __future__ import annotations

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


def _load_builder_module():
    repo = Path(__file__).resolve().parents[4]
    path = repo / "scripts" / "bench" / "build_token_budget_corpus.py"
    spec = importlib.util.spec_from_file_location("build_token_budget_corpus", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_exporter_module():
    repo = Path(__file__).resolve().parents[4]
    path = repo / "scripts" / "bench" / "export_source_tree_jsonl.py"
    spec = importlib.util.spec_from_file_location("export_source_tree_jsonl", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_embedder_module():
    repo = Path(__file__).resolve().parents[4]
    path = repo / "scripts" / "bench" / "embed_open_corpus.py"
    spec = importlib.util.spec_from_file_location("embed_open_corpus", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class WordCounter:
    name = "test/word-counter"
    fingerprint = "word-counter-v1"
    advertised_limit = 8

    @staticmethod
    def count(text: str) -> int:
        return len(list(re.finditer(r"\S+", text))) + 2

    @staticmethod
    def content_offsets(text: str):
        return tuple(match.span() for match in re.finditer(r"\S+", text))


def _args(tmp_path: Path, *, max_chunks: int | None) -> Namespace:
    source = tmp_path / "documents.jsonl"
    source.write_text(
        json.dumps(
            {"id": "doc-1", "text": "one two three four five six seven eight nine"}
        )
        + "\n"
        + json.dumps({"id": "doc-2", "text": "ten eleven twelve"})
        + "\n",
        encoding="utf-8",
    )
    return Namespace(
        input=source,
        output_dir=tmp_path / "corpus",
        contracts=tmp_path / "unused.toml",
        text_field="text",
        id_field="id",
        strategy="fixed_size",
        target_tokens=7,
        overlap_tokens=1,
        min_content_tokens=3,
        boundary_char_size=10_000,
        overflow_policy="split",
        short_chunk_policy="keep",
        max_chunks=max_chunks,
        deduplicate="exact",
    )


def _contract() -> CompositeInputContract:
    return CompositeInputContract(
        (
            ResolvedInputContract(
                model_id="test/model",
                model_revision="immutable-revision",
                counter=WordCounter(),
                effective_context_limit=8,
                renderer=InputRenderer(document_template="passage: {text}"),
                native_dimension=8,
            ),
        )
    )


def test_builder_caps_deterministically_and_publishes_zero_truncation_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_builder_module()
    monkeypatch.setattr(builder, "_load_contracts", lambda _path: _contract())

    manifest = builder.build(_args(tmp_path, max_chunks=2))

    texts = json.loads((tmp_path / "corpus" / "texts.json").read_text())
    assert len(texts) == 2
    assert manifest["chunk_count"] == 2
    assert manifest["max_chunks"] == 2
    assert manifest["stopped_at_max_chunks"] is True
    assert manifest["zero_truncation_asserted"] is True
    assert manifest["token_histogram"]["test/model"]["max"] <= 7
    assert manifest["token_histogram"]["test/model"]["over_target"] == 0
    assert manifest["token_budget"]["overflow_policy"] == "split"
    assert manifest["deduplication_policy"] == "exact"


def test_builder_records_and_removes_exact_duplicate_chunks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_builder_module()
    monkeypatch.setattr(builder, "_load_contracts", lambda _path: _contract())
    args = _args(tmp_path, max_chunks=None)
    args.input.write_text(
        json.dumps({"id": "a", "text": "one two three"})
        + "\n"
        + json.dumps({"id": "b", "text": "one two three"})
        + "\n",
        encoding="utf-8",
    )

    manifest = builder.build(args)

    assert json.loads((tmp_path / "corpus" / "texts.json").read_text()) == [
        "one two three"
    ]
    assert manifest["duplicate_chunk_count"] == 1


def test_builder_rejects_a_non_positive_corpus_cap(tmp_path: Path):
    builder = _load_builder_module()
    with pytest.raises(ValueError, match="max_chunks must be positive"):
        builder.build(_args(tmp_path, max_chunks=0))


def test_builder_rejects_an_empty_emitted_corpus(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_builder_module()
    monkeypatch.setattr(builder, "_load_contracts", lambda _path: _contract())
    args = _args(tmp_path, max_chunks=None)
    args.input.write_text("", encoding="utf-8")

    with pytest.raises(ValueError, match="emitted no chunks"):
        builder.build(args)


def test_source_export_is_stable_sorted_and_excludes_skipped_or_symlinked_inputs(
    tmp_path: Path,
):
    exporter = _load_exporter_module()
    code_root = tmp_path / "code"
    repo = code_root / "repo-a"
    (repo / "src").mkdir(parents=True)
    (repo / "target").mkdir()
    (repo / "src" / "z.rs").write_text("fn z() {}\n", encoding="utf-8")
    (repo / "src" / "a.py").write_text("def a(): pass\n", encoding="utf-8")
    (repo / "src" / "ignored.bin").write_bytes(b"binary")
    (repo / "target" / "generated.rs").write_text("generated", encoding="utf-8")
    (repo / "src" / "link.rs").symlink_to(repo / "src" / "z.rs")
    output = tmp_path / "sources.jsonl"
    args = Namespace(
        code_root=code_root,
        repositories=["repo-a"],
        output=output,
        manifest=None,
        extensions=[".rs", ".py"],
        skip_dirs=["target"],
    )

    first = exporter.export(args)
    first_bytes = output.read_bytes()
    second = exporter.export(args)

    records = [json.loads(line) for line in output.read_text().splitlines()]
    assert [record["id"] for record in records] == [
        "repo-a/src/a.py",
        "repo-a/src/z.rs",
    ]
    assert first_bytes == output.read_bytes()
    assert first["source_inventory_sha256"] == second["source_inventory_sha256"]
    assert first["jsonl_sha256"] == second["jsonl_sha256"]
    assert first["document_count"] == 2


def test_embedding_shards_are_content_and_contract_addressed(tmp_path: Path):
    embedder = _load_embedder_module()
    paths = embedder.prepare_shards(
        output_dir=tmp_path,
        corpus_sha256="a" * 64,
        model_id="BAAI/bge-base-en-v1.5",
        revision="b" * 40,
        contract_fingerprint="c" * 64,
        dimension=3,
        shard_size=2,
        rows=5,
    )
    assert [path.name for path in paths] == [
        "00000000.npy",
        "00000002.npy",
        "00000004.npy",
    ]
    assert "bge-base-en-v1.5" in str(paths[0].parent)

    import numpy as np

    np.save(paths[0], np.zeros((2, 3), dtype=np.float32))
    pending = embedder.pending_shards(paths, rows=5, dimension=3, shard_size=2)
    assert pending == [(2, paths[1]), (4, paths[2])]


def test_embedding_finalizer_emits_query_base_headers_and_pca_spectrum(tmp_path: Path):
    embedder = _load_embedder_module()
    import numpy as np

    first = tmp_path / "first.npy"
    second = tmp_path / "second.npy"
    np.save(first, np.asarray([[0.0, 0.0], [1.0, 0.0]], dtype=np.float32))
    np.save(second, np.asarray([[0.0, 1.0], [1.0, 1.0]], dtype=np.float32))

    result = embedder.finalize_embeddings(
        [first, second],
        output_dir=tmp_path / "out",
        prefix="tiny",
        dimension=2,
        query_rows=1,
    )

    query = np.fromfile(tmp_path / "out" / "tiny_query.u8bin", dtype=np.uint8)
    base = np.fromfile(tmp_path / "out" / "tiny_base.u8bin", dtype=np.uint8)
    assert np.frombuffer(query[:8].tobytes(), dtype="<i4").tolist() == [1, 2]
    assert np.frombuffer(base[:8].tobytes(), dtype="<i4").tolist() == [3, 2]
    assert len(query) == 8 + 2
    assert len(base) == 8 + 6
    assert result["spectrum_full"]["0.7"] == 2
