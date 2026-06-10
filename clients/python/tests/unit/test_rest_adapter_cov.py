"""Offline unit tests for proximadb_sdk.adapters.rest_adapter.RestProtocolAdapter.

Fully offline: the adapter is constructed (load_config does not connect), then its
underlying REST client (``adapter._client``) is replaced with a hand-built fake that
provides the ``_session`` / ``_timeout`` attributes and the protocol methods used by
the adapter. No sockets, no servers, no sleeps.
"""

import pytest

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
    """Permissive fake HTTP response object."""

    def __init__(self, body=None, status_code=200, raise_exc=None):
        self._body = {} if body is None else body
        self.status_code = status_code
        self.headers = {}
        self.text = "ok"
        self.content = b"ok"
        self._raise_exc = raise_exc

    def json(self):
        return self._body

    def raise_for_status(self):
        if self._raise_exc is not None:
            raise self._raise_exc
        return None


class FakeSession:
    """Records calls and returns programmed FakeResp objects per verb."""

    def __init__(self):
        self.calls = []
        self.responses = {}  # verb -> FakeResp or Exception
        self.default = FakeResp({})

    def program(self, verb, resp):
        self.responses[verb] = resp

    def _handle(self, verb, url, **kwargs):
        self.calls.append((verb, url, kwargs))
        r = self.responses.get(verb, self.default)
        if isinstance(r, Exception):
            raise r
        return r

    def get(self, url, **kwargs):
        return self._handle("get", url, **kwargs)

    def post(self, url, **kwargs):
        return self._handle("post", url, **kwargs)

    def put(self, url, **kwargs):
        return self._handle("put", url, **kwargs)

    def delete(self, url, **kwargs):
        return self._handle("delete", url, **kwargs)


class FakeClient:
    """Hand fake of the low-level REST client used by the adapter."""

    def __init__(self):
        self._session = FakeSession()
        self._timeout = 30.0
        self.closed = False
        self._health = None
        self._create_collection_ret = None
        self._get_collection_ret = None
        self._list_collections_ret = []
        self._delete_collection_ret = True
        self._insert_ret = None
        self._upsert_ret = None
        self._delete_vectors_ret = None
        self._search_ret = []
        self._execute_query_ret = {"rows": []}
        self._explain_query_ret = {"plan": "scan"}
        self.last_insert = None
        self.last_search = None

    def health(self):
        return self._health

    def create_collection(self, name=None, config=None, **kwargs):
        return self._create_collection_ret

    def get_collection(self, collection_id):
        return self._get_collection_ret

    def list_collections(self):
        return self._list_collections_ret

    def delete_collection(self, collection_id):
        return self._delete_collection_ret

    def insert_records(self, collection_id, payloads, **kwargs):
        self.last_insert = (collection_id, payloads, kwargs)
        return self._insert_ret

    def upsert_records(self, collection_id, payloads, **kwargs):
        self.last_insert = (collection_id, payloads, kwargs)
        return self._upsert_ret

    def delete_vectors(self, collection_id, vector_ids, **kwargs):
        return self._delete_vectors_ret

    def search(self, **kwargs):
        self.last_search = kwargs
        return self._search_ret

    def execute_query(self, query, **kwargs):
        self.last_query = (query, kwargs)
        return self._execute_query_ret

    def explain_query(self, query, **kwargs):
        self.last_explain = (query, kwargs)
        return self._explain_query_ret

    def close(self):
        self.closed = True


@pytest.fixture
def adapter():
    a = RestProtocolAdapter(url="http://testserver")
    a._client = FakeClient()
    return a


def _collection(cid="collection_a", dim=4):
    return Collection(id=cid, config=CollectionConfig(name=cid, dimension=dim))


# ---------------------------------------------------------------------------
# Construction / properties
# ---------------------------------------------------------------------------


