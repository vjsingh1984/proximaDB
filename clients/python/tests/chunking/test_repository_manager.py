"""
Unit tests for Repository Manager module.

This module tests:
- Git repository detection and operations
- Change detection and tracking
- Index state management
- Blame and history operations
"""

import importlib.util
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import pytest

# Load repository_manager directly without going through proximadb package
# This avoids protobuf import issues
src_path = Path(__file__).parent.parent.parent / "src"
sys.path.insert(0, str(src_path))

# Load module directly to avoid __init__.py import chain
spec = importlib.util.spec_from_file_location(
    "repository_manager", str(src_path / "proximadb_sdk" / "repository_manager.py")
)
repo_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(repo_module)

# Extract classes and functions from loaded module
VCSType = repo_module.VCSType
ChangeType = repo_module.ChangeType
BranchType = repo_module.BranchType
Author = repo_module.Author
Commit = repo_module.Commit
Branch = repo_module.Branch
Tag = repo_module.Tag
FileChange = repo_module.FileChange
DiffHunk = repo_module.DiffHunk
FileDiff = repo_module.FileDiff
BlameEntry = repo_module.BlameEntry
RepositoryInfo = repo_module.RepositoryInfo
IndexState = repo_module.IndexState
GitRepository = repo_module.GitRepository
RepositoryManager = repo_module.RepositoryManager
is_git_repository = repo_module.is_git_repository
get_repository_root = repo_module.get_repository_root
get_current_commit_hash = repo_module.get_current_commit_hash
get_file_git_info = repo_module.get_file_git_info
repository_context = repo_module.repository_context


class TestEnums:
    """Test enum definitions."""

    def test_vcs_type_values(self):
        """Test VCSType enum values."""
        assert VCSType.GIT
        assert VCSType.MERCURIAL
        assert VCSType.SVN
        assert VCSType.NONE

    def test_change_type_values(self):
        """Test ChangeType enum values."""
        assert ChangeType.ADDED.value == "A"
        assert ChangeType.MODIFIED.value == "M"
        assert ChangeType.DELETED.value == "D"
        assert ChangeType.RENAMED.value == "R"
        assert ChangeType.COPIED.value == "C"
        assert ChangeType.UNTRACKED.value == "?"

    def test_branch_type_values(self):
        """Test BranchType enum values."""
        assert BranchType.MAIN
        assert BranchType.DEVELOP
        assert BranchType.FEATURE
        assert BranchType.RELEASE
        assert BranchType.HOTFIX
        assert BranchType.OTHER


class TestAuthor:
    """Test Author data class."""

    def test_author_creation(self):
        """Test creating an author."""
        author = Author(name="Test User", email="test@example.com")
        assert author.name == "Test User"
        assert author.email == "test@example.com"

    def test_author_hash(self):
        """Test author hashing for set operations."""
        author1 = Author(name="Test", email="test@example.com")
        author2 = Author(name="Test", email="test@example.com")
        author3 = Author(name="Other", email="other@example.com")

        assert hash(author1) == hash(author2)
        assert hash(author1) != hash(author3)

    def test_author_equality(self):
        """Test author equality."""
        author1 = Author(name="Test", email="test@example.com")
        author2 = Author(name="Test", email="test@example.com")
        author3 = Author(name="Test", email="different@example.com")

        assert author1 == author2
        assert author1 != author3

    def test_author_in_set(self):
        """Test authors in set operations."""
        author1 = Author(name="Test", email="test@example.com")
        author2 = Author(name="Test", email="test@example.com")

        authors = {author1, author2}
        assert len(authors) == 1


class TestCommit:
    """Test Commit data class."""

    def test_commit_creation(self):
        """Test creating a commit."""
        author = Author(name="Test", email="test@example.com")
        commit = Commit(
            hash="abc123def456",
            short_hash="abc123d",
            author=author,
            committer=author,
            timestamp=datetime.now(),
            message="Test commit",
            parent_hashes=["parent123"],
        )
        assert commit.hash == "abc123def456"
        assert commit.short_hash == "abc123d"
        assert commit.message == "Test commit"
        assert not commit.is_merge

    def test_merge_commit(self):
        """Test merge commit detection."""
        author = Author(name="Test", email="test@example.com")
        commit = Commit(
            hash="abc123",
            short_hash="abc123",
            author=author,
            committer=None,
            timestamp=datetime.now(),
            message="Merge branch",
            parent_hashes=["parent1", "parent2"],
        )
        assert commit.is_merge


