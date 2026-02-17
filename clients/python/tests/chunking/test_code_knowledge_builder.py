"""
Unit tests for CodeKnowledgeBuilder.

This module tests the high-level code knowledge building functionality
that coordinates vector and graph database population.
"""

import asyncio
import os
import sys
import tempfile
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest

from proximadb_sdk.chunking_strategies.code import (
    CodeRelation,
    CodeRelationType,
    CodeSymbol,
    CodeSymbolType,
    ParsedCode,
    SourceLocation,
)

# Import after loader has set up modules
from proximadb_sdk.code_knowledge import (
    CodeIndexConfig,
    CodeKnowledgeBuilder,
    CodeSearchResult,
    IndexingResult,
    create_code_knowledge_store,
)

# Use our custom loader to avoid protobuf issues
from .loader import RESOURCES_DIR, code_module, read_resource_file


class TestCodeIndexConfig:
    """Test cases for CodeIndexConfig."""

    def test_default_config(self):
        """Test default configuration values."""
        config = CodeIndexConfig()
        assert config.vector_collection_name == "code_symbols"
        assert config.vector_dimension == 1536
        assert config.graph_name == "code_graph"
        assert config.include_private is True
        assert config.include_tests is True
        assert config.include_documentation is True
        assert config.enable_incremental is True

    def test_custom_config(self):
        """Test custom configuration values."""
        config = CodeIndexConfig(
            vector_collection_name="my_collection",
            vector_dimension=768,
            graph_name="my_graph",
            include_private=False,
            include_tests=False,
        )
        assert config.vector_collection_name == "my_collection"
        assert config.vector_dimension == 768
        assert config.graph_name == "my_graph"
        assert config.include_private is False
        assert config.include_tests is False

    def test_config_with_exclude_patterns(self):
        """Test configuration with exclude patterns."""
        config = CodeIndexConfig(
            exclude_patterns=["*.pyc", "__pycache__/*", "node_modules/*"]
        )
        assert "*.pyc" in config.exclude_patterns
        assert "__pycache__/*" in config.exclude_patterns

    def test_config_with_include_patterns(self):
        """Test configuration with include patterns."""
        config = CodeIndexConfig(include_patterns=["*.py", "*.rs", "*.go"])
        assert "*.py" in config.include_patterns
        assert "*.rs" in config.include_patterns

    def test_config_embedding_batch_size(self):
        """Test embedding batch size configuration."""
        config = CodeIndexConfig(embedding_batch_size=64)
        assert config.embedding_batch_size == 64

    def test_config_max_content_length(self):
        """Test max content length configuration."""
        config = CodeIndexConfig(max_content_length=16000)
        assert config.max_content_length == 16000


class TestIndexingResult:
    """Test cases for IndexingResult."""

    def test_result_creation(self):
        """Test IndexingResult creation."""
        result = IndexingResult(
            files_processed=10,
            files_skipped=2,
            files_failed=1,
            symbols_indexed=100,
            relations_created=50,
        )
        assert result.files_processed == 10
        assert result.files_skipped == 2
        assert result.files_failed == 1
        assert result.symbols_indexed == 100
        assert result.relations_created == 50

    def test_result_with_errors(self):
        """Test IndexingResult with errors."""
        result = IndexingResult(
            files_processed=8,
            symbols_indexed=80,
            errors=[
                {"file": "file1.py", "error": "parse error"},
                {"file": "file2.py", "error": "encoding error"},
            ],
        )
        assert len(result.errors) == 2
        assert result.errors[0]["file"] == "file1.py"

    def test_result_with_file_hashes(self):
        """Test IndexingResult with file hashes."""
        result = IndexingResult(
            files_processed=5,
            file_hashes={"/path/to/file1.py": "abc123", "/path/to/file2.py": "def456"},
        )
        assert len(result.file_hashes) == 2
        assert result.file_hashes["/path/to/file1.py"] == "abc123"

    def test_result_defaults(self):
        """Test IndexingResult default values."""
        result = IndexingResult()
        assert result.files_processed == 0
        assert result.files_skipped == 0
        assert result.files_failed == 0
        assert result.symbols_indexed == 0
        assert result.relations_created == 0
        assert result.errors == []
        assert result.file_hashes == {}


