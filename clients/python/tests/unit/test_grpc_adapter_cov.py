"""Offline unit tests for proximadb_sdk.adapters.grpc_adapter.GrpcProtocolAdapter.

Fully offline: the underlying ProximaDBSyncGrpcClient is replaced with a hand
fake that never opens a channel. Each test injects a fake/MagicMock backend and
asserts the adapter shapes requests and parses responses into Pydantic models.
"""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

import proximadb_sdk.adapters.grpc_adapter as grpc_adapter_mod
from proximadb_sdk.adapters.grpc_adapter import GrpcProtocolAdapter
from proximadb_sdk.models import (
    BatchResult,
    Collection,
    CollectionConfig,
    HealthStatus,
    OperationMetrics,
    SearchResult,
    VectorOperationResponse,
    VectorRecord,
)


class FakeGrpcClient:
    """Stand-in for ProximaDBSyncGrpcClient that never opens a channel."""

    def __init__(self, server_address, timeout=60.0, pool_size=5, max_message_size=0):
        self.server_address = server_address
        self.timeout = timeout
        self.pool_size = pool_size
        self.max_message_size = max_message_size
        self.closed = False

    def close(self):
        self.closed = True


@pytest.fixture(autouse=True)
def patch_grpc_client(monkeypatch):
    """Replace the underlying gRPC client class so __init__ never connects."""
    monkeypatch.setattr(
        grpc_adapter_mod,
        "ProximaDBSyncGrpcClient",
        FakeGrpcClient,
        raising=False,
    )
    # The adapter imports the symbol lazily inside __init__ from the protocols
    # module, so patch it there too.
    import proximadb_sdk.protocols.grpc_sync as grpc_sync_mod

    monkeypatch.setattr(grpc_sync_mod, "ProximaDBSyncGrpcClient", FakeGrpcClient)


def make_adapter(**kwargs):
    return GrpcProtocolAdapter(server_address="localhost:5678", **kwargs)


# ---------------------------------------------------------------------------
# Construction / properties
# ---------------------------------------------------------------------------


def test_init_basic_properties():
    a = make_adapter()
    assert a.protocol_name == "grpc"
    assert a.is_connected is True
    assert isinstance(a._client, FakeGrpcClient)
    assert a._client.server_address == "localhost:5678"


def test_init_strips_known_kwargs():
    a = make_adapter(auth="token", url="http://x", base_url="http://y", timeout=12.0)
    assert a._client.timeout == 12.0


def test_init_config_overrides_default_address():
    cfg = SimpleNamespace(url="http://example.com:9999", base_url=None)
    a = GrpcProtocolAdapter(config=cfg)
    assert a._server_address == "example.com:9999"


def test_init_config_base_url_when_no_url():
    cfg = SimpleNamespace(url=None, base_url="https://host:7000")
    a = GrpcProtocolAdapter(config=cfg)
    assert a._server_address == "host:7000"


def test_init_config_ignored_when_explicit_address():
    cfg = SimpleNamespace(url="http://example.com:9999", base_url=None)
    a = GrpcProtocolAdapter(server_address="custom:1234", config=cfg)
    assert a._server_address == "custom:1234"


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


def test_health_healthy():
    a = make_adapter()
    a._client.health_check = MagicMock(
        return_value=SimpleNamespace(
            healthy=True, version="1.2.3", uptime_seconds=42, latency_ms=7
        )
    )
    hs = a.health()
    assert isinstance(hs, HealthStatus)
    assert hs.status == "healthy"
    assert hs.version == "1.2.3"
    assert hs.uptime_seconds == 42
    assert hs.timestamp_ms == 7
    assert hs.services["grpc"] == "ok"


def test_health_unhealthy_flag():
    a = make_adapter()
    a._client.health_check = MagicMock(
        return_value=SimpleNamespace(
            healthy=False, version=None, uptime_seconds=None, latency_ms=-5
        )
    )
    hs = a.health()
    assert hs.status == "running"
    assert hs.version == "0.0.0"
    assert hs.timestamp_ms == 0  # negative clamped to 0
    assert hs.services["grpc"] == "unavailable"


