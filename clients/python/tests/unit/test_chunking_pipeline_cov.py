"""Offline unit tests for proximadb_sdk.chunking_strategies.pipeline.

Fully offline: no network, no model downloads, no real DB. The pipeline is a
pure CPU module operating over text; embedding providers and vector stores are
injected as in-process fakes/mocks.
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

SAMPLE_TEXT = (
    "The quick brown fox jumps over the lazy dog. "
    "Pack my box with five dozen liquor jugs. "
    "How vexingly quick daft zebras jump.\n\n"
    "A second paragraph that contains more sentences for chunking. "
    "It keeps going so the chunker has material to work with."
) * 4

SAMPLE_CODE = (
    "def foo(x):\n"
    "    return x + 1\n\n"
    "def bar(y):\n"
    "    return y * 2\n\n"
    "class Baz:\n"
    "    def method(self):\n"
    "        return 42\n"
)


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeEmbeddingProvider:
    """In-process embedding provider returning deterministic vectors."""

    def __init__(self, dim: int = 4):
        self._dim = dim
        self.calls = 0

    def embed_texts(self, texts):
        self.calls += 1
        return [[float(len(t) % 7)] * self._dim for t in texts]

    async def embed_texts_async(self, texts):
        return self.embed_texts(texts)

    @property
    def dimension(self) -> int:
        return self._dim


class FlakyProvider:
    """Fails the first N calls, then succeeds."""

    def __init__(self, fail_times: int, dim: int = 3):
        self.fail_times = fail_times
        self._dim = dim
        self.calls = 0

    def embed_texts(self, texts):
        self.calls += 1
        if self.calls <= self.fail_times:
            raise RuntimeError("transient")
        return [[1.0] * self._dim for _ in texts]

    @property
    def dimension(self) -> int:
        return self._dim


class AlwaysFailProvider:
    def embed_texts(self, texts):
        raise RuntimeError("boom")

    @property
    def dimension(self) -> int:
        return 2


class SyncOnlyProvider:
    """Provider lacking embed_texts_async to exercise the executor fallback."""

    def embed_texts(self, texts):
        return [[0.5, 0.5] for _ in texts]

    @property
    def dimension(self) -> int:
        return 2


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


@pytest.fixture(autouse=True)
def _reset_metrics():
    from proximadb_sdk.chunking_strategies.parser_utils import get_metrics_collector

    get_metrics_collector().clear()
    yield
    get_metrics_collector().clear()


# ---------------------------------------------------------------------------
# Config / dataclasses
# ---------------------------------------------------------------------------


def test_pipeline_config_post_init_default_chunking_config():
    cfg = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
    assert cfg.chunking_config is not None
    assert cfg.chunking_config.strategy == ChunkingStrategy.SENTENCE


def test_pipeline_config_keeps_supplied_chunking_config():
    supplied = ChunkingConfig(strategy=ChunkingStrategy.PARAGRAPH, chunk_size=300)
    cfg = PipelineConfig(
        chunking_strategy=ChunkingStrategy.PARAGRAPH, chunking_config=supplied
    )
    assert cfg.chunking_config is supplied


def test_enums_values():
    assert ProcessingMode.SYNC.value == "sync"
    assert ErrorHandling.FAIL_FAST.value == "fail_fast"


def test_pipeline_result_properties_and_to_dict():
    chunks = [TextChunk("a", 0, 1, "c0"), TextChunk("b", 1, 2, "c1")]
    errs = [{"error": str(i)} for i in range(15)]
    res = PipelineResult(success=False, chunks=chunks, errors=errs, metrics={"x": 1})
    assert res.chunk_count == 2
    assert res.error_count == 15
    d = res.to_dict()
    assert d["chunk_count"] == 2
    assert d["error_count"] == 15
    assert len(d["errors"]) == 10  # capped


def test_pipeline_result_empty_errors_to_dict():
    res = PipelineResult(success=True)
    d = res.to_dict()
    assert d["errors"] == []


def test_batch_result_success_rate():
    assert BatchResult(total_items=0).success_rate == 0.0
    assert BatchResult(total_items=4, processed_items=2).success_rate == 0.5


# ---------------------------------------------------------------------------
# Stages
# ---------------------------------------------------------------------------


def test_validation_stage_disabled_returns_chunk():
    cfg = PipelineConfig(validate_chunks=False)
    stage = ValidationStage(cfg)
    chunk = TextChunk("", 0, 0, "c")
    assert stage.process(chunk) is chunk
    assert stage.name == "validation"


def test_validation_stage_empty_chunk_raises():
    stage = ValidationStage(PipelineConfig(validate_chunks=True))
    with pytest.raises(ValueError):
        stage.process(TextChunk("   ", 0, 0, "empty"))


def test_validation_stage_truncates_long_text():
    cfg = PipelineConfig(
        validate_chunks=True, max_text_length=5, truncate_long_texts=True
    )
    stage = ValidationStage(cfg)
    out = stage.process(TextChunk("abcdefghij", 0, 10, "c"))
    assert out.text == "abcde"
    assert out.metadata["truncated"] is True


def test_validation_stage_long_text_no_truncate_raises():
    cfg = PipelineConfig(
        validate_chunks=True, max_text_length=5, truncate_long_texts=False
    )
    stage = ValidationStage(cfg)
    with pytest.raises(ValueError):
        stage.process(TextChunk("abcdefghij", 0, 10, "c"))


def test_enrichment_stage_applies_funcs():
    stage = EnrichmentStage()
    assert stage.name == "enrichment"

    def tag(chunk):
        chunk.metadata["tagged"] = True
        return chunk

    stage.add_enrichment(tag)
    out = stage.process(TextChunk("x", 0, 1, "c"))
    assert out.metadata["tagged"] is True


def test_enrichment_stage_no_funcs_passthrough():
    stage = EnrichmentStage()
    chunk = TextChunk("x", 0, 1, "c")
    assert stage.process(chunk) is chunk


def test_filter_stage_no_predicates_passthrough():
    stage = FilterStage()
    assert stage.name == "filter"
    chunks = [TextChunk("x", 0, 1, "c")]
    assert stage.process(chunks) is chunks


def test_filter_stage_with_predicate():
    stage = FilterStage()
    stage.add_predicate(lambda c: len(c.text) > 1)
    chunks = [TextChunk("a", 0, 1, "c0"), TextChunk("abc", 0, 3, "c1")]
    out = stage.process(chunks)
    assert [c.chunk_id for c in out] == ["c1"]


@pytest.mark.asyncio
async def test_stage_process_async_default_wraps_sync():
    stage = EnrichmentStage([lambda c: c])
    chunk = TextChunk("x", 0, 1, "c")
    out = await stage.process_async(chunk)
    assert out is chunk


# ---------------------------------------------------------------------------
# BatchEmbedder
# ---------------------------------------------------------------------------


def test_batch_embedder_embeds_across_batches():
    provider = FakeEmbeddingProvider(dim=4)
    embedder = BatchEmbedder(provider, batch_size=2)
    out = embedder.embed_batch(["a", "bb", "ccc", "dddd", "e"])
    assert len(out) == 5
    assert all(len(v) == 4 for v in out)
    # 5 texts, batch_size 2 -> 3 provider invocations
    assert provider.calls == 3
    assert embedder.stats["request_count"] == 3
    assert embedder.stats["batch_size"] == 2


def test_batch_embedder_retry_then_success():
    provider = FlakyProvider(fail_times=2)
    embedder = BatchEmbedder(provider, batch_size=10, max_retries=3, retry_delay=0.0)
    out = embedder.embed_batch(["a", "b"])
    assert len(out) == 2
    assert provider.calls == 3


def test_batch_embedder_exhausts_retries():
    provider = AlwaysFailProvider()
    embedder = BatchEmbedder(provider, batch_size=10, max_retries=2, retry_delay=0.0)
    with pytest.raises(RuntimeError):
        embedder.embed_batch(["a"])


@pytest.mark.asyncio
async def test_batch_embedder_async_with_async_provider():
    provider = FakeEmbeddingProvider(dim=3)
    embedder = BatchEmbedder(provider, batch_size=2)
    out = await embedder.embed_batch_async(["a", "bb", "ccc"])
    assert len(out) == 3


@pytest.mark.asyncio
async def test_batch_embedder_async_sync_fallback():
    provider = SyncOnlyProvider()
    embedder = BatchEmbedder(provider, batch_size=2)
    out = await embedder.embed_batch_async(["a", "b"])
    assert out == [[0.5, 0.5], [0.5, 0.5]]


@pytest.mark.asyncio
async def test_batch_embedder_async_exhausts_retries():
    provider = AlwaysFailProvider()
    embedder = BatchEmbedder(provider, batch_size=10, max_retries=2, retry_delay=0.0)
    with pytest.raises(RuntimeError):
        await embedder.embed_batch_async(["a"])


# ---------------------------------------------------------------------------
# ProgressTracker
# ---------------------------------------------------------------------------


def test_progress_tracker_lifecycle_and_callback():
    seen = []
    tracker = ProgressTracker(lambda c, t, s: seen.append((c, t, s)))
    tracker.start(3, "go")
    tracker.update(1)
    tracker.update(2, status="mid")
    tracker.complete("done")
    assert seen[0] == (0, 3, "go")
    assert seen[-1] == (3, 3, "done")
    assert tracker.elapsed_time >= 0.0
    assert tracker.items_per_second >= 0.0


def test_progress_tracker_no_callback_and_idle_metrics():
    tracker = ProgressTracker()
    # never started -> start_time None
    assert tracker.elapsed_time == 0.0
    assert tracker.items_per_second == 0.0
    tracker.update()  # no callback, no crash


# ---------------------------------------------------------------------------
# Pipeline construction
# ---------------------------------------------------------------------------


def test_pipeline_default_construction():
    p = ChunkingPipeline()
    assert p.embedder is None
    assert p.chunker is not None


def test_pipeline_code_strategy_builds_code_chunker():
    cfg = PipelineConfig(chunking_strategy=ChunkingStrategy.CODE)
    p = ChunkingPipeline(cfg)
    assert p.chunker is not None


def test_pipeline_code_strategy_with_code_config():
    code_cfg = CodeChunkingConfig(chunk_size=256, chunk_overlap=20)
    cfg = PipelineConfig(
        chunking_strategy=ChunkingStrategy.CODE, chunking_config=code_cfg
    )
    p = ChunkingPipeline(cfg)
    assert p.chunker is not None


def test_pipeline_builder_api():
    provider = FakeEmbeddingProvider()
    store = FakeVectorStore()
    p = ChunkingPipeline()
    out = (
        p.with_strategy(ChunkingStrategy.SENTENCE)
        .with_embedding_provider(provider)
        .with_vector_store(store)
        .with_enrichment(lambda c: c)
        .with_filter(lambda c: True)
        .with_progress_callback(lambda c, t, s: None)
    )
    assert out is p
    assert p.embedder is not None
    assert p.vector_store is store
    assert p.config.chunking_strategy == ChunkingStrategy.SENTENCE


# ---------------------------------------------------------------------------
# process_text (sync)
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
    ],
)
def test_process_text_each_strategy(strategy):
    cfg = PipelineConfig(
        chunking_strategy=strategy,
        chunking_config=ChunkingConfig(
            strategy=strategy, chunk_size=120, chunk_overlap=20
        ),
    )
    p = ChunkingPipeline(cfg)
    res = p.process_text(SAMPLE_TEXT, "doc1")
    assert res.success is True
    assert res.chunk_count > 0
    assert res.metrics["input_length"] == len(SAMPLE_TEXT)
    assert res.metrics["chunk_count"] == res.chunk_count


def test_process_text_with_embeddings():
    provider = FakeEmbeddingProvider(dim=4)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, embedding_provider=provider)
    res = p.process_text(SAMPLE_TEXT, "doc1", metadata={"k": "v"})
    assert res.success is True
    assert len(res.embeddings) == res.chunk_count
    assert all(len(v) == 4 for v in res.embeddings)


def test_process_text_embedding_error_collected():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=AlwaysFailProvider(),
        max_retries=1,
        retry_delay=0.0,
        error_handling=ErrorHandling.COLLECT_ERRORS,
    )
    res = p.process_text(SAMPLE_TEXT, "doc1")
    assert res.success is False
    assert any(e["stage"] == "embedding" for e in res.errors)


def test_process_text_embedding_error_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=AlwaysFailProvider(),
        max_retries=1,
        retry_delay=0.0,
        error_handling=ErrorHandling.FAIL_FAST,
    )
    res = p.process_text(SAMPLE_TEXT, "doc1")
    # fail-fast re-raises inside, caught by outer handler -> pipeline stage error
    assert res.success is False
    assert any(e["stage"] == "pipeline" for e in res.errors)


def test_process_text_validation_error_collected():
    # max_text_length tiny + no truncation -> per-chunk validation errors collected
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.COLLECT_ERRORS,
    )
    res = p.process_text(SAMPLE_TEXT, "doc1")
    assert res.success is False
    assert any(e["stage"] == "validation" for e in res.errors)


def test_process_text_validation_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
    )
    res = p.process_text(SAMPLE_TEXT, "doc1")
    assert res.success is False
    assert any(e["stage"] == "pipeline" for e in res.errors)


def test_process_text_chunker_raises(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    def boom(*a, **k):
        raise RuntimeError("chunk-fail")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    res = p.process_text("hello", "doc")
    assert res.success is False
    assert res.errors[0]["stage"] == "pipeline"


def test_process_text_metrics_disabled():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, enable_metrics=False)
    res = p.process_text(SAMPLE_TEXT, "doc1")
    assert res.success is True


# ---------------------------------------------------------------------------
# Async processing
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_process_text_async_with_embeddings():
    provider = FakeEmbeddingProvider(dim=3)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, embedding_provider=provider)
    res = await p.process_text_async(SAMPLE_TEXT, "doc1")
    assert res.success is True
    assert res.metrics["mode"] == "async"
    assert len(res.embeddings) == res.chunk_count


@pytest.mark.asyncio
async def test_process_text_async_chunker_raises(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    def boom(*a, **k):
        raise RuntimeError("x")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    res = await p.process_text_async("hi", "doc")
    assert res.success is False
    assert res.errors[0]["stage"] == "pipeline"


@pytest.mark.asyncio
async def test_process_text_async_validation_collected():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.COLLECT_ERRORS,
    )
    res = await p.process_text_async(SAMPLE_TEXT, "doc1")
    assert res.success is False
    assert any(e["stage"] == "validation" for e in res.errors)


@pytest.mark.asyncio
async def test_process_text_async_embedding_error():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=AlwaysFailProvider(),
        max_retries=1,
        retry_delay=0.0,
    )
    res = await p.process_text_async(SAMPLE_TEXT, "doc1")
    assert res.success is False
    assert any(e["stage"] == "embedding" for e in res.errors)


# ---------------------------------------------------------------------------
# Streaming
# ---------------------------------------------------------------------------


def test_process_stream_yields_chunks():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    chunks = list(p.process_stream(SAMPLE_TEXT, "doc"))
    assert len(chunks) > 0
    assert all(isinstance(c, TextChunk) for c in chunks)


def test_process_stream_skips_invalid_chunks():
    # tiny max length, no truncate, collect -> per-chunk skip (logged), keeps going
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.SKIP_ERRORS,
    )
    chunks = list(p.process_stream(SAMPLE_TEXT, "doc"))
    assert chunks == []


def test_process_stream_fail_fast_raises():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
    )
    with pytest.raises(ValueError):
        list(p.process_stream(SAMPLE_TEXT, "doc"))


def test_process_stream_chunker_raises_fail_fast(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.FAIL_FAST
    )

    def boom(*a, **k):
        raise RuntimeError("x")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    with pytest.raises(RuntimeError):
        list(p.process_stream("hi", "doc"))


def test_process_stream_chunker_raises_collect(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.COLLECT_ERRORS
    )

    def boom(*a, **k):
        raise RuntimeError("x")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    assert list(p.process_stream("hi", "doc")) == []


@pytest.mark.asyncio
async def test_process_stream_async_yields():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    out = [c async for c in p.process_stream_async(SAMPLE_TEXT, "doc")]
    assert len(out) > 0


@pytest.mark.asyncio
async def test_process_stream_async_skips_invalid():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.SKIP_ERRORS,
    )
    out = [c async for c in p.process_stream_async(SAMPLE_TEXT, "doc")]
    assert out == []


@pytest.mark.asyncio
async def test_process_stream_async_fail_fast():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        max_text_length=3,
        truncate_long_texts=False,
        error_handling=ErrorHandling.FAIL_FAST,
    )
    with pytest.raises(ValueError):
        [c async for c in p.process_stream_async(SAMPLE_TEXT, "doc")]


@pytest.mark.asyncio
async def test_process_stream_async_chunker_raises_fail_fast(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.FAIL_FAST
    )

    def boom(*a, **k):
        raise RuntimeError("x")

    monkeypatch.setattr(p.chunker, "chunk", boom)
    with pytest.raises(RuntimeError):
        [c async for c in p.process_stream_async("hi", "doc")]


# ---------------------------------------------------------------------------
# Batch processing
# ---------------------------------------------------------------------------


def test_process_batch():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    items = [
        {"text": SAMPLE_TEXT, "source_id": "a"},
        {"text": SAMPLE_TEXT, "metadata": {"x": 1}},  # source_id defaulted
        {"text": ""},  # empty text -> chunker yields nothing
    ]
    result = p.process_batch(items)
    assert result.total_items == 3
    assert result.processed_items >= 1
    assert len(result.results) == 3


def test_process_batch_item_exception_collected(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    calls = {"n": 0}
    orig = p.process_text

    def maybe_fail(text, source_id, metadata=None):
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("item-fail")
        return orig(text, source_id, metadata)

    monkeypatch.setattr(p, "process_text", maybe_fail)
    result = p.process_batch(
        [{"text": SAMPLE_TEXT}, {"text": SAMPLE_TEXT}]
    )
    assert any(e.get("error") == "item-fail" for e in result.errors)


def test_process_batch_fail_fast_breaks(monkeypatch):
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE, error_handling=ErrorHandling.FAIL_FAST
    )

    def always_fail(text, source_id, metadata=None):
        raise RuntimeError("item-fail")

    monkeypatch.setattr(p, "process_text", always_fail)
    result = p.process_batch([{"text": "a"}, {"text": "b"}, {"text": "c"}])
    # broke after first failure -> only one error recorded, no results
    assert len(result.errors) == 1
    assert result.results == []


@pytest.mark.asyncio
async def test_process_batch_async():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    items = [{"text": SAMPLE_TEXT, "source_id": str(i)} for i in range(3)]
    result = await p.process_batch_async(items, concurrent_limit=2)
    assert result.total_items == 3
    assert result.processed_items == 3


@pytest.mark.asyncio
async def test_process_batch_async_gather_exception(monkeypatch):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    async def boom(text, source_id, metadata=None):
        raise RuntimeError("async-item-fail")

    monkeypatch.setattr(p, "process_text_async", boom)
    result = await p.process_batch_async([{"text": "a"}])
    assert any("async-item-fail" in e.get("error", "") for e in result.errors)
    assert result.processed_items == 0


# ---------------------------------------------------------------------------
# File / directory processing
# ---------------------------------------------------------------------------


def test_process_file(tmp_path):
    f = tmp_path / "doc.txt"
    f.write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    res = p.process_file(f)
    assert res.success is True
    assert res.chunk_count > 0


def test_process_file_not_found(tmp_path):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    res = p.process_file(tmp_path / "missing.txt")
    assert res.success is False
    assert "not found" in res.errors[0]["error"].lower()


def test_process_file_read_error(tmp_path, monkeypatch):
    f = tmp_path / "doc.txt"
    f.write_text("x")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    import pathlib

    def boom(self, *a, **k):
        raise OSError("read-fail")

    monkeypatch.setattr(pathlib.Path, "read_text", boom)
    res = p.process_file(f)
    assert res.success is False
    assert "Failed to read file" in res.errors[0]["error"]


@pytest.mark.asyncio
async def test_process_file_async(tmp_path):
    f = tmp_path / "doc.txt"
    f.write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    res = await p.process_file_async(f)
    assert res.success is True


@pytest.mark.asyncio
async def test_process_file_async_not_found(tmp_path):
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    res = await p.process_file_async(tmp_path / "nope.txt")
    assert res.success is False


@pytest.mark.asyncio
async def test_process_file_async_read_error(tmp_path, monkeypatch):
    f = tmp_path / "doc.txt"
    f.write_text("x")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    import pathlib

    def boom(self, *a, **k):
        raise OSError("read-fail")

    monkeypatch.setattr(pathlib.Path, "read_text", boom)
    res = await p.process_file_async(f)
    assert res.success is False
    assert "Failed to read file" in res.errors[0]["error"]


def test_process_directory(tmp_path):
    (tmp_path / "a.txt").write_text(SAMPLE_TEXT)
    (tmp_path / "b.txt").write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    result = p.process_directory(tmp_path, pattern="*.txt", recursive=True)
    assert result.total_items == 2
    assert result.processed_items == 2


def test_process_directory_non_recursive(tmp_path):
    (tmp_path / "a.txt").write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    result = p.process_directory(tmp_path, pattern="*.txt", recursive=False)
    assert result.total_items == 1


def test_process_directory_not_a_dir(tmp_path):
    f = tmp_path / "file.txt"
    f.write_text("x")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    result = p.process_directory(f)
    assert result.errors
    assert "Not a directory" in result.errors[0]["error"]


@pytest.mark.asyncio
async def test_process_directory_async(tmp_path):
    (tmp_path / "a.txt").write_text(SAMPLE_TEXT)
    (tmp_path / "b.txt").write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    result = await p.process_directory_async(
        tmp_path, pattern="*.txt", concurrent_limit=2
    )
    assert result.total_items == 2
    assert result.processed_items == 2


@pytest.mark.asyncio
async def test_process_directory_async_non_recursive(tmp_path):
    (tmp_path / "a.txt").write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    result = await p.process_directory_async(
        tmp_path, pattern="*.txt", recursive=False
    )
    assert result.total_items == 1


@pytest.mark.asyncio
async def test_process_directory_async_not_a_dir(tmp_path):
    f = tmp_path / "file.txt"
    f.write_text("x")
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    result = await p.process_directory_async(f)
    assert "Not a directory" in result.errors[0]["error"]


@pytest.mark.asyncio
async def test_process_directory_async_gather_exception(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text(SAMPLE_TEXT)
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)

    async def boom(f):
        raise RuntimeError("file-fail")

    monkeypatch.setattr(p, "process_file_async", boom)
    result = await p.process_directory_async(tmp_path, pattern="*.txt")
    assert any("file-fail" in e.get("error", "") for e in result.errors)


# ---------------------------------------------------------------------------
# process_and_store
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_process_and_store_no_vector_store_raises():
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeEmbeddingProvider(),
    )
    with pytest.raises(ValueError):
        await p.process_and_store(SAMPLE_TEXT, "doc")


@pytest.mark.asyncio
async def test_process_and_store_no_provider_raises():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, vector_store=FakeVectorStore())
    with pytest.raises(ValueError):
        await p.process_and_store(SAMPLE_TEXT, "doc")


@pytest.mark.asyncio
async def test_process_and_store_success():
    store = FakeVectorStore()
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeEmbeddingProvider(dim=4),
        vector_store=store,
    )
    res = await p.process_and_store(SAMPLE_TEXT, "doc")
    assert res.success is True
    assert store.records is not None
    assert res.metrics["records_stored"] == len(store.records)
    assert store.records[0]["metadata"]["source_id"] == "doc"


@pytest.mark.asyncio
async def test_process_and_store_storage_error():
    store = FakeVectorStore(fail=True)
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeEmbeddingProvider(dim=4),
        vector_store=store,
    )
    res = await p.process_and_store(SAMPLE_TEXT, "doc")
    assert res.success is False
    assert any(e["stage"] == "storage" for e in res.errors)


@pytest.mark.asyncio
async def test_process_and_store_returns_early_on_failed_process(monkeypatch):
    store = FakeVectorStore()
    p = create_pipeline(
        strategy=ChunkingStrategy.SENTENCE,
        embedding_provider=FakeEmbeddingProvider(),
        vector_store=store,
    )

    async def failed(text, source_id, metadata=None):
        return PipelineResult(success=False)

    monkeypatch.setattr(p, "process_text_async", failed)
    res = await p.process_and_store(SAMPLE_TEXT, "doc")
    assert res.success is False
    assert store.records is None


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------


def test_get_and_reset_metrics():
    provider = FakeEmbeddingProvider()
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, embedding_provider=provider)
    p.process_text(SAMPLE_TEXT, "doc")
    metrics = p.get_metrics()
    assert "embedder_stats" in metrics
    assert "progress" in metrics
    p.reset_metrics()
    assert p._errors == []


def test_get_metrics_without_embedder():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE)
    metrics = p.get_metrics()
    assert metrics["embedder_stats"] == {}


def test_record_metrics_noop_when_disabled():
    p = create_pipeline(strategy=ChunkingStrategy.SENTENCE, enable_metrics=False)
    # call directly; should early-return without touching collector incorrectly
    p._record_metrics({"processing_time_sec": 0.01, "chunk_count": 2})


# ---------------------------------------------------------------------------
# Factory functions and context managers
# ---------------------------------------------------------------------------


def test_create_pipeline_defaults():
    p = create_pipeline()
    assert isinstance(p, ChunkingPipeline)
    assert p.config.chunking_strategy == ChunkingStrategy.SEMANTIC


def test_create_code_pipeline():
    p = create_code_pipeline()
    assert p.config.chunking_strategy == ChunkingStrategy.CODE
    assert p.config.max_text_length == 16384


def test_create_document_pipeline():
    p = create_document_pipeline()
    assert p.config.chunking_strategy == ChunkingStrategy.SEMANTIC
    assert p.config.max_text_length == 8192


def test_pipeline_context_manager():
    with pipeline_context(strategy=ChunkingStrategy.SENTENCE) as p:
        res = p.process_text(SAMPLE_TEXT, "doc")
        assert res.success is True


@pytest.mark.asyncio
async def test_async_pipeline_context_manager():
    async with async_pipeline_context(strategy=ChunkingStrategy.SENTENCE) as p:
        res = await p.process_text_async(SAMPLE_TEXT, "doc")
        assert res.success is True


def test_code_pipeline_processes_code():
    p = create_code_pipeline()
    res = p.process_text(SAMPLE_CODE, "mod.py", metadata={"file_extension": ".py"})
    assert res.chunk_count >= 0  # may be 0 if tree-sitter absent; must not crash
    assert res.success is True
