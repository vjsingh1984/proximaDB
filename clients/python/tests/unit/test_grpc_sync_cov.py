"""Offline unit tests for proximadb_sdk.protocols.grpc_sync.

Fully offline: no real channel is ever opened. The client's pool initializer is
patched out, a fake connection pool is injected, and each *Stub class on the
module is monkeypatched so RPCs return real *_pb2 response messages (or simple
fakes for graph responses that the code only touches via attribute access).
"""

from types import SimpleNamespace

import pytest

import proximadb_sdk.protocols.grpc_sync as gs
from proximadb_sdk.exceptions import ProximaDBError

# Real pb2 modules pulled off the module under test (guaranteed importable).
v1_ct = gs.v1_collection_types_pb2
v1_types = gs.v1_types_pb2
v1_vt = gs.v1_vector_types_pb2
v1_graph = gs.v1_graph_pb2
v2 = gs.v2_record_pb2


# --------------------------------------------------------------------------
# Fake transport plumbing
# --------------------------------------------------------------------------
class FakeChannel:
    """Harmless stand-in for a gRPC channel.

    The real *Stub constructors call channel.unary_unary(...) for every RPC at
    construction time; the pool-helper wrappers build a real stub before the op
    closure builds the fake one. We return a dummy callable so that real stub
    construction never touches a socket.
    """

    def _dummy(self, *args, **kwargs):
        def _call(*a, **k):
            raise AssertionError("dummy channel RPC should never be invoked")

        return _call

    unary_unary = _dummy
    unary_stream = _dummy
    stream_unary = _dummy
    stream_stream = _dummy


class FakePool:
    """Stands in for GrpcConnectionPool (detected by `get_channel`)."""

    def __init__(self):
        self.channel = FakeChannel()
        self.returned = []
        self.closed = False
        self._metrics = {"requests": 1, "channels": 5}

    def get_channel(self):
        return self.channel

    def return_channel(self, channel, success=True):
        self.returned.append(success)

    def get_metrics(self):
        return self._metrics

    def close(self):
        self.closed = True


def make_client(monkeypatch):
    """Build a client without ever initializing a real pool."""
    monkeypatch.setattr(
        gs.ProximaDBSyncGrpcClient, "_init_connection_pool", lambda self: None
    )
    c = gs.ProximaDBSyncGrpcClient(server_address="localhost:5678", timeout=1.0)
    pool = FakePool()
    c._connection_pool = pool
    c._pool = pool
    return c


class _StubInstaller:
    """Install a fake stub class returning canned responses per RPC method."""

    def __init__(self, monkeypatch):
        self.mp = monkeypatch

    def install(self, grpc_module_attr, stub_attr_name, rpc_responses):
        grpc_mod = getattr(gs, grpc_module_attr)

        class _FakeStub:
            def __init__(self, channel):
                self._channel = channel

        def _make_rpc(resp):
            def _rpc(req, timeout=None, metadata=None):
                if callable(resp):
                    return resp(req, timeout=timeout, metadata=metadata)
                return resp

            return _rpc

        for rpc_name, resp in rpc_responses.items():
            setattr(_FakeStub, rpc_name, staticmethod(_make_rpc(resp)))

        self.mp.setattr(grpc_mod, stub_attr_name, _FakeStub)
        return _FakeStub


class FakeRpcError(gs.grpc.RpcError):
    def __init__(self, code, details):
        self._code = code
        self._details = details

    def code(self):
        return self._code

    def details(self):
        return self._details


# --------------------------------------------------------------------------
# Pure helpers / wrappers
# --------------------------------------------------------------------------
def test_collection_wrapper():
    proto = v1_ct.Collection(id="cid", config=v1_ct.CollectionConfig(name="n", dimension=8))
    w = gs.CollectionWrapper(proto)
    assert w.name == "n"
    assert w.dimension == 8
    assert w.id == "cid"
    assert w.config is not None
    assert w.stats is not None
    assert w.created_at == 0  # passthrough __getattr__
    assert "CollectionWrapper" in repr(w)


def test_collection_wrapper_no_config():
    w = gs.CollectionWrapper(SimpleNamespace(id="x"))
    assert w.name is None
    assert w.dimension is None
    assert w.id == "x"


