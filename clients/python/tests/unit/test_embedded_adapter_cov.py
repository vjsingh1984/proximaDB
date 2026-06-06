"""Offline unit tests for proximadb_sdk.adapters.embedded_adapter.

All tests inject a fake/MagicMock as `embedded_db` so the real native ProximaDB
is never booted. No network, no sleeps, no model downloads.
"""

import numpy as np
import pytest

from proximadb_sdk.adapters.embedded_adapter import EmbeddedProtocolAdapter
from proximadb_sdk.models import (
    CollectionConfig,
    DistanceMetric,
    SearchResult,
    VectorOperationResponse,
    VectorRecord,
)


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeNativeDB:
    """A configurable fake native DB. Methods only exist if pre-declared so the
    adapter's `hasattr(...)` branches can be exercised both ways."""

    def __init__(self, **methods):
        for name, fn in methods.items():
            setattr(self, name, fn)


def make_adapter(**methods) -> EmbeddedProtocolAdapter:
    db = FakeNativeDB(**methods)
    return EmbeddedProtocolAdapter(embedded_db=db)


# ---------------------------------------------------------------------------
# Construction & properties
# ---------------------------------------------------------------------------


def test_injected_db_is_connected():
    a = make_adapter()
    assert a.protocol_name == "embedded"
    assert a.is_connected is True


def test_construct_via_embedded_fallback(monkeypatch):
    """When no embedded_db and no native package, falls back to EmbeddedProximaDB."""
    import proximadb_sdk.adapters.embedded_adapter as mod

    captured = {}

    class FakeEmbeddedConfig:
        def __init__(self, **kw):
            captured["config"] = kw

    class FakeEmbeddedProximaDB:
        def __init__(self, config=None):
            captured["db_config"] = config

    import sys
    import types

    fake_embedded = types.ModuleType("proximadb_sdk.embedded")
    fake_embedded.EmbeddedConfig = FakeEmbeddedConfig
    fake_embedded.EmbeddedProximaDB = FakeEmbeddedProximaDB
    monkeypatch.setitem(sys.modules, "proximadb_sdk.embedded", fake_embedded)

    # Force the native package imports to fail so we hit the except ImportError.
    monkeypatch.setitem(sys.modules, "proximadb_embedded", None)
    monkeypatch.setitem(sys.modules, "proximadb", None)

    a = mod.EmbeddedProtocolAdapter(data_dir="/tmp/x", config={"data_dir": "/tmp/x"})
    assert a.is_connected is True
    assert isinstance(a._db, FakeEmbeddedProximaDB)


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


def test_health_not_connected_raises_validation():
    # NOTE: HealthStatus requires version + uptime_seconds which health() never
    # supplies, so the method unconditionally raises ValidationError. We assert
    # the real (buggy) behavior to keep the test honest and offline.
    from pydantic import ValidationError

    a = make_adapter()
    a._connected = False
    a._db = None
    with pytest.raises(ValidationError):
        a.health()


def test_health_ok_raises_validation():
    from pydantic import ValidationError

    a = make_adapter(list_collections=lambda: ["c1", "c2"])
    with pytest.raises(ValidationError):
        a.health()


def test_health_error_branch_raises_validation():
    from pydantic import ValidationError

    def boom():
        raise RuntimeError("nope")

    a = make_adapter(list_collections=boom)
    with pytest.raises(ValidationError):
        a.health()


# ---------------------------------------------------------------------------
# Collections
# ---------------------------------------------------------------------------


def test_create_collection_with_config():
    calls = []
    a = make_adapter(create_collection=lambda *args: calls.append(args))
    cfg = CollectionConfig(
        name="mycollection", dimension=64, distance_metric=DistanceMetric.COSINE
    )
    coll = a.create_collection("mycollection", config=cfg)
    assert coll.id == "mycollection"
    assert coll.dimension == 64
    assert "mycollection" in a._collections


def test_create_collection_typeerror_then_kwargs():
    state = {"positional_tries": 0}

    def create_collection(*args, **kwargs):
        if args:
            state["positional_tries"] += 1
            raise TypeError("positional not supported")
        state["kwargs"] = kwargs

    a = make_adapter(create_collection=create_collection)
    coll = a.create_collection("mycollection", dimension=12, storage_engine="sst")
    assert state["positional_tries"] == 1
    assert state["kwargs"]["name"] == "mycollection"
    assert coll.dimension == 12


def test_create_collection_failure_raises():
    def boom(*a, **k):
        raise ValueError("bad")

    a = make_adapter(create_collection=boom)
    with pytest.raises(ValueError):
        a.create_collection("mycollection", dimension=4)


def test_get_collection_from_cache():
    a = make_adapter(create_collection=lambda *args: None)
    a.create_collection("cachedcoll", dimension=8)
    got = a.get_collection("cachedcoll")
    assert got is not None and got.id == "cachedcoll"


