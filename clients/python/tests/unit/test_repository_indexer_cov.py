"""Offline unit tests for proximadb_sdk.repository_indexer.

Fully mocked: no real git, no network, no embedding model, no real builder.
We construct a RepositoryIndexer with a MagicMock client and replace its
internal CodeKnowledgeBuilder with an async stub. Repository managers and the
git utility functions are monkeypatched so nothing ever shells out to git.
"""

import asyncio
import json
from datetime import datetime
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest

import proximadb_sdk.repository_indexer as ri
from proximadb_sdk.code_knowledge import IndexingResult
from proximadb_sdk.repository_indexer import (
    ChangeStrategy,
    GitEnrichedSearchResult,
    IndexMode,
    RepositoryIndexConfig,
    RepositoryIndexer,
    RepositoryIndexResult,
    create_repository_indexer,
)
from proximadb_sdk.repository_manager import (
    Author,
    ChangeType,
    Commit,
    FileChange,
    IndexState,
)


# ---------------------------------------------------------------------------
# Helpers / fixtures
# ---------------------------------------------------------------------------


def _indexing_result(processed=1, symbols=2, failed=0, errors=None):
    r = IndexingResult()
    r.files_processed = processed
    r.symbols_indexed = symbols
    r.files_failed = failed
    if errors:
        r.errors = list(errors)
    return r


def _make_indexer(config=None):
    """Build an indexer with a fully stubbed builder."""
    client = MagicMock(name="client")
    idx = RepositoryIndexer(client, config=config)
    # Replace the real builder with async stubs
    idx._builder = MagicMock(name="builder")
    idx._builder.index_file = AsyncMock(return_value=_indexing_result())
    idx._builder.search_code = AsyncMock(return_value=[])
    return idx


def _fake_repo_manager(root, commit="abc123", branch="main", last_indexed=None):
    rm = MagicMock(name="repo_manager")
    rm.root = Path(root)
    info = MagicMock()
    info.current_commit = commit
    info.current_branch = branch
    info.root_path = Path(root)
    info.vcs_type = MagicMock()
    info.vcs_type.name = "GIT"
    info.remote_url = "git@example.com:repo.git"
    info.is_dirty = False
    info.total_commits = 5
    info.total_branches = 2
    rm.get_info.return_value = info

    state = IndexState(repository_id="rid", last_indexed_commit=last_indexed)
    state.last_indexed_time = datetime(2025, 1, 1, 12, 0, 0)
    state.indexed_files = {"a.py": "h1"}
    state.branch_states = {"main": commit}
    rm.index_state = state

    rm.get_changes_since_last_index.return_value = []
    rm.get_files_to_reindex.return_value = []
    rm.get_deleted_files.return_value = []
    rm.get_recent_commits.return_value = []
    return rm


# ---------------------------------------------------------------------------
# Dataclasses / config
# ---------------------------------------------------------------------------


def test_config_defaults():
    cfg = RepositoryIndexConfig()
    assert cfg.enable_git_integration is True
    assert cfg.change_strategy is ChangeStrategy.GIT_DIFF
    assert cfg.index_mode is IndexMode.SMART
    assert "main" in cfg.index_branches


def test_repository_index_result_to_dict():
    res = RepositoryIndexResult()
    res.files_processed = 3
    res.symbols_indexed = 7
    res.repository_root = "/repo"
    res.current_commit = "deadbeef"
    res.files_added = 1
    res.authors_encountered = {"a@x.com"}
    res.commits_in_range = 4
    d = res.to_dict()
    assert d["files_processed"] == 3
    assert d["symbols_indexed"] == 7
    assert d["repository_root"] == "/repo"
    assert d["authors_encountered"] == ["a@x.com"]
    assert d["commits_in_range"] == 4
    assert "errors" in d


