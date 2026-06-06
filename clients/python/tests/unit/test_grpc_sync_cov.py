"""Offline unit tests for proximadb_sdk.protocols.grpc_sync.

Every transport is mocked: the connection pool is replaced with a fake that
hands out a dummy channel, and each *Stub class is monkeypatched so RPCs return
real *_pb2 response messages. No real channel, socket, or server is opened.
"""

import pytest

import proximadb_sdk.protocols.grpc_sync as gs
from proximadb_sdk.exceptions import ProximaDBError

import grpc
from proximadb_sdk.v1 import collection_types_pb2 as ct
from proximadb_sdk.v1 import vector_types_pb2 as vt
from proximadb_sdk.v1 import types_pb2 as t
from proximadb_sdk.v1 import graph_pb2 as gpb
from proximadb.v2 import record_pb2 as r2


# --------------------------------------------------------------------------
# Fakes
# --------------------------------------------------------------------------
class FakeChannel:
    """A stand-in for a gRPC channel; stubs ignore it entirely."""


class FakePool:
    def __init__(self):
        self.returned = []
        self.closed = False

    def get_channel(self):
        return FakeChannel()

    def return_channel(self, channel, success=True):
        self.returned.append(success)

    def get_metrics(self):
        return {"pool_size": 5, "active": 1}

    def close(self):
        self.closed = True


class FakeRpcError(grpc.RpcError):
    def __init__(self, code, details):
        self._code = code
        self._details = details

    def code(self):
        return self._code

    def details(self):
        return self._details


def make_stub(method_name, fn):
    """Build a stub class whose ctor takes a channel and exposes method_name=fn."""

    class _Stub:
        def __init__(self, channel):
            self.channel = channel

    setattr(_Stub, method_name, staticmethod(lambda *a, **k: fn(*a, **k)))
    return _Stub


def _patch_vector_stub_noop(monkeypatch):
    """_execute_with_pool builds a VectorServiceStub(channel) and passes that
    stub into the op as its 'channel' arg. Several ops (the *_v1 helpers,
    shortest_path) then construct their own service stub from it. Patch
    VectorServiceStub to a harmless object so no real channel introspection
    (unary_unary) happens against the FakeChannel."""

    class _Noop:
        def __init__(self, channel):
            pass

    monkeypatch.setattr(gs.v1_vector_pb2_grpc, "VectorServiceStub", _Noop)


@pytest.fixture
def client(monkeypatch):
    """Construct the gRPC client with the connection pool fully faked out."""
    fake_pool = FakePool()
    monkeypatch.setattr(gs, "GrpcConnectionPool", lambda **kw: fake_pool)
    c = gs.ProximaDBSyncGrpcClient("localhost:5678", timeout=1.0)
    assert c._connection_pool is fake_pool
    return c


# --------------------------------------------------------------------------
# Pure wrapper classes
# --------------------------------------------------------------------------
def test_collection_wrapper():
    coll = ct.Collection(id="abc", config=ct.CollectionConfig(name="docs", dimension=128))
    w = gs.CollectionWrapper(coll)
    assert w.name == "docs"
    assert w.dimension == 128
    assert w.id == "abc"
    assert w.config is not None
    assert w.stats is not None  # proto default message, not None
    assert "docs" in repr(w)
    # pass-through getattr
    assert w.created_at == coll.created_at


def test_collection_wrapper_missing_config():
    class Bare:
        pass

    w = gs.CollectionWrapper(Bare())
    assert w.name is None
    assert w.dimension is None
    assert w.id is None


def test_search_results_wrapper():
    w = gs.SearchResultsWrapper([1, 2, 3])
    assert len(w) == 3
    assert list(w) == [1, 2, 3]
    assert w[1] == 2
    assert w.results == [1, 2, 3]
    assert "count=3" in repr(w)


def test_vector_wrapper():
    w = gs.VectorWrapper({"id": "v1", "vector": [0.1]})
    assert w.id == "v1"
    assert w["vector"] == [0.1]
    assert w.get("missing", 7) == 7
    assert w.get("id") == "v1"
    assert "v1" in repr(w)


def test_dict_wrapper():
    w = gs.DictWrapper({"status": "deleted", "success": True})
    assert w.status == "deleted"
    assert w["success"] is True
    assert w.get("nope", "d") == "d"
    assert "deleted" in repr(w)


def test_dataclasses():
    h = gs.HealthCheckResponse(True, 1.0, "ok", "addr")
    assert h.healthy and h.version is None
    d = gs.DeleteCollectionResponse(True, "cid")
    assert d.status == "deleted"


# --------------------------------------------------------------------------
# Construction / pool lifecycle
# --------------------------------------------------------------------------
def test_init_compression(monkeypatch):
    captured = {}

    def fake_pool_ctor(**kw):
        captured.update(kw)
        return FakePool()

    monkeypatch.setattr(gs, "GrpcConnectionPool", fake_pool_ctor)
    c = gs.ProximaDBSyncGrpcClient(
        "localhost:5678", enable_compression=True, compression_algorithm="DEFLATE"
    )
    assert captured["compression"] == grpc.Compression.Deflate
    assert c._pool is c._connection_pool


