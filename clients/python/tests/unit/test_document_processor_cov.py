"""Offline unit tests for proximadb_sdk.document_processor.

Fully offline: no network, no model downloads, no real chunkers. The module
only does lazy imports of chunking strategies inside ``_get_chunker``; we
inject a fake chunker so those imports never run.
"""

import asyncio
from types import SimpleNamespace

import pytest

from proximadb_sdk.document_processor import (
    AsyncEmbeddingProvider,  # noqa: F401 (Protocol import smoke)
    CodeDocumentProcessor,
    DocumentProcessor,
    DocumentProcessorRegistry,
    DocumentType,
    EmbeddingProvider,  # noqa: F401
    EmbeddingProviderAdapter,
    PlaceholderEmbeddingProvider,
    ProcessedChunk,
    ProcessingResult,
    ProcessingStrategy,
    ProcessorConfig,
    TextDocumentProcessor,
    VectorRecord,
    VectorStore,  # noqa: F401
    create_embedding_adapter,
    create_processor,
    detect_document_type,
    get_processor_registry,
)


# ---------------------------------------------------------------------------
# Test doubles
# ---------------------------------------------------------------------------


def _make_chunk(chunk_id="c0", text="hello world", start=0, end=11, metadata=None):
    return SimpleNamespace(
        chunk_id=chunk_id,
        text=text,
        start_pos=start,
        end_pos=end,
        metadata=metadata if metadata is not None else {},
    )


class FakeChunker:
    """Mimics a chunking strategy: .chunk(content, source_id, metadata)."""

    def __init__(self, chunks=None):
        self._chunks = chunks
        self.calls = []

    def chunk(self, content, source_id, metadata=None):
        self.calls.append((content, source_id, metadata))
        if self._chunks is not None:
            return self._chunks
        return [_make_chunk("c0", content[:5] or "x", 0, min(5, len(content)))]


class SyncProvider:
    def __init__(self, dim=4):
        self._dim = dim
        self.calls = []

    @property
    def dimension(self):
        return self._dim

    def embed_texts(self, texts):
        self.calls.append(list(texts))
        return [[float(len(t))] * self._dim for t in texts]


class AsyncProvider:
    def __init__(self, dim=3):
        self._dim = dim
        self.calls = []

    @property
    def dimension(self):
        return self._dim

    async def embed_texts_async(self, texts):
        self.calls.append(list(texts))
        return [[1.0] * self._dim for _ in texts]


class FlakyProvider:
    """Fails the first ``fail_times`` calls, then succeeds."""

    def __init__(self, fail_times, dim=2):
        self.fail_times = fail_times
        self.calls = 0
        self._dim = dim

    @property
    def dimension(self):
        return self._dim

    def embed_texts(self, texts):
        self.calls += 1
        if self.calls <= self.fail_times:
            raise ValueError("boom")
        return [[0.0] * self._dim for _ in texts]


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------


def test_processed_chunk_post_init_defaults_embedding_text():
    c = ProcessedChunk(chunk_id="a", text="body", start_pos=0, end_pos=4)
    assert c.embedding_text == "body"
    assert c.metadata == {}


def test_processed_chunk_keeps_explicit_embedding_text():
    c = ProcessedChunk(chunk_id="a", text="body", start_pos=0, end_pos=4, embedding_text="EMB")
    assert c.embedding_text == "EMB"


def test_vector_record_to_dict():
    rec = VectorRecord(id="i", vector=[0.1, 0.2], metadata={"k": "v"}, text="t", source_id="s")
    d = rec.to_dict()
    assert d["id"] == "i"
    assert d["vector"] == [0.1, 0.2]
    assert d["metadata"]["k"] == "v"
    assert d["metadata"]["text"] == "t"
    assert d["metadata"]["source_id"] == "s"


def test_processing_result_counts():
    res = ProcessingResult(
        success=True,
        source_id="s",
        document_type=DocumentType.TEXT,
        chunks=[_make_chunk(), _make_chunk()],
        vectors=[VectorRecord("i", [0.0], {}, "t", "s")],
    )
    assert res.chunk_count == 2
    assert res.vector_count == 1


def test_processor_config_defaults():
    cfg = ProcessorConfig()
    assert cfg.chunk_size == 512
    assert cfg.strategy == ProcessingStrategy.BALANCED
    assert cfg.ocr_enabled is True


