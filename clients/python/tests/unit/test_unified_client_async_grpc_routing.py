"""Facade routing tests: ProximaDBAsyncUnified routes core ops to gRPC-async.

When a gRPC endpoint is configured, the async facade must dispatch core record
ops (insert/upsert/delete/search/get_vector) to the native-async gRPC client
(mirroring the sync unified client's gRPC-over-REST preference) and NOT to the
generated async-REST path. We inject a fake async gRPC client (so no socket /
event-loop-blocking call runs) and assert the right coroutine is awaited with
the right arguments, and that the public return types match.
"""

import pytest

from proximadb_sdk.models import (
    BatchResult,
    DeleteResult,
    OperationMetrics,
    SearchResult,
)
from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified


class _FakeAsyncGrpc:
    """Records awaited core-op calls; returns sync-equivalent values."""

    def __init__(self):
        self.calls = []
        self.closed = False

    async def insert_records(self, collection_id, records, **kw):
        self.calls.append(("insert_records", collection_id, records, kw))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            errors=[],
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )

    async def upsert_records(self, collection_id, records, **kw):
        self.calls.append(("upsert_records", collection_id, records, kw))
        return BatchResult(total=1, success=1, failed=0, errors=[])

    async def search(self, collection_id, **kw):
        self.calls.append(("search", collection_id, kw))
        return [SearchResult(id="x", score=1.0, rank=1)]

    async def get_vector(self, collection_id, vector_id, **kw):
        self.calls.append(("get_vector", collection_id, vector_id, kw))
        return {"id": vector_id, "vector": [1.0]}

    async def delete_vector(self, collection_id, vector_id):
        self.calls.append(("delete_vector", collection_id, vector_id))
        return {"status": "deleted", "vector_id": vector_id, "success": True}

    async def delete_vectors(self, collection_id, vector_ids):
        self.calls.append(("delete_vectors", collection_id, vector_ids))
        return {
            "status": "completed",
            "deleted_count": len(vector_ids),
            "failed_count": 0,
            "total_requested": len(vector_ids),
        }

    async def close(self):
        self.closed = True


def _grpc_facade():
    """Facade with a fake gRPC-async client wired in (no astart / no socket)."""
    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="grpc")
    fake = _FakeAsyncGrpc()
    client._grpc = fake
    client._use_grpc = True
    return client, fake


@pytest.mark.asyncio
async def test_insert_routes_to_grpc_async():
    client, fake = _grpc_facade()
    res = await client.insert_records("coll", [{"id": "a", "vector": [1.0]}])
    assert fake.calls[0][0] == "insert_records"
    assert isinstance(res, BatchResult) and res.success == 1


@pytest.mark.asyncio
async def test_upsert_routes_to_grpc_async_as_insert_upsert():
    client, fake = _grpc_facade()
    await client.upsert_records("coll", [{"id": "a", "vector": [1.0]}])
    # upsert_records -> insert_records(upsert=True) -> grpc.insert_records(upsert=True)
    assert fake.calls[0][0] == "insert_records"
    assert fake.calls[0][3].get("upsert") is True


@pytest.mark.asyncio
async def test_search_routes_to_grpc_async():
    client, fake = _grpc_facade()
    res = await client.search("coll", [0.1, 0.2], top_k=3, metadata_filter={"k": "v"})
    assert fake.calls[0][0] == "search"
    kw = fake.calls[0][2]
    assert kw["top_k"] == 3
    assert kw["metadata_filters"] == {"k": "v"}
    assert isinstance(res, list) and isinstance(res[0], SearchResult)


@pytest.mark.asyncio
async def test_get_vector_routes_to_grpc_async():
    client, fake = _grpc_facade()
    out = await client.get_vector("coll", "v1")
    assert fake.calls[0][0] == "get_vector"
    assert out["id"] == "v1"


@pytest.mark.asyncio
async def test_delete_vector_routes_to_grpc_async_returns_DeleteResult():
    client, fake = _grpc_facade()
    res = await client.delete_vector("coll", "v1")
    assert fake.calls[0][0] == "delete_vector"
    assert isinstance(res, DeleteResult) and res.success and res.deleted_count == 1


@pytest.mark.asyncio
async def test_delete_vectors_routes_to_grpc_batch():
    client, fake = _grpc_facade()
    res = await client.delete_vectors("coll", ["a", "b"])
    assert fake.calls[0][0] == "delete_vectors"
    assert isinstance(res, DeleteResult) and res.deleted_count == 2 and res.success


@pytest.mark.asyncio
async def test_rest_path_used_when_grpc_disabled():
    """When _use_grpc is False, core ops do NOT touch the gRPC client."""
    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="rest")
    fake = _FakeAsyncGrpc()
    client._grpc = fake
    client._use_grpc = False
    # require_gen_client guards: not started, so this raises (REST path taken,
    # NOT the gRPC client) — proving routing keyed on _use_grpc.
    with pytest.raises(RuntimeError):
        await client.insert_records("coll", [{"id": "a", "vector": [1.0]}])
    assert fake.calls == []


@pytest.mark.asyncio
async def test_aclose_awaits_grpc_close():
    client, fake = _grpc_facade()
    await client.aclose()
    assert fake.closed is True
    assert client._grpc is None and client._use_grpc is False
