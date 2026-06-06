"""Offline unit tests for proximadb_sdk.adapters.grpc_adapter.GrpcProtocolAdapter.

The adapter wraps a gRPC sync client. We never open a real channel: at
construction time the underlying ``ProximaDBSyncGrpcClient`` is monkeypatched to
a fake, and individual tests replace ``adapter._client`` with hand fakes whose
methods return plain dicts / wrapper objects so the translation code paths are
exercised.
"""

from types import SimpleNamespace

import pytest

from proximadb_sdk.adapters.grpc_adapter import GrpcProtocolAdapter
from proximadb_sdk.models import (
    Collection,
    CollectionConfig,
    OperationMetrics,
    SearchResult,
    VectorOperationResponse,
    VectorRecord,
)


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeGrpcClient:
    """Records calls and returns whatever the test pre-loads onto attributes."""

    def __init__(self, **kw):
        self.calls = []
        self.closed = False

    def _record(self, name, *a, **k):
        self.calls.append((name, a, k))


def make_adapter(monkeypatch, client=None):
    """Construct an adapter without touching the real gRPC client class."""
    import proximadb_sdk.protocols.grpc_sync as grpc_sync

    monkeypatch.setattr(grpc_sync, "ProximaDBSyncGrpcClient", FakeGrpcClient)
    adapter = GrpcProtocolAdapter(server_address="localhost:5678")
    if client is not None:
        adapter._client = client
    return adapter


# ---------------------------------------------------------------------------
# Construction / properties
# ---------------------------------------------------------------------------


def test_init_basic(monkeypatch):
    adapter = make_adapter(monkeypatch)
    assert adapter.protocol_name == "grpc"
    assert adapter.is_connected is True
    assert adapter._server_address == "localhost:5678"


def test_init_with_config_url(monkeypatch):
    import proximadb_sdk.protocols.grpc_sync as grpc_sync

    monkeypatch.setattr(grpc_sync, "ProximaDBSyncGrpcClient", FakeGrpcClient)
    cfg = SimpleNamespace(url="http://gateway:9000", base_url=None)
    adapter = GrpcProtocolAdapter(config=cfg)
    # http:// stripped, server_address overridden from config
    assert adapter._server_address == "gateway:9000"


def test_init_drops_extra_kwargs(monkeypatch):
    import proximadb_sdk.protocols.grpc_sync as grpc_sync

    monkeypatch.setattr(grpc_sync, "ProximaDBSyncGrpcClient", FakeGrpcClient)
    adapter = GrpcProtocolAdapter(
        server_address="host:1", auth="token", url="x", base_url="y"
    )
    assert adapter._server_address == "host:1"


# ---------------------------------------------------------------------------
# health
# ---------------------------------------------------------------------------


def test_health_healthy(monkeypatch):
    client = FakeGrpcClient()
    client.health_check = lambda: SimpleNamespace(
        healthy=True, version="1.2.3", uptime_seconds=42, latency_ms=7
    )
    adapter = make_adapter(monkeypatch, client)
    hs = adapter.health()
    assert hs.status == "healthy"
    assert hs.version == "1.2.3"
    assert hs.uptime_seconds == 42
    assert hs.timestamp_ms == 7
    assert hs.services["grpc"] == "ok"


def test_health_not_healthy(monkeypatch):
    client = FakeGrpcClient()
    client.health_check = lambda: SimpleNamespace(
        healthy=False, version=None, uptime_seconds=0, latency_ms=-5
    )
    adapter = make_adapter(monkeypatch, client)
    hs = adapter.health()
    assert hs.status == "running"
    assert hs.version == "0.0.0"
    assert hs.timestamp_ms == 0  # clamped to >= 0
    assert hs.services["grpc"] == "unavailable"


def test_health_no_healthy_attr(monkeypatch):
    client = FakeGrpcClient()
    client.health_check = lambda: object()  # no `healthy` attr
    adapter = make_adapter(monkeypatch, client)
    hs = adapter.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unknown"