def test_get_collection_via_native():
    class Result:
        name = "nativecoll"
        dimension = 16
        engine = "sst"
        vector_count = 5

    a = make_adapter(get_collection=lambda cid: Result())
    got = a.get_collection("nativecoll")
    assert got.dimension == 16
    assert got.stats.vector_count == 5


def test_get_collection_fallback_to_list():
    class Obj:
        name = "foundcoll"
        dimension = 8
        engine = "sst"
        vector_count = 1

    a = make_adapter(list_collections=lambda: [Obj()])
    # No get_collection method -> goes to list fallback
    got = a.get_collection("foundcoll")
    assert got is not None and got.id == "foundcoll"


def test_get_collection_not_found_returns_none():
    a = make_adapter(list_collections=lambda: [])
    assert a.get_collection("missingcoll") is None


def test_get_collection_exception_returns_none():
    def boom(cid):
        raise RuntimeError("err")

    a = make_adapter(get_collection=boom)
    assert a.get_collection("anything") is None


def test_list_collections_object_shape():
    class Obj:
        name = "objcoll1"
        dimension = 4
        engine = "sst"
        vector_count = 0

    a = make_adapter(list_collections=lambda: [Obj()])
    out = a.list_collections()
    names = {c.id for c in out}
    assert "objcoll1" in names


def test_list_collections_string_items_dimension_zero_swallowed():
    # String-named items build a Collection with dimension=0 which violates the
    # CollectionConfig (ge=1) constraint; the whole call is wrapped in a
    # try/except that returns [] on failure. Assert that real behavior.
    a = make_adapter(list_collections=lambda: ["strcoll1"])
    assert a.list_collections() == []


def test_list_collections_no_method_uses_cache():
    a = make_adapter(create_collection=lambda *args: None)
    a.create_collection("cachedone", dimension=4)
    out = a.list_collections()
    assert any(c.id == "cachedone" for c in out)


def test_list_collections_error_returns_empty():
    def boom():
        raise RuntimeError("x")

    a = make_adapter(list_collections=boom)
    assert a.list_collections() == []


def test_delete_collection_ok():
    deleted = []
    a = make_adapter(delete_collection=lambda cid: deleted.append(cid))
    assert a.delete_collection("somecoll") is True
    assert deleted == ["somecoll"]


def test_delete_collection_error():
    def boom(cid):
        raise RuntimeError("x")

    a = make_adapter(delete_collection=boom)
    assert a.delete_collection("c") is False


# ---------------------------------------------------------------------------
# Normalization helper
# ---------------------------------------------------------------------------


def test_normalize_vector_records_variants():
    a = make_adapter()
    rec = VectorRecord(id="r1", vector=[1.0, 2.0], metadata={"k": "v"})
    ids, vecs, meta = a._normalize_vector_records(
        [rec, {"id": "r2", "vector": [3.0, 4.0]}]
    )
    assert ids == ["r1", "r2"]
    assert vecs == [[1.0, 2.0], [3.0, 4.0]]
    assert meta == [{"k": "v"}, {}]


def test_normalize_vector_records_no_metadata():
    a = make_adapter()
    ids, vecs, meta = a._normalize_vector_records([{"vector": [1.0]}])
    assert ids == ["vec_0"]
    assert meta is None


# ---------------------------------------------------------------------------
# numpy insert / upsert
# ---------------------------------------------------------------------------


def test_insert_numpy_via_insert_numpy_method():
    a = make_adapter(insert_numpy=lambda *args: 2)
    resp = a.insert_numpy("c", ["a", "b"], [[1.0], [2.0]])
    assert resp.success is True
    assert resp.metrics.successful_count == 2


def test_insert_numpy_fallback_to_insert():
    a = make_adapter(insert=lambda *args: 1)
    resp = a.insert_numpy("c", ["a", "b"], [[1.0], [2.0]])
    assert resp.metrics.successful_count == 1
    assert resp.metrics.failed_count == 1


def test_upsert_numpy_via_upsert_numpy():
    a = make_adapter(upsert_numpy=lambda *args: 3)
    resp = a.upsert_numpy("c", ["a", "b", "c"], [[1.0], [2.0], [3.0]])
    assert resp.operation == "UPSERT"
    assert resp.metrics.successful_count == 3


def test_upsert_numpy_fallback_to_upsert():
    a = make_adapter(upsert=lambda *args: 1)
    resp = a.upsert_numpy("c", ["a"], [[1.0]])
    assert resp.metrics.successful_count == 1


def test_numpy_batch_bad_shape_raises():
    # The ndim check runs before the try/except, so a 1D array raises ValueError.
    a = make_adapter(insert_numpy=lambda *args: 1)
    with pytest.raises(ValueError, match="2D vector array"):
        a.insert_numpy("c", ["a"], [1.0, 2.0, 3.0])


def test_numpy_batch_db_exception():
    def boom(*args):
        raise RuntimeError("disk full")

    a = make_adapter(insert_numpy=boom)
    resp = a.insert_numpy("c", ["a"], [[1.0]])
    assert resp.success is False
    assert "disk full" in resp.error_message


# ---------------------------------------------------------------------------
# Record-native insert/upsert + aliases
# ---------------------------------------------------------------------------