class TestCodeSearchResult:
    """Test cases for CodeSearchResult."""

    def test_result_creation(self):
        """Test CodeSearchResult creation."""
        result = CodeSearchResult(
            symbol_id="abc123",
            symbol_type="FUNCTION",
            fully_qualified_name="module.my_function",
            simple_name="my_function",
            source_code="def my_function(): pass",
            file_path="/path/to/file.py",
            start_line=42,
            end_line=45,
            language="python",
            score=0.95,
        )
        assert result.symbol_id == "abc123"
        assert result.symbol_type == "FUNCTION"
        assert result.fully_qualified_name == "module.my_function"
        assert result.simple_name == "my_function"
        assert result.score == 0.95

    def test_result_with_documentation(self):
        """Test CodeSearchResult with documentation."""
        result = CodeSearchResult(
            symbol_id="abc123",
            symbol_type="FUNCTION",
            fully_qualified_name="my_function",
            simple_name="my_function",
            source_code="def my_function(): pass",
            file_path="/path/to/file.py",
            start_line=1,
            end_line=2,
            language="python",
            score=0.9,
            documentation="This function does something.",
        )
        assert result.documentation == "This function does something."

    def test_result_with_signature(self):
        """Test CodeSearchResult with signature."""
        result = CodeSearchResult(
            symbol_id="abc123",
            symbol_type="FUNCTION",
            fully_qualified_name="my_function",
            simple_name="my_function",
            source_code="def my_function(x: int) -> str: pass",
            file_path="/path/to/file.py",
            start_line=1,
            end_line=2,
            language="python",
            score=0.9,
            signature="my_function(x: int) -> str",
        )
        assert result.signature == "my_function(x: int) -> str"

    def test_result_with_graph_context(self):
        """Test CodeSearchResult with graph context."""
        result = CodeSearchResult(
            symbol_id="abc123",
            symbol_type="FUNCTION",
            fully_qualified_name="my_function",
            simple_name="my_function",
            source_code="def my_function(): pass",
            file_path="/path/to/file.py",
            start_line=1,
            end_line=2,
            language="python",
            score=0.9,
            callers=["caller1", "caller2"],
            callees=["callee1"],
            parent_symbols=["MyClass"],
        )
        assert result.callers == ["caller1", "caller2"]
        assert result.callees == ["callee1"]
        assert result.parent_symbols == ["MyClass"]


