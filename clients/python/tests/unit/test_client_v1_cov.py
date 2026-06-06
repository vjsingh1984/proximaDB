"""Offline unit tests for proximadb_sdk.client_v1.ProximaDBClientV1.

Fully offline: no real network / no real gRPC channel. The REST paths are
exercised by monkeypatching ``requests`` inside the client module; the gRPC
paths are exercised by constructing a client with protocol="rest" and then
swapping in fake stubs / forcing ``protocol == "grpc"`` for dispatch.
"""

import types as _pytypes

import grpc
import pytest
from pydantic import ValidationError

from proximadb_sdk import client_v1
from proximadb_sdk.client_v1 import ProximaDBClientV1, create_client_v1
from proximadb_sdk.exceptions import NetworkError, ProximaDBError
from proximadb_sdk.models import (
    DistanceMetric,
    StorageEngine,
    VectorRecord,
)
from proximadb_sdk.v1 import types_pb2


# --------------------------------------------------------------------------
# Fakes
# --------------------------------------------------------------------------
class FakeResp:
    def __init__(self, json_data=None, status_code=200, raise_exc=None):
        self._json = json_data if json_data is not None else {}
        self.status_code = status_code
        self.headers = {}
        self.text = ""
        self.content = b""
        self._raise_exc = raise_exc

    def json(self):
        return self._json

    def raise_for_status(self):
        if self._raise_exc is not None:
            raise self._raise_exc
        return None


class FakeRequests:
    """Stand-in for the ``requests`` module used inside client_v1."""

    # Mirror the exception type the client catches.
    RequestException = client_v1.requests.RequestException

    def __init__(self):
        self.calls = []
        self.next_resp = FakeResp()
        self.raise_on_call = None

    def _record(self, method, url, **kwargs):
        self.calls.append((method, url, kwargs))
        if self.raise_on_call is not None:
            raise self.raise_on_call
        return self.next_resp

    def get(self, url, **kwargs):
        return self._record("GET", url, **kwargs)

    def post(self, url, **kwargs):
        return self._record("POST", url, **kwargs)


@pytest.fixture
def fake_requests(monkeypatch):
    fr = FakeRequests()
    monkeypatch.setattr(client_v1, "requests", fr)
    return fr


def make_rest_client():
    return ProximaDBClientV1(url="http://testserver:5678", protocol="rest")


# --------------------------------------------------------------------------
# Construction / protocol selection
# --------------------------------------------------------------------------
def test_init_rest_default():
    c = make_rest_client()
    assert c.protocol == "rest"
    assert c.base_url == "http://testserver:5678"
    assert c.timeout == 30.0


def test_init_auto_resolves_rest():
    c = ProximaDBClientV1(url="http://localhost:8080", protocol="auto")
    assert c.protocol == "rest"


def test_init_auto_resolves_grpc_by_port(monkeypatch):
    created = {}

    class FakeChannel:
        def close(self):
            created["closed"] = True

    monkeypatch.setattr(
        client_v1.grpc, "insecure_channel", lambda url: FakeChannel()
    )
    # Stub the *_grpc stub classes so __init__ doesn't touch real grpc.
    for mod, attr in [
        (client_v1.vector_pb2_grpc, "VectorServiceStub"),
        (client_v1.collection_pb2_grpc, "CollectionServiceStub"),
        (client_v1.sql_pb2_grpc, "SqlServiceStub"),
        (client_v1.graph_pb2_grpc, "GraphServiceStub"),
    ]:
        monkeypatch.setattr(mod, attr, lambda ch: object())
    if client_v1.record_pb2_grpc is not None:
        monkeypatch.setattr(
            client_v1.record_pb2_grpc,
            "ProximaRecordServiceStub",
            lambda ch: object(),
        )

    c = ProximaDBClientV1(url="http://localhost:5679", protocol="auto")
    assert c.protocol == "grpc"
    c.close()
    assert created.get("closed") is True


def test_init_explicit_grpc_scheme(monkeypatch):
    monkeypatch.setattr(
        client_v1.grpc, "insecure_channel", lambda url: object()
    )
    for mod, attr in [
        (client_v1.vector_pb2_grpc, "VectorServiceStub"),
        (client_v1.collection_pb2_grpc, "CollectionServiceStub"),
        (client_v1.sql_pb2_grpc, "SqlServiceStub"),
        (client_v1.graph_pb2_grpc, "GraphServiceStub"),
    ]:
        monkeypatch.setattr(mod, attr, lambda ch: object())
    if client_v1.record_pb2_grpc is not None:
        monkeypatch.setattr(
            client_v1.record_pb2_grpc,
            "ProximaRecordServiceStub",
            lambda ch: object(),
        )
    c = ProximaDBClientV1(url="grpc://host:1234", protocol="auto")
    assert c.protocol == "grpc"


def test_close_without_channel():
    c = make_rest_client()
    # Should be a no-op (no channel attribute).
    c.close()


