"""
Unit tests for the unified chunking pipeline.

This module tests:
- Pipeline configuration
- Processing modes (sync, async, streaming, batch)
- Pipeline stages (validation, enrichment, filter)
- Batch embedding
- Progress tracking
- Error handling
- Factory functions
- Context managers
"""

import sys
import tempfile
import time
from pathlib import Path
from unittest.mock import Mock

import pytest

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# Import from loader which handles the module loading

# Get pipeline module from sys.modules
pipeline_module = sys.modules["proximadb.chunking_strategies.pipeline"]

# Get references
ProcessingMode = pipeline_module.ProcessingMode
ErrorHandling = pipeline_module.ErrorHandling
PipelineConfig = pipeline_module.PipelineConfig
PipelineResult = pipeline_module.PipelineResult
BatchResult = pipeline_module.BatchResult
PipelineStage = pipeline_module.PipelineStage
ValidationStage = pipeline_module.ValidationStage
EnrichmentStage = pipeline_module.EnrichmentStage
FilterStage = pipeline_module.FilterStage
BatchEmbedder = pipeline_module.BatchEmbedder
ProgressTracker = pipeline_module.ProgressTracker
ChunkingPipeline = pipeline_module.ChunkingPipeline
create_pipeline = pipeline_module.create_pipeline
create_code_pipeline = pipeline_module.create_code_pipeline
create_document_pipeline = pipeline_module.create_document_pipeline
pipeline_context = pipeline_module.pipeline_context
async_pipeline_context = pipeline_module.async_pipeline_context

# Get base classes
base_module = sys.modules["proximadb.chunking_strategies.base"]
ChunkingStrategy = base_module.ChunkingStrategy
TextChunk = base_module.TextChunk
ChunkingConfig = base_module.ChunkingConfig


class TestProcessingMode:
    """Test ProcessingMode enum."""

    def test_all_modes_exist(self):
        """Test all processing modes exist."""
        assert ProcessingMode.SYNC
        assert ProcessingMode.ASYNC
        assert ProcessingMode.STREAMING
        assert ProcessingMode.BATCH


class TestErrorHandling:
    """Test ErrorHandling enum."""

    def test_all_modes_exist(self):
        """Test all error handling modes exist."""
        assert ErrorHandling.FAIL_FAST
        assert ErrorHandling.SKIP_ERRORS
        assert ErrorHandling.COLLECT_ERRORS
        assert ErrorHandling.RETRY


class TestPipelineConfig:
    """Test PipelineConfig dataclass."""

    def test_default_config(self):
        """Test default configuration."""
        config = PipelineConfig()
        assert config.chunking_strategy == ChunkingStrategy.SEMANTIC
        assert config.embedding_batch_size == 32
        assert config.max_concurrent_tasks == 4
        assert config.error_handling == ErrorHandling.COLLECT_ERRORS
        assert config.enable_metrics is True

    def test_custom_config(self):
        """Test custom configuration."""
        config = PipelineConfig(
            chunking_strategy=ChunkingStrategy.CODE,
            embedding_batch_size=64,
            max_concurrent_tasks=8,
            error_handling=ErrorHandling.FAIL_FAST,
        )
        assert config.chunking_strategy == ChunkingStrategy.CODE
        assert config.embedding_batch_size == 64
        assert config.max_concurrent_tasks == 8

    def test_chunking_config_auto_created(self):
        """Test that chunking_config is auto-created."""
        config = PipelineConfig()
        assert config.chunking_config is not None
        assert isinstance(config.chunking_config, ChunkingConfig)


class TestPipelineResult:
    """Test PipelineResult dataclass."""

    def test_result_creation(self):
        """Test creating pipeline result."""
        result = PipelineResult(success=True)
        assert result.success is True
        assert result.chunk_count == 0
        assert result.error_count == 0

    def test_result_with_chunks(self):
        """Test result with chunks."""
        chunks = [
            TextChunk(text="chunk 1", start_pos=0, end_pos=10, chunk_id="c1"),
            TextChunk(text="chunk 2", start_pos=10, end_pos=20, chunk_id="c2"),
        ]
        result = PipelineResult(success=True, chunks=chunks)
        assert result.chunk_count == 2

    def test_result_with_errors(self):
        """Test result with errors."""
        errors = [{"error": "test error"}]
        result = PipelineResult(success=False, errors=errors)
        assert result.error_count == 1
        assert result.success is False

    def test_to_dict(self):
        """Test converting result to dict."""
        result = PipelineResult(success=True, metrics={"processing_time_sec": 1.5})
        d = result.to_dict()
        assert "success" in d
        assert "chunk_count" in d
        assert "metrics" in d


