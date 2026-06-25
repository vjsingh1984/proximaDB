"""Offline unit tests for proximadb_sdk.chunking_strategies.pipeline.

Fully offline: no network, no real embedding model. The "embedding provider"
and "vector store" are hand fakes / MagicMocks. The chunking strategies are
pure CPU text processing.
"""

import asyncio

import pytest

from proximadb_sdk.chunking_strategies.base import (
    ChunkingConfig,
    ChunkingStrategy,
    TextChunk,
)
from proximadb_sdk.chunking_strategies.code import CodeChunkingConfig
from proximadb_sdk.chunking_strategies.pipeline import (
    BatchEmbedder,
    BatchResult,
    ChunkingPipeline,
    EnrichmentStage,
    ErrorHandling,
    FilterStage,
    PipelineConfig,
    PipelineResult,
    ProcessingMode,
    ProgressTracker,
    ValidationStage,
    async_pipeline_context,
    create_code_pipeline,
    create_document_pipeline,
    create_pipeline,
    pipeline_context,
)

# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeProvider:
    """Deterministic synchronous + async embedding provider."""

    def __init__(self, dim: int = 4):
        self._dim = dim
        self.calls = 0

    def embed_texts(self, texts):
        self.calls += 1
        return [[float(len(t))] * self._dim for t in texts]

    async def embed_texts_async(self, texts):
        self.calls += 1
        return [[float(len(t))] * self._dim for t in texts]

    @property
    def dimension(self):
        return self._dim


class SyncOnlyProvider:
    """Provider without embed_texts_async to exercise the executor fallback."""

    def __init__(self, dim: int = 3):
        self._dim = dim

    def embed_texts(self, texts):
        return [[1.0] * self._dim for _ in texts]

    @property
    def dimension(self):
        return self._dim


class FlakyProvider:
    """Fails the first N calls, then succeeds."""

    def __init__(self, fail_times: int):
        self.fail_times = fail_times
        self.calls = 0

    def embed_texts(self, texts):
        self.calls += 1
        if self.calls <= self.fail_times:
            raise RuntimeError("transient")
        return [[0.5, 0.5] for _ in texts]

    async def embed_texts_async(self, texts):
        self.calls += 1
        if self.calls <= self.fail_times:
            raise RuntimeError("transient")
        return [[0.5, 0.5] for _ in texts]


class AlwaysFailProvider:
    def embed_texts(self, texts):
        raise RuntimeError("boom")

    async def embed_texts_async(self, texts):
        raise RuntimeError("boom")


class FakeVectorStore:
    def __init__(self, fail: bool = False):
        self.records = None
        self.fail = fail

    async def insert(self, records):
        if self.fail:
            raise RuntimeError("store down")
        self.records = records

    async def search(self, query_vector, top_k=10, filter=None):
        return []


LONG_TEXT = (" ".join(f"word{i}" for i in range(400))) + ". End."
PARA_TEXT = "Sentence one. Sentence two.\n\nSecond paragraph here. More text."


def fast_config(**kw):
    """Pipeline config with retry_delay=0 so retries never sleep meaningfully."""
    kw.setdefault("retry_delay", 0.0)
    kw.setdefault("max_retries", 2)
    return PipelineConfig(**kw)


# ---------------------------------------------------------------------------
# Dataclass / result models
# ---------------------------------------------------------------------------


def test_pipeline_config_post_init_creates_chunking_config():
    cfg = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
    assert cfg.chunking_config is not None
    assert cfg.chunking_config.strategy == ChunkingStrategy.SENTENCE


def test_pipeline_config_keeps_supplied_chunking_config():
    cc = ChunkingConfig(strategy=ChunkingStrategy.PARAGRAPH, chunk_size=256)
    cfg = PipelineConfig(chunking_config=cc)
    assert cfg.chunking_config is cc