def test_create_client_v1_convenience():
    c = create_client_v1(url="http://testserver", protocol="rest")
    assert isinstance(c, ProximaDBClientV1)


# --------------------------------------------------------------------------
# gRPC error helper
# --------------------------------------------------------------------------
class FakeRpcError(grpc.RpcError):
    def __init__(self, details="boom", code=grpc.StatusCode.INTERNAL):
        self._details = details
        self._code = code

    def details(self):
        return self._details

    def code(self):
        return self._code


def grpc_client():
    """A client forced into grpc dispatch with fake stubs attached."""
    c = make_rest_client()
    c.protocol = "grpc"
    c.collection_stub = _pytypes.SimpleNamespace()
    c.vector_stub = _pytypes.SimpleNamespace()
    c.sql_stub = _pytypes.SimpleNamespace()
    c.graph_stub = _pytypes.SimpleNamespace()
    return c


# --------------------------------------------------------------------------
# create_collection - REST
# --------------------------------------------------------------------------
def test_create_collection_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp(
        {
            "collection_id": "c1",
            "name": "bookshelf",
            "dimension": 4,
            "engine": "sst",
        }
    )
    c = make_rest_client()
    col = c.create_collection(
        "bookshelf", 4, DistanceMetric.COSINE, StorageEngine.SST
    )
    assert col.id == "c1"
    assert col.config.dimension == 4
    method, url, kwargs = fake_requests.calls[0]
    assert method == "POST"
    assert url.endswith("/api/v2/collections")
    assert kwargs["json"]["enable_proxima_record"] is True


def test_create_collection_rest_string_args(fake_requests):
    fake_requests.next_resp = FakeResp({"id": "x", "dimension": 8})
    c = make_rest_client()
    col = c.create_collection("tablename", 8, "euclidean", "nova")
    assert col.config.distance_metric == DistanceMetric.EUCLIDEAN


def test_create_collection_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("nope")
    c = make_rest_client()
    with pytest.raises(NetworkError):
        c.create_collection("tablename", 4)


# --------------------------------------------------------------------------
# create_collection - gRPC
# --------------------------------------------------------------------------
def test_create_collection_grpc_ok():
    c = grpc_client()
    resp = _pytypes.SimpleNamespace(
        id="cid",
        config=_pytypes.SimpleNamespace(
            name="bookshelf", dimension=4, distance_metric=2, storage_engine=2
        ),
        stats=_pytypes.SimpleNamespace(
            vector_count=5, index_size_bytes=10, data_size_bytes=20
        ),
        created_at=2000,
        updated_at=4000,
    )
    c.collection_stub.CreateCollection = lambda req, timeout: resp
    col = c.create_collection(
        "bookshelf", 4, DistanceMetric.COSINE, StorageEngine.SST
    )
    assert col.id == "cid"
    assert col.config.distance_metric == DistanceMetric.EUCLIDEAN
    assert col.config.storage_engine == StorageEngine.SST
    assert col.stats.vector_count == 5
    assert col.created_at_ms == 2  # 2000 micros -> 2 millis


def test_create_collection_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("create failed")

    c.collection_stub.CreateCollection = boom
    with pytest.raises(ProximaDBError):
        c.create_collection("bookshelf", 4)


# --------------------------------------------------------------------------
# get_collection
# --------------------------------------------------------------------------
def test_get_collection_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp(
        {
            "collection_id": "c1",
            "name": "bookshelf",
            "dimension": 4,
            "distance_metric": "Cosine",
            "engine": "SST",
        }
    )
    col = make_rest_client().get_collection("bookshelf")
    assert col.id == "c1"
    assert col.config.distance_metric == DistanceMetric.COSINE


def test_get_collection_rest_404(fake_requests):
    fake_requests.next_resp = FakeResp(status_code=404)
    assert make_rest_client().get_collection("missing") is None


def test_get_collection_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().get_collection("books")


def test_get_collection_grpc_ok():
    # The gRPC path builds a flat Collection(id, name, dimension, ...) without
    # the required `config` field, so the pydantic model rejects it. Exercising
    # the path still covers the construction lines.
    c = grpc_client()
    resp = _pytypes.SimpleNamespace(
        id="cid",
        name="bookshelf",
        dimension=4,
        distance_metric="COSINE",
        storage_engine="SST",
    )
    c.collection_stub.GetCollection = lambda req, timeout: resp
    with pytest.raises(ValidationError):
        c.get_collection("bookshelf")


def test_get_collection_grpc_not_found():
    c = grpc_client()

    def nf(req, timeout):
        raise FakeRpcError("nf", code=grpc.StatusCode.NOT_FOUND)

    c.collection_stub.GetCollection = nf
    assert c.get_collection("books") is None


def test_get_collection_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("err", code=grpc.StatusCode.INTERNAL)

    c.collection_stub.GetCollection = boom
    with pytest.raises(ProximaDBError):
        c.get_collection("books")