def test_git_enriched_search_result_defaults():
    r = GitEnrichedSearchResult(
        symbol_id="s1",
        symbol_type="function",
        fully_qualified_name="m.f",
        simple_name="f",
        source_code="def f(): pass",
        file_path="/repo/m.py",
        start_line=1,
        end_line=1,
        language="python",
        score=0.9,
    )
    assert r.commit_hash is None
    assert r.contributors == []
    assert r.total_commits == 0


# ---------------------------------------------------------------------------
# _matches_exclude_pattern / _collect_all_files
# ---------------------------------------------------------------------------


def test_matches_exclude_pattern():
    idx = _make_indexer()
    assert idx._matches_exclude_pattern("foo.pyc") is True
    assert idx._matches_exclude_pattern("src/main.py") is False


def test_collect_all_files(tmp_path):
    idx = _make_indexer()
    (tmp_path / "a.py").write_text("x = 1\n")
    (tmp_path / "b.txt").write_text("ignore me")  # unsupported ext
    (tmp_path / "junk.pyc").write_text("nope")  # excluded
    # supported extension (.js) but matches an exclude pattern (*.min.js)
    (tmp_path / "bundle.min.js").write_text("var x=1;")
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "c.py").write_text("y = 2\n")

    files = idx._collect_all_files(tmp_path)
    names = {f.name for f in files}
    assert "a.py" in names
    assert "c.py" in names
    assert "b.txt" not in names
    assert "junk.pyc" not in names
    assert "bundle.min.js" not in names  # excluded by *.min.js pattern


# ---------------------------------------------------------------------------
# _get_repo_manager branches
# ---------------------------------------------------------------------------


def test_get_repo_manager_git_disabled(tmp_path):
    cfg = RepositoryIndexConfig(enable_git_integration=False)
    idx = _make_indexer(cfg)
    assert idx._get_repo_manager(tmp_path) is None


def test_get_repo_manager_not_a_repo(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: False)
    assert idx._get_repo_manager(tmp_path) is None


def test_get_repo_manager_success_and_cache(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)
    rm = _fake_repo_manager(tmp_path)
    monkeypatch.setattr(ri.RepositoryManager, "from_path", lambda p, s: rm)

    got = idx._get_repo_manager(tmp_path)
    assert got is rm
    # second call hits the cache (from_path would raise if called again wrongly)
    monkeypatch.setattr(
        ri.RepositoryManager,
        "from_path",
        lambda p, s: (_ for _ in ()).throw(AssertionError("should be cached")),
    )
    assert idx._get_repo_manager(tmp_path) is rm


def test_get_repo_manager_init_failure(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)

    def boom(p, s):
        raise RuntimeError("git busted")

    monkeypatch.setattr(ri.RepositoryManager, "from_path", boom)
    assert idx._get_repo_manager(tmp_path) is None


# ---------------------------------------------------------------------------
# load / save index state
# ---------------------------------------------------------------------------


def test_load_index_state_missing(tmp_path):
    idx = _make_indexer()
    assert idx._load_index_state(tmp_path) is None


def test_load_index_state_valid(tmp_path):
    idx = _make_indexer()
    state = IndexState(repository_id="rid", last_indexed_commit="c1")
    (tmp_path / idx.config.state_file_name).write_text(json.dumps(state.to_dict()))
    loaded = idx._load_index_state(tmp_path)
    assert loaded is not None
    assert loaded.last_indexed_commit == "c1"


def test_load_index_state_corrupt(tmp_path):
    idx = _make_indexer()
    (tmp_path / idx.config.state_file_name).write_text("{not json")
    assert idx._load_index_state(tmp_path) is None


def test_save_index_state_disabled(tmp_path):
    cfg = RepositoryIndexConfig(persist_state=False)
    idx = _make_indexer(cfg)
    rm = _fake_repo_manager(tmp_path)
    idx._save_index_state(tmp_path, rm)
    assert not (tmp_path / idx.config.state_file_name).exists()


def test_save_index_state_writes(tmp_path):
    idx = _make_indexer()
    rm = _fake_repo_manager(tmp_path)
    idx._save_index_state(tmp_path, rm)
    sf = tmp_path / idx.config.state_file_name
    assert sf.exists()
    data = json.loads(sf.read_text())
    assert data["repository_id"] == "rid"