def test_protocol_properties(adapter):
    assert adapter.protocol_name == "rest"
    assert adapter.is_connected is True
    assert adapter._url == "http://testserver"


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


def test_health_passthrough_model(adapter):
    hs = HealthStatus(
        status="running",
        version="1.0",
        uptime_seconds=5,
        services={},
        timestamp_ms=1,
    )
    adapter._client._health = hs
    assert adapter.health() is hs


def test_health_from_dict(adapter):
    adapter._client._health = {
        "status": "running",
        "version": "2.0",
        "uptime_seconds": 1,
        "services": {},
        "timestamp_ms": 123,
    }
    result = adapter.health()
    assert isinstance(result, HealthStatus)
    assert result.version == "2.0"


def test_health_exception_fallback(adapter):
    def boom():
        raise RuntimeError("down")

    adapter._client.health = boom
    result = adapter.health()
    assert isinstance(result, HealthStatus)
    assert result.services == {"rest": "unavailable"}


# ---------------------------------------------------------------------------
# Collections
# ---------------------------------------------------------------------------


def test_create_collection_model_passthrough(adapter):
    col = _collection()
    adapter._client._create_collection_ret = col
    assert adapter.create_collection("c1") is col


def test_create_collection_from_dict(adapter):
    adapter._client._create_collection_ret = {
        "id": "c2",
        "config": {"name": "collection_two", "dimension": 8},
    }
    result = adapter.create_collection("collection_two")
    assert isinstance(result, Collection)
    assert result.id == "c2"


def test_create_collection_wrapper_object(adapter):
    # The wrapper branch builds Collection(id=, name=, dimension=) which lacks the
    # required `config` field, so the source raises ValidationError here.
    class W:
        id = "c3"
        name = "collection_three"
        dimension = 16

    adapter._client._create_collection_ret = W()
    with pytest.raises(Exception):
        adapter.create_collection(
            "collection_three",
            config=CollectionConfig(name="collection_three", dimension=16),
        )


def test_get_collection_model(adapter):
    col = _collection()
    adapter._client._get_collection_ret = col
    assert adapter.get_collection("c1") is col


def test_get_collection_none(adapter):
    adapter._client._get_collection_ret = None
    assert adapter.get_collection("missing") is None


def test_get_collection_from_dict(adapter):
    adapter._client._get_collection_ret = {
        "id": "c9",
        "config": {"name": "collection_nine", "dimension": 3},
    }
    result = adapter.get_collection("c9")
    assert isinstance(result, Collection)
    assert result.id == "c9"


def test_get_collection_wrapper_object(adapter):
    # Wrapper branch builds an invalid Collection (no config) -> raises, caught,
    # returns None.
    class W:
        id = "cw"
        name = "collection_w"
        dimension = 2

    adapter._client._get_collection_ret = W()
    assert adapter.get_collection("cw") is None


def test_get_collection_exception(adapter):
    def boom(_):
        raise RuntimeError("x")

    adapter._client.get_collection = boom
    assert adapter.get_collection("c") is None


def test_list_collections_mixed(adapter):
    adapter._client._list_collections_ret = [
        _collection("collection_a"),
        {"id": "b", "config": {"name": "collection_b", "dimension": 4}},
    ]
    result = adapter.list_collections()
    assert len(result) == 2
    assert all(isinstance(c, Collection) for c in result)


def test_list_collections_exception(adapter):
    def boom():
        raise RuntimeError("x")

    adapter._client.list_collections = boom
    assert adapter.list_collections() == []


def test_delete_collection_bool(adapter):
    adapter._client._delete_collection_ret = True
    assert adapter.delete_collection("c1") is True


def test_delete_collection_success_attr(adapter):
    class R:
        success = False

    adapter._client._delete_collection_ret = R()
    assert adapter.delete_collection("c1") is False


def test_delete_collection_other(adapter):
    adapter._client._delete_collection_ret = "deleted"
    assert adapter.delete_collection("c1") is True