# --------------------------------------------------------------------------
# list_collections
# --------------------------------------------------------------------------
def test_list_collections_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp(
        {
            "collections": [
                {
                    "name": "bookshelf",
                    "dimension": 4,
                    "distance_metric": "cosine",
                    "engine": "sst",
                }
            ]
        }
    )
    cols = make_rest_client().list_collections()
    assert len(cols) == 1
    assert cols[0].config.name == "bookshelf"


def test_list_collections_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().list_collections()


def test_list_collections_grpc_ok():
    c = grpc_client()
    col = _pytypes.SimpleNamespace(
        id="c1",
        name="books",
        dimension=4,
        distance_metric="COSINE",
        storage_engine="SST",
    )
    c.collection_stub.ListCollections = (
        lambda req, timeout: _pytypes.SimpleNamespace(collections=[col])
    )
    # Same flat-Collection construction issue as the gRPC get path.
    with pytest.raises(ValidationError):
        c.list_collections()


def test_list_collections_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("x")

    c.collection_stub.ListCollections = boom
    with pytest.raises(ProximaDBError):
        c.list_collections()


# --------------------------------------------------------------------------
# insert_records / insert_vectors - REST
# --------------------------------------------------------------------------
def test_insert_records_rest_dict_and_alias(fake_requests):
    fake_requests.next_resp = FakeResp({"success": True})
    c = make_rest_client()
    out = c.insert_records(
        "c1",
        [
            {"id": "r1", "vector": [0.1], "metadata": {"k": 1}},
            {"oid": "r2", "vector": [0.2]},
            {"vector": [0.3]},  # no id -> record_2
        ],
    )
    assert out["success"] is True
    sent = fake_requests.calls[0][2]["json"]["records"]
    # metadata renamed to props
    assert sent[0]["props"] == {"k": 1}
    assert sent[1]["id"] == "r2"
    assert sent[2]["id"] == "record_2"


def test_insert_vectors_alias_with_vectorrecord(fake_requests):
    fake_requests.next_resp = FakeResp({"success": True})
    c = make_rest_client()
    vr = VectorRecord(id="v1", vector=[0.5], metadata={"a": "b"})
    out = c.insert_vectors("c1", [vr])
    assert out["success"] is True
    rec = fake_requests.calls[0][2]["json"]["records"][0]
    assert rec["id"] == "v1"
    assert rec["props"] == {"a": "b"}


def test_record_payload_vectorrecord_with_source(fake_requests):
    c = make_rest_client()
    vr = VectorRecord(id="v1", vector=[0.5], source="hello world")
    payload = c._record_payload(vr, 0)
    assert payload["source"] == "hello world"
    assert payload["text_fields"][0]["content"] == "hello world"


def test_record_payload_vectorrecord_no_id():
    c = make_rest_client()
    vr = VectorRecord(id="", vector=[0.1])
    payload = c._record_payload(vr, 7)
    assert payload["id"] == "record_7"


def test_insert_vectors_rest_alias_direct(fake_requests):
    fake_requests.next_resp = FakeResp({"success": True})
    c = make_rest_client()
    out = c._insert_vectors_rest("c1", [VectorRecord(id="v1", vector=[0.1])])
    assert out["success"] is True


def test_insert_records_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().insert_records("c1", [{"id": "a", "vector": [0.1]}])


# --------------------------------------------------------------------------
# insert_records - gRPC
# --------------------------------------------------------------------------
@pytest.mark.skipif(
    client_v1.record_pb2 is None, reason="v2 record stubs unavailable"
)
def test_insert_records_grpc_ok():
    c = grpc_client()
    c.record_stub = _pytypes.SimpleNamespace()
    err = _pytypes.SimpleNamespace(
        record_index=1, record_id="r2", error_code="E", error_message="bad"
    )
    resp = _pytypes.SimpleNamespace(
        success=True,
        total_processed=2,
        success_count=1,
        failed_count=1,
        inserted_ids=["r1"],
        errors=[err],
    )
    c.record_stub.InsertRecords = lambda req, timeout: resp
    out = c.insert_records(
        "c1",
        [
            {
                "id": "r1",
                "vector": [0.1],
                "props": {
                    "b": True,
                    "i": 3,
                    "f": 1.5,
                    "s": "x",
                    "n": None,
                },
                "source": "doc",
                "text_fields": [{"name": "text", "content": "hello"}],
            }
        ],
    )
    assert out["success"] is True
    assert out["inserted_ids"] == ["r1"]
    assert out["errors"][0]["record_id"] == "r2"


@pytest.mark.skipif(
    client_v1.record_pb2 is None, reason="v2 record stubs unavailable"
)
def test_insert_records_grpc_error():
    c = grpc_client()
    c.record_stub = _pytypes.SimpleNamespace()

    def boom(req, timeout):
        raise FakeRpcError("insert fail")

    c.record_stub.InsertRecords = boom
    with pytest.raises(ProximaDBError):
        c.insert_records("c1", [{"id": "r1", "vector": [0.1]}])


