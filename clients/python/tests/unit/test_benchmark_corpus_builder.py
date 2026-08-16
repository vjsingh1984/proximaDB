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
        allow_partial_corpus=False,
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


def test_builder_rejects_an_accidentally_partial_corpus(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_builder_module()
    monkeypatch.setattr(builder, "_load_contracts", lambda _path: _contract())

    with pytest.raises(ValueError, match="partial corpus"):
        builder.build(_args(tmp_path, max_chunks=2))

    assert not (tmp_path / "corpus" / "texts.json").exists()
    assert not (tmp_path / "corpus" / "chunks.jsonl").exists()


def test_builder_caps_deterministically_only_when_partial_scope_is_explicit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    builder = _load_builder_module()
    monkeypatch.setattr(builder, "_load_contracts", lambda _path: _contract())
    args = _args(tmp_path, max_chunks=2)
    args.allow_partial_corpus = True

    manifest = builder.build(args)

    texts = json.loads((tmp_path / "corpus" / "texts.json").read_text())
    assert len(texts) == 2
    assert manifest["chunk_count"] == 2
    assert manifest["max_chunks"] == 2
    assert manifest["stopped_at_max_chunks"] is True
    assert manifest["corpus_scope"] == "partial_prefix"
    assert manifest["zero_truncation_asserted"] is True
    assert manifest["builder_sha256"] == builder._sha256(Path(builder.__file__))
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
    assert first["exporter_sha256"] == exporter._sha256(Path(exporter.__file__))
    assert first["repository_provenance"]["repo-a"]["vcs"] == "unversioned"


def test_source_export_records_git_revision_and_dirty_state(tmp_path: Path):
    exporter = _load_exporter_module()
    code_root = tmp_path / "code"
    repo = code_root / "repo-a"
    repo.mkdir(parents=True)
    (repo / "tracked.py").write_text("print('clean')\n", encoding="utf-8")
    output = tmp_path / "sources.jsonl"
    args = Namespace(
        code_root=code_root,
        repositories=["repo-a"],
        output=output,
        manifest=None,
        extensions=[".py"],
        skip_dirs=[],
    )

    import subprocess

    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "tracked.py"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
        check=True,
    )

    clean = exporter.export(args)
    revision = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    assert clean["repository_provenance"]["repo-a"] == {
        "dirty": False,
        "head_revision": revision,
        "status_sha256": exporter.hashlib.sha256(b"").hexdigest(),
        "vcs": "git",
    }

    (repo / "tracked.py").write_text("print('dirty')\n", encoding="utf-8")
    dirty = exporter.export(args)
    assert dirty["repository_provenance"]["repo-a"]["dirty"] is True
    assert dirty["repository_provenance"]["repo-a"]["head_revision"] == revision


def test_source_export_fails_closed_on_missing_or_unreadable_sources(tmp_path: Path):
    exporter = _load_exporter_module()
    code_root = tmp_path / "code"
    code_root.mkdir()
    output = tmp_path / "sources.jsonl"
    args = Namespace(
        code_root=code_root,
        repositories=["missing"],
        output=output,
        manifest=None,
        extensions=[".py"],
        skip_dirs=[],
        allow_unreadable_sources=False,
    )

    with pytest.raises(ValueError, match="missing source repositories"):
        exporter.export(args)

    repo = code_root / "repo-a"
    repo.mkdir()
    (repo / "invalid.py").write_bytes(b"valid-prefix\xffinvalid")
    args.repositories = ["repo-a"]
    with pytest.raises(ValueError, match="unreadable source files"):
        exporter.export(args)
    assert not output.exists()


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

    query_paths = embedder.prepare_shards(
        output_dir=tmp_path,
        corpus_sha256="a" * 64,
        model_id="BAAI/bge-base-en-v1.5",
        revision="b" * 40,
        contract_fingerprint="c" * 64,
        dimension=3,
        shard_size=2,
        rows=5,
        input_role="query",
    )
    assert query_paths[0].parent != paths[0].parent


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
    assert result["evaluation_mode"] == "geometry_probe"


