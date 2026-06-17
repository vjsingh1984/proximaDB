import pytest

from proximadb_sdk.chunking_strategies.base import TextChunk
from proximadb_sdk.document_pipeline import (
    BatchResult,
    DocumentPipeline,
    ErrorStrategy,
    PipelineConfig,
    PipelineMetrics,
    PipelineMode,
    ProgressTracker,
    async_pipeline_context,
    create_code_pipeline,
    create_document_pipeline,
    pipeline_context,
)
from proximadb_sdk.document_processor import (
    CodeDocumentProcessor,
    DocumentProcessor,
    DocumentProcessorRegistry,
    DocumentType,
    EmbeddingProviderAdapter,
    PlaceholderEmbeddingProvider,
    ProcessedChunk,
    ProcessingResult,
    ProcessingStrategy,
    ProcessorConfig,
    TextDocumentProcessor,
    VectorRecord,
    create_embedding_adapter,
    create_processor,
    detect_document_type,
)


class SyncEmbeddingProvider:
    dimension = 3

    def __init__(self):
        self.calls = []

    def embed_texts(self, texts):
        self.calls.append(list(texts))
        return [[float(len(text)), 1.0, 0.0] for text in texts]


class AsyncEmbeddingProvider:
    dimension = 2

    async def embed_texts_async(self, texts):
        return [[1.0, float(i)] for i, _ in enumerate(texts)]


class FailingEmbeddingProvider:
    dimension = 1

    def embed_texts(self, texts):
        raise RuntimeError("boom")


class SimpleProcessor(DocumentProcessor):
    @property
    def supported_types(self):
        return [DocumentType.TEXT]

    @property
    def name(self):
        return "simple"

    def can_process(self, content, file_path=None):
        return True

    def chunk(self, content, source_id, metadata=None):
        return [
            ProcessedChunk(
                chunk_id=f"{source_id}_chunk_0",
                text=content,
                start_pos=0,
                end_pos=len(content),
                metadata={"kind": "simple"},
            )
        ]


class FailingProcessor(SimpleProcessor):
    @property
    def name(self):
        return "failing"

    def chunk(self, content, source_id, metadata=None):
        raise RuntimeError("chunk failed")


class FakeRegistry:
    def __init__(self, processor):
        self.processor = processor

    def get(self, name):
        return self.processor if name in {self.processor.name, "text"} else None

    def get_for_type(self, doc_type):
        return self.processor

    def detect_and_get(self, content, source_id):
        return self.processor


class FakeVectorStore:
    def __init__(self, fail=False):
        self.fail = fail
        self.records = []

    async def insert(self, records):
        if self.fail:
            raise RuntimeError("store failed")
        self.records.extend(records)


class FakeChunker:
    def chunk(self, content, source_id, metadata=None):
        return [
            TextChunk(
                text="def fn(): pass",
                start_pos=0,
                end_pos=14,
                chunk_id=f"{source_id}_code_0",
                metadata={
                    "fully_qualified_name": "pkg.fn",
                    "symbol_type": "function",
                    "documentation": "docs",
                    "signature": "fn()",
                    "language": "python",
                },
            )
        ]


def test_document_processor_data_structures_and_placeholder_embeddings():
    chunk = ProcessedChunk("chunk-1", "hello", 0, 5)
    assert chunk.embedding_text == "hello"

    record = VectorRecord(
        id="chunk-1",
        vector=[0.1, 0.2],
        metadata={"kind": "text"},
        text="hello",
        source_id="doc",
    )
    assert record.to_dict() == {
        "id": "chunk-1",
        "vector": [0.1, 0.2],
        "metadata": {"kind": "text", "text": "hello", "source_id": "doc"},
    }

    result = ProcessingResult(
        success=True,
        source_id="doc",
        document_type=DocumentType.TEXT,
        chunks=[chunk],
        vectors=[record],
    )
    assert result.chunk_count == 1
    assert result.vector_count == 1

    provider = PlaceholderEmbeddingProvider(dimension=4)
    first = provider.embed_texts(["same"])[0]
    second = provider.embed_texts(["same"])[0]
    assert provider.dimension == 4
    assert first == second
    assert len(first) == 4


