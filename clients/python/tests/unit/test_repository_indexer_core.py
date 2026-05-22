from datetime import datetime
from pathlib import Path

import pytest

import proximadb_sdk.repository_indexer as repository_indexer
from proximadb_sdk.code_knowledge import CodeSearchResult, IndexingResult
from proximadb_sdk.repository_indexer import (
    ChangeStrategy,
    GitEnrichedSearchResult,
    IndexMode,
    RepositoryIndexConfig,
    RepositoryIndexer,
    RepositoryIndexResult,
    create_repository_indexer,
    index_repository,
)
from proximadb_sdk.repository_manager import (
    Author,
    ChangeType,
    Commit,
    FileChange,
    IndexState,
    RepositoryInfo,
    VCSType,
)


class FakeBuilder:
    def __init__(self, client, config=None, embedding_provider=None):
        self.client = client
        self.config = config
        self.embedding_provider = embedding_provider
        self.indexed = []

    async def index_file(self, file_path, content):
        self.indexed.append((Path(file_path), content))
        return IndexingResult(
            files_processed=1,
            symbols_indexed=2,
            relations_created=1,
            file_hashes={str(file_path): "builder-hash"},
        )

    async def search_code(self, query, top_k=10, language=None, **kwargs):
        return [
            CodeSearchResult(
                symbol_id="sym-1",
                symbol_type="function",
                fully_qualified_name="pkg.module.fn",
                simple_name="fn",
                source_code="def fn(): pass",
                file_path="/repo/src/main.py",
                start_line=1,
                end_line=1,
                language=language or "python",
                score=0.9,
            )
        ]


class FakeRepoManager:
    def __init__(self, root: Path):
        self.root = root
        self.index_state = IndexState(
            repository_id="repo",
            last_indexed_commit="old",
            indexed_files={"src/old.py": "old-hash"},
        )
        self.author = Author("Ada", "ada@example.com")
        self.commit = Commit(
            hash="a" * 40,
            short_hash="a" * 7,
            author=self.author,
            committer=self.author,
            timestamp=datetime(2026, 5, 22, 12, 0, 0),
            message="commit",
        )
        self.updated_hashes = None

    def get_info(self):
        return RepositoryInfo(
            root_path=self.root,
            vcs_type=VCSType.GIT,
            remote_url="git@example.com:org/repo.git",
            current_branch="main",
            current_commit=self.commit.hash,
            is_dirty=False,
            total_commits=5,
            total_branches=2,
            total_tags=1,
        )

    def get_files_to_reindex(self, filter_code_files=True):
        return [
            FileChange("src/main.py", ChangeType.ADDED),
            FileChange("src/lib.rs", ChangeType.MODIFIED),
        ]

    def get_deleted_files(self):
        return [FileChange("src/old.py", ChangeType.DELETED)]

    def get_changes_since_last_index(self):
        return [
            FileChange("src/main.py", ChangeType.ADDED),
            FileChange("src/lib.rs", ChangeType.MODIFIED),
            FileChange("src/new_name.py", ChangeType.RENAMED, old_path="src/old.py"),
        ]

    def update_index_state(self, indexed_files=None):
        self.updated_hashes = indexed_files
        self.index_state.last_indexed_commit = self.commit.hash
        if indexed_files:
            self.index_state.indexed_files.update(indexed_files)

    def get_recent_commits(self, limit=100):
        return [self.commit]


@pytest.fixture
def patched_builder(monkeypatch):
    monkeypatch.setattr(repository_indexer, "CodeKnowledgeBuilder", FakeBuilder)