def test_init_unknown_compression(monkeypatch):
    captured = {}
    monkeypatch.setattr(
        gs, "GrpcConnectionPool", lambda **kw: captured.update(kw) or FakePool()
    )
    gs.ProximaDBSyncGrpcClient(
        "localhost:5678", enable_compression=True, compression_algorithm="bogus"
    )
    assert captured["compression"] == grpc.Compression.Gzip


def test_init_pool_failure(monkeypatch):
    def boom(**kw):
        raise RuntimeError("no channels")

    monkeypatch.setattr(gs, "GrpcConnectionPool", boom)
    with pytest.raises(ProximaDBError, match="connection pool initialization failed"):
        gs.ProximaDBSyncGrpcClient("localhost:5678")


def test_pool_metrics_and_close(client):
    assert client.get_pool_metrics() == {"pool_size": 5, "active": 1}
    with client as c:
        assert c is client
    assert client._connection_pool.closed is True


def test_close_handles_error(client):
    def boom():
        raise RuntimeError("x")

    client._connection_pool.close = boom
    client.close()  # swallowed, no raise


def test_get_pool_metrics_none(client):
    client._connection_pool = None
    assert client.get_pool_metrics() is None


# --------------------------------------------------------------------------
# SqlValue / TypedValue conversions
# --------------------------------------------------------------------------
def test_python_to_sql_value_roundtrip(client):
    payload = {
        "s": "hello",
        "i": 42,
        "f": 3.5,
        "b": True,
        "n": None,
        "by": b"raw",
        "arr": [1, "two", [3]],
        "obj": {"k": "v"},
        "other": (1, 2),
    }
    sv = client._python_to_sql_value(payload)
    out = client._sql_value_to_python(sv)
    assert out["s"] == "hello"
    assert out["i"] == 42
    assert out["f"] == 3.5
    assert out["b"] is True
    assert out["n"] is None
    assert out["by"] == b"raw"
    assert out["arr"][1] == "two"
    assert out["obj"] == {"k": "v"}


def test_python_to_sql_value_fallback(client):
    class Custom:
        def __str__(self):
            return "custom-str"

    sv = client._python_to_sql_value(Custom())
    assert client._sql_value_to_python(sv) == "custom-str"


def test_sql_value_to_python_empty(client):
    assert client._sql_value_to_python(t.SqlValue()) is None


def test_python_to_v2_typed_value_variants(client):
    assert client._v2_typed_value_to_python(client._python_to_v2_typed_value(None)) is None
    assert client._v2_typed_value_to_python(client._python_to_v2_typed_value(True)) is True
    assert client._v2_typed_value_to_python(client._python_to_v2_typed_value(7)) == 7
    assert client._v2_typed_value_to_python(client._python_to_v2_typed_value(1.25)) == 1.25
    assert client._v2_typed_value_to_python(client._python_to_v2_typed_value(b"x")) == b"x"
    assert client._v2_typed_value_to_python(client._python_to_v2_typed_value("str")) == "str"
    arr = client._python_to_v2_typed_value([1, 2, 3])
    assert client._v2_typed_value_to_python(arr) == [1, 2, 3]
    obj = client._python_to_v2_typed_value({"a": 1})
    assert client._v2_typed_value_to_python(obj) == {"a": 1}


def test_python_to_v2_typed_value_type_hints(client):
    f32 = client._python_to_v2_typed_value({"type": "float32", "value": 1.5})
    assert abs(client._v2_typed_value_to_python(f32) - 1.5) < 1e-3
    sym = client._python_to_v2_typed_value({"type": "symbol", "value": "SYM"})
    assert client._v2_typed_value_to_python(sym) == "SYM"


def test_python_to_v2_typed_value_object_fallback(client):
    class Custom:
        def __str__(self):
            return "obj"

    tv = client._python_to_v2_typed_value(Custom())
    assert client._v2_typed_value_to_python(tv) == "obj"


# --------------------------------------------------------------------------
# Record normalization helpers
# --------------------------------------------------------------------------
def test_normalize_vector_alias_records(client):
    class Pyd:
        def model_dump(self, exclude_none=False):
            return {"id": "p1", "vector": [0.1], "metadata": {"k": "v"}}

    class Obj:
        def __init__(self):
            self.id = "o1"
            self.vector = [0.2]
            self._private = "hide"

    recs = client._normalize_vector_alias_records(
        [
            {"id": "d1", "vector": [0.3], "props": {"a": 1}, "version": 2},
            Pyd(),
            Obj(),
            [0.9, 0.8],  # bare list -> wrapped
        ]
    )
    assert recs[0]["id"] == "d1"
    assert recs[0]["props"] == {"a": 1}
    assert recs[0]["version"] == 2
    assert recs[1]["id"] == "p1"
    assert recs[1]["props"] == {"k": "v"}
    assert recs[2]["id"] == "o1"
    assert recs[3]["id"] == "record_3"
    assert recs[3]["vector"] == [0.9, 0.8]


