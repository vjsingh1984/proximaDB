"""Native-async gRPC client tests (TD-126).

These prove ``protocols/grpc_async.ProximaDBAsyncGrpcClient`` is a *genuine*
``grpc.aio`` client (no longer a sync subclass):

  * Each core op ``await``\\s the correct ``ProximaRecordServiceStub`` RPC.
  * It builds the right v2 proto request (the shared transport-agnostic codec).
  * It parses real v2 response protos into the SAME public return types the
    sync client produces (BatchResult / list[SearchResult] / dict).
  * Context-manager lifecycle: ``connect`` opens the aio channel + binds the
    generated stub; ``close`` awaits ``channel.close()``.

We mock only the stub: each RPC is an *async coroutine factory* (so the client
must ``await`` it) that records the request proto and returns a real generated
response proto. Real request-building and response-parsing run end to end.
"""

import pytest

from proximadb.v2 import record_pb2 as pb
from proximadb_sdk.models import BatchResult, SearchResult
from proximadb_sdk.protocols.grpc_async import ProximaDBAsyncGrpcClient


class _FakeUnaryCall:
    """Awaitable returned by a fake RPC method (mimics aio MultiCallable())."""

    def __init__(self, recorder, name, request, response):
        recorder.append((name, request))
        self._response = response

    def __await__(self):
        async def _coro():
            return self._response

        return _coro().__await__()


class _FakeStub:
    """Fake ProximaRecordServiceStub whose RPCs are awaitable + recorded."""

    def __init__(self, responses):
        self.calls = []  # list[(rpc_name, request_proto)]
        self._responses = responses

    def _make(self, name):
        def _rpc(request, timeout=None):
            return _FakeUnaryCall(self.calls, name, request, self._responses[name])

        return _rpc

    def __getattr__(self, name):
        return self._make(name)


def _batch_response(*, total, success, failed, errors=None):
    resp = pb.ProximaRecordBatchResponse(
        success=failed == 0,
        total_processed=total,
        success_count=success,
        failed_count=failed,
        processing_time_us=123,
    )
    for err in errors or []:
        resp.errors.add(
            record_id=err.get("record_id", ""),
            record_index=err.get("record_index", 0),
            error_message=err.get("error_message", ""),
        )
    return resp


async def _connected_client(responses) -> ProximaDBAsyncGrpcClient:
    """Build a client and inject a fake stub + pb2 (no real channel/grpc)."""
    client = ProximaDBAsyncGrpcClient("localhost:5679")
    client._stub = _FakeStub(responses)
    client._pb2 = pb
    # Mark connected without opening a socket.
    client._channel = object()
    return client


@pytest.mark.asyncio
async def test_insert_records_awaits_InsertRecords_and_parses_batch():
    stub_responses = {
        "InsertRecords": _batch_response(total=2, success=2, failed=0),
    }
    client = await _connected_client(stub_responses)
    records = [
        {"id": "a", "vector": [1.0, 2.0], "metadata": {"k": "v"}},
        {"id": "b", "vector": [3.0, 4.0]},
    ]
    result = await client.insert_records("coll", records)

    # awaited the right RPC
    assert [name for name, _ in client._stub.calls] == ["InsertRecords"]
    _, req = client._stub.calls[0]
    assert req.collection_id == "coll"
    assert req.write_mode == pb.INSERT
    assert [r.id for r in req.records] == ["a", "b"]
    assert list(req.records[0].vector) == [1.0, 2.0]
    # parsed to the sync return type
    assert isinstance(result, BatchResult)
    assert result.total == 2 and result.success == 2 and result.failed == 0


@pytest.mark.asyncio
async def test_upsert_records_awaits_UpsertRecords():
    client = await _connected_client(
        {"UpsertRecords": _batch_response(total=1, success=1, failed=0)}
    )
    result = await client.upsert_records("coll", [{"id": "x", "vector": [1.0]}])
    assert [name for name, _ in client._stub.calls] == ["UpsertRecords"]
    _, req = client._stub.calls[0]
    assert req.write_mode == pb.UPSERT
    assert isinstance(result, BatchResult)
    assert result.success == 1


@pytest.mark.asyncio
async def test_insert_records_upsert_kwarg_routes_to_upsert():
    client = await _connected_client(
        {"UpsertRecords": _batch_response(total=1, success=1, failed=0)}
    )
    await client.insert_records("coll", [{"id": "x", "vector": [1.0]}], upsert=True)
    assert [name for name, _ in client._stub.calls] == ["UpsertRecords"]