def test_pipeline_result_properties_and_to_dict():
    chunks = [TextChunk("a", 0, 1, "c0"), TextChunk("b", 1, 2, "c1")]
    errors = [{"error": str(i)} for i in range(15)]
    r = PipelineResult(success=False, chunks=chunks, errors=errors, metrics={"x": 1})
    assert r.chunk_count == 2
    assert r.error_count == 15
    d = r.to_dict()
    assert d["chunk_count"] == 2
    assert d["error_count"] == 15
    assert len(d["errors"]) == 10  # capped at 10
    assert d["metrics"] == {"x": 1}


def test_pipeline_result_to_dict_no_errors():
    r = PipelineResult(success=True, chunks=[TextChunk("a", 0, 1, "c0")])
    d = r.to_dict()
    assert d["success"] is True
    assert d["errors"] == []


def test_batch_result_success_rate():
    assert BatchResult().success_rate == 0.0
    br = BatchResult(total_items=4, processed_items=3)
    assert br.success_rate == 0.75


def test_enums_have_expected_members():
    assert ProcessingMode.SYNC.value == "sync"
    assert ProcessingMode.STREAMING.value == "streaming"
    assert ErrorHandling.FAIL_FAST.value == "fail_fast"
    assert ErrorHandling.RETRY.value == "retry"


# ---------------------------------------------------------------------------
# Stages
# ---------------------------------------------------------------------------


def test_validation_stage_disabled_passthrough():
    cfg = PipelineConfig(validate_chunks=False)
    stage = ValidationStage(cfg)
    assert stage.name == "validation"
    ch = TextChunk("", 0, 0, "c0")
    assert stage.process(ch) is ch  # not validated


def test_validation_stage_empty_chunk_raises():
    stage = ValidationStage(PipelineConfig())
    with pytest.raises(ValueError):
        stage.process(TextChunk("   ", 0, 3, "c0"))


def test_validation_stage_truncates_long_text():
    cfg = PipelineConfig(max_text_length=5, truncate_long_texts=True)
    stage = ValidationStage(cfg)
    ch = TextChunk("abcdefghij", 0, 10, "c0")
    out = stage.process(ch)
    assert out.text == "abcde"
    assert out.metadata["truncated"] is True


def test_validation_stage_long_text_raises_when_no_truncate():
    cfg = PipelineConfig(max_text_length=5, truncate_long_texts=False)
    stage = ValidationStage(cfg)
    with pytest.raises(ValueError):
        stage.process(TextChunk("abcdefghij", 0, 10, "c0"))


def test_enrichment_stage_runs_funcs_and_add():
    stage = EnrichmentStage()
    assert stage.name == "enrichment"

    def tag(c):
        c.metadata["tagged"] = True
        return c

    stage.add_enrichment(tag)
    out = stage.process(TextChunk("x", 0, 1, "c0"))
    assert out.metadata["tagged"] is True


def test_enrichment_stage_with_initial_funcs():
    def upper(c):
        c.text = c.text.upper()
        return c

    stage = EnrichmentStage([upper])
    out = stage.process(TextChunk("ab", 0, 2, "c0"))
    assert out.text == "AB"


def test_filter_stage_no_predicates_passthrough():
    stage = FilterStage()
    assert stage.name == "filter"
    chunks = [TextChunk("a", 0, 1, "c0")]
    assert stage.process(chunks) is chunks


def test_filter_stage_applies_predicates():
    stage = FilterStage()
    stage.add_predicate(lambda c: len(c.text) > 1)
    chunks = [TextChunk("a", 0, 1, "c0"), TextChunk("abc", 0, 3, "c1")]
    out = stage.process(chunks)
    assert [c.chunk_id for c in out] == ["c1"]


# ---------------------------------------------------------------------------
# BatchEmbedder
# ---------------------------------------------------------------------------


def test_batch_embedder_batches_and_stats():
    prov = FakeProvider()
    emb = BatchEmbedder(prov, batch_size=2, max_retries=1, retry_delay=0.0)
    out = emb.embed_batch(["aa", "bbb", "c", "dddd", "e"])
    assert len(out) == 5
    stats = emb.stats
    assert stats["batch_size"] == 2
    assert stats["request_count"] == 3  # ceil(5/2)


