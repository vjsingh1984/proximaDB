"""Offline unit tests for proximadb_sdk.document_pipeline.

Fully offline: no network, no model downloads, no real DB. The pipeline's
only dependency is document_processor (pure Python). We feed fake processors
(via monkeypatching the pipeline's registry / _get_processor) and fake
embedding providers so nothing heavy ever runs.
"""

import asyncio

import pytest

from proximadb_sdk import document_pipeline as dp
from proximadb_sdk.document_processor import (
    DocumentType,
    ProcessedChunk,
    ProcessingResult,
    VectorRecord,
)

# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeEmbeddingProvider:
    """Minimal sync embedding provider."""

    def __init__(self, dim=4):
        self._dim = dim

    @property
    def dimension(self):
        return self._dim

    def embed_texts(self, texts):
        return [[float(i)] * self._dim for i, _ in enumerate(texts)]


class FakeProcessor:
    """Fake DocumentProcessor that avoids real chunking strategies."""

    def __init__(self, name="fake", chunks=None, raise_on_process=False):
        self._name = name
        self._chunks = (
            chunks
            if chunks is not None
            else [ProcessedChunk(chunk_id="c0", text="hello", start_pos=0, end_pos=5)]
        )
        self._raise = raise_on_process

    @property
    def name(self):
        return self._name

    @property
    def supported_types(self):
        return [DocumentType.TEXT]

    def chunk(self, content, source_id, metadata=None):
        return list(self._chunks)

    def enrich_metadata(self, chunk, source_metadata=None):
        md = dict(chunk.metadata)
        md["processor"] = self._name
        return md

    async def process(self, content, source_id, embedding_adapter=None, metadata=None):
        if self._raise:
            raise RuntimeError("boom")
        chunks = self.chunk(content, source_id, metadata)
        vectors = []
        if embedding_adapter and chunks:
            embs = await embedding_adapter.embed_texts_async([c.text for c in chunks])
            for c, e in zip(chunks, embs):
                vectors.append(
                    VectorRecord(
                        id=c.chunk_id,
                        vector=e,
                        metadata=c.metadata,
                        text=c.text,
                        source_id=source_id,
                    )
                )
        return ProcessingResult(
            success=True,
            source_id=source_id,
            document_type=DocumentType.TEXT,
            chunks=chunks,
            vectors=vectors,
            metrics={"processing_time_sec": 0.0},
        )


class FakeStore:
    def __init__(self, fail=False):
        self.inserted = []
        self.fail = fail

    async def insert(self, records):
        if self.fail:
            raise RuntimeError("store down")
        self.inserted.append(records)


def make_pipeline(provider=None, store=None, config=None, fake_proc=None):
    pl = dp.DocumentPipeline(
        embedding_provider=provider, vector_store=store, config=config
    )
    if fake_proc is not None:
        pl._get_processor = lambda *a, **k: fake_proc
    return pl


# ---------------------------------------------------------------------------
# Config / dataclasses / metrics
# ---------------------------------------------------------------------------


def test_pipeline_config_post_init():
    cfg = dp.PipelineConfig()
    assert cfg.processor_config is not None
    assert cfg.mode == dp.PipelineMode.EMBED


def test_enums():
    assert dp.PipelineMode.STORE.value == "store"
    assert dp.ErrorStrategy.FAIL_FAST.value == "fail_fast"


def test_metrics_success_rate_and_to_dict():
    m = dp.PipelineMetrics()
    assert m.success_rate == 0.0
    m.total_documents = 4
    m.processed_documents = 3
    assert m.success_rate == 0.75
    d = m.to_dict()
    assert d["total_documents"] == 4
    assert d["success_rate"] == 0.75
    assert d["error_count"] == 0


def test_batch_result_counts_and_vectors():
    ok = ProcessingResult(
        success=True,
        source_id="a",
        document_type=DocumentType.TEXT,
        vectors=[
            VectorRecord(id="1", vector=[0.1], metadata={}, text="t", source_id="a")
        ],
    )
    bad = ProcessingResult(
        success=False, source_id="b", document_type=DocumentType.TEXT
    )
    br = dp.BatchResult(results=[ok, bad])
    assert br.success_count == 1
    assert br.failure_count == 1
    assert len(br.get_vectors()) == 1


# ---------------------------------------------------------------------------
# ProgressTracker
# ---------------------------------------------------------------------------