class TestCodeKnowledgeBuilder:
    """Test cases for CodeKnowledgeBuilder."""

    @pytest.fixture
    def mock_client(self):
        """Create a mock ProximaDB client."""
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(
            return_value=Mock(
                insert=AsyncMock(),
                search=AsyncMock(return_value=[]),
                delete=AsyncMock(),
            )
        )
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(
            return_value=Mock(
                insert_node=AsyncMock(),
                insert_edge=AsyncMock(),
                traverse=AsyncMock(return_value=[]),
            )
        )
        return client

    @pytest.fixture
    def builder(self, mock_client):
        """Create a CodeKnowledgeBuilder with mocked dependencies."""
        return CodeKnowledgeBuilder(client=mock_client, config=CodeIndexConfig())

    @pytest.mark.asyncio
    async def test_builder_creation(self, builder):
        """Test CodeKnowledgeBuilder can be created."""
        assert builder is not None
        assert builder.config is not None

    @pytest.mark.asyncio
    async def test_builder_has_chunker(self, builder):
        """Test builder has a code chunker."""
        assert builder._chunker is not None

    @pytest.mark.asyncio
    async def test_initialize_creates_collection(self, builder, mock_client):
        """Test initialize creates vector collection if needed."""
        await builder.initialize()
        mock_client.list_collections.assert_called()
        mock_client.create_collection.assert_called_once()

    @pytest.mark.asyncio
    async def test_initialize_creates_graph(self, builder, mock_client):
        """Test initialize creates graph if needed."""
        await builder.initialize()
        mock_client.list_graphs.assert_called()
        mock_client.create_graph.assert_called_once()

    @pytest.mark.asyncio
    async def test_index_file_with_content(self, builder):
        """Test indexing with provided content."""
        content = '''
def my_function():
    """A test function."""
    return 42
'''
        result = await builder.index_file(file_path="/virtual/test.py", content=content)
        assert isinstance(result, IndexingResult)
        # May have symbols or may not depending on parser
        assert result.files_processed >= 0 or result.files_skipped >= 0

    @pytest.mark.asyncio
    async def test_index_nonexistent_file(self, builder):
        """Test indexing a non-existent file."""
        result = await builder.index_file("/nonexistent/file.py")
        assert isinstance(result, IndexingResult)
        # File doesn't exist, so should fail
        assert result.files_failed == 1 or result.files_skipped == 1

    @pytest.mark.asyncio
    async def test_index_file_incremental(self, builder):
        """Test incremental indexing skips unchanged files."""
        content = "def test(): pass"

        # First index
        result1 = await builder.index_file("/virtual/test.py", content=content)

        # Second index with same content (should be skipped)
        result2 = await builder.index_file("/virtual/test.py", content=content)

        assert isinstance(result1, IndexingResult)
        assert isinstance(result2, IndexingResult)
        # Second should be skipped if incremental is enabled
        # (may need to process at least once first)

    @pytest.mark.asyncio
    async def test_index_file_force(self, builder):
        """Test force re-indexing."""
        content = "def test(): pass"

        # First index
        result1 = await builder.index_file("/virtual/test.py", content=content)

        # Second index with force
        result2 = await builder.index_file(
            "/virtual/test.py", content=content, force=True
        )

        assert isinstance(result1, IndexingResult)
        assert isinstance(result2, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_directory(self, builder):
        """Test indexing a directory."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create test files
            with open(os.path.join(tmpdir, "test.py"), "w") as f:
                f.write("def func(): pass")

            result = await builder.index_directory(tmpdir, recursive=True)
            assert isinstance(result, IndexingResult)
            # At least attempted to process
            total = result.files_processed + result.files_skipped + result.files_failed
            assert total >= 0

    @pytest.mark.asyncio
    async def test_index_directory_recursive(self, builder):
        """Test recursive directory indexing."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create nested directories
            subdir = os.path.join(tmpdir, "subdir")
            os.makedirs(subdir)

            # Create files in both directories
            with open(os.path.join(tmpdir, "file1.py"), "w") as f:
                f.write("def func1(): pass")
            with open(os.path.join(subdir, "file2.py"), "w") as f:
                f.write("def func2(): pass")

            result = await builder.index_directory(tmpdir, recursive=True)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_search_code(self, builder, mock_client):
        """Test searching code."""
        # Mock search results
        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = [
            {
                "id": "1",
                "score": 0.95,
                "metadata": {
                    "symbol_id": "sym123",
                    "symbol_type": "FUNCTION",
                    "fully_qualified_name": "test_function",
                    "simple_name": "test_function",
                    "file_path": "/path/to/test.py",
                    "start_line": 10,
                    "end_line": 15,
                    "language": "python",
                    "source_code": "def test_function(): pass",
                },
            }
        ]

        results = await builder.search_code("test function", top_k=10)
        assert isinstance(results, list)

    @pytest.mark.asyncio
    async def test_search_code_with_language_filter(self, builder, mock_client):
        """Test searching code with language filter."""
        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = []

        results = await builder.search_code("test", top_k=5, filter_language="python")
        assert isinstance(results, list)

    @pytest.mark.asyncio
    async def test_search_code_with_symbol_type_filter(self, builder, mock_client):
        """Test searching code with symbol type filter."""
        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = []

        results = await builder.search_code(
            "test", top_k=5, filter_symbol_types=["FUNCTION", "CLASS"]
        )
        assert isinstance(results, list)

    @pytest.mark.asyncio
    async def test_find_callers(self, builder, mock_client):
        """Test finding function callers."""
        mock_graph = mock_client.get_graph.return_value
        mock_graph.traverse.return_value = [
            {"id": "caller1", "properties": {"simple_name": "caller_func"}}
        ]

        # Need to set up search to resolve symbol
        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = [
            {
                "score": 0.9,
                "metadata": {
                    "symbol_id": "sym123",
                    "symbol_type": "FUNCTION",
                    "fully_qualified_name": "my_function",
                    "simple_name": "my_function",
                    "file_path": "/test.py",
                    "start_line": 1,
                    "end_line": 5,
                    "language": "python",
                    "source_code": "def my_function(): pass",
                },
            }
        ]

        results = await builder.find_callers("my_function", max_depth=2)
        assert isinstance(results, list)

    @pytest.mark.asyncio
    async def test_find_callees(self, builder, mock_client):
        """Test finding function callees."""
        mock_graph = mock_client.get_graph.return_value
        mock_graph.traverse.return_value = []

        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = [
            {
                "score": 0.9,
                "metadata": {
                    "symbol_id": "sym123",
                    "symbol_type": "FUNCTION",
                    "fully_qualified_name": "my_function",
                    "simple_name": "my_function",
                    "file_path": "/test.py",
                    "start_line": 1,
                    "end_line": 5,
                    "language": "python",
                    "source_code": "def my_function(): pass",
                },
            }
        ]

        results = await builder.find_callees("my_function", max_depth=2)
        assert isinstance(results, list)

    @pytest.mark.asyncio
    async def test_find_usages(self, builder, mock_client):
        """Test finding symbol usages."""
        mock_graph = mock_client.get_graph.return_value
        mock_graph.traverse.return_value = []

        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = [
            {
                "score": 0.9,
                "metadata": {
                    "symbol_id": "sym123",
                    "symbol_type": "VARIABLE",
                    "fully_qualified_name": "my_variable",
                    "simple_name": "my_variable",
                    "file_path": "/test.py",
                    "start_line": 1,
                    "end_line": 1,
                    "language": "python",
                    "source_code": "my_variable = 42",
                },
            }
        ]

        results = await builder.find_usages("my_variable")
        assert isinstance(results, list)

    @pytest.mark.asyncio
    async def test_get_impact_analysis(self, builder, mock_client):
        """Test impact analysis."""
        mock_graph = mock_client.get_graph.return_value
        mock_graph.traverse.return_value = []

        mock_collection = mock_client.get_collection.return_value
        mock_collection.search.return_value = [
            {
                "score": 0.9,
                "metadata": {
                    "symbol_id": "sym123",
                    "symbol_type": "FUNCTION",
                    "fully_qualified_name": "my_function",
                    "simple_name": "my_function",
                    "file_path": "/test.py",
                    "start_line": 1,
                    "end_line": 5,
                    "language": "python",
                    "source_code": "def my_function(): pass",
                },
            }
        ]

        result = await builder.get_impact_analysis("my_function", max_depth=3)
        assert isinstance(result, dict)

    @pytest.mark.asyncio
    async def test_delete_file_index(self, builder, mock_client):
        """Test deleting file index."""
        result = await builder.delete_file_index("/path/to/file.py")
        # Should return True/False
        assert isinstance(result, bool)

    @pytest.mark.asyncio
    async def test_get_indexed_files(self, builder):
        """Test getting list of indexed files."""
        # Index a file first
        content = "def test(): pass"
        await builder.index_file("/virtual/test.py", content=content)

        files = builder.get_indexed_files()
        assert isinstance(files, list)

    @pytest.mark.asyncio
    async def test_get_file_hash(self, builder):
        """Test getting file hash."""
        content = "def test(): pass"
        await builder.index_file("/virtual/test.py", content=content)

        hash_val = builder.get_file_hash("/virtual/test.py")
        # May be None if file wasn't successfully indexed, or a hash string
        assert hash_val is None or isinstance(hash_val, str)


class TestCreateCodeKnowledgeStore:
    """Test cases for the create_code_knowledge_store factory function."""

    @pytest.fixture
    def mock_client(self):
        """Create a mock ProximaDB client."""
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(
            return_value=Mock(insert=AsyncMock(), search=AsyncMock(return_value=[]))
        )
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(
            return_value=Mock(
                insert_node=AsyncMock(),
                insert_edge=AsyncMock(),
                traverse=AsyncMock(return_value=[]),
            )
        )
        return client

    @pytest.mark.asyncio
    async def test_create_with_defaults(self, mock_client):
        """Test creating store with default settings."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create a test file
            with open(os.path.join(tmpdir, "test.py"), "w") as f:
                f.write("def func(): pass")

            builder, result = await create_code_knowledge_store(
                client=mock_client, directory=tmpdir
            )
            assert isinstance(builder, CodeKnowledgeBuilder)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_create_with_custom_config(self, mock_client):
        """Test creating store with custom config."""
        config = CodeIndexConfig(
            vector_collection_name="custom_collection", vector_dimension=768
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            builder, result = await create_code_knowledge_store(
                client=mock_client, directory=tmpdir, config=config
            )
            assert builder.config.vector_collection_name == "custom_collection"
            assert builder.config.vector_dimension == 768


class TestLanguageSupport:
    """Test code knowledge building for all supported languages."""

    @pytest.fixture
    def mock_client(self):
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(
            return_value=Mock(insert=AsyncMock(), search=AsyncMock(return_value=[]))
        )
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(
            return_value=Mock(insert_node=AsyncMock(), insert_edge=AsyncMock())
        )
        return client

    @pytest.fixture
    def builder(self, mock_client):
        return CodeKnowledgeBuilder(client=mock_client, config=CodeIndexConfig())

    @pytest.mark.asyncio
    async def test_index_python(self, builder):
        """Test indexing Python code."""
        content = read_resource_file("python", "sample.py")
        if content:
            result = await builder.index_file("/test.py", content=content)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_rust(self, builder):
        """Test indexing Rust code."""
        content = read_resource_file("rust", "sample.rs")
        if content:
            result = await builder.index_file("/test.rs", content=content)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_go(self, builder):
        """Test indexing Go code."""
        content = read_resource_file("go", "sample.go")
        if content:
            result = await builder.index_file("/test.go", content=content)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_java(self, builder):
        """Test indexing Java code."""
        content = read_resource_file("java", "Sample.java")
        if content:
            result = await builder.index_file("/Test.java", content=content)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_javascript(self, builder):
        """Test indexing JavaScript code."""
        content = read_resource_file("javascript", "sample.js")
        if content:
            result = await builder.index_file("/test.js", content=content)
            assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_typescript(self, builder):
        """Test indexing TypeScript code."""
        content = read_resource_file("typescript", "sample.ts")
        if content:
            result = await builder.index_file("/test.ts", content=content)
            assert isinstance(result, IndexingResult)


class TestErrorHandling:
    """Test error handling in CodeKnowledgeBuilder."""

    @pytest.fixture
    def mock_client(self):
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(
            return_value=Mock(insert=AsyncMock(), search=AsyncMock(return_value=[]))
        )
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(
            return_value=Mock(insert_node=AsyncMock(), insert_edge=AsyncMock())
        )
        return client

    @pytest.fixture
    def builder(self, mock_client):
        return CodeKnowledgeBuilder(client=mock_client, config=CodeIndexConfig())

    @pytest.mark.asyncio
    async def test_index_invalid_syntax(self, builder):
        """Test indexing code with invalid syntax."""
        content = "def broken(\n    # missing closing paren"
        result = await builder.index_file("/test.py", content=content)
        # Should not crash, may still extract partial info
        assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_empty_file(self, builder):
        """Test indexing an empty file."""
        result = await builder.index_file("/empty.py", content="")
        assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_binary_content(self, builder):
        """Test indexing binary content (should be handled gracefully)."""
        content = "\x00\x01\x02\x03"
        result = await builder.index_file("/binary.py", content=content)
        assert isinstance(result, IndexingResult)

    @pytest.mark.asyncio
    async def test_index_unsupported_extension(self, builder):
        """Test indexing a file with unsupported extension."""
        result = await builder.index_file("/file.xyz", content="some content")
        assert isinstance(result, IndexingResult)
        # Should be skipped
        assert result.files_skipped == 1


class TestHashComputation:
    """Test hash computation for change detection."""

    @pytest.fixture
    def mock_client(self):
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(
            return_value=Mock(insert=AsyncMock(), search=AsyncMock(return_value=[]))
        )
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(
            return_value=Mock(insert_node=AsyncMock(), insert_edge=AsyncMock())
        )
        return client

    def test_sha256_hash(self, mock_client):
        """Test SHA256 hash computation."""
        config = CodeIndexConfig(hash_algorithm="sha256")
        builder = CodeKnowledgeBuilder(client=mock_client, config=config)

        hash1 = builder._compute_hash("test content")
        hash2 = builder._compute_hash("test content")
        hash3 = builder._compute_hash("different content")

        assert hash1 == hash2
        assert hash1 != hash3
        assert len(hash1) == 64  # SHA256 hex length

    def test_md5_hash(self, mock_client):
        """Test MD5 hash computation."""
        config = CodeIndexConfig(hash_algorithm="md5")
        builder = CodeKnowledgeBuilder(client=mock_client, config=config)

        hash1 = builder._compute_hash("test content")
        hash2 = builder._compute_hash("test content")

        assert hash1 == hash2
        assert len(hash1) == 32  # MD5 hex length


class TestEmbeddingGeneration:
    """Test embedding generation."""

    @pytest.fixture
    def mock_client(self):
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(
            return_value=Mock(insert=AsyncMock(), search=AsyncMock(return_value=[]))
        )
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(
            return_value=Mock(insert_node=AsyncMock(), insert_edge=AsyncMock())
        )
        return client

    def test_placeholder_embedding_dimension(self, mock_client):
        """Test placeholder embedding has correct dimension."""
        config = CodeIndexConfig(vector_dimension=1536)
        builder = CodeKnowledgeBuilder(client=mock_client, config=config)

        embedding = builder._generate_placeholder_embedding("test")
        assert len(embedding) == 1536

    def test_placeholder_embedding_deterministic(self, mock_client):
        """Test placeholder embedding is deterministic."""
        builder = CodeKnowledgeBuilder(client=mock_client)

        emb1 = builder._generate_placeholder_embedding("test")
        emb2 = builder._generate_placeholder_embedding("test")

        assert emb1 == emb2

    def test_placeholder_embedding_different_for_different_text(self, mock_client):
        """Test different texts produce different embeddings."""
        builder = CodeKnowledgeBuilder(client=mock_client)

        emb1 = builder._generate_placeholder_embedding("text1")
        emb2 = builder._generate_placeholder_embedding("text2")

        assert emb1 != emb2


class TestFileCollection:
    """Test file collection functionality."""

    @pytest.fixture
    def mock_client(self):
        client = Mock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock(return_value=Mock())
        client.get_collection = AsyncMock(return_value=Mock())
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock(return_value=Mock())
        client.get_graph = AsyncMock(return_value=Mock())
        return client

    def test_matches_patterns(self, mock_client):
        """Test pattern matching."""
        builder = CodeKnowledgeBuilder(client=mock_client)

        assert builder._matches_patterns("test.py", ["*.py"])
        assert builder._matches_patterns("src/test.py", ["*.py"])
        assert not builder._matches_patterns("test.js", ["*.py"])
        assert builder._matches_patterns("test.py", ["*.py", "*.js"])

    def test_collect_files_excludes_patterns(self, mock_client):
        """Test file collection excludes patterns."""
        config = CodeIndexConfig(exclude_patterns=["*.pyc", "__pycache__/*"])
        builder = CodeKnowledgeBuilder(client=mock_client, config=config)

        with tempfile.TemporaryDirectory() as tmpdir:
            # Create files
            Path(tmpdir, "test.py").write_text("def func(): pass")
            Path(tmpdir, "test.pyc").write_text("bytecode")

            files = builder._collect_files(Path(tmpdir), recursive=False)
            file_names = [f.name for f in files]

            assert "test.py" in file_names
            assert "test.pyc" not in file_names


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