# ---------------------------------------------------------------------------
# EmbeddingProviderAdapter
# ---------------------------------------------------------------------------


def test_adapter_dimension_from_provider():
    a = EmbeddingProviderAdapter(SyncProvider(dim=7))
    assert a.dimension == 7


def test_adapter_dimension_missing_returns_zero():
    class NoDim:
        def embed_texts(self, texts):
            return [[0.0] for _ in texts]

    a = EmbeddingProviderAdapter(NoDim())
    assert a.dimension == 0


def test_adapter_embed_texts_empty_returns_empty():
    a = EmbeddingProviderAdapter(SyncProvider())
    assert a.embed_texts([]) == []


def test_adapter_embed_texts_batches():
    p = SyncProvider(dim=2)
    a = EmbeddingProviderAdapter(p, batch_size=2)
    out = a.embed_texts(["aa", "bbb", "c"])
    assert len(out) == 3
    # 3 texts / batch 2 => two provider calls
    assert len(p.calls) == 2
    assert a.stats["request_count"] == 2
    assert a.stats["total_texts"] == 3
    assert a.stats["batch_size"] == 2
    assert a.stats["is_async"] is False


def test_adapter_detects_async_provider():
    a = EmbeddingProviderAdapter(AsyncProvider())
    assert a.stats["is_async"] is True


def test_adapter_embed_texts_async_empty():
    a = EmbeddingProviderAdapter(SyncProvider())
    assert asyncio.run(a.embed_texts_async([])) == []


def test_adapter_embed_texts_async_with_async_provider():
    p = AsyncProvider(dim=3)
    a = EmbeddingProviderAdapter(p, batch_size=2)
    out = asyncio.run(a.embed_texts_async(["x", "y", "z"]))
    assert len(out) == 3
    assert all(len(v) == 3 for v in out)
    assert len(p.calls) == 2


def test_adapter_embed_texts_async_runs_sync_in_executor():
    p = SyncProvider(dim=2)
    a = EmbeddingProviderAdapter(p, batch_size=10)
    out = asyncio.run(a.embed_texts_async(["one", "two"]))
    assert len(out) == 2
    assert len(p.calls) == 1


def test_adapter_sync_retries_then_succeeds(monkeypatch):
    import proximadb_sdk.document_processor as mod

    sleeps = []
    monkeypatch.setattr(mod.time, "sleep", lambda s: sleeps.append(s))
    p = FlakyProvider(fail_times=2)
    a = EmbeddingProviderAdapter(p, max_retries=3, retry_delay=0.01)
    out = a.embed_texts(["a"])
    assert len(out) == 1
    assert p.calls == 3
    assert len(sleeps) == 2  # slept after the two failures


def test_adapter_sync_exhausts_retries(monkeypatch):
    import proximadb_sdk.document_processor as mod

    monkeypatch.setattr(mod.time, "sleep", lambda s: None)
    p = FlakyProvider(fail_times=5)
    a = EmbeddingProviderAdapter(p, max_retries=3, retry_delay=0.01)
    with pytest.raises(RuntimeError, match="Embedding failed after 3 attempts"):
        a.embed_texts(["a"])


def test_adapter_async_retries_then_succeeds(monkeypatch):
    import proximadb_sdk.document_processor as mod

    async def fake_sleep(s):
        return None

    monkeypatch.setattr(mod.asyncio, "sleep", fake_sleep)
    p = FlakyProvider(fail_times=1)
    a = EmbeddingProviderAdapter(p, max_retries=3, retry_delay=0.01)
    out = asyncio.run(a.embed_texts_async(["a"]))
    assert len(out) == 1
    assert p.calls == 2


def test_adapter_async_exhausts_retries(monkeypatch):
    import proximadb_sdk.document_processor as mod

    async def fake_sleep(s):
        return None

    monkeypatch.setattr(mod.asyncio, "sleep", fake_sleep)
    p = FlakyProvider(fail_times=10)
    a = EmbeddingProviderAdapter(p, max_retries=2, retry_delay=0.01)
    with pytest.raises(RuntimeError, match="Async embedding failed after 2 attempts"):
        asyncio.run(a.embed_texts_async(["a"]))


