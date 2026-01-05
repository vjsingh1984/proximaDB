"""
Unit tests for document processor and pipeline.

This module tests:
- Document type detection
- Document processors (code, text)
- Embedding provider adapter
- Document pipeline
- Batch processing
- Integration scenarios
"""

import pytest
import sys
import asyncio
import tempfile
from pathlib import Path
from unittest.mock import Mock, MagicMock, AsyncMock
from dataclasses import dataclass

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# Use the loader module to set up the module structure without triggering protobuf imports
# This imports loader.py which sets up the minimal proximadb package structure
from loader import RESOURCES_DIR  # noqa: F401 - triggers module setup

# Now we can import from the modules that were loaded by the loader
document_processor = sys.modules["proximadb.document_processor"]
document_pipeline = sys.modules["proximadb.document_pipeline"]

# Import types from document_processor
DocumentType = document_processor.DocumentType
ProcessingStrategy = document_processor.ProcessingStrategy
ProcessedChunk = document_processor.ProcessedChunk
VectorRecord = document_processor.VectorRecord
ProcessingResult = document_processor.ProcessingResult
ProcessorConfig = document_processor.ProcessorConfig
EmbeddingProviderAdapter = document_processor.EmbeddingProviderAdapter
PlaceholderEmbeddingProvider = document_processor.PlaceholderEmbeddingProvider
DocumentProcessor = document_processor.DocumentProcessor
CodeDocumentProcessor = document_processor.CodeDocumentProcessor
TextDocumentProcessor = document_processor.TextDocumentProcessor
DocumentProcessorRegistry = document_processor.DocumentProcessorRegistry
get_processor_registry = document_processor.get_processor_registry
detect_document_type = document_processor.detect_document_type
create_processor = document_processor.create_processor
create_embedding_adapter = document_processor.create_embedding_adapter

# Import types from document_pipeline
PipelineMode = document_pipeline.PipelineMode
ErrorStrategy = document_pipeline.ErrorStrategy
PipelineConfig = document_pipeline.PipelineConfig
PipelineMetrics = document_pipeline.PipelineMetrics
BatchResult = document_pipeline.BatchResult
ProgressTracker = document_pipeline.ProgressTracker
DocumentPipeline = document_pipeline.DocumentPipeline
create_document_pipeline = document_pipeline.create_document_pipeline
create_code_pipeline = document_pipeline.create_code_pipeline
pipeline_context = document_pipeline.pipeline_context
async_pipeline_context = document_pipeline.async_pipeline_context


class TestDocumentType:
    """Test DocumentType enum."""

    def test_all_types_exist(self):
        """Test all document types exist."""
        assert DocumentType.CODE
        assert DocumentType.MARKDOWN
        assert DocumentType.TEXT
        assert DocumentType.PDF
        assert DocumentType.IMAGE
        assert DocumentType.BINARY
        assert DocumentType.UNKNOWN


class TestDetectDocumentType:
    """Test document type detection."""

    def test_detect_python(self):
        """Test detecting Python code."""
        assert detect_document_type("", "test.py") == DocumentType.CODE
        assert detect_document_type("def foo():\n    pass", None) == DocumentType.CODE

    def test_detect_rust(self):
        """Test detecting Rust code."""
        assert detect_document_type("", "main.rs") == DocumentType.CODE
        assert detect_document_type("fn main() {}", None) == DocumentType.CODE

    def test_detect_javascript(self):
        """Test detecting JavaScript."""
        assert detect_document_type("", "app.js") == DocumentType.CODE
        assert detect_document_type("function test() {}", None) == DocumentType.CODE

    def test_detect_markdown(self):
        """Test detecting Markdown."""
        assert detect_document_type("", "README.md") == DocumentType.MARKDOWN
        assert (
            detect_document_type("# Header\n\nContent", None) == DocumentType.MARKDOWN
        )

    def test_detect_text(self):
        """Test detecting plain text."""
        assert detect_document_type("", "notes.txt") == DocumentType.TEXT
        assert (
            detect_document_type("Just plain text content", None) == DocumentType.TEXT
        )

    def test_detect_pdf(self):
        """Test detecting PDF."""
        assert detect_document_type("", "document.pdf") == DocumentType.PDF

    def test_detect_image(self):
        """Test detecting images."""
        assert detect_document_type("", "photo.png") == DocumentType.IMAGE
        assert detect_document_type("", "image.jpg") == DocumentType.IMAGE

    def test_detect_binary(self):
        """Test detecting binary files."""
        assert detect_document_type("", "program.exe") == DocumentType.BINARY
        assert detect_document_type("", "library.dll") == DocumentType.BINARY