def test_record_proto_for_grpc(client):
    proto = client._record_proto_for_grpc(
        {
            "id": "rec1",
            "vector": [0.1, 0.2],
            "vector_dimension": 2,
            "props": {"a": "x"},
            "typed_fields": {"score": {"value": 5, "value_type": "integer"}},
            "timestamp_ms": 1000,
            "version": 3,
            "partition_values": {"p": "v"},
            "custom_metadata": {"m": "n"},
        }
    )
    assert proto.id == "rec1"
    assert list(proto.vector) == pytest.approx([0.1, 0.2])
    assert proto.vector_dimension == 2
    assert proto.timestamp_ms == 1000
    assert proto.version == 3


def test_record_proto_for_grpc_from_embeddings(client):
    proto = client._record_proto_for_grpc(
        {"id": "e1", "embeddings": [{"values": [1.0, 2.0]}]}
    )
    assert list(proto.vector) == pytest.approx([1.0, 2.0])


def test_record_proto_for_grpc_missing_vector(client):
    with pytest.raises(ValueError, match="missing vector"):
        client._record_proto_for_grpc({"id": "x"})


def test_record_proto_for_grpc_bad_type(client):
    with pytest.raises(TypeError):
        client._record_proto_for_grpc(12345)


# --------------------------------------------------------------------------
# Collection RPC wrappers
# --------------------------------------------------------------------------
def _patch_collection_stub(monkeypatch, method, fn):
    monkeypatch.setattr(
        gs.v1_collection_pb2_grpc, "CollectionServiceStub", make_stub(method, fn)
    )


def test_create_collection(client, monkeypatch):
    def create(config, timeout=None):
        assert config.name == "docs"
        return ct.Collection(id="c1", config=ct.CollectionConfig(name="docs", dimension=64))

    _patch_collection_stub(monkeypatch, "CreateCollection", create)
    w = client.create_collection(
        "docs", 64, distance_metric=1, indexing_algorithm=1, storage_engine=2
    )
    assert w.name == "docs"
    assert w.dimension == 64


def test_create_collection_engine_alias_and_str(client, monkeypatch):
    def create(config, timeout=None):
        assert config.storage_engine != 0
        return ct.Collection(id="c", config=ct.CollectionConfig(name="n", dimension=8))

    _patch_collection_stub(monkeypatch, "CreateCollection", create)
    w = client.create_collection("n", 8, engine="viper")
    assert w.name == "n"


def test_create_collection_unknown_engine(client):
    with pytest.raises(ValueError, match="Unknown storage engine"):
        client.create_collection("n", 8, storage_engine="nonsense")


def test_create_collection_with_index_and_quant(client, monkeypatch):
    def create(config, timeout=None):
        return ct.Collection(id="c", config=ct.CollectionConfig(name="n", dimension=8))

    _patch_collection_stub(monkeypatch, "CreateCollection", create)
    ic = ct.IndexConfig(index_name="i1")
    w = client.create_collection(
        "n", 8, index_configs=[ic],
        canonical_embedding_precision=2,
    )
    assert w.name == "n"


def test_get_collection(client, monkeypatch):
    _patch_collection_stub(
        monkeypatch,
        "GetCollection",
        lambda req, timeout=None: ct.Collection(
            id=req.collection_id, config=ct.CollectionConfig(name="g", dimension=16)
        ),
    )
    w = client.get_collection("cid")
    assert w.id == "cid"
    assert w.dimension == 16


def test_list_collections(client, monkeypatch):
    resp = ct.ListCollectionsResponse(
        collections=[
            ct.Collection(id="a", config=ct.CollectionConfig(name="a", dimension=1)),
            ct.Collection(id="b", config=ct.CollectionConfig(name="b", dimension=2)),
        ]
    )
    _patch_collection_stub(monkeypatch, "ListCollections", lambda req, timeout=None: resp)
    out = client.list_collections()
    assert [c.name for c in out] == ["a", "b"]


def test_delete_collection(client, monkeypatch):
    _patch_collection_stub(
        monkeypatch,
        "DeleteCollection",
        lambda req, timeout=None: ct.DeleteCollectionResponse(success=True),
    )
    out = client.delete_collection("cid")
    assert out.success is True
    assert out.collection_id == "cid"