class TestBranch:
    """Test Branch data class."""

    def test_branch_creation(self):
        """Test creating a branch."""
        branch = Branch(
            name="main",
            commit_hash="abc123",
            is_current=True,
        )
        assert branch.name == "main"
        assert branch.is_current

    def test_branch_classification(self):
        """Test branch type classification."""
        assert Branch.classify("main") == BranchType.MAIN
        assert Branch.classify("master") == BranchType.MAIN
        assert Branch.classify("develop") == BranchType.DEVELOP
        assert Branch.classify("dev") == BranchType.DEVELOP
        assert Branch.classify("feature/new-thing") == BranchType.FEATURE
        assert Branch.classify("feat/stuff") == BranchType.FEATURE
        assert Branch.classify("release/1.0") == BranchType.RELEASE
        assert Branch.classify("hotfix/urgent") == BranchType.HOTFIX
        assert Branch.classify("fix/bug") == BranchType.HOTFIX
        assert Branch.classify("random-branch") == BranchType.OTHER


class TestFileChange:
    """Test FileChange data class."""

    def test_file_change_creation(self):
        """Test creating a file change."""
        change = FileChange(
            path="src/main.py",
            change_type=ChangeType.MODIFIED,
            additions=10,
            deletions=5,
        )
        assert change.path == "src/main.py"
        assert change.change_type == ChangeType.MODIFIED
        assert change.is_code_file

    def test_is_code_file(self):
        """Test code file detection."""
        python = FileChange(path="main.py", change_type=ChangeType.ADDED)
        rust = FileChange(path="lib.rs", change_type=ChangeType.ADDED)
        javascript = FileChange(path="app.js", change_type=ChangeType.ADDED)
        markdown = FileChange(path="README.md", change_type=ChangeType.ADDED)
        config = FileChange(path="config.json", change_type=ChangeType.ADDED)

        assert python.is_code_file
        assert rust.is_code_file
        assert javascript.is_code_file
        assert not markdown.is_code_file
        assert not config.is_code_file


class TestFileDiff:
    """Test FileDiff data class."""

    def test_file_diff_creation(self):
        """Test creating a file diff."""
        diff = FileDiff(
            path="src/main.py",
            old_path=None,
            change_type=ChangeType.MODIFIED,
            hunks=[],
        )
        assert diff.path == "src/main.py"
        assert diff.total_additions == 0
        assert diff.total_deletions == 0

    def test_diff_with_hunks(self):
        """Test diff with hunks."""
        hunk = DiffHunk(
            old_start=1,
            old_count=3,
            new_start=1,
            new_count=5,
            content="-old line\n+new line 1\n+new line 2\n context",
        )
        diff = FileDiff(
            path="test.py",
            old_path=None,
            change_type=ChangeType.MODIFIED,
            hunks=[hunk],
        )
        assert diff.total_additions == 2
        assert diff.total_deletions == 1


class TestIndexState:
    """Test IndexState data class."""

    def test_index_state_creation(self):
        """Test creating index state."""
        state = IndexState(
            repository_id="test123",
            last_indexed_commit="abc123",
        )
        assert state.repository_id == "test123"
        assert state.last_indexed_commit == "abc123"

    def test_to_dict(self):
        """Test serialization to dict."""
        state = IndexState(
            repository_id="test123",
            last_indexed_commit="abc123",
            last_indexed_time=datetime(2025, 1, 1, 12, 0, 0),
            indexed_files={"main.py": "hash123"},
            branch_states={"main": "abc123"},
        )
        data = state.to_dict()

        assert data["repository_id"] == "test123"
        assert data["last_indexed_commit"] == "abc123"
        assert data["indexed_files"] == {"main.py": "hash123"}

    def test_from_dict(self):
        """Test deserialization from dict."""
        data = {
            "repository_id": "test123",
            "last_indexed_commit": "abc123",
            "last_indexed_time": "2025-01-01T12:00:00",
            "indexed_files": {"main.py": "hash123"},
            "branch_states": {"main": "abc123"},
        }
        state = IndexState.from_dict(data)

        assert state.repository_id == "test123"
        assert state.last_indexed_commit == "abc123"
        assert state.last_indexed_time == datetime(2025, 1, 1, 12, 0, 0)


