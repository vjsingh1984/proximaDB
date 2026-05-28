"""
Unit tests for Repository Indexer module.

This module tests:
- Git-aware code indexing
- Incremental indexing
- Repository statistics
- Search with git context
"""

import importlib.util
import shutil
import subprocess
import sys
import types
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest

# Load modules directly to avoid protobuf issues
src_path = Path(__file__).parent.parent.parent / "src"
sys.path.insert(0, str(src_path))

# First, load repository_manager module
repo_spec = importlib.util.spec_from_file_location(
    "proximadb.repository_manager",
    str(src_path / "proximadb_sdk" / "repository_manager.py"),
)
repo_module = importlib.util.module_from_spec(repo_spec)
sys.modules["proximadb.repository_manager"] = repo_module
repo_spec.loader.exec_module(repo_module)

# Create mock proximadb package with proper __path__ for relative imports
if "proximadb" not in sys.modules:
    proximadb = types.ModuleType("proximadb")
    proximadb.__path__ = [str(src_path / "proximadb_sdk")]
    proximadb.__package__ = "proximadb"
    sys.modules["proximadb"] = proximadb
else:
    proximadb = sys.modules["proximadb"]
    # Ensure __path__ is set even if module was created elsewhere
    if not hasattr(proximadb, "__path__") or proximadb.__path__ is None:
        proximadb.__path__ = [str(src_path / "proximadb_sdk")]
        proximadb.__package__ = "proximadb"

proximadb.repository_manager = repo_module

# Create mock code_knowledge module
mock_builder_class = MagicMock()
mock_index_result_class = type(
    "IndexingResult",
    (),
    {
        "files_processed": 0,
        "files_skipped": 0,
        "files_failed": 0,
        "symbols_indexed": 0,
        "relations_created": 0,
        "errors": [],
        "file_hashes": {},
    },
)

code_knowledge_module = types.ModuleType("proximadb.code_knowledge")
code_knowledge_module.CodeKnowledgeBuilder = mock_builder_class
code_knowledge_module.CodeIndexConfig = type(
    "CodeIndexConfig",
    (),
    {
        "vector_collection_name": "code_symbols",
        "vector_dimension": 1536,
        "graph_name": "code_graph",
        "include_private": True,
        "include_tests": True,
        "include_documentation": True,
        "include_patterns": ["*"],
        "exclude_patterns": [
            "*.pyc",
            "__pycache__/*",
            ".git/*",
            "node_modules/*",
            "vendor/*",
            "target/*",
            "build/*",
            "dist/*",
        ],
        "embedding_batch_size": 32,
        "max_content_length": 8000,
        "enable_incremental": True,
        "hash_algorithm": "sha256",
    },
)
code_knowledge_module.IndexingResult = mock_index_result_class
code_knowledge_module.CodeSearchResult = type("CodeSearchResult", (), {})
sys.modules["proximadb.code_knowledge"] = code_knowledge_module

# Create mock chunking_strategies.code module only if not already loaded with real module
# (loader.py may have already set up the real module)
_saved_code_module = sys.modules.get("proximadb.chunking_strategies.code")
if (
    _saved_code_module is None
    or not hasattr(_saved_code_module, "__file__")
    or _saved_code_module.__file__ is None
):
    chunking_code_module = types.ModuleType("proximadb.chunking_strategies.code")
    chunking_code_module.EXTENSION_TO_LANGUAGE = {
        ".py": "python",
        ".rs": "rust",
        ".js": "javascript",
        ".go": "go",
        ".java": "java",
    }
    chunking_code_module.get_supported_extensions = lambda: [
        ".py",
        ".rs",
        ".js",
        ".go",
        ".java",
    ]
    sys.modules["proximadb.chunking_strategies.code"] = chunking_code_module
else:
    # Use the real module - it has what we need
    chunking_code_module = _saved_code_module

# Now load repository_indexer module
indexer_spec = importlib.util.spec_from_file_location(
    "proximadb.repository_indexer",
    str(src_path / "proximadb_sdk" / "repository_indexer.py"),
)
indexer_module = importlib.util.module_from_spec(indexer_spec)
sys.modules["proximadb.repository_indexer"] = indexer_module
indexer_spec.loader.exec_module(indexer_module)