def test_search_results_wrapper():
    w = gs.SearchResultsWrapper(["a", "b", "c"])
    assert len(w) == 3
    assert w[0] == "a"
    assert list(iter(w)) == ["a", "b", "c"]
    assert w.results == ["a", "b", "c"]
    assert "count=3" in repr(w)


def test_vector_wrapper():
    w = gs.VectorWrapper({"id": "v1", "vector": [1, 2], "metadata": {"k": "v"}})
    assert w.id == "v1"
    assert w["vector"] == [1, 2]
    assert w.get("metadata") == {"k": "v"}
    assert w.get("missing", 42) == 42
    assert "v1" in repr(w)


def test_dict_wrapper():
    w = gs.DictWrapper({"status": "ok", "n": 1})
    assert w.status == "ok"
    assert w["n"] == 1
    assert w.get("missing") is None
    assert "status" in repr(w)


def test_health_and_delete_dataclasses():
    h = gs.HealthCheckResponse(healthy=True, latency_ms=1.0, status="ok", server_address="a")
    assert h.healthy and h.version is None
    d = gs.DeleteCollectionResponse(success=True, collection_id="c")
    assert d.status == "deleted"


# --------------------------------------------------------------------------
# SqlValue <-> python round trip
# --------------------------------------------------------------------------
def test_sql_value_roundtrip(monkeypatch):
    c = make_client(monkeypatch)
    samples = [None, True, 123, 3.14, b"bytes", "hello", [1, "two", 3.0], {"a": 1, "b": [True, None]}]
    for s in samples:
        sv = c._python_to_sql_value(s)
        back = c._sql_value_to_python(sv)
        if isinstance(s, (bytes, bytearray)):
            assert back == bytes(s)
        else:
            assert back == s


def test_sql_value_unknown_kind(monkeypatch):
    c = make_client(monkeypatch)
    empty = v1_types.SqlValue()  # no oneof set
    assert c._sql_value_to_python(empty) is None
    sv = c._python_to_sql_value(object())
    assert c._sql_value_to_python(sv).startswith("<object")


# --------------------------------------------------------------------------
# v2 TypedValue <-> python
# --------------------------------------------------------------------------
def test_v2_typed_value_roundtrip(monkeypatch):
    c = make_client(monkeypatch)
    cases = [None, True, 7, 2.5, b"bin", "text", [1, 2, "x"], {"nested": {"a": 1}}]
    for case in cases:
        tv = c._python_to_v2_typed_value(case)
        back = c._v2_typed_value_to_python(tv)
        if case is None:
            assert back is None
        elif isinstance(case, (bytes, bytearray)):
            assert back == bytes(case)
        else:
            assert back == case


def test_v2_typed_value_hints(monkeypatch):
    c = make_client(monkeypatch)
    tv32 = c._python_to_v2_typed_value({"type": "float32", "value": 1.5})
    assert c._v2_typed_value_to_python(tv32) == pytest.approx(1.5)
    tvsym = c._python_to_v2_typed_value({"type": "symbol", "value": "S"})
    assert c._v2_typed_value_to_python(tvsym) == "S"
    tvobj = c._python_to_v2_typed_value(object())
    assert isinstance(c._v2_typed_value_to_python(tvobj), str)


def test_v2_typed_value_empty(monkeypatch):
    c = make_client(monkeypatch)
    assert c._v2_typed_value_to_python(v2.TypedValue()) is None


# --------------------------------------------------------------------------
# normalize / record proto building
# --------------------------------------------------------------------------
def test_normalize_vector_alias_records(monkeypatch):
    c = make_client(monkeypatch)
    out = c._normalize_vector_alias_records(
        [
            {"id": "a", "vector": [1.0], "metadata": {"m": 1}, "version": 3},
            {"oid": "b", "vector": [2.0], "props": {"p": 2}},
            [9.0, 8.0],
        ]
    )
    assert out[0]["id"] == "a" and out[0]["props"] == {"m": 1} and out[0]["version"] == 3
    assert out[1]["id"] == "b" and out[1]["props"] == {"p": 2}
    assert out[2]["id"] == "record_2" and out[2]["vector"] == [9.0, 8.0]