def test_batch_embedder_retry_then_success():
    prov = FlakyProvider(fail_times=1)
    emb = BatchEmbedder(prov, batch_size=10, max_retries=3, retry_delay=0.0)
    out = emb.embed_batch(["a", "b"])
    assert len(out) == 2
    assert prov.calls == 2  # one failure + one success


def test_batch_embedder_exhausts_retries_raises():
    emb = BatchEmbedder(
        AlwaysFailProvider(), batch_size=10, max_retries=2, retry_delay=0.0
    )
    with pytest.raises(RuntimeError, match="Embedding failed after"):
        emb.embed_batch(["a"])


def test_batch_embedder_async_with_async_provider():
    prov = FakeProvider()
    emb = BatchEmbedder(prov, batch_size=2, max_retries=1, retry_delay=0.0)
    out = asyncio.run(emb.embed_batch_async(["aa", "bb", "cc"]))
    assert len(out) == 3


def test_batch_embedder_async_fallback_to_executor():
    prov = SyncOnlyProvider()
    emb = BatchEmbedder(prov, batch_size=5, max_retries=1, retry_delay=0.0)
    out = asyncio.run(emb.embed_batch_async(["a", "b"]))
    assert len(out) == 2
    assert all(len(v) == 3 for v in out)


def test_batch_embedder_async_retry_then_success():
    prov = FlakyProvider(fail_times=1)
    emb = BatchEmbedder(prov, batch_size=10, max_retries=3, retry_delay=0.0)
    out = asyncio.run(emb.embed_batch_async(["a"]))
    assert len(out) == 1


def test_batch_embedder_async_exhausts_retries():
    emb = BatchEmbedder(
        AlwaysFailProvider(), batch_size=10, max_retries=2, retry_delay=0.0
    )
    with pytest.raises(RuntimeError, match="Async embedding failed after"):
        asyncio.run(emb.embed_batch_async(["a"]))


# ---------------------------------------------------------------------------
# ProgressTracker
# ---------------------------------------------------------------------------


def test_progress_tracker_lifecycle_and_callback():
    events = []
    pt = ProgressTracker(lambda cur, tot, st: events.append((cur, tot, st)))
    pt.start(3, "go")
    pt.update(1)
    pt.update(2, status="midway")
    pt.complete()
    assert events[0] == (0, 3, "go")
    assert events[-1] == (3, 3, "completed")
    assert pt.elapsed_time >= 0.0
    assert pt.items_per_second >= 0.0


def test_progress_tracker_defaults_no_callback():
    pt = ProgressTracker()
    # No start -> elapsed 0, items/s 0
    assert pt.elapsed_time == 0.0
    assert pt.items_per_second == 0.0
    pt.start(0, "x")  # total 0 -> items_per_second uses current 0
    assert pt.items_per_second == 0.0


# ---------------------------------------------------------------------------
# Pipeline init and builder API
# ---------------------------------------------------------------------------


def test_pipeline_default_init_no_embedder():
    p = ChunkingPipeline()
    assert p.embedder is None
    assert p.chunker is not None


def test_pipeline_code_strategy_init_with_code_config():
    cfg = PipelineConfig(
        chunking_strategy=ChunkingStrategy.CODE,
        chunking_config=CodeChunkingConfig(chunk_size=128),
    )
    p = ChunkingPipeline(cfg)
    assert p.chunker is not None


def test_pipeline_code_strategy_init_converts_plain_config():
    cfg = PipelineConfig(
        chunking_strategy=ChunkingStrategy.CODE,
        chunking_config=ChunkingConfig(chunk_size=200, chunk_overlap=20),
    )
    p = ChunkingPipeline(cfg)
    assert p.chunker is not None