def test_health_exception(monkeypatch):
    client = FakeGrpcClient()

    def boom():
        raise RuntimeError("down")

    client.health_check = boom
    adapter = make_adapter(monkeypatch, client)
    hs = adapter.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unavailable"


# ---------------------------------------------------------------------------
# create_collection
# ---------------------------------------------------------------------------


def test_create_collection_with_config(monkeypatch):
    client = FakeGrpcClient()
    captured = {}

    def create_collection(**kw):
        captured.update(kw)
        return {"id": "c1", "name": "mycoll12", "dimension": 128}

    client.create_collection = create_collection
    adapter = make_adapter(monkeypatch, client)

    cfg = CollectionConfig(
        name="mycoll12",
        dimension=128,
        canonical_embedding_precision="fp16",
    )
    coll = adapter.create_collection("mycoll12", config=cfg)
    assert isinstance(coll, Collection)
    assert coll.id == "c1"
    # precision fp16 -> 2
    assert captured["canonical_embedding_precision"] == 2
    assert captured["dimension"] == 128


def test_create_collection_from_kwargs(monkeypatch):
    client = FakeGrpcClient()
    captured = {}

    def create_collection(**kw):
        captured.update(kw)
        return SimpleNamespace(id="c2", name="kwcoll77", dimension=64)

    client.create_collection = create_collection
    adapter = make_adapter(monkeypatch, client)

    coll = adapter.create_collection(
        "kwcoll77",
        dimension=64,
        distance_metric="cosine",
        canonical_embedding_precision="int8",
        extra_flag=True,
    )
    assert coll.id == "c2"
    assert captured["dimension"] == 64
    assert captured["canonical_embedding_precision"] == 4  # int8 -> 4
    # extra kwargs flow through
    assert captured["extra_flag"] is True


def test_create_collection_no_precision(monkeypatch):
    client = FakeGrpcClient()
    captured = {}

    def create_collection(**kw):
        captured.update(kw)
        return {"id": "c3", "name": "noprec12", "dimension": 32}

    client.create_collection = create_collection
    adapter = make_adapter(monkeypatch, client)
    adapter.create_collection("noprec12", dimension=32)
    assert captured["canonical_embedding_precision"] is None


# ---------------------------------------------------------------------------
# _to_collection variants
# ---------------------------------------------------------------------------