# ---------------------------------------------------------------------------
# PlaceholderEmbeddingProvider
# ---------------------------------------------------------------------------


def test_placeholder_dimension_and_shape():
    p = PlaceholderEmbeddingProvider(dimension=16)
    assert p.dimension == 16
    out = p.embed_texts(["hello", "world"])
    assert len(out) == 2
    assert all(len(v) == 16 for v in out)
    assert all(-1.0 <= x <= 1.0 for v in out for x in v)


def test_placeholder_deterministic():
    p = PlaceholderEmbeddingProvider(dimension=8)
    assert p.embed_texts(["same"]) == p.embed_texts(["same"])


def test_placeholder_async_matches_sync():
    p = PlaceholderEmbeddingProvider(dimension=8)
    a = asyncio.run(p.embed_texts_async(["x"]))
    assert a == p.embed_texts(["x"])


# ---------------------------------------------------------------------------
# CodeDocumentProcessor
# ---------------------------------------------------------------------------


def test_code_processor_basic_props():
    p = CodeDocumentProcessor()
    assert p.name == "code"
    assert p.supported_types == [DocumentType.CODE]


def test_code_can_process_by_extension():
    p = CodeDocumentProcessor()
    assert p.can_process("anything", "module.py") is True
    assert p.can_process("anything", "lib.rs") is True


def test_code_can_process_by_content():
    p = CodeDocumentProcessor()
    assert p.can_process("def foo():\n    pass") is True
    assert p.can_process("just some prose without keywords") is False


def test_code_can_process_unknown_ext_falls_to_content():
    p = CodeDocumentProcessor()
    assert p.can_process("class A: pass", "data.bin") is True


def test_code_chunk_uses_injected_chunker_and_prepares_embedding():
    p = CodeDocumentProcessor()
    meta = {
        "fully_qualified_name": "pkg.mod.func",
        "symbol_type": "function",
        "documentation": "x" * 600,  # exercises truncation branch
        "signature": "func(a, b)",
        "language": "python",
    }
    p._chunker = FakeChunker(chunks=[_make_chunk("c1", "def func(): ...", 0, 14, meta)])
    chunks = p.chunk("def func(): ...", "src1")
    assert len(chunks) == 1
    emb = chunks[0].embedding_text
    assert "Symbol: pkg.mod.func" in emb
    assert "Type: function" in emb
    assert "Documentation: " in emb
    assert "..." in emb  # truncated doc
    assert "Signature: func(a, b)" in emb
    assert "Code:\n" in emb


def test_code_prepare_embedding_minimal_metadata():
    p = CodeDocumentProcessor()
    p._chunker = FakeChunker(chunks=[_make_chunk("c2", "x = 1", 0, 5, {})])
    chunks = p.chunk("x = 1", "src2")
    assert chunks[0].embedding_text.startswith("Code:\n")


def test_code_prepare_for_embedding_fallback_to_text():
    p = CodeDocumentProcessor()
    chunk = ProcessedChunk(chunk_id="c", text="t", start_pos=0, end_pos=1, embedding_text=None)
    chunk.embedding_text = None  # force the or-branch
    assert p.prepare_for_embedding(chunk) == "t"


def test_code_enrich_metadata_defaults():
    p = CodeDocumentProcessor()
    chunk = ProcessedChunk(chunk_id="c", text="code", start_pos=0, end_pos=4, metadata={})
    md = p.enrich_metadata(chunk, source_metadata={"file": "a.py"})
    assert md["processor"] == "code"
    assert md["is_code"] is True
    assert md["language"] == "unknown"
    assert md["symbol_type"] == "unknown"
    assert md["source"] == {"file": "a.py"}
    assert md["text_length"] == 4