def test_normalize_with_model_dump(monkeypatch):
    c = make_client(monkeypatch)

    class Rec:
        def model_dump(self, exclude_none=False):
            return {"id": "md", "vector": [1.0], "metadata": {"x": 1}}

    out = c._normalize_vector_alias_records([Rec()])
    assert out[0]["id"] == "md"


def test_record_proto_for_grpc(monkeypatch):
    c = make_client(monkeypatch)
    proto = c._record_proto_for_grpc(
        {
            "id": "r1",
            "vector": [1.0, 2.0],
            "vector_dimension": 2,
            "props": {"a": "x"},
            "typed_fields": {"tf": {"value": 5, "value_type": "integer"}},
            "timestamp_ms": 100,
            "version": 2,
            "partition_values": {"p": "v"},
            "custom_metadata": {"c": "m"},
        }
    )
    assert proto.id == "r1"
    assert list(proto.vector) == [1.0, 2.0]
    assert proto.vector_dimension == 2
    assert "a" in proto.props
    assert proto.timestamp_ms == 100


def test_record_proto_from_embeddings(monkeypatch):
    c = make_client(monkeypatch)
    proto = c._record_proto_for_grpc({"id": "e", "embeddings": [{"values": [3.0]}]})
    assert list(proto.vector) == [3.0]


def test_record_proto_missing_vector(monkeypatch):
    c = make_client(monkeypatch)
    with pytest.raises(ValueError):
        c._record_proto_for_grpc({"id": "no_vec"})


def test_record_proto_bad_type(monkeypatch):
    c = make_client(monkeypatch)
    with pytest.raises(TypeError):
        c._record_proto_for_grpc(12345)


# --------------------------------------------------------------------------
# Pool lifecycle
# --------------------------------------------------------------------------
def test_pool_metrics_and_close(monkeypatch):
    c = make_client(monkeypatch)
    assert c.get_pool_metrics() == {"requests": 1, "channels": 5}
    c.close()
    assert c._connection_pool.closed is True


def test_context_manager(monkeypatch):
    c = make_client(monkeypatch)
    with c as ctx:
        assert ctx is c
    assert c._connection_pool.closed is True


def test_get_pool_metrics_none(monkeypatch):
    c = make_client(monkeypatch)
    c._connection_pool = None
    assert c.get_pool_metrics() is None
    c.close()  # no-op


def test_init_pool_failure(monkeypatch):
    def boom(*a, **k):
        raise RuntimeError("nope")

    monkeypatch.setattr(gs, "GrpcConnectionPool", boom)
    with pytest.raises(ProximaDBError):
        gs.ProximaDBSyncGrpcClient(server_address="localhost:5678")


def test_init_pool_compression(monkeypatch):
    captured = {}

    class FakeGcp:
        def __init__(self, **kwargs):
            captured.update(kwargs)

        def get_metrics(self):
            return {}

    monkeypatch.setattr(gs, "GrpcConnectionPool", FakeGcp)
    gs.ProximaDBSyncGrpcClient(
        server_address="x:1", enable_compression=True, compression_algorithm="deflate"
    )
    assert captured["compression"] is not None
    gs.ProximaDBSyncGrpcClient(
        server_address="x:1", enable_compression=True, compression_algorithm="weird"
    )  # unknown -> gzip fallback


# --------------------------------------------------------------------------
# Collection RPC wrappers (v1)
# --------------------------------------------------------------------------
def test_create_collection_v1(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_ct.Collection(id="c1", config=v1_ct.CollectionConfig(name="n", dimension=4))
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"CreateCollection": resp}
    )
    out = c.create_collection_v1("n", 4, distance_metric=1, storage_engine=1, tags=["t"], description="d")
    assert out.id == "c1"