@pytest.mark.asyncio
async def test_embedding_adapter_sync_async_empty_and_failure_paths():
    sync_provider = SyncEmbeddingProvider()
    adapter = EmbeddingProviderAdapter(sync_provider, batch_size=2)

    assert adapter.dimension == 3
    assert adapter.embed_texts([]) == []
    assert adapter.embed_texts(["a", "bb", "ccc"]) == [
        [1.0, 1.0, 0.0],
        [2.0, 1.0, 0.0],
        [3.0, 1.0, 0.0],
    ]
    assert adapter.stats["request_count"] == 2
    assert adapter.stats["total_texts"] == 3
    assert adapter.stats["is_async"] is False

    async_adapter = EmbeddingProviderAdapter(AsyncEmbeddingProvider(), batch_size=10)
    assert await async_adapter.embed_texts_async([]) == []
    assert await async_adapter.embed_texts_async(["x", "y"]) == [[1.0, 0.0], [1.0, 1.0]]
    assert async_adapter.stats["is_async"] is True

    failing = EmbeddingProviderAdapter(
        FailingEmbeddingProvider(), max_retries=1, retry_delay=0
    )
    with pytest.raises(RuntimeError, match="Embedding failed"):
        failing.embed_texts(["x"])
    with pytest.raises(RuntimeError, match="Async embedding failed"):
        await failing.embed_texts_async(["x"])

    with pytest.raises(ValueError):
        create_embedding_adapter(None)
    placeholder = create_embedding_adapter(
        None, use_placeholder=True, placeholder_dimension=2
    )
    assert placeholder.dimension == 2


@pytest.mark.asyncio
async def test_text_and_code_processors_process_and_enrich_metadata():
    processor = SimpleProcessor(ProcessorConfig(strategy=ProcessingStrategy.FAST))
    adapter = EmbeddingProviderAdapter(SyncEmbeddingProvider())

    result = await processor.process(
        "hello",
        "doc",
        embedding_adapter=adapter,
        metadata={"tenant": "acme"},
    )
    assert result.success is True
    assert result.document_type == DocumentType.TEXT
    assert result.vector_count == 1
    assert result.chunks[0].metadata["processor"] == "simple"
    assert result.chunks[0].metadata["source"] == {"tenant": "acme"}
    assert result.metrics["processor"] == "simple"

    failed = await FailingProcessor().process("bad", "doc")
    assert failed.success is False
    assert failed.document_type == DocumentType.UNKNOWN
    assert failed.errors[0]["stage"] == "processing"

    text_processor = TextDocumentProcessor(
        ProcessorConfig(chunk_size=40, min_chunk_size=1)
    )
    assert text_processor.can_process("text", "README.md") is True
    assert text_processor.can_process("text", "image.png") is False
    text_chunks = text_processor.chunk("# Title\n\n" + "Body sentence. " * 10, "doc")
    assert text_chunks

    code_processor = CodeDocumentProcessor()
    code_processor._chunker = FakeChunker()
    assert code_processor.can_process("", "main.py") is True
    assert code_processor.can_process("def fn(): pass") is True
    assert code_processor.can_process("plain text") is False
    code_chunks = code_processor.chunk("def fn(): pass", "main.py")
    assert code_chunks[0].embedding_text.startswith("Symbol: pkg.fn")
    assert "Documentation: docs" in code_chunks[0].embedding_text
    metadata = code_processor.enrich_metadata(code_chunks[0])
    assert metadata["is_code"] is True
    assert metadata["language"] == "python"


