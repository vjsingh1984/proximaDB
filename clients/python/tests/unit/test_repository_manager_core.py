from datetime import datetime
from pathlib import Path
from types import SimpleNamespace

import pytest

from proximadb_sdk.repository_manager import (
    Author,
    BlameEntry,
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
)


class FakeBackend(VCSBackend):
    def __init__(self, root: Path):
        self.root = root
        self.author = Author("Ada", "ada@example.com")
        self.commit = Commit(
            hash="a" * 40,
            short_hash="a" * 7,
            author=self.author,
            committer=self.author,
            timestamp=datetime(2026, 5, 22, 12, 0, 0),
            message="initial",
        )
        self.changes = [
            FileChange("src/main.py", ChangeType.ADDED),
            FileChange("README.md", ChangeType.MODIFIED),
            FileChange("src/old.py", ChangeType.DELETED),
        ]

    @property
    def vcs_type(self):
        return VCSType.GIT

    def get_root(self):
        return self.root

    def get_current_commit(self):
        return self.commit

    def get_current_branch(self):
        return "main"

    def get_branches(self, include_remote=False):
        branches = [Branch("main", self.commit.hash, is_current=True)]
        if include_remote:
            branches.append(Branch("origin/main", self.commit.hash, is_remote=True))
        return branches

    def get_tags(self):
        return [Tag("v1.0.0", self.commit.hash)]

    def get_commit(self, ref):
        return self.commit if ref in {"HEAD", self.commit.hash} else None

    def get_commits(self, since=None, until=None, path=None, limit=None):
        commits = [self.commit]
        return commits[:limit] if limit else commits

    def get_changed_files(self, from_ref=None, to_ref="HEAD", include_untracked=True):
        return self.changes

    def get_file_diff(self, path, from_ref=None, to_ref="HEAD"):
        return FileDiff(
            path=path,
            old_path=None,
            change_type=ChangeType.MODIFIED,
            hunks=[DiffHunk(1, 1, 1, 2, "-old\n+new\n+more")],
        )

    def get_file_content(self, path, ref="HEAD"):
        return "print('ok')\n" if path.endswith(".py") else None

    def get_blame(self, path, ref="HEAD"):
        return [
            BlameEntry(
                commit_hash=self.commit.hash,
                author=self.author,
                timestamp=self.commit.timestamp,
                line_start=1,
                line_end=1,
                content="print('ok')",
            )
        ]

    def is_dirty(self):
        return True

    def get_remote_url(self):
        return "git@example.com:org/repo.git"

    def get_stats(self):
        return {
            "total_commits": 1,
            "total_branches": 1,
            "total_tags": 1,
            "total_contributors": 1,
        }


def test_repository_dataclasses_and_serialization_roundtrip(tmp_path):
    author = Author("Ada", "ada@example.com")
    same_author = Author("Ada", "ada@example.com")
    other = Author("Grace", "grace@example.com")

    assert author == same_author
    assert author != other
    assert (author == object()) is False
    assert len({author, same_author, other}) == 2

    merge_commit = Commit(
        hash="b" * 40,
        short_hash="b" * 7,
        author=author,
        committer=None,
        timestamp=datetime(2026, 5, 22, 12, 0, 0),
        message="merge",
        parent_hashes=["a" * 40, "c" * 40],
    )
    assert merge_commit.is_merge is True

    assert Branch.classify("main") == BranchType.MAIN
    assert Branch.classify("dev") == BranchType.DEVELOP
    assert Branch.classify("feature/search") == BranchType.FEATURE
    assert Branch.classify("rel/1.0") == BranchType.RELEASE
    assert Branch.classify("fix/security") == BranchType.HOTFIX
    assert Branch.classify("topic") == BranchType.OTHER

    assert FileChange("src/lib.rs", ChangeType.MODIFIED).is_code_file is True
    assert FileChange("notes.txt", ChangeType.MODIFIED).is_code_file is False

    diff = FileDiff(
        path="src/main.py",
        old_path="src/main.py",
        change_type=ChangeType.MODIFIED,
        hunks=[DiffHunk(1, 2, 1, 3, "--- a\n+++ b\n-old\n+new\n+more")],
    )
    assert diff.total_additions == 2
    assert diff.total_deletions == 1

    info = RepositoryInfo(
        root_path=tmp_path,
        vcs_type=VCSType.GIT,
        remote_url="git@example.com:org/repo.git",
        current_branch="main",
        current_commit=merge_commit.hash,
        is_dirty=True,
        total_commits=3,
        total_branches=2,
        total_tags=1,
    )
    assert info.to_dict()["root_path"] == str(tmp_path)
    assert info.to_dict()["vcs_type"] == "GIT"

    state = IndexState(
        repository_id="repo",
        last_indexed_commit=merge_commit.hash,
        last_indexed_time=datetime(2026, 5, 22, 12, 0, 0),
        indexed_files={"src/main.py": "hash"},
        branch_states={"main": merge_commit.hash},
    )
    assert IndexState.from_dict(state.to_dict()) == state