def test_pipeline_builder_chain():
    prov = FakeProvider()
    store = FakeVectorStore()
    cb_calls = []
    p = (
        ChunkingPipeline(fast_config())
        .with_strategy(ChunkingStrategy.SENTENCE)
        .with_embedding_provider(prov)
        .with_vector_store(store)
        .with_enrichment(lambda c: c)
        .with_filter(lambda c: True)
        .with_progress_callback(lambda *a: cb_calls.append(a))
    )
    assert p.embedder is not None
    assert p.vector_store is store
    assert p.config.chunking_strategy == ChunkingStrategy.SENTENCE


# ---------------------------------------------------------------------------
# process_text (sync)
# ---------------------------------------------------------------------------


def test_process_text_with_embeddings():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        embedding_provider=FakeProvider(),
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )
    r = p.process_text(LONG_TEXT, "doc1")
    assert r.success is True
    assert r.chunk_count >= 1
    assert len(r.embeddings) == r.chunk_count
    assert "chunks_per_second" in r.metrics


def test_process_text_no_embedder():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    r = p.process_text(PARA_TEXT, "doc1")
    assert r.success is True
    assert r.embeddings == []


def test_process_text_validation_error_collected():
    # Force validation failure: max length 1, no truncation, collect errors.
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.COLLECT_ERRORS,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )
    r = p.process_text(LONG_TEXT, "doc1")
    assert r.success is False
    assert any(e["stage"] == "validation" for e in r.errors)


def test_process_text_fail_fast_on_validation():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )
    r = p.process_text(LONG_TEXT, "doc1")
    # Outer except catches the re-raised error -> pipeline stage error.
    assert r.success is False
    assert any(e["stage"] == "pipeline" for e in r.errors)


def test_process_text_embedding_error_collected():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=AlwaysFailProvider(),
        error_handling=ErrorHandling.COLLECT_ERRORS,
        retry_delay=0.0,
        max_retries=1,
    )
    r = p.process_text("A short sentence here.", "doc1")
    assert r.success is False
    assert any(e["stage"] == "embedding" for e in r.errors)


def test_process_text_embedding_error_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=AlwaysFailProvider(),
        error_handling=ErrorHandling.FAIL_FAST,
        retry_delay=0.0,
        max_retries=1,
    )
    r = p.process_text("A short sentence here.", "doc1")
    assert r.success is False
    assert any(e["stage"] == "pipeline" for e in r.errors)


def test_process_text_chunker_exception(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    def boom(*a, **k):
        raise RuntimeError("chunk failure")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    r = p.process_text("text", "doc1")
    assert r.success is False
    assert r.errors[0]["stage"] == "pipeline"


def test_process_text_metrics_disabled():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, enable_metrics=False)
    r = p.process_text("Hello there.", "doc1")
    assert r.success is True


# ---------------------------------------------------------------------------
# process_text_async
# ---------------------------------------------------------------------------


def test_process_text_async_basic():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, embedding_provider=FakeProvider()
    )
    r = asyncio.run(p.process_text_async("One. Two. Three.", "doc1"))
    assert r.success is True
    assert r.metrics["mode"] == "async"


def test_process_text_async_embedding_error_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=AlwaysFailProvider(),
        error_handling=ErrorHandling.FAIL_FAST,
        retry_delay=0.0,
        max_retries=1,
    )
    r = asyncio.run(p.process_text_async("A sentence.", "doc1"))
    assert r.success is False


def test_process_text_async_chunker_exception(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    def boom(*a, **k):
        raise RuntimeError("nope")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    r = asyncio.run(p.process_text_async("text", "doc1"))
    assert r.success is False
    assert r.errors[0]["stage"] == "pipeline"


def test_process_text_async_validation_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )
    r = asyncio.run(p.process_text_async(LONG_TEXT, "doc1"))
    assert r.success is False


# ---------------------------------------------------------------------------
# Streaming
# ---------------------------------------------------------------------------