def test_get_list_delete_collection_v1(monkeypatch):
    c = make_client(monkeypatch)
    coll = v1_ct.Collection(id="c1", config=v1_ct.CollectionConfig(name="n", dimension=4))
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc",
        "CollectionServiceStub",
        {
            "GetCollection": coll,
            "ListCollections": v1_ct.ListCollectionsResponse(collections=[coll]),
            "DeleteCollection": v1_ct.DeleteCollectionResponse(success=True),
        },
    )
    assert c.get_collection_v1("c1").id == "c1"
    assert c.list_collections_v1(limit=1, offset=0, include_stats=True).collections[0].id == "c1"
    assert c.delete_collection_v1("c1").success is True


def test_create_collection_unified(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_ct.Collection(id="c1", config=v1_ct.CollectionConfig(name="unified", dimension=16))
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"CreateCollection": resp}
    )
    wrapped = c.create_collection(
        name="unified", dimension=16, distance_metric=1, indexing_algorithm=1,
        storage_engine=1,
    )
    assert wrapped.name == "unified" and wrapped.dimension == 16


def test_create_collection_engine_alias_and_string(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_ct.Collection(id="c", config=v1_ct.CollectionConfig(name="s", dimension=8))
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"CreateCollection": resp}
    )
    out = c.create_collection(name="s", dimension=8, engine="viper")
    assert out.name == "s"


def test_create_collection_bad_engine(monkeypatch):
    c = make_client(monkeypatch)
    with pytest.raises(ValueError):
        c.create_collection(name="s", dimension=8, storage_engine="not_an_engine")


def test_get_collection_unified(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_ct.Collection(id="c", config=v1_ct.CollectionConfig(name="g", dimension=2))
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"GetCollection": resp}
    )
    assert c.get_collection("g").name == "g"


def test_list_collections_unified(monkeypatch):
    c = make_client(monkeypatch)
    coll = v1_ct.Collection(id="c", config=v1_ct.CollectionConfig(name="L", dimension=2))
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc",
        "CollectionServiceStub",
        {"ListCollections": v1_ct.ListCollectionsResponse(collections=[coll, coll])},
    )
    out = c.list_collections()
    assert len(out) == 2 and out[0].name == "L"


def test_delete_collection_unified(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc",
        "CollectionServiceStub",
        {"DeleteCollection": v1_ct.DeleteCollectionResponse(success=True)},
    )
    out = c.delete_collection("c1")
    assert out.success and out.collection_id == "c1"


# --------------------------------------------------------------------------
# Error mapping
# --------------------------------------------------------------------------
def test_execute_collection_rpc_error_unavailable(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.UNAVAILABLE, "down")

    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"GetCollection": boom}
    )
    with pytest.raises(ProximaDBError, match="connection failed"):
        c.get_collection("x")


def test_execute_collection_rpc_error_generic(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "boom")

    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"GetCollection": boom}
    )
    with pytest.raises(ProximaDBError, match="RPC failed"):
        c.get_collection("x")


def test_execute_collection_generic_exception(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise ValueError("kaboom")

    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"GetCollection": boom}
    )
    with pytest.raises(ProximaDBError, match="failed"):
        c.get_collection("x")


def test_execute_with_pool_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.UNAVAILABLE, "x")

    _StubInstaller(monkeypatch).install(
        "v1_vector_pb2_grpc", "VectorServiceStub", {"VectorGet": boom}
    )
    with pytest.raises(ProximaDBError, match="connection failed"):
        c.get_vector("col", "v1")


def test_grpc_unavailable_guard(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "GRPC_AVAILABLE", False)
    with pytest.raises(ProximaDBError, match="gRPC not available"):
        c._execute_with_pool("op", lambda stub: None)
    with pytest.raises(ProximaDBError, match="gRPC not available"):
        c._execute_collection_with_pool("op", lambda stub: None)


def test_record_pool_unavailable(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "v2_record_pb2_grpc", None)
    with pytest.raises(ProximaDBError, match="v2 record gRPC stubs"):
        c._execute_record_with_pool("op", lambda stub: None)


# --------------------------------------------------------------------------
# Health check
# --------------------------------------------------------------------------
def test_health_check_ok(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc",
        "CollectionServiceStub",
        {"ListCollections": v1_ct.ListCollectionsResponse(collections=[])},
    )
    h = c.health_check()
    assert h.healthy is True and h.status == "connected"