class TestBatchResult:
    """Test BatchResult dataclass."""

    def test_result_creation(self):
        """Test creating batch result."""
        result = BatchResult()
        assert result.total_items == 0
        assert result.processed_items == 0
        assert result.success_rate == 0.0

    def test_success_rate(self):
        """Test success rate calculation."""
        result = BatchResult(total_items=10, processed_items=8)
        assert result.success_rate == 0.8


class TestProgressTracker:
    """Test ProgressTracker class."""

    def test_tracker_creation(self):
        """Test creating progress tracker."""
        tracker = ProgressTracker()
        assert tracker.elapsed_time == 0.0

    def test_tracker_with_callback(self):
        """Test tracker with callback."""
        updates = []

        def callback(current, total, status):
            updates.append((current, total, status))

        tracker = ProgressTracker(callback=callback)
        tracker.start(10, "starting")
        tracker.update(1)
        tracker.update(2)
        tracker.complete("done")

        assert len(updates) == 4
        assert updates[-1] == (10, 10, "done")

    def test_items_per_second(self):
        """Test items per second calculation."""
        tracker = ProgressTracker()
        tracker.start(100)
        time.sleep(0.1)
        tracker.update(50)
        assert tracker.items_per_second > 0


class TestValidationStage:
    """Test ValidationStage class."""

    def test_stage_creation(self):
        """Test creating validation stage."""
        config = PipelineConfig()
        stage = ValidationStage(config)
        assert stage.name == "validation"

    def test_validate_normal_chunk(self):
        """Test validating a normal chunk."""
        config = PipelineConfig()
        stage = ValidationStage(config)
        chunk = TextChunk(text="Hello, World!", start_pos=0, end_pos=13, chunk_id="c1")
        result = stage.process(chunk)
        assert result.text == "Hello, World!"

    def test_validate_empty_chunk_raises(self):
        """Test that empty chunks raise error."""
        config = PipelineConfig(validate_chunks=True)
        stage = ValidationStage(config)
        chunk = TextChunk(text="", start_pos=0, end_pos=0, chunk_id="c1")

        with pytest.raises(ValueError):
            stage.process(chunk)

    def test_truncate_long_text(self):
        """Test truncating long text."""
        config = PipelineConfig(max_text_length=10, truncate_long_texts=True)
        stage = ValidationStage(config)
        chunk = TextChunk(
            text="This is a very long text that should be truncated",
            start_pos=0,
            end_pos=50,
            chunk_id="c1",
        )
        result = stage.process(chunk)
        assert len(result.text) == 10
        assert result.metadata.get("truncated") is True

    def test_skip_validation(self):
        """Test skipping validation."""
        config = PipelineConfig(validate_chunks=False)
        stage = ValidationStage(config)
        chunk = TextChunk(text="", start_pos=0, end_pos=0, chunk_id="c1")
        result = stage.process(chunk)
        assert result.text == ""


class TestEnrichmentStage:
    """Test EnrichmentStage class."""

    def test_stage_creation(self):
        """Test creating enrichment stage."""
        stage = EnrichmentStage()
        assert stage.name == "enrichment"

    def test_add_enrichment(self):
        """Test adding enrichment function."""
        stage = EnrichmentStage()

        def add_timestamp(chunk):
            chunk.metadata["timestamp"] = "2024-01-01"
            return chunk

        stage.add_enrichment(add_timestamp)

        chunk = TextChunk(text="Hello", start_pos=0, end_pos=5, chunk_id="c1")
        result = stage.process(chunk)
        assert result.metadata.get("timestamp") == "2024-01-01"

    def test_multiple_enrichments(self):
        """Test multiple enrichment functions."""
        stage = EnrichmentStage()

        def add_a(chunk):
            chunk.metadata["a"] = 1
            return chunk

        def add_b(chunk):
            chunk.metadata["b"] = 2
            return chunk

        stage.add_enrichment(add_a)
        stage.add_enrichment(add_b)

        chunk = TextChunk(text="Hello", start_pos=0, end_pos=5, chunk_id="c1")
        result = stage.process(chunk)
        assert result.metadata.get("a") == 1
        assert result.metadata.get("b") == 2