def test_process_stream_yields_chunks():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    chunks = list(p.process_stream("One. Two. Three.", "doc1"))
    assert all(isinstance(c, TextChunk) for c in chunks)


def test_process_stream_skips_invalid_collect():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.COLLECT_ERRORS,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )
    chunks = list(p.process_stream(LONG_TEXT, "doc1"))
    assert chunks == []  # all skipped


def test_process_stream_fail_fast_raises():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )
    with pytest.raises(ValueError):
        list(p.process_stream(LONG_TEXT, "doc1"))


def test_process_stream_chunker_exception_fail_fast(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.FAIL_FAST
    )

    def boom(*a, **k):
        raise RuntimeError("nope")

    # process_stream routes through chunk_stream (genuine incremental path for
    # streamable strategies like SENTENCE), so patch that.
    monkeypatch.setattr(p.chunker, "chunk_stream", boom)
    with pytest.raises(RuntimeError):
        list(p.process_stream("text", "doc1"))


def test_process_stream_chunker_exception_collect(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.COLLECT_ERRORS
    )

    def boom(*a, **k):
        raise RuntimeError("nope")

    monkeypatch.setattr(p.chunker, "chunk_stream", boom)
    assert list(p.process_stream("text", "doc1")) == []


def test_process_stream_async_yields():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    async def collect():
        return [c async for c in p.process_stream_async("One. Two.", "doc1")]

    chunks = asyncio.run(collect())
    assert all(isinstance(c, TextChunk) for c in chunks)


def test_process_stream_async_skip_collect():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.COLLECT_ERRORS,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )

    async def collect():
        return [c async for c in p.process_stream_async(LONG_TEXT, "doc1")]

    assert asyncio.run(collect()) == []


def test_process_stream_async_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.FIXED_SIZE,
        max_text_length=1,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
        chunking_config=ChunkingConfig(
            chunk_size=50, chunk_overlap=0, min_chunk_size=10
        ),
    )

    async def collect():
        return [c async for c in p.process_stream_async(LONG_TEXT, "doc1")]

    with pytest.raises(ValueError):
        asyncio.run(collect())


def test_process_stream_async_chunker_exception_fail_fast(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.FAIL_FAST
    )

    def boom(*a, **k):
        raise RuntimeError("nope")

    monkeypatch.setattr(p.chunker, "chunk_stream", boom)

    async def collect():
        return [c async for c in p.process_stream_async("text", "doc1")]

    with pytest.raises(RuntimeError):
        asyncio.run(collect())


# ---------------------------------------------------------------------------
# Batch processing
# ---------------------------------------------------------------------------


def test_process_batch_mixed_success():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    items = [
        {"text": "One. Two.", "source_id": "a"},
        {"text": "Three. Four.", "source_id": "b"},
        {},  # uses defaults; empty text -> 0 chunks, still success
    ]
    br = p.process_batch(items)
    assert br.total_items == 3
    assert br.processed_items >= 2
    assert isinstance(br.results, list)