def test_embedding_finalizer_keeps_qrels_queries_out_of_the_passage_base(
    tmp_path: Path,
):
    embedder = _load_embedder_module()
    import numpy as np

    base_shard = tmp_path / "base.npy"
    query_shard = tmp_path / "query.npy"
    np.save(
        base_shard,
        np.asarray(
            [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            dtype=np.float32,
        ),
    )
    np.save(query_shard, np.asarray([[0.25, 0.25], [0.75, 0.75]], dtype=np.float32))

    result = embedder.finalize_embeddings(
        [base_shard],
        output_dir=tmp_path / "out",
        prefix="qrels",
        dimension=2,
        query_rows=0,
        query_shard_paths=[query_shard],
    )

    query = np.fromfile(tmp_path / "out" / "qrels_query.u8bin", dtype=np.uint8)
    base = np.fromfile(tmp_path / "out" / "qrels_base.u8bin", dtype=np.uint8)
    assert np.frombuffer(query[:8].tobytes(), dtype="<i4").tolist() == [2, 2]
    assert np.frombuffer(base[:8].tobytes(), dtype="<i4").tolist() == [4, 2]
    assert result["base_rows"] == 4
    assert result["query_rows"] == 2
    assert result["evaluation_mode"] == "qrels"
    assert result["quantization_min"] == 0.0
    assert result["quantization_max"] == 1.0
    assert result["query_clip_low_count"] == 0
    assert result["query_clip_high_count"] == 0


def test_qrels_queries_cannot_change_passage_quantization_bounds(tmp_path: Path):
    embedder = _load_embedder_module()
    import numpy as np

    base_shard = tmp_path / "base.npy"
    query_shard = tmp_path / "query.npy"
    np.save(base_shard, np.asarray([[0.0, 0.0], [1.0, 1.0]], dtype=np.float32))
    np.save(query_shard, np.asarray([[-2.0, 3.0]], dtype=np.float32))

    result = embedder.finalize_embeddings(
        [base_shard],
        output_dir=tmp_path / "out",
        prefix="stable-base",
        dimension=2,
        query_rows=0,
        query_shard_paths=[query_shard],
    )

    assert result["quantization_min"] == 0.0
    assert result["quantization_max"] == 1.0
    assert result["query_clip_low_count"] == 1
    assert result["query_clip_high_count"] == 1


def test_qrels_validation_requires_unique_known_query_ids(tmp_path: Path):
    embedder = _load_embedder_module()
    qrels = tmp_path / "qrels.jsonl"
    qrels.write_text(
        json.dumps({"query_id": "q1", "corpus_id": "doc-1", "relevance": 1}) + "\n",
        encoding="utf-8",
    )

    summary = embedder.validate_qrels(qrels, ("q1",))
    assert summary["row_count"] == 1
    assert summary["query_count"] == 1

    with pytest.raises(ValueError, match="unknown query_id"):
        embedder.validate_qrels(qrels, ("different",))
    with pytest.raises(ValueError, match="unknown corpus_id"):
        embedder.validate_qrels(qrels, ("q1",), {"different-document"})


def test_qrels_mode_routes_passages_and_queries_through_distinct_roles(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    embedder = _load_embedder_module()
    import numpy as np

    revision = "a" * 40
    contract = ResolvedInputContract(
        model_id="test/model",
        model_revision=revision,
        counter=WordCounter(),
        effective_context_limit=8,
        renderer=InputRenderer(
            document_template="passage: {text}", query_template="query: {text}"
        ),
        native_dimension=2,
        output_dimension=2,
    )
    texts = tmp_path / "texts.json"
    texts.write_text(json.dumps(["a", "b", "c", "d"]), encoding="utf-8")
    corpus_manifest = tmp_path / "corpus.manifest.json"
    chunks = tmp_path / "chunks.jsonl"
    chunks.write_text(
        json.dumps({"chunk_id": "chunk-1", "chunk_index": 0}) + "\n",
        encoding="utf-8",
    )
    corpus_manifest.write_text(
        json.dumps(
            {
                "texts_sha256": embedder._sha256(texts),
                "chunks_sha256": embedder._sha256(chunks),
                "input_contract": {"contracts": [contract.to_manifest()]},
            }
        ),
        encoding="utf-8",
    )
    queries = tmp_path / "queries.jsonl"
    queries.write_text(
        json.dumps({"id": "q1", "text": "find a"}) + "\n", encoding="utf-8"
    )
    qrels = tmp_path / "qrels.jsonl"
    qrels.write_text(
        json.dumps({"query_id": "q1", "corpus_id": "chunk-1", "relevance": 1}) + "\n",
        encoding="utf-8",
    )

    class FakeProvider:
        passage_calls: list[list[str]] = []
        query_calls: list[list[str]] = []

        @staticmethod
        def get_input_contract():
            return contract

        @staticmethod
        def get_dimension():
            return 2

        @classmethod
        def embed_passages(cls, values):
            cls.passage_calls.append(list(values))
            return np.asarray(
                [[float(index % 2), float(index // 2)] for index in range(len(values))],
                dtype=np.float32,
            )

        @classmethod
        def embed_queries(cls, values):
            cls.query_calls.append(list(values))
            return np.asarray([[0.5, 0.5] for _ in values], dtype=np.float32)

    monkeypatch.setattr(
        embedder, "create_open_model_provider", lambda *args, **kwargs: FakeProvider()
    )
    result = embedder.run(
        Namespace(
            texts=texts,
            corpus_manifest=corpus_manifest,
            model="test/model",
            revision=revision,
            dimension=2,
            output_dir=tmp_path / "output",
            prefix="tiny",
            shard_size=4,
            query_rows=1,
            queries_jsonl=queries,
            qrels=qrels,
            chunks_jsonl=chunks,
            query_id_field="id",
            query_text_field="text",
            batch_size=4,
            device=None,
        )
    )

    assert FakeProvider.passage_calls == [["a", "b", "c", "d"]]
    assert FakeProvider.query_calls == [["find a"]]
    assert result["evaluation_mode"] == "qrels"
    assert result["base_rows"] == 4
    assert result["query_rows"] == 1
    assert result["qrels"]["query_count"] == 1
    assert result["transport_sha256"] == embedder._sha256(Path(embedder.__file__))
    assert json.loads(Path(result["query_ids"]["path"]).read_text()) == ["q1"]
