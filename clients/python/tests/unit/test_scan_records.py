"""Unit tests for the spec-driven vector-free metadata-scan surface.

The scan method (sync ``ProximaDBClient.scan``/``scan_page`` and async
``ProximaDBAsyncUnified.scan``/``scan_page``) wires the GENERATED ``scan_records``
endpoint (``sync``/``asyncio_detailed``) through the existing facade plumbing
(``_rest_codegen`` / ``_rest_codegen_async``). These tests mock only the HTTP
transport so the real generated request-building runs end to end, and assert:

  * ``scan`` builds the right request body (filter + limit + cursor) on the wire.
  * cursor pagination loops until ``next_cursor`` is exhausted.
  * empty results are handled.
  * records are mapped into the public ``SearchResult`` type.
"""

from __future__ import annotations

import json

import httpx
import pytest

from proximadb_sdk.models import SearchResult
from proximadb_sdk.unified_client import ProximaDBClient
from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

SCAN_PATH = "/api/v2/collections/docs/records/scan"


def _record(rid: str, props: dict) -> dict:
    return {"id": rid, "props": props, "version": 1, "timestamp": 123}


class _ScanRecorder:
    """Serves canned scan pages keyed by the request's cursor, records bodies."""

    def __init__(self, pages: list[dict]):
        # pages: list of response bodies served in order, indexed by call count.
        self.pages = pages
        self.bodies: list[dict] = []
        self.calls = 0

    def handler(self, request: httpx.Request) -> httpx.Response:
        if request.url.path != SCAN_PATH or request.method != "POST":
            return httpx.Response(404, json={"error": "no mock route"})
        self.bodies.append(json.loads(request.content.decode() or "{}"))
        body = self.pages[min(self.calls, len(self.pages) - 1)]
        self.calls += 1
        return httpx.Response(200, json=body)


# ---------------------------------------------------------------------------
# Sync facade — mocks the generated ``scan_records.sync`` transport.
# ---------------------------------------------------------------------------


def _sync_client(rec: _ScanRecorder) -> ProximaDBClient:
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    rest = c._create_rest_client()
    rest._http_client = httpx.Client(
        base_url="http://testserver",
        transport=httpx.MockTransport(rec.handler),
    )
    c._rest_client = rest
    c._client = rest
    return c


def test_sync_scan_page_builds_filter_and_maps_results():
    rec = _ScanRecorder([{"records": [_record("a", {"k": "v"})], "next_cursor": None}])
    c = _sync_client(rec)

    records, cursor = c.scan_page(
        "docs",
        filter=[{"field": "k", "op": "eq", "value": "v"}],
        limit=50,
    )

    assert cursor is None
    assert len(records) == 1
    assert isinstance(records[0], SearchResult)
    assert records[0].id == "a"
    assert records[0].metadata == {"k": "v"}
    # The generated request body carried the filter + limit on the wire.
    sent = rec.bodies[0]
    assert sent["filter"] == [{"field": "k", "op": "eq", "value": "v"}]
    assert sent["limit"] == 50


def test_sync_scan_paginates_until_cursor_exhausted():
    rec = _ScanRecorder(
        [
            {"records": [_record("a", {})], "next_cursor": "c1"},
            {"records": [_record("b", {})], "next_cursor": "c2"},
            {"records": [_record("c", {})], "next_cursor": None},
        ]
    )
    c = _sync_client(rec)

    records = c.scan("docs", filter={"tenant": "t1"}, page_size=1)

    assert [r.id for r in records] == ["a", "b", "c"]
    assert rec.calls == 3
    # First page sends no cursor; subsequent pages echo the prior next_cursor.
    assert "cursor" not in rec.bodies[0]
    assert rec.bodies[1]["cursor"] == "c1"
    assert rec.bodies[2]["cursor"] == "c2"
    # Equality-map filter forwarded verbatim each page.
    assert all(b["filter"] == {"tenant": "t1"} for b in rec.bodies)


def test_sync_scan_empty():
    rec = _ScanRecorder([{"records": [], "next_cursor": None}])
    c = _sync_client(rec)
    records = c.scan("docs")
    assert records == []
    assert rec.calls == 1


def test_sync_scan_respects_max_rows():
    rec = _ScanRecorder(
        [
            {"records": [_record("a", {}), _record("b", {})], "next_cursor": "c1"},
            {"records": [_record("c", {})], "next_cursor": None},
        ]
    )
    c = _sync_client(rec)
    records = c.scan("docs", page_size=2, max_rows=2)
    # Stops at the cap without fetching the second page.
    assert [r.id for r in records] == ["a", "b"]
    assert rec.calls == 1


# ---------------------------------------------------------------------------
# Async facade — mocks the generated ``scan_records.asyncio_detailed`` transport.
# ---------------------------------------------------------------------------


async def _async_client(rec: _ScanRecorder) -> ProximaDBAsyncUnified:
    client = ProximaDBAsyncUnified(url="http://testserver", protocol="rest")
    await client.astart()
    mock = httpx.AsyncClient(
        base_url="http://testserver",
        transport=httpx.MockTransport(rec.handler),
    )
    client._async_http = mock
    client._gen_client.set_async_httpx_client(mock)
    return client


@pytest.mark.asyncio
async def test_async_scan_page_builds_filter_and_maps_results():
    rec = _ScanRecorder([{"records": [_record("a", {"k": "v"})], "next_cursor": None}])
    client = await _async_client(rec)
    try:
        records, cursor = await client.scan_page(
            "docs",
            filter=[{"field": "k", "op": "eq", "value": "v"}],
            limit=25,
        )
    finally:
        await client.aclose()

    assert cursor is None
    assert len(records) == 1
    assert isinstance(records[0], SearchResult)
    assert records[0].id == "a"
    assert records[0].metadata == {"k": "v"}
    assert rec.bodies[0]["filter"] == [{"field": "k", "op": "eq", "value": "v"}]
    assert rec.bodies[0]["limit"] == 25


@pytest.mark.asyncio
async def test_async_scan_paginates_until_cursor_exhausted():
    rec = _ScanRecorder(
        [
            {"records": [_record("a", {})], "next_cursor": "c1"},
            {"records": [_record("b", {})], "next_cursor": None},
        ]
    )
    client = await _async_client(rec)
    try:
        records = await client.scan("docs", filter={"x": 1}, page_size=1)
    finally:
        await client.aclose()

    assert [r.id for r in records] == ["a", "b"]
    assert rec.calls == 2
    assert "cursor" not in rec.bodies[0]
    assert rec.bodies[1]["cursor"] == "c1"


@pytest.mark.asyncio
async def test_async_scan_empty():
    rec = _ScanRecorder([{"records": [], "next_cursor": None}])
    client = await _async_client(rec)
    try:
        records = await client.scan("docs")
    finally:
        await client.aclose()
    assert records == []
    assert rec.calls == 1
