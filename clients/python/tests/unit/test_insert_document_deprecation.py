"""Deprecation contract tests for ``insert_document`` (ADR-041, P3).

P3 deprecates the three public ``insert_document`` facades in favor of
``ingest_documents``. These tests pin the deprecation CONTRACT — not legacy
behavior — so a careless edit (wrong stacklevel, missing warning, changed
message) fails loudly here:

1. each public facade emits a ``DeprecationWarning`` citing ADR-041,
2. the warning's reported ``filename`` is the CALLER's frame (stacklevel=3 guard),
3. the return value is unchanged when the warning is suppressed (behavior-preserving),
4. the shared helper's message names the replacement surface.

All offline: the unified facade uses a ``MagicMock`` adapter (returns a dict, so
no fallback path), the document facade runs the real ``DocumentRepository``
against a mock client, and the async embedded facade runs against a patched
``httpx.AsyncClient``.
"""

import asyncio
import warnings
from unittest.mock import MagicMock

import httpx
import pytest

from proximadb_sdk._deprecations import warn_insert_document_deprecated
from proximadb_sdk.config import Protocol
from proximadb_sdk.document import ProximaDBDocument
from proximadb_sdk.embedded import EmbeddedProximaDB
from proximadb_sdk.unified_client import ProximaDBClient

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


def make_unified_client():
    """REST-mode client whose adapter returns a dict (no fallback path)."""
    c = ProximaDBClient(url="http://testserver:5678", protocol="rest")
    adapter = MagicMock()
    adapter.insert_document.return_value = {
        "id": "d1",
        "version": 3,
        "document": {"a": 1},
    }
    c._adapter = adapter
    c._client = MagicMock()
    c._active_protocol = Protocol.REST
    return c, adapter


def make_document_facade():
    """``ProximaDBDocument`` over a mock client (real DocumentRepository)."""
    client = MagicMock()
    client.insert_document.return_value = {}  # server echoes no id -> keep caller id
    return ProximaDBDocument(client), client


# --- minimal async fakes (self-contained; mirror test_embedded_cov.py) ---


def _run(coro):
    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(coro)
    finally:
        loop.close()


class _FakeResp:
    def __init__(self, json_body):
        self._json = json_body
        self.status_code = 200
        self.headers = {}
        self.text = ""

    def json(self):
        return self._json

    def raise_for_status(self):
        pass


class _FakeAsyncClient:
    """Drop-in for ``httpx.AsyncClient`` (async context manager)."""

    def __init__(self, *a, **kw):
        pass

    async def __aenter__(self):
        return self

    async def __aexit__(self, *a):
        return False

    async def post(self, url, **kw):
        return _FakeResp({"id": "doc1", "version": 1})


@pytest.fixture
def started_embedded(monkeypatch):
    """An ``EmbeddedProximaDB`` flagged started, with httpx patched out."""
    monkeypatch.setattr(httpx, "AsyncClient", _FakeAsyncClient)
    db = EmbeddedProximaDB(data_dir="/tmp/proximadb-test-deprecation")
    db._started = True
    return db


# ---------------------------------------------------------------------------
# Facade 1: ProximaDBClient.insert_document
# ---------------------------------------------------------------------------


def test_unified_facade_warns_and_points_at_caller():
    c, _ = make_unified_client()
    with pytest.warns(DeprecationWarning, match="ADR-041") as record:
        result = c.insert_document("coll", {"a": 1})
    # stacklevel=3 guard: the warning is attributed to THIS test file, not SDK internals.
    assert record[0].filename == __file__
    assert "ingest_documents" in str(record[0].message)
    assert result == {"id": "d1", "version": 3, "document": {"a": 1}}


def test_unified_facade_behavior_unchanged_when_suppressed():
    c, adapter = make_unified_client()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        result = c.insert_document("coll", {"a": 1}, id="x")
    adapter.insert_document.assert_called_once()
    assert result == adapter.insert_document.return_value


# ---------------------------------------------------------------------------
# Facade 2: ProximaDBDocument.insert_document
# ---------------------------------------------------------------------------


def test_document_facade_warns_and_points_at_caller():
    docs, _ = make_document_facade()
    with pytest.warns(DeprecationWarning, match="ADR-041") as record:
        result = docs.insert_document("code_files", {"content": "hi"}, id="doc:main.py")
    assert record[0].filename == __file__
    assert "ingest_documents" in str(record[0].message)
    # Behavior unchanged: the legacy {id, version, document} shape is preserved.
    assert result["id"] == "doc:main.py"
    assert result["document"] == {"content": "hi"}
    assert "version" in result


def test_document_facade_behavior_unchanged_when_suppressed():
    docs, _ = make_document_facade()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        result = docs.insert_document("code_files", {"content": "hi"}, id="doc:main.py")
    assert result["id"] == "doc:main.py"


# ---------------------------------------------------------------------------
# Facade 3: EmbeddedProximaDB.insert_document (async)
# ---------------------------------------------------------------------------


def test_embedded_facade_warns_with_async_note(started_embedded):
    with pytest.warns(DeprecationWarning, match="ADR-041") as record:
        result = _run(started_embedded.insert_document("coll", {"content": "hi"}))
    # NOTE: we do NOT assert record[0].filename here. ``warnings.warn``'s
    # stacklevel is unreliable across the coroutine/event-loop boundary (the
    # reported frame lands in asyncio internals, not the caller) — a known
    # Python limitation. The warning still fires with the canonical message;
    # that message is what guides migration. The deterministic stacklevel guard
    # lives in the two SYNC facade tests above.
    msg = str(record[0].message)
    assert "ingest_documents" in msg
    # Async variant appends the future-work note.
    assert "async ingest_documents variant is planned" in msg
    assert result == {"id": "doc1", "version": 1}


def test_embedded_facade_behavior_unchanged_when_suppressed(started_embedded):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        result = _run(started_embedded.insert_document("coll", {"content": "hi"}))
    assert result == {"id": "doc1", "version": 1}


# ---------------------------------------------------------------------------
# Helper message contract
# ---------------------------------------------------------------------------


def test_helper_message_names_replacement_surface():
    with pytest.warns(DeprecationWarning) as record:
        warn_insert_document_deprecated()
    assert len(record) == 1
    msg = str(record[0].message)
    for needle in (
        "insert_document()",
        "ingest_documents()",
        "ADR-041",
        "removed in a future minor",
    ):
        assert needle in msg, f"message missing {needle!r}: {msg!r}"