def test_process_batch_item_exception_collect(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    calls = {"n": 0}
    orig = p.process_text

    def flaky(text, source_id, metadata=None):
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("first fails")
        return orig(text, source_id, metadata)

    monkeypatch.setattr(p, "process_text", flaky)
    br = p.process_batch([{"text": "a. b."}, {"text": "c. d."}])
    assert any("error" in e for e in br.errors)


def test_process_batch_item_exception_fail_fast(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.FAIL_FAST
    )

    def boom(*a, **k):
        raise RuntimeError("stop")

    monkeypatch.setattr(p, "process_text", boom)
    br = p.process_batch([{"text": "a."}, {"text": "b."}])
    # Breaks after first failure.
    assert br.processed_items == 0


def test_process_batch_async():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    items = [
        {"text": "One. Two.", "source_id": "a"},
        {"text": "Three.", "source_id": "b"},
    ]
    br = asyncio.run(p.process_batch_async(items, concurrent_limit=2))
    assert br.total_items == 2
    assert br.processed_items == 2


def test_process_batch_async_collects_task_exception(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    async def boom(*a, **k):
        raise RuntimeError("async stop")

    monkeypatch.setattr(p, "process_text_async", boom)
    br = asyncio.run(p.process_batch_async([{"text": "a."}]))
    assert br.failed_items == 1
    assert any("error" in e for e in br.errors)


# ---------------------------------------------------------------------------
# File / directory processing
# ---------------------------------------------------------------------------


def test_process_file(tmp_path):
    f = tmp_path / "doc.txt"
    f.write_text("Hello world. Second sentence.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    r = p.process_file(f)
    assert r.success is True


def test_process_file_missing():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    r = p.process_file("/nonexistent/path/xyz.txt")
    assert r.success is False
    assert "File not found" in r.errors[0]["error"]


def test_process_file_read_error(tmp_path, monkeypatch):
    f = tmp_path / "doc.txt"
    f.write_text("data")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    import pathlib

    def bad_read(self, *a, **k):
        raise OSError("denied")

    monkeypatch.setattr(pathlib.Path, "read_text", bad_read)
    r = p.process_file(f)
    assert r.success is False
    assert "Failed to read file" in r.errors[0]["error"]


def test_process_file_async(tmp_path):
    f = tmp_path / "doc.txt"
    f.write_text("Async file. Content here.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    r = asyncio.run(p.process_file_async(f))
    assert r.success is True


def test_process_file_async_missing():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    r = asyncio.run(p.process_file_async("/nope/missing.txt"))
    assert r.success is False


def test_process_file_async_read_error(tmp_path, monkeypatch):
    f = tmp_path / "doc.txt"
    f.write_text("data")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    import pathlib

    def bad_read(self, *a, **k):
        raise OSError("denied")

    monkeypatch.setattr(pathlib.Path, "read_text", bad_read)
    r = asyncio.run(p.process_file_async(f))
    assert r.success is False
    assert "Failed to read file" in r.errors[0]["error"]


def test_process_directory(tmp_path):
    (tmp_path / "a.txt").write_text("First. Doc.")
    (tmp_path / "b.txt").write_text("Second. Doc.")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "c.txt").write_text("Nested. Doc.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    br = p.process_directory(tmp_path, pattern="**/*.txt", recursive=True)
    assert br.total_items == 3
    assert br.processed_items == 3


def test_process_directory_non_recursive(tmp_path):
    (tmp_path / "a.txt").write_text("First. Doc.")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "c.txt").write_text("Nested.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    br = p.process_directory(tmp_path, pattern="**/*.txt", recursive=False)
    assert br.total_items == 1


def test_process_directory_not_a_dir(tmp_path):
    f = tmp_path / "notdir.txt"
    f.write_text("x")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    br = p.process_directory(f)
    assert br.total_items == 0
    assert any("Not a directory" in e["error"] for e in br.errors)


def test_process_directory_async(tmp_path):
    (tmp_path / "a.txt").write_text("First. Doc.")
    (tmp_path / "b.txt").write_text("Second. Doc.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    br = asyncio.run(p.process_directory_async(tmp_path, pattern="**/*.txt"))
    assert br.total_items == 2
    assert br.processed_items == 2


def test_process_directory_async_not_a_dir(tmp_path):
    f = tmp_path / "notdir.txt"
    f.write_text("x")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    br = asyncio.run(p.process_directory_async(f))
    assert any("Not a directory" in e["error"] for e in br.errors)


def test_process_directory_async_task_exception(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("First. Doc.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    async def boom(*a, **k):
        raise RuntimeError("file fail")

    monkeypatch.setattr(p, "process_file_async", boom)
    br = asyncio.run(p.process_directory_async(tmp_path, pattern="**/*.txt"))
    assert br.failed_items == 1
    assert any("error" in e for e in br.errors)


def test_process_directory_async_non_recursive(tmp_path):
    (tmp_path / "a.txt").write_text("First.")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "c.txt").write_text("Nested.")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    br = asyncio.run(
        p.process_directory_async(tmp_path, pattern="**/*.txt", recursive=False)
    )
    assert br.total_items == 1


# ---------------------------------------------------------------------------
# Vector store integration
# ---------------------------------------------------------------------------


def test_process_and_store_success():
    store = FakeVectorStore()
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeProvider(),
        vector_store=store,
    )
    r = asyncio.run(p.process_and_store("One. Two. Three.", "doc1"))
    assert r.success is True
    assert store.records is not None
    assert r.metrics["records_stored"] == r.chunk_count


def test_process_and_store_requires_vector_store():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, embedding_provider=FakeProvider()
    )
    with pytest.raises(ValueError, match="No vector store"):
        asyncio.run(p.process_and_store("text", "doc1"))


def test_process_and_store_requires_embedding_provider():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, vector_store=FakeVectorStore()
    )
    with pytest.raises(ValueError, match="No embedding provider"):
        asyncio.run(p.process_and_store("text", "doc1"))


def test_process_and_store_empty_result_returns_early():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeProvider(),
        vector_store=FakeVectorStore(),
    )
    # Empty text -> no chunks -> early return without storing.
    r = asyncio.run(p.process_and_store("", "doc1"))
    assert r.chunk_count == 0