class TestProcessedChunk:
    """Test ProcessedChunk dataclass."""

    def test_chunk_creation(self):
        """Test creating a processed chunk."""
        chunk = ProcessedChunk(
            chunk_id="c1", text="Hello, World!", start_pos=0, end_pos=13
        )
        assert chunk.chunk_id == "c1"
        assert chunk.text == "Hello, World!"
        assert chunk.embedding_text == "Hello, World!"

    def test_chunk_with_embedding_text(self):
        """Test chunk with custom embedding text."""
        chunk = ProcessedChunk(
            chunk_id="c1",
            text="def foo(): pass",
            start_pos=0,
            end_pos=15,
            embedding_text="Function: foo\nCode: def foo(): pass",
        )
        assert chunk.embedding_text != chunk.text


class TestVectorRecord:
    """Test VectorRecord dataclass."""

    def test_record_creation(self):
        """Test creating a vector record."""
        record = VectorRecord(
            id="r1",
            vector=[0.1, 0.2, 0.3],
            metadata={"type": "test"},
            text="Hello",
            source_id="doc1",
        )
        assert record.id == "r1"
        assert len(record.vector) == 3

    def test_to_dict(self):
        """Test converting record to dict."""
        record = VectorRecord(
            id="r1",
            vector=[0.1],
            metadata={"key": "value"},
            text="Hello",
            source_id="doc1",
        )
        d = record.to_dict()
        assert d["id"] == "r1"
        assert d["vector"] == [0.1]
        assert d["metadata"]["text"] == "Hello"
        assert d["metadata"]["source_id"] == "doc1"


class TestProcessorConfig:
    """Test ProcessorConfig dataclass."""

    def test_default_config(self):
        """Test default configuration."""
        config = ProcessorConfig()
        assert config.chunk_size == 512
        assert config.chunk_overlap == 50
        assert config.embedding_batch_size == 32
        assert config.strategy == ProcessingStrategy.BALANCED

    def test_custom_config(self):
        """Test custom configuration."""
        config = ProcessorConfig(
            chunk_size=1024, chunk_overlap=100, extract_symbols=False
        )
        assert config.chunk_size == 1024
        assert config.extract_symbols is False


class TestPlaceholderEmbeddingProvider:
    """Test PlaceholderEmbeddingProvider."""

    def test_provider_creation(self):
        """Test creating provider."""
        provider = PlaceholderEmbeddingProvider(dimension=128)
        assert provider.dimension == 128

    def test_embed_texts(self):
        """Test embedding texts."""
        provider = PlaceholderEmbeddingProvider(dimension=64)
        embeddings = provider.embed_texts(["hello", "world"])

        assert len(embeddings) == 2
        assert len(embeddings[0]) == 64
        assert len(embeddings[1]) == 64

    def test_deterministic_embeddings(self):
        """Test embeddings are deterministic."""
        provider = PlaceholderEmbeddingProvider(dimension=32)
        emb1 = provider.embed_texts(["test"])
        emb2 = provider.embed_texts(["test"])

        assert emb1 == emb2

    @pytest.mark.asyncio
    async def test_async_embed(self):
        """Test async embedding."""
        provider = PlaceholderEmbeddingProvider(dimension=32)
        embeddings = await provider.embed_texts_async(["hello"])

        assert len(embeddings) == 1
        assert len(embeddings[0]) == 32