def test_registry_detection_and_factory_functions():
    registry = DocumentProcessorRegistry()
    registry._ensure_initialized()
    old_processors = registry._processors.copy()
    old_type_mapping = registry._type_mapping.copy()
    old_initialized = registry._initialized

    try:
        custom = SimpleProcessor()
        registry.register(custom)

        assert registry.get("simple") is custom
        assert registry.get_for_type(DocumentType.TEXT) is custom
        assert registry.detect_and_get("anything", "doc.bin") is custom
        assert "simple" in registry.list_processors()

        assert isinstance(create_processor("auto"), DocumentProcessor)
        configured = create_processor("text", ProcessorConfig(chunk_size=64))
        assert configured.config.chunk_size == 64
        with pytest.raises(ValueError):
            create_processor("missing")
    finally:
        registry._processors = old_processors
        registry._type_mapping = old_type_mapping
        registry._initialized = old_initialized

    assert detect_document_type("", "main.py") == DocumentType.CODE
    assert detect_document_type("", "paper.pdf") == DocumentType.PDF
    assert detect_document_type("", "README.md") == DocumentType.MARKDOWN
    assert detect_document_type("", "doc.txt") == DocumentType.TEXT
    assert detect_document_type("", "index.html") == DocumentType.HTML
    assert detect_document_type("", "data.json") == DocumentType.JSON
    assert detect_document_type("", "feed.xml") == DocumentType.XML
    assert detect_document_type("", "image.png") == DocumentType.IMAGE
    assert detect_document_type("", "app.exe") == DocumentType.BINARY
    assert detect_document_type("def fn(): pass") == DocumentType.CODE
    assert detect_document_type("# heading") == DocumentType.MARKDOWN
    assert detect_document_type("<html></html>") == DocumentType.HTML
    assert detect_document_type('{"a": 1}') == DocumentType.JSON
    assert detect_document_type("<?xml version='1.0'?>") == DocumentType.XML
    assert detect_document_type("ordinary prose") == DocumentType.TEXT


def test_pipeline_config_metrics_batch_and_progress_helpers():
    config = PipelineConfig(processor_config=None)
    assert config.processor_config is not None
    assert config.mode == PipelineMode.EMBED
    assert ErrorStrategy.COLLECT.value == "collect"

    metrics = PipelineMetrics(total_documents=4, processed_documents=3)
    assert metrics.success_rate == 0.75
    assert metrics.to_dict()["error_count"] == 0
    assert PipelineMetrics().success_rate == 0.0

    success = ProcessingResult(True, "ok", DocumentType.TEXT, vectors=[])
    failure = ProcessingResult(False, "bad", DocumentType.TEXT)
    batch = BatchResult(results=[success, failure])
    assert batch.success_count == 1
    assert batch.failure_count == 1
    assert batch.get_vectors() == []

    events = []
    tracker = ProgressTracker(
        lambda current, total, status: events.append((current, total, status))
    )
    assert tracker.elapsed_time == 0.0
    tracker.start(2, "start")
    tracker.update(status="middle")
    tracker.complete("done")
    assert events == [(0, 2, "start"), (1, 2, "middle"), (2, 2, "done")]
    assert tracker.elapsed_time >= 0.0