def test_health_no_healthy_attr():
    a = make_adapter()
    a._client.health_check = MagicMock(return_value=object())
    hs = a.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unknown"


def test_health_exception():
    a = make_adapter()
    a._client.health_check = MagicMock(side_effect=RuntimeError("boom"))
    hs = a.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unavailable"


# ---------------------------------------------------------------------------
# Collections
# ---------------------------------------------------------------------------


def test_create_collection_with_config():
    a = make_adapter()
    captured = {}

    def fake_create(**kw):
        captured.update(kw)
        return {"id": "c1", "name": "mycoll01", "dimension": 256}

    a._client.create_collection = fake_create
    cfg = CollectionConfig(
        name="mycoll01", dimension=256, canonical_embedding_precision="fp16"
    )
    coll = a.create_collection("mycoll01", config=cfg)
    assert isinstance(coll, Collection)
    assert coll.id == "c1"
    assert captured["dimension"] == 256
    assert captured["canonical_embedding_precision"] == 2  # fp16 -> 2


def test_create_collection_from_kwargs_no_config():
    a = make_adapter()
    captured = {}

    def fake_create(**kw):
        captured.update(kw)
        return {"id": "kid", "name": "kollname", "dimension": 64}

    a._client.create_collection = fake_create
    coll = a.create_collection(
        "kollname", dimension=64, canonical_embedding_precision="int8", extra_arg="z"
    )
    assert isinstance(coll, Collection)
    assert captured["dimension"] == 64
    assert captured["canonical_embedding_precision"] == 4  # int8 -> 4
    assert captured["extra_arg"] == "z"


def test_create_collection_precision_none():
    a = make_adapter()
    a._client.create_collection = MagicMock(
        return_value={"id": "pid", "name": "precname", "dimension": 8}
    )
    a.create_collection("precname", dimension=8)
    assert (
        a._client.create_collection.call_args.kwargs["canonical_embedding_precision"]
        is None
    )


def test_create_collection_default_dimension():
    a = make_adapter()
    a._client.create_collection = MagicMock(
        return_value={"id": "did", "name": "defdimen", "dimension": 128}
    )
    a.create_collection("defdimen")
    assert a._client.create_collection.call_args.kwargs["dimension"] == 128


def test_get_collection_found_dict():
    a = make_adapter()
    a._client.get_collection = MagicMock(
        return_value={"id": "g1", "name": "getcollx", "dimension": 32}
    )
    coll = a.get_collection("g1")
    assert isinstance(coll, Collection)
    assert coll.id == "g1"


def test_get_collection_none():
    a = make_adapter()
    a._client.get_collection = MagicMock(return_value=None)
    assert a.get_collection("missing") is None


def test_get_collection_exception_returns_none():
    a = make_adapter()
    a._client.get_collection = MagicMock(side_effect=RuntimeError("nope"))
    assert a.get_collection("err") is None


def test_list_collections_mixed():
    a = make_adapter()
    good = {"id": "okcollid", "name": "okcollnm", "dimension": 4}
    a._client.list_collections = MagicMock(return_value=[good, None])
    out = a.list_collections()
    # the None entry triggers an exception inside _to_collection and is skipped
    assert len(out) == 1
    assert out[0].id == "okcollid"


def test_list_collections_empty():
    a = make_adapter()
    a._client.list_collections = MagicMock(return_value=None)
    assert a.list_collections() == []


def test_delete_collection_with_success_attr():
    a = make_adapter()
    a._client.delete_collection = MagicMock(return_value=SimpleNamespace(success=True))
    assert a.delete_collection("c") is True


def test_delete_collection_no_success_attr():
    a = make_adapter()
    a._client.delete_collection = MagicMock(return_value="done")
    assert a.delete_collection("c") is True


def test_delete_collection_exception():
    a = make_adapter()
    a._client.delete_collection = MagicMock(side_effect=RuntimeError("x"))
    assert a.delete_collection("c") is False