@pytest.mark.asyncio
async def test_delete_vector_awaits_DeleteRecords():
    client = await _connected_client(
        {"DeleteRecords": _batch_response(total=1, success=1, failed=0)}
    )
    res = await client.delete_vector("coll", "v1")
    assert [name for name, _ in client._stub.calls] == ["DeleteRecords"]
    _, req = client._stub.calls[0]
    assert req.write_mode == pb.DELETE
    assert [r.id for r in req.records] == ["v1"]
    assert res["success"] is True and res["vector_id"] == "v1"


@pytest.mark.asyncio
async def test_delete_vectors_batch_awaits_DeleteRecords_once():
    client = await _connected_client(
        {"DeleteRecords": _batch_response(total=3, success=3, failed=0)}
    )
    res = await client.delete_vectors("coll", ["a", "b", "c"])
    assert [name for name, _ in client._stub.calls] == ["DeleteRecords"]
    _, req = client._stub.calls[0]
    assert [r.id for r in req.records] == ["a", "b", "c"]
    assert res["deleted_count"] == 3 and res["failed_count"] == 0


@pytest.mark.asyncio
async def test_search_awaits_Search_and_parses_results():
    resp = pb.TypedSearchResponse()
    item = resp.results.add()
    item.id = "doc1"
    item.score = 0.99
    item.props["color"].text_value = "red"
    item.props["color"].declared_type = pb.TEXT
    client = await _connected_client({"Search": resp})

    results = await client.search(
        "coll",
        query_vector=[0.1, 0.2],
        top_k=5,
        metadata_filters={"color": "red"},
        include_metadata=True,
    )
    assert [name for name, _ in client._stub.calls] == ["Search"]
    _, req = client._stub.calls[0]
    assert req.collection_id == "coll"
    assert req.top_k == 5
    assert list(req.query_vector) == pytest.approx([0.1, 0.2])
    assert req.filters[0].field_name == "color"
    assert req.filters[0].operator == pb.EQ
    # parsed
    assert isinstance(results, list) and isinstance(results[0], SearchResult)
    assert results[0].id == "doc1"
    assert results[0].score == pytest.approx(0.99)
    assert results[0].metadata == {"color": "red"}


@pytest.mark.asyncio
async def test_get_vector_awaits_GetRecord_and_parses_dict():
    resp = pb.GetRecordResponse(found=True)
    resp.record.id = "v1"
    resp.record.vector.extend([1.0, 2.0, 3.0])
    resp.record.props["n"].CopyFrom(pb.TypedValue(integer_value=7))
    resp.record.props["n"].declared_type = pb.INTEGER
    client = await _connected_client({"GetRecord": resp})

    out = await client.get_vector("coll", "v1")
    assert [name for name, _ in client._stub.calls] == ["GetRecord"]
    _, req = client._stub.calls[0]
    assert req.collection_id == "coll" and req.id == "v1"
    assert out["id"] == "v1"
    assert out["vector"] == [1.0, 2.0, 3.0]
    assert out["metadata"] == {"n": 7}


@pytest.mark.asyncio
async def test_get_vector_not_found_raises():
    from proximadb_sdk.exceptions import ProximaDBError

    client = await _connected_client({"GetRecord": pb.GetRecordResponse(found=False)})
    with pytest.raises(ProximaDBError):
        await client.get_vector("coll", "missing")


@pytest.mark.asyncio
async def test_require_before_connect_raises():
    from proximadb_sdk.exceptions import ProximaDBError

    client = ProximaDBAsyncGrpcClient("localhost:5679")
    with pytest.raises(ProximaDBError):
        await client.insert_records("coll", [{"id": "x", "vector": [1.0]}])


@pytest.mark.asyncio
async def test_target_strips_scheme():
    assert ProximaDBAsyncGrpcClient("http://h:5679").target == "h:5679"
    assert ProximaDBAsyncGrpcClient("grpc://h:5679/").target == "h:5679"
    assert ProximaDBAsyncGrpcClient("h:5679").target == "h:5679"


@pytest.mark.asyncio
async def test_lifecycle_connect_opens_channel_close_awaits():
    """connect() builds a real grpc.aio channel + generated stub; close() awaits it."""
    grpc_aio = pytest.importorskip("grpc.aio")
    assert grpc_aio is not None

    client = ProximaDBAsyncGrpcClient("localhost:5679")
    assert client._channel is None
    await client.connect()
    import grpc.aio as _aio

    assert isinstance(client._channel, _aio.Channel)
    # generated stub bound on the aio channel; its RPCs are awaitable callables
    assert client._stub is not None
    assert isinstance(client._stub.InsertRecords, _aio.UnaryUnaryMultiCallable)
    await client.close()
    assert client._channel is None and client._stub is None


@pytest.mark.asyncio
async def test_async_context_manager_lifecycle():
    async with ProximaDBAsyncGrpcClient("localhost:5679") as client:
        assert client._channel is not None
        assert client._stub is not None
    assert client._channel is None
