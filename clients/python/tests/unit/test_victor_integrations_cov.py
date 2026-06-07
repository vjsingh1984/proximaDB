"""Offline unit tests for the victor / agentic integration wrappers.

Covers:
- proximadb_sdk.integrations.victor_graph.ProximaDBGraphStore
- proximadb_sdk.integrations.agentic_store (ProximaBaseStore, ProximaCheckpointSaver)
- proximadb_sdk.integrations.agentic_io (ProximaEventStore, ProximaMapperSession,
  ProximaQuery, helpers)

All transports are mocked. Nothing connects to a server or boots a DB.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from proximadb_sdk.graph import GraphEdge as SDKGraphEdge
from proximadb_sdk.graph import GraphNode as SDKGraphNode
from proximadb_sdk.integrations import agentic_io as aio
from proximadb_sdk.integrations import agentic_store as ast_mod
from proximadb_sdk.integrations import victor_graph as vg
from proximadb_sdk.integrations.victor_graph import (
    ProximaDBGraphStore,
    VictorGraphEdge,
    VictorGraphNode,
)


def run(coro):
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# victor_graph helpers
# ---------------------------------------------------------------------------


def test_is_scalar():
    assert vg._is_scalar(None)
    assert vg._is_scalar("x")
    assert vg._is_scalar(3)
    assert vg._is_scalar(3.0)
    assert vg._is_scalar(True)
    assert not vg._is_scalar([1, 2])
    assert not vg._is_scalar({"a": 1})


def test_safe_json_loads():
    assert vg._safe_json_loads(None) == {}
    assert vg._safe_json_loads("") == {}
    assert vg._safe_json_loads({"a": 1}) == {"a": 1}
    assert vg._safe_json_loads('{"a": 1}') == {"a": 1}
    assert vg._safe_json_loads("not json") == {}
    assert vg._safe_json_loads("[1,2,3]") == {}  # list -> {}


def test_coerce_int():
    assert vg._coerce_int(None) is None
    assert vg._coerce_int("") is None
    assert vg._coerce_int("12") == 12
    assert vg._coerce_int(7) == 7
    assert vg._coerce_int("abc") is None


def test_coerce_float():
    assert vg._coerce_float(None) is None
    assert vg._coerce_float("") is None
    assert vg._coerce_float("1.5") == 1.5
    assert vg._coerce_float("nope") is None


def test_static_id_helpers():
    fid = ProximaDBGraphStore._file_state_node_id("a/b.py")
    assert fid.startswith(vg._FILE_STATE_LABEL + ":")
    sid = ProximaDBGraphStore._subgraph_cache_node_id("xyz")
    assert sid == f"{vg._SUBGRAPH_CACHE_LABEL}:xyz"
    k1 = ProximaDBGraphStore._subgraph_cache_key("n1", 2, ["CALLS", "USES"])
    k2 = ProximaDBGraphStore._subgraph_cache_key("n1", 2, ["USES", "CALLS"])
    assert k1 == k2  # sorted edge types -> stable key
    assert k1 != ProximaDBGraphStore._subgraph_cache_key("n1", 3, None)


def test_sdk_node_from_victor():
    node = VictorGraphNode(
        node_id="n1",
        type="function",
        name="foo",
        file="a.py",
        line=10,
        end_line=20,
        lang="python",
        metadata={"labels": ["extra1", "extra2"], "scalar": 5, "complex": {"x": 1}},
    )
    out = ProximaDBGraphStore._sdk_node_from_victor(node)
    assert out["id"] == "n1"
    assert "function" in out["labels"]
    assert "extra1" in out["labels"] and "extra2" in out["labels"]
    props = out["properties"]
    assert props["name"] == "foo"
    assert props["scalar"] == 5  # scalar metadata promoted
    assert "complex" not in props  # non-scalar metadata not promoted
    # None-valued core props dropped
    assert "docstring" not in props


def test_sdk_edge_from_victor_with_explicit_id_and_weight():
    edge = VictorGraphEdge(
        src="a",
        dst="b",
        type="CALLS",
        weight=0.5,
        metadata={"id": "edge-1", "scalar": "v", "obj": {"k": 1}},
    )
    out = ProximaDBGraphStore._sdk_edge_from_victor(edge)
    assert out["id"] == "edge-1"
    assert out["from_node_id"] == "a"
    assert out["to_node_id"] == "b"
    assert out["edge_type"] == "CALLS"
    assert out["weight"] == 0.5
    assert out["properties"]["scalar"] == "v"
    assert "obj" not in out["properties"]


def test_sdk_edge_from_victor_derived_id_no_weight():
    edge = VictorGraphEdge(src="a", dst="b", type="USES")
    out = ProximaDBGraphStore._sdk_edge_from_victor(edge)
    assert out["id"] == "USES:a:b"
    assert "weight" not in out


def test_victor_node_from_sdk_full():
    node = SDKGraphNode(
        id="n1",
        labels=["function", vg._FILE_STATE_LABEL],
        properties={
            "type": "function",
            "name": "foo",
            "file_path": "a.py",
            "line_start": "3",
            "line_end": "9",
            "lang": "python",
            "metadata_json": '{"k": "v"}',
            "loose": "extra",
        },
    )
    out = ProximaDBGraphStore._victor_node_from_sdk(node)
    assert out.node_id == "n1"
    assert out.type == "function"
    assert out.name == "foo"
    assert out.file == "a.py"
    assert out.line == 3
    assert out.end_line == 9
    assert out.metadata["k"] == "v"
    assert out.metadata["loose"] == "extra"  # non-core prop folded into metadata


def test_victor_node_from_sdk_type_from_label():
    node = SDKGraphNode(
        id="n2",
        labels=[vg._FILE_STATE_LABEL, "class"],
        properties={},
    )
    out = ProximaDBGraphStore._victor_node_from_sdk(node)
    # internal label skipped, "class" chosen
    assert out.type == "class"
    assert out.name == "n2"  # falls back to node id
    assert out.file == ""


def test_victor_node_from_sdk_default_type():
    node = SDKGraphNode(id="n3", labels=[], properties={})
    out = ProximaDBGraphStore._victor_node_from_sdk(node)
    assert out.type == "node"


def test_victor_edge_from_sdk():
    edge = SDKGraphEdge(
        id="e1",
        from_node="a",
        to_node="b",
        edge_type="CALLS",
        weight=1.0,
        properties={"metadata_json": '{"x": 1}', "loose": "y"},
    )
    out = ProximaDBGraphStore._victor_edge_from_sdk(edge)
    assert out.src == "a"
    assert out.dst == "b"
    assert out.type == "CALLS"
    assert out.weight == 1.0
    assert out.metadata["x"] == 1
    assert out.metadata["loose"] == "y"
    assert out.metadata["id"] == "e1"


# ---------------------------------------------------------------------------
# victor_graph ProximaDBGraphStore behaviour (graph layer mocked)
# ---------------------------------------------------------------------------


def make_store(create_if_missing=True):
    client = MagicMock()
    store = ProximaDBGraphStore(
        client=client, graph_id="g1", create_if_missing=create_if_missing
    )
    graph = MagicMock()
    store._graph = graph
    return store, client, graph


def test_init_default_client(monkeypatch):
    sentinel = object()
    created = {}

    def fake_client(url=None):
        created["url"] = url
        return sentinel

    monkeypatch.setattr(vg, "ProximaDBClient", fake_client)
    monkeypatch.setattr(vg, "ProximaDBGraph", lambda c, gid: SimpleNamespace())
    store = ProximaDBGraphStore()
    assert store._client is sentinel
    assert created["url"] == "embedded://local"


def test_initialize_creates_graph_and_is_idempotent():
    store, client, _ = make_store()
    run(store.initialize())
    client.create_graph.assert_called_once_with("g1")
    # second call is a no-op
    run(store.initialize())
    client.create_graph.assert_called_once()


def test_initialize_swallows_create_error():
    store, client, _ = make_store()
    client.create_graph.side_effect = RuntimeError("exists")
    run(store.initialize())
    assert store._initialized


def test_initialize_skips_create_when_disabled():
    store, client, _ = make_store(create_if_missing=False)
    run(store.initialize())
    client.create_graph.assert_not_called()


def test_close_calls_client_close():
    store, client, _ = make_store()
    run(store.close())
    client.close.assert_called_once()


def test_close_no_close_method():
    client = SimpleNamespace()  # no close attr
    store = ProximaDBGraphStore(client=client, graph_id="g1")
    store._graph = MagicMock()
    run(store.close())  # should not raise


def test_upsert_nodes_empty():
    store, client, graph = make_store()
    run(store.upsert_nodes([]))
    graph.batch_create_nodes.assert_not_called()


def test_upsert_nodes_deletes_existing():
    store, client, graph = make_store()
    graph.get_node_by_id.side_effect = [SDKGraphNode(id="n1"), None]
    nodes = [
        VictorGraphNode(node_id="n1", type="t", name="a", file="f"),
        VictorGraphNode(node_id="n2", type="t", name="b", file="f"),
    ]
    run(store.upsert_nodes(nodes))
    client.delete_node.assert_called_once_with(node_id="n1", graph_id="g1")
    graph.batch_create_nodes.assert_called_once()
    assert len(graph.batch_create_nodes.call_args[0][0]) == 2


def test_upsert_nodes_delete_error_swallowed():
    store, client, graph = make_store()
    graph.get_node_by_id.return_value = SDKGraphNode(id="n1")
    client.delete_node.side_effect = RuntimeError("boom")
    run(store.upsert_nodes([VictorGraphNode(node_id="n1", type="t", name="a", file="f")]))
    graph.batch_create_nodes.assert_called_once()


def test_upsert_edges_empty_and_nonempty():
    store, client, graph = make_store()
    run(store.upsert_edges([]))
    graph.batch_create_edges.assert_not_called()
    run(store.upsert_edges([VictorGraphEdge(src="a", dst="b", type="CALLS")]))
    graph.batch_create_edges.assert_called_once()


def test_get_neighbors():
    store, client, graph = make_store()
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e1", from_node="a", to_node="b", edge_type="CALLS")
    ]
    edges = run(store.get_neighbors("a", edge_types=["CALLS"], direction="out", max_depth=2))
    assert len(edges) == 1
    assert edges[0].src == "a"
    graph.get_neighbors.assert_called_once()


def test_find_nodes():
    store, client, graph = make_store()
    graph.find_nodes.return_value = [SDKGraphNode(id="n1", properties={"name": "x"})]
    out = run(store.find_nodes(name="x"))
    assert out[0].node_id == "n1"


def test_search_symbols():
    store, client, graph = make_store()
    graph.search_symbols.return_value = [SDKGraphNode(id="n1")]
    out = run(store.search_symbols("foo", limit=5, symbol_types=["function"]))
    assert out[0].node_id == "n1"


def test_get_node_by_id_found_and_missing():
    store, client, graph = make_store()
    graph.get_node_by_id.return_value = SDKGraphNode(id="n1")
    assert run(store.get_node_by_id("n1")).node_id == "n1"
    graph.get_node_by_id.return_value = None
    assert run(store.get_node_by_id("nope")) is None


def test_get_all_nodes():
    store, client, graph = make_store()
    graph.get_all_nodes.return_value = [SDKGraphNode(id="n1")]
    out = run(store.get_all_nodes())
    assert out[0].node_id == "n1"
    graph.get_all_nodes.assert_called_with(include_internal=False)


def test_get_nodes_by_file():
    store, client, graph = make_store()
    graph.get_nodes_by_file.return_value = [SDKGraphNode(id="n1")]
    out = run(store.get_nodes_by_file("a.py"))
    assert out[0].node_id == "n1"


def test_update_file_mtime():
    store, client, graph = make_store()
    graph.get_node_by_id.return_value = None
    run(store.update_file_mtime("a.py", 123.0))
    graph.batch_create_nodes.assert_called_once()


def test_get_stale_files():
    store, client, graph = make_store()
    graph.get_all_nodes.return_value = [
        SDKGraphNode(
            id="s1",
            properties={"file": "a.py", "metadata_json": '{"mtime": 100.0}'},
        ),
        SDKGraphNode(
            id="s2",
            properties={"file": "b.py", "mtime": "50.0"},
        ),
    ]
    stale = run(
        store.get_stale_files({"a.py": 200.0, "b.py": 40.0, "c.py": 1.0})
    )
    assert "a.py" in stale  # newer mtime
    assert "b.py" not in stale  # older mtime
    assert "c.py" in stale  # unknown file


def test_delete_by_file():
    store, client, graph = make_store()
    graph.get_nodes_by_file.return_value = [
        SDKGraphNode(id="n1"),
        SDKGraphNode(id="n2"),
    ]
    run(store.delete_by_file("a.py"))
    # n1, n2, plus the file-state node
    assert client.delete_node.call_count == 3


def test_delete_by_file_error_swallowed():
    store, client, graph = make_store()
    graph.get_nodes_by_file.return_value = [SDKGraphNode(id="n1")]
    client.delete_node.side_effect = RuntimeError("boom")
    run(store.delete_by_file("a.py"))  # should not raise


def test_delete_by_repo_recreates():
    store, client, graph = make_store()
    run(store.delete_by_repo())
    client.delete_graph.assert_called_once_with("g1")
    # create_graph called twice: once in initialize, once after delete
    assert client.create_graph.call_count == 2


def test_delete_by_repo_no_recreate():
    store, client, graph = make_store(create_if_missing=False)
    run(store.delete_by_repo())
    client.delete_graph.assert_called_once_with("g1")


def test_stats_dict_with_data_key():
    store, client, graph = make_store()
    graph.get_stats.return_value = {"data": {"nodes": 3}}
    assert run(store.stats()) == {"nodes": 3}


def test_stats_non_dict():
    store, client, graph = make_store()
    graph.get_stats.return_value = "weird"
    assert run(store.stats()) == {}


def test_get_all_edges():
    store, client, graph = make_store()
    graph.get_all_edges.return_value = [
        SDKGraphEdge(id="e1", from_node="a", to_node="b", edge_type="CALLS")
    ]
    out = run(store.get_all_edges())
    assert out[0].type == "CALLS"


def test_get_nodes_by_statement_type():
    store, client, graph = make_store()
    graph.get_all_nodes.return_value = [
        SDKGraphNode(id="n1", properties={"statement_type": "if", "file": "a.py"}),
        SDKGraphNode(id="n2", properties={"statement_type": "for", "file": "a.py"}),
        SDKGraphNode(id="n3", properties={"statement_type": "if", "file": "b.py"}),
    ]
    out = run(store.get_nodes_by_statement_type("if", file="a.py"))
    assert [n.node_id for n in out] == ["n1"]
    out2 = run(store.get_nodes_by_statement_type("if"))
    assert {n.node_id for n in out2} == {"n1", "n3"}


def test_get_nodes_by_requirement():
    store, client, graph = make_store()
    graph.get_all_nodes.return_value = [
        SDKGraphNode(id="n1", properties={"requirement_id": "R1"}),
        SDKGraphNode(id="n2", properties={"requirement_id": "R2"}),
    ]
    out = run(store.get_nodes_by_requirement("R1"))
    assert [n.node_id for n in out] == ["n1"]


def test_get_nodes_by_scope():
    store, client, graph = make_store()
    graph.get_all_nodes.return_value = [
        SDKGraphNode(id="n1", properties={"scope_id": "S1"}),
        SDKGraphNode(id="n2", properties={"scope_id": "S2"}),
    ]
    out = run(store.get_nodes_by_scope("S1"))
    assert [n.node_id for n in out] == ["n1"]


def test_get_subgraph_cache_miss_then_caches():
    store, client, graph = make_store()
    # cache lookup returns None (miss)
    graph.get_node_by_id.return_value = None
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e1", from_node="anchor", to_node="b", edge_type="CALLS"),
        SDKGraphEdge(id="e2", from_node="b", to_node="c", edge_type="USES"),
    ]
    sg = run(store.get_subgraph("anchor", radius=2, edge_types=["CALLS"]))
    assert sg.anchor_node_id == "anchor"
    assert set(sg.node_ids) == {"anchor", "b", "c"}
    assert sg.node_count == 3
    # caching upserts a subgraph cache node
    graph.batch_create_nodes.assert_called()


def test_get_subgraph_cache_hit():
    import json

    store, client, graph = make_store()
    cached_payload = {
        "payload": {
            "node_ids": ["anchor", "b"],
            "edges": [
                {
                    "src": "anchor",
                    "dst": "b",
                    "type": "CALLS",
                    "weight": None,
                    "metadata": {},
                }
            ],
            "node_count": 2,
            "computed_at": "123.0",
        }
    }
    cache_node = SDKGraphNode(
        id="cache",
        properties={"metadata_json": json.dumps(cached_payload)},
    )
    graph.get_node_by_id.return_value = cache_node
    sg = run(store.get_subgraph("anchor", radius=2, edge_types=["CALLS"]))
    assert sg.node_ids == ["anchor", "b"]
    assert sg.node_count == 2
    assert sg.edges[0].type == "CALLS"
    # cache hit -> no neighbor traversal
    graph.get_neighbors.assert_not_called()


def test_invalidate_subgraph():
    store, client, graph = make_store()
    run(store.invalidate_subgraph("sg1"))
    client.delete_node.assert_called_once()


def test_invalidate_subgraph_error_swallowed():
    store, client, graph = make_store()
    client.delete_node.side_effect = RuntimeError("boom")
    run(store.invalidate_subgraph("sg1"))  # no raise


def test_multi_hop_traverse():
    store, client, graph = make_store()
    graph.get_node_by_id.side_effect = lambda nid: SDKGraphNode(id=nid)
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e1", from_node="start", to_node="b", edge_type="CALLS"),
    ]
    result = run(store.multi_hop_traverse(["start"], max_hops=1, edge_types=["CALLS"]))
    assert result.query == "multi_hop_traverse"
    assert any(n.node_id == "start" for n in result.nodes)
    assert len(result.edges) == 1


def test_multi_hop_traverse_max_nodes_cap():
    store, client, graph = make_store()
    graph.get_node_by_id.side_effect = lambda nid: SDKGraphNode(id=nid)
    graph.get_neighbors.return_value = [
        SDKGraphEdge(id="e1", from_node="start", to_node="b", edge_type="CALLS"),
        SDKGraphEdge(id="e2", from_node="b", to_node="c", edge_type="CALLS"),
    ]
    result = run(store.multi_hop_traverse(["start"], max_hops=1, max_nodes=1))
    assert len(result.nodes) <= 1


def test_iter_nodes():
    store, client, graph = make_store()
    graph.find_nodes.return_value = [SDKGraphNode(id=f"n{i}") for i in range(5)]

    async def collect():
        batches = []
        async for batch in store.iter_nodes(batch_size=2):
            batches.append(batch)
        return batches

    batches = run(collect())
    assert [len(b) for b in batches] == [2, 2, 1]


def test_import_graphify_graph():
    store, client, graph = make_store()
    graph.import_graph_json.return_value = {"imported": 4}
    out = run(store.import_graphify_graph({"nodes": [], "edges": []}))
    assert out == {"imported": 4}


# ---------------------------------------------------------------------------
# agentic_store helpers
# ---------------------------------------------------------------------------


def test_namespace_helpers():
    ns = ast_mod._namespace(["a", "b"])
    assert ns == ("a", "b")
    assert ast_mod._namespace_path(ns) == "a\x1fb"
    assert ast_mod._store_doc_id(ns, "k").endswith("\x1ek")


def test_document_payload():
    assert ast_mod._document_payload(None) == {}
    assert ast_mod._document_payload({"a": 1}) == {"a": 1}
    assert ast_mod._document_payload({"document": {"x": 1}}) == {"x": 1}
    assert ast_mod._document_payload(123) == {}


def test_store_item_from_document():
    assert ast_mod._store_item_from_document(None) is None
    item = ast_mod._store_item_from_document(
        {"namespace": ["a"], "key": "k", "value": {"v": 1},
         "created_at": 1.0, "updated_at": 2.0}
    )
    assert item.key == "k"
    assert item.value == {"v": 1}


def test_text_for_index():
    assert ast_mod._text_for_index({"a": 1}, None) == '{"a": 1}'
    assert ast_mod._text_for_index({"a": 1}, True) == '{"a": 1}'
    assert ast_mod._text_for_index({"a": "x", "b": "y"}, ["a", "b", "missing"]) == "x\ny"


def test_matches_filter():
    assert ast_mod._matches_filter({"a": 1}, None)
    assert ast_mod._matches_filter({"a": 1}, {"a": 1})
    assert not ast_mod._matches_filter({"a": 1}, {"a": 2})


def test_checkpoint_keys_and_ids():
    tid, ns, cid = ast_mod._checkpoint_keys(
        {"configurable": {"thread_id": "t1", "checkpoint_ns": "n", "checkpoint_id": "c"}}
    )
    assert (tid, ns, cid) == ("t1", "n", "c")
    with pytest.raises(ValueError):
        ast_mod._checkpoint_keys({"configurable": {}})
    assert ast_mod._checkpoint_doc_id("t", "n", "c") == "t\x1fn\x1fc"
    assert ast_mod._write_doc_id("t", "n", None, "task", 0) == "t\x1fn\x1f\x1ftask\x1f0"


# ---------------------------------------------------------------------------
# agentic_store ProximaBaseStore
# ---------------------------------------------------------------------------


def make_base_store(embed=None, dims=None):
    adapter = MagicMock()
    adapter.get_document.return_value = None
    adapter.query_documents.return_value = {"documents": []}
    store = ast_mod.ProximaBaseStore(adapter, embed=embed, dims=dims)
    return store, adapter


def test_base_store_setup_no_embed():
    store, adapter = make_base_store()
    store.setup()
    adapter.create_document_collection.assert_called_once()
    adapter.create_collection.assert_not_called()
    # idempotent
    store.setup()
    adapter.create_document_collection.assert_called_once()


def test_base_store_setup_with_embed():
    store, adapter = make_base_store(embed=lambda t: [[0.0, 1.0]], dims=2)
    store.setup()
    adapter.create_collection.assert_called_once()


def test_base_store_setup_embed_typeerror_fallback():
    store, adapter = make_base_store(embed=lambda t: [[0.0]], dims=2)
    adapter.create_collection.side_effect = [TypeError("bad"), None]
    store.setup()
    assert adapter.create_collection.call_count == 2


def test_base_store_put_new_with_index():
    embed = MagicMock(return_value=[[0.1, 0.2]])
    store, adapter = make_base_store(embed=embed, dims=2)
    store.put(("a",), "k1", {"text": "hi"})
    adapter.insert_document.assert_called_once()
    # vector inserted (insert_records uses adapter.insert_records or insert_vectors)
    assert adapter.insert_records.called or adapter.insert_vectors.called
    embed.assert_called()


def test_base_store_put_index_false_skips_vector():
    embed = MagicMock(return_value=[[0.1]])
    store, adapter = make_base_store(embed=embed, dims=1)
    store.put(("a",), "k1", {"text": "hi"}, index=False)
    embed.assert_not_called()


def test_base_store_put_existing_deletes_first():
    store, adapter = make_base_store()
    adapter.get_document.return_value = {"created_at": 5.0}
    store.put(("a",), "k1", {"v": 1})
    adapter.delete_document.assert_called_once()


def test_base_store_get_found_and_missing():
    store, adapter = make_base_store()
    adapter.get_document.return_value = {
        "namespace": ["a"], "key": "k1", "value": {"v": 1},
        "created_at": 1.0, "updated_at": 2.0,
    }
    item = store.get(("a",), "k1")
    assert item.key == "k1"
    adapter.get_document.return_value = None
    assert store.get(("a",), "k1") is None


def test_base_store_delete():
    store, adapter = make_base_store()
    store.delete(("a",), "k1")
    adapter.delete_document.assert_called_once()
    adapter.delete_vectors.assert_called_once()


def test_base_store_delete_vector_error_swallowed():
    store, adapter = make_base_store()
    adapter.delete_vectors.side_effect = RuntimeError("boom")
    store.delete(("a",), "k1")  # no raise


def test_base_store_search_document_path():
    store, adapter = make_base_store()
    adapter.query_documents.return_value = {
        "documents": [
            {"namespace": ["a"], "key": "k1", "value": {"x": 1},
             "created_at": 1.0, "updated_at": 5.0},
            {"namespace": ["a"], "key": "k2", "value": {"x": 2},
             "created_at": 1.0, "updated_at": 9.0},
        ]
    }
    items = store.search(("a",))
    # sorted by updated_at desc
    assert [i.key for i in items] == ["k2", "k1"]
    filtered = store.search(("a",), filter={"x": 1})
    assert [i.key for i in filtered] == ["k1"]


def test_base_store_search_vector_path():
    embed = MagicMock(return_value=[[0.1, 0.2]])
    store, adapter = make_base_store(embed=embed, dims=2)
    hit = SimpleNamespace(metadata={"key": "k1"}, score=0.9)
    hit_no_key = SimpleNamespace(metadata={}, score=0.5)
    adapter.search.return_value = [hit, hit_no_key]
    # get() returns a matching item
    adapter.get_document.return_value = {
        "namespace": ["a"], "key": "k1", "value": {"x": 1},
        "created_at": 1.0, "updated_at": 2.0,
    }
    items = store.search(("a",), query="find", limit=5)
    assert len(items) == 1
    assert items[0].score == 0.9


def test_base_store_list_namespaces():
    store, adapter = make_base_store()
    adapter.query_documents.return_value = {
        "documents": [
            {"namespace": ["a", "b"]},
            {"namespace": ["a", "c"]},
            {"namespace": ["x", "y"]},
        ]
    }
    out = store.list_namespaces(prefix=("a",))
    assert ("a", "b") in out and ("a", "c") in out
    assert ("x", "y") not in out
    # suffix
    out2 = store.list_namespaces(suffix=("b",))
    assert ("a", "b") in out2
    # max_depth truncation
    out3 = store.list_namespaces(max_depth=1)
    assert ("a",) in out3 and ("x",) in out3


# ---------------------------------------------------------------------------
# agentic_store ProximaCheckpointSaver
# ---------------------------------------------------------------------------


def make_saver():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    adapter.query_documents.return_value = {"documents": []}
    return ast_mod.ProximaCheckpointSaver(adapter), adapter


def test_saver_setup_idempotent():
    saver, adapter = make_saver()
    saver.setup()
    assert adapter.create_document_collection.call_count == 2
    saver.setup()
    assert adapter.create_document_collection.call_count == 2


def test_saver_put_and_aput():
    saver, adapter = make_saver()
    config = {"configurable": {"thread_id": "t1"}}
    out = saver.put(config, {"id": "c1"}, {"m": 1})
    assert out["configurable"]["checkpoint_id"] == "c1"
    adapter.insert_document.assert_called_once()
    # existing -> delete first
    adapter.get_document.return_value = {"id": "x"}
    out2 = run(saver.aput(config, {"id": "c2"}, {}))
    assert out2["configurable"]["checkpoint_id"] == "c2"
    adapter.delete_document.assert_called()


def test_saver_put_generates_uuid_when_missing():
    saver, adapter = make_saver()
    out = saver.put({"configurable": {"thread_id": "t1"}}, {}, {})
    assert out["configurable"]["checkpoint_id"]


def test_saver_put_writes_and_aput_writes():
    saver, adapter = make_saver()
    config = {"configurable": {"thread_id": "t1", "checkpoint_id": "c1"}}
    saver.put_writes(config, [("ch1", "v1"), ("ch2", "v2")], "task1")
    assert adapter.insert_document.call_count == 2
    adapter.insert_document.reset_mock()
    run(saver.aput_writes(config, [("ch1", "v1")], "task2"))
    assert adapter.insert_document.call_count == 1


def test_saver_get_tuple_and_aget():
    saver, adapter = make_saver()
    docs = {
        "documents": [
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1",
             "config": {}, "checkpoint": {"a": 1}, "metadata": {},
             "parent_config": None, "created_at": 1.0},
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c2",
             "config": {}, "checkpoint": {"a": 2}, "metadata": {},
             "parent_config": None, "created_at": 9.0},
        ]
    }
    # checkpoint query then writes query (empty)
    adapter.query_documents.side_effect = [docs, {"documents": []}]
    tup = saver.get_tuple({"configurable": {"thread_id": "t1"}})
    assert tup.checkpoint == {"a": 2}  # newest

    adapter.query_documents.side_effect = [docs, {"documents": []}]
    tup2 = run(saver.aget_tuple({"configurable": {"thread_id": "t1"}}))
    assert tup2.checkpoint == {"a": 2}


def test_saver_get_tuple_specific_id():
    saver, adapter = make_saver()
    docs = {
        "documents": [
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1",
             "config": {}, "checkpoint": {"a": 1}, "metadata": {},
             "created_at": 1.0},
        ]
    }
    adapter.query_documents.side_effect = [docs, {"documents": []}]
    tup = saver.get_tuple(
        {"configurable": {"thread_id": "t1", "checkpoint_id": "c1"}}
    )
    assert tup.checkpoint == {"a": 1}


def test_saver_get_tuple_none():
    saver, adapter = make_saver()
    adapter.query_documents.return_value = {"documents": []}
    assert saver.get_tuple({"configurable": {"thread_id": "t1"}}) is None


def test_saver_list_and_alist():
    saver, adapter = make_saver()
    docs = {
        "documents": [
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c1",
             "config": {}, "checkpoint": {}, "metadata": {}, "created_at": 1.0},
            {"thread_id": "t1", "checkpoint_ns": "", "checkpoint_id": "c3",
             "config": {}, "checkpoint": {}, "metadata": {}, "created_at": 3.0},
        ]
    }
    # list: checkpoint docs, then writes for each returned tuple
    adapter.query_documents.side_effect = [docs, {"documents": []}, {"documents": []}]
    out = saver.list({"configurable": {"thread_id": "t1"}})
    assert len(out) == 2

    # with before + limit
    adapter.query_documents.side_effect = [docs, {"documents": []}]
    out2 = saver.list(
        {"configurable": {"thread_id": "t1"}},
        before={"configurable": {"thread_id": "t1", "checkpoint_id": "c3"}},
        limit=5,
    )
    assert all(t.config == {} for t in out2)

    adapter.query_documents.side_effect = [docs, {"documents": []}, {"documents": []}]
    out3 = run(saver.alist({"configurable": {"thread_id": "t1"}}))
    assert len(out3) == 2


def test_saver_delete_thread():
    saver, adapter = make_saver()
    adapter.query_documents.return_value = {
        "documents": [{"id": "d1"}, {"id": "d2"}]
    }
    saver.delete_thread("t1")
    # 2 docs per collection * 2 collections = 4 deletes
    assert adapter.delete_document.call_count == 4


# ---------------------------------------------------------------------------
# agentic_io helpers
# ---------------------------------------------------------------------------


def test_io_documents_and_payload():
    assert aio._documents({"documents": [1, 2]}) == [1, 2]
    assert aio._documents("x") == []
    assert aio._payload(None) == {}
    assert aio._payload({"document": {"a": 1}}) == {"a": 1}
    assert aio._payload({"a": 1}) == {"a": 1}
    assert aio._payload(5) == {}


def test_io_event_roundtrip():
    rec = aio.EventRecord(
        stream_id="s1", version=1, event_type="created", data={"x": 1},
        metadata={"m": 1}, event_id="e1", global_position=1, created_at=1.0,
    )
    doc = aio._event_to_document(rec)
    back = aio._event_from_document(doc)
    assert back == rec
    assert aio._event_doc_id("s1", 1, "e1").startswith("s1\x1f")


@dataclass
class _Sample:
    id: str
    name: str


def test_model_to_dict_variants():
    assert aio._model_to_dict({"a": 1}) == {"a": 1}
    assert aio._model_to_dict(_Sample(id="1", name="x")) == {"id": "1", "name": "x"}

    class WithDump:
        def model_dump(self):
            return {"k": "v"}

    assert aio._model_to_dict(WithDump()) == {"k": "v"}

    class WithDict:
        def dict(self):
            return {"k2": "v2"}

    assert aio._model_to_dict(WithDict()) == {"k2": "v2"}

    with pytest.raises(TypeError):
        aio._model_to_dict(object())


def test_dict_to_model_variants():
    assert aio._dict_to_model(dict, {"a": 1}) == {"a": 1}
    s = aio._dict_to_model(_Sample, {"id": "1", "name": "x", "extra": "ignored"})
    assert s == _Sample(id="1", name="x")

    class WithValidate:
        @classmethod
        def model_validate(cls, payload):
            obj = cls()
            obj.payload = payload
            return obj

    assert aio._dict_to_model(WithValidate, {"a": 1}).payload == {"a": 1}

    class WithParseObj:
        @classmethod
        def parse_obj(cls, payload):
            obj = cls()
            obj.payload = payload
            return obj

    assert aio._dict_to_model(WithParseObj, {"a": 1}).payload == {"a": 1}

    class Plain:
        def __init__(self, **kw):
            self.kw = kw

    assert aio._dict_to_model(Plain, {"a": 1}).kw == {"a": 1}


def test_default_collection_name():
    assert aio._default_collection_name(_Sample) == "_samples"


# ---------------------------------------------------------------------------
# agentic_io ProximaEventStore
# ---------------------------------------------------------------------------


def make_event_store():
    adapter = MagicMock()
    adapter.query_documents.return_value = {"documents": []}
    return aio.ProximaEventStore(adapter), adapter


def test_event_store_setup_idempotent():
    store, adapter = make_event_store()
    store.setup()
    adapter.create_document_collection.assert_called_once()
    store.setup()
    adapter.create_document_collection.assert_called_once()


def test_event_store_append_first():
    store, adapter = make_event_store()
    rec = store.append("s1", "created", {"x": 1}, expected_version=0)
    assert rec.version == 1
    assert rec.global_position == 1
    adapter.insert_document.assert_called_once()


def test_event_store_append_version_conflict():
    store, adapter = make_event_store()
    with pytest.raises(ValueError):
        store.append("s1", "created", {}, expected_version=5)


def test_event_store_read_stream_filters_and_orders():
    store, adapter = make_event_store()
    docs = [
        aio._event_to_document(aio.EventRecord(
            stream_id="s1", version=v, event_type="e", data={}, metadata={},
            event_id=f"e{v}", global_position=v, created_at=float(v)))
        for v in (3, 1, 2)
    ]
    adapter.query_documents.return_value = {"documents": docs}
    out = store.read_stream("s1", after_version=1, limit=10)
    assert [e.version for e in out] == [2, 3]


def test_event_store_read_all():
    store, adapter = make_event_store()
    docs = [
        aio._event_to_document(aio.EventRecord(
            stream_id="s", version=1, event_type="e", data={}, metadata={},
            event_id=f"e{p}", global_position=p, created_at=1.0))
        for p in (2, 1, 3)
    ]
    adapter.query_documents.return_value = {"documents": docs}
    out = store.read_all(after_position=1, limit=10)
    assert [e.global_position for e in out] == [2, 3]


def test_event_store_append_increments_over_existing():
    store, adapter = make_event_store()
    existing = [
        aio._event_to_document(aio.EventRecord(
            stream_id="s1", version=1, event_type="e", data={}, metadata={},
            event_id="e1", global_position=1, created_at=1.0))
    ]
    # read_stream (existing) for current version, read_all for global pos
    adapter.query_documents.side_effect = [
        {"documents": existing},  # read_stream in append
        {"documents": existing},  # read_all in _next_global_position
    ]
    rec = store.append("s1", "updated", {"y": 2})
    assert rec.version == 2
    assert rec.global_position == 2


def test_event_store_snapshot():
    store, adapter = make_event_store()
    adapter.query_documents.return_value = {"documents": []}
    rec = store.snapshot("s1", {"state": 1})
    assert rec.event_type == "$snapshot"


# ---------------------------------------------------------------------------
# agentic_io ProximaMapperSession + ProximaQuery
# ---------------------------------------------------------------------------


def make_session():
    adapter = MagicMock()
    adapter.get_document.return_value = None
    adapter.query_documents.return_value = {"documents": []}
    return aio.ProximaMapperSession(adapter), adapter


def test_session_register():
    session, adapter = make_session()
    session.register(_Sample, collection="samples", indexed_paths=["$.name"])
    assert session._collections[_Sample] == "samples"
    adapter.create_document_collection.assert_called_once()


def test_session_register_create_error_swallowed():
    session, adapter = make_session()
    adapter.create_document_collection.side_effect = RuntimeError("boom")
    session.register(_Sample)  # no raise


def test_session_upsert_dataclass():
    session, adapter = make_session()
    doc_id = session.upsert(_Sample(id="i1", name="x"))
    assert doc_id == "i1"
    adapter.insert_document.assert_called_once()


def test_session_upsert_generates_id_and_deletes_existing():
    session, adapter = make_session()
    adapter.get_document.return_value = {"id": "old"}
    doc_id = session.upsert({"name": "x"}, collection="c")
    assert doc_id
    adapter.delete_document.assert_called_once()


def test_session_upsert_with_vector():
    session, adapter = make_session()
    doc_id = session.upsert(
        {"id": "i1"}, collection="c", vector=[0.1, 0.2, 0.3]
    )
    assert doc_id == "i1"
    adapter.create_collection.assert_called_once()
    assert adapter.insert_records.called or adapter.insert_vectors.called


def test_session_upsert_vector_create_error_swallowed():
    session, adapter = make_session()
    adapter.create_collection.side_effect = RuntimeError("exists")
    session.upsert({"id": "i1"}, collection="c", vector=[0.1])
    assert adapter.insert_records.called or adapter.insert_vectors.called


def test_session_get_found_and_missing():
    session, adapter = make_session()
    adapter.get_document.return_value = {"id": "i1", "name": "x"}
    out = session.get(_Sample, "i1", collection="c")
    assert out == _Sample(id="i1", name="x")
    adapter.get_document.return_value = None
    assert session.get(_Sample, "i1", collection="c") is None


def test_session_delete():
    session, adapter = make_session()
    session.delete(_Sample, "i1", collection="c")
    adapter.delete_document.assert_called_once_with("c", "i1")


def test_session_vector_search():
    session, adapter = make_session()
    hit = SimpleNamespace(id="i1", score=0.8, metadata={"m": 1})
    hit_no_id = SimpleNamespace(id=None, score=0.5, metadata={})
    adapter.search.return_value = [hit, hit_no_id]
    adapter.get_document.return_value = {"id": "i1", "name": "x"}
    results = session.vector_search(_Sample, [0.1, 0.2], collection="c")
    assert len(results) == 1
    assert results[0].item == _Sample(id="i1", name="x")
    assert results[0].score == 0.8


def test_session_vector_search_skips_missing_doc():
    session, adapter = make_session()
    adapter.search.return_value = [SimpleNamespace(id="i1", score=0.8, metadata={})]
    adapter.get_document.return_value = None
    results = session.vector_search(_Sample, [0.1], collection="c")
    assert results == []


def test_session_link_primary_signature():
    session, adapter = make_session()
    adapter.create_edge.return_value = {"ok": True}
    out = session.link("a", "CALLS", "b", properties={"w": 1})
    assert out == {"ok": True}
    adapter.create_edge.assert_called_once()


def test_session_link_typeerror_fallback():
    session, adapter = make_session()
    adapter.create_edge.side_effect = [TypeError("bad sig"), {"ok": True}]
    out = session.link("a", "CALLS", "b")
    assert out == {"ok": True}
    assert adapter.create_edge.call_count == 2


def test_session_collection_for_auto_registers():
    session, adapter = make_session()
    name = session._collection_for(_Sample)
    assert name == "_samples"


def test_query_builder_all_and_first():
    session, adapter = make_session()
    adapter.query_documents.return_value = {
        "documents": [
            {"id": "i1", "name": "a"},
            {"id": "i2", "name": "b"},
            {"id": "i3", "name": "c"},
        ]
    }
    q = session.query(_Sample, collection="c").where(name="a").limit(2).offset(1)
    items = q.all()
    assert [i.id for i in items] == ["i2", "i3"]

    first = session.query(_Sample, collection="c").first()
    assert first.id == "i1"


def test_query_first_empty():
    session, adapter = make_session()
    adapter.query_documents.return_value = {"documents": []}
    assert session.query(_Sample, collection="c").first() is None