class TestFilterStage:
    """Test FilterStage class."""

    def test_stage_creation(self):
        """Test creating filter stage."""
        stage = FilterStage()
        assert stage.name == "filter"

    def test_no_filter(self):
        """Test no filtering."""
        stage = FilterStage()
        chunks = [
            TextChunk(text="a", start_pos=0, end_pos=1, chunk_id="c1"),
            TextChunk(text="b", start_pos=1, end_pos=2, chunk_id="c2"),
        ]
        result = stage.process(chunks)
        assert len(result) == 2

    def test_filter_by_length(self):
        """Test filtering by length."""
        stage = FilterStage()
        stage.add_predicate(lambda c: len(c.text) > 5)

        chunks = [
            TextChunk(text="short", start_pos=0, end_pos=5, chunk_id="c1"),
            TextChunk(text="longer text", start_pos=0, end_pos=11, chunk_id="c2"),
        ]
        result = stage.process(chunks)
        assert len(result) == 1
        assert result[0].chunk_id == "c2"

    def test_multiple_filters(self):
        """Test multiple filter predicates."""
        stage = FilterStage()
        stage.add_predicate(lambda c: len(c.text) > 3)
        stage.add_predicate(lambda c: "x" not in c.text)

        chunks = [
            TextChunk(text="ab", start_pos=0, end_pos=2, chunk_id="c1"),  # Too short
            TextChunk(text="abcd", start_pos=0, end_pos=4, chunk_id="c2"),  # Pass
            TextChunk(
                text="abcdx", start_pos=0, end_pos=5, chunk_id="c3"
            ),  # Contains x
        ]
        result = stage.process(chunks)
        assert len(result) == 1
        assert result[0].chunk_id == "c2"


class TestBatchEmbedder:
    """Test BatchEmbedder class."""

    def test_embedder_creation(self):
        """Test creating batch embedder."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1, 0.2]])

        embedder = BatchEmbedder(mock_provider, batch_size=2)
        assert embedder.batch_size == 2

    def test_embed_batch(self):
        """Test batch embedding."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(
            side_effect=[[[0.1, 0.2], [0.3, 0.4]], [[0.5, 0.6]]]
        )

        embedder = BatchEmbedder(mock_provider, batch_size=2)
        texts = ["text1", "text2", "text3"]
        embeddings = embedder.embed_batch(texts)

        assert len(embeddings) == 3
        assert mock_provider.embed_texts.call_count == 2

    def test_embed_with_retry(self):
        """Test embedding with retry on failure."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(
            side_effect=[Exception("Temporary error"), [[0.1, 0.2]]]
        )

        embedder = BatchEmbedder(
            mock_provider, batch_size=2, max_retries=3, retry_delay=0.01
        )
        embeddings = embedder.embed_batch(["text1"])

        assert len(embeddings) == 1
        assert mock_provider.embed_texts.call_count == 2

    def test_embed_stats(self):
        """Test embedder stats."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1, 0.2]])

        embedder = BatchEmbedder(mock_provider, batch_size=2)
        embedder.embed_batch(["text1"])

        stats = embedder.stats
        assert stats["request_count"] == 1
        assert stats["batch_size"] == 2


class TestChunkingPipeline:
    """Test ChunkingPipeline class."""

    def test_pipeline_creation(self):
        """Test creating pipeline."""
        pipeline = ChunkingPipeline()
        assert pipeline.config is not None

    def test_pipeline_with_config(self):
        """Test pipeline with custom config."""
        config = PipelineConfig(
            chunking_strategy=ChunkingStrategy.CODE, embedding_batch_size=64
        )
        pipeline = ChunkingPipeline(config=config)
        assert pipeline.config.embedding_batch_size == 64

    def test_process_text(self):
        """Test processing text."""
        # Use SENTENCE strategy which is more reliable for short texts
        config = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
        pipeline = ChunkingPipeline(config=config)
        result = pipeline.process_text(
            "This is a test document. It has multiple sentences. Each sentence should be processed. "
            "We need enough content here. To ensure proper chunking. At least several sentences work well.",
            source_id="test_doc",
        )
        assert result.success is True
        assert result.chunk_count > 0

    def test_process_text_with_metadata(self):
        """Test processing text with metadata."""
        pipeline = ChunkingPipeline()
        result = pipeline.process_text(
            "Hello, World!", source_id="test_doc", metadata={"author": "test"}
        )
        assert result.success is True

    def test_builder_pattern(self):
        """Test builder pattern for configuration."""
        pipeline = (
            ChunkingPipeline()
            .with_strategy(ChunkingStrategy.SENTENCE)
            .with_enrichment(lambda c: c)
            .with_filter(lambda c: len(c.text) > 0)
        )
        assert pipeline.config.chunking_strategy == ChunkingStrategy.SENTENCE

    def test_process_stream(self):
        """Test streaming processing."""
        config = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
        pipeline = ChunkingPipeline(config=config)
        text = (
            "This is a test sentence. It has multiple sentences. We want to stream them. "
            "Here is more content. And even more content. Streaming should work well."
        )

        chunks = list(pipeline.process_stream(text, "test_doc"))
        assert len(chunks) > 0

    def test_process_batch(self):
        """Test batch processing."""
        pipeline = ChunkingPipeline()
        items = [
            {"text": "First document content.", "source_id": "doc1"},
            {"text": "Second document content.", "source_id": "doc2"},
        ]

        result = pipeline.process_batch(items)
        assert result.total_items == 2
        assert result.processed_items == 2

    def test_progress_callback(self):
        """Test progress callback."""
        progress_updates = []

        def callback(current, total, status):
            progress_updates.append((current, total, status))

        pipeline = ChunkingPipeline().with_progress_callback(callback)
        items = [
            {"text": "Doc 1", "source_id": "d1"},
            {"text": "Doc 2", "source_id": "d2"},
        ]

        pipeline.process_batch(items)
        assert len(progress_updates) > 0

    def test_error_handling_skip(self):
        """Test skip errors mode."""
        config = PipelineConfig(
            error_handling=ErrorHandling.SKIP_ERRORS, validate_chunks=True
        )
        pipeline = ChunkingPipeline(config=config)

        # Process with some content
        result = pipeline.process_text("Some valid content", "test")
        assert result.success is True

    def test_get_metrics(self):
        """Test getting metrics."""
        pipeline = ChunkingPipeline()
        pipeline.process_text("Test content", "test")

        metrics = pipeline.get_metrics()
        assert isinstance(metrics, dict)
        assert "progress" in metrics