def test_delete_collection_exception(adapter):
    def boom(_):
        raise RuntimeError("x")

    adapter._client.delete_collection = boom
    assert adapter.delete_collection("c1") is False


# ---------------------------------------------------------------------------
# Record operations + helpers
# ---------------------------------------------------------------------------


def test_record_payloads_variants():
    rec_model = VectorRecord(id="v1", vector=[0.1, 0.2], metadata={"k": "v"})
    payloads = RestProtocolAdapter._record_payloads(
        [{"id": "d1", "vector": [1.0]}, rec_model]
    )
    assert payloads[0] == {"id": "d1", "vector": [1.0]}
    assert payloads[1]["id"] == "v1"


def test_to_batch_result_passthrough():
    br = BatchResult(total=1, success=1, failed=0)
    assert RestProtocolAdapter._to_batch_result(br, 1) is br


def test_to_batch_result_from_vector_response():
    vor = VectorOperationResponse(
        success=True,
        operation="INSERT",
        metrics=OperationMetrics(successful_count=3, failed_count=1),
        error_message="oops",
    )
    br = RestProtocolAdapter._to_batch_result(vor, 4)
    assert br.success == 3
    assert br.failed == 1
    assert br.errors == ["oops"]


def test_to_batch_result_from_dict():
    br = RestProtocolAdapter._to_batch_result(
        {"success": 5, "failed": 2, "total": 7, "errors": ["e"]}, 7
    )
    assert br.total == 7
    assert br.success == 5
    assert br.failed == 2


def test_to_batch_result_from_object_bool_success():
    class R:
        success = True
        failed = 0

    br = RestProtocolAdapter._to_batch_result(R(), 9)
    assert br.success == 9


def test_to_batch_result_from_object_count_success():
    class R:
        success = 4
        failed = 1

    br = RestProtocolAdapter._to_batch_result(R(), 5)
    assert br.success == 4
    assert br.failed == 1


def test_insert_records(adapter):
    adapter._client._insert_ret = {"success": 2, "failed": 0}
    result = adapter.insert_records("c1", [{"id": "a"}, {"id": "b"}])
    assert isinstance(result, BatchResult)
    assert result.success == 2


def test_upsert_records_native(adapter):
    adapter._client._upsert_ret = {"success": 1, "failed": 0}
    result = adapter.upsert_records("c1", [{"id": "a"}])
    assert result.success == 1


def test_upsert_records_fallback(adapter):
    # Client without upsert_records -> falls back to insert_records(upsert=True)
    class Minimal:
        def __init__(self):
            self._session = FakeSession()
            self._timeout = 30.0
            self.last_insert = None
            self._insert_ret = {"success": 1, "failed": 0}

        def insert_records(self, collection_id, payloads, **kwargs):
            self.last_insert = (collection_id, payloads, kwargs)
            return self._insert_ret

    m = Minimal()
    adapter._client = m
    result = adapter.upsert_records("c1", [{"id": "a"}])
    assert result.success == 1
    assert m.last_insert[2].get("upsert") is True


# ---------------------------------------------------------------------------
# Vector compatibility aliases
# ---------------------------------------------------------------------------


