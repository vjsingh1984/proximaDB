"""Embedded SDK HTTP record-write migration tests."""

import pytest

from proximadb_sdk.embedded import EmbeddedConfig, EmbeddedProximaDB


class JsonResponse:
    def __init__(self, payload):
        self._payload = payload

    def json(self):
        return self._payload

    def raise_for_status(self):
        # Success path: the real httpx Response is a no-op on 2xx.
        return None


class RecordingAsyncClient:
    calls = []

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc, tb):
        return False

    async def post(self, url, *, json, timeout):
        self.calls.append((url, json, timeout))
        return JsonResponse({"inserted_count": len(json["records"]), "failed_count": 0})


@pytest.mark.asyncio
async def test_embedded_insert_records_uses_v2_record_endpoint(monkeypatch, tmp_path):
    import httpx

    RecordingAsyncClient.calls = []
    monkeypatch.setattr(httpx, "AsyncClient", RecordingAsyncClient)

    # transport="tcp" exercises the plain httpx path, so no UDS socket is needed.
    #
    # Constructed normally rather than by bypassing the constructor. Skipping
    # __init__ to avoid socket setup is what broke these two tests: when
    # HTTP-client pooling added `self._shared_http_client` to __init__, the
    # half-built object stopped carrying the attribute `_http_client` reads, and
    # both failed with an AttributeError that pointed at the code under test
    # rather than at their own setup. A test that skips initialisation breaks
    # every time initialisation grows.
    db = EmbeddedProximaDB(
        config=EmbeddedConfig(data_dir=str(tmp_path), rest_port=15678, transport="tcp")
    )

    result = await db._insert_records(
        "items",
        [{"id": "r1", "vector": [1, 2], "metadata": {"kind": "note"}}],
    )

    assert result == {"inserted_count": 1, "failed_count": 0}
    url, payload, timeout = RecordingAsyncClient.calls[0]
    assert url == "http://localhost:15678/api/v2/collections/items/records/batch"
    assert timeout == 60.0
    assert {
        "records": [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}],
        "validate_schema": True,
    }.items() <= payload.items()


@pytest.mark.asyncio
async def test_embedded_insert_vectors_aliases_record_insert(monkeypatch, tmp_path):
    import httpx

    RecordingAsyncClient.calls = []
    monkeypatch.setattr(httpx, "AsyncClient", RecordingAsyncClient)

    db = EmbeddedProximaDB(
        config=EmbeddedConfig(data_dir=str(tmp_path), rest_port=15678, transport="tcp")
    )

    await db._insert_vectors("items", [{"id": "r1", "vector": [1, 2]}])

    assert RecordingAsyncClient.calls[0][0].endswith(
        "/api/v2/collections/items/records/batch"
    )