# ---------------------------------------------------------------------------
# _to_collection direct branches
# ---------------------------------------------------------------------------


def test_to_collection_passthrough_collection():
    a = make_adapter()
    existing = Collection(
        id="x", config=CollectionConfig(name="passcoll", dimension=1)
    )
    assert a._to_collection(existing) is existing


def test_to_collection_dict_with_config():
    a = make_adapter()
    payload = {"id": "y", "config": {"name": "dictcoll", "dimension": 3}}
    coll = a._to_collection(payload)
    assert coll.config.dimension == 3


def test_to_collection_protobuf_like_object():
    a = make_adapter()
    obj = SimpleNamespace(id="pb", name="pbobjnam", dimension=16)
    coll = a._to_collection(obj)
    assert coll.id == "pb"
    assert coll.config.dimension == 16


def test_to_collection_object_uses_fallbacks():
    a = make_adapter()
    obj = object()  # no name/dimension/id attrs
    coll = a._to_collection(obj, fallback_name="fbcollnm", fallback_dimension=7)
    assert coll.config.name == "fbcollnm"
    assert coll.config.dimension == 7


# ---------------------------------------------------------------------------
# Record / vector inserts & upserts
# ---------------------------------------------------------------------------


def test_record_payloads_variants():
    dict_rec = {"id": "d", "vector": [1.0]}
    model_rec = VectorRecord(id="m", vector=[2.0])
    payloads = GrpcProtocolAdapter._record_payloads([dict_rec, model_rec])
    assert payloads[0] == dict_rec
    assert payloads[1]["id"] == "m"


def test_record_payloads_proto_conversion_branch():
    # Object without model_dump -> routed through ProtoConverter.vector_record_to_dict
    raw = SimpleNamespace(id="p", vector=[1.0, 2.0], metadata={"k": "v"})
    payloads = GrpcProtocolAdapter._record_payloads([raw])
    assert payloads[0]["id"] == "p"
    assert payloads[0]["vector"] == [1.0, 2.0]


def test_insert_records_via_insert_records():
    a = make_adapter()
    a._client.insert_records = MagicMock(
        return_value={"success": True, "successful_count": 2, "failed_count": 0}
    )
    res = a.insert_records(
        "c", [{"id": "1", "vector": [1.0]}, {"id": "2", "vector": [2.0]}]
    )
    assert isinstance(res, BatchResult)
    assert res.total == 2
    assert res.success == 2
    assert res.failed == 0
    a._client.insert_records.assert_called_once()


def test_insert_records_fallback_insert_vectors():
    a = make_adapter()
    a._client.insert_vectors = MagicMock(
        return_value={"success": True, "successful_count": 1, "failed_count": 0}
    )
    res = a.insert_records("c", [{"id": "1", "vector": [1.0]}])
    assert res.success == 1
    a._client.insert_vectors.assert_called_once()


def test_upsert_records_via_upsert_records():
    a = make_adapter()
    a._client.upsert_records = MagicMock(
        return_value={"success": True, "successful_count": 3, "failed_count": 0}
    )
    res = a.upsert_records(
        "c", [{"id": str(i), "vector": [float(i)]} for i in range(3)]
    )
    assert res.success == 3


def test_upsert_records_fallback_insert_vectors():
    a = make_adapter()
    a._client.insert_vectors = MagicMock(
        return_value={"success": True, "successful_count": 1, "failed_count": 0}
    )
    res = a.upsert_records("c", [{"id": "1", "vector": [1.0]}])
    assert res.success == 1
    assert a._client.insert_vectors.call_args.kwargs["upsert"] is True