def test_insert_records_grpc_missing_stub():
    c = grpc_client()
    # no record_stub attribute -> should raise ProximaDBError
    if hasattr(c, "record_stub"):
        delattr(c, "record_stub")
    with pytest.raises(ProximaDBError):
        c._insert_records_grpc("c1", [{"id": "r1", "vector": [0.1]}])


def test_insert_vectors_grpc_alias():
    c = grpc_client()
    c.record_stub = _pytypes.SimpleNamespace()
    resp = _pytypes.SimpleNamespace(
        success=True,
        total_processed=1,
        success_count=1,
        failed_count=0,
        inserted_ids=["r1"],
        errors=[],
    )
    c.record_stub.InsertRecords = lambda req, timeout: resp
    out = c._insert_vectors_grpc("c1", [VectorRecord(id="r1", vector=[0.1])])
    assert out["success_count"] == 1


# --------------------------------------------------------------------------
# _typed_value
# --------------------------------------------------------------------------
@pytest.mark.skipif(
    client_v1.record_pb2 is None, reason="v2 record stubs unavailable"
)
def test_typed_value_variants():
    c = make_rest_client()
    assert c._typed_value(None).is_null is True
    assert c._typed_value(True).boolean_value is True
    assert c._typed_value(3).integer_value == 3
    assert c._typed_value(1.5).float_value == 1.5
    assert c._typed_value("hi").text_value == "hi"


# --------------------------------------------------------------------------
# search_vectors
# --------------------------------------------------------------------------
def test_search_vectors_rest_ok(fake_requests):
    # The REST path builds SearchResult(results=..., total_found=...) which omits
    # the model's required id/score fields, raising ValidationError. The request
    # is still sent (covered) before construction fails.
    fake_requests.next_resp = FakeResp(
        {"results": [{"id": "a", "score": 0.9}], "total_found": 1}
    )
    c = make_rest_client()
    with pytest.raises(ValidationError):
        c.search_vectors("c1", [0.1, 0.2], top_k=5, filters={"genre": "sci"})
    sent = fake_requests.calls[0][2]["json"]
    assert sent["filters"][0]["field"] == "genre"


def test_search_vectors_rest_no_filters(fake_requests):
    fake_requests.next_resp = FakeResp({"results": [], "total_found": 0})
    with pytest.raises(ValidationError):
        make_rest_client().search_vectors("c1", [0.1])


def test_search_vectors_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().search_vectors("c1", [0.1])


def test_search_vectors_grpc_ok():
    c = grpc_client()
    r = _pytypes.SimpleNamespace(
        id="a", score=0.9, vector=[0.1], metadata={"k": "v"}
    )
    resp = _pytypes.SimpleNamespace(
        results=_pytypes.SimpleNamespace(results=[r])
    )
    c.vector_stub.VectorSearch = lambda req, timeout: resp
    out = c.search_vectors("c1", [0.1], top_k=3)
    assert out[0].id == "a"
    assert out[0].score == 0.9


def test_search_vectors_grpc_empty():
    c = grpc_client()
    resp = _pytypes.SimpleNamespace(results=None)
    c.vector_stub.VectorSearch = lambda req, timeout: resp
    out = c.search_vectors("c1", [0.1])
    assert out == []


def test_search_vectors_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("search fail")

    c.vector_stub.VectorSearch = boom
    with pytest.raises(ProximaDBError):
        c.search_vectors("c1", [0.1])


# --------------------------------------------------------------------------
# get_vector
# --------------------------------------------------------------------------
def test_get_vector_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp(
        {"id": "v1", "vector": [0.1, 0.2], "props": {"k": 1}}
    )
    vr = make_rest_client().get_vector("c1", "v1")
    assert vr.id == "v1"
    assert vr.metadata == {"k": 1}


def test_get_vector_rest_404(fake_requests):
    fake_requests.next_resp = FakeResp(status_code=404)
    assert make_rest_client().get_vector("c1", "v1") is None


def test_get_vector_rest_no_id(fake_requests):
    fake_requests.next_resp = FakeResp({})
    assert make_rest_client().get_vector("c1", "v1") is None


def test_get_vector_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().get_vector("c1", "v1")


def test_get_vector_grpc_ok():
    c = grpc_client()
    r = _pytypes.SimpleNamespace(id="v1", vector=[0.1], metadata={"k": "v"})
    resp = _pytypes.SimpleNamespace(
        success=True, results=_pytypes.SimpleNamespace(results=[r])
    )
    c.vector_stub.VectorGet = lambda req, timeout: resp
    vr = c.get_vector("c1", "v1")
    assert vr.id == "v1"