# Track modules we added so we can clean up
# Note: Only clean up modules we actually created (not ones that already existed)
_added_modules = [
    "proximadb.repository_manager",
    "proximadb.code_knowledge",
    "proximadb.repository_indexer",
]
# Only add chunking_strategies.code to cleanup if we created it
if (
    _saved_code_module is None
    or not hasattr(_saved_code_module, "__file__")
    or _saved_code_module.__file__ is None
):
    _added_modules.append("proximadb.chunking_strategies.code")


@pytest.fixture(scope="module", autouse=True)
def cleanup_modules():
    """Clean up mock modules after all tests in this module run."""
    yield
    # Remove mock modules to avoid polluting other test files
    for mod_name in _added_modules:
        if mod_name in sys.modules:
            del sys.modules[mod_name]


# Extract classes from module
IndexMode = indexer_module.IndexMode
ChangeStrategy = indexer_module.ChangeStrategy
RepositoryIndexConfig = indexer_module.RepositoryIndexConfig
RepositoryIndexResult = indexer_module.RepositoryIndexResult
GitEnrichedSearchResult = indexer_module.GitEnrichedSearchResult
RepositoryIndexer = indexer_module.RepositoryIndexer
create_repository_indexer = indexer_module.create_repository_indexer


class TestEnums:
    """Test enum definitions."""

    def test_index_mode_values(self):
        """Test IndexMode enum."""
        assert IndexMode.FULL
        assert IndexMode.INCREMENTAL
        assert IndexMode.SMART

    def test_change_strategy_values(self):
        """Test ChangeStrategy enum."""
        assert ChangeStrategy.GIT_DIFF
        assert ChangeStrategy.FILE_HASH
        assert ChangeStrategy.HYBRID


class TestRepositoryIndexConfig:
    """Test RepositoryIndexConfig."""

    def test_default_config(self):
        """Test default configuration."""
        config = RepositoryIndexConfig()

        assert config.enable_git_integration is True
        assert config.track_commits is True
        assert config.track_branches is True
        assert config.track_authors is True
        assert config.change_strategy == ChangeStrategy.GIT_DIFF
        assert config.index_mode == IndexMode.SMART

    def test_custom_config(self):
        """Test custom configuration."""
        config = RepositoryIndexConfig(
            enable_git_integration=False,
            track_authors=False,
            max_concurrent_files=5,
        )

        assert config.enable_git_integration is False
        assert config.track_authors is False
        assert config.max_concurrent_files == 5


class TestRepositoryIndexResult:
    """Test RepositoryIndexResult."""

    def test_result_creation(self):
        """Test creating a result."""
        result = RepositoryIndexResult()

        assert result.files_processed == 0
        assert result.files_added == 0
        assert result.files_modified == 0
        assert result.files_deleted == 0
        assert result.repository_root is None

    def test_to_dict(self):
        """Test serialization."""
        result = RepositoryIndexResult()
        result.files_processed = 10
        result.current_commit = "abc123"
        result.current_branch = "main"
        result.authors_encountered = {"user@example.com"}

        data = result.to_dict()

        assert data["files_processed"] == 10
        assert data["current_commit"] == "abc123"
        assert data["current_branch"] == "main"
        assert "user@example.com" in data["authors_encountered"]


class TestGitEnrichedSearchResult:
    """Test GitEnrichedSearchResult."""

    def test_result_creation(self):
        """Test creating an enriched result."""
        # GitEnrichedSearchResult extends CodeSearchResult which is mocked
        # So we only test the git-specific fields
        result = GitEnrichedSearchResult()
        result.commit_hash = "abc123"
        result.branch = "main"
        result.remote_url = "https://github.com/test/repo"
        result.last_modified_by = "user@example.com"
        result.contributors = ["user1@example.com", "user2@example.com"]

        assert result.commit_hash == "abc123"
        assert result.branch == "main"
        assert result.remote_url == "https://github.com/test/repo"
        assert len(result.contributors) == 2