class TestChunkingPipelineAsync:
    """Test async methods of ChunkingPipeline."""

    @pytest.mark.asyncio
    async def test_process_text_async(self):
        """Test async text processing."""
        config = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
        pipeline = ChunkingPipeline(config=config)
        result = await pipeline.process_text_async(
            "This is a test document for async processing. It has multiple sentences. "
            "Each sentence will be processed. We need enough content here. Async works well.",
            source_id="test_doc",
        )
        assert result.success is True
        assert result.chunk_count > 0

    @pytest.mark.asyncio
    async def test_process_batch_async(self):
        """Test async batch processing."""
        pipeline = ChunkingPipeline()
        items = [
            {"text": "First async document.", "source_id": "doc1"},
            {"text": "Second async document.", "source_id": "doc2"},
            {"text": "Third async document.", "source_id": "doc3"},
        ]

        result = await pipeline.process_batch_async(items, concurrent_limit=2)
        assert result.total_items == 3
        assert result.processed_items == 3

    @pytest.mark.asyncio
    async def test_process_stream_async(self):
        """Test async streaming."""
        config = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
        pipeline = ChunkingPipeline(config=config)
        text = (
            "This is async streaming. It should work properly. Here is more content. "
            "And even more content here. We need multiple sentences. For streaming to work."
        )

        chunks = []
        async for chunk in pipeline.process_stream_async(text, "test"):
            chunks.append(chunk)

        assert len(chunks) > 0


class TestPipelineFileProcessing:
    """Test file processing methods."""

    def test_process_file(self):
        """Test processing a file."""
        config = PipelineConfig(chunking_strategy=ChunkingStrategy.SENTENCE)
        pipeline = ChunkingPipeline(config=config)

        # Create temp file with enough content
        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as f:
            f.write(
                "This is test file content for chunking pipeline. It has multiple sentences. "
                "Each sentence should be processed. We need enough content here. "
                "File processing should work well. Here is more content for chunking."
            )
            f.flush()

            result = pipeline.process_file(f.name)
            assert result.success is True
            assert result.chunk_count > 0

            # Cleanup
            Path(f.name).unlink()

    def test_process_nonexistent_file(self):
        """Test processing nonexistent file."""
        pipeline = ChunkingPipeline()
        result = pipeline.process_file("/nonexistent/file.txt")
        assert result.success is False
        assert result.error_count > 0

    @pytest.mark.asyncio
    async def test_process_file_async(self):
        """Test async file processing."""
        pipeline = ChunkingPipeline()

        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as f:
            f.write("Async file processing test content.")
            f.flush()

            result = await pipeline.process_file_async(f.name)
            assert result.success is True

            Path(f.name).unlink()

    def test_process_directory(self):
        """Test processing a directory."""
        pipeline = ChunkingPipeline()

        # Create temp directory with files
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create test files
            for i in range(3):
                (Path(tmpdir) / f"test_{i}.txt").write_text(f"Content of file {i}.")

            result = pipeline.process_directory(tmpdir, pattern="*.txt")
            assert result.total_items == 3
            assert result.processed_items == 3

    @pytest.mark.asyncio
    async def test_process_directory_async(self):
        """Test async directory processing."""
        pipeline = ChunkingPipeline()

        with tempfile.TemporaryDirectory() as tmpdir:
            for i in range(3):
                (Path(tmpdir) / f"test_{i}.txt").write_text(f"Async content {i}.")

            result = await pipeline.process_directory_async(
                tmpdir, pattern="*.txt", concurrent_limit=2
            )
            assert result.total_items == 3