def test_health_check_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.UNAVAILABLE, "no")

    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"ListCollections": boom}
    )
    h = c.health_check()
    assert h.healthy is False and "error" in h.status


def test_health_check_generic_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise ValueError("weird")

    _StubInstaller(monkeypatch).install(
        "v1_collection_pb2_grpc", "CollectionServiceStub", {"ListCollections": boom}
    )
    h = c.health_check()
    assert h.healthy is False and "ValueError" in h.status


def test_health_check_grpc_unavailable(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "GRPC_AVAILABLE", False)
    with pytest.raises(ProximaDBError):
        c.health_check()


# --------------------------------------------------------------------------
# SQL
# --------------------------------------------------------------------------
def test_execute_sql(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_types.ExecuteQueryResponse(
        rows_scanned=10, rows_returned=1, execution_time_ms=2, columns=["a"], column_types=["INT"]
    )
    row = resp.rows.add()
    field = row.fields.add()
    field.key = "a"
    field.value.int64_value = 99
    _StubInstaller(monkeypatch).install("v1_sql_pb2_grpc", "QueryServiceStub", {"ExecuteQuery": resp})
    out = c.execute_sql("SELECT 1", parameters=[1, "x"], collection="col")
    assert out["row_count"] == 1
    assert out["rows"][0]["a"] == 99
    assert out["columns"] == ["a"]


def test_execute_sql_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "syntax")

    _StubInstaller(monkeypatch).install("v1_sql_pb2_grpc", "QueryServiceStub", {"ExecuteQuery": boom})
    with pytest.raises(ProximaDBError, match="execute_sql RPC failed"):
        c.execute_sql("BAD")


def test_execute_sql_generic_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise RuntimeError("oops")

    _StubInstaller(monkeypatch).install("v1_sql_pb2_grpc", "QueryServiceStub", {"ExecuteQuery": boom})
    with pytest.raises(ProximaDBError, match="execute_sql failed"):
        c.execute_sql("X")


def test_execute_sql_grpc_unavailable(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "GRPC_AVAILABLE", False)
    with pytest.raises(ProximaDBError):
        c.execute_sql("X")


# --------------------------------------------------------------------------
# Record insert / upsert / vector aliases
# --------------------------------------------------------------------------
def _batch_response(success=2, failed=0):
    resp = v2.ProximaRecordBatchResponse(
        success=failed == 0,
        total_processed=success + failed,
        success_count=success,
        failed_count=failed,
        processing_time_us=10,
    )
    if failed:
        err = resp.errors.add()
        err.record_index = 0
        err.record_id = "bad"
        err.error_message = "nope"
    return resp


def test_insert_records(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"InsertRecords": _batch_response()}
    )
    out = c.insert_records("col", [{"id": "a", "vector": [1.0]}], schema_id="s")
    assert out.total == 2 and out.success == 2 and out.failed == 0


def test_insert_records_upsert_delegates(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"UpsertRecords": _batch_response()}
    )
    out = c.insert_records("col", [{"id": "a", "vector": [1.0]}], upsert=True)
    assert out.success == 2


def test_upsert_records_with_errors(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc",
        "ProximaRecordServiceStub",
        {"UpsertRecords": _batch_response(success=1, failed=1)},
    )
    out = c.upsert_records("col", [{"id": "a", "vector": [1.0]}], schema_id="s")
    assert out.failed == 1 and out.errors


def test_insert_records_stub_missing(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "v2_record_pb2", None)
    with pytest.raises(ProximaDBError, match="stubs are required"):
        c.insert_records("col", [{"id": "a", "vector": [1.0]}])


def test_insert_vectors(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"InsertRecords": _batch_response()}
    )
    out = c.insert_vectors("col", [{"id": "v1", "vector": [1.0]}])
    assert out.success is True
    assert out.operation == "INSERT"
    assert out.vector_ids == ["v1"]


def test_insert_vectors_upsert(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"UpsertRecords": _batch_response()}
    )
    out = c.insert_vectors("col", [{"id": "v1", "vector": [1.0]}], upsert=True)
    assert out.operation == "UPSERT"


