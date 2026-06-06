"""Offline unit tests for the victor / agentic integration wrappers.

Covers:
  * proximadb_sdk.integrations.victor_graph.ProximaDBGraphStore
  * proximadb_sdk.integrations.agentic_store (ProximaBaseStore, ProximaCheckpointSaver)
  * proximadb_sdk.integrations.agentic_io (ProximaEventStore, ProximaMapperSession, ProximaQuery)

All transports are mocked; nothing connects, downloads, or boots a real DB.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from unittest.mock import MagicMock

import pytest

from proximadb_sdk.graph import GraphEdge as SDKGraphEdge
from proximadb_sdk.graph import GraphNode as SDKGraphNode
from proximadb_sdk.integrations import victor_graph as vg
from proximadb_sdk.integrations.victor_graph import (
    ProximaDBGraphStore,
    VictorGraphEdge,
    VictorGraphNode,
)
from proximadb_sdk.integrations import agentic_store as ags
from proximadb_sdk.integrations.agentic_store import (
    ProximaBaseStore,
    ProximaCheckpointSaver,
    StoreItem,
)
from proximadb_sdk.integrations import agentic_io as aio
from proximadb_sdk.integrations.agentic_io import (
    ProximaEventStore,
    ProximaMapperSession,
    ProximaQuery,
)


def run(coro):
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# victor_graph helpers (module-level pure functions)
# ---------------------------------------------------------------------------


def test_pure_helpers():
    assert vg._is_scalar(None) is True
    assert vg._is_scalar("x") is True
    assert vg._is_scalar(3) is True
    assert vg._is_scalar([1, 2]) is False

    assert vg._safe_json_loads("") == {}
    assert vg._safe_json_loads({"a": 1}) == {"a": 1}
    assert vg._safe_json_loads('{"a": 1}') == {"a": 1}
    assert vg._safe_json_loads("not json") == {}
    assert vg._safe_json_loads("[1,2]") == {}  # not a dict

    assert vg._coerce_int(None) is None
    assert vg._coerce_int("") is None
    assert vg._coerce_int("7") == 7
    assert vg._coerce_int("nope") is None

    assert vg._coerce_float(None) is None
    assert vg._coerce_float("") is None
    assert vg._coerce_float("1.5") == 1.5
    assert vg._coerce_float("nope") is None


def make_store():
    """Create a ProximaDBGraphStore with a mock client and mock graph."""
    client = MagicMock()
    store = ProximaDBGraphStore(client=client, graph_id="g1")
    store._graph = MagicMock()
    return store, client, store._graph


def test_node_edge_conversion_roundtrip():
    store, _, _ = make_store()
    vnode = VictorGraphNode(
        node_id="n1",
        type="function",
        name="foo",
        file="a.py",
        line=10,
        end_line=20,
        lang="py",
        metadata={"labels": ["Extra"], "custom": "val", "nested": {"k": 1}},
    )
    sdk = store._sdk_node_from_victor(vnode)
    assert sdk["id"] == "n1"
    assert "function" in sdk["labels"]
    assert "Extra" in sdk["labels"]
    assert sdk["properties"]["custom"] == "val"
    assert "nested" not in sdk["properties"]  # non-scalar dropped

    vedge = VictorGraphEdge(src="a", dst="b", type="CALLS", weight=2.0, metadata={"x": 1})
    sdk_e = store._sdk_edge_from_victor(vedge)
    assert sdk_e["from_node_id"] == "a"
    assert sdk_e["to_node_id"] == "b"
    assert sdk_e["weight"] == 2.0
    assert sdk_e["properties"]["x"] == 1

    # edge with explicit id in metadata and no weight
    vedge2 = VictorGraphEdge(src="a", dst="b", type="T", metadata={"id": "EID"})
    sdk_e2 = store._sdk_edge_from_victor(vedge2)
    assert sdk_e2["id"] == "EID"
    assert "weight" not in sdk_e2


def test_victor_from_sdk():
    store, _, _ = make_store()
    snode = SDKGraphNode(
        id="n1",
        labels=["__FileState", "function"],
        properties={
            "type": "function",
            "name": "foo",
            "file": "a.py",
            "line": "10",
            "end_line": "20",
            "metadata_json": '{"m": 1}',
            "extra": "e",
        },
    )
    v = store._victor_node_from_sdk(snode)
    assert v.node_id == "n1"
    assert v.type == "function"
    assert v.line == 10
    assert v.metadata["m"] == 1
    assert v.metadata["extra"] == "e"

    # node without type prop -> derives from non-internal label
    snode2 = SDKGraphNode(id="n2", labels=["__FileState", "class"], properties={})
    v2 = store._victor_node_from_sdk(snode2)
    assert v2.type == "class"
    assert v2.name == "n2"

    sedge = SDKGraphEdge(
        id="e1", from_node="a", to_node="b", edge_type="CALLS",
        properties={"metadata_json": '{"w": 1}', "rel": "x"}, weight=1.5,
    )
    ve = store._victor_edge_from_sdk(sedge)
    assert ve.src == "a"
    assert ve.dst == "b"
    assert ve.metadata["w"] == 1
    assert ve.metadata["rel"] == "x"
    assert ve.metadata["id"] == "e1"


def test_static_id_helpers():
    store, _, _ = make_store()
    assert store._file_state_node_id("a.py").startswith("__FileState:")
    assert store._subgraph_cache_node_id("sg").startswith("__SubgraphCache:")
    k1 = store._subgraph_cache_key("anchor", 2, ["A", "B"])
    k2 = store._subgraph_cache_key("anchor", 2, ["B", "A"])
    assert k1 == k2  # edge types are sorted


def test_initialize_and_close():
    store, client, _ = make_store()
    run(store.initialize())
    client.create_graph.assert_called_once_with("g1")
    # second call is a no-op
    run(store.initialize())
    assert client.create_graph.call_count == 1
    run(store.close())
    client.close.assert_called_once()

    # create_if_missing raising is swallowed
    store2, client2, _ = make_store()
    client2.create_graph.side_effect = RuntimeError("boom")
    run(store2.initialize())  # no raise


def test_initialize_skips_when_not_create():
    client = MagicMock()
    store = ProximaDBGraphStore(client=client, graph_id="g1", create_if_missing=False)
    store._graph = MagicMock()
    run(store.initialize())
    client.create_graph.assert_not_called()


def test_close_no_callable():
    store = ProximaDBGraphStore(client=object(), graph_id="g1")
    store._graph = MagicMock()
    run(store.close())  # no .close attribute -> no-op


def test_upsert_nodes_and_edges():
    store, client, graph = make_store()
    # one node already exists -> triggers delete
    graph.get_node_by_id.return_value = SDKGraphNode(id="n1", labels=["t"])
    node = VictorGraphNode(node_id="n1", type="t", name="x", file="f")
    run(store.upsert_nodes([node]))
    client.delete_node.assert_called_once()
    graph.batch_create_nodes.assert_called_once()

    # empty list returns early
    graph.batch_create_nodes.reset_mock()
    run(store.upsert_nodes([]))
    graph.batch_create_nodes.assert_not_called()

    # delete raising is swallowed
    client.delete_node.side_effect = RuntimeError("x")
    run(store.upsert_nodes([node]))

    run(store.upsert_edges([VictorGraphEdge(src="a", dst="b", type="T")]))
    graph.batch_create_edges.assert_called_once()
    graph.batch_create_edges.reset_mock()
    run(store.upsert_edges([]))
    graph.batch_create_edges.assert_not_called()


def test_get_neighbors_and_find_nodes():
    store, _, graph = make_store()
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e", from_node="a", to_node="b", edge_type="T")
    ]
    edges = run(store.get_neighbors("a", edge_types=["T"], direction="out", max_depth=2))
    assert edges[0].src == "a"

    graph.find_nodes.return_value = [SDKGraphNode(id="n", labels=["t"])]
    nodes = run(store.find_nodes(name="foo", type="t", file="f"))
    assert nodes[0].node_id == "n"


def test_search_symbols_and_lookups():
    store, _, graph = make_store()
    graph.search_symbols.return_value = [SDKGraphNode(id="s", labels=["fn"])]
    res = run(store.search_symbols("q", limit=5, symbol_types=["fn"]))
    assert res[0].node_id == "s"

    graph.get_node_by_id.return_value = None
    assert run(store.get_node_by_id("missing")) is None
    graph.get_node_by_id.return_value = SDKGraphNode(id="n", labels=["t"])
    assert run(store.get_node_by_id("n")).node_id == "n"

    graph.get_all_nodes.return_value = [SDKGraphNode(id="a", labels=["t"])]
    assert len(run(store.get_all_nodes())) == 1

    graph.get_nodes_by_file.return_value = [SDKGraphNode(id="a", labels=["t"], properties={"file": "f"})]
    assert len(run(store.get_nodes_by_file("f"))) == 1


def test_update_file_mtime_and_stale():
    store, client, graph = make_store()
    graph.get_node_by_id.return_value = None
    run(store.update_file_mtime("a.py", 123.0))
    graph.batch_create_nodes.assert_called_once()

    graph.get_all_nodes.return_value = [
        SDKGraphNode(
            id="fs", labels=["__FileState"],
            properties={"file": "a.py", "metadata_json": '{"mtime": 100.0}'},
        ),
    ]
    stale = run(store.get_stale_files({"a.py": 200.0, "b.py": 5.0}))
    assert "a.py" in stale  # newer mtime
    assert "b.py" in stale  # never indexed


def test_delete_by_file_and_repo():
    store, client, graph = make_store()
    graph.get_nodes_by_file.return_value = [
        SDKGraphNode(id="n1", labels=["t"]),
        SDKGraphNode(id="n2", labels=["t"]),
    ]
    client.delete_node.side_effect = [None, RuntimeError("x"), None]
    run(store.delete_by_file("a.py"))
    assert client.delete_node.call_count == 3

    run(store.delete_by_repo())
    client.delete_graph.assert_called_once_with("g1")
    # recreate attempted (create_if_missing True)
    assert client.create_graph.called


def test_delete_by_repo_recreate_swallows():
    store, client, graph = make_store()
    client.delete_graph.side_effect = RuntimeError("del fail")
    client.create_graph.side_effect = RuntimeError("recreate fail")
    with pytest.raises(RuntimeError):
        run(store.delete_by_repo())


def test_stats():
    store, _, graph = make_store()
    graph.get_stats.return_value = {"data": {"node_count": 5}}
    assert run(store.stats()) == {"node_count": 5}
    graph.get_stats.return_value = {"node_count": 7}
    assert run(store.stats()) == {"node_count": 7}
    graph.get_stats.return_value = "notadict"
    assert run(store.stats()) == {}


def test_get_all_edges_and_by_statement_requirement_scope():
    store, _, graph = make_store()
    graph.get_all_edges.return_value = [
        SDKGraphEdge(id="e", from_node="a", to_node="b", edge_type="T")
    ]
    assert len(run(store.get_all_edges())) == 1

    graph.get_all_nodes.return_value = [
        SDKGraphNode(id="n1", labels=["t"], properties={"statement_type": "if", "file": "a.py"}),
        SDKGraphNode(id="n2", labels=["t"], properties={"statement_type": "if", "file": "b.py"}),
        SDKGraphNode(id="n3", labels=["t"], properties={"statement_type": "for"}),
    ]
    res = run(store.get_nodes_by_statement_type("if", file="a.py"))
    assert len(res) == 1
    res_all = run(store.get_nodes_by_statement_type("if"))
    assert len(res_all) == 2

    graph.get_all_nodes.return_value = [
        SDKGraphNode(id="n1", labels=["t"], properties={"requirement_id": "R1"}),
        SDKGraphNode(id="n2", labels=["t"], properties={"requirement_id": "R2"}),
    ]
    assert len(run(store.get_nodes_by_requirement("R1"))) == 1

    graph.get_all_nodes.return_value = [
        SDKGraphNode(id="n1", labels=["t"], properties={"scope_id": "S1"}),
    ]
    assert len(run(store.get_nodes_by_scope("S1"))) == 1


def test_get_subgraph_compute_and_cache():
    store, _, graph = make_store()
    # no cache node -> compute
    graph.get_node_by_id.return_value = None
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e", from_node="anchor", to_node="b", edge_type="T")
    ]
    sg = run(store.get_subgraph("anchor", radius=1, edge_types=["T"]))
    assert "anchor" in sg.node_ids
    assert "b" in sg.node_ids
    assert sg.node_count == 2
    # cache write happened (upsert_nodes -> batch_create_nodes)
    graph.batch_create_nodes.assert_called()


def test_get_subgraph_from_cache():
    store, _, graph = make_store()
    import json as _json
    cached_payload = {
        "payload": {
            "node_ids": ["anchor", "b"],
            "edges": [{"src": "anchor", "dst": "b", "type": "T", "weight": None, "metadata": {}}],
            "node_count": 2,
            "computed_at": "123",
        }
    }
    cache_node = SDKGraphNode(
        id="cache", labels=["__SubgraphCache"],
        properties={"metadata_json": _json.dumps(cached_payload)},
    )
    graph.get_node_by_id.return_value = cache_node
    sg = run(store.get_subgraph("anchor", radius=2))
    assert sg.node_ids == ["anchor", "b"]
    assert sg.node_count == 2
    # served from cache: no neighbor computation
    graph.get_neighbors.assert_not_called()


def test_invalidate_subgraph():
    store, client, _ = make_store()
    run(store.invalidate_subgraph("sg1"))
    client.delete_node.assert_called_once()
    client.delete_node.side_effect = RuntimeError("x")
    run(store.invalidate_subgraph("sg2"))  # swallowed


def test_multi_hop_traverse():
    store, _, graph = make_store()

    def node_for(nid):
        return SDKGraphNode(id=nid, labels=["t"], properties={"name": nid})

    graph.get_node_by_id.side_effect = lambda nid: (
        None if nid.startswith("__SubgraphCache") else node_for(nid)
    )
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e", from_node="start", to_node="b", edge_type="T")
    ]
    result = run(store.multi_hop_traverse(["start"], max_hops=1, edge_types=["T"], max_nodes=10))
    assert result.query == "multi_hop_traverse"
    node_ids = {n.node_id for n in result.nodes}
    assert "start" in node_ids
    assert "b" in node_ids
    assert len(result.edges) == 1


def test_iter_nodes():
    store, _, graph = make_store()
    graph.find_nodes.return_value = [
        SDKGraphNode(id=f"n{i}", labels=["t"]) for i in range(5)
    ]

    async def collect():
        batches = []
        async for batch in store.iter_nodes(batch_size=2):
            batches.append(batch)
        return batches

    batches = run(collect())
    assert len(batches) == 3
    assert len(batches[0]) == 2


def test_import_graphify_graph():
    store, _, graph = make_store()
    graph.import_graph_json.return_value = {"nodes": 3}
    res = run(store.import_graphify_graph({"nodes": []}))
    assert res == {"nodes": 3}


def test_store_default_url_construction(monkeypatch):
    captured = {}

    class FakeClient:
        def __init__(self, *, url):
            captured["url"] = url

    monkeypatch.setattr(vg, "ProximaDBClient", FakeClient)
    monkeypatch.setattr(vg, "ProximaDBGraph", lambda c, g: MagicMock())
    s = ProximaDBGraphStore()
    assert captured["url"] == "embedded://local"


# ---------------------------------------------------------------------------
# agentic_store
# ---------------------------------------------------------------------------


def test_agentic_store_pure_helpers():
    assert ags._namespace(["a", 1]) == ("a", "1")
    assert ags._namespace_path(("a", "b")) == "a\x1fb"
    assert ags._store_doc_id(("a",), "k") == "a\x1ek"
    assert ags._document_payload(None) == {}
    assert ags._document_payload({"document": {"x": 1}}) == {"x": 1}
    assert ags._document_payload({"x": 1}) == {"x": 1}
    assert ags._document_payload(7) == {}
    assert ags._store_item_from_document(None) is None
    assert ags._store_item_from_document({}) is None
    item = ags._store_item_from_document({"key": "k", "value": {"a": 1}, "namespace": ["n"]})
    assert isinstance(item, StoreItem)

    assert ags._text_for_index({"a": 1}, None) == '{"a": 1}'
    assert ags._text_for_index({"a": 1}, True) == '{"a": 1}'
    assert ags._text_for_index({"a": "x", "b": "y"}, ["a", "b"]) == "x\ny"
    assert ags._matches_filter({"a": 1}, None) is True
    assert ags._matches_filter({"a": 1}, {"a": 1}) is True
    assert ags._matches_filter({"a": 1}, {"a": 2}) is False


def test_checkpoint_keys_helpers():
    with pytest.raises(ValueError):
        ags._checkpoint_keys({"configurable": {}})
    t, ns, cid = ags._checkpoint_keys({"configurable": {"thread_id": "t1"}})
    assert t == "t1" and ns == "" and cid is None
    assert ags._checkpoint_doc_id("t", "ns", "c") == "t\x1fns\x1fc"
    assert ags._write_doc_id("t", "ns", None, "task", 0).endswith("\x1ftask\x1f0")


def test_base_store_setup_put_get_delete():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    embed = lambda texts: [[0.1, 0.2]]
    store = ProximaBaseStore(adapter, embed=embed, dims=2)
    store.put(["a", "b"], "k", {"name": "v"})
    adapter.create_document_collection.assert_called()
    adapter.create_collection.assert_called()
    adapter.insert_document.assert_called()
    # vector inserted via insert_records / insert_vectors fallback
    assert adapter.insert_records.called or adapter.insert_vectors.called

    # idempotent setup
    store.setup()

    # get returns None when missing
    adapter.get_document.return_value = None
    assert store.get(["a"], "missing") is None
    # get returns item
    adapter.get_document.return_value = {
        "key": "k", "value": {"x": 1}, "namespace": ["a"],
        "created_at": 1.0, "updated_at": 2.0,
    }
    got = store.get(["a"], "k")
    assert got.key == "k"

    store.delete(["a"], "k")
    adapter.delete_document.assert_called()


def test_base_store_put_existing_and_no_embed():
    adapter = MagicMock()
    adapter.get_document.return_value = {"created_at": 5.0}
    store = ProximaBaseStore(adapter)  # no embed
    store.put(["a"], "k", {"v": 1}, index=False)
    # existing -> delete then insert
    adapter.delete_document.assert_called()
    adapter.insert_document.assert_called()
    # no embed -> no vector insert
    adapter.insert_records.assert_not_called()


def test_base_store_setup_create_collection_typeerror():
    adapter = MagicMock()
    adapter.create_collection.side_effect = [TypeError("bad sig"), None]
    embed = lambda t: [[0.0, 0.0]]
    store = ProximaBaseStore(adapter, embed=embed, dims=2)
    store.setup()
    assert adapter.create_collection.call_count == 2


def test_base_store_search_vector_path():
    adapter = MagicMock()
    embed = lambda t: [[0.1]]
    store = ProximaBaseStore(adapter, embed=embed, dims=1)

    hit = MagicMock()
    hit.metadata = {"key": "k"}
    hit.score = 0.9
    adapter.search.return_value = [hit]
    adapter.get_document.return_value = {
        "key": "k", "value": {"x": 1}, "namespace": ["a"],
        "created_at": 1.0, "updated_at": 2.0,
    }
    results = store.search(["a"], query="hello", limit=5)
    assert results and results[0].score == 0.9

    # hit without key skipped
    hit2 = MagicMock()
    hit2.metadata = {}
    adapter.search.return_value = [hit2]
    assert store.search(["a"], query="x") == []


def test_base_store_search_document_path():
    adapter = MagicMock()
    store = ProximaBaseStore(adapter)  # no embed -> document path
    adapter.query_documents.return_value = {
        "documents": [
            {"key": "k1", "value": {"a": 1}, "namespace": ["a"], "created_at": 1.0, "updated_at": 5.0},
            {"key": "k2", "value": {"a": 2}, "namespace": ["a"], "created_at": 1.0, "updated_at": 9.0},
        ]
    }
    results = store.search(["a"], filter={"a": 2})
    assert len(results) == 1
    assert results[0].key == "k2"


def test_base_store_list_namespaces():
    adapter = MagicMock()
    store = ProximaBaseStore(adapter)
    adapter.query_documents.return_value = {
        "documents": [
            {"namespace": ["a", "b", "c"]},
            {"namespace": ["a", "x"]},
            {"namespace": ["z"]},
        ]
    }
    res = store.list_namespaces(prefix=["a"])
    assert ("a", "b", "c") in res
    assert ("z",) not in res
    res2 = store.list_namespaces(prefix=["a"], max_depth=1)
    assert ("a",) in res2
    res3 = store.list_namespaces(suffix=["c"])
    assert ("a", "b", "c") in res3


def test_checkpoint_saver_put_get_list_delete():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    saver = ProximaCheckpointSaver(adapter)
    config = {"configurable": {"thread_id": "t1", "checkpoint_ns": ""}}
    next_cfg = saver.put(config, {"id": "c1"}, {"meta": 1})
    assert next_cfg["configurable"]["checkpoint_id"] == "c1"
    adapter.insert_document.assert_called()

    # put with existing -> delete first
    adapter.get_document.return_value = {"id": "x"}
    saver.put(config, {"id": "c2"}, {})
    adapter.delete_document.assert_called()

    # put_writes
    saver.put_writes(
        {"configurable": {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1"}},
        [("ch", "val")],
        task_id="task1",
    )

    # get_tuple
    adapter.query_documents.return_value = {
        "documents": [
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1",
             "config": {}, "checkpoint": {}, "metadata": {}, "created_at": 1.0},
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c2",
             "config": {}, "checkpoint": {}, "metadata": {}, "created_at": 2.0},
        ]
    }
    tup = saver.get_tuple(config)
    assert tup is not None

    # get_tuple with specific checkpoint_id
    cfg_id = {"configurable": {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1"}}
    tup2 = saver.get_tuple(cfg_id)
    assert tup2 is not None

    # no docs
    adapter.query_documents.return_value = {"documents": []}
    assert saver.get_tuple(config) is None

    # list with before + limit
    adapter.query_documents.return_value = {
        "documents": [
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1",
             "config": {}, "checkpoint": {}, "metadata": {}, "created_at": 1.0},
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c2",
             "config": {}, "checkpoint": {}, "metadata": {}, "created_at": 2.0},
        ]
    }
    before = {"configurable": {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c2"}}
    listed = saver.list(config, before=before, limit=10)
    assert all(t.checkpoint == {} for t in listed)

    # delete_thread
    adapter.query_documents.return_value = {"documents": [{"id": "doc1"}]}
    saver.delete_thread("t1")
    adapter.delete_document.assert_called()


def test_checkpoint_saver_async_wrappers():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    adapter.query_documents.return_value = {"documents": []}
    saver = ProximaCheckpointSaver(adapter)
    config = {"configurable": {"thread_id": "t1"}}
    run(saver.aput(config, {"id": "c1"}, {}))
    run(saver.aput_writes(
        {"configurable": {"thread_id": "t1", "checkpoint_id": "c1"}},
        [("ch", "v")], "task",
    ))
    assert run(saver.aget_tuple(config)) is None
    assert run(saver.alist(config)) == []


def test_checkpoint_saver_setup_swallows():
    adapter = MagicMock()
    adapter.create_document_collection.side_effect = RuntimeError("x")
    saver = ProximaCheckpointSaver(adapter)
    saver.setup()
    saver.setup()  # idempotent


# ---------------------------------------------------------------------------
# agentic_io
# ---------------------------------------------------------------------------


@dataclass
class SampleModel:
    id: str
    name: str


def test_io_pure_helpers():
    assert aio._event_doc_id("s", 1, "e").startswith("s\x1f")
    assert aio._documents({"documents": [1, 2]}) == [1, 2]
    assert aio._documents("x") == []
    assert aio._payload(None) == {}
    assert aio._payload({"document": {"a": 1}}) == {"a": 1}
    assert aio._payload({"a": 1}) == {"a": 1}
    assert aio._payload(7) == {}
    assert aio._default_collection_name(SampleModel) == "samplemodels"

    assert aio._model_to_dict({"a": 1}) == {"a": 1}
    assert aio._model_to_dict(SampleModel(id="1", name="n")) == {"id": "1", "name": "n"}

    class PydLike:
        def model_dump(self):
            return {"a": 1}

    assert aio._model_to_dict(PydLike()) == {"a": 1}

    class OldPyd:
        def dict(self):
            return {"b": 2}

    assert aio._model_to_dict(OldPyd()) == {"b": 2}

    with pytest.raises(TypeError):
        aio._model_to_dict(object())

    # _dict_to_model
    assert aio._dict_to_model(dict, {"a": 1}) == {"a": 1}
    m = aio._dict_to_model(SampleModel, {"id": "1", "name": "n", "extra": "ignored"})
    assert m.id == "1"

    class HasValidate:
        @classmethod
        def model_validate(cls, payload):
            return ("validated", payload)

    assert aio._dict_to_model(HasValidate, {"x": 1})[0] == "validated"

    class HasParse:
        @classmethod
        def parse_obj(cls, payload):
            return ("parsed", payload)

    assert aio._dict_to_model(HasParse, {"x": 1})[0] == "parsed"

    class Plain:
        def __init__(self, **kw):
            self.kw = kw

    assert aio._dict_to_model(Plain, {"x": 1}).kw == {"x": 1}


def test_event_store_append_and_read():
    adapter = MagicMock()
    adapter.query_documents.return_value = {"documents": []}
    store = ProximaEventStore(adapter)
    rec = store.append("stream1", "created", {"k": "v"}, expected_version=0)
    assert rec.version == 1
    assert rec.event_type == "created"
    adapter.create_document_collection.assert_called()
    adapter.insert_document.assert_called()


def test_event_store_version_conflict():
    adapter = MagicMock()
    docs = [aio._event_to_document(
        aio.EventRecord("s", 1, "t", {}, {}, "e1", 1, 1.0)
    )]
    adapter.query_documents.return_value = {"documents": docs}
    store = ProximaEventStore(adapter)
    with pytest.raises(ValueError):
        store.append("s", "t", {}, expected_version=0)


def test_event_store_read_stream_and_all():
    adapter = MagicMock()
    docs = [
        aio._event_to_document(aio.EventRecord("s", 1, "t", {}, {}, "e1", 1, 1.0)),
        aio._event_to_document(aio.EventRecord("s", 2, "t", {}, {}, "e2", 2, 2.0)),
    ]
    adapter.query_documents.return_value = {"documents": docs}
    store = ProximaEventStore(adapter)
    events = store.read_stream("s", after_version=1, limit=10)
    assert len(events) == 1
    assert events[0].version == 2

    all_events = store.read_all(after_position=1, limit=10)
    assert len(all_events) == 1
    assert all_events[0].global_position == 2


def test_event_store_snapshot():
    adapter = MagicMock()
    adapter.query_documents.return_value = {"documents": []}
    store = ProximaEventStore(adapter)
    rec = store.snapshot("s", {"state": 1})
    assert rec.event_type == "$snapshot"


def test_mapper_session_register_upsert_get_delete():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    session = ProximaMapperSession(adapter)
    session.register(SampleModel, indexed_paths=["$.name"])
    adapter.create_document_collection.assert_called()

    item = SampleModel(id="1", name="n")
    doc_id = session.upsert(item)
    assert doc_id == "1"
    adapter.insert_document.assert_called()

    # upsert with existing + vector
    adapter.get_document.return_value = {"id": "1"}
    doc_id2 = session.upsert(item, vector=[0.1, 0.2], source="text")
    adapter.delete_document.assert_called()
    assert adapter.insert_records.called or adapter.insert_vectors.called

    # get None and present
    adapter.get_document.return_value = None
    assert session.get(SampleModel, "missing") is None
    adapter.get_document.return_value = {"id": "1", "name": "n"}
    got = session.get(SampleModel, "1")
    assert got.id == "1"

    session.delete(SampleModel, "1")


def test_mapper_session_upsert_unregistered_type():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    session = ProximaMapperSession(adapter)
    # not registered, no collection -> auto register
    doc_id = session.upsert(SampleModel(id="x", name="y"))
    assert doc_id == "x"


def test_mapper_vector_search():
    adapter = MagicMock()
    session = ProximaMapperSession(adapter)
    session.register(SampleModel)

    hit = MagicMock()
    hit.id = "1"
    hit.score = 0.5
    hit.metadata = {"m": 1}
    hit_no_id = MagicMock()
    hit_no_id.id = None
    adapter.search.return_value = [hit, hit_no_id]
    adapter.get_document.return_value = {"id": "1", "name": "n"}
    results = session.vector_search(SampleModel, [0.1, 0.2], top_k=5)
    assert len(results) == 1
    assert results[0].score == 0.5

    # hit whose document missing -> skipped
    adapter.get_document.return_value = None
    assert session.vector_search(SampleModel, [0.1]) == []


def test_mapper_link_both_signatures():
    adapter = MagicMock()
    session = ProximaMapperSession(adapter)
    adapter.create_edge.return_value = {"ok": True}
    res = session.link("a", "REL", "b", properties={"p": 1})
    assert res == {"ok": True}

    # first signature raises TypeError -> fallback
    adapter2 = MagicMock()
    adapter2.create_edge.side_effect = [TypeError("bad"), {"ok2": True}]
    session2 = ProximaMapperSession(adapter2)
    res2 = session2.link("a", "REL", "b")
    assert res2 == {"ok2": True}


def test_proxima_query():
    adapter = MagicMock()
    session = ProximaMapperSession(adapter)
    session.register(SampleModel)
    adapter.query_documents.return_value = {
        "documents": [
            {"id": "1", "name": "a"},
            {"id": "2", "name": "b"},
            {"id": "3", "name": "c"},
        ]
    }
    q = session.query(SampleModel).where(name="a").limit(2).offset(1)
    items = q.all()
    assert len(items) == 2

    # first
    first = session.query(SampleModel).first()
    assert first.id == "1"

    adapter.query_documents.return_value = {"documents": []}
    assert session.query(SampleModel).first() is None