def test_collection_v1_helpers(client, monkeypatch):
    calls = {}

    class _Stub:
        def __init__(self, channel):
            pass

        def CreateCollection(self, cfg, timeout=None):
            calls["create"] = cfg.name
            return ct.Collection(id="x")

        def GetCollection(self, req, timeout=None):
            calls["get"] = req.collection_id
            return ct.Collection(id=req.collection_id)

        def ListCollections(self, req, timeout=None):
            calls["list"] = (req.limit, req.offset, req.include_stats)
            return ct.ListCollectionsResponse()

        def DeleteCollection(self, req, timeout=None):
            calls["delete"] = req.collection_id
            return ct.DeleteCollectionResponse(success=True)

    monkeypatch.setattr(gs.v1_collection_pb2_grpc, "CollectionServiceStub", _Stub)
    # The *_v1 helpers route through _execute_with_pool, which first builds a
    # VectorServiceStub(channel) and passes that as the op's "channel"; make it
    # a harmless no-op so the inner CollectionServiceStub(...) accepts it.
    _patch_vector_stub_noop(monkeypatch)
    client.create_collection_v1("n", 4, 1, 2, tags=["t"], description="d")
    client.get_collection_v1("cid")
    client.list_collections_v1(limit=5, offset=2, include_stats=True)
    client.delete_collection_v1("cid")
    assert calls["create"] == "n"
    assert calls["get"] == "cid"
    assert calls["list"] == (5, 2, True)
    assert calls["delete"] == "cid"


# --------------------------------------------------------------------------
# health_check
# --------------------------------------------------------------------------
def test_health_check_ok(client, monkeypatch):
    _patch_collection_stub(
        monkeypatch,
        "ListCollections",
        lambda req, timeout=None: ct.ListCollectionsResponse(),
    )
    h = client.health_check()
    assert h.healthy is True
    assert h.status == "connected"
    assert h.latency_ms >= 0


def test_health_check_rpc_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise FakeRpcError(grpc.StatusCode.UNAVAILABLE, "down")

    _patch_collection_stub(monkeypatch, "ListCollections", boom)
    h = client.health_check()
    assert h.healthy is False
    assert "error" in h.status
    assert h.details == "down"


def test_health_check_generic_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise RuntimeError("oops")

    _patch_collection_stub(monkeypatch, "ListCollections", boom)
    h = client.health_check()
    assert h.healthy is False
    assert "RuntimeError" in h.status


# --------------------------------------------------------------------------
# Error mapping in pool executors
# --------------------------------------------------------------------------
def test_collection_rpc_unavailable_maps_to_connection_failed(client, monkeypatch):
    def boom(req, timeout=None):
        raise FakeRpcError(grpc.StatusCode.UNAVAILABLE, "no route")

    _patch_collection_stub(monkeypatch, "GetCollection", boom)
    with pytest.raises(ProximaDBError, match="connection failed"):
        client.get_collection("cid")


def test_collection_rpc_other_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise FakeRpcError(grpc.StatusCode.INTERNAL, "boom internal")

    _patch_collection_stub(monkeypatch, "GetCollection", boom)
    with pytest.raises(ProximaDBError, match="RPC failed"):
        client.get_collection("cid")


def test_collection_generic_exception(client, monkeypatch):
    def boom(req, timeout=None):
        raise ValueError("bad")

    _patch_collection_stub(monkeypatch, "GetCollection", boom)
    with pytest.raises(ProximaDBError, match="failed"):
        client.get_collection("cid")


def test_grpc_unavailable_via_message(client, monkeypatch):
    # code is not UNAVAILABLE but details mention "connect"
    def boom(req, timeout=None):
        raise FakeRpcError(grpc.StatusCode.INTERNAL, "cannot connect to peer")

    _patch_collection_stub(monkeypatch, "GetCollection", boom)
    with pytest.raises(ProximaDBError, match="connection failed"):
        client.get_collection("cid")


# --------------------------------------------------------------------------
# execute_sql
# --------------------------------------------------------------------------
def test_execute_sql(client, monkeypatch):
    def execute(req, timeout=None):
        assert req.query == "SELECT 1"
        assert req.collection == "docs"
        assert len(req.parameters) == 1
        resp = t.ExecuteSqlResponse(
            rows_scanned=10, rows_returned=1, execution_time_ms=5,
            columns=["a"], column_types=["int"],
        )
        row = resp.rows.add()
        fld = row.fields.add()
        fld.key = "a"
        fld.value.int64_value = 99
        return resp

    monkeypatch.setattr(gs.v1_sql_pb2_grpc, "SqlServiceStub", make_stub("ExecuteSql", execute))
    out = client.execute_sql("SELECT 1", parameters=[7], collection="docs")
    assert out["row_count"] == 1
    assert out["rows"][0]["a"] == 99
    assert out["rows_scanned"] == 10
    assert out["columns"] == ["a"]


def test_execute_sql_rpc_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise FakeRpcError(grpc.StatusCode.INTERNAL, "sql bad")

    monkeypatch.setattr(gs.v1_sql_pb2_grpc, "SqlServiceStub", make_stub("ExecuteSql", boom))
    with pytest.raises(ProximaDBError, match="execute_sql RPC failed"):
        client.execute_sql("SELECT 1")