class TestEmbeddingProviderAdapter:
    """Test EmbeddingProviderAdapter."""

    def test_adapter_creation(self):
        """Test creating adapter."""
        mock_provider = Mock()
        mock_provider.dimension = 128
        mock_provider.embed_texts = Mock(return_value=[[0.1] * 128])

        adapter = EmbeddingProviderAdapter(mock_provider, batch_size=16)
        assert adapter.batch_size == 16
        assert adapter.dimension == 128

    def test_sync_embedding(self):
        """Test synchronous embedding."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1, 0.2]])

        adapter = EmbeddingProviderAdapter(mock_provider, batch_size=2)
        embeddings = adapter.embed_texts(["text1"])

        assert len(embeddings) == 1
        mock_provider.embed_texts.assert_called_once()

    def test_batch_processing(self):
        """Test batch processing."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(side_effect=[[[0.1], [0.2]], [[0.3]]])

        adapter = EmbeddingProviderAdapter(mock_provider, batch_size=2)
        embeddings = adapter.embed_texts(["t1", "t2", "t3"])

        assert len(embeddings) == 3
        assert mock_provider.embed_texts.call_count == 2

    @pytest.mark.asyncio
    async def test_async_embedding(self):
        """Test async embedding with sync provider."""
        # Create a mock that only has embed_texts (no embed_texts_async)
        # Use spec to ensure only the attributes we define are available
        mock_provider = Mock(spec=["embed_texts"])
        mock_provider.embed_texts = Mock(return_value=[[0.1]])

        adapter = EmbeddingProviderAdapter(mock_provider, batch_size=2)
        embeddings = await adapter.embed_texts_async(["text1"])

        assert len(embeddings) == 1

    def test_retry_on_failure(self):
        """Test retry on failure."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(
            side_effect=[Exception("Temporary error"), [[0.1]]]
        )

        adapter = EmbeddingProviderAdapter(
            mock_provider, max_retries=3, retry_delay=0.01
        )
        embeddings = adapter.embed_texts(["text1"])

        assert len(embeddings) == 1
        assert mock_provider.embed_texts.call_count == 2

    def test_stats(self):
        """Test adapter stats."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1]])

        adapter = EmbeddingProviderAdapter(mock_provider, batch_size=2)
        adapter.embed_texts(["t1", "t2", "t3"])

        stats = adapter.stats
        assert stats["request_count"] == 2
        assert stats["total_texts"] == 3


class TestTextDocumentProcessor:
    """Test TextDocumentProcessor."""

    def test_processor_creation(self):
        """Test creating processor."""
        processor = TextDocumentProcessor()
        assert processor.name == "text"
        assert DocumentType.TEXT in processor.supported_types

    def test_can_process_text(self):
        """Test can_process for text files."""
        processor = TextDocumentProcessor()
        assert processor.can_process("plain text", "doc.txt")
        assert processor.can_process("plain text", "doc.md")

    def test_chunk_text(self):
        """Test chunking text."""
        processor = TextDocumentProcessor()
        content = "This is a test. " * 50  # Make it long enough

        chunks = processor.chunk(content, "test_doc")
        assert len(chunks) > 0
        assert all(isinstance(c, ProcessedChunk) for c in chunks)

    @pytest.mark.asyncio
    async def test_process_text(self):
        """Test full processing."""
        processor = TextDocumentProcessor()
        provider = PlaceholderEmbeddingProvider(dimension=64)
        adapter = EmbeddingProviderAdapter(provider)

        content = "This is test content. " * 20
        result = await processor.process(
            content=content, source_id="test.txt", embedding_adapter=adapter
        )

        assert result.success
        assert result.chunk_count > 0
        assert result.vector_count > 0


class TestCodeDocumentProcessor:
    """Test CodeDocumentProcessor."""

    def test_processor_creation(self):
        """Test creating processor."""
        processor = CodeDocumentProcessor()
        assert processor.name == "code"
        assert DocumentType.CODE in processor.supported_types

    def test_can_process_python(self):
        """Test can_process for Python."""
        processor = CodeDocumentProcessor()
        assert processor.can_process("def foo(): pass", "test.py")

    def test_can_process_rust(self):
        """Test can_process for Rust."""
        processor = CodeDocumentProcessor()
        assert processor.can_process("fn main() {}", "main.rs")

    def test_chunk_code(self):
        """Test chunking code."""
        processor = CodeDocumentProcessor()
        content = '''
def hello():
    """Say hello."""
    print("Hello, World!")

def goodbye():
    """Say goodbye."""
    print("Goodbye!")
'''
        chunks = processor.chunk(content, "test.py")
        assert len(chunks) > 0

    @pytest.mark.asyncio
    async def test_process_code(self):
        """Test full code processing."""
        processor = CodeDocumentProcessor()
        provider = PlaceholderEmbeddingProvider(dimension=64)
        adapter = EmbeddingProviderAdapter(provider)

        content = '''
def calculate(x, y):
    """Calculate sum of two numbers."""
    return x + y

class Calculator:
    """A simple calculator."""

    def add(self, a, b):
        return a + b
'''
        result = await processor.process(
            content=content, source_id="calc.py", embedding_adapter=adapter
        )

        assert result.success
        assert result.document_type == DocumentType.CODE


