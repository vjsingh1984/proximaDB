"""Offline unit tests for proximadb_sdk.adapters.rest_adapter.RestProtocolAdapter.

Fully offline: the underlying ProximaDBClient is constructed (it does NOT open
a socket on init) and then its transport (the wrapped client and its HTTP
session) is replaced with hand fakes / MagicMocks. No network, no server.
"""

from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from pydantic import ValidationError

from proximadb_sdk.adapters.rest_adapter import RestProtocolAdapter
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


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeResp:
    """Minimal requests-like response."""

    def __init__(self, payload=None, status_code=200):
        self._payload = payload if payload is not None else {}
        self.status_code = status_code
        self.headers = {}
        self.text = ""
        self.content = b""
        self.raised = False

    def json(self):
        return self._payload

    def raise_for_status(self):
        self.raised = True
        return None


class FakeSession:
    """Records the last call and returns a queued FakeResp per verb."""

    def __init__(self):
        self.calls = []
        self.responses = {}  # verb -> FakeResp or Exception

    def _set(self, verb, resp):
        self.responses[verb] = resp

    def _handle(self, verb, url, **kw):
        self.calls.append((verb, url, kw))
        resp = self.responses.get(verb, FakeResp())
        if isinstance(resp, Exception):
            raise resp
        return resp

    def get(self, url, **kw):
        return self._handle("get", url, **kw)

    def post(self, url, **kw):
        return self._handle("post", url, **kw)

    def put(self, url, **kw):
        return self._handle("put", url, **kw)

    def delete(self, url, **kw):
        return self._handle("delete", url, **kw)


def _make_collection(name="testcollection", dim=4):
    return Collection(id="c1", config=CollectionConfig(name=name, dimension=dim))


@pytest.fixture
def adapter():
    """Construct a RestProtocolAdapter with a mocked underlying client + session."""
    a = RestProtocolAdapter(url="http://testserver")
    a._client = MagicMock()
    sess = FakeSession()
    a._client._session = sess
    a._client._timeout = 5.0
    # remove auto-magic optional methods unless a test wants them
    return a


# ---------------------------------------------------------------------------
# Basic properties / construction
# ---------------------------------------------------------------------------


def test_properties(adapter):
    assert adapter.protocol_name == "rest"
    assert adapter.is_connected is True


def test_close_calls_client_close(adapter):
    adapter._client.close = MagicMock()
    adapter.close()
    adapter._client.close.assert_called_once()
    assert adapter.is_connected is False


def test_close_without_close_method():
    a = RestProtocolAdapter(url="http://testserver")
    # client lacking a close method
    a._client = SimpleNamespace()
    a.close()
    assert a.is_connected is False


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


def test_health_passthrough_healthstatus(adapter):
    hs = HealthStatus(
        status="running",
        version="1.0.0",
        uptime_seconds=10,
        services={"rest": "ok"},
        timestamp_ms=1,
    )
    adapter._client.health.return_value = hs
    assert adapter.health() is hs


def test_health_from_dict(adapter):
    adapter._client.health.return_value = {
        "status": "running",
        "version": "1.0.0",
        "uptime_seconds": 1,
        "services": {"rest": "ok"},
        "timestamp_ms": 1,
    }
    result = adapter.health()
    assert isinstance(result, HealthStatus)
    assert result.status == "running"


def test_health_exception_fallback(adapter):
    adapter._client.health.side_effect = RuntimeError("boom")
    result = adapter.health()
    assert isinstance(result, HealthStatus)
    assert result.services == {"rest": "unavailable"}


# ---------------------------------------------------------------------------
# Collection ops
# ---------------------------------------------------------------------------


def test_create_collection_passthrough(adapter):
    coll = _make_collection()
    adapter._client.create_collection.return_value = coll
    assert adapter.create_collection("testcollection") is coll


def test_create_collection_from_dict(adapter):
    coll = _make_collection()
    adapter._client.create_collection.return_value = coll.model_dump()
    result = adapter.create_collection("testcollection")
    assert isinstance(result, Collection)