class TestRepositoryInfo:
    """Test RepositoryInfo data class."""

    def test_repository_info_creation(self):
        """Test creating repository info."""
        info = RepositoryInfo(
            root_path=Path("/tmp/repo"),
            vcs_type=VCSType.GIT,
            remote_url="https://github.com/test/repo",
            current_branch="main",
            current_commit="abc123",
        )
        assert info.vcs_type == VCSType.GIT
        assert info.current_branch == "main"

    def test_to_dict(self):
        """Test serialization to dict."""
        info = RepositoryInfo(
            root_path=Path("/tmp/repo"),
            vcs_type=VCSType.GIT,
        )
        data = info.to_dict()
        assert data["vcs_type"] == "GIT"
        assert data["root_path"] == "/tmp/repo"


class TestGitRepository:
    """Test GitRepository class with a real repository."""

    @pytest.fixture
    def temp_git_repo(self, tmp_path):
        """Create a temporary git repository for testing."""
        repo_path = tmp_path / "test_repo"
        repo_path.mkdir()

        # Initialize git repo
        subprocess.run(["git", "init"], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@example.com"],
            cwd=repo_path,
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test User"],
            cwd=repo_path,
            capture_output=True,
        )

        # Create initial commit
        test_file = repo_path / "test.py"
        test_file.write_text("print('hello')")
        subprocess.run(["git", "add", "."], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Initial commit"],
            cwd=repo_path,
            capture_output=True,
        )

        yield repo_path

        # Cleanup
        shutil.rmtree(repo_path)

    def test_detect_git_repo(self, temp_git_repo):
        """Test detecting a git repository."""
        repo = GitRepository(temp_git_repo)
        assert repo.vcs_type == VCSType.GIT
        assert repo.get_root() == temp_git_repo

    def test_get_current_commit(self, temp_git_repo):
        """Test getting current commit."""
        repo = GitRepository(temp_git_repo)
        commit = repo.get_current_commit()

        assert commit is not None
        assert len(commit.hash) == 40
        assert commit.message.strip() == "Initial commit"
        assert commit.author.name == "Test User"

    def test_get_current_branch(self, temp_git_repo):
        """Test getting current branch."""
        repo = GitRepository(temp_git_repo)
        branch = repo.get_current_branch()

        # Branch name depends on git config (main or master)
        assert branch in ("main", "master")

    def test_get_branches(self, temp_git_repo):
        """Test getting branches."""
        repo = GitRepository(temp_git_repo)
        branches = repo.get_branches()

        assert len(branches) >= 1
        current = [b for b in branches if b.is_current]
        assert len(current) == 1

    def test_is_dirty_clean(self, temp_git_repo):
        """Test dirty detection on clean repo."""
        repo = GitRepository(temp_git_repo)
        assert not repo.is_dirty()

    def test_is_dirty_with_changes(self, temp_git_repo):
        """Test dirty detection with uncommitted changes."""
        repo = GitRepository(temp_git_repo)

        # Make a change
        test_file = temp_git_repo / "test.py"
        test_file.write_text("print('modified')")

        assert repo.is_dirty()

    def test_get_changed_files_uncommitted(self, temp_git_repo):
        """Test getting uncommitted changes."""
        repo = GitRepository(temp_git_repo)

        # Modify existing file
        test_file = temp_git_repo / "test.py"
        test_file.write_text("print('modified')")

        # Add new file
        new_file = temp_git_repo / "new.py"
        new_file.write_text("print('new')")

        changes = repo.get_changed_files(include_untracked=True)

        paths = {c.path for c in changes}
        assert "test.py" in paths
        assert "new.py" in paths

    def test_get_file_content_at_ref(self, temp_git_repo):
        """Test getting file content at ref."""
        repo = GitRepository(temp_git_repo)

        content = repo.get_file_content("test.py", "HEAD")
        assert content == "print('hello')"

    def test_not_a_git_repo(self, tmp_path):
        """Test error on non-git directory."""
        with pytest.raises(ValueError, match="Not a git repository"):
            GitRepository(tmp_path)

    def test_get_commits(self, temp_git_repo):
        """Test getting commits."""
        repo = GitRepository(temp_git_repo)

        # Add another commit
        test_file = temp_git_repo / "test.py"
        test_file.write_text("print('updated')")
        subprocess.run(["git", "add", "."], cwd=temp_git_repo, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Second commit"],
            cwd=temp_git_repo,
            capture_output=True,
        )

        commits = repo.get_commits(limit=10)
        assert len(commits) == 2
        assert commits[0].message.strip() == "Second commit"
        assert commits[1].message.strip() == "Initial commit"

    def test_get_file_history(self, temp_git_repo):
        """Test getting file history."""
        repo = GitRepository(temp_git_repo)

        # Modify and commit
        test_file = temp_git_repo / "test.py"
        test_file.write_text("print('v2')")
        subprocess.run(["git", "add", "."], cwd=temp_git_repo, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Update test.py"],
            cwd=temp_git_repo,
            capture_output=True,
        )

        history = repo.get_file_history("test.py")
        assert len(history) == 2