def test_git_repository_methods_with_stubbed_git_output():
    git = GitRepository.__new__(GitRepository)
    git._root = Path("/repo")
    git._commit_cache = {}
    commit_output = "\n".join(
        [
            "a" * 40,
            "",
            "Ada <ada@example.com>",
            "Ada <ada@example.com>",
            "1770000000",
            "subject",
        ]
    )

    def fake_run_git(*args, **kwargs):
        if args[:2] == ("rev-parse", "--abbrev-ref"):
            return SimpleNamespace(stdout="main\n")
        if args[:3] == (
            "branch",
            "-v",
            "--format=%(refname:short)|%(objectname)|%(HEAD)|%(upstream:short)",
        ):
            return SimpleNamespace(stdout=f"main|{'a' * 40}|*|origin/main\n")
        if args[:2] == ("branch", "-r"):
            return SimpleNamespace(stdout=f"origin/main|{'b' * 40}\n")
        if args[0] == "tag":
            return SimpleNamespace(
                stdout=f"v1.0.0|{'c' * 40}|tag|Ada <ada@example.com>|1770000000|release\n"
            )
        if args[:2] == ("log", "-1"):
            return SimpleNamespace(stdout=commit_output)
        if args[:2] == ("log", "--format=%an|%ae"):
            return SimpleNamespace(stdout="Ada|ada@example.com\n")
        if args[0] == "log" and args[1].startswith("--format="):
            return SimpleNamespace(stdout=f"{commit_output}\x00")
        if args[:3] == ("diff", "--name-status", "--numstat"):
            return SimpleNamespace(stdout="M\tmod.py\n")
        if args[:3] == ("diff", "--cached", "--name-status"):
            return SimpleNamespace(stdout="A\tstaged.py\n")
        if args[:2] == ("diff", "--name-status"):
            return SimpleNamespace(stdout="M\tstaged.py\nM\tunstaged.py\n")
        if args[:2] == ("ls-files", "--others"):
            return SimpleNamespace(stdout="new.py\n")
        if args[0] == "diff":
            return SimpleNamespace(
                stdout="\n".join(
                    [
                        "diff --git a/src/main.py b/src/main.py",
                        "--- a/src/main.py",
                        "+++ b/src/main.py",
                        "@@ -1 +1 @@",
                        "-old",
                        "+new",
                    ]
                )
            )
        if args[0] == "show":
            return SimpleNamespace(stdout="print('ok')\n")
        if args[0] == "blame":
            return SimpleNamespace(
                stdout="\n".join(
                    [
                        f"{'a' * 40} 1 1 1",
                        "author Ada",
                        "author-mail <ada@example.com>",
                        "author-time 1770000000",
                        "\tprint('ok')",
                    ]
                )
            )
        if args[:2] == ("status", "--porcelain"):
            return SimpleNamespace(stdout=" M src/main.py\n")
        if args[:3] == ("remote", "get-url", "origin"):
            return SimpleNamespace(stdout="git@example.com:org/repo.git\n")
        if args[:2] == ("rev-list", "--count"):
            return SimpleNamespace(stdout="5\n")
        raise AssertionError(f"Unexpected git args: {args}")

    git._run_git = fake_run_git

    assert git.get_root() == Path("/repo")
    assert git.vcs_type == VCSType.GIT
    assert git.get_current_commit().hash == "a" * 40
    assert git.get_current_branch() == "main"

    branches = git.get_branches(include_remote=True)
    assert branches[0].is_current is True
    assert branches[1].is_remote is True

    tags = git.get_tags()
    assert tags[0].is_annotated is True
    assert tags[0].tagger == Author("Ada", "ada@example.com")
    assert tags[0].message == "release"

    assert git.get_commit("HEAD").hash == "a" * 40
    assert git.get_commit("HEAD").hash == "a" * 40
    assert git.get_commits(limit=1)[0].hash == "a" * 40
    assert git.get_file_history("src/main.py", limit=1)[0].hash == "a" * 40

    assert git.get_changed_files(from_ref="old")[0].path == "mod.py"
    local_changes = git.get_changed_files()
    assert [change.path for change in local_changes] == [
        "staged.py",
        "unstaged.py",
        "new.py",
    ]

    assert git.get_file_diff("src/main.py").total_additions == 1
    assert git.get_file_content("src/main.py") == "print('ok')\n"
    assert git.get_blame("src/main.py")[0].author == Author("Ada", "ada@example.com")
    assert git.is_dirty() is True
    assert git.get_remote_url() == "git@example.com:org/repo.git"
    assert git.get_contributors() == [Author("Ada", "ada@example.com")]
    assert git.get_stats() == {
        "total_commits": 5,
        "total_branches": 1,
        "total_tags": 1,
        "total_contributors": 1,
    }