def test_execute_sql_generic_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise RuntimeError("kaboom")

    monkeypatch.setattr(gs.v1_sql_pb2_grpc, "SqlServiceStub", make_stub("ExecuteSql", boom))
    with pytest.raises(ProximaDBError, match="execute_sql failed"):
        client.execute_sql("SELECT 1")


# --------------------------------------------------------------------------
# v2 record / vector operations
# --------------------------------------------------------------------------
def _patch_record_stub(monkeypatch, method, fn):
    monkeypatch.setattr(
        gs.v2_record_pb2_grpc, "ProximaRecordServiceStub", make_stub(method, fn)
    )


def _batch_response(success=2, failed=0, errors=None):
    resp = r2.ProximaRecordBatchResponse(
        success=failed == 0,
        total_processed=success + failed,
        success_count=success,
        failed_count=failed,
        processing_time_us=123,
    )
    for e in errors or []:
        be = resp.errors.add()
        be.record_id = e[0]
        be.error_message = e[1]
    return resp


def test_insert_records(client, monkeypatch):
    def insert(req, timeout=None):
        assert req.collection_id == "docs"
        assert req.write_mode == r2.INSERT
        assert len(req.records) == 2
        return _batch_response(success=2)

    _patch_record_stub(monkeypatch, "InsertRecords", insert)
    res = client.insert_records(
        "docs",
        [{"id": "a", "vector": [0.1]}, {"id": "b", "vector": [0.2]}],
        schema_id="s1",
    )
    assert res.total == 2
    assert res.success == 2
    assert res.failed == 0


def test_insert_records_with_errors(client, monkeypatch):
    _patch_record_stub(
        monkeypatch,
        "InsertRecords",
        lambda req, timeout=None: _batch_response(
            success=1, failed=1, errors=[("a", "dup")]
        ),
    )
    res = client.insert_records("docs", [{"id": "a", "vector": [0.1]}])
    assert res.failed == 1
    assert res.errors == ["a: dup"]


def test_insert_records_upsert_delegates(client, monkeypatch):
    def upsert(req, timeout=None):
        assert req.write_mode == r2.UPSERT
        return _batch_response(success=1)

    _patch_record_stub(monkeypatch, "UpsertRecords", upsert)
    res = client.insert_records("docs", [{"id": "a", "vector": [0.1]}], upsert=True)
    assert res.success == 1


def test_upsert_records(client, monkeypatch):
    _patch_record_stub(
        monkeypatch,
        "UpsertRecords",
        lambda req, timeout=None: _batch_response(success=1),
    )
    res = client.upsert_records("docs", [{"id": "a", "vector": [0.1]}], schema_id="s")
    assert res.success == 1


def test_insert_vectors(client, monkeypatch):
    _patch_record_stub(
        monkeypatch,
        "InsertRecords",
        lambda req, timeout=None: _batch_response(success=1),
    )
    out = client.insert_vectors(
        "docs", [{"id": "v1", "vector": [0.1], "metadata": {"k": "v"}}]
    )
    assert out.success is True
    assert out.operation == "INSERT"
    assert out.vector_ids == ["v1"]


def test_insert_vectors_upsert(client, monkeypatch):
    _patch_record_stub(
        monkeypatch,
        "UpsertRecords",
        lambda req, timeout=None: _batch_response(success=1, failed=1, errors=[("v1", "e")]),
    )
    out = client.insert_vectors("docs", [{"id": "v1", "vector": [0.1]}], upsert=True)
    assert out.operation == "UPSERT"
    assert out.success is False
    assert out.error_message == "v1: e"


def test_insert_vector_single(client, monkeypatch):
    _patch_record_stub(
        monkeypatch, "InsertRecords", lambda req, timeout=None: _batch_response(success=1)
    )
    out = client.insert_vector("docs", "v1", [0.1], metadata={"k": 1})
    assert out.vector_ids == ["v1"]


def test_update_vector(client, monkeypatch):
    _patch_record_stub(
        monkeypatch, "UpsertRecords", lambda req, timeout=None: _batch_response(success=1)
    )
    out = client.update_vector("docs", "v1", vector=[0.1], metadata={"k": 1})
    assert out["status"] == "updated"
    assert out["success"] is True


def test_delete_vector(client, monkeypatch):
    def delete(req, timeout=None):
        assert req.write_mode == r2.DELETE
        assert req.records[0].id == "v1"
        return _batch_response(success=1, failed=0)

    _patch_record_stub(monkeypatch, "DeleteRecords", delete)
    out = client.delete_vector("docs", "v1")
    assert out["status"] == "deleted"
    assert out.success is True


def test_delete_vector_failed(client, monkeypatch):
    _patch_record_stub(
        monkeypatch,
        "DeleteRecords",
        lambda req, timeout=None: _batch_response(success=0, failed=1),
    )
    out = client.delete_vector("docs", "v1")
    assert out["status"] == "failed"