class TestDocumentProcessorRegistry:
    """Test DocumentProcessorRegistry."""

    def test_registry_singleton(self):
        """Test registry is singleton."""
        reg1 = get_processor_registry()
        reg2 = get_processor_registry()
        assert reg1 is reg2

    def test_get_processor(self):
        """Test getting processor by name."""
        registry = get_processor_registry()
        processor = registry.get("text")
        assert processor is not None
        assert processor.name == "text"

    def test_get_for_type(self):
        """Test getting processor for type."""
        registry = get_processor_registry()
        processor = registry.get_for_type(DocumentType.CODE)
        assert processor is not None

    def test_detect_and_get(self):
        """Test auto-detection."""
        registry = get_processor_registry()
        processor = registry.detect_and_get("def foo(): pass", "test.py")
        assert processor.name == "code"

    def test_list_processors(self):
        """Test listing processors."""
        registry = get_processor_registry()
        names = registry.list_processors()
        assert "text" in names
        assert "code" in names


class TestPipelineConfig:
    """Test PipelineConfig."""

    def test_default_config(self):
        """Test default configuration."""
        config = PipelineConfig()
        assert config.mode == PipelineMode.EMBED
        assert config.embedding_batch_size == 32
        assert config.error_strategy == ErrorStrategy.COLLECT

    def test_custom_config(self):
        """Test custom configuration."""
        config = PipelineConfig(
            mode=PipelineMode.STORE, max_concurrent=8, use_placeholder_embeddings=True
        )
        assert config.mode == PipelineMode.STORE
        assert config.max_concurrent == 8


class TestPipelineMetrics:
    """Test PipelineMetrics."""

    def test_metrics_creation(self):
        """Test creating metrics."""
        metrics = PipelineMetrics()
        assert metrics.total_documents == 0
        assert metrics.success_rate == 0.0

    def test_success_rate(self):
        """Test success rate calculation."""
        metrics = PipelineMetrics(
            total_documents=10, processed_documents=8, failed_documents=2
        )
        assert metrics.success_rate == 0.8

    def test_to_dict(self):
        """Test converting to dict."""
        metrics = PipelineMetrics(total_chunks=100)
        d = metrics.to_dict()
        assert "total_chunks" in d
        assert d["total_chunks"] == 100


class TestProgressTracker:
    """Test ProgressTracker."""

    def test_tracker_creation(self):
        """Test creating tracker."""
        tracker = ProgressTracker()
        assert tracker.elapsed_time == 0.0

    def test_tracker_with_callback(self):
        """Test tracker with callback."""
        updates = []

        def callback(current, total, status):
            updates.append((current, total, status))

        tracker = ProgressTracker(callback=callback)
        tracker.start(10, "starting")
        tracker.update(5)
        tracker.complete("done")

        assert len(updates) == 3
        assert updates[-1][0] == 10  # current == total after complete


class TestDocumentPipeline:
    """Test DocumentPipeline."""

    def test_pipeline_creation(self):
        """Test creating pipeline."""
        pipeline = DocumentPipeline()
        assert pipeline.config is not None

    def test_pipeline_with_placeholder(self):
        """Test pipeline with placeholder embeddings."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)
        assert pipeline.embedding_adapter is not None

    @pytest.mark.asyncio
    async def test_process_text(self):
        """Test processing text."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        content = "This is a test document. " * 20
        result = await pipeline.process(content, "test.txt")

        assert result.success
        assert result.chunk_count > 0

    @pytest.mark.asyncio
    async def test_process_code(self):
        """Test processing code."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        content = '''
def hello():
    """A greeting function."""
    return "Hello!"
'''
        result = await pipeline.process(content, "hello.py")

        assert result.success
        assert result.document_type == DocumentType.CODE

    @pytest.mark.asyncio
    async def test_process_batch(self):
        """Test batch processing."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        documents = [
            {"content": "Text document one. " * 10, "source_id": "doc1.txt"},
            {"content": "Text document two. " * 10, "source_id": "doc2.txt"},
            {"content": "def foo(): pass", "source_id": "code.py"},
        ]

        result = await pipeline.process_batch(documents)

        assert result.success_count >= 2
        assert len(result.results) == 3

    @pytest.mark.asyncio
    async def test_process_file(self):
        """Test processing a file."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as f:
            f.write("This is test file content. " * 20)
            f.flush()

            result = await pipeline.process_file(f.name)
            Path(f.name).unlink()

        assert result.success
        assert result.chunk_count > 0

    @pytest.mark.asyncio
    async def test_process_directory(self):
        """Test processing a directory."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        with tempfile.TemporaryDirectory() as tmpdir:
            # Create test files
            (Path(tmpdir) / "file1.txt").write_text("Content one. " * 20)
            (Path(tmpdir) / "file2.txt").write_text("Content two. " * 20)

            result = await pipeline.process_directory(tmpdir, pattern="*.txt")

        assert result.success_count == 2

    def test_builder_pattern(self):
        """Test builder pattern."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1]])

        pipeline = (
            DocumentPipeline()
            .with_embedding_provider(mock_provider)
            .with_progress_callback(lambda c, t, s: None)
        )

        assert pipeline.embedding_adapter is not None

    @pytest.mark.asyncio
    async def test_process_stream(self):
        """Test streaming processing."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        content = "This is streaming content. " * 20
        chunks = []
        async for chunk in pipeline.process_stream(content, "stream.txt"):
            chunks.append(chunk)

        assert len(chunks) > 0

    def test_get_metrics(self):
        """Test getting metrics."""
        pipeline = DocumentPipeline()
        metrics = pipeline.get_metrics()

        assert "total_documents" in metrics
        assert "success_rate" in metrics