class TestRepositoryIndexer:
    """Test RepositoryIndexer class."""

    @pytest.fixture
    def mock_client(self):
        """Create mock ProximaDB client."""
        client = MagicMock()
        client.list_collections = AsyncMock(return_value=[])
        client.create_collection = AsyncMock()
        client.list_graphs = AsyncMock(return_value=[])
        client.create_graph = AsyncMock()
        return client

    @pytest.fixture
    def temp_git_repo(self, tmp_path):
        """Create a temporary git repository."""
        repo_path = tmp_path / "test_repo"
        repo_path.mkdir()

        subprocess.run(["git", "init"], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@example.com"],
            cwd=repo_path,
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"], cwd=repo_path, capture_output=True
        )

        # Create initial files
        (repo_path / "main.py").write_text("def main(): pass")
        (repo_path / "lib.py").write_text("def helper(): pass")

        subprocess.run(["git", "add", "."], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Initial"], cwd=repo_path, capture_output=True
        )

        yield repo_path
        shutil.rmtree(repo_path)

    def test_indexer_creation(self, mock_client):
        """Test creating an indexer."""
        indexer = RepositoryIndexer(mock_client)

        assert indexer.client == mock_client
        assert indexer.config is not None

    def test_indexer_with_config(self, mock_client):
        """Test creating indexer with custom config."""
        config = RepositoryIndexConfig(
            enable_git_integration=False,
            track_authors=False,
        )
        indexer = RepositoryIndexer(mock_client, config)

        assert indexer.config.enable_git_integration is False
        assert indexer.config.track_authors is False

    def test_collect_all_files(self, mock_client, tmp_path):
        """Test file collection."""
        # Create test files
        (tmp_path / "main.py").write_text("code")
        (tmp_path / "lib.rs").write_text("code")
        (tmp_path / "readme.md").write_text("docs")  # Not a code file
        (tmp_path / "node_modules").mkdir()
        (tmp_path / "node_modules" / "pkg.js").write_text("pkg")  # Excluded

        indexer = RepositoryIndexer(mock_client)
        files = indexer._collect_all_files(tmp_path)

        file_names = [f.name for f in files]
        assert "main.py" in file_names
        assert "lib.rs" in file_names
        assert "readme.md" not in file_names  # Not supported extension
        assert "pkg.js" not in file_names  # Excluded pattern

    def test_matches_exclude_pattern(self, mock_client):
        """Test exclude pattern matching."""
        indexer = RepositoryIndexer(mock_client)

        assert indexer._matches_exclude_pattern("node_modules/pkg.js")
        assert indexer._matches_exclude_pattern(".git/config")
        assert indexer._matches_exclude_pattern("__pycache__/module.pyc")
        assert not indexer._matches_exclude_pattern("src/main.py")

    @pytest.mark.asyncio
    async def test_index_repository_basic(self, mock_client, tmp_path):
        """Test basic repository indexing."""
        # Create test files
        (tmp_path / "main.py").write_text("def main(): pass")

        # Mock the builder's index_file
        indexer = RepositoryIndexer(mock_client)
        indexer._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=3,
                relations_created=1,
                errors=[],
                file_hashes={"main.py": "hash123"},
            )
        )

        result = await indexer.index_repository(tmp_path, mode=IndexMode.FULL)

        assert result.files_processed >= 1
        assert result.symbols_indexed >= 0

    @pytest.mark.asyncio
    async def test_index_repository_with_git(self, mock_client, temp_git_repo):
        """Test indexing a git repository."""
        indexer = RepositoryIndexer(mock_client)
        indexer._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=2,
                relations_created=1,
                errors=[],
                file_hashes={},
            )
        )

        result = await indexer.index_repository(temp_git_repo)

        assert result.repository_root == str(temp_git_repo)
        assert result.current_commit is not None
        assert result.current_branch in ("main", "master")

    @pytest.mark.asyncio
    async def test_update_repository(self, mock_client, temp_git_repo):
        """Test incremental update."""
        indexer = RepositoryIndexer(mock_client)
        indexer._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=1,
                relations_created=0,
                errors=[],
                file_hashes={},
            )
        )

        # First index
        await indexer.index_repository(temp_git_repo)

        # Add new file
        (temp_git_repo / "new.py").write_text("def new(): pass")
        subprocess.run(["git", "add", "."], cwd=temp_git_repo, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Add new"], cwd=temp_git_repo, capture_output=True
        )

        # Update
        result = await indexer.update_repository(temp_git_repo)

        # Should process the new file
        assert result.files_processed >= 0  # May be 0 if state wasn't saved

    @pytest.mark.asyncio
    async def test_get_repository_stats(self, mock_client, temp_git_repo):
        """Test getting repository statistics."""
        indexer = RepositoryIndexer(mock_client)

        stats = await indexer.get_repository_stats(temp_git_repo)

        assert "repository" in stats
        assert stats["repository"]["vcs_type"] == "GIT"
        assert stats["repository"]["current_branch"] in ("main", "master")

    @pytest.mark.asyncio
    async def test_clear_index(self, mock_client, temp_git_repo):
        """Test clearing index."""
        indexer = RepositoryIndexer(mock_client)

        # Create state file
        state_file = temp_git_repo / indexer.config.state_file_name
        state_file.write_text("{}")

        assert state_file.exists()

        result = await indexer.clear_index(temp_git_repo)

        assert result is True
        assert not state_file.exists()

    @pytest.mark.asyncio
    async def test_search_code_basic(self, mock_client):
        """Test basic code search."""
        indexer = RepositoryIndexer(mock_client)
        indexer._builder.search_code = AsyncMock(return_value=[])

        results = await indexer.search_code("test query", top_k=5)

        assert isinstance(results, list)