def test_repository_index_config_and_result_to_dict():
    config = RepositoryIndexConfig()
    assert config.enable_git_integration is True
    assert config.change_strategy == ChangeStrategy.GIT_DIFF
    assert config.index_mode == IndexMode.SMART
    assert config.index_branches == ["main", "master", "develop"]

    result = RepositoryIndexResult(
        files_processed=2,
        files_skipped=1,
        symbols_indexed=4,
        relations_created=3,
        repository_root="/repo",
        current_commit="new",
        previous_commit="old",
        files_added=1,
        files_modified=1,
        files_deleted=1,
        files_renamed=1,
        authors_encountered={"ada@example.com"},
        commits_in_range=2,
        errors=[{"file": "bad.py"}],
    )

    as_dict = result.to_dict()
    assert as_dict["files_processed"] == 2
    assert as_dict["repository_root"] == "/repo"
    assert as_dict["authors_encountered"] == ["ada@example.com"]
    assert as_dict["commits_in_range"] == 2

    enriched = GitEnrichedSearchResult(
        symbol_id="sym",
        symbol_type="function",
        fully_qualified_name="pkg.fn",
        simple_name="fn",
        source_code="def fn(): pass",
        file_path="/repo/src/main.py",
        start_line=1,
        end_line=1,
        language="python",
        score=1.0,
        commit_hash="abc",
        contributors=["ada@example.com"],
    )
    assert enriched.commit_hash == "abc"
    assert enriched.contributors == ["ada@example.com"]


def test_repo_manager_cache_state_and_file_collection(
    tmp_path, monkeypatch, patched_builder
):
    src = tmp_path / "src"
    src.mkdir()
    keep = src / "main.py"
    keep.write_text("print('ok')\n")
    skip = tmp_path / "node_modules" / "ignore.py"
    skip.parent.mkdir()
    skip.write_text("ignore\n")
    (tmp_path / "notes.txt").write_text("notes\n")

    state = IndexState(repository_id="repo", last_indexed_commit="old")
    (tmp_path / ".proximadb_index_state.json").write_text(
        repository_indexer.json.dumps(state.to_dict())
    )

    fake_manager = FakeRepoManager(tmp_path)
    monkeypatch.setattr(repository_indexer, "is_git_repository", lambda path: True)
    monkeypatch.setattr(
        repository_indexer.RepositoryManager,
        "from_path",
        lambda path, state=None: fake_manager,
    )

    indexer = RepositoryIndexer(object())
    assert indexer._load_index_state(tmp_path) == state
    assert indexer._get_repo_manager(tmp_path) is fake_manager
    assert indexer._get_repo_manager(tmp_path) is fake_manager

    collected = indexer._collect_all_files(tmp_path)
    assert keep in collected
    assert skip not in collected
    assert tmp_path / "notes.txt" not in collected
    assert indexer._matches_exclude_pattern("node_modules/ignore.py") is True
    assert indexer._matches_exclude_pattern("src/main.py") is False

    indexer._save_index_state(tmp_path, fake_manager)
    assert (tmp_path / ".proximadb_index_state.json").exists()

    disabled = RepositoryIndexer(
        object(), RepositoryIndexConfig(enable_git_integration=False)
    )
    assert disabled._get_repo_manager(tmp_path) is None

    (tmp_path / ".proximadb_index_state.json").write_text("{not-json")
    assert indexer._load_index_state(tmp_path) is None