def test_delete_vectors(client, monkeypatch):
    def delete(req, timeout=None):
        assert len(req.records) == 2
        return _batch_response(success=2)

    _patch_record_stub(monkeypatch, "DeleteRecords", delete)
    out = client.delete_vectors("docs", ["a", "b"])
    assert out["deleted_count"] == 2
    assert out["total_requested"] == 2


def test_record_rpc_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise FakeRpcError(grpc.StatusCode.UNAVAILABLE, "down")

    _patch_record_stub(monkeypatch, "InsertRecords", boom)
    with pytest.raises(ProximaDBError, match="connection failed"):
        client.insert_records("docs", [{"id": "a", "vector": [0.1]}])


def test_record_generic_error(client, monkeypatch):
    def boom(req, timeout=None):
        raise RuntimeError("x")

    _patch_record_stub(monkeypatch, "InsertRecords", boom)
    with pytest.raises(ProximaDBError, match="failed"):
        client.insert_records("docs", [{"id": "a", "vector": [0.1]}])


# --------------------------------------------------------------------------
# search
# --------------------------------------------------------------------------
def _search_response():
    resp = r2.TypedSearchResponse(total_found=1)
    item = resp.results.add()
    item.id = "v1"
    item.score = 0.9
    item.vector.extend([0.1, 0.2])
    item.timestamp_ms = 1000
    item.version = 2
    item.source = "ingest"
    item.props["k"].text_value = "hello"
    item.props["k"].declared_type = r2.TEXT
    return resp


def test_search_vectors(client, monkeypatch):
    def search(req, timeout=None):
        assert req.collection_id == "docs"
        assert req.top_k == 5
        return _search_response()

    _patch_record_stub(monkeypatch, "Search", search)
    out = client.search_vectors(
        "docs",
        query_vector=[0.1, 0.2],
        top_k=5,
        metadata_filters={"k": "hello"},
        include_vectors=True,
        search_hints={"mode": "fast"},
    )
    assert len(out) == 1
    res = out[0]
    assert res.id == "v1"
    assert res.score == pytest.approx(0.9)
    assert res.vector == pytest.approx([0.1, 0.2])
    assert res.metadata == {"k": "hello"}
    assert res.timestamp == 1000
    assert res.version == 2


def test_search_vectors_no_metadata(client, monkeypatch):
    _patch_record_stub(monkeypatch, "Search", lambda req, timeout=None: _search_response())
    out = client.search_vectors("docs", query_vector=[0.1], include_metadata=False)
    assert out[0].metadata is None


def test_search_vectors_requires_query(client):
    with pytest.raises(ValueError, match="query_vector"):
        client.search_vectors("docs")


def test_search_alias(client, monkeypatch):
    _patch_record_stub(monkeypatch, "Search", lambda req, timeout=None: _search_response())
    out = client.search(collection_name="docs", query_vector=[0.1], k=3)
    assert out[0].id == "v1"


def test_search_alias_defaults(client, monkeypatch):
    _patch_record_stub(monkeypatch, "Search", lambda req, timeout=None: _search_response())
    out = client.search("docs", [0.1])  # positional, default top_k
    assert len(out) == 1


def test_search_alias_missing_collection(client):
    with pytest.raises(ValueError, match="collection_id or collection_name"):
        client.search(query_vector=[0.1])


# --------------------------------------------------------------------------
# get_vector (v1 VectorService)
# --------------------------------------------------------------------------
def _vector_get_response(success=True, with_result=True):
    resp = vt.VectorOperationResponse(success=success)
    if with_result:
        item = resp.results.results.add()
        item.id = "v1"
        item.vector.extend([0.1, 0.2])
        item.metadata["s"].string_value = "txt"
        item.metadata["i"].int64_value = 5
        item.metadata["n"].number_value = 1.5
        item.metadata["b"].bool_value = True
        item.timestamp = 999
        item.version = 3
        item.source = "src"
    return resp


def test_get_vector(client, monkeypatch):
    monkeypatch.setattr(
        gs.v1_vector_pb2_grpc,
        "VectorServiceStub",
        make_stub("VectorGet", lambda req, timeout=None: _vector_get_response()),
    )
    out = client.get_vector("docs", "v1")
    assert out.id == "v1"
    assert out["vector"] == pytest.approx([0.1, 0.2])
    assert out.metadata["s"] == "txt"
    assert out.metadata["i"] == 5
    assert out["timestamp_ms"] == 999
    assert out["version"] == 3


def test_get_vector_not_found_unsuccessful(client, monkeypatch):
    monkeypatch.setattr(
        gs.v1_vector_pb2_grpc,
        "VectorServiceStub",
        make_stub("VectorGet", lambda req, timeout=None: _vector_get_response(success=False)),
    )
    with pytest.raises(ProximaDBError, match="not found"):
        client.get_vector("docs", "v1")