def test_create_collection_wrapper_raises(adapter):
    # Wrapper objects exercise the legacy branch which builds Collection with
    # name/dimension kwargs -- Collection now requires a `config`, so it raises.
    wrapper = SimpleNamespace(id="x", name="testcollection", dimension=7)
    adapter._client.create_collection.return_value = wrapper
    with pytest.raises(ValidationError):
        adapter.create_collection("testcollection")


def test_create_collection_other(adapter):
    adapter._client.create_collection.return_value = 12345
    assert adapter.create_collection("testcollection") == 12345


def test_get_collection_passthrough(adapter):
    coll = _make_collection()
    adapter._client.get_collection.return_value = coll
    assert adapter.get_collection("c1") is coll


def test_get_collection_none(adapter):
    adapter._client.get_collection.return_value = None
    assert adapter.get_collection("c1") is None


def test_get_collection_from_dict(adapter):
    coll = _make_collection()
    adapter._client.get_collection.return_value = coll.model_dump()
    assert isinstance(adapter.get_collection("c1"), Collection)


def test_get_collection_wrapper_swallowed(adapter):
    # The wrapper branch builds Collection(name=, dimension=) which raises;
    # get_collection wraps everything in try/except and returns None.
    adapter._client.get_collection.return_value = SimpleNamespace(
        id="z", name="othercollection", dimension=3
    )
    assert adapter.get_collection("c1") is None


def test_get_collection_other_object(adapter):
    obj = object()
    adapter._client.get_collection.return_value = obj
    assert adapter.get_collection("c1") is obj


def test_get_collection_exception(adapter):
    adapter._client.get_collection.side_effect = ValueError("nope")
    assert adapter.get_collection("c1") is None


def test_list_collections_passthrough_and_dict(adapter):
    coll = _make_collection()
    adapter._client.list_collections.return_value = [
        coll,
        coll.model_dump(),
        object(),  # ignored (no name attr)
    ]
    result = adapter.list_collections()
    assert len(result) == 2
    assert all(isinstance(c, Collection) for c in result)


def test_list_collections_wrapper_item_raises(adapter):
    # An item with a `name` attr hits the legacy Collection(name=,dim=) build,
    # which raises -- the conversion loop is not guarded per-item.
    adapter._client.list_collections.return_value = [
        SimpleNamespace(id="w", name="wrappercollection", dimension=2),
    ]
    with pytest.raises(ValidationError):
        adapter.list_collections()


def test_list_collections_exception(adapter):
    adapter._client.list_collections.side_effect = RuntimeError("x")
    assert adapter.list_collections() == []


def test_delete_collection_bool(adapter):
    adapter._client.delete_collection.return_value = True
    assert adapter.delete_collection("c1") is True


def test_delete_collection_success_attr(adapter):
    adapter._client.delete_collection.return_value = SimpleNamespace(success=False)
    assert adapter.delete_collection("c1") is False


def test_delete_collection_other(adapter):
    adapter._client.delete_collection.return_value = "weird"
    assert adapter.delete_collection("c1") is True


def test_delete_collection_exception(adapter):
    adapter._client.delete_collection.side_effect = RuntimeError("x")
    assert adapter.delete_collection("c1") is False


# ---------------------------------------------------------------------------
# Record payload helper / batch result conversion
# ---------------------------------------------------------------------------


def test_record_payloads_variants():
    class HasDump:
        def model_dump(self, exclude_none=True):
            return {"id": "m", "vector": [1.0]}

    payloads = RestProtocolAdapter._record_payloads(
        [{"id": "d", "vector": [0.1]}, HasDump()]
    )
    assert payloads[0]["id"] == "d"
    assert payloads[1]["id"] == "m"


def test_record_payloads_proto_converter_branch(monkeypatch):
    # A record that is neither a dict nor has model_dump falls through to the
    # ProtoConverter path.
    import proximadb_sdk.adapters.rest_adapter as mod

    monkeypatch.setattr(
        mod.ProtoConverter,
        "vector_record_to_dict",
        staticmethod(lambda rec: {"converted": True}),
    )
    payloads = RestProtocolAdapter._record_payloads([object()])
    assert payloads == [{"converted": True}]


def test_to_batch_result_passthrough():
    br = BatchResult(total=3, success=3)
    assert RestProtocolAdapter._to_batch_result(br, 3) is br