def test_git_repository_parsers_cover_diff_commit_author_and_blame():
    git = GitRepository.__new__(GitRepository)
    git._commit_cache = {}

    assert git._parse_author("Ada Lovelace <ada@example.com>") == (
        "Ada Lovelace",
        "ada@example.com",
    )
    assert git._parse_author("No Email") == ("No Email", "")

    commit = git._parse_commit(
        "\n".join(
            [
                "a" * 40,
                "b" * 40,
                "Ada <ada@example.com>",
                "Grace <grace@example.com>",
                "1770000000",
                "subject",
                "body",
            ]
        )
    )
    assert commit.short_hash == "a" * 7
    assert commit.author.email == "ada@example.com"
    assert commit.message == "subject\nbody"

    with pytest.raises(ValueError):
        git._parse_commit("too short")

    changes = git._parse_diff_name_status(
        "A\tnew.py\nM\tmod.py\nD\told.py\nR100\told_name.py\tnew_name.py\n"
    )
    assert [change.change_type for change in changes] == [
        ChangeType.ADDED,
        ChangeType.MODIFIED,
        ChangeType.DELETED,
        ChangeType.RENAMED,
    ]
    assert changes[-1].old_path == "old_name.py"
    assert changes[-1].path == "new_name.py"

    parsed_diff = git._parse_unified_diff(
        "\n".join(
            [
                "diff --git a/file.py b/file.py",
                "--- a/file.py",
                "+++ b/file.py",
                "@@ -1,1 +1,2 @@ function",
                "-old",
                "+new",
                "+more",
            ]
        ),
        "file.py",
    )
    assert parsed_diff.change_type == ChangeType.MODIFIED
    assert parsed_diff.total_additions == 2
    assert parsed_diff.total_deletions == 1
    assert parsed_diff.hunks[0].header == "function"

    binary_diff = git._parse_unified_diff("Binary files differ", "asset.bin")
    assert binary_diff.is_binary is True

    blame = git._parse_blame_porcelain(
        "\n".join(
            [
                f"{'a' * 40} 1 1 1",
                "author Ada",
                "author-mail <ada@example.com>",
                "author-time 1770000000",
                "\tprint('ok')",
            ]
        )
    )
    assert blame[0].author == Author("Ada", "ada@example.com")
    assert blame[0].line_start == 1
    assert blame[0].content == "print('ok')"


def test_repository_manager_facade_uses_backend_and_state(tmp_path):
    backend = FakeBackend(tmp_path)
    manager = RepositoryManager(backend)

    assert manager.vcs_type == VCSType.GIT
    assert manager.root == tmp_path
    assert manager.index_state.repository_id

    info = manager.get_info()
    assert info.current_branch == "main"
    assert info.current_commit == backend.commit.hash
    assert info.is_dirty is True
    assert info.total_commits == 1

    assert manager.get_changes_since_last_index() == []
    manager.index_state.last_indexed_commit = "previous"
    assert manager.get_changes_since_last_index() == backend.changes
    files = manager.get_files_to_reindex(extensions={".py"})
    assert [file.path for file in files] == ["src/main.py"]
    assert manager.get_deleted_files()[0].path == "src/old.py"

    manager.update_index_state(indexed_files={"src/main.py": "hash"})
    assert manager.index_state.last_indexed_commit == backend.commit.hash
    assert manager.index_state.branch_states["main"] == backend.commit.hash
    assert manager.index_state.indexed_files["src/main.py"] == "hash"

    assert manager.get_file_content("src/main.py") == "print('ok')\n"
    assert manager.get_file_blame("src/main.py")[0].author == backend.author
    assert manager.get_file_authors("src/main.py") == [backend.author]
    assert manager.get_commit_info().hash == backend.commit.hash
    assert manager.get_recent_commits(limit=1) == [backend.commit]

    state_path = tmp_path / "state.json"
    manager.save_state(state_path)
    assert RepositoryManager.load_state(state_path) == manager.index_state
    assert RepositoryManager.load_state(tmp_path / "missing.json") is None

    state_path.write_text("{not-json")
    assert RepositoryManager.load_state(state_path) is None


def test_repository_manager_detection_helpers_for_local_markers(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    nested = repo / "src"
    nested.mkdir()

    hg_repo = tmp_path / "hg"
    hg_repo.mkdir()
    (hg_repo / ".hg").mkdir()
    svn_repo = tmp_path / "svn"
    svn_repo.mkdir()
    (svn_repo / ".svn").mkdir()

    assert RepositoryManager.detect_vcs(nested) == VCSType.GIT
    assert RepositoryManager.detect_vcs(hg_repo) == VCSType.MERCURIAL
    assert RepositoryManager.detect_vcs(svn_repo) == VCSType.SVN
    assert RepositoryManager.detect_vcs(tmp_path) == VCSType.NONE
    assert is_git_repository(nested) is True
    assert get_repository_root(tmp_path) is None
    assert get_current_commit_hash(tmp_path) is None
    assert get_file_git_info(tmp_path / "missing.py") is None