def test_get_vector_empty_results(client, monkeypatch):
    monkeypatch.setattr(
        gs.v1_vector_pb2_grpc,
        "VectorServiceStub",
        make_stub(
            "VectorGet",
            lambda req, timeout=None: _vector_get_response(with_result=False),
        ),
    )
    with pytest.raises(ProximaDBError, match="not found"):
        client.get_vector("docs", "v1")


# --------------------------------------------------------------------------
# Graph operations (v1 GraphService)
# --------------------------------------------------------------------------
def _patch_graph_stub(monkeypatch, method, fn):
    monkeypatch.setattr(gs.v1_graph_pb2_grpc, "GraphServiceStub", make_stub(method, fn))


def _node(node_id="n1"):
    n = gpb.Node(id=node_id, labels=["L"], created_at_ms=1000, updated_at_ms=2000)
    n.properties["name"].string_value = "alice"
    n.properties["age"].int_value = 30
    n.properties["score"].double_value = 1.5
    n.properties["active"].bool_value = True
    n.properties["tags"].array_value.values.add().string_value = "x"
    n.properties["meta"].object_value.fields["k"].string_value = "v"
    return n


def test_create_node(client, monkeypatch):
    def create(req, timeout=None):
        assert req.node.id == "n1"
        return _node()

    _patch_graph_stub(monkeypatch, "CreateNode", create)
    out = client.create_node(
        "n1", ["L"], properties={"name": "alice", "tags": ["x"], "meta": {"k": "v"}}
    )
    assert out["id"] == "n1"
    assert out["properties"]["name"] == "alice"
    assert out["properties"]["age"] == 30
    assert out["properties"]["tags"] == ["x"]
    assert out["properties"]["meta"] == {"k": "v"}
    assert out["created_at"] is not None


def test_create_node_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "CreateNode",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "graph err")
        ),
    )
    with pytest.raises(ProximaDBError, match="create_node RPC failed"):
        client.create_node("n1", ["L"])


def test_create_node_generic_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "CreateNode",
        lambda req, timeout=None: (_ for _ in ()).throw(RuntimeError("x")),
    )
    with pytest.raises(ProximaDBError, match="create_node failed"):
        client.create_node("n1", ["L"])


def _edge(edge_id="e1"):
    e = gpb.Edge(
        id=edge_id, from_node_id="a", to_node_id="b", edge_type="KNOWS",
        weight=2.5, created_at_ms=1000, updated_at_ms=2000,
    )
    e.properties["since"].int_value = 2020
    return e


def test_create_edge(client, monkeypatch):
    def create(req, timeout=None):
        assert req.edge.edge_type == "KNOWS"
        return _edge()

    _patch_graph_stub(monkeypatch, "CreateEdge", create)
    out = client.create_edge("e1", "a", "b", "KNOWS", properties={"since": 2020}, weight=2.5)
    assert out["id"] == "e1"
    assert out["weight"] == pytest.approx(2.5)
    assert out["properties"]["since"] == 2020


def test_create_edge_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "CreateEdge",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "e")
        ),
    )
    with pytest.raises(ProximaDBError, match="create_edge RPC failed"):
        client.create_edge("e1", "a", "b", "KNOWS")


def test_traverse_graph(client, monkeypatch):
    def traverse(req, timeout=None):
        assert req.start_node_id == "n1"
        assert req.algorithm == gpb.TRAVERSAL_ALGORITHM_DFS
        resp = gpb.TraversalResponse()
        resp.nodes.append(_node())
        resp.edges.append(_edge())
        resp.paths.add()  # GraphPath has no node_ids -> converted to []
        resp.stats.nodes_visited = 2
        resp.stats.edges_traversed = 1
        return resp

    _patch_graph_stub(monkeypatch, "TraverseGraph", traverse)
    out = client.traverse_graph("n1", algorithm="DFS", edge_types=["KNOWS"], limit=10)
    assert len(out["nodes"]) == 1
    assert out["paths"] == [[]]
    assert out["stats"]["nodes_visited"] == 2


def test_traverse_graph_parallel_bfs(client, monkeypatch):
    def traverse(req, timeout=None):
        assert req.algorithm == gpb.TRAVERSAL_ALGORITHM_PARALLEL_BFS
        return gpb.TraversalResponse()

    _patch_graph_stub(monkeypatch, "TraverseGraph", traverse)
    out = client.traverse_graph("n1", algorithm="PARALLEL_BFS")
    assert out["nodes"] == []


def test_traverse_graph_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "TraverseGraph",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "t")
        ),
    )
    with pytest.raises(ProximaDBError, match="traverse_graph RPC failed"):
        client.traverse_graph("n1")


def test_query_nodes(client, monkeypatch):
    def query(req, timeout=None):
        assert req.labels == ["L"]
        assert len(req.filters) == 1
        resp = gpb.QueryNodesResponse() if hasattr(gpb, "QueryNodesResponse") else gpb.TraversalResponse()
        resp.nodes.append(_node())
        return resp

    _patch_graph_stub(monkeypatch, "QueryNodes", query)
    out = client.query_nodes(labels=["L"], properties={"name": "alice"}, limit=5, offset=0)
    assert out["total_count"] == 1
    assert out["nodes"][0]["id"] == "n1"