def test_progress_tracker_lifecycle():
    events = []
    pt = dp.ProgressTracker(lambda c, t, s: events.append((c, t, s)))
    assert pt.elapsed_time == 0.0
    pt.start(3, "running")
    pt.update(1, "step")
    pt.update()
    pt.complete()
    assert pt.elapsed_time >= 0.0
    # start, update, update, complete -> 4 notifications
    assert len(events) == 4
    assert events[-1] == (3, 3, "completed")


def test_progress_tracker_no_callback():
    pt = dp.ProgressTracker()
    pt.start(1)
    pt.update()
    pt.complete()  # no exception without callback


# ---------------------------------------------------------------------------
# Constructor branches for embedding adapter
# ---------------------------------------------------------------------------


def test_init_with_provider():
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    assert pl.embedding_adapter is not None


def test_init_with_placeholder():
    cfg = dp.PipelineConfig(use_placeholder_embeddings=True, placeholder_dimension=8)
    pl = dp.DocumentPipeline(config=cfg)
    assert pl.embedding_adapter is not None
    assert pl.embedding_adapter.dimension == 8


def test_init_without_provider_or_placeholder():
    pl = dp.DocumentPipeline()
    assert pl.embedding_adapter is None


# ---------------------------------------------------------------------------
# Fluent config API
# ---------------------------------------------------------------------------


def test_with_embedding_provider():
    pl = dp.DocumentPipeline()
    ret = pl.with_embedding_provider(FakeEmbeddingProvider())
    assert ret is pl
    assert pl.embedding_adapter is not None


def test_with_vector_store():
    pl = dp.DocumentPipeline()
    store = FakeStore()
    ret = pl.with_vector_store(store)
    assert ret is pl
    assert pl.vector_store is store


def test_with_progress_callback():
    pl = dp.DocumentPipeline()
    cb = lambda c, t, s: None
    ret = pl.with_progress_callback(cb)
    assert ret is pl
    assert pl.config.progress_callback is cb


# ---------------------------------------------------------------------------
# process()
# ---------------------------------------------------------------------------


def test_process_success_with_embeddings():
    proc = FakeProcessor()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)
    res = asyncio.run(pl.process("hello world", "doc.txt"))
    assert res.success
    assert res.vector_count == 1
    m = pl.get_metrics()
    assert m["total_documents"] == 1
    assert m["processed_documents"] == 1
    assert m["total_vectors"] == 1


def test_process_only_mode_no_embeddings():
    proc = FakeProcessor()
    cfg = dp.PipelineConfig(mode=dp.PipelineMode.PROCESS_ONLY)
    pl = make_pipeline(provider=FakeEmbeddingProvider(), config=cfg, fake_proc=proc)
    res = asyncio.run(pl.process("hello", "doc.txt"))
    assert res.success
    assert res.vector_count == 0


def test_process_failure_result_counts_as_failed():
    proc = FakeProcessor()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)

    async def fake_process(*a, **k):
        return ProcessingResult(
            success=False, source_id="doc.txt", document_type=DocumentType.TEXT
        )

    proc.process = fake_process
    res = asyncio.run(pl.process("x", "doc.txt"))
    assert not res.success
    assert pl.get_metrics()["failed_documents"] == 1


def test_process_exception_path():
    proc = FakeProcessor(raise_on_process=True)
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)
    res = asyncio.run(pl.process("x", "bad.txt", document_type=DocumentType.CODE))
    assert not res.success
    assert res.document_type == DocumentType.CODE
    assert res.errors
    assert pl.get_metrics()["failed_documents"] == 1
    assert pl.get_metrics()["error_count"] == 1


# ---------------------------------------------------------------------------
# process_and_store()
# ---------------------------------------------------------------------------


def test_process_and_store_no_store_raises():
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    with pytest.raises(ValueError):
        asyncio.run(pl.process_and_store("x", "doc.txt"))


def test_process_and_store_success():
    proc = FakeProcessor()
    store = FakeStore()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), store=store, fake_proc=proc)
    res = asyncio.run(pl.process_and_store("hello", "doc.txt"))
    assert res.success
    assert res.metrics["records_stored"] == 1
    assert store.inserted
    assert "storage_time_sec" in res.metrics


def test_process_and_store_no_vectors_returns_early():
    # PROCESS_ONLY -> no vectors -> returns result before storing
    proc = FakeProcessor()
    store = FakeStore()
    cfg = dp.PipelineConfig(mode=dp.PipelineMode.PROCESS_ONLY)
    pl = make_pipeline(
        provider=FakeEmbeddingProvider(), store=store, config=cfg, fake_proc=proc
    )
    res = asyncio.run(pl.process_and_store("hello", "doc.txt"))
    assert res.success
    assert not store.inserted