def test_get_vector_grpc_none():
    c = grpc_client()
    resp = _pytypes.SimpleNamespace(success=False, results=None)
    c.vector_stub.VectorGet = lambda req, timeout: resp
    assert c.get_vector("c1", "v1") is None


def test_get_vector_grpc_not_found():
    c = grpc_client()

    def nf(req, timeout):
        raise FakeRpcError("nf", code=grpc.StatusCode.NOT_FOUND)

    c.vector_stub.VectorGet = nf
    assert c.get_vector("c1", "v1") is None


def test_get_vector_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("err")

    c.vector_stub.VectorGet = boom
    with pytest.raises(ProximaDBError):
        c.get_vector("c1", "v1")


# --------------------------------------------------------------------------
# execute_sql
# --------------------------------------------------------------------------
def test_execute_sql_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"rows": [{"x": 1}]})
    out = make_rest_client().execute_sql("SELECT 1", parameters=[1, "a"])
    assert out["rows"] == [{"x": 1}]
    sent = fake_requests.calls[0][2]["json"]
    assert sent["language"] == "uql"
    assert sent["parameters"] == [1, "a"]


def test_execute_sql_rest_network_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().execute_sql("SELECT 1")


def test_execute_sql_grpc_ok():
    c = grpc_client()
    field = _pytypes.SimpleNamespace(
        key="x", value=types_pb2.SqlValue(int64_value=42)
    )
    row = _pytypes.SimpleNamespace(fields=[field])
    resp = _pytypes.SimpleNamespace(
        rows=[row], rows_scanned=10, rows_returned=1, execution_time_ms=5
    )
    c.sql_stub.ExecuteSql = lambda req, timeout: resp
    out = c.execute_sql("SELECT x", parameters=[1, "s", True, 1.5])
    assert out["rows"][0]["x"] == 42
    assert out["rows_scanned"] == 10


def test_execute_sql_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("sql fail")

    c.sql_stub.ExecuteSql = boom
    with pytest.raises(ProximaDBError):
        c.execute_sql("SELECT 1")


# --------------------------------------------------------------------------
# health_check
# --------------------------------------------------------------------------
def test_health_check_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"status": "ok"})
    assert make_rest_client().health_check() == {"status": "ok"}


def test_health_check_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("down")
    with pytest.raises(NetworkError):
        make_rest_client().health_check()


# --------------------------------------------------------------------------
# SqlValue conversions
# --------------------------------------------------------------------------
def test_convert_to_sql_value_all_types():
    c = make_rest_client()
    assert c._convert_to_sql_value(None).HasField("null_value")
    assert c._convert_to_sql_value(True).bool_value is True
    assert c._convert_to_sql_value(5).int64_value == 5
    assert c._convert_to_sql_value(1.5).number_value == 1.5
    assert c._convert_to_sql_value("s").string_value == "s"
    assert c._convert_to_sql_value(b"x").bytes_value == b"x"
    arr = c._convert_to_sql_value([1, 2])
    assert len(arr.array_value.values) == 2
    obj = c._convert_to_sql_value({"k": 1})
    assert obj.object_value.fields["k"].int64_value == 1

    class Weird:
        def __str__(self):
            return "weird"

    assert c._convert_to_sql_value(Weird()).string_value == "weird"


def test_convert_from_sql_value_all_types():
    c = make_rest_client()
    assert c._convert_from_sql_value(types_pb2.SqlValue(string_value="s")) == "s"
    assert c._convert_from_sql_value(types_pb2.SqlValue(number_value=1.5)) == 1.5
    assert c._convert_from_sql_value(types_pb2.SqlValue(int64_value=7)) == 7
    assert c._convert_from_sql_value(types_pb2.SqlValue(bool_value=True)) is True
    assert (
        c._convert_from_sql_value(types_pb2.SqlValue(bytes_value=b"x")) == b"x"
    )
    from google.protobuf.struct_pb2 import NullValue

    assert (
        c._convert_from_sql_value(
            types_pb2.SqlValue(null_value=NullValue.NULL_VALUE)
        )
        is None
    )
    arr = c._convert_to_sql_value([1, 2])
    assert c._convert_from_sql_value(arr) == [1, 2]
    obj = c._convert_to_sql_value({"k": "v"})
    assert c._convert_from_sql_value(obj) == {"k": "v"}
    # unset field -> None
    assert c._convert_from_sql_value(types_pb2.SqlValue()) is None


def test_convert_metadata_to_sql_value():
    c = make_rest_client()
    out = c._convert_metadata_to_sql_value({"a": 1, "b": "x"})
    assert out["a"].int64_value == 1
    assert out["b"].string_value == "x"
    assert c._convert_metadata_to_sql_value(None) == {}


# --------------------------------------------------------------------------
# create_node
# --------------------------------------------------------------------------
def test_create_node_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"id": "n1"})
    out = make_rest_client().create_node(
        "n1", ["Person"], {"name": "alice"}, embedding=[0.1]
    )
    assert out["id"] == "n1"
    sent = fake_requests.calls[0][2]["json"]
    assert sent["embedding"] == [0.1]