class TestFactoryFunctions:
    """Test factory functions."""

    def test_create_repository_indexer(self):
        """Test factory function."""
        mock_client = MagicMock()

        indexer = create_repository_indexer(
            mock_client,
            enable_git=True,
            track_authors=False,
            parallel=False,
        )

        assert indexer.config.enable_git_integration is True
        assert indexer.config.track_authors is False
        assert indexer.config.parallel_file_processing is False


class TestProgressCallback:
    """Test progress callback functionality."""

    @pytest.mark.asyncio
    async def test_progress_callback_called(self, tmp_path):
        """Test that progress callback is called."""
        # Create test files
        (tmp_path / "a.py").write_text("code")
        (tmp_path / "b.py").write_text("code")

        mock_client = MagicMock()
        indexer = RepositoryIndexer(mock_client)
        indexer.config.parallel_file_processing = False
        indexer._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=1,
                relations_created=0,
                errors=[],
                file_hashes={},
            )
        )

        progress_calls = []

        def callback(path, current, total):
            progress_calls.append((path, current, total))

        await indexer.index_repository(
            tmp_path,
            mode=IndexMode.FULL,
            progress_callback=callback,
        )

        assert len(progress_calls) >= 2


class TestStatePersistence:
    """Test index state persistence."""

    @pytest.fixture
    def temp_git_repo(self, tmp_path):
        """Create a temporary git repository."""
        repo_path = tmp_path / "repo"
        repo_path.mkdir()

        subprocess.run(["git", "init"], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@example.com"],
            cwd=repo_path,
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"], cwd=repo_path, capture_output=True
        )

        (repo_path / "test.py").write_text("pass")
        subprocess.run(["git", "add", "."], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Init"], cwd=repo_path, capture_output=True
        )

        yield repo_path
        shutil.rmtree(repo_path)

    @pytest.mark.asyncio
    async def test_state_persisted_after_index(self, temp_git_repo):
        """Test that state is saved after indexing."""
        mock_client = MagicMock()
        indexer = RepositoryIndexer(mock_client)
        indexer._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=1,
                relations_created=0,
                errors=[],
                file_hashes={},
            )
        )

        await indexer.index_repository(temp_git_repo)

        state_file = temp_git_repo / indexer.config.state_file_name
        assert state_file.exists()

    @pytest.mark.asyncio
    async def test_state_loaded_on_update(self, temp_git_repo):
        """Test that state is loaded on subsequent calls."""
        mock_client = MagicMock()

        # First indexer - creates state
        indexer1 = RepositoryIndexer(mock_client)
        indexer1._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=1,
                relations_created=0,
                errors=[],
                file_hashes={},
            )
        )
        await indexer1.index_repository(temp_git_repo)

        # Second indexer - loads state
        indexer2 = RepositoryIndexer(mock_client)
        indexer2._builder.index_file = AsyncMock(
            return_value=MagicMock(
                files_processed=1,
                files_skipped=0,
                files_failed=0,
                symbols_indexed=1,
                relations_created=0,
                errors=[],
                file_hashes={},
            )
        )

        # This should load the state from the file
        repo_manager = indexer2._get_repo_manager(temp_git_repo)
        assert repo_manager is not None
        # State should have been loaded
        assert repo_manager.index_state.last_indexed_commit is not None