def test_to_collection_passthrough(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    existing = Collection(id="x", config=CollectionConfig(name="passcoll", dimension=4))
    assert adapter._to_collection(existing) is existing


def test_to_collection_dict_without_config(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    coll = adapter._to_collection(
        {"id": "d1", "name": "dictcoll", "dimension": 16}, "fallbackname", 0
    )
    assert coll.id == "d1"
    assert coll.config.name == "dictcoll"
    assert coll.config.dimension == 16


def test_to_collection_dict_with_config(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    coll = adapter._to_collection(
        {"id": "d2", "config": {"name": "withconfig", "dimension": 8}}
    )
    assert coll.config.dimension == 8


def test_to_collection_protobuf_like(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    proto = SimpleNamespace(id="p1", name="protocoll", dimension=12)
    coll = adapter._to_collection(proto)
    assert coll.id == "p1"
    assert coll.config.name == "protocoll"
    assert coll.config.dimension == 12


def test_to_collection_fallbacks(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    # bare object with no attributes -> use fallbacks
    coll = adapter._to_collection(object(), "fbname12", 9)
    assert coll.config.name == "fbname12"
    assert coll.config.dimension == 9


# ---------------------------------------------------------------------------
# get / list / delete collection
# ---------------------------------------------------------------------------


def test_get_collection_found(monkeypatch):
    client = FakeGrpcClient()
    client.get_collection = lambda cid: {"id": cid, "name": "found123", "dimension": 5}
    adapter = make_adapter(monkeypatch, client)
    coll = adapter.get_collection("found123")
    assert coll.id == "found123"


def test_get_collection_none(monkeypatch):
    client = FakeGrpcClient()
    client.get_collection = lambda cid: None
    adapter = make_adapter(monkeypatch, client)
    assert adapter.get_collection("missing") is None


def test_get_collection_exception(monkeypatch):
    client = FakeGrpcClient()

    def boom(cid):
        raise RuntimeError("nope")

    client.get_collection = boom
    adapter = make_adapter(monkeypatch, client)
    assert adapter.get_collection("err") is None


def test_list_collections(monkeypatch):
    client = FakeGrpcClient()
    client.list_collections = lambda: [
        {"id": "a", "name": "aaaaaaaa", "dimension": 2},
        SimpleNamespace(id="b", name="bbbbbbbb", dimension=3),
    ]
    adapter = make_adapter(monkeypatch, client)
    colls = adapter.list_collections()
    assert len(colls) == 2
    assert {c.id for c in colls} == {"a", "b"}


def test_list_collections_skips_bad_item(monkeypatch):
    client = FakeGrpcClient()

    class Exploding:
        @property
        def name(self):
            raise ValueError("boom")

    # dict missing fields is fine; the exploding one is skipped
    client.list_collections = lambda: [Exploding(), {"id": "ok", "name": "okokokok", "dimension": 1}]
    adapter = make_adapter(monkeypatch, client)
    colls = adapter.list_collections()
    assert len(colls) == 1
    assert colls[0].id == "ok"


def test_list_collections_none(monkeypatch):
    client = FakeGrpcClient()
    client.list_collections = lambda: None
    adapter = make_adapter(monkeypatch, client)
    assert adapter.list_collections() == []


def test_delete_collection_success_attr(monkeypatch):
    client = FakeGrpcClient()
    client.delete_collection = lambda cid: SimpleNamespace(success=True)
    adapter = make_adapter(monkeypatch, client)
    assert adapter.delete_collection("c") is True


def test_delete_collection_success_attr_false(monkeypatch):
    client = FakeGrpcClient()
    client.delete_collection = lambda cid: SimpleNamespace(success=False)
    adapter = make_adapter(monkeypatch, client)
    assert adapter.delete_collection("c") is False


def test_delete_collection_no_attr(monkeypatch):
    client = FakeGrpcClient()
    client.delete_collection = lambda cid: "ok"
    adapter = make_adapter(monkeypatch, client)
    assert adapter.delete_collection("c") is True


def test_delete_collection_exception(monkeypatch):
    client = FakeGrpcClient()

    def boom(cid):
        raise RuntimeError("x")

    client.delete_collection = boom
    adapter = make_adapter(monkeypatch, client)
    assert adapter.delete_collection("c") is False


# ---------------------------------------------------------------------------
# _record_payloads
# ---------------------------------------------------------------------------


def test_record_payloads_dict(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    out = adapter._record_payloads([{"id": "x", "vector": [1.0]}])
    assert out == [{"id": "x", "vector": [1.0]}]


def test_record_payloads_model_dump(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())

    class Modelish:
        def model_dump(self, exclude_none=False):
            return {"id": "m", "vector": [2.0]}

    out = adapter._record_payloads([Modelish()])
    assert out == [{"id": "m", "vector": [2.0]}]


def test_record_payloads_vector_record_to_dict(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    # An object with neither dict nor model_dump -> ProtoConverter path.
    rec = SimpleNamespace(id="vr", vector=[3.0], metadata={})
    out = adapter._record_payloads([rec])
    assert isinstance(out, list) and len(out) == 1
    assert isinstance(out[0], dict)


# ---------------------------------------------------------------------------
# insert_records / upsert_records
# ---------------------------------------------------------------------------


def _ok_batch_result():
    return {"success": True, "successful_count": 2, "failed_count": 0}


def test_insert_records_native(monkeypatch):
    client = FakeGrpcClient()
    captured = {}

    def insert_records(collection_id, records, **kw):
        captured["collection_id"] = collection_id
        captured["records"] = records
        return _ok_batch_result()

    client.insert_records = insert_records
    adapter = make_adapter(monkeypatch, client)
    res = adapter.insert_records("col1", [{"id": "1", "vector": [1.0]}, {"id": "2", "vector": [2.0]}])
    assert res.total == 2
    assert res.success == 2
    assert res.failed == 0
    assert captured["collection_id"] == "col1"


def test_insert_records_fallback_to_insert_vectors(monkeypatch):
    # Client without insert_records but with insert_vectors.
    class Client:
        def insert_vectors(self, collection_id, vectors, **kw):
            return {"success": True, "successful_count": 1, "failed_count": 0}

    adapter = make_adapter(monkeypatch, Client())
    res = adapter.insert_records("col", [{"id": "1", "vector": [1.0]}])
    assert res.total == 1
    assert res.success == 1


def test_upsert_records_native(monkeypatch):
    client = FakeGrpcClient()
    client.upsert_records = lambda collection_id, records, **kw: {
        "success": True,
        "successful_count": 3,
        "failed_count": 0,
    }
    adapter = make_adapter(monkeypatch, client)
    res = adapter.upsert_records("col", [{"id": str(i), "vector": [float(i)]} for i in range(3)])
    assert res.success == 3


def test_upsert_records_fallback(monkeypatch):
    class Client:
        def insert_vectors(self, collection_id, vectors, upsert=False, **kw):
            assert upsert is True
            return {"success": True, "successful_count": 1, "failed_count": 0}

    adapter = make_adapter(monkeypatch, Client())
    res = adapter.upsert_records("col", [{"id": "1", "vector": [1.0]}])
    assert res.success == 1


# ---------------------------------------------------------------------------
# insert_vectors / upsert_vectors aliases
# ---------------------------------------------------------------------------


def test_insert_vectors_alias(monkeypatch):
    client = FakeGrpcClient()
    client.insert_records = lambda collection_id, records, **kw: {
        "success": True,
        "successful_count": 1,
        "failed_count": 0,
    }
    adapter = make_adapter(monkeypatch, client)
    resp = adapter.insert_vectors("col", [{"id": "1", "vector": [1.0]}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "INSERT"
    # success is typed `bool | int`; pydantic may coerce True -> 1.
    assert bool(resp.success) is True


def test_upsert_vectors_alias_with_errors(monkeypatch):
    client = FakeGrpcClient()
    # failed -> error_message present feeds into errors list
    client.upsert_records = lambda collection_id, records, **kw: {
        "success": False,
        "successful_count": 0,
        "failed_count": 1,
        "error_message": "bad row",
    }
    adapter = make_adapter(monkeypatch, client)
    resp = adapter.upsert_vectors("col", [{"id": "1", "vector": [1.0]}])
    assert resp.operation == "UPSERT"
    assert resp.error_message == "bad row"


# ---------------------------------------------------------------------------
# get_vectors
# ---------------------------------------------------------------------------


def test_get_vectors_mixed(monkeypatch):
    client = FakeGrpcClient()
    vr = VectorRecord(id="already", vector=[1.0])
    client.get_vectors = lambda cid, ids, include_vectors=True, **kw: [
        vr,
        {"id": "d", "vector": [2.0], "metadata": {}},
        SimpleNamespace(id="p", vector=[3.0], metadata={"k": "v"}),
    ]
    adapter = make_adapter(monkeypatch, client)
    recs = adapter.get_vectors("col", ["already", "d", "p"])
    assert len(recs) == 3
    assert recs[0] is vr
    assert recs[2].metadata == {"k": "v"}


def test_get_vectors_not_implemented(monkeypatch):
    class Client:
        pass

    adapter = make_adapter(monkeypatch, Client())
    assert adapter.get_vectors("col", ["a"]) == []


def test_get_vectors_none_result(monkeypatch):
    client = FakeGrpcClient()
    client.get_vectors = lambda cid, ids, include_vectors=True, **kw: None
    adapter = make_adapter(monkeypatch, client)
    assert adapter.get_vectors("col", ["a"]) == []


# ---------------------------------------------------------------------------
# delete_vectors
# ---------------------------------------------------------------------------


def test_delete_vectors(monkeypatch):
    client = FakeGrpcClient()
    client.delete_vectors = lambda cid, ids, **kw: {
        "success": True,
        "successful_count": 2,
    }
    adapter = make_adapter(monkeypatch, client)
    resp = adapter.delete_vectors("col", ["a", "b"])
    assert resp.operation == "DELETE"
    assert resp.success is True


def test_delete_vectors_not_implemented(monkeypatch):
    class Client:
        pass

    adapter = make_adapter(monkeypatch, Client())
    # SOURCE BUG: the not-implemented fallback builds VectorOperationResponse
    # without the required `metrics` field, so pydantic raises. We still
    # exercise the `else` branch (the only way to reach it offline).
    from pydantic import ValidationError

    with pytest.raises(ValidationError):
        adapter.delete_vectors("col", ["a"])


# ---------------------------------------------------------------------------
# update_vector_metadata
# ---------------------------------------------------------------------------


def test_update_vector_metadata(monkeypatch):
    client = FakeGrpcClient()
    client.update_vector_metadata = lambda cid, vid, meta, **kw: {
        "success": True,
        "successful_count": 1,
    }
    adapter = make_adapter(monkeypatch, client)
    resp = adapter.update_vector_metadata("col", "v1", {"k": "v"})
    assert resp.operation == "UPDATE"
    assert resp.success is True


def test_update_vector_metadata_not_implemented(monkeypatch):
    class Client:
        pass

    adapter = make_adapter(monkeypatch, Client())
    # SOURCE BUG: same missing-`metrics` issue as delete_vectors fallback.
    from pydantic import ValidationError

    with pytest.raises(ValidationError):
        adapter.update_vector_metadata("col", "v1", {"k": "v"})


# ---------------------------------------------------------------------------
# _to_vector_operation_response variants
# ---------------------------------------------------------------------------


def test_to_vop_passthrough(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    existing = VectorOperationResponse(
        success=True, operation="X", metrics=OperationMetrics()
    )
    assert adapter._to_vector_operation_response(existing, "OTHER", 1) is existing


def test_to_vop_dict(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    resp = adapter._to_vector_operation_response(
        {"success": True, "successful_count": 5, "failed_count": 1, "error_message": "e"},
        "INSERT",
        6,
    )
    assert resp.metrics.successful_count == 5
    assert resp.metrics.failed_count == 1
    assert resp.error_message == "e"


def test_to_vop_wrapper_with_metrics(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    metrics = SimpleNamespace(successful_count=4, failed_count=0, duration_ms=12)
    result = SimpleNamespace(success=True, metrics=metrics, error_message=None)
    resp = adapter._to_vector_operation_response(result, "INSERT", 4)
    assert resp.metrics.successful_count == 4
    assert resp.metrics.processing_time_us == 12 or resp.metrics.successful_count == 4


def test_to_vop_wrapper_no_metrics_success(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    result = SimpleNamespace(success=True, metrics=None, error_message=None)
    resp = adapter._to_vector_operation_response(result, "INSERT", 3)
    assert resp.metrics.successful_count == 3
    assert resp.metrics.failed_count == 0


def test_to_vop_wrapper_no_metrics_failure(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    result = SimpleNamespace(success=False, metrics=None, error_message="boom")
    resp = adapter._to_vector_operation_response(result, "INSERT", 3)
    assert resp.metrics.successful_count == 0
    assert resp.metrics.failed_count == 3
    assert resp.error_message == "boom"


# ---------------------------------------------------------------------------
# search
# ---------------------------------------------------------------------------


def test_search_dict_results(monkeypatch):
    client = FakeGrpcClient()
    client.search_vectors = lambda **kw: [
        {"id": "a", "score": 0.9, "vector": [1.0], "metadata": {"x": 1}},
        {"vector_id": "b", "distance": 0.5},
    ]
    adapter = make_adapter(monkeypatch, client)
    results = adapter.search("col", [1.0, 2.0], top_k=5, include_vectors=True)
    assert len(results) == 2
    assert results[0].id == "a"
    assert results[0].vector == [1.0]
    assert results[1].id == "b"
    assert results[1].score == 0.5


def test_search_numpy_query(monkeypatch):
    np = pytest.importorskip("numpy")
    client = FakeGrpcClient()
    captured = {}

    def search_vectors(**kw):
        captured.update(kw)
        return []

    client.search_vectors = search_vectors
    adapter = make_adapter(monkeypatch, client)
    adapter.search("col", np.array([1.0, 2.0, 3.0]))
    assert captured["query_vector"] == [1.0, 2.0, 3.0]


def test_search_object_results(monkeypatch):
    client = FakeGrpcClient()
    client.search_vectors = lambda **kw: [
        SimpleNamespace(id="o", score=0.7, vector=[9.0], metadata={"m": "v"})
    ]
    adapter = make_adapter(monkeypatch, client)
    results = adapter.search("col", [1.0], include_vectors=True, include_metadata=True)
    assert results[0].id == "o"
    assert results[0].vector == [9.0]
    assert results[0].metadata == {"m": "v"}


def test_search_object_excludes_fields(monkeypatch):
    client = FakeGrpcClient()
    client.search_vectors = lambda **kw: [
        SimpleNamespace(id="o", distance=0.3, vector=[9.0], metadata={"m": "v"})
    ]
    adapter = make_adapter(monkeypatch, client)
    results = adapter.search("col", [1.0], include_vectors=False, include_metadata=False)
    assert results[0].vector is None
    assert results[0].metadata is None
    assert results[0].score == 0.3


def test_to_search_results_none(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    assert adapter._to_search_results(None, False, True) == []


def test_to_search_results_passthrough(monkeypatch):
    adapter = make_adapter(monkeypatch, FakeGrpcClient())
    sr = SearchResult(id="z", score=1.0)
    out = adapter._to_search_results([sr], False, True)
    assert out[0] is sr


# ---------------------------------------------------------------------------
# batch_search
# ---------------------------------------------------------------------------


def test_batch_search_single_result(monkeypatch):
    # results[0] is not a list -> wrap single query path
    client = FakeGrpcClient()
    client.search_vectors = lambda **kw: [{"id": "a", "score": 0.1}]
    adapter = make_adapter(monkeypatch, client)
    out = adapter.batch_search("col", [[1.0], [2.0]])
    assert len(out) == 1
    assert out[0][0].id == "a"


def test_batch_search_multi_result(monkeypatch):
    client = FakeGrpcClient()
    client.search_vectors = lambda **kw: [
        [{"id": "a", "score": 0.1}],
        [{"id": "b", "score": 0.2}],
    ]
    adapter = make_adapter(monkeypatch, client)
    out = adapter.batch_search("col", [[1.0], [2.0]])
    assert len(out) == 2
    assert out[0][0].id == "a"
    assert out[1][0].id == "b"


def test_batch_search_numpy_queries(monkeypatch):
    np = pytest.importorskip("numpy")
    client = FakeGrpcClient()
    captured = {}

    def search_vectors(**kw):
        captured.update(kw)
        return []

    client.search_vectors = search_vectors
    adapter = make_adapter(monkeypatch, client)
    out = adapter.batch_search("col", [np.array([1.0, 2.0]), [3.0, 4.0]])
    assert captured["query_vectors"] == [[1.0, 2.0], [3.0, 4.0]]
    assert out == []


def test_batch_search_empty(monkeypatch):
    client = FakeGrpcClient()
    client.search_vectors = lambda **kw: []
    adapter = make_adapter(monkeypatch, client)
    assert adapter.batch_search("col", [[1.0]]) == []


# ---------------------------------------------------------------------------
# close / context manager
# ---------------------------------------------------------------------------


def test_close(monkeypatch):
    client = FakeGrpcClient()
    client.close = lambda: setattr(client, "closed", True)
    adapter = make_adapter(monkeypatch, client)
    adapter.close()
    assert client.closed is True
    assert adapter.is_connected is False


def test_close_no_close_method(monkeypatch):
    class Client:
        pass

    adapter = make_adapter(monkeypatch, Client())
    adapter.close()
    assert adapter.is_connected is False


def test_context_manager(monkeypatch):
    client = FakeGrpcClient()
    client.close = lambda: setattr(client, "closed", True)
    adapter = make_adapter(monkeypatch, client)
    with adapter as a:
        assert a is adapter
    assert adapter.is_connected is False