def test_code_get_chunker_lazy_import(monkeypatch):
    # Patch the lazily-imported module so no real chunking strategy loads.
    import sys
    import types as _types

    fake_mod = _types.ModuleType("proximadb_sdk.chunking_strategies.code")
    captured = {}

    class CodeChunkingConfig:
        def __init__(self, **kw):
            captured.update(kw)

    class CodeChunkingStrategy:
        def __init__(self, cfg):
            self.cfg = cfg

    fake_mod.CodeChunkingConfig = CodeChunkingConfig
    fake_mod.CodeChunkingStrategy = CodeChunkingStrategy
    monkeypatch.setitem(sys.modules, "proximadb_sdk.chunking_strategies.code", fake_mod)

    p = CodeDocumentProcessor(ProcessorConfig(chunk_size=100, chunk_overlap=10, extract_symbols=False))
    chunker = p._get_chunker()
    assert isinstance(chunker, CodeChunkingStrategy)
    assert captured["chunk_size"] == 100
    assert captured["chunk_overlap"] == 10
    assert captured["extract_relations"] is False
    # Cached on second call
    assert p._get_chunker() is chunker


# ---------------------------------------------------------------------------
# TextDocumentProcessor
# ---------------------------------------------------------------------------


def test_text_processor_props():
    p = TextDocumentProcessor()
    assert p.name == "text"
    assert DocumentType.TEXT in p.supported_types
    assert DocumentType.MARKDOWN in p.supported_types


def test_text_can_process_by_extension():
    p = TextDocumentProcessor()
    assert p.can_process("x", "notes.md") is True
    assert p.can_process("x", "doc.pdf") is False


def test_text_can_process_no_path_default_true():
    p = TextDocumentProcessor()
    assert p.can_process("anything") is True


def test_text_chunk_uses_injected_chunker():
    p = TextDocumentProcessor()
    p._chunker = FakeChunker(chunks=[_make_chunk("t1", "para text", 0, 9, {"k": 1})])
    chunks = p.chunk("para text", "src")
    assert len(chunks) == 1
    assert isinstance(chunks[0], ProcessedChunk)
    assert chunks[0].text == "para text"
    assert chunks[0].metadata == {"k": 1}


def test_text_get_chunker_lazy_import(monkeypatch):
    import sys
    import types as _types

    base_mod = _types.ModuleType("proximadb_sdk.chunking_strategies.base")

    class ChunkingStrategy:
        SEMANTIC = "semantic"

    class ChunkingConfig:
        def __init__(self, **kw):
            self.kw = kw

    base_mod.ChunkingStrategy = ChunkingStrategy
    base_mod.ChunkingConfig = ChunkingConfig

    sem_mod = _types.ModuleType("proximadb_sdk.chunking_strategies.semantic")

    class SemanticStrategy:
        def __init__(self, cfg):
            self.cfg = cfg

    sem_mod.SemanticStrategy = SemanticStrategy

    monkeypatch.setitem(sys.modules, "proximadb_sdk.chunking_strategies.base", base_mod)
    monkeypatch.setitem(sys.modules, "proximadb_sdk.chunking_strategies.semantic", sem_mod)

    p = TextDocumentProcessor(ProcessorConfig(chunk_size=64, chunk_overlap=8))
    chunker = p._get_chunker()
    assert isinstance(chunker, SemanticStrategy)
    assert p._get_chunker() is chunker


# ---------------------------------------------------------------------------
# DocumentProcessor.process (async end-to-end)
# ---------------------------------------------------------------------------


def test_process_without_adapter():
    p = TextDocumentProcessor()
    p._chunker = FakeChunker(chunks=[_make_chunk("c", "body", 0, 4, {})])
    res = asyncio.run(p.process("body", "s1"))
    assert res.success is True
    assert res.chunk_count == 1
    assert res.vector_count == 0
    assert res.document_type == DocumentType.TEXT
    assert res.metrics["processor"] == "text"
    assert res.metrics["content_length"] == 4
    # metadata got enriched on the chunk
    assert res.chunks[0].metadata["processor"] == "text"


def test_process_with_adapter_creates_vectors():
    p = TextDocumentProcessor()
    p._chunker = FakeChunker(
        chunks=[_make_chunk("c1", "one", 0, 3, {}), _make_chunk("c2", "two", 3, 6, {})]
    )
    adapter = EmbeddingProviderAdapter(AsyncProvider(dim=3))
    res = asyncio.run(p.process("onetwo", "s2", embedding_adapter=adapter))
    assert res.success is True
    assert res.vector_count == 2
    assert all(isinstance(v, VectorRecord) for v in res.vectors)
    assert res.vectors[0].source_id == "s2"
    assert len(res.vectors[0].vector) == 3