def test_insert_vector_single(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"InsertRecords": _batch_response(1, 0)}
    )
    out = c.insert_vector("col", "v1", [1.0], metadata={"m": 1})
    assert out.success is True


def test_update_vector(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"UpsertRecords": _batch_response(1, 0)}
    )
    out = c.update_vector("col", "v1", vector=[2.0], metadata={"m": 2})
    assert out["status"] == "updated" and out["success"] is True


# --------------------------------------------------------------------------
# Search
# --------------------------------------------------------------------------
def _search_response():
    resp = v2.TypedSearchResponse(collection_id="col", total_found=1)
    item = resp.results.add()
    item.id = "r1"
    item.score = 0.9
    item.vector.extend([1.0, 2.0])
    item.props["k"].text_value = "v"
    item.timestamp_ms = 123
    item.version = 2
    item.source = "src"
    return resp


def test_search_vectors(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"Search": _search_response()}
    )
    out = c.search_vectors(
        "col", query_vector=[1.0, 2.0], top_k=5, metadata_filters={"k": "v"},
        include_vectors=True, search_hints={"h": "1"},
    )
    assert len(out) == 1
    r = out[0]
    assert r.id == "r1" and r.score == pytest.approx(0.9)
    assert r.vector == [1.0, 2.0]
    assert r.metadata == {"k": "v"}


def test_search_alias(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"Search": _search_response()}
    )
    out = c.search(collection_name="col", query_vector=[1.0, 2.0], k=3)
    assert len(out) == 1


def test_search_missing_vector(monkeypatch):
    c = make_client(monkeypatch)
    with pytest.raises(ValueError):
        c.search_vectors("col")


def test_search_missing_collection(monkeypatch):
    c = make_client(monkeypatch)
    with pytest.raises(ValueError):
        c.search(query_vector=[1.0])


# --------------------------------------------------------------------------
# get_vector / delete_vector(s)
# --------------------------------------------------------------------------
def test_get_vector(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_vt.VectorOperationResponse(success=True)
    item = resp.results.results.add()
    item.id = "v1"
    item.vector.extend([1.0, 2.0])
    item.metadata["sk"].string_value = "sv"
    item.metadata["ik"].int64_value = 7
    item.timestamp = 100
    item.version = 3
    item.source = "src"
    _StubInstaller(monkeypatch).install(
        "v1_vector_pb2_grpc", "VectorServiceStub", {"VectorGet": resp}
    )
    out = c.get_vector("col", "v1")
    assert out.id == "v1"
    assert out["vector"] == [1.0, 2.0]
    assert out["metadata"]["sk"] == "sv"
    assert out["metadata"]["ik"] == 7
    assert out["version"] == 3


def test_get_vector_not_found(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_vector_pb2_grpc",
        "VectorServiceStub",
        {"VectorGet": v1_vt.VectorOperationResponse(success=False)},
    )
    with pytest.raises(ProximaDBError, match="not found"):
        c.get_vector("col", "missing")


def test_get_vector_empty_results(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_vector_pb2_grpc",
        "VectorServiceStub",
        {"VectorGet": v1_vt.VectorOperationResponse(success=True)},
    )
    with pytest.raises(ProximaDBError, match="not found"):
        c.get_vector("col", "x")


def test_delete_vector(monkeypatch):
    c = make_client(monkeypatch)
    resp = v2.ProximaRecordBatchResponse(success=True, failed_count=0, success_count=1)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"DeleteRecords": resp}
    )
    out = c.delete_vector("col", "v1")
    assert out["status"] == "deleted" and out["success"] is True


def test_delete_vectors(monkeypatch):
    c = make_client(monkeypatch)
    resp = v2.ProximaRecordBatchResponse(success=True, failed_count=0, success_count=2)
    _StubInstaller(monkeypatch).install(
        "v2_record_pb2_grpc", "ProximaRecordServiceStub", {"DeleteRecords": resp}
    )
    out = c.delete_vectors("col", ["a", "b"])
    assert out["deleted_count"] == 2 and out["total_requested"] == 2