class TestRepositoryManager:
    """Test RepositoryManager class."""

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
            ["git", "config", "user.name", "Test User"],
            cwd=repo_path,
            capture_output=True,
        )

        test_file = repo_path / "main.py"
        test_file.write_text("def main(): pass")
        subprocess.run(["git", "add", "."], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Initial"], cwd=repo_path, capture_output=True
        )

        yield repo_path
        shutil.rmtree(repo_path)

    def test_from_path(self, temp_git_repo):
        """Test creating manager from path."""
        manager = RepositoryManager.from_path(temp_git_repo)
        assert manager.vcs_type == VCSType.GIT
        assert manager.root == temp_git_repo

    def test_detect_vcs(self, temp_git_repo, tmp_path):
        """Test VCS detection."""
        assert RepositoryManager.detect_vcs(temp_git_repo) == VCSType.GIT
        assert RepositoryManager.detect_vcs(tmp_path) == VCSType.NONE

    def test_get_info(self, temp_git_repo):
        """Test getting repository info."""
        manager = RepositoryManager.from_path(temp_git_repo)
        info = manager.get_info()

        assert info.vcs_type == VCSType.GIT
        assert info.current_commit is not None
        assert not info.is_dirty

    def test_get_changes_first_index(self, temp_git_repo):
        """Test getting changes for first index."""
        manager = RepositoryManager.from_path(temp_git_repo)

        # First index - should return all files
        changes = manager.get_changes_since_last_index()

        assert len(changes) == 1
        assert changes[0].path == "main.py"
        assert changes[0].change_type == ChangeType.ADDED

    def test_get_changes_incremental(self, temp_git_repo):
        """Test incremental change detection."""
        manager = RepositoryManager.from_path(temp_git_repo)

        # Mark initial state as indexed
        commit = manager.get_commit_info()
        manager.update_index_state(commit.hash)

        # Add a new file and commit
        new_file = temp_git_repo / "new.py"
        new_file.write_text("print('new')")
        subprocess.run(["git", "add", "."], cwd=temp_git_repo, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Add new file"],
            cwd=temp_git_repo,
            capture_output=True,
        )

        # Get changes since last index
        changes = manager.get_changes_since_last_index()

        assert len(changes) == 1
        assert changes[0].path == "new.py"

    def test_get_files_to_reindex(self, temp_git_repo):
        """Test filtering files to reindex."""
        manager = RepositoryManager.from_path(temp_git_repo)

        # Add multiple files
        (temp_git_repo / "code.py").write_text("code")
        (temp_git_repo / "readme.md").write_text("readme")
        (temp_git_repo / "app.js").write_text("js")

        subprocess.run(["git", "add", "."], cwd=temp_git_repo, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Add files"], cwd=temp_git_repo, capture_output=True
        )

        # First index returns all files
        changes = manager.get_files_to_reindex(filter_code_files=True)
        paths = {c.path for c in changes}

        assert "code.py" in paths or "main.py" in paths
        assert "app.js" in paths
        assert "readme.md" not in paths  # Not a code file

    def test_filter_by_extension(self, temp_git_repo):
        """Test filtering by extension."""
        manager = RepositoryManager.from_path(temp_git_repo)

        # Add files
        (temp_git_repo / "a.py").write_text("py")
        (temp_git_repo / "b.rs").write_text("rs")
        (temp_git_repo / "c.js").write_text("js")

        subprocess.run(["git", "add", "."], cwd=temp_git_repo, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Add"], cwd=temp_git_repo, capture_output=True
        )

        changes = manager.get_files_to_reindex(extensions={".py", ".rs"})
        paths = {c.path for c in changes}

        assert "a.py" in paths or "main.py" in paths
        assert "b.rs" in paths
        assert "c.js" not in paths

    def test_update_index_state(self, temp_git_repo):
        """Test updating index state."""
        manager = RepositoryManager.from_path(temp_git_repo)

        manager.update_index_state(indexed_files={"main.py": "hash123"})

        assert manager.index_state.last_indexed_commit is not None
        assert manager.index_state.last_indexed_time is not None
        assert manager.index_state.indexed_files == {"main.py": "hash123"}

    def test_save_and_load_state(self, temp_git_repo, tmp_path):
        """Test saving and loading index state."""
        manager = RepositoryManager.from_path(temp_git_repo)
        state_file = tmp_path / "state.json"

        # Update and save state
        manager.update_index_state(indexed_files={"main.py": "hash1"})
        manager.save_state(state_file)

        assert state_file.exists()

        # Load state in new manager
        loaded_state = RepositoryManager.load_state(state_file)
        assert loaded_state is not None
        assert loaded_state.indexed_files == {"main.py": "hash1"}