def test_to_batch_result_from_vector_response():
    resp = VectorOperationResponse(
        success=True,
        operation="INSERT",
        metrics=OperationMetrics(successful_count=2, failed_count=1),
        error_message="oops",
    )
    br = RestProtocolAdapter._to_batch_result(resp, 3)
    assert br.success == 2
    assert br.failed == 1
    assert br.errors == ["oops"]


def test_to_batch_result_from_dict():
    br = RestProtocolAdapter._to_batch_result(
        {"successful_count": 4, "failed_count": 1, "errors": ["e"]}, 5
    )
    assert br.success == 4
    assert br.failed == 1
    assert br.total == 5


def test_to_batch_result_from_object():
    obj = SimpleNamespace(success=True, failed=0)
    br = RestProtocolAdapter._to_batch_result(obj, 7)
    assert br.success == 7  # bool True -> total_count

    obj2 = SimpleNamespace(success=False, failed=0)
    br2 = RestProtocolAdapter._to_batch_result(obj2, 7)
    assert br2.success == 0


def test_batch_to_vector_response():
    br = BatchResult(total=2, success=2, metrics=OperationMetrics(successful_count=2))
    resp = RestProtocolAdapter._batch_to_vector_response(br, "INSERT")
    assert resp.operation == "INSERT"
    assert resp.error_message is None

    br2 = BatchResult(total=1, success=0, errors=["a", "b"])
    resp2 = RestProtocolAdapter._batch_to_vector_response(br2, "UPSERT")
    assert resp2.error_message == "a; b"


# ---------------------------------------------------------------------------
# Insert / upsert records + vector aliases
# ---------------------------------------------------------------------------


def test_insert_records(adapter):
    adapter._client.insert_records.return_value = {
        "successful_count": 2,
        "failed_count": 0,
    }
    result = adapter.insert_records("c1", [{"id": "a", "vector": [1.0]}, {"id": "b", "vector": [2.0]}])
    assert isinstance(result, BatchResult)
    assert result.success == 2


def test_upsert_records_native(adapter):
    adapter._client.upsert_records.return_value = {"successful_count": 1}
    result = adapter.upsert_records("c1", [{"id": "a", "vector": [1.0]}])
    assert result.success == 1


def test_upsert_records_fallback(adapter):
    # client without upsert_records: must fall back to insert_records(upsert=True)
    a = RestProtocolAdapter(url="http://testserver")
    fake = SimpleNamespace()
    captured = {}

    def insert_records(cid, payloads, **kw):
        captured["kw"] = kw
        return {"successful_count": 1}

    fake.insert_records = insert_records
    a._client = fake
    result = a.upsert_records("c1", [{"id": "a", "vector": [1.0]}])
    assert result.success == 1
    assert captured["kw"].get("upsert") is True


