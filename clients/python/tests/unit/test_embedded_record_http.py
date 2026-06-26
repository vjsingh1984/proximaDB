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
async def test_embedded_insert_records_uses_v2_record_endpoint(monkeypatch):
    import httpx

    RecordingAsyncClient.calls = []
    monkeypatch.setattr(httpx, "AsyncClient", RecordingAsyncClient)

    # transport="tcp" exercises the plain httpx path (no UDS socket dir needed);
    # the embedded default changed to "uds", which requires _socket_dir setup.
    db = EmbeddedProximaDB.__new__(EmbeddedProximaDB)
    db.config = EmbeddedConfig(rest_port=15678, transport="tcp")

    result = await db._insert_records(
        "items",
        [{"id": "r1", "vector": [1, 2], "metadata": {"kind": "note"}}],
    )

    assert result == {"inserted_count": 1, "failed_count": 0}
    url, payload, timeout = RecordingAsyncClient.calls[0]
    assert url == "http://localhost:15678/api/v2/collections/items/records/batch"
    assert timeout == 60.0
    assert payload == {
        "records": [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}],
        "validate_schema": True,
    }


@pytest.mark.asyncio
async def test_embedded_insert_vectors_aliases_record_insert(monkeypatch):
    import httpx

    RecordingAsyncClient.calls = []
    monkeypatch.setattr(httpx, "AsyncClient", RecordingAsyncClient)

    db = EmbeddedProximaDB.__new__(EmbeddedProximaDB)
    db.config = EmbeddedConfig(rest_port=15678, transport="tcp")

    await db._insert_vectors("items", [{"id": "r1", "vector": [1, 2]}])

    assert RecordingAsyncClient.calls[0][0].endswith(
        "/api/v2/collections/items/records/batch"
    )