class TestFactoryFunctions:
    """Test factory functions."""

    def test_create_pipeline(self):
        """Test create_pipeline factory."""
        pipeline = create_pipeline()
        assert isinstance(pipeline, ChunkingPipeline)

    def test_create_pipeline_with_strategy(self):
        """Test create_pipeline with strategy."""
        pipeline = create_pipeline(strategy=ChunkingStrategy.CODE)
        assert pipeline.config.chunking_strategy == ChunkingStrategy.CODE

    def test_create_pipeline_with_kwargs(self):
        """Test create_pipeline with kwargs."""
        pipeline = create_pipeline(
            strategy=ChunkingStrategy.SEMANTIC,
            embedding_batch_size=64,
            max_concurrent_tasks=8,
        )
        assert pipeline.config.embedding_batch_size == 64
        assert pipeline.config.max_concurrent_tasks == 8

    def test_create_code_pipeline(self):
        """Test create_code_pipeline factory."""
        pipeline = create_code_pipeline()
        assert pipeline.config.chunking_strategy == ChunkingStrategy.CODE
        assert pipeline.config.embedding_batch_size == 16  # Smaller for code
        assert pipeline.config.max_text_length == 16384  # Larger for code

    def test_create_document_pipeline(self):
        """Test create_document_pipeline factory."""
        pipeline = create_document_pipeline()
        assert pipeline.config.chunking_strategy == ChunkingStrategy.SEMANTIC


class TestContextManagers:
    """Test context managers."""

    def test_pipeline_context(self):
        """Test sync pipeline context manager."""
        with pipeline_context(strategy=ChunkingStrategy.SENTENCE) as pipeline:
            assert isinstance(pipeline, ChunkingPipeline)
            result = pipeline.process_text("Test content.", "test")
            assert result.success is True

    @pytest.mark.asyncio
    async def test_async_pipeline_context(self):
        """Test async pipeline context manager."""
        async with async_pipeline_context() as pipeline:
            assert isinstance(pipeline, ChunkingPipeline)
            result = await pipeline.process_text_async("Test content.", "test")
            assert result.success is True


class TestPipelineWithEmbedding:
    """Test pipeline with mock embedding provider."""

    def test_process_with_embeddings(self):
        """Test processing with embeddings."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1] * 128])

        pipeline = ChunkingPipeline(embedding_provider=mock_provider)
        result = pipeline.process_text("Test text for embedding.", "test")

        assert result.success is True
        if result.chunk_count > 0:
            assert len(result.embeddings) == result.chunk_count

    @pytest.mark.asyncio
    async def test_process_async_with_embeddings(self):
        """Test async processing with embeddings."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(return_value=[[0.1] * 128])

        pipeline = ChunkingPipeline(embedding_provider=mock_provider)
        result = await pipeline.process_text_async(
            "Async test with embeddings.", "test"
        )

        assert result.success is True


class TestPipelineErrorHandling:
    """Test error handling scenarios."""

    def test_embedding_error_collected(self):
        """Test embedding errors are collected."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(side_effect=Exception("Embedding failed"))

        config = PipelineConfig(
            error_handling=ErrorHandling.COLLECT_ERRORS, max_retries=1
        )
        pipeline = ChunkingPipeline(config=config, embedding_provider=mock_provider)
        result = pipeline.process_text("Test content.", "test")

        # Should still have chunks even if embedding failed
        if result.chunk_count > 0:
            assert result.error_count > 0

    def test_fail_fast_mode(self):
        """Test fail fast error handling."""
        mock_provider = Mock()
        mock_provider.embed_texts = Mock(side_effect=Exception("Immediate failure"))

        config = PipelineConfig(error_handling=ErrorHandling.FAIL_FAST, max_retries=1)
        pipeline = ChunkingPipeline(config=config, embedding_provider=mock_provider)
        result = pipeline.process_text("Test content.", "test")

        # In fail fast, result should indicate failure
        if result.error_count > 0:
            assert result.success is False


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