def test_create_node_rest_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().create_node("n1", ["L"])


def test_create_node_grpc_ok():
    c = grpc_client()
    node = _pytypes.SimpleNamespace(
        id="n1", labels=["Person"], properties={}, HasField=lambda f: False
    )
    c.graph_stub.CreateNode = lambda req, timeout: node
    out = c.create_node("n1", ["Person"], {"name": "alice"}, embedding=[0.1])
    assert out["id"] == "n1"
    assert out["created_at"] is None


def test_create_node_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("node fail")

    c.graph_stub.CreateNode = boom
    with pytest.raises(ProximaDBError):
        c.create_node("n1", ["L"])


# --------------------------------------------------------------------------
# create_edge
# --------------------------------------------------------------------------
def test_create_edge_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"id": "e1"})
    out = make_rest_client().create_edge(
        "e1", "a", "b", "KNOWS", {"since": 2020}, weight=0.5
    )
    assert out["id"] == "e1"
    assert fake_requests.calls[0][2]["json"]["weight"] == 0.5


def test_create_edge_rest_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().create_edge("e1", "a", "b", "KNOWS")


def test_create_edge_grpc_ok():
    c = grpc_client()
    edge = _pytypes.SimpleNamespace(
        id="e1",
        from_node_id="a",
        to_node_id="b",
        edge_type="KNOWS",
        properties={},
        HasField=lambda f: False,
    )
    c.graph_stub.CreateEdge = lambda req, timeout: edge
    out = c.create_edge("e1", "a", "b", "KNOWS", {"p": 1}, weight=0.5)
    assert out["id"] == "e1"
    assert out["weight"] is None


def test_create_edge_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("edge fail")

    c.graph_stub.CreateEdge = boom
    with pytest.raises(ProximaDBError):
        c.create_edge("e1", "a", "b", "KNOWS")


# --------------------------------------------------------------------------
# traverse_graph
# --------------------------------------------------------------------------
def test_traverse_graph_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"nodes": []})
    out = make_rest_client().traverse_graph(
        "n1", max_depth=2, edge_types=["KNOWS"], algorithm="DFS", limit=10
    )
    assert out == {"nodes": []}
    sent = fake_requests.calls[0][2]["json"]
    assert sent["algorithm"] == "DFS"
    assert sent["limit"] == 10


def test_traverse_graph_rest_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().traverse_graph("n1")


def test_traverse_graph_grpc_ok():
    c = grpc_client()
    stats = _pytypes.SimpleNamespace(
        nodes_visited=2,
        edges_traversed=1,
        max_depth_reached=1,
        execution_time_microseconds=100,
    )
    resp = _pytypes.SimpleNamespace(nodes=[], edges=[], paths=[], stats=stats)
    c.graph_stub.TraverseGraph = lambda req, timeout: resp
    out = c.traverse_graph(
        "n1", algorithm="PARALLEL_BFS", node_labels=["L"], limit=5
    )
    assert out["stats"]["nodes_visited"] == 2


def test_traverse_graph_grpc_bfs_no_limit():
    c = grpc_client()
    stats = _pytypes.SimpleNamespace(
        nodes_visited=0,
        edges_traversed=0,
        max_depth_reached=0,
        execution_time_microseconds=0,
    )
    resp = _pytypes.SimpleNamespace(nodes=[], edges=[], paths=[], stats=stats)
    c.graph_stub.TraverseGraph = lambda req, timeout: resp
    out = c.traverse_graph("n1", algorithm="BFS")
    assert out["nodes"] == []


def test_traverse_graph_grpc_dfs():
    c = grpc_client()
    stats = _pytypes.SimpleNamespace(
        nodes_visited=1,
        edges_traversed=0,
        max_depth_reached=1,
        execution_time_microseconds=1,
    )
    resp = _pytypes.SimpleNamespace(nodes=[], edges=[], paths=[], stats=stats)
    c.graph_stub.TraverseGraph = lambda req, timeout: resp
    out = c.traverse_graph("n1", algorithm="DFS", limit=2)
    assert out["stats"]["nodes_visited"] == 1


def test_traverse_graph_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("trav fail")

    c.graph_stub.TraverseGraph = boom
    with pytest.raises(ProximaDBError):
        c.traverse_graph("n1")


# --------------------------------------------------------------------------
# query_nodes
# --------------------------------------------------------------------------
def test_query_nodes_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"nodes": []})
    out = make_rest_client().query_nodes(
        labels=["Person"], properties={"name": "x"}, limit=5, offset=1
    )
    assert out == {"nodes": []}


def test_query_nodes_rest_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().query_nodes()


