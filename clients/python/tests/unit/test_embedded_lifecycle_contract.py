"""The supervisor half of the embedded process-lifecycle contract.

The server publishes `<data_dir>/.proximadb-runtime.json` (pid, phase,
advancing heartbeat) before it does any slow work, because listeners bind
only after recovery completes. These tests pin the client behavior that
record exists to enable:

* startup waits while the server proves progress, instead of applying a fixed
  deadline to an unbounded recovery (proximaDB#1667 — a 1.3GB data dir was
  measured still recovering at 180s while the SDK gave up at 30s and silently
  fell back to another backend);
* a data dir with a LIVE owner is never silently double-spawned, and one with
  a STALE owner is reclaimed (anvai-labs/victor#911 — setsid'd servers outlive
  their parent and keep rewriting a dir the caller believed it had cleared).

Every decision is a pure function of the record plus an injected clock, so
the contract is testable without spawning a server.
"""

import json
import os
import time
from pathlib import Path

import pytest

from proximadb_sdk.embedded import (
    RUNTIME_STATE_FILE,
    EmbeddedConfig,
    read_runtime_state,
    runtime_state_is_stale,
)

HEARTBEAT_MS = 2000
STALE_AFTER = 15  # intervals; mirrors the server constant


def write_state(
    data_dir: Path,
    *,
    pid: int,
    phase: str = "recovering_storage",
    updated_at_ms: float | None = None,
) -> dict:
    state = {
        "pid": pid,
        "phase": phase,
        "started_at_ms": 0,
        "updated_at_ms": time.time() * 1000 if updated_at_ms is None else updated_at_ms,
        "heartbeat_interval_ms": HEARTBEAT_MS,
    }
    data_dir.mkdir(parents=True, exist_ok=True)
    (data_dir / RUNTIME_STATE_FILE).write_text(json.dumps(state))
    return state


# ── Reading the record ────────────────────────────────────────────────────


def test_absent_record_reads_as_no_owner(tmp_path: Path):
    assert read_runtime_state(tmp_path) is None


def test_corrupt_record_reads_as_no_owner(tmp_path: Path):
    (tmp_path / RUNTIME_STATE_FILE).write_text("{not json")
    assert (
        read_runtime_state(tmp_path) is None
    ), "a corrupt record must never wedge a client"


def test_record_round_trips(tmp_path: Path):
    write_state(tmp_path, pid=os.getpid(), phase="binding")
    state = read_runtime_state(tmp_path)
    assert state is not None
    assert state["phase"] == "binding"
    assert state["pid"] == os.getpid()


# ── Staleness: the decision that separates "slow" from "dead" ─────────────


def test_live_pid_with_fresh_heartbeat_is_not_stale(tmp_path: Path):
    now = time.time() * 1000
    state = write_state(tmp_path, pid=os.getpid(), updated_at_ms=now - HEARTBEAT_MS)
    assert runtime_state_is_stale(state, now) is False


def test_dead_pid_is_stale_even_with_fresh_heartbeat(tmp_path: Path):
    now = time.time() * 1000
    state = write_state(tmp_path, pid=0x7FFFFFFF, updated_at_ms=now)
    assert (
        runtime_state_is_stale(state, now) is True
    ), "a vanished owner cannot hold the data dir, however recent its beat"


def test_frozen_heartbeat_is_stale(tmp_path: Path):
    now = time.time() * 1000
    state = write_state(
        tmp_path,
        pid=os.getpid(),
        updated_at_ms=now - HEARTBEAT_MS * (STALE_AFTER + 5),
    )
    assert runtime_state_is_stale(state, now) is True, "wedged owner ⇒ stale"


def test_a_few_missed_beats_are_tolerated(tmp_path: Path):
    # The whole point of the contract: a busy or briefly paused server must
    # not be declared dead, or supervisors resume killing healthy servers.
    now = time.time() * 1000
    state = write_state(tmp_path, pid=os.getpid(), updated_at_ms=now - HEARTBEAT_MS * 3)
    assert runtime_state_is_stale(state, now) is False


# ── Config knobs the contract introduces ──────────────────────────────────


def test_startup_ceiling_is_generous_and_configurable():
    cfg = EmbeddedConfig()
    assert cfg.startup_timeout_s >= 300, (
        "the ceiling is a safety stop, not a schedule; a small constant "
        "reintroduces proximaDB#1667"
    )
    assert EmbeddedConfig(startup_timeout_s=42.0).startup_timeout_s == 42.0


def test_foreign_owner_protection_defaults_on():
    assert EmbeddedConfig().fail_on_foreign_owner is True