def test_save_index_state_error_swallowed(monkeypatch):
    idx = _make_indexer()
    rm = _fake_repo_manager("/repo")
    # Path that cannot be written -> write_text raises, must be swallowed
    bad = Path("/nonexistent_dir_xyz/deeper")
    idx._save_index_state(bad, rm)  # no exception


# ---------------------------------------------------------------------------
# _aggregate_result
# ---------------------------------------------------------------------------


def test_aggregate_result():
    idx = _make_indexer()
    agg = RepositoryIndexResult()
    single = _indexing_result(processed=2, symbols=3, failed=1, errors=[{"e": 1}])
    single.file_hashes = {"x.py": "h"}
    idx._aggregate_result(agg, single)
    assert agg.files_processed == 2
    assert agg.symbols_indexed == 3
    assert agg.files_failed == 1
    assert agg.errors == [{"e": 1}]
    assert agg.file_hashes == {"x.py": "h"}


# ---------------------------------------------------------------------------
# _index_single_file
# ---------------------------------------------------------------------------


def test_index_single_file_read_error(tmp_path):
    idx = _make_indexer()
    missing = tmp_path / "ghost.py"  # does not exist
    result, content_hash = asyncio.run(idx._index_single_file(missing, None))
    assert result.files_failed == 1
    assert content_hash is None
    assert result.errors


def test_index_single_file_no_repo(tmp_path):
    idx = _make_indexer()
    f = tmp_path / "ok.py"
    f.write_text("a = 1\n")
    result, content_hash = asyncio.run(idx._index_single_file(f, None))
    assert content_hash is not None
    assert result.files_processed == 1
    idx._builder.index_file.assert_awaited()


def test_index_single_file_with_git_metadata(tmp_path, monkeypatch):
    idx = _make_indexer()
    f = tmp_path / "ok.py"
    f.write_text("a = 1\n")
    rm = _fake_repo_manager(tmp_path)
    monkeypatch.setattr(
        ri,
        "get_file_git_info",
        lambda p: {
            "commit_hash": "c1",
            "branch": "main",
            "remote_url": "url",
            "authors": [{"email": "a@x.com"}, {"email": "b@x.com"}],
        },
    )
    result, content_hash = asyncio.run(idx._index_single_file(f, rm))
    assert content_hash is not None
    assert result.files_processed == 1


def test_index_single_file_git_info_exception(tmp_path, monkeypatch):
    idx = _make_indexer()
    f = tmp_path / "ok.py"
    f.write_text("a = 1\n")
    rm = _fake_repo_manager(tmp_path)

    def boom(p):
        raise RuntimeError("git info failed")

    monkeypatch.setattr(ri, "get_file_git_info", boom)
    # exception is swallowed; file still indexed
    result, content_hash = asyncio.run(idx._index_single_file(f, rm))
    assert content_hash is not None
    assert result.files_processed == 1


# ---------------------------------------------------------------------------
# _handle_deleted_files
# ---------------------------------------------------------------------------


def test_handle_deleted_files():
    idx = _make_indexer()
    res = RepositoryIndexResult()
    deleted = [
        FileChange(path="gone1.py", change_type=ChangeType.DELETED),
        FileChange(path="gone2.py", change_type=ChangeType.DELETED),
    ]
    asyncio.run(idx._handle_deleted_files(deleted, res))
    assert res.files_deleted == 2


# ---------------------------------------------------------------------------
# _get_changed_files / _get_deleted_files
# ---------------------------------------------------------------------------


def test_get_changed_files_maps_to_abs(tmp_path):
    idx = _make_indexer()
    rm = _fake_repo_manager(tmp_path)
    rm.get_files_to_reindex.return_value = [
        FileChange(path="src/a.py", change_type=ChangeType.MODIFIED),
    ]
    out = asyncio.run(idx._get_changed_files(rm))
    assert out == [tmp_path / "src/a.py"]