def test_insert_vectors(adapter):
    adapter._client._insert_ret = {"success": 2, "failed": 0}
    resp = adapter.insert_vectors("c1", [{"id": "a"}, {"id": "b"}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "INSERT"
    assert resp.success == 2


def test_upsert_vectors(adapter):
    adapter._client._upsert_ret = {"success": 1, "failed": 1, "errors": ["x"]}
    resp = adapter.upsert_vectors("c1", [{"id": "a"}, {"id": "b"}])
    assert resp.operation == "UPSERT"
    assert resp.error_message == "x"


def test_get_vectors_native(adapter):
    def get_vectors(collection_id, vector_ids, include_vectors=True, **kw):
        return [
            VectorRecord(id="v1", vector=[1.0], metadata={}),
            {"id": "v2", "vector": [2.0], "metadata": {}},
        ]

    adapter._client.get_vectors = get_vectors
    recs = adapter.get_vectors("c1", ["v1", "v2"])
    assert len(recs) == 2
    assert all(isinstance(r, VectorRecord) for r in recs)


def test_get_vectors_object(adapter):
    class V:
        id = "v3"
        vector = [3.0]
        metadata = {"a": 1}

    def get_vectors(collection_id, vector_ids, include_vectors=True, **kw):
        return [V()]

    adapter._client.get_vectors = get_vectors
    recs = adapter.get_vectors("c1", ["v3"])
    assert recs[0].id == "v3"


def test_get_vectors_fallback_get_vector(adapter):
    calls = {"n": 0}

    def get_vector(collection_id, vid):
        calls["n"] += 1
        if vid == "bad":
            raise RuntimeError("nope")
        return VectorRecord(id=vid, vector=[1.0], metadata={})

    adapter._client.get_vector = get_vector
    recs = adapter.get_vectors("c1", ["good", "bad"])
    assert len(recs) == 1
    assert recs[0].id == "good"


def test_delete_vectors_passthrough(adapter):
    vor = VectorOperationResponse(
        success=True, operation="DELETE", metrics=OperationMetrics()
    )
    adapter._client._delete_vectors_ret = vor
    assert adapter.delete_vectors("c1", ["a"]) is vor


def test_delete_vectors_synthesizes_response(adapter):
    adapter._client._delete_vectors_ret = {"raw": True}
    resp = adapter.delete_vectors("c1", ["a", "b"])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "DELETE"
    assert resp.metrics.successful_count == 2


def test_update_vector_metadata_native(adapter):
    def update_vector_metadata(collection_id, vid, metadata, **kw):
        return VectorOperationResponse(
            success=True, operation="UPDATE", metrics=OperationMetrics()
        )

    adapter._client.update_vector_metadata = update_vector_metadata
    resp = adapter.update_vector_metadata("c1", "v1", {"k": "v"})
    assert resp.operation == "UPDATE"


def test_update_vector_metadata_native_non_model(adapter):
    def update_vector_metadata(collection_id, vid, metadata, **kw):
        return {"ok": True}

    adapter._client.update_vector_metadata = update_vector_metadata
    resp = adapter.update_vector_metadata("c1", "v1", {"k": "v"})
    assert isinstance(resp, VectorOperationResponse)
    assert resp.success is True


def test_update_vector_metadata_update_metadata_alias(adapter):
    class C:
        def __init__(self):
            self._session = FakeSession()
            self._timeout = 30.0

        def update_metadata(self, collection_id, vid, metadata, **kw):
            return VectorOperationResponse(
                success=True, operation="UPDATE", metrics=OperationMetrics()
            )

    adapter._client = C()
    resp = adapter.update_vector_metadata("c1", "v1", {"k": "v"})
    assert resp.operation == "UPDATE"


def test_update_vector_metadata_fallback_found(adapter):
    # Minimal client lacking update_* methods; provides get_vector + upsert path.
    class Minimal:
        def __init__(self):
            self._session = FakeSession()
            self._timeout = 30.0
            self.last_insert = None

        def get_vector(self, collection_id, vid):
            return VectorRecord(id=vid, vector=[1.0], metadata={"old": 1})

        def upsert_records(self, collection_id, payloads, **kwargs):
            self.last_insert = (collection_id, payloads, kwargs)
            return {"success": 1, "failed": 0}

    adapter._client = Minimal()
    resp = adapter.update_vector_metadata("c1", "v1", {"new": 2})
    assert resp.operation == "UPSERT"
    assert resp.success == 1


def test_update_vector_metadata_fallback_not_found(adapter):
    class Minimal:
        def __init__(self):
            self._session = FakeSession()
            self._timeout = 30.0

        def get_vector(self, collection_id, vid):
            raise RuntimeError("missing")

    adapter._client = Minimal()
    # The not-found fallback builds VectorOperationResponse without the required
    # `metrics` field, so the source raises ValidationError on this branch.
    with pytest.raises(Exception):
        adapter.update_vector_metadata("c1", "missing", {"k": "v"})


# ---------------------------------------------------------------------------
# Search
# ---------------------------------------------------------------------------


def test_search_dict_results(adapter):
    adapter._client._search_ret = [
        {"id": "a", "score": 0.9, "metadata": {"m": 1}, "vector": [1.0]},
        {"vector_id": "b", "distance": 0.5},
    ]
    results = adapter.search("c1", [0.1, 0.2], top_k=5, include_vectors=True)
    assert len(results) == 2
    assert results[0].id == "a"
    assert results[1].id == "b"


def test_search_numpy_query(adapter):
    np = pytest.importorskip("numpy")
    adapter._client._search_ret = [SearchResult(id="a", score=0.1)]
    results = adapter.search("c1", np.array([0.1, 0.2]))
    assert results[0].id == "a"
    assert isinstance(adapter._client.last_search["query_vector"], list)


def test_search_object_results(adapter):
    class R:
        id = "z"
        score = 0.42
        vector = [1.0]
        metadata = {"k": "v"}

    adapter._client._search_ret = [R()]
    results = adapter.search("c1", [0.1], include_vectors=True, include_metadata=True)
    assert results[0].id == "z"
    assert results[0].score == 0.42


def test_search_passthrough_model(adapter):
    sr = SearchResult(id="m", score=0.7)
    adapter._client._search_ret = [sr]
    results = adapter.search("c1", [0.1])
    assert results[0] is sr


def test_search_none_results(adapter):
    adapter._client._search_ret = None
    assert adapter.search("c1", [0.1]) == []


# ---------------------------------------------------------------------------
# Query surface
# ---------------------------------------------------------------------------


def test_execute_query(adapter):
    adapter._client._execute_query_ret = {"rows": [1, 2]}
    out = adapter.execute_query("SELECT 1", language="uql", limit=10)
    assert out == {"rows": [1, 2]}
    assert adapter._client.last_query[0] == "SELECT 1"


def test_execute_uql(adapter):
    adapter._client._execute_query_ret = {"rows": []}
    adapter.execute_uql("FOR d IN c RETURN d", collection="c")
    assert adapter._client.last_query[1]["language"] == "uql"


def test_execute_aql(adapter):
    adapter.execute_aql("FOR d IN c RETURN d")
    assert adapter._client.last_query[1]["language"] == "aql"


def test_execute_federated(adapter):
    adapter.execute_federated("SELECT * FROM remote", limit=3)
    assert adapter._client.last_query[1]["language"] == "federated"


def test_explain_query(adapter):
    out = adapter.explain_query("SELECT 1", language="aql", collection="c")
    assert out == {"plan": "scan"}
    assert adapter._client.last_explain[1]["language"] == "aql"


# ---------------------------------------------------------------------------
# Batch search
# ---------------------------------------------------------------------------


def test_batch_search_fallback(adapter):
    adapter._client._search_ret = [{"id": "a", "score": 0.1}]
    out = adapter.batch_search("c1", [[0.1], [0.2]], top_k=3)
    assert len(out) == 2
    assert out[0][0].id == "a"


def test_batch_search_native(adapter):
    np = pytest.importorskip("numpy")

    def batch_search(**kwargs):
        return [
            [SearchResult(id="a", score=0.1)],
            [{"id": "b", "score": 0.2, "metadata": {"m": 1}, "vector": [1.0]}],
        ]

    adapter._client.batch_search = batch_search
    out = adapter.batch_search(
        "c1", [np.array([0.1]), [0.2]], include_vectors=True, include_metadata=True
    )
    assert out[0][0].id == "a"
    assert out[1][0].id == "b"


def test_batch_search_native_none(adapter):
    def batch_search(**kwargs):
        return None

    adapter._client.batch_search = batch_search
    assert adapter.batch_search("c1", [[0.1]]) == []


# ---------------------------------------------------------------------------
# Document operations (HTTP via _session)
# ---------------------------------------------------------------------------


def test_create_document_collection(adapter):
    adapter._client._session.program("post", FakeResp({"name": "docs"}))
    out = adapter.create_document_collection("docs", config={"shards": 2})
    assert out == {"name": "docs"}
    verb, url, kw = adapter._client._session.calls[-1]
    assert verb == "post"
    assert "document-collections" in url


def test_create_document_collection_error(adapter):
    adapter._client._session.program("post", RuntimeError("boom"))
    with pytest.raises(RuntimeError):
        adapter.create_document_collection("docs")


def test_insert_document(adapter):
    adapter._client._session.program("post", FakeResp({"id": "d1"}))
    out = adapter.insert_document("docs", {"title": "x"}, id="d1")
    assert out == {"id": "d1"}


def test_insert_document_error(adapter):
    adapter._client._session.program("post", FakeResp(raise_exc=RuntimeError("x")))
    with pytest.raises(RuntimeError):
        adapter.insert_document("docs", {"title": "x"})


def test_get_document(adapter):
    adapter._client._session.program("get", FakeResp({"id": "d1", "title": "x"}))
    out = adapter.get_document("docs", "d1", projection=["title"])
    assert out["id"] == "d1"
    verb, url, kw = adapter._client._session.calls[-1]
    assert kw["params"]["projection"] == "title"


def test_get_document_404(adapter):
    adapter._client._session.program("get", FakeResp({}, status_code=404))
    assert adapter.get_document("docs", "missing") is None


def test_get_document_error(adapter):
    adapter._client._session.program("get", RuntimeError("x"))
    assert adapter.get_document("docs", "d1") is None


def test_query_documents(adapter):
    adapter._client._session.program("post", FakeResp({"documents": [1, 2]}))
    out = adapter.query_documents(
        "docs", filter={"a": 1}, projection=["title"], limit=5
    )
    assert out["documents"] == [1, 2]


def test_query_documents_error(adapter):
    adapter._client._session.program("post", RuntimeError("x"))
    with pytest.raises(RuntimeError):
        adapter.query_documents("docs")


def test_update_document(adapter):
    adapter._client._session.program("put", FakeResp({"updated": True}))
    out = adapter.update_document("docs", "d1", [{"set": {"a": 1}}])
    assert out["updated"] is True


def test_update_document_error(adapter):
    adapter._client._session.program("put", RuntimeError("x"))
    with pytest.raises(RuntimeError):
        adapter.update_document("docs", "d1", [])


def test_delete_document(adapter):
    adapter._client._session.program("delete", FakeResp({"deleted": True}))
    assert adapter.delete_document("docs", "d1") is True


def test_delete_document_error(adapter):
    adapter._client._session.program("delete", RuntimeError("x"))
    assert adapter.delete_document("docs", "d1") is False


def test_list_document_collections(adapter):
    adapter._client._session.program("get", FakeResp({"collections": [{"name": "a"}]}))
    out = adapter.list_document_collections()
    assert out == [{"name": "a"}]


def test_list_document_collections_error(adapter):
    adapter._client._session.program("get", RuntimeError("x"))
    assert adapter.list_document_collections() == []


def test_delete_document_collection(adapter):
    adapter._client._session.program("delete", FakeResp({"success": True}))
    assert adapter.delete_document_collection("docs") is True


def test_delete_document_collection_error(adapter):
    adapter._client._session.program("delete", RuntimeError("x"))
    assert adapter.delete_document_collection("docs") is False


# ---------------------------------------------------------------------------
# Hybrid search
# ---------------------------------------------------------------------------


def test_hybrid_search(adapter):
    adapter._client._session.program("post", FakeResp({"results": [1]}))
    out = adapter.hybrid_search("c1", "find me", [0.1, 0.2], top_k=3)
    assert out["results"] == [1]
    verb, url, kw = adapter._client._session.calls[-1]
    assert "hybrid/search" in url
    assert kw["json"]["fusion_strategy"] == "rrf"


def test_hybrid_search_error(adapter):
    adapter._client._session.program("post", RuntimeError("x"))
    with pytest.raises(RuntimeError):
        adapter.hybrid_search("c1", "q", [0.1])


# ---------------------------------------------------------------------------
# Time-series operations
# ---------------------------------------------------------------------------


def test_create_timeseries_collection(adapter):
    adapter._client._session.program("post", FakeResp({"name": "ts"}))
    out = adapter.create_timeseries_collection("ts", config={"retention": "7d"})
    assert out["name"] == "ts"


def test_create_timeseries_collection_error(adapter):
    adapter._client._session.program("post", RuntimeError("x"))
    with pytest.raises(RuntimeError):
        adapter.create_timeseries_collection("ts")


def test_ingest_timeseries(adapter):
    adapter._client._session.program("post", FakeResp({"ingested": 2}))
    out = adapter.ingest_timeseries("ts", [{"t": 1, "v": 1.0}, {"t": 2, "v": 2.0}])
    assert out["ingested"] == 2


def test_ingest_timeseries_error(adapter):
    adapter._client._session.program("post", RuntimeError("x"))
    with pytest.raises(RuntimeError):
        adapter.ingest_timeseries("ts", [])


def test_query_timeseries_full(adapter):
    adapter._client._session.program("post", FakeResp({"points": [1, 2]}))
    out = adapter.query_timeseries(
        "ts",
        start_time="2024-01-01",
        end_time="2024-01-02",
        aggregation="sum",
        bucket_ms=60000,
        tag_filters={"host": "a"},
    )
    assert out["points"] == [1, 2]
    verb, url, kw = adapter._client._session.calls[-1]
    assert kw["json"]["bucket_ms"] == 60000
    assert kw["json"]["tag_filters"] == {"host": "a"}


def test_query_timeseries_minimal(adapter):
    adapter._client._session.program("post", FakeResp({"points": []}))
    out = adapter.query_timeseries("ts", start_time="a", end_time="b")
    verb, url, kw = adapter._client._session.calls[-1]
    assert "bucket_ms" not in kw["json"]
    assert "tag_filters" not in kw["json"]
    assert out["points"] == []


def test_query_timeseries_error(adapter):
    adapter._client._session.program("post", RuntimeError("x"))
    with pytest.raises(RuntimeError):
        adapter.query_timeseries("ts", "a", "b")


def test_list_timeseries_collections(adapter):
    adapter._client._session.program("get", FakeResp({"collections": ["ts1"]}))
    assert adapter.list_timeseries_collections() == ["ts1"]


def test_list_timeseries_collections_error(adapter):
    adapter._client._session.program("get", RuntimeError("x"))
    assert adapter.list_timeseries_collections() == []


def test_delete_timeseries_collection(adapter):
    adapter._client._session.program("delete", FakeResp({"success": True}))
    assert adapter.delete_timeseries_collection("ts") is True


def test_delete_timeseries_collection_error(adapter):
    adapter._client._session.program("delete", RuntimeError("x"))
    assert adapter.delete_timeseries_collection("ts") is False


# ---------------------------------------------------------------------------
# Lifecycle
# ---------------------------------------------------------------------------


def test_close(adapter):
    adapter.close()
    assert adapter._client.closed is True
    assert adapter.is_connected is False


def test_close_no_close_method(adapter):
    class NoClose:
        pass

    adapter._client = NoClose()
    adapter.close()
    assert adapter.is_connected is False