@pytest.mark.asyncio
async def test_index_repository_incremental_parallel_and_search(
    tmp_path, monkeypatch, patched_builder
):
    src = tmp_path / "src"
    src.mkdir()
    (src / "main.py").write_text("print('main')\n")
    (src / "lib.rs").write_text("fn lib() {}\n")
    fake_manager = FakeRepoManager(tmp_path)

    monkeypatch.setattr(
        repository_indexer,
        "get_file_git_info",
        lambda path: {
            "commit_hash": "a" * 40,
            "branch": "main",
            "remote_url": "git@example.com:org/repo.git",
            "authors": [{"email": "ada@example.com"}],
        },
    )

    progress = []
    indexer = RepositoryIndexer(
        object(),
        RepositoryIndexConfig(
            parallel_file_processing=True,
            persist_state=False,
            max_concurrent_files=2,
        ),
    )
    indexer._get_repo_manager = lambda path: fake_manager

    result = await indexer.index_repository(
        tmp_path,
        mode=IndexMode.INCREMENTAL,
        progress_callback=lambda file, current, total: progress.append(
            (file, current, total)
        ),
    )

    assert result.repository_root == str(tmp_path)
    assert result.current_commit == "a" * 40
    assert result.previous_commit == "old"
    assert result.files_processed == 2
    assert result.symbols_indexed == 4
    assert result.relations_created == 2
    assert result.files_added == 1
    assert result.files_modified == 1
    assert result.files_renamed == 1
    assert result.files_deleted == 1
    assert result.authors_encountered == {"ada@example.com"}
    assert result.commits_in_range == 1
    assert fake_manager.updated_hashes
    assert len(progress) == 2

    updated = await indexer.update_repository(tmp_path)
    assert updated.files_processed == 2

    plain_results = await indexer.search_code(
        "find fn", include_git_context=False, language="python"
    )
    assert isinstance(plain_results[0], GitEnrichedSearchResult)
    assert plain_results[0].commit_hash is None

    enriched_results = await indexer.search_code("find fn", include_git_context=True)
    assert enriched_results[0].commit_hash == "a" * 40
    assert enriched_results[0].last_modified_by == "ada@example.com"
    assert enriched_results[0].contributors == ["ada@example.com"]

    stats = await indexer.get_repository_stats(tmp_path)
    assert stats["repository"]["current_branch"] == "main"
    assert stats["index"]["last_indexed_commit"] == "a" * 40
    assert stats["pending_changes"]["files_deleted"] == 1

    missing = RepositoryIndexer(object())
    missing._get_repo_manager = lambda path: None
    assert await missing.get_repository_stats(tmp_path) == {
        "error": "Not a git repository"
    }


@pytest.mark.asyncio
async def test_index_single_file_errors_full_index_clear_and_factories(
    tmp_path, monkeypatch, patched_builder
):
    src = tmp_path / "src"
    src.mkdir()
    good = src / "main.py"
    good.write_text("print('ok')\n")
    unreadable = src / "missing.py"

    indexer = RepositoryIndexer(
        object(),
        RepositoryIndexConfig(parallel_file_processing=False, persist_state=False),
    )
    indexer._get_repo_manager = lambda path: None

    progress = []
    result = await indexer.index_repository(
        tmp_path,
        mode=IndexMode.FULL,
        progress_callback=lambda file, current, total: progress.append(
            (file, current, total)
        ),
    )
    assert result.files_processed == 1
    assert progress == [(str(good), 1, 1)]

    failed_result, failed_hash = await indexer._index_single_file(unreadable, None)
    assert failed_result.files_failed == 1
    assert failed_hash is None

    aggregate = RepositoryIndexResult()
    single = IndexingResult(
        files_processed=1,
        files_failed=1,
        symbols_indexed=3,
        relations_created=2,
        errors=[{"file": "bad"}],
        file_hashes={"bad": "hash"},
    )
    indexer._aggregate_result(aggregate, single)
    assert aggregate.files_processed == 1
    assert aggregate.files_failed == 1
    assert aggregate.symbols_indexed == 3
    assert aggregate.file_hashes == {"bad": "hash"}

    path_str = str(tmp_path.resolve())
    indexer._repo_managers[path_str] = FakeRepoManager(tmp_path)
    state_file = tmp_path / indexer.config.state_file_name
    state_file.write_text("{}")
    assert await indexer.clear_index(tmp_path) is True
    assert path_str not in indexer._repo_managers
    assert not state_file.exists()

    created = create_repository_indexer(
        object(), enable_git=False, track_authors=False, parallel=False
    )
    assert created.config.enable_git_integration is False
    assert created.config.track_authors is False
    assert created.config.parallel_file_processing is False

    async def fake_index_repository(
        self, path, mode=None, force=False, progress_callback=None
    ):
        return RepositoryIndexResult(
            repository_root=str(path), current_branch=mode.name
        )

    monkeypatch.setattr(RepositoryIndexer, "index_repository", fake_index_repository)

    convenience = await index_repository(object(), tmp_path, incremental=False)
    assert convenience.repository_root == str(tmp_path)
    assert convenience.current_branch == "FULL"