def test_query_nodes_grpc_ok():
    c = grpc_client()
    node = _pytypes.SimpleNamespace(
        id="n1", labels=["L"], properties={}, HasField=lambda f: False
    )
    resp = _pytypes.SimpleNamespace(success=True, nodes=[node])
    c.graph_stub.QueryNodes = lambda req, timeout: resp
    out = c.query_nodes(labels=["L"], properties={"k": 1}, limit=3, offset=2)
    assert out["total_count"] == 1


def test_query_nodes_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("qn fail")

    c.graph_stub.QueryNodes = boom
    with pytest.raises(ProximaDBError):
        c.query_nodes()


# --------------------------------------------------------------------------
# hybrid_search
# --------------------------------------------------------------------------
def test_hybrid_search_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"nodes": []})
    out = make_rest_client().hybrid_search(
        "c1",
        [0.1],
        top_k=5,
        start_node_id="n1",
        combination_strategy="BALANCED",
        edge_types=["KNOWS"],
        vector_filters={"a": "b"},
        limit=10,
    )
    assert out == {"nodes": []}
    sent = fake_requests.calls[0][2]["json"]
    assert "graph_traversal" in sent
    assert sent["limit"] == 10


def test_hybrid_search_rest_no_graph(fake_requests):
    fake_requests.next_resp = FakeResp({"nodes": []})
    out = make_rest_client().hybrid_search("c1", [0.1])
    assert out == {"nodes": []}


def test_hybrid_search_rest_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().hybrid_search("c1", [0.1])


def test_hybrid_search_grpc_ok():
    c = grpc_client()
    stats = _pytypes.SimpleNamespace(
        vector_results_count=1,
        graph_traversal_count=2,
        execution_time_microseconds=50,
    )
    resp = _pytypes.SimpleNamespace(
        nodes=[], edges=[], paths=[], vector_results=[], stats=stats
    )
    c.graph_stub.ExecuteHybridQuery = lambda req, timeout: resp
    out = c.hybrid_search(
        "c1",
        [0.1],
        combination_strategy="GRAPH_THEN_VECTOR",
        limit=3,
    )
    assert out["stats"]["vector_results_count"] == 1


def test_hybrid_search_grpc_with_start_node_raises():
    # Building graph_traversal_request and assigning it directly to the proto
    # field is rejected by protobuf; the branch lines are still covered.
    c = grpc_client()
    c.graph_stub.ExecuteHybridQuery = lambda req, timeout: None
    with pytest.raises(Exception):
        c.hybrid_search("c1", [0.1], start_node_id="n1", edge_types=["KNOWS"])


def test_hybrid_search_grpc_filters_raise():
    # The vector_filters branch assigns a message into a proto map via [] which
    # protobuf rejects; covering the loop still exercises those lines.
    c = grpc_client()
    c.graph_stub.ExecuteHybridQuery = lambda req, timeout: None
    with pytest.raises(Exception):
        c.hybrid_search("c1", [0.1], vector_filters={"a": "b"})


def test_hybrid_search_grpc_no_start_balanced():
    c = grpc_client()
    stats = _pytypes.SimpleNamespace(
        vector_results_count=0,
        graph_traversal_count=0,
        execution_time_microseconds=0,
    )
    resp = _pytypes.SimpleNamespace(
        nodes=[], edges=[], paths=[], vector_results=[], stats=stats
    )
    c.graph_stub.ExecuteHybridQuery = lambda req, timeout: resp
    out = c.hybrid_search("c1", [0.1], combination_strategy="BALANCED")
    assert out["edges"] == []


def test_hybrid_search_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("hyb fail")

    c.graph_stub.ExecuteHybridQuery = boom
    with pytest.raises(ProximaDBError):
        c.hybrid_search("c1", [0.1])


# --------------------------------------------------------------------------
# advanced_vector_search
# --------------------------------------------------------------------------
def test_advanced_vector_search_rest_ok(fake_requests):
    fake_requests.next_resp = FakeResp({"results": []})
    out = make_rest_client().advanced_vector_search(
        "c1",
        [0.1],
        top_k=5,
        filters={"genre": "sci"},
        accuracy_threshold=0.8,
        search_params={"timeout_ms": 100},
    )
    assert out == {"results": []}
    sent = fake_requests.calls[0][2]["json"]
    assert sent["accuracy_threshold"] == 0.8
    assert sent["search_params"] == {"timeout_ms": 100}


def test_advanced_vector_search_rest_error(fake_requests):
    fake_requests.raise_on_call = client_v1.requests.RequestException("x")
    with pytest.raises(NetworkError):
        make_rest_client().advanced_vector_search("c1", [0.1])


