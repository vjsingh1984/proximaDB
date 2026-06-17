"""Offline unit tests for proximadb_sdk.repository_manager.

Fully offline: no real git, no real subprocess, no real filesystem walk.
All git/subprocess calls are replaced via a fake backend or by monkeypatching
GitRepository._run_git. Filesystem existence checks are monkeypatched on
pathlib.Path.exists where needed.
"""

import json
import subprocess
import types
from datetime import datetime
from pathlib import Path

import pytest

import proximadb_sdk.repository_manager as rm
from proximadb_sdk.repository_manager import (
    Author,
    Branch,
    BranchType,
    ChangeType,
    Commit,
    DiffHunk,
    FileChange,
    FileDiff,
    GitRepository,
    IndexState,
    RepositoryInfo,
    RepositoryManager,
    Tag,
    VCSBackend,
    VCSType,
    get_current_commit_hash,
    get_file_git_info,
    get_repository_root,
    is_git_repository,
    repository_context,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _proc(stdout="", returncode=0):
    return subprocess.CompletedProcess(
        args=["git"], returncode=returncode, stdout=stdout, stderr=""
    )


COMMIT_OUT = (
    "abcdef1234567890abcdef1234567890abcdef12\n"  # hash
    "p1 p2\n"  # parents (two => merge)
    "Alice <alice@example.com>\n"  # author
    "Bob <bob@example.com>\n"  # committer
    "1700000000\n"  # ts
    "subject line\n\nbody"  # message
)


class FakeBackend(VCSBackend):
    """A controllable VCSBackend implementation (not a GitRepository)."""

    def __init__(self):
        self._root = Path("/repo")
        self.dirty = False
        self._remote = "git@example.com:org/repo.git"
        self._current_commit = Commit(
            hash="h" * 40,
            short_hash="hhhhhhh",
            author=Author("Alice", "alice@example.com"),
            committer=None,
            timestamp=datetime(2024, 1, 1),
            message="msg",
        )
        self._branch = "main"
        self.changed = []
        self.blame = []

    @property
    def vcs_type(self):
        return VCSType.GIT

    def get_root(self):
        return self._root

    def get_current_commit(self):
        return self._current_commit

    def get_current_branch(self):
        return self._branch

    def get_branches(self, include_remote=False):
        return [Branch("main", "h" * 40, is_current=True)]

    def get_tags(self):
        return [Tag("v1", "h" * 40)]

    def get_commit(self, ref):
        return self._current_commit

    def get_commits(self, since=None, until=None, path=None, limit=None):
        return [self._current_commit]

    def get_changed_files(self, from_ref=None, to_ref="HEAD", include_untracked=True):
        return list(self.changed)

    def get_file_diff(self, path, from_ref=None, to_ref="HEAD"):
        return FileDiff(path=path, old_path=None, change_type=ChangeType.MODIFIED)

    def get_file_content(self, path, ref="HEAD"):
        return "content"

    def get_blame(self, path, ref="HEAD"):
        return list(self.blame)

    def is_dirty(self):
        return self.dirty

    def get_remote_url(self):
        return self._remote


# ---------------------------------------------------------------------------
# Dataclasses / enums / properties
# ---------------------------------------------------------------------------


def test_author_hash_eq():
    a = Author("X", "x@e.com")
    b = Author("X", "x@e.com")
    c = Author("Y", "y@e.com")
    assert a == b
    assert a != c
    assert a != "not-an-author"
    assert hash(a) == hash(b)
    assert len({a, b, c}) == 2


def test_commit_is_merge():
    c1 = Commit(
        "h", "h", Author("a", "a"), None, datetime.now(), "m", parent_hashes=["p"]
    )
    c2 = Commit(
        "h",
        "h",
        Author("a", "a"),
        None,
        datetime.now(),
        "m",
        parent_hashes=["p1", "p2"],
    )
    assert c1.is_merge is False
    assert c2.is_merge is True


def test_branch_classify():
    assert Branch.classify("main") == BranchType.MAIN
    assert Branch.classify("master") == BranchType.MAIN
    assert Branch.classify("develop") == BranchType.DEVELOP
    assert Branch.classify("dev") == BranchType.DEVELOP
    assert Branch.classify("feature/x") == BranchType.FEATURE
    assert Branch.classify("feat/x") == BranchType.FEATURE
    assert Branch.classify("release/1.0") == BranchType.RELEASE
    assert Branch.classify("rel/1.0") == BranchType.RELEASE
    assert Branch.classify("hotfix/y") == BranchType.HOTFIX
    assert Branch.classify("fix/y") == BranchType.HOTFIX
    assert Branch.classify("random") == BranchType.OTHER


def test_filechange_is_code_file():
    assert FileChange("a.py", ChangeType.ADDED).is_code_file is True
    assert FileChange("a.rs", ChangeType.ADDED).is_code_file is True
    assert FileChange("README.md", ChangeType.ADDED).is_code_file is False
    assert FileChange("no_ext", ChangeType.ADDED).is_code_file is False


def test_filediff_totals():
    hunk = DiffHunk(
        1, 2, 1, 2, content="+added\n-removed\n+++header\n---header\n context"
    )
    fd = FileDiff(
        path="f", old_path=None, change_type=ChangeType.MODIFIED, hunks=[hunk]
    )
    assert fd.total_additions == 1
    assert fd.total_deletions == 1


def test_repository_info_to_dict():
    info = RepositoryInfo(
        root_path=Path("/r"),
        vcs_type=VCSType.GIT,
        remote_url="u",
        current_branch="main",
        current_commit="abc",
        is_dirty=True,
        total_commits=5,
    )
    d = info.to_dict()
    assert d["root_path"] == "/r"
    assert d["vcs_type"] == "GIT"
    assert d["is_dirty"] is True
    assert d["total_commits"] == 5


def test_index_state_roundtrip():
    st = IndexState(
        repository_id="abc",
        last_indexed_commit="c1",
        last_indexed_time=datetime(2024, 5, 1, 12, 0, 0),
        indexed_files={"a.py": "h1"},
        branch_states={"main": "c1"},
    )
    d = st.to_dict()
    assert d["repository_id"] == "abc"
    assert d["last_indexed_time"].startswith("2024-05-01")
    back = IndexState.from_dict(d)
    assert back.repository_id == "abc"
    assert back.last_indexed_commit == "c1"
    assert back.indexed_files == {"a.py": "h1"}
    assert back.branch_states == {"main": "c1"}


def test_index_state_from_dict_minimal():
    back = IndexState.from_dict({"repository_id": "x"})
    assert back.repository_id == "x"
    assert back.last_indexed_commit is None
    assert back.last_indexed_time is None
    assert back.indexed_files == {}


# ---------------------------------------------------------------------------
# GitRepository with mocked _run_git and filesystem
# ---------------------------------------------------------------------------


@pytest.fixture
def git_repo(monkeypatch):
    """Construct a GitRepository without touching the real filesystem."""
    # Make _find_root succeed deterministically.
    monkeypatch.setattr(GitRepository, "_find_root", lambda self: Path("/repo"))
    repo = GitRepository("/repo/sub")
    return repo


def test_find_root_found(monkeypatch):
    # Real _find_root logic: walk up until a `.git` exists.
    seen = {}

    def fake_exists(self):
        return str(self) == "/repo/.git"

    monkeypatch.setattr(Path, "exists", fake_exists)
    repo = GitRepository("/repo/deep/nested")
    assert repo.get_root() == Path("/repo")


def test_find_root_not_found(monkeypatch):
    monkeypatch.setattr(Path, "exists", lambda self: False)
    with pytest.raises(ValueError):
        GitRepository("/no/such/repo")


def test_run_git_success(git_repo, monkeypatch):
    captured = {}

    def fake_run(cmd, capture_output, text, check):
        captured["cmd"] = cmd
        return _proc(stdout="ok")

    monkeypatch.setattr(subprocess, "run", fake_run)
    result = git_repo._run_git("status")
    assert result.stdout == "ok"
    assert captured["cmd"][:4] == ["git", "-C", "/repo", "status"]


def test_run_git_failure(git_repo, monkeypatch):
    def fake_run(cmd, capture_output, text, check):
        raise subprocess.CalledProcessError(1, cmd, stderr="boom")

    monkeypatch.setattr(subprocess, "run", fake_run)
    with pytest.raises(subprocess.CalledProcessError):
        git_repo._run_git("bad")


def test_parse_author(git_repo):
    assert git_repo._parse_author("Alice <a@e.com>") == ("Alice", "a@e.com")
    assert git_repo._parse_author("PlainName") == ("PlainName", "")


def test_parse_commit_ok(git_repo):
    c = git_repo._parse_commit(COMMIT_OUT)
    assert c.hash.startswith("abcdef")
    assert c.short_hash == "abcdef1"
    assert c.author == Author("Alice", "alice@example.com")
    assert c.committer == Author("Bob", "bob@example.com")
    assert c.parent_hashes == ["p1", "p2"]
    assert c.is_merge is True
    assert "subject line" in c.message


def test_parse_commit_no_committer(git_repo):
    out = (
        "h" * 40 + "\n"
        "\n"  # no parents
        "Alice <a@e.com>\n"
        " \n"  # blank committer -> committer_name empty
        "1700000000\n"
        "msg"
    )
    c = git_repo._parse_commit(out)
    assert c.parent_hashes == []
    assert c.committer is None


def test_parse_commit_invalid(git_repo):
    with pytest.raises(ValueError):
        git_repo._parse_commit("too\nshort")


def test_vcs_type_and_root(git_repo):
    assert git_repo.vcs_type == VCSType.GIT
    assert git_repo.get_root() == Path("/repo")


def test_get_current_commit_ok(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(COMMIT_OUT))
    c = git_repo.get_current_commit()
    assert c is not None and c.author.name == "Alice"


def test_get_current_commit_fail(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_current_commit() is None


def test_get_current_branch_ok(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc("main\n"))
    assert git_repo.get_current_branch() == "main"


def test_get_current_branch_detached(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc("HEAD\n"))
    assert git_repo.get_current_branch() is None


def test_get_current_branch_fail(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_current_branch() is None


def test_get_branches_local_and_remote(git_repo, monkeypatch):
    local = "main|aaa|*|origin/main\nfeature/x|bbb||\n\n"
    remote = "origin/main|ccc\norigin/HEAD -> origin/main\n"

    def fake(*args, **k):
        if "-r" in args:
            return _proc(remote)
        return _proc(local)

    monkeypatch.setattr(git_repo, "_run_git", fake)
    branches = git_repo.get_branches(include_remote=True)
    names = {b.name for b in branches}
    assert "main" in names
    assert "feature/x" in names
    assert "origin/main" in names
    main = next(b for b in branches if b.name == "main")
    assert main.is_current is True
    assert main.upstream == "origin/main"
    remote_b = next(b for b in branches if b.is_remote)
    assert remote_b.branch_type == BranchType.MAIN


def test_get_branches_errors(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_branches(include_remote=True) == []


def test_get_tags(git_repo, monkeypatch):
    annotated = "v1.0|aaa|tag|Tagger <t@e.com>|1700000000|release one"
    lightweight = "v0.9|bbb|commit"
    out = annotated + "\n" + lightweight + "\n\n"
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(out))
    tags = git_repo.get_tags()
    assert {t.name for t in tags} == {"v1.0", "v0.9"}
    v1 = next(t for t in tags if t.name == "v1.0")
    assert v1.is_annotated is True
    assert v1.tagger == Author("Tagger", "t@e.com")
    assert v1.message == "release one"
    assert v1.timestamp is not None


def test_get_tags_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_tags() == []


def test_get_commit_caches(git_repo, monkeypatch):
    calls = {"n": 0}

    def fake(*a, **k):
        calls["n"] += 1
        return _proc(COMMIT_OUT)

    monkeypatch.setattr(git_repo, "_run_git", fake)
    c1 = git_repo.get_commit("HEAD")
    c2 = git_repo.get_commit("HEAD")
    assert c1 is c2
    assert calls["n"] == 1


def test_get_commit_fail(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_commit("badref") is None


def test_get_commits_variants(git_repo, monkeypatch):
    out = COMMIT_OUT + "\x00" + COMMIT_OUT + "\x00" + "garbage-entry\x00"
    seen = {}

    def fake(*args, **k):
        seen["args"] = args
        return _proc(out)

    monkeypatch.setattr(git_repo, "_run_git", fake)
    commits = git_repo.get_commits(since="a", until="b", path="f.py", limit=5)
    assert len(commits) == 2  # garbage entry skipped via ValueError
    assert "-5" in seen["args"]
    assert "a..b" in seen["args"]
    assert "--" in seen["args"]


def test_get_commits_until_only(git_repo, monkeypatch):
    monkeypatch.setattr(
        git_repo, "_run_git", lambda *a, **k: _proc(COMMIT_OUT + "\x00")
    )
    commits = git_repo.get_commits(until="HEAD~5")
    assert len(commits) == 1


def test_get_commits_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_commits() == []


def test_parse_diff_name_status(git_repo):
    out = "A\tnew.py\nM\tmod.py\nD\tgone.py\nR100\told.py\trenamed.py\nX\tweird.py\n"
    changes = git_repo._parse_diff_name_status(out)
    by_path = {c.path: c for c in changes}
    assert by_path["new.py"].change_type == ChangeType.ADDED
    assert by_path["mod.py"].change_type == ChangeType.MODIFIED
    assert by_path["gone.py"].change_type == ChangeType.DELETED
    assert by_path["renamed.py"].change_type == ChangeType.RENAMED
    assert by_path["renamed.py"].old_path == "old.py"
    assert by_path["weird.py"].change_type == ChangeType.MODIFIED  # fallback


def test_get_changed_files_between_refs(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc("M\tf.py\n"))
    changes = git_repo.get_changed_files(from_ref="a", to_ref="b")
    assert len(changes) == 1


def test_get_changed_files_between_refs_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_changed_files(from_ref="a") == []


def test_get_changed_files_uncommitted(git_repo, monkeypatch):
    def fake(*args, **k):
        if "--cached" in args:
            return _proc("A\tstaged.py\n")
        if "ls-files" in args:
            return _proc("untracked.py\n")
        # unstaged: staged.py repeats (deduped) + extra
        return _proc("M\tstaged.py\nM\tunstaged.py\n")

    monkeypatch.setattr(git_repo, "_run_git", fake)
    changes = git_repo.get_changed_files()
    paths = [c.path for c in changes]
    assert "staged.py" in paths
    assert "unstaged.py" in paths
    assert "untracked.py" in paths
    # staged.py appears only once (dedupe)
    assert paths.count("staged.py") == 1


def test_get_changed_files_uncommitted_diff_error(git_repo, monkeypatch):
    def fake(*args, **k):
        if "ls-files" in args:
            return _proc("u.py\n")
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", fake)
    changes = git_repo.get_changed_files()
    assert [c.path for c in changes] == ["u.py"]


def test_get_changed_files_untracked_error(git_repo, monkeypatch):
    def fake(*args, **k):
        if "ls-files" in args:
            raise subprocess.CalledProcessError(1, "git")
        return _proc("")

    monkeypatch.setattr(git_repo, "_run_git", fake)
    assert git_repo.get_changed_files() == []


def test_get_changed_files_no_untracked(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc("M\tf.py\n"))
    changes = git_repo.get_changed_files(include_untracked=False)
    # Both staged and unstaged calls return the same -> deduped to one
    assert [c.path for c in changes] == ["f.py"]


DIFF_OUT = """diff --git a/foo.py b/foo.py
index 111..222 100644
--- a/foo.py
+++ b/foo.py
@@ -1,3 +1,4 @@ def f():
 unchanged
-old line
+new line
+another new
"""


def test_get_file_diff_from_ref(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(DIFF_OUT))
    fd = git_repo.get_file_diff("foo.py", from_ref="a", to_ref="b")
    assert fd is not None
    assert len(fd.hunks) == 1
    assert fd.hunks[0].old_start == 1
    assert fd.change_type == ChangeType.MODIFIED


def test_get_file_diff_no_from(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(DIFF_OUT))
    fd = git_repo.get_file_diff("foo.py")
    assert fd is not None


def test_get_file_diff_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_file_diff("foo.py") is None


def test_parse_unified_diff_added(git_repo):
    out = """diff --git a/new.py b/new.py
new file mode 100644
--- /dev/null
+++ b/new.py
@@ -0,0 +1,2 @@
+line1
+line2
"""
    fd = git_repo._parse_unified_diff(out, "new.py")
    assert fd.change_type == ChangeType.ADDED
    assert len(fd.hunks) == 1


def test_parse_unified_diff_binary(git_repo):
    out = (
        "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n"
    )
    fd = git_repo._parse_unified_diff(out, "img.png")
    assert fd.is_binary is True
    assert fd.hunks == []


def test_parse_unified_diff_multi_hunk(git_repo):
    out = """--- a/f.py
+++ b/f.py
@@ -1 +1 @@
-a
+b
@@ -10,2 +10,2 @@ ctx
-c
+d
"""
    fd = git_repo._parse_unified_diff(out, "f.py")
    assert len(fd.hunks) == 2
    assert fd.hunks[1].old_start == 10
    assert fd.hunks[1].header == "ctx"


def test_get_file_content_ok(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc("file body"))
    assert git_repo.get_file_content("f.py") == "file body"


def test_get_file_content_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_file_content("f.py") is None


BLAME_OUT = (
    "1111111111111111111111111111111111111111 1 1 2\n"
    "author Alice\n"
    "author-mail <alice@example.com>\n"
    "author-time 1700000000\n"
    "summary x\n"
    "\tcode line 1\n"
    "1111111111111111111111111111111111111111 2 2\n"
    "\tcode line 2\n"
)


def test_get_blame_ok(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(BLAME_OUT))
    entries = git_repo.get_blame("f.py")
    assert len(entries) >= 1
    assert entries[0].author == Author("Alice", "alice@example.com")
    assert entries[0].line_start == 1
    assert entries[0].line_end == 2


def test_get_blame_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_blame("f.py") == []


def test_parse_blame_no_author(git_repo):
    # Header present but no author metadata -> entry skipped
    out = (
        "2222222222222222222222222222222222222222 1 1 1\n"
        "summary nope\n"
        "\tsome line\n"
    )
    entries = git_repo._parse_blame_porcelain(out)
    assert entries == []


def test_is_dirty_true(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(" M file\n"))
    assert git_repo.is_dirty() is True


def test_is_dirty_false(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(""))
    assert git_repo.is_dirty() is False


def test_is_dirty_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.is_dirty() is False


def test_get_remote_url_ok(git_repo, monkeypatch):
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc("git@x:r.git\n"))
    assert git_repo.get_remote_url() == "git@x:r.git"


def test_get_remote_url_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_remote_url() is None


def test_get_file_history(git_repo, monkeypatch):
    monkeypatch.setattr(
        git_repo, "_run_git", lambda *a, **k: _proc(COMMIT_OUT + "\x00")
    )
    hist = git_repo.get_file_history("f.py", limit=3)
    assert len(hist) == 1


def test_get_contributors(git_repo, monkeypatch):
    out = "Alice|a@e.com\nBob|b@e.com\nAlice|a@e.com\nbadline\n"
    monkeypatch.setattr(git_repo, "_run_git", lambda *a, **k: _proc(out))
    contributors = git_repo.get_contributors()
    assert Author("Alice", "a@e.com") in contributors
    assert len(contributors) == 2  # deduped


def test_get_contributors_error(git_repo, monkeypatch):
    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(git_repo, "_run_git", raise_err)
    assert git_repo.get_contributors() == []


def test_get_stats(git_repo, monkeypatch):
    def fake(*args, **k):
        if "rev-list" in args:
            return _proc("42\n")
        if "branch" in args:
            return _proc("main|aaa|*|\n")
        if "tag" in args:
            return _proc("v1|bbb|commit\n")
        if "log" in args:
            return _proc("Alice|a@e.com\n")
        return _proc("")

    monkeypatch.setattr(git_repo, "_run_git", fake)
    stats = git_repo.get_stats()
    assert stats["total_commits"] == 42
    assert stats["total_branches"] == 1
    assert stats["total_tags"] == 1
    assert stats["total_contributors"] == 1


def test_get_stats_revlist_error(git_repo, monkeypatch):
    def fake(*args, **k):
        if "rev-list" in args:
            raise subprocess.CalledProcessError(1, "git")
        return _proc("")

    monkeypatch.setattr(git_repo, "_run_git", fake)
    stats = git_repo.get_stats()
    assert stats["total_commits"] == 0


# ---------------------------------------------------------------------------
# RepositoryManager (facade) with FakeBackend
# ---------------------------------------------------------------------------


def test_manager_init_computes_repo_id():
    mgr = RepositoryManager(FakeBackend())
    assert len(mgr.index_state.repository_id) == 16


def test_manager_init_with_state():
    st = IndexState(repository_id="given")
    mgr = RepositoryManager(FakeBackend(), index_state=st)
    assert mgr.index_state.repository_id == "given"


def test_manager_properties():
    mgr = RepositoryManager(FakeBackend())
    assert mgr.vcs_type == VCSType.GIT
    assert mgr.root == Path("/repo")


def test_manager_from_path_ok(monkeypatch):
    monkeypatch.setattr(GitRepository, "_find_root", lambda self: Path("/repo"))
    mgr = RepositoryManager.from_path("/repo/sub")
    assert isinstance(mgr, RepositoryManager)


def test_manager_from_path_not_vcs(monkeypatch):
    monkeypatch.setattr(GitRepository, "_find_root", lambda self: None)
    with pytest.raises(ValueError):
        RepositoryManager.from_path("/nope")


def test_detect_vcs(monkeypatch):
    def make_exists(marker):
        def fake(self):
            return str(self).endswith(marker)

        return fake

    monkeypatch.setattr(Path, "exists", make_exists("/.git"))
    assert RepositoryManager.detect_vcs("/repo/sub") == VCSType.GIT

    monkeypatch.setattr(Path, "exists", make_exists("/.hg"))
    assert RepositoryManager.detect_vcs("/repo/sub") == VCSType.MERCURIAL

    monkeypatch.setattr(Path, "exists", make_exists("/.svn"))
    assert RepositoryManager.detect_vcs("/repo/sub") == VCSType.SVN

    monkeypatch.setattr(Path, "exists", lambda self: False)
    assert RepositoryManager.detect_vcs("/repo/sub") == VCSType.NONE


def test_get_info():
    mgr = RepositoryManager(FakeBackend())
    info = mgr.get_info()
    assert info.vcs_type == VCSType.GIT
    assert info.current_branch == "main"
    assert info.current_commit == "h" * 40


def test_get_info_no_current_commit():
    backend = FakeBackend()
    backend._current_commit = None
    mgr = RepositoryManager(backend)
    info = mgr.get_info()
    assert info.current_commit is None


def test_get_changes_since_last_index_with_commit():
    backend = FakeBackend()
    backend.changed = [FileChange("a.py", ChangeType.MODIFIED)]
    mgr = RepositoryManager(backend)
    mgr.index_state.last_indexed_commit = "prev"
    changes = mgr.get_changes_since_last_index()
    assert len(changes) == 1


def test_get_changes_since_last_index_first_run(monkeypatch):
    # No last_indexed_commit -> _get_all_tracked_files path (GitRepository)
    monkeypatch.setattr(GitRepository, "_find_root", lambda self: Path("/repo"))
    repo = GitRepository("/repo")
    monkeypatch.setattr(repo, "_run_git", lambda *a, **k: _proc("a.py\nb.py\n"))
    mgr = RepositoryManager(repo)
    changes = mgr.get_changes_since_last_index()
    assert {c.path for c in changes} == {"a.py", "b.py"}
    assert all(c.change_type == ChangeType.ADDED for c in changes)


def test_get_all_tracked_files_error(monkeypatch):
    monkeypatch.setattr(GitRepository, "_find_root", lambda self: Path("/repo"))
    repo = GitRepository("/repo")

    def raise_err(*a, **k):
        raise subprocess.CalledProcessError(1, "git")

    monkeypatch.setattr(repo, "_run_git", raise_err)
    mgr = RepositoryManager(repo)
    assert mgr._get_all_tracked_files() == []


def test_get_all_tracked_files_non_git_backend():
    # FakeBackend is not a GitRepository -> returns []
    mgr = RepositoryManager(FakeBackend())
    assert mgr._get_all_tracked_files() == []


def test_get_files_to_reindex_filtering():
    backend = FakeBackend()
    backend.changed = [
        FileChange("a.py", ChangeType.MODIFIED),
        FileChange("README.md", ChangeType.ADDED),
        FileChange("b.rs", ChangeType.ADDED),
        FileChange("gone.py", ChangeType.DELETED),
    ]
    mgr = RepositoryManager(backend)
    mgr.index_state.last_indexed_commit = "prev"

    # code files only, deleted dropped
    out = mgr.get_files_to_reindex()
    paths = {c.path for c in out}
    assert paths == {"a.py", "b.rs"}

    # extension filter
    out2 = mgr.get_files_to_reindex(extensions={".py"})
    assert {c.path for c in out2} == {"a.py"}

    # no code filter -> markdown included (but deleted still dropped)
    out3 = mgr.get_files_to_reindex(filter_code_files=False)
    assert "README.md" in {c.path for c in out3}
    assert "gone.py" not in {c.path for c in out3}


def test_get_deleted_files():
    backend = FakeBackend()
    backend.changed = [
        FileChange("a.py", ChangeType.MODIFIED),
        FileChange("gone.py", ChangeType.DELETED),
    ]
    mgr = RepositoryManager(backend)
    mgr.index_state.last_indexed_commit = "prev"
    deleted = mgr.get_deleted_files()
    assert [c.path for c in deleted] == ["gone.py"]


def test_update_index_state_default_commit():
    backend = FakeBackend()
    mgr = RepositoryManager(backend)
    mgr.update_index_state(indexed_files={"a.py": "h1"})
    assert mgr.index_state.last_indexed_commit == "h" * 40
    assert mgr.index_state.last_indexed_time is not None
    assert mgr.index_state.indexed_files == {"a.py": "h1"}
    assert mgr.index_state.branch_states["main"] == "h" * 40


def test_update_index_state_explicit_commit():
    backend = FakeBackend()
    mgr = RepositoryManager(backend)
    mgr.update_index_state(commit_hash="explicit")
    assert mgr.index_state.last_indexed_commit == "explicit"
    assert mgr.index_state.branch_states["main"] == "explicit"


def test_update_index_state_no_current_commit():
    backend = FakeBackend()
    backend._current_commit = None
    mgr = RepositoryManager(backend)
    mgr.update_index_state()
    assert mgr.index_state.last_indexed_commit is None


def test_manager_file_helpers():
    backend = FakeBackend()
    backend.blame = [
        rm.BlameEntry("h", Author("Alice", "a@e.com"), datetime.now(), 1, 1, "x"),
        rm.BlameEntry("h", Author("Alice", "a@e.com"), datetime.now(), 2, 2, "y"),
        rm.BlameEntry("h", Author("Bob", "b@e.com"), datetime.now(), 3, 3, "z"),
    ]
    mgr = RepositoryManager(backend)
    assert mgr.get_file_content("f.py") == "content"
    assert len(mgr.get_file_blame("f.py")) == 3
    authors = mgr.get_file_authors("f.py")
    assert len(authors) == 2  # deduped


def test_manager_commit_helpers():
    mgr = RepositoryManager(FakeBackend())
    assert mgr.get_commit_info().hash == "h" * 40
    assert len(mgr.get_recent_commits(limit=5)) == 1


def test_save_and_load_state(tmp_path):
    mgr = RepositoryManager(FakeBackend())
    mgr.index_state.last_indexed_commit = "c1"
    state_file = tmp_path / "state.json"
    mgr.save_state(state_file)
    assert state_file.exists()
    loaded = RepositoryManager.load_state(state_file)
    assert loaded is not None
    assert loaded.last_indexed_commit == "c1"


def test_load_state_missing(tmp_path):
    assert RepositoryManager.load_state(tmp_path / "nope.json") is None


def test_load_state_bad_json(tmp_path):
    p = tmp_path / "bad.json"
    p.write_text("{not valid json")
    assert RepositoryManager.load_state(p) is None


def test_load_state_missing_key(tmp_path):
    p = tmp_path / "missing.json"
    p.write_text(json.dumps({"no_repo_id": 1}))
    assert RepositoryManager.load_state(p) is None


# ---------------------------------------------------------------------------
# Module-level utility functions
# ---------------------------------------------------------------------------


def test_is_git_repository(monkeypatch):
    monkeypatch.setattr(
        RepositoryManager, "detect_vcs", classmethod(lambda cls, p: VCSType.GIT)
    )
    assert is_git_repository("/x") is True
    monkeypatch.setattr(
        RepositoryManager, "detect_vcs", classmethod(lambda cls, p: VCSType.NONE)
    )
    assert is_git_repository("/x") is False


def test_get_repository_root_ok(monkeypatch):
    fake_mgr = RepositoryManager(FakeBackend())
    monkeypatch.setattr(
        RepositoryManager, "from_path", classmethod(lambda cls, p, s=None: fake_mgr)
    )
    assert get_repository_root("/x") == Path("/repo")


def test_get_repository_root_fail(monkeypatch):
    def raise_err(cls, p, s=None):
        raise ValueError("no vcs")

    monkeypatch.setattr(RepositoryManager, "from_path", classmethod(raise_err))
    assert get_repository_root("/x") is None


def test_get_current_commit_hash_ok(monkeypatch):
    fake_mgr = RepositoryManager(FakeBackend())
    monkeypatch.setattr(
        RepositoryManager, "from_path", classmethod(lambda cls, p, s=None: fake_mgr)
    )
    assert get_current_commit_hash("/x") == "h" * 40


def test_get_current_commit_hash_no_commit(monkeypatch):
    backend = FakeBackend()
    backend._current_commit = None
    fake_mgr = RepositoryManager(backend)
    monkeypatch.setattr(
        RepositoryManager, "from_path", classmethod(lambda cls, p, s=None: fake_mgr)
    )
    monkeypatch.setattr(fake_mgr._backend, "get_commit", lambda ref="HEAD": None)
    assert get_current_commit_hash("/x") is None


def test_get_current_commit_hash_fail(monkeypatch):
    def raise_err(cls, p, s=None):
        raise ValueError("no vcs")

    monkeypatch.setattr(RepositoryManager, "from_path", classmethod(raise_err))
    assert get_current_commit_hash("/x") is None


def test_get_file_git_info_ok(monkeypatch):
    backend = FakeBackend()
    backend.blame = [
        rm.BlameEntry("h", Author("Alice", "a@e.com"), datetime.now(), 1, 1, "x"),
    ]
    fake_mgr = RepositoryManager(backend)
    monkeypatch.setattr(
        RepositoryManager, "from_path", classmethod(lambda cls, p, s=None: fake_mgr)
    )
    # Path.resolve and relative_to must yield deterministic values.
    monkeypatch.setattr(Path, "resolve", lambda self: Path("/repo/src/main.py"))
    info = get_file_git_info("/repo/src/main.py")
    assert info is not None
    assert info["repo_root"] == "/repo"
    assert info["relative_path"] == "src/main.py"
    assert info["authors"] == [{"name": "Alice", "email": "a@e.com"}]


def test_get_file_git_info_fail(monkeypatch):
    def raise_err(cls, p, s=None):
        raise ValueError("no vcs")

    monkeypatch.setattr(RepositoryManager, "from_path", classmethod(raise_err))
    assert get_file_git_info("/x") is None


# ---------------------------------------------------------------------------
# repository_context context manager
# ---------------------------------------------------------------------------


def test_repository_context_no_state_file(monkeypatch):
    fake_mgr = RepositoryManager(FakeBackend())
    monkeypatch.setattr(
        RepositoryManager, "from_path", classmethod(lambda cls, p, s=None: fake_mgr)
    )
    with repository_context("/repo") as repo:
        assert repo is fake_mgr


def test_repository_context_with_state_file(monkeypatch, tmp_path):
    fake_mgr = RepositoryManager(FakeBackend())
    loaded_state = IndexState(repository_id="loaded")
    captured = {}

    monkeypatch.setattr(
        RepositoryManager, "load_state", classmethod(lambda cls, p: loaded_state)
    )

    def fake_from_path(cls, p, s=None):
        captured["state"] = s
        return fake_mgr

    monkeypatch.setattr(RepositoryManager, "from_path", classmethod(fake_from_path))

    def fake_save(self, p):
        captured["saved"] = True

    monkeypatch.setattr(RepositoryManager, "save_state", fake_save)

    state_file = tmp_path / "s.json"
    with repository_context("/repo", state_file=state_file) as repo:
        assert repo is fake_mgr
    assert captured["state"] is loaded_state
    assert captured["saved"] is True