def test_process_embedding_failure_recorded():
    p = TextDocumentProcessor()
    p._chunker = FakeChunker(chunks=[_make_chunk("c", "body", 0, 4, {})])

    class BadAdapter:
        async def embed_texts_async(self, texts):
            raise RuntimeError("embed kaboom")

    res = asyncio.run(p.process("body", "s3", embedding_adapter=BadAdapter()))
    assert res.success is False
    assert res.vector_count == 0
    assert res.errors[0]["stage"] == "embedding"
    assert "embed kaboom" in res.errors[0]["error"]
    # chunks still present despite embedding failure
    assert res.chunk_count == 1


def test_process_chunking_failure_recorded():
    p = TextDocumentProcessor()

    class ExplodingChunker:
        def chunk(self, content, source_id, metadata=None):
            raise ValueError("chunk kaboom")

    p._chunker = ExplodingChunker()
    res = asyncio.run(p.process("body", "s4"))
    assert res.success is False
    assert res.document_type == DocumentType.UNKNOWN
    assert res.errors[0]["stage"] == "processing"
    assert "chunk kaboom" in res.errors[0]["error"]
    assert "processing_time_sec" in res.metrics


def test_process_no_chunks_skips_embedding():
    p = TextDocumentProcessor()
    p._chunker = FakeChunker(chunks=[])
    adapter = EmbeddingProviderAdapter(AsyncProvider())
    res = asyncio.run(p.process("body", "s5", embedding_adapter=adapter))
    assert res.success is True
    assert res.chunk_count == 0
    assert res.vector_count == 0


def test_base_prepare_for_embedding_and_enrich():
    # Exercise the base-class (non-overridden) methods via TextDocumentProcessor.
    p = TextDocumentProcessor()
    chunk = ProcessedChunk(chunk_id="c", text="t", start_pos=1, end_pos=2, metadata={"a": 1})
    assert DocumentProcessor.prepare_for_embedding(p, chunk) == "t"
    md = DocumentProcessor.enrich_metadata(p, chunk)
    assert md["a"] == 1
    assert md["processor"] == "text"
    assert "source" not in md


def test_supported_types_empty_yields_unknown_doctype():
    # A processor whose supported_types is empty falls back to UNKNOWN.
    class EmptyProc(DocumentProcessor):
        @property
        def supported_types(self):
            return []

        @property
        def name(self):
            return "empty"

        def can_process(self, content, file_path=None):
            return True

        def chunk(self, content, source_id, metadata=None):
            return [ProcessedChunk("c", "x", 0, 1, {})]

    proc = EmptyProc()
    res = asyncio.run(proc.process("x", "s"))
    assert res.document_type == DocumentType.UNKNOWN
    assert res.success is True


# ---------------------------------------------------------------------------
# Registry
# ---------------------------------------------------------------------------


def test_registry_is_singleton():
    assert DocumentProcessorRegistry() is DocumentProcessorRegistry()
    assert get_processor_registry() is DocumentProcessorRegistry()


def test_registry_default_processors_and_lookup():
    reg = get_processor_registry()
    names = reg.list_processors()
    assert "code" in names
    assert "text" in names
    assert reg.get("code").name == "code"
    assert reg.get("missing") is None


def test_registry_get_for_type():
    reg = get_processor_registry()
    assert reg.get_for_type(DocumentType.CODE).name == "code"
    assert reg.get_for_type(DocumentType.TEXT).name == "text"
    assert reg.get_for_type(DocumentType.BINARY) is None


def test_registry_detect_and_get():
    reg = get_processor_registry()
    code_proc = reg.detect_and_get("def foo(): pass", "x.py")
    assert code_proc.name == "code"
    text_proc = reg.detect_and_get("plain prose", "notes.txt")
    assert text_proc.name in ("code", "text")


def test_registry_detect_and_get_text_fallback(monkeypatch):
    # When no processor matches, detect_and_get falls back to the text processor.
    reg = get_processor_registry()
    reg._ensure_initialized()

    class NeverMatch:
        def __init__(self, name):
            self.name = name

        def can_process(self, content, file_path=None):
            return False

    fake_processors = {"a": NeverMatch("a"), "text": reg.get("text")}
    # Make even the text proc decline so the loop exhausts and hits the fallback.
    text_proc = reg.get("text")
    monkeypatch.setattr(text_proc, "can_process", lambda content, file_path=None: False)
    monkeypatch.setattr(reg, "_processors", fake_processors)

    result = reg.detect_and_get("no match anywhere", None)
    assert result is text_proc