def test_insert_records_native_method():
    a = make_adapter(insert_records=lambda cid, recs, **kw: 2)
    res = a.insert_records("c", [{"id": "a"}, {"id": "b"}])
    assert res.success == 2
    assert res.failed == 0


def test_insert_records_native_exception():
    def boom(cid, recs, **kw):
        raise RuntimeError("oops")

    a = make_adapter(insert_records=boom)
    res = a.insert_records("c", [{"id": "a"}])
    assert res.success == 0
    assert res.failed == 1
    assert res.errors == ["oops"]


def test_insert_records_fallback_numpy():
    a = make_adapter(insert_numpy=lambda *args: 1)
    res = a.insert_records("c", [{"id": "a", "vector": [1.0]}])
    assert res.total == 1
    assert res.success == 1


def test_upsert_records_native_tuple_result():
    a = make_adapter(upsert_records=lambda cid, recs, **kw: (2, 0))
    res = a.upsert_records("c", [{"id": "a"}, {"id": "b"}])
    assert res.success == 2


def test_upsert_records_native_exception():
    def boom(cid, recs, **kw):
        raise RuntimeError("bad")

    a = make_adapter(upsert_records=boom)
    res = a.upsert_records("c", [{"id": "a"}])
    assert res.failed == 1


def test_upsert_records_fallback_numpy():
    a = make_adapter(upsert_numpy=lambda *args: 1)
    res = a.upsert_records("c", [{"id": "a", "vector": [1.0]}])
    assert res.success == 1