def test_insert_vectors_alias(adapter):
    adapter._client.insert_records.return_value = {"successful_count": 1}
    resp = adapter.insert_vectors("c1", [{"id": "a", "vector": [1.0]}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "INSERT"


def test_upsert_vectors_alias(adapter):
    adapter._client.upsert_records.return_value = {"successful_count": 1}
    resp = adapter.upsert_vectors("c1", [{"id": "a", "vector": [1.0]}])
    assert resp.operation == "UPSERT"


# ---------------------------------------------------------------------------
# get_vectors
# ---------------------------------------------------------------------------


def test_get_vectors_batch(adapter):
    vr = VectorRecord(id="a", vector=[1.0])
    adapter._client.get_vectors.return_value = [
        vr,
        {"id": "b", "vector": [2.0]},
        SimpleNamespace(id="c", vector=[3.0], metadata={"k": "v"}),
    ]
    result = adapter.get_vectors("c1", ["a", "b", "c"])
    assert len(result) == 3
    assert all(isinstance(r, VectorRecord) for r in result)


def test_get_vectors_fallback_single(adapter):
    a = RestProtocolAdapter(url="http://testserver")
    fake = SimpleNamespace()

    def get_vector(cid, vid):
        if vid == "missing":
            raise RuntimeError("not found")
        return VectorRecord(id=vid, vector=[1.0])

    fake.get_vector = get_vector
    a._client = fake
    result = a.get_vectors("c1", ["a", "missing", "b"])
    ids = {r.id for r in result}
    assert ids == {"a", "b"}


# ---------------------------------------------------------------------------
# delete_vectors / update_vector_metadata
# ---------------------------------------------------------------------------


def test_delete_vectors_passthrough(adapter):
    resp = VectorOperationResponse(
        success=True, operation="DELETE", metrics=OperationMetrics()
    )
    adapter._client.delete_vectors.return_value = resp
    assert adapter.delete_vectors("c1", ["a"]) is resp


def test_delete_vectors_built(adapter):
    adapter._client.delete_vectors.return_value = SimpleNamespace(success=True)
    result = adapter.delete_vectors("c1", ["a", "b"])
    assert isinstance(result, VectorOperationResponse)
    assert result.operation == "DELETE"
    assert result.metrics.successful_count == 2


def test_update_vector_metadata_native(adapter):
    resp = VectorOperationResponse(
        success=True, operation="UPDATE", metrics=OperationMetrics()
    )
    adapter._client.update_vector_metadata.return_value = resp
    assert adapter.update_vector_metadata("c1", "a", {"k": "v"}) is resp


def test_update_vector_metadata_native_builds(adapter):
    adapter._client.update_vector_metadata.return_value = {"ok": True}
    result = adapter.update_vector_metadata("c1", "a", {"k": "v"})
    assert isinstance(result, VectorOperationResponse)
    assert result.operation == "UPDATE"


def test_update_vector_metadata_update_metadata_method(adapter):
    a = RestProtocolAdapter(url="http://testserver")
    fake = SimpleNamespace()
    resp = VectorOperationResponse(
        success=True, operation="UPDATE", metrics=OperationMetrics()
    )
    fake.update_metadata = lambda cid, vid, meta, **kw: resp
    a._client = fake
    assert a.update_vector_metadata("c1", "a", {"k": "v"}) is resp


def test_update_vector_metadata_fallback_found(adapter):
    a = RestProtocolAdapter(url="http://testserver")
    fake = SimpleNamespace()
    fake.get_vector = lambda cid, vid: VectorRecord(
        id=vid, vector=[1.0], metadata={"old": "1"}
    )
    fake.insert_records = lambda cid, payloads, **kw: {"successful_count": 1}
    a._client = fake
    result = a.update_vector_metadata("c1", "a", {"new": "2"})
    assert isinstance(result, VectorOperationResponse)
    assert result.operation == "UPSERT"


def test_update_vector_metadata_fallback_not_found(adapter):
    # No vector found -> builds the "not found" VectorOperationResponse, but
    # that model requires `metrics` and the adapter omits it, so it raises.
    a = RestProtocolAdapter(url="http://testserver")
    fake = SimpleNamespace()
    fake.get_vector = lambda cid, vid: None
    a._client = fake
    with pytest.raises(ValidationError):
        a.update_vector_metadata("c1", "missing", {"k": "v"})


# ---------------------------------------------------------------------------
# search / batch_search
# ---------------------------------------------------------------------------


class _NumpyLike:
    def __init__(self, data):
        self._data = data

    def tolist(self):
        return self._data


def test_search_mixed_results(adapter):
    sr = SearchResult(id="a", score=0.9)
    adapter._client.search.return_value = [
        sr,
        {"vector_id": "b", "distance": 0.5, "vector": [1.0], "metadata": {"x": 1}},
        SimpleNamespace(id="c", distance=0.3, vector=[2.0], metadata={"y": 2}),
    ]
    result = adapter.search(
        "c1", _NumpyLike([1.0, 2.0]), top_k=3, include_vectors=True
    )
    assert len(result) == 3
    assert all(isinstance(r, SearchResult) for r in result)
    assert result[1].id == "b"


def test_search_none_results(adapter):
    adapter._client.search.return_value = None
    assert adapter.search("c1", [1.0]) == []


def test_batch_search_native(adapter):
    adapter._client.batch_search.return_value = [
        [SearchResult(id="a", score=0.9), {"id": "b", "score": 0.5}],
        None,
    ]
    result = adapter.batch_search(
        "c1", [_NumpyLike([1.0]), [2.0]], include_vectors=True, include_metadata=True
    )
    assert len(result) == 2
    assert len(result[0]) == 2
    assert result[1] == []


def test_batch_search_fallback(adapter):
    a = RestProtocolAdapter(url="http://testserver")
    fake = SimpleNamespace()
    fake.search = lambda **kw: [{"id": "z", "score": 0.1}]
    a._client = fake
    result = a.batch_search("c1", [[1.0], [2.0]])
    assert len(result) == 2
    assert result[0][0].id == "z"


# ---------------------------------------------------------------------------
# Query operations
# ---------------------------------------------------------------------------


def test_execute_query(adapter):
    adapter._client.execute_query.return_value = {"rows": []}
    assert adapter.execute_query("MATCH ...", collection="c") == {"rows": []}


def test_execute_uql(adapter):
    adapter._client.execute_query.return_value = {"ok": 1}
    out = adapter.execute_uql("q", parameters=[1], collection="c", limit=5)
    assert out == {"ok": 1}
    _, kwargs = adapter._client.execute_query.call_args
    assert kwargs["language"] == "uql"


def test_execute_aql(adapter):
    adapter._client.execute_query.return_value = {}
    adapter.execute_aql("q")
    _, kwargs = adapter._client.execute_query.call_args
    assert kwargs["language"] == "aql"


def test_execute_federated(adapter):
    adapter._client.execute_query.return_value = {}
    adapter.execute_federated("q")
    _, kwargs = adapter._client.execute_query.call_args
    assert kwargs["language"] == "federated"


def test_explain_query(adapter):
    adapter._client.explain_query.return_value = {"plan": "scan"}
    assert adapter.explain_query("q", collection="c") == {"plan": "scan"}


# ---------------------------------------------------------------------------
# Document operations (session-based)
# ---------------------------------------------------------------------------


def test_create_document_collection(adapter):
    adapter._client._session.responses["post"] = FakeResp({"id": "dc"})
    out = adapter.create_document_collection("docs", {"shards": 2})
    assert out == {"id": "dc"}
    verb, url, kw = adapter._client._session.calls[-1]
    assert verb == "post"
    assert "document-collections" in url
    assert kw["json"]["shards"] == 2


def test_create_document_collection_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.create_document_collection("docs")


def test_insert_document(adapter):
    adapter._client._session.responses["post"] = FakeResp({"inserted": True})
    out = adapter.insert_document("docs", {"a": 1}, id="d1")
    assert out == {"inserted": True}


def test_insert_document_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.insert_document("docs", {"a": 1})


def test_get_document_found(adapter):
    adapter._client._session.responses["get"] = FakeResp({"id": "d1", "a": 1})
    out = adapter.get_document("docs", "d1", projection=["a", "b"])
    assert out["id"] == "d1"
    _, _, kw = adapter._client._session.calls[-1]
    assert kw["params"]["projection"] == "a,b"


def test_get_document_404(adapter):
    adapter._client._session.responses["get"] = FakeResp(status_code=404)
    assert adapter.get_document("docs", "missing") is None


def test_get_document_exception(adapter):
    adapter._client._session.responses["get"] = RuntimeError("boom")
    assert adapter.get_document("docs", "d1") is None


def test_query_documents(adapter):
    adapter._client._session.responses["post"] = FakeResp({"documents": []})
    out = adapter.query_documents(
        "docs", filter={"a": 1}, projection=["a"], limit=50
    )
    assert out == {"documents": []}
    _, _, kw = adapter._client._session.calls[-1]
    assert kw["json"]["limit"] == 50
    assert kw["json"]["filter"] == {"a": 1}


def test_query_documents_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.query_documents("docs")


def test_update_document(adapter):
    adapter._client._session.responses["put"] = FakeResp({"updated": True})
    out = adapter.update_document("docs", "d1", [{"op": "set"}])
    assert out == {"updated": True}


def test_update_document_error(adapter):
    adapter._client._session.responses["put"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.update_document("docs", "d1", [])


def test_delete_document_true(adapter):
    adapter._client._session.responses["delete"] = FakeResp({"deleted": True})
    assert adapter.delete_document("docs", "d1") is True


def test_delete_document_false(adapter):
    adapter._client._session.responses["delete"] = FakeResp({})
    assert adapter.delete_document("docs", "d1") is False


def test_delete_document_exception(adapter):
    adapter._client._session.responses["delete"] = RuntimeError("fail")
    assert adapter.delete_document("docs", "d1") is False


def test_list_document_collections(adapter):
    adapter._client._session.responses["get"] = FakeResp(
        {"collections": [{"name": "x"}]}
    )
    out = adapter.list_document_collections()
    assert out == [{"name": "x"}]


def test_list_document_collections_error(adapter):
    adapter._client._session.responses["get"] = RuntimeError("fail")
    assert adapter.list_document_collections() == []


def test_delete_document_collection_true(adapter):
    adapter._client._session.responses["delete"] = FakeResp({"success": True})
    assert adapter.delete_document_collection("docs") is True


def test_delete_document_collection_error(adapter):
    adapter._client._session.responses["delete"] = RuntimeError("fail")
    assert adapter.delete_document_collection("docs") is False


# ---------------------------------------------------------------------------
# Hybrid search
# ---------------------------------------------------------------------------


def test_hybrid_search(adapter):
    adapter._client._session.responses["post"] = FakeResp({"results": [1, 2]})
    out = adapter.hybrid_search("c", "text", [1.0, 2.0], fusion_strategy="weighted")
    assert out == {"results": [1, 2]}
    _, url, kw = adapter._client._session.calls[-1]
    assert "hybrid/search" in url
    assert kw["json"]["fusion_strategy"] == "weighted"


def test_hybrid_search_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.hybrid_search("c", "text", [1.0])


# ---------------------------------------------------------------------------
# Time-series operations
# ---------------------------------------------------------------------------


def test_create_timeseries_collection(adapter):
    adapter._client._session.responses["post"] = FakeResp({"id": "ts"})
    out = adapter.create_timeseries_collection("ts", {"retention": "7d"})
    assert out == {"id": "ts"}
    _, url, kw = adapter._client._session.calls[-1]
    assert "timeseries/collections" in url
    assert kw["json"]["retention"] == "7d"


def test_create_timeseries_collection_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.create_timeseries_collection("ts")


def test_ingest_timeseries(adapter):
    adapter._client._session.responses["post"] = FakeResp({"ingested": 3})
    out = adapter.ingest_timeseries("ts", [{"t": 1, "v": 2}])
    assert out == {"ingested": 3}


def test_ingest_timeseries_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.ingest_timeseries("ts", [])


def test_query_timeseries_full(adapter):
    adapter._client._session.responses["post"] = FakeResp({"series": []})
    out = adapter.query_timeseries(
        "ts",
        "2020-01-01",
        "2020-01-02",
        aggregation="sum",
        bucket_ms=1000,
        tag_filters={"host": "a"},
    )
    assert out == {"series": []}
    _, _, kw = adapter._client._session.calls[-1]
    assert kw["json"]["bucket_ms"] == 1000
    assert kw["json"]["tag_filters"] == {"host": "a"}
    assert kw["json"]["aggregation"] == "sum"


def test_query_timeseries_minimal(adapter):
    adapter._client._session.responses["post"] = FakeResp({"series": []})
    out = adapter.query_timeseries("ts", "2020-01-01", "2020-01-02")
    assert out == {"series": []}
    _, _, kw = adapter._client._session.calls[-1]
    assert "bucket_ms" not in kw["json"]
    assert "tag_filters" not in kw["json"]


def test_query_timeseries_error(adapter):
    adapter._client._session.responses["post"] = RuntimeError("fail")
    with pytest.raises(RuntimeError):
        adapter.query_timeseries("ts", "a", "b")


def test_list_timeseries_collections(adapter):
    adapter._client._session.responses["get"] = FakeResp(
        {"collections": [{"name": "ts"}]}
    )
    assert adapter.list_timeseries_collections() == [{"name": "ts"}]


def test_list_timeseries_collections_error(adapter):
    adapter._client._session.responses["get"] = RuntimeError("fail")
    assert adapter.list_timeseries_collections() == []


def test_delete_timeseries_collection_true(adapter):
    adapter._client._session.responses["delete"] = FakeResp({"success": True})
    assert adapter.delete_timeseries_collection("ts") is True


def test_delete_timeseries_collection_error(adapter):
    adapter._client._session.responses["delete"] = RuntimeError("fail")
    assert adapter.delete_timeseries_collection("ts") is False