class TestFactoryFunctions:
    """Test factory functions."""

    def test_create_document_pipeline(self):
        """Test create_document_pipeline."""
        pipeline = create_document_pipeline(mode=PipelineMode.PROCESS_ONLY)
        assert pipeline.config.mode == PipelineMode.PROCESS_ONLY

    def test_create_code_pipeline(self):
        """Test create_code_pipeline."""
        pipeline = create_code_pipeline()
        # Should have code-optimized defaults
        assert pipeline.config.processor_config.extract_symbols is True

    def test_create_processor(self):
        """Test create_processor."""
        processor = create_processor("text")
        assert processor.name == "text"

    def test_create_embedding_adapter(self):
        """Test create_embedding_adapter."""
        adapter = create_embedding_adapter(
            None, use_placeholder=True, placeholder_dimension=64
        )
        assert adapter.dimension == 64


class TestContextManagers:
    """Test context managers."""

    def test_pipeline_context(self):
        """Test sync pipeline context."""
        with pipeline_context(use_placeholder_embeddings=True) as pipeline:
            assert isinstance(pipeline, DocumentPipeline)

    @pytest.mark.asyncio
    async def test_async_pipeline_context(self):
        """Test async pipeline context."""
        async with async_pipeline_context(use_placeholder_embeddings=True) as pipeline:
            assert isinstance(pipeline, DocumentPipeline)

            result = await pipeline.process("Test content. " * 20, "test.txt")
            assert result.success


class TestIntegrationScenarios:
    """Integration tests for complete workflows."""

    @pytest.mark.asyncio
    async def test_code_to_vectors_workflow(self):
        """Test complete code to vectors workflow."""
        config = PipelineConfig(
            use_placeholder_embeddings=True, mode=PipelineMode.EMBED
        )
        pipeline = DocumentPipeline(config=config)

        code = '''
class DataProcessor:
    """Process data from various sources."""

    def __init__(self, source):
        self.source = source

    def process(self, data):
        """Process the data."""
        return self.transform(data)

    def transform(self, data):
        """Transform the data."""
        return data.upper()
'''
        result = await pipeline.process(code, "processor.py")

        assert result.success
        assert result.document_type == DocumentType.CODE
        assert result.chunk_count > 0
        assert result.vector_count > 0

        # Check vectors have proper structure
        for vector in result.vectors:
            assert len(vector.vector) > 0
            assert "processor" in vector.metadata

    @pytest.mark.asyncio
    async def test_mixed_batch_processing(self):
        """Test batch with mixed document types."""
        config = PipelineConfig(use_placeholder_embeddings=True)
        pipeline = DocumentPipeline(config=config)

        documents = [
            {"content": "def hello(): print('Hello!')", "source_id": "hello.py"},
            {
                "content": "# Documentation\n\nThis is a guide. " * 10,
                "source_id": "guide.md",
            },
            {"content": "Plain text content here. " * 10, "source_id": "notes.txt"},
        ]

        result = await pipeline.process_batch(documents)

        # All should succeed
        assert result.success_count == 3

        # Check document types detected correctly
        types = [r.document_type for r in result.results]
        assert DocumentType.CODE in types

    @pytest.mark.asyncio
    async def test_with_mock_vector_store(self):
        """Test with mock vector store."""
        # Create mock vector store
        mock_store = AsyncMock()
        mock_store.insert = AsyncMock(return_value=None)

        config = PipelineConfig(
            use_placeholder_embeddings=True, mode=PipelineMode.STORE
        )
        pipeline = DocumentPipeline(config=config, vector_store=mock_store)

        content = "Store this content. " * 20
        result = await pipeline.process_and_store(content, "stored.txt")

        assert result.success
        mock_store.insert.assert_called_once()

        # Check records were passed
        call_args = mock_store.insert.call_args[0][0]
        assert len(call_args) > 0


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