def test_get_deleted_files():
    idx = _make_indexer()
    rm = _fake_repo_manager("/repo")
    rm.get_deleted_files.return_value = [
        FileChange(path="d.py", change_type=ChangeType.DELETED)
    ]
    out = asyncio.run(idx._get_deleted_files(rm))
    assert len(out) == 1


# ---------------------------------------------------------------------------
# _index_files_parallel
# ---------------------------------------------------------------------------


def test_index_files_parallel(tmp_path):
    idx = _make_indexer()
    files = []
    for i in range(3):
        f = tmp_path / f"f{i}.py"
        f.write_text(f"x = {i}\n")
        files.append(f)

    seen = []

    def cb(path, done, total):
        seen.append((done, total))

    results = asyncio.run(idx._index_files_parallel(files, None, cb))
    assert len(results) == 3
    assert all(r[2] is not None for r in results)  # content hashes present
    assert len(seen) == 3


# ---------------------------------------------------------------------------
# index_repository — full / incremental / smart / fallback
# ---------------------------------------------------------------------------


def test_index_repository_full_mode_no_repo(tmp_path, monkeypatch):
    # git disabled => repo_manager None => fallback collects all files
    cfg = RepositoryIndexConfig(enable_git_integration=False)
    idx = _make_indexer(cfg)
    (tmp_path / "a.py").write_text("a = 1\n")
    (tmp_path / "b.py").write_text("b = 2\n")
    res = asyncio.run(idx.index_repository(tmp_path, mode=IndexMode.FULL))
    assert res.files_processed == 2
    assert res.repository_root is None


def test_index_repository_incremental_no_manager_fallback(tmp_path, monkeypatch):
    # git enabled but path is not a repo -> repo_manager None; mode INCREMENTAL
    # hits the final else fallback (collect all files).
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: False)
    (tmp_path / "a.py").write_text("a = 1\n")
    res = asyncio.run(idx.index_repository(tmp_path, mode=IndexMode.INCREMENTAL))
    assert res.files_processed == 1
    assert res.repository_root is None


def test_handle_deleted_files_exception(monkeypatch):
    idx = _make_indexer()
    res = RepositoryIndexResult()

    # Make logger.info raise so the except branch records an error.
    def boom(*a, **k):
        raise RuntimeError("log blew up")

    monkeypatch.setattr(ri.logger, "info", boom)
    deleted = [FileChange(path="gone.py", change_type=ChangeType.DELETED)]
    asyncio.run(idx._handle_deleted_files(deleted, res))
    assert res.errors
    assert res.errors[0]["file"] == "gone.py"


def test_index_repository_sequential(tmp_path, monkeypatch):
    cfg = RepositoryIndexConfig(
        enable_git_integration=False, parallel_file_processing=False
    )
    idx = _make_indexer(cfg)
    (tmp_path / "a.py").write_text("a = 1\n")
    calls = []
    res = asyncio.run(
        idx.index_repository(
            tmp_path,
            mode=IndexMode.FULL,
            progress_callback=lambda p, i, t: calls.append((i, t)),
        )
    )
    assert res.files_processed == 1
    assert calls == [(1, 1)]