def test_process_and_store_storage_failure():
    proc = FakeProcessor()
    store = FakeStore(fail=True)
    pl = make_pipeline(provider=FakeEmbeddingProvider(), store=store, fake_proc=proc)
    res = asyncio.run(pl.process_and_store("hello", "doc.txt"))
    assert not res.success
    assert any(e.get("stage") == "storage" for e in res.errors)


# ---------------------------------------------------------------------------
# process_batch()
# ---------------------------------------------------------------------------


def test_process_batch_success():
    proc = FakeProcessor()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)
    docs = [
        {"content": "a", "source_id": "a.txt"},
        {"content": "b", "source_id": "b.txt", "metadata": {"k": 1}},
    ]
    br = asyncio.run(pl.process_batch(docs))
    assert len(br.results) == 2
    assert br.success_count == 2


def test_process_batch_handles_exception_result():
    proc = FakeProcessor()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)

    async def boom(*a, **k):
        raise RuntimeError("gather-fail")

    # Patch pipeline.process so gather captures the exception
    pl.process = boom
    docs = [{"content": "a", "source_id": "a.txt"}]
    br = asyncio.run(pl.process_batch(docs, concurrent_limit=2))
    assert len(br.results) == 1
    assert not br.results[0].success
    assert br.results[0].source_id == "a.txt"


# ---------------------------------------------------------------------------
# process_batch_and_store()
# ---------------------------------------------------------------------------


def test_process_batch_and_store_no_store_raises():
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    with pytest.raises(ValueError):
        asyncio.run(pl.process_batch_and_store([{"content": "a", "source_id": "a"}]))


def test_process_batch_and_store_success():
    proc = FakeProcessor()
    store = FakeStore()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), store=store, fake_proc=proc)
    docs = [
        {"content": "a", "source_id": "a.txt"},
        {"content": "b", "source_id": "b.txt"},
    ]
    br = asyncio.run(pl.process_batch_and_store(docs))
    assert br.success_count == 2
    assert store.inserted
    assert len(store.inserted[0]) == 2  # two vectors stored


def test_process_batch_and_store_storage_failure():
    proc = FakeProcessor()
    store = FakeStore(fail=True)
    pl = make_pipeline(provider=FakeEmbeddingProvider(), store=store, fake_proc=proc)
    docs = [{"content": "a", "source_id": "a.txt"}]
    br = asyncio.run(pl.process_batch_and_store(docs))
    # batch succeeded; storage failed and recorded in metrics
    assert br.success_count == 1
    assert any(e.get("stage") == "batch_storage" for e in pl._metrics.errors)


def test_process_batch_and_store_no_vectors():
    proc = FakeProcessor()
    store = FakeStore()
    cfg = dp.PipelineConfig(mode=dp.PipelineMode.PROCESS_ONLY)
    pl = make_pipeline(
        provider=FakeEmbeddingProvider(), store=store, config=cfg, fake_proc=proc
    )
    docs = [{"content": "a", "source_id": "a.txt"}]
    br = asyncio.run(pl.process_batch_and_store(docs))
    assert br.success_count == 1
    assert not store.inserted


# ---------------------------------------------------------------------------
# process_file()
# ---------------------------------------------------------------------------


def test_process_file_not_found(tmp_path):
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())
    missing = tmp_path / "nope.txt"
    res = asyncio.run(pl.process_file(missing))
    assert not res.success
    assert "File not found" in res.errors[0]["error"]


def test_process_file_success(tmp_path):
    f = tmp_path / "doc.txt"
    f.write_text("hello content")
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())
    res = asyncio.run(pl.process_file(f))
    assert res.success


def test_process_file_read_error(tmp_path, monkeypatch):
    f = tmp_path / "doc.txt"
    f.write_text("data")
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())

    from pathlib import Path as _P

    def bad_read(self, encoding="utf-8"):
        raise OSError("read fail")

    monkeypatch.setattr(_P, "read_text", bad_read)
    res = asyncio.run(pl.process_file(f))
    assert not res.success
    assert "Failed to read file" in res.errors[0]["error"]


# ---------------------------------------------------------------------------
# process_directory()
# ---------------------------------------------------------------------------


def test_process_directory_not_a_dir(tmp_path):
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())
    f = tmp_path / "file.txt"
    f.write_text("x")
    br = asyncio.run(pl.process_directory(f))
    assert any("Not a directory" in e["error"] for e in br.metrics.errors)