# --------------------------------------------------------------------------
# Graph property value conversions
# --------------------------------------------------------------------------
def test_property_value_roundtrip(monkeypatch):
    c = make_client(monkeypatch)
    for val in ["s", True, 5, 1.5, b"by", ["a", 1], {"k": "v"}]:
        pv = c._convert_to_property_value(val)
        back = c._convert_from_property_value(pv)
        assert back == val


def test_property_value_unknown(monkeypatch):
    c = make_client(monkeypatch)
    pv = c._convert_to_property_value(object())
    assert isinstance(c._convert_from_property_value(pv), str)
    assert c._convert_from_property_value(v1_graph.PropertyValue()) is None


def test_convert_node_and_edge(monkeypatch):
    c = make_client(monkeypatch)
    node = v1_graph.Node(id="n1", labels=["L"], created_at_ms=1000, updated_at_ms=2000)
    node.properties["p"].string_value = "v"
    nd = c._convert_node_from_proto(node)
    assert nd["id"] == "n1" and nd["labels"] == ["L"] and nd["properties"]["p"] == "v"
    assert nd["created_at"] is not None

    edge = v1_graph.Edge(
        id="e1", from_node_id="a", to_node_id="b", edge_type="T", weight=0.5,
        created_at_ms=1000, updated_at_ms=0,
    )
    ed = c._convert_edge_from_proto(edge)
    assert ed["weight"] == pytest.approx(0.5)
    assert ed["updated_at"] is None

    path = SimpleNamespace(node_ids=["a", "b"])
    assert c._convert_path_from_proto(path) == ["a", "b"]
    assert c._convert_path_from_proto(SimpleNamespace()) == []


# --------------------------------------------------------------------------
# Graph RPCs
# --------------------------------------------------------------------------
def test_create_node(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc",
        "GraphServiceStub",
        {"CreateNode": v1_graph.Node(id="n1", labels=["L"], created_at_ms=100)},
    )
    out = c.create_node("n1", ["L"], properties={"p": "v"}, graph_id="g")
    assert out["id"] == "n1"


def test_create_node_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"CreateNode": boom}
    )
    with pytest.raises(ProximaDBError, match="create_node RPC failed"):
        c.create_node("n", ["L"])


def test_create_node_generic_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise ValueError("x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"CreateNode": boom}
    )
    with pytest.raises(ProximaDBError, match="create_node failed"):
        c.create_node("n", ["L"])


def test_create_edge(monkeypatch):
    c = make_client(monkeypatch)
    resp = v1_graph.Edge(id="e1", from_node_id="a", to_node_id="b", edge_type="T", weight=1.0, created_at_ms=10)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"CreateEdge": resp}
    )
    out = c.create_edge("e1", "a", "b", "T", properties={"p": 1}, weight=1.0)
    assert out["id"] == "e1" and out["edge_type"] == "T"


def test_create_edge_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"CreateEdge": boom}
    )
    with pytest.raises(ProximaDBError, match="create_edge RPC failed"):
        c.create_edge("e", "a", "b", "T")


def test_traverse_graph(monkeypatch):
    c = make_client(monkeypatch)
    node = v1_graph.Node(id="n1", labels=["L"])
    edge = v1_graph.Edge(id="e1", from_node_id="a", to_node_id="b", edge_type="T")
    stats = SimpleNamespace(
        nodes_visited=1, edges_traversed=1, max_depth_reached=2, execution_time_microseconds=5
    )
    resp = SimpleNamespace(nodes=[node], edges=[edge], paths=[SimpleNamespace(node_ids=["a", "b"])], stats=stats)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"TraverseGraph": resp}
    )
    out = c.traverse_graph("a", algorithm="DFS", limit=10)
    assert out["nodes"][0]["id"] == "n1"
    assert out["paths"][0] == ["a", "b"]
    assert out["stats"]["nodes_visited"] == 1


def test_traverse_graph_parallel(monkeypatch):
    c = make_client(monkeypatch)
    resp = SimpleNamespace(
        nodes=[], edges=[], paths=[],
        stats=SimpleNamespace(
            nodes_visited=0, edges_traversed=0, max_depth_reached=0, execution_time_microseconds=0
        ),
    )
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"TraverseGraph": resp}
    )
    out = c.traverse_graph("a", algorithm="PARALLEL_BFS")
    assert out["nodes"] == []