def test_process_and_store_insert_failure():
    store = FakeVectorStore(fail=True)
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeProvider(),
        vector_store=store,
    )
    r = asyncio.run(p.process_and_store("One. Two.", "doc1"))
    assert r.success is False
    assert any(e["stage"] == "storage" for e in r.errors)


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------


def test_get_and_reset_metrics():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, embedding_provider=FakeProvider()
    )
    p.process_text("Hello. World.", "doc1")
    m = p.get_metrics()
    assert "embedder_stats" in m
    assert "progress" in m
    p.reset_metrics()


def test_get_metrics_no_embedder():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    m = p.get_metrics()
    assert m["embedder_stats"] == {}


def test_record_metrics_disabled_noop():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, enable_metrics=False)
    # Should not raise even when metrics disabled.
    p._record_metrics({"processing_time_sec": 0.1, "chunk_count": 2})


# ---------------------------------------------------------------------------
# Factory + context managers
# ---------------------------------------------------------------------------


def test_create_code_pipeline():
    p = create_code_pipeline()
    assert p.config.chunking_strategy == ChunkingStrategy.CODE
    assert p.config.embedding_batch_size == 16
    assert p.config.max_text_length == 16384


def test_create_document_pipeline():
    p = create_document_pipeline()
    assert p.config.chunking_strategy == ChunkingStrategy.SEMANTIC
    assert p.config.embedding_batch_size == 32


def test_pipeline_context_manager():
    with pipeline_context(strategy=ChunkingStrategy.SENTENCE) as p:
        r = p.process_text("Hi there. Bye now.", "doc1")
        assert r.success is True


def test_async_pipeline_context_manager():
    async def run():
        async with async_pipeline_context(strategy=ChunkingStrategy.SENTENCE) as p:
            return await p.process_text_async("Hi there.", "doc1")

    r = asyncio.run(run())
    assert r.success is True


# ---------------------------------------------------------------------------
# Strategy coverage sweep
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "strategy",
    [
        ChunkingStrategy.SLIDING_WINDOW,
        ChunkingStrategy.SENTENCE,
        ChunkingStrategy.PARAGRAPH,
        ChunkingStrategy.SEMANTIC,
        ChunkingStrategy.RECURSIVE,
        ChunkingStrategy.FIXED_SIZE,
        ChunkingStrategy.CODE,
    ],
)
def test_all_strategies_process_offline(strategy):
    p = create_pipeline(strategy=strategy)
    r = p.process_text(PARA_TEXT, "doc1")
    assert r.success is True
    assert isinstance(r.chunk_count, int)