# ── Ownership decisions at spawn time ─────────────────────────────────────


@pytest.mark.asyncio
async def test_start_refuses_when_a_live_server_owns_the_dir(tmp_path: Path):
    from proximadb_sdk.embedded import EmbeddedProximaDB

    write_state(tmp_path, pid=os.getpid(), phase="serving")
    db = EmbeddedProximaDB(str(tmp_path))
    with pytest.raises(RuntimeError, match="already serving"):
        await db.start()


@pytest.mark.asyncio
async def test_start_reclaims_a_stale_record(tmp_path: Path, monkeypatch):
    """A dead owner's record is removed, not obeyed — otherwise a crashed
    server would permanently lock its data dir out of use."""
    from proximadb_sdk.embedded import EmbeddedProximaDB

    now = time.time() * 1000
    write_state(tmp_path, pid=0x7FFFFFFF, updated_at_ms=now, phase="serving")

    db = EmbeddedProximaDB(str(tmp_path))
    # Stop before spawning: we only assert the reclaim decision.
    monkeypatch.setattr(
        db,
        "_find_binary",
        lambda: (_ for _ in ()).throw(FileNotFoundError("no binary")),
    )
    with pytest.raises(FileNotFoundError):
        await db.start()
    assert not (
        tmp_path / RUNTIME_STATE_FILE
    ).exists(), "a stale record must be reclaimed so the dir is usable again"


@pytest.mark.asyncio
async def test_second_writer_allowed_only_when_explicitly_requested(
    tmp_path: Path, monkeypatch
):
    from proximadb_sdk.embedded import EmbeddedProximaDB

    write_state(tmp_path, pid=os.getpid(), phase="serving")
    db = EmbeddedProximaDB(
        config=EmbeddedConfig(data_dir=str(tmp_path), fail_on_foreign_owner=False)
    )
    monkeypatch.setattr(
        db,
        "_find_binary",
        lambda: (_ for _ in ()).throw(FileNotFoundError("no binary")),
    )
    # Proceeds past the ownership gate (fails later, at binary discovery).
    with pytest.raises(FileNotFoundError):
        await db.start()


# ── Ownership at process exit (victor#911) ────────────────────────────────


def test_spawned_servers_are_registered_for_exit_teardown(tmp_path: Path):
    """Every spawn registers teardown, so a setsid'd child cannot outlive the
    interpreter that started it — the orphan mechanism behind victor#911."""
    from proximadb_sdk import embedded as emb

    class FakeDB:
        def __init__(self):
            self._process = object()
            self.killed = False

        def _kill_process(self):
            self.killed = True
            self._process = None

    fake = FakeDB()
    emb._register_owned_server(fake)
    assert fake in emb._OWNED_SERVERS

    emb._stop_owned_servers()
    assert fake.killed, "exit teardown must stop servers this process started"


def test_exit_teardown_never_raises(tmp_path: Path):
    """The exit path must be unconditionally safe: a handle that explodes on
    teardown cannot be allowed to break interpreter shutdown."""
    from proximadb_sdk import embedded as emb

    class Exploding:
        _process = object()

        def _kill_process(self):
            raise RuntimeError("boom")

    emb._register_owned_server(Exploding())
    emb._stop_owned_servers()  # must not raise


def test_teardown_is_idempotent():
    """Stopping twice is harmless — stop() and the atexit hook both run in a
    normal clean shutdown."""
    from proximadb_sdk import embedded as emb

    class FakeDB:
        def __init__(self):
            self._process = object()
            self.kills = 0

        def _kill_process(self):
            self.kills += 1
            self._process = None

    fake = FakeDB()
    emb._register_owned_server(fake)
    emb._stop_owned_servers()
    emb._stop_owned_servers()
    assert fake.kills == 1, "a stopped server is not stopped again"


def test_explicit_timeout_overrides_the_config_ceiling():
    """An explicit `start(timeout=...)` must be honored exactly.

    An earlier draft took max(caller, config) — which silently promoted a
    1s probe into a 900s wait. The generous ceiling exists for the IMPLICIT
    case (the one that used to hard-code 30s); a caller with its own deadline
    outranks it.
    """
    import inspect

    from proximadb_sdk.embedded import EmbeddedProximaDB

    sig = inspect.signature(EmbeddedProximaDB.start)
    assert sig.parameters["timeout"].default is None, (
        "timeout must default to None so the config ceiling applies only when "
        "the caller expressed no deadline"
    )
    src = inspect.getsource(EmbeddedProximaDB.start)
    assert (
        "max(float(timeout)" not in src
    ), "the ceiling must not be max(caller, config) — that overrides callers"