def test_registry_register_custom():
    reg = get_processor_registry()

    class CustomProc(DocumentProcessor):
        @property
        def supported_types(self):
            return [DocumentType.HTML]

        @property
        def name(self):
            return "custom_html"

        def can_process(self, content, file_path=None):
            return False

        def chunk(self, content, source_id, metadata=None):
            return []

    reg.register(CustomProc())
    assert "custom_html" in reg.list_processors()
    assert reg.get_for_type(DocumentType.HTML).name == "custom_html"


# ---------------------------------------------------------------------------
# detect_document_type
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "path,expected",
    [
        ("a.py", DocumentType.CODE),
        ("a.rs", DocumentType.CODE),
        ("a.pdf", DocumentType.PDF),
        ("a.md", DocumentType.MARKDOWN),
        ("a.txt", DocumentType.TEXT),
        ("a.html", DocumentType.HTML),
        ("a.json", DocumentType.JSON),
        ("a.xml", DocumentType.XML),
        ("a.png", DocumentType.IMAGE),
        ("a.dll", DocumentType.BINARY),
    ],
)
def test_detect_by_extension(path, expected):
    assert detect_document_type("", path) == expected


def test_detect_by_content_code():
    assert detect_document_type("def main():\n  pass") == DocumentType.CODE


def test_detect_by_content_markdown_hash():
    assert detect_document_type("# Title\nbody") == DocumentType.MARKDOWN


def test_detect_by_content_markdown_fence():
    assert detect_document_type("text ```block``` more") == DocumentType.MARKDOWN


def test_detect_by_content_markdown_frontmatter():
    assert detect_document_type("---\ntitle: x\n---") == DocumentType.MARKDOWN


def test_detect_by_content_html():
    assert detect_document_type("<!DOCTYPE html><html></html>") == DocumentType.HTML


def test_detect_by_content_json():
    assert detect_document_type('{"a": 1}') == DocumentType.JSON


def test_detect_by_content_xml():
    assert detect_document_type("<?xml version='1.0'?><root/>") == DocumentType.XML


def test_detect_by_content_xml_bare_tag():
    assert detect_document_type("<root>data</root>") == DocumentType.XML


def test_detect_by_content_fallback_text():
    assert detect_document_type("just some plain prose here") == DocumentType.TEXT


def test_detect_empty_content():
    assert detect_document_type("") == DocumentType.TEXT


def test_detect_unknown_extension_falls_through_to_content():
    # Extension matches none of the special sets -> falls through (924->928)
    # to content-based detection.
    assert detect_document_type("def x(): pass", "weird.zzz") == DocumentType.CODE
    assert detect_document_type("plain prose only", "weird.zzz") == DocumentType.TEXT


# ---------------------------------------------------------------------------
# Factory functions
# ---------------------------------------------------------------------------


def test_create_processor_auto():
    p = create_processor("auto")
    assert p.name == "text"


def test_create_processor_named_with_config():
    cfg = ProcessorConfig(chunk_size=999)
    p = create_processor("code", cfg)
    assert p.name == "code"
    assert p.config.chunk_size == 999


def test_create_processor_named_no_config():
    # Named processor, config is None -> skips the config-assignment branch (998->1000).
    p = create_processor("code")
    assert p.name == "code"


def test_create_processor_unknown_raises():
    with pytest.raises(ValueError, match="Unknown processor type"):
        create_processor("nope")


def test_create_embedding_adapter_with_provider():
    a = create_embedding_adapter(SyncProvider(dim=5), batch_size=8)
    assert isinstance(a, EmbeddingProviderAdapter)
    assert a.batch_size == 8
    assert a.dimension == 5


def test_create_embedding_adapter_placeholder():
    a = create_embedding_adapter(use_placeholder=True, placeholder_dimension=12)
    assert a.dimension == 12
    out = a.embed_texts(["hi"])
    assert len(out[0]) == 12


def test_create_embedding_adapter_no_provider_raises():
    with pytest.raises(ValueError, match="No embedding provider"):
        create_embedding_adapter(provider=None, use_placeholder=False)