def test_query_nodes_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "QueryNodes",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "q")
        ),
    )
    with pytest.raises(ProximaDBError, match="query_nodes RPC failed"):
        client.query_nodes()


def test_query_edges(client, monkeypatch):
    def query(req, timeout=None):
        assert req.from_node_id == "a"
        resp = gpb.QueryEdgesResponse() if hasattr(gpb, "QueryEdgesResponse") else gpb.TraversalResponse()
        resp.edges.append(_edge())
        return resp

    _patch_graph_stub(monkeypatch, "QueryEdges", query)
    out = client.query_edges(
        edge_type="KNOWS", from_node_id="a", to_node_id="b",
        properties={"since": 2020}, limit=5, offset=1,
    )
    assert out["total_count"] == 1
    assert out["edges"][0]["id"] == "e1"


def test_query_edges_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "QueryEdges",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "qe")
        ),
    )
    with pytest.raises(ProximaDBError, match="query_edges RPC failed"):
        client.query_edges(edge_type="X")


def test_get_node(client, monkeypatch):
    _patch_graph_stub(monkeypatch, "GetNode", lambda req, timeout=None: _node("n5"))
    out = client.get_node("n5")
    assert out["id"] == "n5"


def test_get_node_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "GetNode",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "g")
        ),
    )
    with pytest.raises(ProximaDBError, match="get_node RPC failed"):
        client.get_node("n5")


def test_delete_node(client, monkeypatch):
    _patch_graph_stub(monkeypatch, "DeleteNode", lambda req, timeout=None: _node("n9"))
    out = client.delete_node("n9")
    assert out["id"] == "n9"


def test_delete_node_rpc_error(client, monkeypatch):
    _patch_graph_stub(
        monkeypatch,
        "DeleteNode",
        lambda req, timeout=None: (_ for _ in ()).throw(
            FakeRpcError(grpc.StatusCode.INTERNAL, "d")
        ),
    )
    with pytest.raises(ProximaDBError, match="delete_node RPC failed"):
        client.delete_node("n9")


def test_get_outgoing_and_incoming_edges(client, monkeypatch):
    def query(req, timeout=None):
        resp = gpb.TraversalResponse()
        resp.edges.append(_edge())
        return resp

    _patch_graph_stub(monkeypatch, "QueryEdges", query)
    out = client.get_outgoing_edges("a", edge_types=["KNOWS"])
    assert len(out) == 1
    inc = client.get_incoming_edges("b")
    assert len(inc) == 1


def test_shortest_path(client, monkeypatch):
    def sp(req, timeout=None, metadata=None):
        assert req.start_node_id == "a"
        assert ("x-graph-prefetch-enabled", "true") in (metadata or [])
        assert ("x-graph-prefetch-budget", "16") in (metadata or [])
        resp = gpb.TraversalResponse()
        resp.paths.add()  # GraphPath has no node_ids -> converted to []
        return resp

    _patch_vector_stub_noop(monkeypatch)
    _patch_graph_stub(monkeypatch, "ShortestPath", sp)
    out = client.shortest_path(
        "a", "b", max_depth=5, algorithm="ASTAR",
        enable_prefetch=True, prefetch_budget=16,
    )
    assert len(out.paths) == 1


def test_shortest_path_default_algo(client, monkeypatch):
    def sp(req, timeout=None, metadata=None):
        assert req.algorithm == gpb.ShortestPathAlgorithm.SHORTEST_PATH_ALGORITHM_DIJKSTRA
        assert metadata == []
        return gpb.TraversalResponse()

    _patch_vector_stub_noop(monkeypatch)
    _patch_graph_stub(monkeypatch, "ShortestPath", sp)
    client.shortest_path("a", "b", algorithm="UNKNOWN")


# --------------------------------------------------------------------------
# Property value conversion edge cases
# --------------------------------------------------------------------------
def test_convert_property_value_variants(client):
    for v, expected in [
        ("s", "s"),
        (True, True),
        (5, 5),
        (1.5, 1.5),
        (b"raw", b"raw"),
        ([1, 2], [1, 2]),
        ({"k": "v"}, {"k": "v"}),
    ]:
        pv = client._convert_to_property_value(v)
        assert client._convert_from_property_value(pv) == expected


def test_convert_property_value_fallback(client):
    class C:
        def __str__(self):
            return "cc"

    pv = client._convert_to_property_value(C())
    assert client._convert_from_property_value(pv) == "cc"


def test_convert_path_from_proto_no_attr(client):
    class NoNodeIds:
        pass

    assert client._convert_path_from_proto(NoNodeIds()) == []


def test_module_alias():
    assert gs.ProximaDBClient is gs.ProximaDBSyncGrpcClient