def test_insert_vectors_alias():
    a = make_adapter(insert_numpy=lambda *args: 1)
    resp = a.insert_vectors("c", [{"id": "a", "vector": [1.0]}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "INSERT"


def test_upsert_vectors_alias():
    a = make_adapter(upsert_numpy=lambda *args: 1)
    resp = a.upsert_vectors("c", [{"id": "a", "vector": [1.0]}])
    assert resp.operation == "UPSERT"


# ---------------------------------------------------------------------------
# get / delete / update metadata
# ---------------------------------------------------------------------------


def test_get_vectors_via_get_vectors():
    class Rec:
        id = "v1"
        vector = [1.0, 2.0]
        metadata = {"k": "v"}

    a = make_adapter(get_vectors=lambda cid, ids: [Rec(), {"id": "v2", "vector": [3.0]}])
    out = a.get_vectors("c", ["v1", "v2"])
    assert {r.id for r in out} == {"v1", "v2"}


def test_get_vectors_via_get_vector_singular():
    class Rec:
        id = "v1"
        vector = [1.0]
        metadata = {}

    a = make_adapter(get_vector=lambda cid, vid: Rec())
    out = a.get_vectors("c", ["v1"], include_vectors=True)
    assert out[0].id == "v1"


def test_get_vectors_not_implemented():
    a = make_adapter()
    assert a.get_vectors("c", ["x"]) == []


def test_get_vectors_exception():
    def boom(cid, ids):
        raise RuntimeError("err")

    a = make_adapter(get_vectors=boom)
    assert a.get_vectors("c", ["x"]) == []


def test_delete_vectors_int_result():
    a = make_adapter(delete_vectors=lambda cid, ids: 2)
    resp = a.delete_vectors("c", ["a", "b", "c"])
    assert resp.success is True
    assert resp.metrics.successful_count == 2
    assert resp.metrics.failed_count == 1


def test_delete_vectors_singular():
    a = make_adapter(delete_vector=lambda cid, vid: True)
    resp = a.delete_vectors("c", ["a"])
    assert resp.metrics.successful_count == 1


def test_delete_vectors_non_int_result():
    # A non-int (e.g. list) result falls through to the "assume all succeeded"
    # branch. (bool is an int subclass, so we use a list here.)
    a = make_adapter(delete_vectors=lambda cid, ids: ["a", "b"])
    resp = a.delete_vectors("c", ["a", "b"])
    assert resp.metrics.successful_count == 2


def test_delete_vectors_not_implemented():
    # The not-implemented branch first tries to build a response without the
    # required `metrics`, which raises ValidationError; that is then caught by
    # the surrounding except and converted into a failed DELETE response (with
    # metrics) reporting all ids failed.
    a = make_adapter()
    resp = a.delete_vectors("c", ["a"])
    assert resp.success is False
    assert resp.metrics.failed_count == 1


def test_delete_vectors_exception():
    def boom(cid, ids):
        raise RuntimeError("x")

    a = make_adapter(delete_vectors=boom)
    resp = a.delete_vectors("c", ["a"])
    assert resp.success is False


def test_update_vector_metadata_native():
    a = make_adapter(update_metadata=lambda cid, vid, md: True)
    resp = a.update_vector_metadata("c", "v1", {"k": "v"})
    assert resp.success is True
    assert resp.operation == "UPDATE"


def test_update_vector_metadata_fallback_get_upsert():
    class Rec:
        id = "v1"
        vector = [1.0]
        metadata = {"old": 1}

    a = make_adapter(
        get_vectors=lambda cid, ids: [Rec()],
        upsert_numpy=lambda *args: 1,
    )
    resp = a.update_vector_metadata("c", "v1", {"new": 2})
    assert resp.success  # success may be an int from the upsert path


def test_update_vector_metadata_fallback_not_found():
    a = make_adapter(get_vectors=lambda cid, ids: [])
    resp = a.update_vector_metadata("c", "v1", {"k": "v"})
    assert resp.success is False
    assert "not found" in resp.error_message


def test_update_vector_metadata_exception():
    def boom(cid, vid, md):
        raise RuntimeError("x")

    a = make_adapter(update_metadata=boom)
    resp = a.update_vector_metadata("c", "v1", {"k": "v"})
    assert resp.success is False


# ---------------------------------------------------------------------------
# Search
# ---------------------------------------------------------------------------


def test_search_numpy_with_filter():
    captured = {}

    def search_numpy(cid, arr, top_k, filter):
        captured["filter"] = filter
        return [("id1", 0.9, {"m": 1}, [1.0])]

    a = make_adapter(search_numpy=search_numpy)
    out = a.search(
        "c",
        np.asarray([1.0, 2.0], dtype=np.float32),
        top_k=5,
        filter={"k": "v"},
        include_vectors=True,
        include_metadata=True,
    )
    assert captured["filter"] == "k = 'v'"
    assert out[0].id == "id1"
    assert out[0].vector == [1.0]


def test_search_list_query_no_numpy():
    a = make_adapter(search=lambda cid, q, top_k, filter: [("id1", 0.5)])
    out = a.search("c", [1.0, 2.0])
    assert out[0].id == "id1"
    assert out[0].score == 0.5


def test_search_exception_returns_empty():
    def boom(*a, **k):
        raise RuntimeError("x")

    a = make_adapter(search_numpy=boom)
    assert a.search("c", [1.0]) == []


def test_batch_search_native():
    a = make_adapter(
        batch_search=lambda *a, **k: [[("id1", 0.9)], [("id2", 0.8)]]
    )
    out = a.batch_search("c", [[1.0], [2.0]])
    assert len(out) == 2
    assert out[0][0].id == "id1"


def test_batch_search_fallback_individual():
    a = make_adapter(search=lambda cid, q, top_k, filter: [("idx", 0.1)])
    out = a.batch_search("c", [np.asarray([1.0], dtype=np.float32), [2.0]])
    assert len(out) == 2
    assert out[0][0].id == "idx"


def test_batch_search_exception_returns_empty_lists():
    def boom(*a, **k):
        raise RuntimeError("x")

    a = make_adapter(batch_search=boom)
    out = a.batch_search("c", [[1.0], [2.0]])
    assert out == [[], []]


def test_to_search_results_all_shapes():
    a = make_adapter()

    class Obj:
        id = "o1"
        score = 0.7
        vector = [1.0]
        metadata = {"x": 1}

    sr_existing = SearchResult(id="pre", score=1.0)
    results = a._to_search_results(
        [
            sr_existing,
            ("t1", 0.9, {"m": 1}, [2.0]),
            {"id": "d1", "score": 0.8, "vector": [3.0], "metadata": {"y": 2}},
            Obj(),
        ],
        include_vectors=True,
        include_metadata=True,
    )
    ids = [r.id for r in results]
    assert ids == ["pre", "t1", "d1", "o1"]


def test_to_search_results_none():
    a = make_adapter()
    assert a._to_search_results(None, False, False) == []


def test_to_search_results_dict_distance_alias():
    a = make_adapter()
    out = a._to_search_results(
        [{"vector_id": "v", "distance": 0.3}], False, False
    )
    assert out[0].id == "v"
    assert out[0].score == 0.3


# ---------------------------------------------------------------------------
# Graph operations
# ---------------------------------------------------------------------------


def test_create_graph_positional():
    a = make_adapter(create_graph=lambda gid, engine: None)
    out = a.create_graph("g1", engine="cedar")
    assert out == {"success": True, "graph_id": "g1"}


def test_create_graph_typeerror_kwargs():
    state = {"tries": 0}

    def create_graph(*args, **kwargs):
        if args:
            state["tries"] += 1
            raise TypeError("no positional")

    a = make_adapter(create_graph=create_graph)
    out = a.create_graph("g1")
    assert state["tries"] == 1
    assert out["success"] is True


def test_create_graph_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.create_graph("g1")


def test_delete_graph():
    a = make_adapter(delete_graph=lambda gid: None)
    assert a.delete_graph("g1")["success"] is True


def test_delete_graph_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.delete_graph("g1")


def test_query_nodes():
    a = make_adapter(
        query_nodes=lambda **kw: [
            {"id": "n1", "labels": ["L"], "properties": {"p": 1}},
            None,
        ]
    )
    out = a.query_nodes(graph="g", labels=["L"])
    assert out["total_count"] == 2
    assert out["nodes"][0]["id"] == "n1"
    assert out["nodes"][1] is None


def test_query_nodes_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.query_nodes()


def test_traverse_graph_dict_result():
    a = make_adapter(traverse_graph=lambda **kw: {"nodes": [1], "edges": []})
    out = a.traverse_graph("start", graph="g")
    assert out["nodes"] == [1]


def test_traverse_graph_non_dict_result():
    a = make_adapter(traverse_graph=lambda **kw: "weird")
    out = a.traverse_graph("start")
    assert out == {"nodes": [], "edges": []}


def test_traverse_graph_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.traverse_graph("start")


def test_create_node():
    a = make_adapter(create_node=lambda **kw: "ok")
    out = a.create_node("n1", ["L"], {"p": 1}, graph="g")
    assert out["node_id"] == "n1"
    assert out["result"] == "ok"


def test_create_node_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.create_node("n1", [], {})


def test_create_edge():
    captured = {}
    a = make_adapter(create_edge=lambda **kw: captured.update(kw) or "ok")
    out = a.create_edge("e1", "REL", from_node="a", to_node="b", properties={"w": 1})
    assert out["edge_id"] == "e1"
    assert captured["from_node_id"] == "a"
    assert captured["to_node_id"] == "b"


def test_create_edge_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.create_edge("e1", "REL")


def test_get_node():
    class Node:
        id = "n1"
        labels = ["L"]
        properties = {"p": 1}

    a = make_adapter(get_node=lambda **kw: Node())
    out = a.get_node("n1", graph="g")
    assert out["id"] == "n1"
    assert out["labels"] == ["L"]


def test_get_node_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.get_node("n1")


def test_get_outgoing_edges():
    a = make_adapter(
        get_outgoing_edges=lambda **kw: [
            {"id": "e1", "from_node": "a", "to_node": "b", "edge_type": "R"}
        ]
    )
    out = a.get_outgoing_edges("a", graph="g")
    assert out[0]["from_node_id"] == "a"
    assert out[0]["to_node_id"] == "b"


def test_get_outgoing_edges_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.get_outgoing_edges("a")


def test_get_incoming_edges_object_edge():
    class Edge:
        id = "e1"
        from_node_id = "a"
        to_node_id = "b"
        edge_type = "R"
        weight = 2
        properties = {"x": 1}

    a = make_adapter(get_incoming_edges=lambda **kw: [Edge()])
    out = a.get_incoming_edges("b")
    assert out[0]["id"] == "e1"
    assert out[0]["weight"] == 2


def test_get_incoming_edges_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.get_incoming_edges("b")


def test_delete_node():
    a = make_adapter(delete_node=lambda **kw: True)
    assert a.delete_node("n1", graph="g") is True


def test_delete_node_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.delete_node("n1")


def test_get_graph_stats():
    class Stats:
        total_nodes = 3
        total_edges = 5

    a = make_adapter(graph_stats=lambda gid: Stats())
    out = a.get_graph_stats("g")
    assert out == {"total_nodes": 3, "total_edges": 5}


def test_get_graph_stats_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.get_graph_stats("g")


def test_execute_graph_query_direct():
    a = make_adapter(execute_graph_query=lambda graph, query: ["row"])
    out = a.execute_graph_query("g", "MATCH (n)")
    assert out["results"] == ["row"]
    assert out["query"] == "MATCH (n)"


def test_execute_graph_query_multimodal_fallback_raises_importerror():
    # The fallback imports MultiModalQuery/QueryComponent from ..models, which
    # no longer exist -> ImportError propagates out of the method.
    a = make_adapter(execute_multi_modal_query=lambda mm: ["mmrow"])
    with pytest.raises(ImportError):
        a.execute_graph_query("g", "MATCH (n)")


def test_execute_graph_query_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.execute_graph_query("g", "q")


# ---------------------------------------------------------------------------
# Documents
# ---------------------------------------------------------------------------


def test_create_document_collection_native():
    a = make_adapter(
        create_document_collection=lambda name, paths: "ok"
    )
    out = a.create_document_collection("docs", config={"indexed_paths": ["a"]})
    assert out["success"] is True
    assert out["collection_id"] == "docs"


def test_create_document_collection_fallback_raises():
    # Fallback calls self.create_collection(name, config={"dimension": ...}); the
    # dict config hits `config.dimension` -> AttributeError -> re-raised.
    a = make_adapter(create_collection=lambda *args: None)
    with pytest.raises(AttributeError):
        a.create_document_collection("docsfallback", config={"dimension": 32})


def test_create_document_collection_error():
    def boom(name, paths):
        raise RuntimeError("x")

    a = make_adapter(create_document_collection=boom)
    with pytest.raises(RuntimeError):
        a.create_document_collection("docs")


def test_insert_document_native():
    a = make_adapter(insert_document=lambda cn, doc, did: ("d1", 2))
    out = a.insert_document("docs", {"k": "v"}, id="d1")
    assert out == {"id": "d1", "success": True, "version": 2}


def test_insert_document_fallback_vector():
    a = make_adapter(insert_numpy=lambda *args: 1)
    out = a.insert_document("docs", {"k": "v"}, id="d1")
    assert out["implementation"] == "vector_fallback"
    assert out["id"] == "d1"


def test_insert_document_error():
    def boom(cn, doc, did):
        raise RuntimeError("x")

    a = make_adapter(insert_document=boom)
    with pytest.raises(RuntimeError):
        a.insert_document("docs", {})


def test_get_document_native():
    a = make_adapter(get_document=lambda cn, did: {"id": did, "document": {"x": 1}})
    out = a.get_document("docs", "d1")
    assert out["id"] == "d1"


def test_get_document_fallback_json_source():
    # _get_document_as_vector calls get_vectors(..., include_vectors=False); the
    # native get_vectors returns dict rows which become VectorRecord(**row).
    rec = {"id": "d1", "vector": [0.0], "metadata": {"m": 1}, "source": '{"a": 1}'}
    a = make_adapter(get_vectors=lambda cid, ids: [rec])
    out = a.get_document("docs", "d1")
    assert out["document"] == {"a": 1}


def test_get_document_fallback_no_vectors():
    a = make_adapter(get_vectors=lambda cid, ids, **k: [])
    assert a.get_document("docs", "d1") is None


def test_get_document_fallback_bad_json_source():
    rec = {"id": "d1", "vector": [0.0], "metadata": {}, "source": "not-json"}
    a = make_adapter(get_vectors=lambda cid, ids: [rec])
    out = a.get_document("docs", "d1")
    assert out["document"] == {"source": "not-json"}


def test_get_document_native_exception_returns_none():
    def boom(cn, did):
        raise RuntimeError("x")

    a = make_adapter(get_document=boom)
    assert a.get_document("docs", "d1") is None


def test_query_documents_native():
    a = make_adapter(
        query_documents=lambda cn, fexpr, lim: [("d1", {"x": 1}), ("d2", {"y": 2})]
    )
    out = a.query_documents("docs", filter={"k": "v"}, limit=10)
    assert out["count"] == 2
    assert out["documents"][0]["id"] == "d1"


def test_query_documents_fallback():
    a = make_adapter()
    out = a.query_documents("docs")
    assert out["implementation"] == "vector_fallback"
    assert out["count"] == 0


def test_query_documents_error():
    def boom(cn, fexpr, lim):
        raise RuntimeError("x")

    a = make_adapter(query_documents=boom)
    with pytest.raises(RuntimeError):
        a.query_documents("docs", filter={"k": "v"})


def test_update_document_native():
    captured = {}
    a = make_adapter(
        update_document=lambda cn, did, m: captured.update({"m": m})
    )
    out = a.update_document(
        "docs", "d1", [{"path": "a.b", "value": 5}, {"value": "nopath"}]
    )
    assert out["success"] is True
    assert captured["m"] == {"a.b": 5}


def test_update_document_fallback():
    rec = {"id": "d1", "vector": [0.0], "metadata": {}, "source": '{"a": {"b": 1}}'}
    a = make_adapter(
        get_vectors=lambda cid, ids: [rec],
        insert_numpy=lambda *args: 1,
    )
    out = a.update_document(
        "docs", "d1", [{"path": "$.a.c", "value": 9, "operation": "SET"}]
    )
    assert out["implementation"] == "vector_fallback"


def test_update_document_fallback_not_found():
    a = make_adapter(get_vectors=lambda cid, ids, **k: [])
    out = a.update_document("docs", "d1", [{"path": "a", "value": 1}])
    assert out["success"] is False


def test_update_document_error():
    def boom(cn, did, m):
        raise RuntimeError("x")

    a = make_adapter(update_document=boom)
    with pytest.raises(RuntimeError):
        a.update_document("docs", "d1", [])


def test_delete_document_native():
    a = make_adapter(delete_document=lambda cn, did: True)
    assert a.delete_document("docs", "d1") is True


def test_delete_document_fallback():
    a = make_adapter(delete_vectors=lambda cid, ids: 1)
    assert a.delete_document("docs", "d1") is True


def test_delete_document_error():
    def boom(cn, did):
        raise RuntimeError("x")

    a = make_adapter(delete_document=boom)
    assert a.delete_document("docs", "d1") is False


def test_list_document_collections_native():
    a = make_adapter(list_document_collections=lambda: [{"name": "d"}])
    assert a.list_document_collections() == [{"name": "d"}]


def test_list_document_collections_native_non_list():
    a = make_adapter(list_document_collections=lambda: "weird")
    assert a.list_document_collections() == []


def test_list_document_collections_fallback():
    class Obj:
        name = "thiscoll"
        dimension = 8
        engine = "sst"
        vector_count = 0

    a = make_adapter(list_collections=lambda: [Obj()])
    out = a.list_document_collections()
    assert out[0]["name"] == "thiscoll"


def test_list_document_collections_error():
    def boom():
        raise RuntimeError("x")

    a = make_adapter(list_document_collections=boom)
    assert a.list_document_collections() == []


def test_delete_document_collection_native():
    a = make_adapter(delete_document_collection=lambda cn: True)
    assert a.delete_document_collection("docs") is True


def test_delete_document_collection_fallback():
    a = make_adapter(delete_collection=lambda cid: None)
    assert a.delete_document_collection("docs") is True


def test_delete_document_collection_error():
    def boom(cn):
        raise RuntimeError("x")

    a = make_adapter(delete_document_collection=boom)
    assert a.delete_document_collection("docs") is False


# ---------------------------------------------------------------------------
# Hybrid search
# ---------------------------------------------------------------------------


def test_hybrid_search_native():
    a = make_adapter(hybrid_search=lambda **kw: {"results": ["r"]})
    out = a.hybrid_search("c", "text", [1.0], top_k=3)
    assert out["results"] == ["r"]


def test_hybrid_search_fallback():
    a = make_adapter(search=lambda cid, q, top_k, filter: [("id1", 0.9, {"m": 1})])
    out = a.hybrid_search("c", "text", [1.0])
    assert out["fusion_strategy"] == "vector_only"
    assert out["results"][0]["id"] == "id1"


def test_hybrid_search_error():
    def boom(**kw):
        raise RuntimeError("x")

    a = make_adapter(hybrid_search=boom)
    with pytest.raises(RuntimeError):
        a.hybrid_search("c", "text", [1.0])


# ---------------------------------------------------------------------------
# Time-series
# ---------------------------------------------------------------------------


def test_create_timeseries_collection_native():
    a = make_adapter(create_timeseries_collection=lambda name, config: "ok")
    out = a.create_timeseries_collection("ts", config={"x": 1})
    assert out["success"] is True


def test_create_timeseries_collection_fallback_raises():
    # Fallback also passes a dict config into create_collection -> AttributeError.
    a = make_adapter(create_collection=lambda *args: None)
    with pytest.raises(AttributeError):
        a.create_timeseries_collection("tsfallback", config={"dimension": 64})


def test_create_timeseries_collection_error():
    def boom(name, config):
        raise RuntimeError("x")

    a = make_adapter(create_timeseries_collection=boom)
    with pytest.raises(RuntimeError):
        a.create_timeseries_collection("ts")


def test_ingest_timeseries_native():
    a = make_adapter(ingest_timeseries=lambda **kw: {"ingested_count": 2})
    out = a.ingest_timeseries("ts", [{"timestamp": "t1"}])
    assert out["ingested_count"] == 2


def test_ingest_timeseries_fallback_raises():
    # The fallback builds a VectorRecord whose metadata holds nested dict/list
    # values (tags, metric_names), which violates the flat metadata schema ->
    # ValidationError -> re-raised by ingest_timeseries.
    from pydantic import ValidationError

    a = make_adapter(insert_numpy=lambda *args: 1)
    with pytest.raises(ValidationError):
        a.ingest_timeseries(
            "ts", [{"timestamp": "t1", "values": {"v": 1}, "tags": {"a": "b"}}]
        )


def test_ingest_timeseries_error():
    def boom(**kw):
        raise RuntimeError("x")

    a = make_adapter(ingest_timeseries=boom)
    with pytest.raises(RuntimeError):
        a.ingest_timeseries("ts", [])


def test_query_timeseries_native():
    a = make_adapter(query_timeseries=lambda **kw: {"raw_points": [1]})
    out = a.query_timeseries("ts", "t0", "t9")
    assert out["raw_points"] == [1]


def _ts_rec(timestamp, source, **meta):
    # VectorRecord metadata must be flat (str/int/float/bool), so timestamp is
    # the only metadata key we can carry; no nested tags in storage.
    md = {"timestamp": timestamp}
    md.update(meta)
    return {"id": f"ts_{timestamp}", "vector": [0.0], "metadata": md, "source": source}


def test_query_timeseries_fallback():
    a = make_adapter(
        get_vectors=lambda cid, ids: [_ts_rec("t5", '{"values": {"v": 1}}')]
    )
    out = a.query_timeseries("ts", "t0", "t9")
    assert out["total_points"] == 1
    assert out["raw_points"][0]["values"] == {"v": 1}


def test_query_timeseries_fallback_tag_filtered_out():
    a = make_adapter(get_vectors=lambda cid, ids: [_ts_rec("t5", "bad-json")])
    # Storage has no tags, so a tag filter excludes the point.
    out = a.query_timeseries("ts", "t0", "t9", tag_filters={"a": "b"})
    assert out["total_points"] == 0


def test_query_timeseries_fallback_time_excluded():
    a = make_adapter(get_vectors=lambda cid, ids: [_ts_rec("t9", None)])
    out = a.query_timeseries("ts", "t0", "t5")
    assert out["total_points"] == 0


def test_query_timeseries_error():
    def boom(**kw):
        raise RuntimeError("x")

    a = make_adapter(query_timeseries=boom)
    with pytest.raises(RuntimeError):
        a.query_timeseries("ts", "t0", "t9")


def test_list_timeseries_collections_native():
    a = make_adapter(list_timeseries_collections=lambda: [{"name": "ts"}])
    assert a.list_timeseries_collections() == [{"name": "ts"}]


def test_list_timeseries_collections_native_non_list():
    a = make_adapter(list_timeseries_collections=lambda: None)
    assert a.list_timeseries_collections() == []


def test_list_timeseries_collections_fallback():
    class Obj:
        name = "tscoll12"
        dimension = 8
        engine = "sst"
        vector_count = 0

    a = make_adapter(list_collections=lambda: [Obj()])
    out = a.list_timeseries_collections()
    assert out[0]["name"] == "tscoll12"


def test_list_timeseries_collections_error():
    def boom():
        raise RuntimeError("x")

    a = make_adapter(list_timeseries_collections=boom)
    assert a.list_timeseries_collections() == []


def test_delete_timeseries_collection_native_dict():
    a = make_adapter(delete_timeseries_collection=lambda collection: {"success": True})
    assert a.delete_timeseries_collection("ts") is True


def test_delete_timeseries_collection_native_bool():
    a = make_adapter(delete_timeseries_collection=lambda collection: True)
    assert a.delete_timeseries_collection("ts") is True


def test_delete_timeseries_collection_fallback():
    a = make_adapter(delete_collection=lambda cid: None)
    assert a.delete_timeseries_collection("ts") is True


def test_delete_timeseries_collection_error():
    def boom(collection):
        raise RuntimeError("x")

    a = make_adapter(delete_timeseries_collection=boom)
    assert a.delete_timeseries_collection("ts") is False


# ---------------------------------------------------------------------------
# SQL / unified query / observability
# ---------------------------------------------------------------------------


def test_execute_sql_native():
    a = make_adapter(execute_sql=lambda q, p, c: {"rows": []})
    out = a.execute_sql("SELECT 1")
    assert out == {"rows": []}


def test_execute_sql_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.execute_sql("SELECT 1")


def test_execute_unified_query_native():
    a = make_adapter(execute_unified_query=lambda q, qv, fs: [{"r": 1}])
    out = a.execute_unified_query("q", query_vector=[1.0], fusion_strategy="rrf")
    assert out == [{"r": 1}]


def test_execute_unified_query_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.execute_unified_query("q")


def test_create_observability_namespace_native():
    a = make_adapter(create_observability_namespace=lambda name, days: None)
    out = a.create_observability_namespace("ns", retention_days=7)
    assert out == {"success": True, "namespace": "ns"}


def test_create_observability_namespace_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.create_observability_namespace("ns")


def test_ingest_logs():
    a = make_adapter(ingest_logs=lambda ns, logs: 3)
    assert a.ingest_logs("ns", [{"l": 1}]) == 3


def test_ingest_logs_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.ingest_logs("ns", [])


def test_query_logs():
    a = make_adapter(query_logs=lambda *args: [{"line": "x"}])
    assert a.query_logs("ns", 0, 1) == [{"line": "x"}]


def test_query_logs_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.query_logs("ns", 0, 1)


def test_ingest_metrics():
    a = make_adapter(ingest_metrics=lambda ns, s: 5)
    assert a.ingest_metrics("ns", [{}]) == 5


def test_ingest_metrics_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.ingest_metrics("ns", [])


def test_aggregate_metrics():
    a = make_adapter(aggregate_metrics=lambda *args: [{"v": 1}])
    assert a.aggregate_metrics("ns", "cpu") == [{"v": 1}]


def test_aggregate_metrics_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.aggregate_metrics("ns", "cpu")


def test_ingest_traces():
    a = make_adapter(ingest_traces=lambda ns, t: 2)
    assert a.ingest_traces("ns", [{}]) == 2


def test_ingest_traces_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.ingest_traces("ns", [])


def test_query_traces():
    a = make_adapter(query_traces=lambda *args: [{"t": 1}])
    assert a.query_traces("ns", 0, 1) == [{"t": 1}]


def test_query_traces_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.query_traces("ns", 0, 1)


def test_get_trace():
    a = make_adapter(get_trace=lambda ns, tid: {"trace_id": tid})
    assert a.get_trace("ns", "t1") == {"trace_id": "t1"}


def test_get_trace_not_implemented():
    a = make_adapter()
    with pytest.raises(NotImplementedError):
        a.get_trace("ns", "t1")


# ---------------------------------------------------------------------------
# Lifecycle
# ---------------------------------------------------------------------------


def test_close_via_close():
    closed = []
    a = make_adapter(close=lambda: closed.append(True))
    a.close()
    assert closed == [True]
    assert a._db is None
    assert a._connected is False


def test_close_via_shutdown():
    shut = []
    a = make_adapter(shutdown=lambda: shut.append(True))
    a.close()
    assert shut == [True]
    assert a._db is None


def test_close_swallows_exception():
    def boom():
        raise RuntimeError("x")

    a = make_adapter(close=boom)
    a.close()  # should not raise
    assert a._db is None


def test_close_when_already_none():
    a = make_adapter()
    a._db = None
    a.close()  # no-op, should not raise