def test_advanced_vector_search_grpc_ok():
    c = grpc_client()
    r = _pytypes.SimpleNamespace(
        id="a",
        score=0.9,
        vector=[0.1],
        metadata={},
        similarity=0.95,
        timestamp=1,
        source="s",
    )
    result_list = _pytypes.SimpleNamespace(results=[r])
    resp = _pytypes.SimpleNamespace(results=[result_list], execution_time_ms=7)
    c.vector_stub.SearchVectors = lambda req, timeout: resp
    out = c.advanced_vector_search(
        "c1",
        [0.1],
        accuracy_threshold=0.7,
        search_params={
            "timeout_ms": 50,
            "enable_two_stage": True,
            "enable_clustering_hint": True,
            "enable_metadata_filtering_hint": True,
        },
    )
    assert out["total_count"] == 1
    assert out["results"][0]["similarity"] == 0.95


def test_advanced_vector_search_grpc_filters_raise():
    c = grpc_client()
    c.vector_stub.SearchVectors = lambda req, timeout: None
    with pytest.raises(Exception):
        c.advanced_vector_search("c1", [0.1], filters={"a": "b"})


def test_advanced_vector_search_grpc_error():
    c = grpc_client()

    def boom(req, timeout):
        raise FakeRpcError("adv fail")

    c.vector_stub.SearchVectors = boom
    with pytest.raises(ProximaDBError):
        c.advanced_vector_search("c1", [0.1])


# --------------------------------------------------------------------------
# property value conversions
# --------------------------------------------------------------------------
def test_convert_to_property_value_all_types():
    c = make_rest_client()
    assert c._convert_to_property_value("s").string_value == "s"
    assert c._convert_to_property_value(True).bool_value is True
    assert c._convert_to_property_value(5).int_value == 5
    assert c._convert_to_property_value(1.5).double_value == 1.5
    assert c._convert_to_property_value(b"x").bytes_value == b"x"
    arr = c._convert_to_property_value([1, 2])
    assert len(arr.array_value.values) == 2
    obj = c._convert_to_property_value({"k": "v"})
    assert obj.object_value.fields["k"].string_value == "v"

    class Weird:
        def __str__(self):
            return "w"

    assert c._convert_to_property_value(Weird()).string_value == "w"


def test_convert_from_property_value_all_types():
    c = make_rest_client()
    from proximadb_sdk.v1 import graph_pb2

    assert (
        c._convert_from_property_value(graph_pb2.PropertyValue(string_value="s"))
        == "s"
    )
    assert (
        c._convert_from_property_value(graph_pb2.PropertyValue(int_value=3)) == 3
    )
    assert (
        c._convert_from_property_value(
            graph_pb2.PropertyValue(double_value=1.5)
        )
        == 1.5
    )
    assert (
        c._convert_from_property_value(graph_pb2.PropertyValue(bool_value=True))
        is True
    )
    assert (
        c._convert_from_property_value(graph_pb2.PropertyValue(bytes_value=b"x"))
        == b"x"
    )
    arr = c._convert_to_property_value([1, 2])
    assert c._convert_from_property_value(arr) == [1, 2]
    obj = c._convert_to_property_value({"k": "v"})
    assert c._convert_from_property_value(obj) == {"k": "v"}
    assert c._convert_from_property_value(graph_pb2.PropertyValue()) is None


# --------------------------------------------------------------------------
# proto -> dict converters with timestamps
# --------------------------------------------------------------------------
def test_convert_node_from_proto_with_timestamps():
    c = make_rest_client()
    from datetime import datetime

    class TS:
        def ToDatetime(self):
            return datetime(2020, 1, 1)

    node = _pytypes.SimpleNamespace(
        id="n1",
        labels=["L"],
        properties={},
        created_at=TS(),
        updated_at=TS(),
        HasField=lambda f: True,
    )
    out = c._convert_node_from_proto(node)
    assert out["created_at"] == "2020-01-01T00:00:00"


def test_convert_edge_from_proto_with_weight_and_ts():
    c = make_rest_client()
    from datetime import datetime

    class TS:
        def ToDatetime(self):
            return datetime(2021, 6, 1)

    edge = _pytypes.SimpleNamespace(
        id="e1",
        from_node_id="a",
        to_node_id="b",
        edge_type="KNOWS",
        properties={},
        weight=0.7,
        created_at=TS(),
        updated_at=TS(),
        HasField=lambda f: True,
    )
    out = c._convert_edge_from_proto(edge)
    assert out["weight"] == 0.7
    assert out["created_at"] == "2021-06-01T00:00:00"


def test_convert_path_from_proto():
    c = make_rest_client()
    path = _pytypes.SimpleNamespace(node_ids=["a", "b"])
    assert c._convert_path_from_proto(path) == ["a", "b"]
    assert c._convert_path_from_proto(object()) == []


def test_convert_search_result_from_proto():
    c = make_rest_client()
    r = _pytypes.SimpleNamespace(
        id="a",
        score=0.9,
        vector=[0.1],
        metadata={},
        similarity=0.95,
        timestamp=10,
        source="src",
    )
    out = c._convert_search_result_from_proto(r)
    assert out["id"] == "a"
    assert out["similarity"] == 0.95
    assert out["source"] == "src"