def test_index_repository_incremental_with_repo(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)
    rm = _fake_repo_manager(tmp_path, last_indexed="oldsha")
    # changed files (one of each classified type)
    changed = tmp_path / "changed.py"
    changed.write_text("c = 1\n")
    rm.get_files_to_reindex.return_value = [
        FileChange(path="changed.py", change_type=ChangeType.MODIFIED)
    ]
    rm.get_deleted_files.return_value = [
        FileChange(path="del.py", change_type=ChangeType.DELETED)
    ]
    rm.get_changes_since_last_index.return_value = [
        FileChange(path="added.py", change_type=ChangeType.ADDED),
        FileChange(path="changed.py", change_type=ChangeType.MODIFIED),
        FileChange(path="ren.py", change_type=ChangeType.RENAMED),
    ]
    rm.get_recent_commits.return_value = [
        Commit(
            hash="h1",
            short_hash="h1",
            author=Author("Ann", "ann@x.com"),
            committer=None,
            timestamp=datetime(2025, 1, 1),
            message="m",
        )
    ]
    monkeypatch.setattr(ri.RepositoryManager, "from_path", lambda p, s: rm)

    res = asyncio.run(idx.index_repository(tmp_path, mode=IndexMode.INCREMENTAL))
    assert res.repository_root == str(tmp_path)
    assert res.current_commit == "abc123"
    assert res.previous_commit == "oldsha"
    assert res.files_added == 1
    assert res.files_modified == 1
    assert res.files_renamed == 1
    assert res.files_deleted == 1
    assert res.authors_encountered == {"ann@x.com"}
    assert res.commits_in_range == 1
    rm.update_index_state.assert_called_once()


def test_index_repository_force_with_repo(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)
    rm = _fake_repo_manager(tmp_path)
    monkeypatch.setattr(ri.RepositoryManager, "from_path", lambda p, s: rm)
    (tmp_path / "a.py").write_text("a = 1\n")
    res = asyncio.run(idx.index_repository(tmp_path, force=True))
    # force collects all files even though repo manager exists
    assert res.files_processed == 1
    assert res.repository_root == str(tmp_path)


def test_index_repository_author_collection_exception(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)
    rm = _fake_repo_manager(tmp_path)
    rm.get_recent_commits.side_effect = RuntimeError("no log")
    monkeypatch.setattr(ri.RepositoryManager, "from_path", lambda p, s: rm)
    (tmp_path / "a.py").write_text("a = 1\n")
    res = asyncio.run(idx.index_repository(tmp_path, force=True))
    # exception swallowed; still completes
    assert res.authors_encountered == set()


def test_update_repository(tmp_path, monkeypatch):
    idx = _make_indexer()
    captured = {}

    async def fake_index(path, mode=None, progress_callback=None):
        captured["mode"] = mode
        return RepositoryIndexResult()

    monkeypatch.setattr(idx, "index_repository", fake_index)
    asyncio.run(idx.update_repository(tmp_path))
    assert captured["mode"] is IndexMode.INCREMENTAL


# ---------------------------------------------------------------------------
# search_code
# ---------------------------------------------------------------------------


def _base_result(path="/repo/m.py"):
    from proximadb_sdk.code_knowledge import CodeSearchResult

    return CodeSearchResult(
        symbol_id="s1",
        symbol_type="function",
        fully_qualified_name="m.f",
        simple_name="f",
        source_code="def f(): pass",
        file_path=path,
        start_line=1,
        end_line=2,
        language="python",
        score=0.5,
    )


def test_search_code_no_git_context():
    idx = _make_indexer()
    idx._builder.search_code = AsyncMock(return_value=[_base_result()])
    out = asyncio.run(idx.search_code("q", include_git_context=False))
    assert len(out) == 1
    assert isinstance(out[0], GitEnrichedSearchResult)
    assert out[0].commit_hash is None


def test_search_code_with_git_context(monkeypatch):
    idx = _make_indexer()
    idx._builder.search_code = AsyncMock(return_value=[_base_result()])
    monkeypatch.setattr(
        ri,
        "get_file_git_info",
        lambda p: {
            "commit_hash": "c9",
            "branch": "dev",
            "remote_url": "u",
            "authors": [{"email": "a@x.com"}, {"email": "b@x.com"}],
        },
    )
    out = asyncio.run(idx.search_code("q", include_git_context=True))
    assert out[0].commit_hash == "c9"
    assert out[0].branch == "dev"
    assert out[0].last_modified_by == "a@x.com"
    assert out[0].contributors == ["a@x.com", "b@x.com"]