@pytest.mark.asyncio
async def test_document_pipeline_process_store_batch_file_directory_and_stream(
    tmp_path,
):
    processor = SimpleProcessor()
    store = FakeVectorStore()
    progress_events = []
    pipeline = DocumentPipeline(
        embedding_provider=SyncEmbeddingProvider(),
        vector_store=store,
        config=PipelineConfig(mode=PipelineMode.STORE, max_concurrent=2),
    )
    pipeline._registry = FakeRegistry(processor)
    pipeline.with_progress_callback(
        lambda current, total, status: progress_events.append((current, total, status))
    )

    assert pipeline.with_embedding_provider(SyncEmbeddingProvider()) is pipeline
    assert pipeline.with_vector_store(store) is pipeline

    result = await pipeline.process("hello", "doc", metadata={"tenant": "acme"})
    assert result.success is True
    assert result.vector_count == 1
    assert pipeline.get_metrics()["processed_documents"] == 1

    stored = await pipeline.process_and_store("stored", "stored-doc")
    assert stored.metrics["records_stored"] == 1
    assert store.records

    batch = await pipeline.process_batch(
        [
            {"content": "a", "source_id": "a"},
            {"content": "b", "source_id": "b", "processor": "simple"},
        ],
        concurrent_limit=1,
    )
    assert batch.success_count == 2
    assert progress_events[0] == (0, 2, "batch_processing")

    batch_store = await pipeline.process_batch_and_store(
        [{"content": "c", "source_id": "c"}]
    )
    assert batch_store.success_count == 1

    file_path = tmp_path / "doc.txt"
    file_path.write_text("file text")
    file_result = await pipeline.process_file(file_path)
    assert file_result.success is True
    assert file_result.source_id == str(file_path)

    missing = await pipeline.process_file(tmp_path / "missing.txt")
    assert missing.success is False

    directory = await pipeline.process_directory(tmp_path, extensions=[".txt"])
    assert directory.success_count == 1
    not_dir = await pipeline.process_directory(tmp_path / "missing")
    assert not_dir.metrics.errors[0]["error"].startswith("Not a directory")

    streamed = [
        chunk async for chunk in pipeline.process_stream("streamed", "stream-doc")
    ]
    assert streamed[0].metadata["processor"] == "simple"

    assert pipeline._get_processor("x", "doc", processor_name="simple") is processor
    assert (
        pipeline._get_processor("x", "doc", document_type=DocumentType.TEXT)
        is processor
    )

    pipeline.reset_metrics()
    assert pipeline.get_metrics()["total_documents"] == 0

    with pytest.raises(ValueError):
        await DocumentPipeline().process_and_store("x", "doc")
    with pytest.raises(ValueError):
        await DocumentPipeline().process_batch_and_store([{"content": "x"}])

    failing_store = FakeVectorStore(fail=True)
    failing_pipeline = DocumentPipeline(
        embedding_provider=SyncEmbeddingProvider(),
        vector_store=failing_store,
        config=PipelineConfig(mode=PipelineMode.STORE),
    )
    failing_pipeline._registry = FakeRegistry(processor)
    failed_store = await failing_pipeline.process_and_store("x", "doc")
    assert failed_store.success is False
    assert failed_store.errors[0]["stage"] == "storage"


@pytest.mark.asyncio
async def test_pipeline_error_paths_factories_and_contexts():
    failing_pipeline = DocumentPipeline(
        config=PipelineConfig(mode=PipelineMode.PROCESS_ONLY)
    )
    failing_pipeline._registry = FakeRegistry(FailingProcessor())
    failed = await failing_pipeline.process("bad", "doc")
    assert failed.success is False
    assert failed.errors[0]["stage"] in {"processing", "pipeline"}

    no_auto = DocumentPipeline(
        config=PipelineConfig(auto_detect_type=False, default_processor="text")
    )
    no_auto._registry = FakeRegistry(SimpleProcessor())
    assert isinstance(no_auto._get_processor("x", "doc"), SimpleProcessor)

    created = create_document_pipeline(
        embedding_provider=SyncEmbeddingProvider(), mode=PipelineMode.PROCESS_ONLY
    )
    assert created.config.mode == PipelineMode.PROCESS_ONLY

    code_pipeline = create_code_pipeline(embedding_provider=SyncEmbeddingProvider())
    assert code_pipeline.config.default_processor == "code"
    assert code_pipeline.config.embedding_batch_size == 16

    with pipeline_context(mode=PipelineMode.PROCESS_ONLY) as pipeline:
        pipeline._metrics.total_documents = 1
    assert pipeline.get_metrics()["total_documents"] == 0

    async with async_pipeline_context(mode=PipelineMode.PROCESS_ONLY) as async_pipeline:
        async_pipeline._metrics.total_documents = 1
    assert async_pipeline.get_metrics()["total_documents"] == 0
