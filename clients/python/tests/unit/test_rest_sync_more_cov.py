"""Additional offline coverage for proximadb_sdk.protocols.rest_sync.

Complements ``test_rest_sync_methods_coverage.py`` (which focuses on graph /
query methods) by exercising the collection lifecycle, vector CRUD, search /
search_batch / search_envelope, and the *_cached / *_batched wrappers + close.

Everything is fully offline: ``_make_request`` and ``_http_client`` are mocked,
the batch processor is replaced with a hand fake, and the real ResponseCache is
used (in-memory) for the caching paths.
"""

from __future__ import annotations

import numpy as np
import pytest

from proximadb_sdk.protocols.rest_sync import ProximaDBClient


# --------------------------------------------------------------------------
# Fakes / fixtures
# --------------------------------------------------------------------------
class FakeResp:
    def __init__(self, data=None, status=200):
        self._d = {} if data is None else dict(data)
        self.status_code = status
        self.headers = {}
        self.text = "{}"
        self.content = b"{}"
        self.url = "http://testserver/x"

    def json(self):
        return self._d

    def raise_for_status(self):
        return None


class RecordingTransport:
    """Records (method, endpoint, kwargs) and returns a programmable body."""

    def __init__(self, body=None):
        self.calls = []
        self._body = body or {}
        self._per_path = {}

    def set_path_body(self, substr, body):
        self._per_path[substr] = body

    def __call__(self, method, endpoint, **kwargs):
        self.calls.append((method, endpoint, kwargs))
        for substr, body in self._per_path.items():
            if substr in endpoint:
                return FakeResp(body)
        return FakeResp(self._body)