def test_search_code_git_info_exception(monkeypatch):
    idx = _make_indexer()
    idx._builder.search_code = AsyncMock(return_value=[_base_result()])

    def boom(p):
        raise RuntimeError("fail")

    monkeypatch.setattr(ri, "get_file_git_info", boom)
    out = asyncio.run(idx.search_code("q", include_git_context=True))
    assert out[0].commit_hash is None  # enrichment failed silently


def test_search_code_git_info_none(monkeypatch):
    idx = _make_indexer()
    idx._builder.search_code = AsyncMock(return_value=[_base_result()])
    monkeypatch.setattr(ri, "get_file_git_info", lambda p: None)
    out = asyncio.run(idx.search_code("q", include_git_context=True))
    assert out[0].commit_hash is None


# ---------------------------------------------------------------------------
# get_repository_stats
# ---------------------------------------------------------------------------


def test_get_repository_stats_not_a_repo(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: False)
    out = asyncio.run(idx.get_repository_stats(tmp_path))
    assert out == {"error": "Not a git repository"}


def test_get_repository_stats_success(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)
    rm = _fake_repo_manager(tmp_path)
    rm.get_files_to_reindex.return_value = [
        FileChange(path="a.py", change_type=ChangeType.MODIFIED)
    ]
    rm.get_deleted_files.return_value = []
    monkeypatch.setattr(ri.RepositoryManager, "from_path", lambda p, s: rm)
    out = asyncio.run(idx.get_repository_stats(tmp_path))
    assert out["repository"]["vcs_type"] == "GIT"
    assert out["repository"]["current_commit"] == "abc123"
    assert out["index"]["indexed_files_count"] == 1
    assert out["index"]["last_indexed_time"] is not None
    assert out["pending_changes"]["files_to_reindex"] == 1


def test_get_repository_stats_no_indexed_time(tmp_path, monkeypatch):
    idx = _make_indexer()
    monkeypatch.setattr(ri, "is_git_repository", lambda p: True)
    rm = _fake_repo_manager(tmp_path)
    rm.index_state.last_indexed_time = None
    monkeypatch.setattr(ri.RepositoryManager, "from_path", lambda p, s: rm)
    out = asyncio.run(idx.get_repository_stats(tmp_path))
    assert out["index"]["last_indexed_time"] is None


# ---------------------------------------------------------------------------
# clear_index
# ---------------------------------------------------------------------------


def test_clear_index_removes_cache_and_state(tmp_path):
    idx = _make_indexer()
    # seed cache
    idx._repo_managers[str(tmp_path.resolve())] = MagicMock()
    sf = tmp_path / idx.config.state_file_name
    sf.write_text("{}")
    ok = asyncio.run(idx.clear_index(tmp_path, clear_state=True))
    assert ok is True
    assert not sf.exists()
    assert str(tmp_path.resolve()) not in idx._repo_managers


def test_clear_index_keep_state(tmp_path):
    idx = _make_indexer()
    sf = tmp_path / idx.config.state_file_name
    sf.write_text("{}")
    ok = asyncio.run(idx.clear_index(tmp_path, clear_state=False))
    assert ok is True
    assert sf.exists()


# ---------------------------------------------------------------------------
# Factory functions
# ---------------------------------------------------------------------------


def test_create_repository_indexer():
    client = MagicMock()
    idx = create_repository_indexer(
        client, enable_git=False, track_authors=False, parallel=False
    )
    assert isinstance(idx, RepositoryIndexer)
    assert idx.config.enable_git_integration is False
    assert idx.config.track_authors is False
    assert idx.config.parallel_file_processing is False


def test_module_level_index_repository(tmp_path, monkeypatch):
    client = MagicMock()
    # Patch the class method to avoid touching git / fs deeply
    captured = {}

    async def fake_index(self, path, mode=None):
        captured["mode"] = mode
        return RepositoryIndexResult()

    monkeypatch.setattr(RepositoryIndexer, "index_repository", fake_index)
    res = asyncio.run(
        ri.index_repository(client, tmp_path, incremental=False, enable_git=False)
    )
    assert isinstance(res, RepositoryIndexResult)
    assert captured["mode"] is IndexMode.FULL