def test_process_directory_success(tmp_path):
    (tmp_path / "a.py").write_text("print(1)")
    (tmp_path / "b.txt").write_text("text")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "c.py").write_text("print(2)")
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())
    br = asyncio.run(pl.process_directory(tmp_path))
    assert len(br.results) == 3
    assert br.success_count == 3


def test_process_directory_extension_filter(tmp_path):
    (tmp_path / "a.py").write_text("print(1)")
    (tmp_path / "b.txt").write_text("text")
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())
    br = asyncio.run(pl.process_directory(tmp_path, extensions=[".py"]))
    assert len(br.results) == 1


def test_process_directory_non_recursive(tmp_path):
    (tmp_path / "a.py").write_text("print(1)")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "c.py").write_text("print(2)")
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=FakeProcessor())
    br = asyncio.run(pl.process_directory(tmp_path, recursive=False))
    # non-recursive: only top-level a.py (sub dir filtered out as non-file)
    assert len(br.results) == 1


# ---------------------------------------------------------------------------
# process_stream()
# ---------------------------------------------------------------------------


def test_process_stream_yields_chunks():
    chunks = [
        ProcessedChunk(chunk_id="c0", text="one", start_pos=0, end_pos=3),
        ProcessedChunk(chunk_id="c1", text="two", start_pos=3, end_pos=6),
    ]
    proc = FakeProcessor(chunks=chunks)
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)

    async def collect():
        out = []
        async for c in pl.process_stream("content", "doc.txt", {"m": 1}):
            out.append(c)
        return out

    out = asyncio.run(collect())
    assert len(out) == 2
    assert all("processor" in c.metadata for c in out)


# ---------------------------------------------------------------------------
# _get_processor() branches (real registry, no chunking invoked)
# ---------------------------------------------------------------------------


def test_get_processor_by_name():
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    proc = pl._get_processor("x", "s", processor_name="text")
    assert proc.name == "text"


def test_get_processor_by_type():
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    proc = pl._get_processor("x", "s", document_type=DocumentType.CODE)
    assert proc is not None


def test_get_processor_auto_detect():
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    proc = pl._get_processor("def foo(): pass", "s.py")
    assert proc is not None


def test_get_processor_default_no_autodetect():
    cfg = dp.PipelineConfig(auto_detect_type=False, default_processor="text")
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider(), config=cfg)
    proc = pl._get_processor("plain", "s")
    assert proc.name == "text"


def test_get_processor_unknown_name_falls_through():
    # Unknown processor_name -> get returns None -> falls to auto-detect
    pl = dp.DocumentPipeline(embedding_provider=FakeEmbeddingProvider())
    proc = pl._get_processor("plain text", "s", processor_name="nonexistent")
    assert proc is not None


# ---------------------------------------------------------------------------
# Metrics reset
# ---------------------------------------------------------------------------


def test_reset_metrics():
    proc = FakeProcessor()
    pl = make_pipeline(provider=FakeEmbeddingProvider(), fake_proc=proc)
    asyncio.run(pl.process("hi", "doc.txt"))
    assert pl.get_metrics()["total_documents"] == 1
    pl.reset_metrics()
    assert pl.get_metrics()["total_documents"] == 0


# ---------------------------------------------------------------------------
# Factory functions + context managers
# ---------------------------------------------------------------------------


def test_create_document_pipeline():
    pl = dp.create_document_pipeline(
        embedding_provider=FakeEmbeddingProvider(),
        mode=dp.PipelineMode.STORE,
        max_concurrent=8,
    )
    assert isinstance(pl, dp.DocumentPipeline)
    assert pl.config.mode == dp.PipelineMode.STORE
    assert pl.config.max_concurrent == 8


def test_create_code_pipeline():
    pl = dp.create_code_pipeline(embedding_provider=FakeEmbeddingProvider())
    assert pl.config.default_processor == "code"
    assert pl.config.embedding_batch_size == 16
    assert pl.config.processor_config.chunk_size == 1024


def test_pipeline_context_sync():
    with dp.pipeline_context(embedding_provider=FakeEmbeddingProvider()) as pl:
        assert isinstance(pl, dp.DocumentPipeline)
        pl._metrics.total_documents = 5
    # reset on exit
    assert pl.get_metrics()["total_documents"] == 0


def test_async_pipeline_context():
    async def run():
        async with dp.async_pipeline_context(
            embedding_provider=FakeEmbeddingProvider()
        ) as pl:
            assert isinstance(pl, dp.DocumentPipeline)
            pl._metrics.total_documents = 3
        return pl

    pl = asyncio.run(run())
    assert pl.get_metrics()["total_documents"] == 0