def test_insert_vectors_alias_returns_vector_response():
    a = make_adapter()
    a._client.insert_records = MagicMock(
        return_value={"success": True, "successful_count": 1, "failed_count": 0}
    )
    resp = a.insert_vectors("c", [{"id": "1", "vector": [1.0]}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "INSERT"
    # _batch_to_vector_response passes BatchResult.success (a count) through
    assert resp.success == 1


def test_upsert_vectors_alias_returns_vector_response():
    a = make_adapter()
    a._client.upsert_records = MagicMock(
        return_value={"success": True, "successful_count": 1, "failed_count": 0}
    )
    resp = a.upsert_vectors("c", [{"id": "1", "vector": [1.0]}])
    assert resp.operation == "UPSERT"


def test_batch_to_vector_response_with_errors():
    br = BatchResult(
        total=2,
        success=1,
        failed=1,
        errors=["bad1", "bad2"],
        metrics=OperationMetrics(successful_count=1, failed_count=1),
    )
    resp = GrpcProtocolAdapter._batch_to_vector_response(br, "INSERT")
    assert resp.error_message == "bad1; bad2"
    assert resp.operation == "INSERT"


# ---------------------------------------------------------------------------
# get_vectors
# ---------------------------------------------------------------------------


def test_get_vectors_dict_and_obj_and_record():
    a = make_adapter()
    obj = SimpleNamespace(id="o", vector=[3.0], metadata={"k": "v"})
    existing = VectorRecord(id="e", vector=[9.0])
    a._client.get_vectors = MagicMock(
        return_value=[{"id": "d", "vector": [1.0]}, obj, existing, 12345]
    )
    out = a.get_vectors("c", ["d", "o", "e"])
    ids = {r.id for r in out}
    # int 12345 has no .id and is not dict/VectorRecord -> skipped
    assert ids == {"d", "o", "e"}


def test_get_vectors_fallback_no_method():
    a = make_adapter()
    # FakeGrpcClient has no get_vectors -> warning + empty list
    out = a.get_vectors("c", ["x"])
    assert out == []


def test_get_vectors_none_results():
    a = make_adapter()
    a._client.get_vectors = MagicMock(return_value=None)
    assert a.get_vectors("c", ["x"]) == []


# ---------------------------------------------------------------------------
# delete_vectors / update_vector_metadata
# ---------------------------------------------------------------------------


def test_delete_vectors_success():
    a = make_adapter()
    a._client.delete_vectors = MagicMock(
        return_value={"success": True, "successful_count": 2, "failed_count": 0}
    )
    resp = a.delete_vectors("c", ["a", "b"])
    assert resp.operation == "DELETE"
    assert resp.success is True


def test_delete_vectors_fallback_raises():
    a = make_adapter()
    # FakeGrpcClient lacks delete_vectors -> builds invalid response (no metrics)
    with pytest.raises(Exception):
        a.delete_vectors("c", ["a"])


def test_update_vector_metadata_success():
    a = make_adapter()
    a._client.update_vector_metadata = MagicMock(
        return_value={"success": True, "successful_count": 1, "failed_count": 0}
    )
    resp = a.update_vector_metadata("c", "v1", {"k": "v"})
    assert resp.operation == "UPDATE"
    assert resp.success is True


def test_update_vector_metadata_fallback_raises():
    a = make_adapter()
    with pytest.raises(Exception):
        a.update_vector_metadata("c", "v1", {"k": "v"})


# ---------------------------------------------------------------------------
# _to_vector_operation_response branches
# ---------------------------------------------------------------------------


def test_to_vop_passthrough():
    a = make_adapter()
    vop = VectorOperationResponse(
        success=True, operation="X", metrics=OperationMetrics()
    )
    assert a._to_vector_operation_response(vop, "Y", 1) is vop


def test_to_vop_dict():
    a = make_adapter()
    out = a._to_vector_operation_response(
        {
            "success": False,
            "successful_count": 5,
            "failed_count": 2,
            "error_message": "err",
        },
        "INSERT",
        7,
    )
    assert out.success is False
    assert out.metrics.successful_count == 5
    assert out.metrics.failed_count == 2
    assert out.error_message == "err"


def test_to_vop_object_with_metrics():
    a = make_adapter()
    metrics = SimpleNamespace(successful_count=4, failed_count=1, duration_ms=10)
    obj = SimpleNamespace(success=True, metrics=metrics, error_message=None)
    out = a._to_vector_operation_response(obj, "UPSERT", 5)
    assert out.metrics.successful_count == 4
    assert out.metrics.failed_count == 1


def test_to_vop_object_no_metrics_success():
    a = make_adapter()
    obj = SimpleNamespace(success=True, metrics=None)
    out = a._to_vector_operation_response(obj, "INSERT", 3)
    assert out.metrics.successful_count == 3
    assert out.metrics.failed_count == 0


def test_to_vop_object_no_metrics_failure():
    a = make_adapter()
    obj = SimpleNamespace(success=False, metrics=None, error_message="x")
    out = a._to_vector_operation_response(obj, "INSERT", 3)
    assert out.metrics.successful_count == 0
    assert out.metrics.failed_count == 3
    assert out.error_message == "x"


# ---------------------------------------------------------------------------
# search / batch_search
# ---------------------------------------------------------------------------


def test_search_dict_results_with_tolist():
    a = make_adapter()
    a._client.search_vectors = MagicMock(
        return_value=[
            {"id": "r1", "score": 0.9, "vector": [1.0], "metadata": {"m": 1}},
            {"vector_id": "r2", "distance": 0.5},
        ]
    )

    class FakeArray:
        def tolist(self):
            return [0.1, 0.2]

    out = a.search("c", FakeArray(), top_k=2, include_vectors=True)
    assert [r.id for r in out] == ["r1", "r2"]
    assert out[0].vector == [1.0]
    assert out[0].metadata == {"m": 1}


def test_search_plain_list_query():
    a = make_adapter()
    a._client.search_vectors = MagicMock(return_value=[])
    out = a.search("c", [0.1, 0.2, 0.3])
    assert out == []


def test_to_search_results_object_and_searchresult():
    a = make_adapter()
    obj = SimpleNamespace(id="o", score=0.7, vector=[1.0], metadata={"a": 1})
    existing = SearchResult(id="e", score=0.4)
    out = a._to_search_results(
        [obj, existing], include_vectors=True, include_metadata=True
    )
    assert out[0].id == "o"
    assert out[0].vector == [1.0]
    assert out[0].metadata == {"a": 1}
    assert out[1] is existing


def test_to_search_results_object_distance_fallback():
    a = make_adapter()
    obj = SimpleNamespace(id="d", distance=0.33, vector=None, metadata=None)
    out = a._to_search_results([obj], include_vectors=True, include_metadata=True)
    assert out[0].score == 0.33
    assert out[0].vector is None
    assert out[0].metadata == {}


def test_to_search_results_none():
    a = make_adapter()
    assert a._to_search_results(None, False, False) == []


def test_batch_search_single_query_wrapped():
    a = make_adapter()
    # Returns a flat list (single query) -> wrapped in one outer list
    a._client.search_vectors = MagicMock(return_value=[{"id": "r1", "score": 0.9}])
    out = a.batch_search("c", [[0.1, 0.2]])
    assert len(out) == 1
    assert out[0][0].id == "r1"


def test_batch_search_multiple_query_results():
    a = make_adapter()
    a._client.search_vectors = MagicMock(
        return_value=[
            [{"id": "a", "score": 0.9}],
            [{"id": "b", "score": 0.8}],
        ]
    )

    class FakeArray:
        def __init__(self, data):
            self._data = data

        def tolist(self):
            return self._data

    out = a.batch_search("c", [FakeArray([0.1]), FakeArray([0.2])])
    assert len(out) == 2
    assert out[0][0].id == "a"
    assert out[1][0].id == "b"


def test_batch_search_empty_results():
    a = make_adapter()
    a._client.search_vectors = MagicMock(return_value=[])
    out = a.batch_search("c", [[0.1]])
    assert out == []


# ---------------------------------------------------------------------------
# close
# ---------------------------------------------------------------------------


def test_close_calls_client_close():
    a = make_adapter()
    a.close()
    assert a._client.closed is True
    assert a.is_connected is False


def test_close_without_client_close():
    a = make_adapter()
    a._client = SimpleNamespace()  # no close()
    a.close()
    assert a.is_connected is False