def test_traverse_graph_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"TraverseGraph": boom}
    )
    with pytest.raises(ProximaDBError, match="traverse_graph RPC failed"):
        c.traverse_graph("a")


def test_query_nodes(monkeypatch):
    c = make_client(monkeypatch)
    resp = SimpleNamespace(success=True, nodes=[v1_graph.Node(id="n1", labels=["L"])])
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"QueryNodes": resp}
    )
    out = c.query_nodes(labels=["L"], properties={"p": "v"}, limit=5, offset=0)
    assert out["total_count"] == 1 and out["nodes"][0]["id"] == "n1"


def test_query_nodes_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"QueryNodes": boom}
    )
    with pytest.raises(ProximaDBError, match="query_nodes RPC failed"):
        c.query_nodes()


def test_query_edges(monkeypatch):
    c = make_client(monkeypatch)
    edge = v1_graph.Edge(id="e1", from_node_id="a", to_node_id="b", edge_type="T")
    resp = SimpleNamespace(success=True, edges=[edge], next_token="tok")
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"QueryEdges": resp}
    )
    out = c.query_edges(
        edge_type="T", from_node_id="a", to_node_id="b", properties={"p": 1}, limit=5, offset=0
    )
    assert out["total_count"] == 1 and out["next_token"] == "tok"


def test_query_edges_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"QueryEdges": boom}
    )
    with pytest.raises(ProximaDBError, match="query_edges RPC failed"):
        c.query_edges()


def test_get_node(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"GetNode": v1_graph.Node(id="n1", labels=["L"])}
    )
    assert c.get_node("n1")["id"] == "n1"


def test_get_node_rpc_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise FakeRpcError(gs.grpc.StatusCode.INTERNAL, "x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"GetNode": boom}
    )
    with pytest.raises(ProximaDBError, match="get_node RPC failed"):
        c.get_node("n1")


def test_delete_node(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"DeleteNode": v1_graph.Node(id="n1", labels=["L"])}
    )
    assert c.delete_node("n1")["id"] == "n1"


def test_delete_node_generic_error(monkeypatch):
    c = make_client(monkeypatch)

    def boom(req, timeout=None, metadata=None):
        raise RuntimeError("x")

    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"DeleteNode": boom}
    )
    with pytest.raises(ProximaDBError, match="delete_node failed"):
        c.delete_node("n1")


def test_outgoing_incoming_edges(monkeypatch):
    c = make_client(monkeypatch)
    edge = v1_graph.Edge(id="e1", from_node_id="a", to_node_id="b", edge_type="T")
    resp = SimpleNamespace(success=True, edges=[edge], next_token="")
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"QueryEdges": resp}
    )
    assert len(c.get_outgoing_edges("a", edge_types=["T"])) == 1
    assert len(c.get_incoming_edges("b")) == 1  # default edge_types


def test_shortest_path(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"ShortestPath": SimpleNamespace(path_found=True)}
    )
    out = c.shortest_path(
        "a", "b", max_depth=5, edge_types=["T"], algorithm="ASTAR", k=2,
        enable_prefetch=True, prefetch_budget=100,
    )
    assert out.path_found is True


def test_shortest_path_default_algo(monkeypatch):
    c = make_client(monkeypatch)
    _StubInstaller(monkeypatch).install(
        "v1_graph_pb2_grpc", "GraphServiceStub", {"ShortestPath": SimpleNamespace(path_found=False)}
    )
    out = c.shortest_path("a", "b", enable_prefetch=False)
    assert out.path_found is False


def test_graph_stubs_missing(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "v1_graph_pb2_grpc", None)
    with pytest.raises(ProximaDBError, match="GraphService stubs"):
        c.create_node("n", ["L"])
    with pytest.raises(ProximaDBError, match="GraphService stubs"):
        c.shortest_path("a", "b")


def test_graph_grpc_unavailable(monkeypatch):
    c = make_client(monkeypatch)
    monkeypatch.setattr(gs, "GRPC_AVAILABLE", False)
    with pytest.raises(ProximaDBError, match="gRPC not available"):
        c.create_node("n", ["L"])