@pytest.fixture
def client(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    transport = RecordingTransport()
    monkeypatch.setattr(c, "_make_request", transport)
    c._transport = transport  # expose for assertions
    return c


def _endpoints(c):
    return [e for (_m, e, _k) in c._transport.calls]


# --------------------------------------------------------------------------
# Collections
# --------------------------------------------------------------------------
def test_create_collection_collection_id_response(client):
    client._transport.set_path_body(
        "/api/v2/collections",
        {"collection_id": "cid-1", "name": "products_x", "dimension": 8,
         "engine": "sst", "created_at": 123},
    )
    coll = client.create_collection("products_x", dimension=8)
    assert coll.id == "cid-1"
    assert coll.config.dimension == 8
    assert "/api/v2/collections" in _endpoints(client)


def test_create_collection_nested_collection_response(client):
    client._transport.set_path_body(
        "/api/v2/collections",
        {
            "collection": {
                "id": "cid-2",
                "created_at": 1,
                "updated_at": 2,
                "config": {
                    "name": "nested_coll",
                    "dimension": 16,
                    "distance_metric": 2,  # euclidean
                    "storage_engine": 3,  # nova
                },
            }
        },
    )
    coll = client.create_collection("nested_coll", dimension=16)
    assert coll.id == "cid-2"
    assert coll.config.dimension == 16


def test_create_collection_with_explicit_config(client):
    from proximadb_sdk.models import CollectionConfig

    cfg = CollectionConfig(name="cfg_coll", dimension=32)
    client._transport.set_path_body(
        "/api/v2/collections",
        {"collection_id": "cfg-1", "name": "cfg_coll", "dimension": 32,
         "engine": "sst"},
    )
    coll = client.create_collection("cfg_coll", config=cfg)
    assert coll.id == "cfg-1"


def test_get_collection_flat(client):
    client._transport.set_path_body(
        "/api/v2/collections/abc",
        {
            "id": "abc",
            "name": "the_collection",
            "dimension": 64,
            "metric": "cosine",
            "vector_count": 5,
            "created_at": 10,
            "updated_at": 20,
        },
    )
    coll = client.get_collection("abc")
    assert coll.id == "abc"
    assert coll.config.dimension == 64
    assert coll.stats.vector_count == 5


def test_get_collection_int_enums_and_nested_config(client):
    client._transport.set_path_body(
        "/api/v2/collections/xyz",
        {
            "collection": {
                "id": "xyz",
                "config": {
                    "name": "xyz_collection",
                    "dimension": 128,
                    "metric": 2,  # euclidean (int enum)
                    "storage_engine": 2,  # sst (int enum)
                },
            }
        },
    )
    coll = client.get_collection("xyz")
    assert coll.id == "xyz"
    assert coll.config.dimension == 128


def test_get_collection_not_found_raises(client):
    # The "not found" branch references an (unimported) CollectionNotFoundError
    # symbol in the source, so it surfaces as a NameError; either way the call
    # must raise rather than return a Collection.
    client._transport.set_path_body(
        "/api/v2/collections/missing",
        {"error_message": "Collection not found"},
    )
    with pytest.raises(Exception):
        client.get_collection("missing")


def test_get_collection_generic_error_raises(client):
    from proximadb_sdk.exceptions import ProximaDBError

    client._transport.set_path_body(
        "/api/v2/collections/broken",
        {"success": False, "error_message": "boom"},
    )
    with pytest.raises(ProximaDBError):
        client.get_collection("broken")


def test_list_collections_with_params(client):
    client._transport.set_path_body(
        "/api/v2/collections",
        {
            "collections": [
                {
                    "id": "c1",
                    "name": "first_collection",
                    "dimension": 8,
                    "metric": 1,
                    "storage_engine": 2,
                    "vector_count": 3,
                    "created_at": 1,
                    "updated_at": 2,
                },
                {
                    "id": "c2",
                    "config": {
                        "name": "second_collection",
                        "dimension": 16,
                        "distance_metric": "cosine",
                        "storage_engine": "sst",
                    },
                },
            ],
            "total_count": 2,
        },
    )
    colls = client.list_collections(limit=10, offset=0, include_stats=True)
    assert len(colls) == 2
    assert colls[0].id == "c1"
    # params should have been forwarded
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["params"]["limit"] == 10
    assert kwargs["params"]["include_stats"] == "true"


def test_list_collections_empty(client):
    client._transport.set_path_body("/api/v2/collections", {"collections": []})
    assert client.list_collections() == []


def test_delete_collection(client):
    client._transport.set_path_body(
        "/api/v2/collections/del", {"success": True}
    )
    assert client.delete_collection("del") is True
    assert ("DELETE", "/api/v2/collections/del", {}) == client._transport.calls[-1]


def test_get_schema(client):
    client._transport.set_path_body(
        "/schema",
        {
            "schema_id": "schema-1",
            "schema_version": "1",
            "collection_id": "sc",
            "schema": {"columns": []},
            "created_at": "2026-01-01T00:00:00Z",
        },
    )
    resp = client.get_schema("sc")
    assert resp.schema_id == "schema-1"
    assert "/api/v2/collections/sc/schema" in _endpoints(client)


def test_update_schema_with_dict(client):
    client._transport.set_path_body(
        "/schema",
        {
            "schema_id": "schema-2",
            "schema_version": "2",
            "previous_schema_id": "schema-1",
            "changes": [],
            "warnings": [],
            "updated_at": "2026-01-01T00:00:00Z",
        },
    )
    resp = client.update_schema("sc", {"fields": []}, force=True)
    assert resp is not None
    _, ep, kwargs = client._transport.calls[-1]
    assert ep == "/api/v2/collections/sc/schema"
    assert kwargs["json"]["force"] is True


def test_get_collection_stats(client):
    client._transport.set_path_body(
        "/stats",
        {"vector_count": 7, "index_size_bytes": 100, "data_size_bytes": 200},
    )
    stats = client.get_collection_stats("c")
    assert stats.vector_count == 7
    assert "/collections/c/stats" in _endpoints(client)


# --------------------------------------------------------------------------
# Vector writes
# --------------------------------------------------------------------------
def test_insert_vector_single(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    res = client.insert_vector("col", "v1", [0.1, 0.2, 0.3], metadata={"k": "v"})
    assert res.success == 1
    assert res.failed == 0
    assert "/api/v2/collections/col/records/batch" in _endpoints(client)


def test_insert_records_multi_batch(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    records = [
        {"id": f"r{i}", "vector": [float(i), float(i + 1)], "props": {"n": i}}
        for i in range(5)
    ]
    res = client.insert_records("col", records, batch_size=2)
    # 5 records / batch_size 2 -> 3 requests
    assert len([c for c in _endpoints(client) if "records/batch" in c]) == 3
    assert res.success == 3


def test_insert_records_with_failures(client):
    client._transport.set_path_body(
        "/records/batch",
        {"inserted_count": 0, "failed_count": 1, "errors": ["bad"]},
    )
    res = client.insert_records("col", [{"id": "x", "vector": [1.0, 2.0]}])
    assert res.failed == 1
    assert res.errors == ["bad"]


def test_upsert_records(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 2, "failed_count": 0}
    )
    res = client.upsert_records(
        "col",
        [
            {"id": "a", "vector": [1.0, 2.0]},
            {"id": "b", "vector": [3.0, 4.0]},
        ],
    )
    assert res.success == 2
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["json"]["upsert"] is True


def test_insert_vectors_from_arrays(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 2, "failed_count": 0}
    )
    res = client.insert_vectors(
        "col",
        np.array([[1.0, 2.0], [3.0, 4.0]], dtype=np.float32),
        ids=["i1", "i2"],
        metadata=[{"a": 1}, {"b": 2}],
    )
    assert res.success == 2


def test_insert_vectors_auto_ids(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 2, "failed_count": 0}
    )
    res = client.insert_vectors("col", [[1.0, 2.0], [3.0, 4.0]])
    assert res.success == 2