class TestUtilityFunctions:
    """Test utility functions."""

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

        test_file = repo_path / "test.py"
        test_file.write_text("pass")
        subprocess.run(["git", "add", "."], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Init"], cwd=repo_path, capture_output=True
        )

        yield repo_path
        shutil.rmtree(repo_path)

    def test_is_git_repository(self, temp_git_repo, tmp_path):
        """Test is_git_repository function."""
        assert is_git_repository(temp_git_repo)
        assert not is_git_repository(tmp_path)

    def test_get_repository_root(self, temp_git_repo, tmp_path):
        """Test get_repository_root function."""
        root = get_repository_root(temp_git_repo)
        assert root == temp_git_repo

        root = get_repository_root(tmp_path)
        assert root is None

    def test_get_current_commit_hash(self, temp_git_repo):
        """Test get_current_commit_hash function."""
        commit_hash = get_current_commit_hash(temp_git_repo)
        assert commit_hash is not None
        assert len(commit_hash) == 40

    def test_get_file_git_info(self, temp_git_repo):
        """Test get_file_git_info function."""
        file_path = temp_git_repo / "test.py"
        info = get_file_git_info(file_path)

        assert info is not None
        assert info["relative_path"] == "test.py"
        assert info["commit_hash"] is not None
        assert info["branch"] in ("main", "master")


class TestRepositoryContext:
    """Test repository_context context manager."""

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

        test_file = repo_path / "main.py"
        test_file.write_text("code")
        subprocess.run(["git", "add", "."], cwd=repo_path, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "Init"], cwd=repo_path, capture_output=True
        )

        yield repo_path
        shutil.rmtree(repo_path)

    def test_context_manager_basic(self, temp_git_repo):
        """Test basic context manager usage."""
        with repository_context(temp_git_repo) as repo:
            assert repo.vcs_type == VCSType.GIT
            info = repo.get_info()
            assert info.current_commit is not None

    def test_context_manager_with_state_file(self, temp_git_repo, tmp_path):
        """Test context manager with state file."""
        state_file = tmp_path / "state.json"

        # First run - creates state
        with repository_context(temp_git_repo, state_file) as repo:
            repo.update_index_state(indexed_files={"main.py": "hash1"})

        assert state_file.exists()

        # Second run - loads state
        with repository_context(temp_git_repo, state_file) as repo:
            assert repo.index_state.indexed_files == {"main.py": "hash1"}