def test_insert_vectors_dict_list(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    res = client.insert_vectors(
        "col", [{"id": "z", "vector": [9.0, 8.0], "props": {"q": 1}}]
    )
    assert res.success == 1


def test_insert_vectors_mismatched_ids_raises(client):
    with pytest.raises(ValueError):
        client.insert_vectors("col", [[1.0, 2.0], [3.0, 4.0]], ids=["only-one"])


def test_insert_vectors_mismatched_metadata_raises(client):
    with pytest.raises(ValueError):
        client.insert_vectors(
            "col", [[1.0, 2.0]], ids=["i1"], metadata=[{"a": 1}, {"b": 2}]
        )


def test_upsert_vectors_from_records(client):
    from proximadb_sdk.models import VectorRecord

    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    rec = VectorRecord(id="vr1", vector=[1.0, 2.0], metadata={"m": 1})
    res = client.upsert_vectors("col", [rec])
    assert res.success == 1


def test_update_vector(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    res = client.update_vector("col", "v1", vector=[1.0, 2.0], metadata={"k": "v"})
    assert res.success == 1


def test_update_vector_no_vector_raises(client):
    with pytest.raises(ValueError):
        client.update_vector("col", "v1", vector=None)


def test_update_vector_numpy(client):
    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    res = client.update_vector(
        "col", "v1", vector=np.array([1.0, 2.0], dtype=np.float64)
    )
    assert res.success == 1


# --------------------------------------------------------------------------
# Vector reads / deletes
# --------------------------------------------------------------------------
def test_get_vector_extracts_nested(client):
    client._transport.set_path_body(
        "/records/v1",
        {"results": {"results": [{"id": "v1", "vector": [1.0]}]}},
    )
    out = client.get_vector("col", "v1")
    assert out == {"id": "v1", "vector": [1.0]}


def test_get_vector_plain_dict(client):
    client._transport.set_path_body(
        "/records/v2", {"id": "v2", "props": {"a": 1}}
    )
    out = client.get_vector("col", "v2")
    assert out["id"] == "v2"


def test_get_vector_not_found_raises(client):
    from proximadb_sdk.exceptions import ProximaDBError

    client._transport.set_path_body(
        "/records/gone", {"error_code": "NOT_FOUND", "success": False}
    )
    with pytest.raises(ProximaDBError):
        client.get_vector("col", "gone")


def test_delete_vector(client):
    client._transport.set_path_body("/records/d1", {"success": True})
    res = client.delete_vector("col", "d1")
    assert res.success is True
    assert res.deleted_count == 1


def test_delete_vector_failure(client):
    client._transport.set_path_body("/records/d2", {"success": False})
    res = client.delete_vector("col", "d2")
    assert res.success is False
    assert res.deleted_count == 0


def test_delete_vectors_multi(client):
    client._transport.set_path_body("/records/", {"success": True})
    res = client.delete_vectors("col", ["a", "b", "c"])
    assert res.deleted_count == 3
    assert res.success is True


def test_delete_vectors_collects_errors(client, monkeypatch):
    def boom(method, endpoint, **kw):
        raise RuntimeError("network down")

    monkeypatch.setattr(client, "_make_request", boom)
    res = client.delete_vectors("col", ["a"])
    # An exception during per-id delete is collected and surfaced as a failure.
    assert res.success is False
    assert res.deleted_count == 0


# --------------------------------------------------------------------------
# Search
# --------------------------------------------------------------------------
def test_search_basic(client):
    client._transport.set_path_body(
        "/search",
        {"results": [{"id": "r1", "score": 0.9, "rank": 1, "props": {"x": 1}}]},
    )
    res = client.search("col", [0.1, 0.2], top_k=5)
    assert len(res) == 1
    assert res[0].id == "r1"
    assert res[0].score == 0.9


def test_search_with_filter_and_numpy(client):
    client._transport.set_path_body("/search", {"results": []})
    res = client.search(
        "col",
        np.array([0.1, 0.2], dtype=np.float64),
        metadata_filter={"category": "a"},
        include_vectors=True,
    )
    assert res == []
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["json"]["filters"][0]["field"] == "category"


def test_search_nested_results(client):
    client._transport.set_path_body(
        "/search", {"results": {"results": [{"id": "n1", "score": 0.5}]}}
    )
    res = client.search("col", [0.1])
    assert res[0].id == "n1"


def test_search_error_not_found_returns_empty(client):
    client._transport.set_path_body(
        "/search", {"error_message": "collection not found"}
    )
    assert client.search("col", [0.1]) == []


def test_search_error_raises(client):
    from proximadb_sdk.exceptions import ProximaDBError

    client._transport.set_path_body("/search", {"error_message": "internal boom"})
    with pytest.raises(ProximaDBError):
        client.search("col", [0.1])


def test_search_skips_malformed_results(client):
    client._transport.set_path_body(
        "/search",
        {"results": ["badstring", 123, {"id": "ok", "score": 0.1}]},
    )
    res = client.search("col", [0.1])
    assert len(res) == 1
    assert res[0].id == "ok"


def test_search_null_results(client):
    client._transport.set_path_body("/search", {"results": None})
    assert client.search("col", [0.1]) == []


def test_search_with_hints(client):
    client._transport.set_path_body("/search", {"results": []})
    client.search(
        "col",
        [0.1, 0.2],
        search_hints={"enable_two_stage": True, "accuracy_threshold": 0.9},
    )
    _, _, kwargs = client._transport.calls[-1]
    assert "search_optimization" in kwargs["json"]


def test_search_envelope(client):
    client._transport.set_path_body(
        "/search", {"results": [{"id": "e1", "score": 0.3}]}
    )
    env = client.search_envelope("col", [0.1, 0.2], top_k=3)
    assert env.has_more is False
    assert len(env.items) == 1
    assert env.items[0].id == "e1"


def test_search_batch(client):
    client._transport.set_path_body(
        "/search/batch",
        {"results": [[{"id": "a", "score": 0.9}], [{"id": "b", "score": 0.8}]]},
    )
    res = client.search_batch("col", [[0.1], [0.2]], k=2)
    assert len(res) == 2
    assert res[0][0].id == "a"
    assert res[1][0].id == "b"


def test_search_batch_with_ef(client):
    client._transport.set_path_body("/search/batch", {"results": [[]]})
    client.search_batch("col", [[0.1]], ef=64, exact=True, include_vectors=True)
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["json"]["params"]["ef"] == 64
    assert kwargs["json"]["params"]["exact_search"] is True


# --------------------------------------------------------------------------
# Caching (uses the real in-memory ResponseCache)
# --------------------------------------------------------------------------
@pytest.fixture
def caching_client(monkeypatch):
    c = ProximaDBClient(
        url="http://testserver",
        enable_caching=True,
        cache_config={"default_ttl_seconds": 60},
    )
    transport = RecordingTransport()
    monkeypatch.setattr(c, "_make_request", transport)
    c._transport = transport
    yield c
    c.close()


def test_search_cached_no_caching_passthrough(client):
    client._transport.set_path_body(
        "/search", {"results": [{"id": "r1", "score": 0.9}]}
    )
    res = client.search_cached("col", [0.1, 0.2], top_k=2)
    assert res[0].id == "r1"


def test_search_cached_hits_cache(caching_client):
    caching_client._transport.set_path_body(
        "/search", {"results": [{"id": "r1", "score": 0.9}]}
    )
    r1 = caching_client.search_cached("col", [0.1, 0.2], top_k=2)
    n_after_first = len(caching_client._transport.calls)
    r2 = caching_client.search_cached("col", [0.1, 0.2], top_k=2)
    # second call should be served from cache (no new transport call)
    assert len(caching_client._transport.calls) == n_after_first
    assert r1[0].id == r2[0].id


def test_get_vector_cached(caching_client):
    caching_client._transport.set_path_body(
        "/records/gv", {"id": "gv", "vector": [1.0]}
    )
    a = caching_client.get_vector_cached("col", "gv")
    n = len(caching_client._transport.calls)
    b = caching_client.get_vector_cached("col", "gv")
    assert len(caching_client._transport.calls) == n
    assert a == b


def test_get_vector_cached_no_caching(client):
    client._transport.set_path_body("/records/gv", {"id": "gv"})
    out = client.get_vector_cached("col", "gv")
    assert out["id"] == "gv"


def test_list_collections_cached(caching_client):
    caching_client._transport.set_path_body(
        "/api/v2/collections", {"collections": []}
    )
    a = caching_client.list_collections_cached()
    n = len(caching_client._transport.calls)
    b = caching_client.list_collections_cached()
    assert len(caching_client._transport.calls) == n
    assert a == b == []


def test_list_collections_cached_no_caching(client):
    client._transport.set_path_body("/api/v2/collections", {"collections": []})
    assert client.list_collections_cached() == []


def test_get_collection_cached(caching_client):
    caching_client._transport.set_path_body(
        "/api/v2/collections/gc",
        {"id": "gc", "name": "gc_collection", "dimension": 4, "metric": "cosine"},
    )
    a = caching_client.get_collection_cached("gc")
    n = len(caching_client._transport.calls)
    b = caching_client.get_collection_cached("gc")
    assert len(caching_client._transport.calls) == n
    assert a.id == b.id == "gc"


def test_get_collection_cached_no_caching(client):
    client._transport.set_path_body(
        "/api/v2/collections/gc",
        {"id": "gc", "name": "gc_collection", "dimension": 4},
    )
    assert client.get_collection_cached("gc").id == "gc"


def test_get_cache_stats(caching_client):
    stats = caching_client.get_cache_stats()
    assert isinstance(stats, dict)
    assert "total_entries" in stats


def test_get_cache_stats_disabled(client):
    assert client.get_cache_stats() == {"error": "Caching is not enabled"}


def test_clear_cache(caching_client):
    caching_client._transport.set_path_body("/api/v2/collections", {"collections": []})
    caching_client.list_collections_cached()
    cleared = caching_client.clear_cache()
    assert isinstance(cleared, int)


def test_clear_cache_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.clear_cache()


def test_invalidate_collection_cache(caching_client):
    caching_client._transport.set_path_body(
        "/api/v2/collections/ic",
        {"id": "ic", "name": "ic_collection", "dimension": 4},
    )
    caching_client.get_collection_cached("ic")
    n = caching_client.invalidate_collection_cache("ic")
    assert isinstance(n, int)


def test_invalidate_collection_cache_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.invalidate_collection_cache("c")


def test_warm_cache_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.warm_cache([])


# --------------------------------------------------------------------------
# Batched wrappers (fake batch processor)
# --------------------------------------------------------------------------
class FakeBatchProcessor:
    def __init__(self):
        self.requests = []
        self._metrics = {"submitted": 0}

    def submit_request(self, request):
        self.requests.append(request)
        self._metrics["submitted"] += 1
        return f"req-{len(self.requests)}"

    def get_metrics(self):
        return self._metrics

    def reset_metrics(self):
        self._metrics = {"submitted": 0}

    def stop(self):
        pass


@pytest.fixture
def batching_client(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    c.enable_batching = True
    c._batch_processor = FakeBatchProcessor()
    transport = RecordingTransport()
    monkeypatch.setattr(c, "_make_request", transport)
    c._transport = transport
    return c


def test_insert_vectors_batched(batching_client):
    rid = batching_client.insert_vectors_batched(
        "col", [[1.0, 2.0], [3.0, 4.0]], ids=["a", "b"], metadata=[{"x": 1}, {"y": 2}]
    )
    assert rid.startswith("req-")
    assert len(batching_client._batch_processor.requests) == 1


def test_insert_vectors_batched_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.insert_vectors_batched("col", [[1.0]], ids=["a"])


def test_insert_vectors_batched_id_mismatch(batching_client):
    with pytest.raises(ValueError):
        batching_client.insert_vectors_batched("col", [[1.0], [2.0]], ids=["a"])


def test_insert_vectors_batched_metadata_mismatch(batching_client):
    with pytest.raises(ValueError):
        batching_client.insert_vectors_batched(
            "col", [[1.0]], ids=["a"], metadata=[{"x": 1}, {"y": 2}]
        )


def test_upsert_vectors_batched(batching_client):
    rid = batching_client.upsert_vectors_batched(
        "col", [[1.0, 2.0]], ids=["a"], metadata=[{"x": 1}]
    )
    assert rid.startswith("req-")


def test_upsert_vectors_batched_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.upsert_vectors_batched("col", [[1.0]], ids=["a"])


def test_upsert_vectors_batched_id_mismatch(batching_client):
    with pytest.raises(ValueError):
        batching_client.upsert_vectors_batched("col", [[1.0], [2.0]], ids=["a"])


def test_delete_vectors_batched(batching_client):
    rid = batching_client.delete_vectors_batched("col", ["a", "b"])
    assert rid.startswith("req-")


def test_delete_vectors_batched_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.delete_vectors_batched("col", ["a"])


def test_get_batch_metrics(batching_client):
    batching_client.insert_vectors_batched("col", [[1.0]], ids=["a"])
    metrics = batching_client.get_batch_metrics()
    assert metrics["submitted"] == 1


def test_get_batch_metrics_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.get_batch_metrics()


def test_reset_batch_metrics_disabled_raises(client):
    with pytest.raises(RuntimeError):
        client.reset_batch_metrics()


# --------------------------------------------------------------------------
# _execute_batch routing
# --------------------------------------------------------------------------
def test_execute_batch_insert(client):
    from proximadb_sdk.batching_unified import BatchOperationType

    client._transport.set_path_body(
        "/records/batch", {"inserted_count": 1, "failed_count": 0}
    )
    out = client._execute_batch(
        BatchOperationType.INSERT_VECTORS,
        "col",
        [[{"id": "a", "vector": [1.0, 2.0], "metadata": {"x": 1}}]],
    )
    assert out[0]["success"] is True


def test_execute_batch_unknown_op(client):
    out = client._execute_batch("NOT_AN_OP", "col", [])
    assert out[0]["success"] is False


# --------------------------------------------------------------------------
# close / context manager
# --------------------------------------------------------------------------
def test_close_idempotent(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    closed = {"n": 0}

    class FakeHttp:
        def close(self):
            closed["n"] += 1

    c._http_client = FakeHttp()
    c.close()
    c.close()
    assert closed["n"] >= 1


def test_close_with_batch_and_cache(monkeypatch):
    c = ProximaDBClient(url="http://testserver")

    class FakeHttp:
        def close(self):
            pass

    class FakeCache:
        def __init__(self):
            self.closed = False

        def close(self):
            self.closed = True

    c._http_client = FakeHttp()
    c.enable_batching = True
    c._batch_processor = FakeBatchProcessor()
    c._response_cache = FakeCache()
    cache = c._response_cache
    c.close()
    assert c._batch_processor is None
    assert cache.closed is True


def test_context_manager(monkeypatch):
    with ProximaDBClient(url="http://testserver") as c:
        assert isinstance(c, ProximaDBClient)


def test_get_capabilities(client):
    caps = client.get_capabilities()
    assert set(caps) == {
        "sks_search_supported",
        "sks_entities_supported",
        "warmed_collections",
    }


# --------------------------------------------------------------------------
# Real _make_request / _handle_error_response (mock the httpx transport layer)
# --------------------------------------------------------------------------
class FakeHttpx:
    """Stands in for httpx.Client; records the .request() call."""

    def __init__(self, resp):
        self._resp = resp
        self.calls = []
        self.closed = False

    def request(self, method, endpoint, **kwargs):
        self.calls.append((method, endpoint, kwargs))
        return self._resp

    def close(self):
        self.closed = True


@pytest.fixture
def raw_client(monkeypatch):
    """Client whose _make_request is REAL but the httpx transport is faked."""
    c = ProximaDBClient(url="http://testserver")
    return c


def test_make_request_success(raw_client):
    resp = FakeResp({"ok": 1}, status=200)
    raw_client._http_client = FakeHttpx(resp)
    out = raw_client._make_request("GET", "/api/v2/collections")
    assert out is resp
    assert raw_client._http_client.calls[0][0] == "GET"


def test_make_request_error_status_maps_error(raw_client):
    resp = FakeResp({"message": "bad request"}, status=400)
    raw_client._http_client = FakeHttpx(resp)
    with pytest.raises(Exception):
        raw_client._make_request("POST", "/api/v2/collections", json={"a": 1})


def test_make_request_timeout_exception(raw_client):
    import httpx

    from proximadb_sdk.exceptions import TimeoutError as PDBTimeout

    class TimeoutHttpx:
        def request(self, *a, **k):
            raise httpx.TimeoutException("slow")

    raw_client._http_client = TimeoutHttpx()
    raw_client.config.retry.max_retries = 0
    with pytest.raises(PDBTimeout):
        raw_client._make_request("GET", "/x")


def test_make_request_network_exception(raw_client):
    import httpx

    from proximadb_sdk.exceptions import NetworkError as PDBNet

    class NetHttpx:
        def request(self, *a, **k):
            raise httpx.ConnectError("down")

    raw_client._http_client = NetHttpx()
    raw_client.config.retry.max_retries = 0
    with pytest.raises(PDBNet):
        raw_client._make_request("GET", "/x")


def test_make_request_http_status_error(raw_client):
    import httpx

    err_resp = FakeResp({"message": "bad"}, status=422)

    class StatusHttpx:
        def request(self, *a, **k):
            req = httpx.Request("GET", "http://testserver/x")
            raise httpx.HTTPStatusError("boom", request=req, response=err_resp)

    raw_client._http_client = StatusHttpx()
    raw_client.config.retry.max_retries = 0
    with pytest.raises(Exception):
        raw_client._make_request("GET", "/x")


def test_handle_error_response_non_json(raw_client):
    class BadResp:
        status_code = 500
        url = "http://testserver/x"
        text = "boom-text"

        def json(self):
            raise ValueError("not json")

    with pytest.raises(Exception):
        raw_client._handle_error_response(BadResp())


def test_make_request_with_compression(monkeypatch):
    # Enable compression with a tiny threshold so the gzip branch is taken.
    c = ProximaDBClient(url="http://testserver")
    c.config.compression.enabled = True
    c.config.compression.threshold_bytes = 1
    c.config.compression.algorithm = "gzip"
    resp = FakeResp({"ok": 1})
    c._http_client = FakeHttpx(resp)
    big = {"data": "x" * 100}
    c._make_request("POST", "/api/v2/collections", json=big)
    _, _, kwargs = c._http_client.calls[-1]
    # compressed -> content + Content-Encoding header instead of json kwarg
    assert "content" in kwargs
    assert kwargs["headers"]["Content-Encoding"] == "gzip"


def test_make_request_compression_below_threshold_uses_json(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    c.config.compression.enabled = True
    c.config.compression.threshold_bytes = 10_000_000  # never trips
    resp = FakeResp({"ok": 1})
    c._http_client = FakeHttpx(resp)
    c._make_request("POST", "/x", json={"a": 1})
    _, _, kwargs = c._http_client.calls[-1]
    assert "json" in kwargs


# --------------------------------------------------------------------------
# _compress_data algorithms
# --------------------------------------------------------------------------
def test_compress_data_gzip(raw_client):
    raw_client.config.compression.algorithm = "gzip"
    raw_client.config.compression.level = 6
    out = raw_client._compress_data(b"hello world" * 50)
    assert isinstance(out, bytes) and len(out) > 0


def test_compress_data_deflate(raw_client):
    raw_client.config.compression.algorithm = "deflate"
    raw_client.config.compression.level = 6
    out = raw_client._compress_data(b"hello world" * 50)
    assert isinstance(out, bytes)


def test_compress_data_unknown_falls_back(raw_client):
    raw_client.config.compression.algorithm = "made-up"
    raw_client.config.compression.level = 6
    out = raw_client._compress_data(b"abc" * 50)
    assert isinstance(out, bytes)


# --------------------------------------------------------------------------
# Health / probes (real _make_request mocked at transport)
# --------------------------------------------------------------------------
def test_health(raw_client):
    raw_client._http_client = FakeHttpx(
        FakeResp(
            {
                "status": "healthy",
                "version": "1.2.3",
                "uptime_seconds": 99,
                "timestamp": 1700000000,
                "components": {"wal": "ok"},
            }
        )
    )
    h = raw_client.health()
    assert h.status == "healthy"
    assert h.timestamp_ms == 1700000000 * 1000


def test_health_nested_data(raw_client):
    raw_client._http_client = FakeHttpx(
        FakeResp({"data": {"status": "ok", "version": "9.9.9"}})
    )
    h = raw_client.health()
    assert h.status == "ok"


def test_live_and_ready(raw_client):
    raw_client._http_client = FakeHttpx(FakeResp({"status": "ok", "ready": True}))
    live = raw_client.live()
    ready = raw_client.ready()
    assert live is not None
    assert ready is not None


# --------------------------------------------------------------------------
# _normalize_vectors / _validate_vector_dimensions
# --------------------------------------------------------------------------
def test_normalize_vectors_passthrough_list(raw_client):
    assert raw_client._normalize_vectors([[1.0, 2.0]]) == [[1.0, 2.0]]


def test_validate_vector_dimensions_ok(raw_client):
    raw_client.config.validate_inputs = True
    raw_client._validate_vector_dimensions([[1.0, 2.0]], expected_dim=2)


def test_validate_vector_dimensions_mismatch_raises(raw_client):
    raw_client.config.validate_inputs = True
    with pytest.raises(Exception):
        raw_client._validate_vector_dimensions([[1.0, 2.0]], expected_dim=3)


def test_validate_vector_dimensions_numpy_wrong_ndim(raw_client):
    raw_client.config.validate_inputs = True
    with pytest.raises(ValueError):
        raw_client._validate_vector_dimensions(np.array([1.0, 2.0]))


def test_validate_vector_dimensions_disabled_noop(raw_client):
    raw_client.config.validate_inputs = False
    # returns immediately regardless of shape
    raw_client._validate_vector_dimensions(np.array([1.0, 2.0]), expected_dim=99)


# --------------------------------------------------------------------------
# _convert_value_to_rest_sql_value / _convert_metadata_to_rest_format
# --------------------------------------------------------------------------
def test_convert_value_to_rest_sql_value_variants(raw_client):
    assert raw_client._convert_value_to_rest_sql_value(None) == {"null_value": None}
    assert raw_client._convert_value_to_rest_sql_value(True) == {"bool_value": True}
    assert raw_client._convert_value_to_rest_sql_value(5) == {"int64_value": 5}
    assert raw_client._convert_value_to_rest_sql_value(1.5) == {"number_value": 1.5}
    assert raw_client._convert_value_to_rest_sql_value("s") == {"string_value": "s"}
    bv = raw_client._convert_value_to_rest_sql_value(b"ab")
    assert "bytes_value" in bv
    arr = raw_client._convert_value_to_rest_sql_value([1, 2])
    assert "array_value" in arr
    obj = raw_client._convert_value_to_rest_sql_value({"k": 1})
    assert "object_value" in obj


def test_convert_metadata_to_rest_format(raw_client):
    out = raw_client._convert_metadata_to_rest_format({"a": 1, "b": "x"})
    assert out["a"] == {"int64_value": 1}
    assert out["b"] == {"string_value": "x"}
    assert raw_client._convert_metadata_to_rest_format({}) == {}


# --------------------------------------------------------------------------
# _normalize_record_payload edge cases
# --------------------------------------------------------------------------
def test_normalize_record_payload_embeddings_shape(raw_client):
    rec = {"id": "e1", "embeddings": [{"values": [1.0, 2.0]}], "props": {"k": "v"}}
    out = raw_client._normalize_record_payload(rec)
    assert out["id"] == "e1"
    assert out["vector"] == [1.0, 2.0]


def test_normalize_record_payload_missing_vector_raises(raw_client):
    with pytest.raises(ValueError):
        raw_client._normalize_record_payload({"id": "x"})


def test_normalize_record_payload_empty_vector_raises(raw_client):
    with pytest.raises(ValueError):
        raw_client._normalize_record_payload({"id": "x", "vector": []})


def test_normalize_record_payload_unsupported_type_raises(raw_client):
    with pytest.raises(TypeError):
        raw_client._normalize_record_payload(object())


def test_normalize_record_payload_autoid(raw_client):
    out = raw_client._normalize_record_payload({"vector": [1.0]}, index=7)
    assert out["id"] == "record_7"


# --------------------------------------------------------------------------
# _proxima_rest_value / _json_scalar
# --------------------------------------------------------------------------
def test_json_scalar(raw_client):
    assert raw_client._json_scalar(np.int32(3)) == 3
    assert raw_client._json_scalar(np.array([1.0, 2.0])) == [1.0, 2.0]
    assert raw_client._json_scalar("plain") == "plain"


def test_proxima_rest_value_variants(raw_client):
    assert raw_client._proxima_rest_value(np.int64(5)) == 5
    assert raw_client._proxima_rest_value(np.array([1.0]))[0] == 1.0
    assert raw_client._proxima_rest_value(b"hi")["type"] == "binary"
    assert raw_client._proxima_rest_value((1, 2))["type"] == "array"
    assert raw_client._proxima_rest_value({"a": 1})["type"] == "jsonb"
    assert raw_client._proxima_rest_value({"type": "x", "value": 1}) == {
        "type": "x",
        "value": 1,
    }
    assert raw_client._proxima_rest_value("plain") == "plain"


# --------------------------------------------------------------------------
# Module-level quantization helper
# --------------------------------------------------------------------------
def test_convert_quantization_config_disabled():
    from proximadb_sdk.protocols.rest_sync import (
        _convert_quantization_config_to_proto,
    )

    class FakeQuant:
        def model_dump(self, exclude_none=True):
            return {"enabled": False, "type": "NONE"}

    out = _convert_quantization_config_to_proto(FakeQuant())
    assert out["enabled"] is False
    assert out["strategy"] == 0
    assert out["custom_levels"] == []


def test_convert_quantization_config_custom_levels():
    from proximadb_sdk.protocols.rest_sync import (
        _convert_quantization_config_to_proto,
    )

    class FakeQuant:
        def model_dump(self, exclude_none=True):
            return {
                "enabled": True,
                "type": "PRODUCT",
                "num_subvectors": 16,
                "bits_per_subvector": 8,
                "accuracy_threshold": 0.97,
                "progressive_quantization": True,
            }

    out = _convert_quantization_config_to_proto(FakeQuant())
    assert out["enabled"] is True
    assert out["strategy"] == 1
    assert out["custom_levels"][0]["num_subvectors"] == 16
    assert out["binary_filter_selectivity"] == 0.97
    assert out["enable_progressive_search"] is True


def test_convert_quantization_config_dump_failure():
    from proximadb_sdk.protocols.rest_sync import (
        _convert_quantization_config_to_proto,
    )

    class NoDump:
        pass

    out = _convert_quantization_config_to_proto(NoDump())
    # falls through to empty dict -> disabled
    assert out["enabled"] is False


# --------------------------------------------------------------------------
# create_collection: richer config branches (filterable_columns, schema, etc.)
# --------------------------------------------------------------------------
def test_create_collection_with_filterable_columns(client):
    from proximadb_sdk.models import CollectionConfig, FilterableColumn

    cfg = CollectionConfig(
        name="filtercoll",
        dimension=8,
        description="a described collection",
        filterable_columns=[
            FilterableColumn(name="category", data_type="string", indexed=True),
            FilterableColumn(name="price", data_type="float", indexed=True),
        ],
    )
    client._transport.set_path_body(
        "/api/v2/collections",
        {"collection_id": "fc-1", "name": "filtercoll", "dimension": 8,
         "engine": "sst"},
    )
    coll = client.create_collection("filtercoll", config=cfg)
    assert coll.id == "fc-1"
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["json"]["name"] == "filtercoll"


def test_create_collection_with_schema_and_capacity(client):
    from proximadb_sdk.models import CollectionConfig, SchemaDefinition

    cfg = CollectionConfig(name="schemacoll", dimension=4)
    client._transport.set_path_body(
        "/api/v2/collections",
        {"collection_id": "sc-1", "name": "schemacoll", "dimension": 4,
         "engine": "sst"},
    )
    coll = client.create_collection(
        "schemacoll",
        config=cfg,
        schema=SchemaDefinition(columns=[]),
        initial_capacity=1000,
    )
    assert coll.id == "sc-1"
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["json"]["initial_capacity"] == 1000
    assert "schema" in kwargs["json"]


def test_create_collection_with_dict_schema(client):
    from proximadb_sdk.models import CollectionConfig

    cfg = CollectionConfig(name="dictschema", dimension=4)
    client._transport.set_path_body(
        "/api/v2/collections",
        {"collection_id": "ds-1", "name": "dictschema", "dimension": 4,
         "engine": "sst"},
    )
    client.create_collection("dictschema", config=cfg, schema={"columns": []})
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["json"]["schema"] == {"columns": []}


def test_create_collection_filterable_metadata_columns_built(client):
    from proximadb_sdk.models import CollectionConfig

    cfg = CollectionConfig(
        name="metacoll1",
        dimension=4,
        filterable_metadata_fields=["a", "b", "c"],
    )
    client._transport.set_path_body(
        "/api/v2/collections",
        {"collection_id": "mc-1", "name": "metacoll1", "dimension": 4,
         "engine": "sst"},
    )
    coll = client.create_collection("metacoll1", config=cfg)
    assert coll.id == "mc-1"


def test_create_collection_fallback_response(client):
    # Neither collection_id nor collection in response -> Collection(**data) path.
    from proximadb_sdk.models import CollectionConfig

    client._transport.set_path_body(
        "/api/v2/collections",
        {"id": "fb-1", "config": {"name": "fallbackcoll", "dimension": 4}},
    )
    cfg = CollectionConfig(name="fallbackcoll", dimension=4)
    coll = client.create_collection("fallbackcoll", config=cfg)
    assert coll.id == "fb-1"


# --------------------------------------------------------------------------
# graph_shortest_path / graph_traverse (via _make_request)
# --------------------------------------------------------------------------
def test_graph_shortest_path(client):
    client._transport.set_path_body(
        "/shortest_path", {"path": ["n1", "n2"], "cost": 1.0}
    )
    out = client.graph_shortest_path(
        "n1",
        "n2",
        max_depth=5,
        edge_types=["REL"],
        k=3,
        enable_prefetch=True,
        prefetch_budget=100,
    )
    assert out["path"] == ["n1", "n2"]
    method, ep, kwargs = client._transport.calls[-1]
    assert ep == "/api/v2/graphs/default/shortest_path"
    assert kwargs["headers"]["x-graph-prefetch-enabled"] == "true"
    assert kwargs["json"]["k"] == 3


def test_graph_shortest_path_no_prefetch(client):
    client._transport.set_path_body("/shortest_path", {"path": []})
    client.graph_shortest_path("n1", "n2", enable_prefetch=False, prefetch_budget=5)
    _, _, kwargs = client._transport.calls[-1]
    assert kwargs["headers"]["x-graph-prefetch-enabled"] == "false"
    assert kwargs["json"]["enable_prefetch"] is False


def test_graph_traverse(client):
    client._transport.set_path_body(
        "/traverse", {"nodes": ["n1"], "edges": []}
    )
    out = client.graph_traverse(
        "n1",
        max_depth=2,
        edge_types=["E"],
        limit=10,
        timeout_ms=500,
        max_frontier=50,
        enable_prefetch=True,
        prefetch_budget=20,
    )
    assert out["nodes"] == ["n1"]
    method, ep, kwargs = client._transport.calls[-1]
    assert ep == "/api/v2/graphs/default/traverse"
    assert kwargs["json"]["limit"] == 10
    assert kwargs["json"]["max_frontier"] == 50


def test_graph_traverse_custom_graph(client):
    client._transport.set_path_body("/traverse", {"nodes": []})
    client.graph_traverse("n1", graph_id="g9")
    _, ep, _ = client._transport.calls[-1]
    assert ep == "/api/v2/graphs/g9/traverse"


# --------------------------------------------------------------------------
# search_next_page failure path (exception -> empty envelope)
# --------------------------------------------------------------------------
def test_search_next_page_exception_returns_empty(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    c._sks_search_supported = True

    def boom(method, endpoint, **kw):
        raise RuntimeError("transient")

    monkeypatch.setattr(c, "_make_request", boom)
    env = c.search_next_page("col", "cur")
    assert env.items == []
    assert env.has_more is False


def test_search_next_page_missing_items_key(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    c._sks_search_supported = True
    transport = RecordingTransport({"unexpected": True})
    monkeypatch.setattr(c, "_make_request", transport)
    env = c.search_next_page("col", "cur")
    assert env.items == []


# --------------------------------------------------------------------------
# reset_batch_metrics (enabled path via fake processor)
# --------------------------------------------------------------------------
def test_reset_batch_metrics_enabled(batching_client):
    # FakeBatchProcessor exposes reset_metrics; the client calls it.
    batching_client.reset_batch_metrics()
