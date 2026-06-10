"""Offline unit tests for proximadb_sdk.graph.

Injects a mock backend client (no network, no server). Exercises node/edge/
traverse/query/find-callers wrapper methods, dataclass helpers, JSON import,
and pattern/cypher parsing branches.
"""

from __future__ import annotations

import json
from unittest.mock import MagicMock

import pytest

from proximadb_sdk.graph import (
    GraphEdge,
    GraphNode,
    GraphPath,
    GraphQueryResult,
    ProximaDBGraph,
    create_graph_api,
)


# ---------------------------------------------------------------------------
# Dataclasses
# ---------------------------------------------------------------------------


def test_graphnode_to_dict():
    n = GraphNode(id="a", labels=["Function"], properties={"name": "main"})
    assert n.to_dict() == {
        "id": "a",
        "labels": ["Function"],
        "properties": {"name": "main"},
    }
    n2 = GraphNode(id="b")
    assert n2.labels == []
    assert n2.properties == {}
    assert n2.embedding is None


def test_graphedge_to_dict_with_and_without_weight():
    e = GraphEdge(id="e1", from_node="a", to_node="b", edge_type="CALLS")
    d = e.to_dict()
    assert "weight" not in d
    assert d["from_node"] == "a" and d["to_node"] == "b"

    e2 = GraphEdge(
        id="e2", from_node="a", to_node="b", edge_type="CALLS", weight=3.5
    )
    assert e2.to_dict()["weight"] == 3.5


def test_graphpath_and_queryresult_defaults():
    p = GraphPath()
    assert p.nodes == [] and p.edges == [] and p.total_weight == 0.0
    r = GraphQueryResult()
    assert r.nodes == [] and r.edges == [] and r.paths == [] and r.stats == {}


def test_create_graph_api_factory():
    client = MagicMock()
    g = create_graph_api(client, "gid")
    assert isinstance(g, ProximaDBGraph)
    assert g._graph_id == "gid"
    assert g._client is client


# ---------------------------------------------------------------------------
# Static normalizers
# ---------------------------------------------------------------------------


def test_is_internal_label():
    assert ProximaDBGraph._is_internal_label("__meta") is True
    assert ProximaDBGraph._is_internal_label("Function") is False


def test_normalize_node_variants():
    assert ProximaDBGraph._normalize_node(None) is None

    gn = GraphNode(id="x")
    assert ProximaDBGraph._normalize_node(gn) is gn

    from_dict = ProximaDBGraph._normalize_node(
        {"id": "d", "labels": ["L"], "properties": {"k": "v"}}
    )
    assert from_dict.id == "d" and from_dict.labels == ["L"]

    class Obj:
        id = "o"
        labels = ["A"]
        properties = {"p": 1}

    from_obj = ProximaDBGraph._normalize_node(Obj())
    assert from_obj.id == "o" and from_obj.properties == {"p": 1}


def test_normalize_edge_variants():
    assert ProximaDBGraph._normalize_edge(None) is None

    ge = GraphEdge(id="e", from_node="a", to_node="b", edge_type="T")
    assert ProximaDBGraph._normalize_edge(ge) is ge

    from_dict = ProximaDBGraph._normalize_edge(
        {"id": "e", "from_node_id": "a", "to_node_id": "b", "type": "CALLS",
         "weight": 2}
    )
    assert from_dict.from_node == "a" and from_dict.edge_type == "CALLS"
    assert from_dict.weight == 2

    from_dict2 = ProximaDBGraph._normalize_edge(
        {"from_node": "x", "to_node": "y", "edge_type": "IMPORTS"}
    )
    assert from_dict2.from_node == "x" and from_dict2.edge_type == "IMPORTS"

    class EObj:
        id = "eo"
        from_node_id = "f"
        to_node_id = "t"
        edge_type = "REL"
        properties = {"w": 1}
        weight = 9.0

    from_obj = ProximaDBGraph._normalize_edge(EObj())
    assert from_obj.from_node == "f" and from_obj.weight == 9.0


def test_normalize_json_node():
    assert ProximaDBGraph._normalize_json_node({}) is None

    out = ProximaDBGraph._normalize_json_node(
        {"node_id": "n1", "type": "Function", "extra": "value"}
    )
    assert out["id"] == "n1"
    assert out["labels"] == ["Function"]
    assert out["properties"]["extra"] == "value"

    out2 = ProximaDBGraph._normalize_json_node({"key": "k", "labels": "Single"})
    assert out2["labels"] == ["Single"]

    out3 = ProximaDBGraph._normalize_json_node(
        {"id": "n3", "labels": ["A", "B"], "properties": {"x": 1}}
    )
    assert out3["labels"] == ["A", "B"] and out3["properties"]["x"] == 1


def test_normalize_json_edge():
    assert ProximaDBGraph._normalize_json_edge({}, 0) is None

    out = ProximaDBGraph._normalize_json_edge(
        {"source": "a", "target": "b", "label": "CALLS", "extra": 1}, 5
    )
    assert out["from_node_id"] == "a" and out["to_node_id"] == "b"
    assert out["edge_type"] == "CALLS"
    assert out["properties"]["extra"] == 1
    assert out["id"]

    out2 = ProximaDBGraph._normalize_json_edge(
        {"from": "x", "to": "y", "id": "myid", "weight": 1.5}, 0
    )
    assert out2["edge_type"] == "RELATED_TO"
    assert out2["id"] == "myid"
    assert out2["weight"] == 1.5


# ---------------------------------------------------------------------------
# Batch create
# ---------------------------------------------------------------------------


def test_batch_create_nodes_empty():
    g = ProximaDBGraph(MagicMock(), "gid")
    assert g.batch_create_nodes([]) == {"success": True, "created": 0}


def test_batch_create_nodes_success_mixed_types():
    client = MagicMock()
    g = ProximaDBGraph(client, "gid")
    nodes = [
        GraphNode(id="a", labels=["F"], properties={"n": 1}),
        {"id": "b", "labels": ["F"], "properties": {"n": 2}},
    ]
    res = g.batch_create_nodes(nodes, batch_size=1)
    assert res == {"success": True, "created": 2, "failed": 0, "errors": []}
    assert client.create_node.call_count == 2


def test_batch_create_nodes_failure():
    client = MagicMock()
    client.create_node.side_effect = RuntimeError("boom")
    g = ProximaDBGraph(client, "gid")
    res = g.batch_create_nodes([{"id": "a"}])
    assert res["success"] is False
    assert res["failed"] == 1
    assert "boom" in res["errors"][0]["error"]


def test_batch_create_edges_empty():
    g = ProximaDBGraph(MagicMock(), "gid")
    assert g.batch_create_edges([]) == {"success": True, "created": 0}


def test_batch_create_edges_success():
    client = MagicMock()
    g = ProximaDBGraph(client, "gid")
    edges = [
        GraphEdge(id="e1", from_node="a", to_node="b", edge_type="CALLS"),
        {"from": "a", "to": "c", "type": "CALLS"},
    ]
    res = g.batch_create_edges(edges, batch_size=10)
    assert res["success"] is True and res["created"] == 2
    assert client.create_edge.call_count == 2


def test_batch_create_edges_missing_fields_records_failure():
    client = MagicMock()
    g = ProximaDBGraph(client, "gid")
    res = g.batch_create_edges([{"from": "a", "to": "b"}])
    assert res["success"] is False
    assert res["failed"] == 1


# ---------------------------------------------------------------------------
# Cypher query
# ---------------------------------------------------------------------------


def test_query_cypher_no_match_returns_empty():
    g = ProximaDBGraph(MagicMock(), "gid")
    res = g.query_cypher("RETURN 1")
    assert isinstance(res, GraphQueryResult)
    assert res.nodes == []


def test_query_cypher_no_start_nodes():
    client = MagicMock()
    client.query_nodes.return_value = {"nodes": []}
    g = ProximaDBGraph(client, "gid")
    res = g.query_cypher("MATCH (c:Function) WHERE c.name = 'main' RETURN c")
    assert res.nodes == []


def test_query_cypher_with_traversal():
    client = MagicMock()
    client.query_nodes.return_value = {"nodes": [{"id": "func:main"}]}
    client.traverse_graph.return_value = {
        "nodes": [{"id": "func:parse", "labels": ["Function"], "properties": {}}],
        "edges": [
            {"id": "e", "from_node_id": "func:main", "to_node_id": "func:parse",
             "edge_type": "CALLS"}
        ],
    }
    g = ProximaDBGraph(client, "gid")
    res = g.query_cypher(
        'MATCH (c:Function)-[r:CALLS]->(f:Function) WHERE c.name = "main" RETURN c, f'
    )
    assert len(res.nodes) == 1 and res.nodes[0].id == "func:parse"
    assert len(res.edges) == 1
    _, kwargs = client.traverse_graph.call_args
    assert kwargs["edge_types"] == ["CALLS"]


def test_execute_traversal_swallows_exception():
    client = MagicMock()
    client.traverse_graph.side_effect = RuntimeError("nope")
    g = ProximaDBGraph(client, "gid")
    res = g._execute_traversal(["a"], {"traversal": None})
    assert res.nodes == [] and res.edges == []


def test_parse_cypher_properties_in_pattern():
    g = ProximaDBGraph(MagicMock(), "gid")
    parsed = g._parse_cypher('MATCH (c:Function {name: "main"}) RETURN c')
    assert parsed["match"] is True
    assert parsed["start_labels"] == ["Function"]
    assert parsed["start_properties"]["name"] == "main"


# ---------------------------------------------------------------------------
# get_node_by_id / _get_node_raw fallback
# ---------------------------------------------------------------------------


def test_get_node_by_id_direct_hit():
    client = MagicMock()
    client.get_node.return_value = {"id": "n", "labels": ["L"], "properties": {}}
    g = ProximaDBGraph(client, "gid")
    node = g.get_node_by_id("n")
    assert node.id == "n"


def test_get_node_raw_fallback_to_query_scan():
    client = MagicMock()
    client.get_node.side_effect = RuntimeError("not found")
    client.query_nodes.return_value = {
        "nodes": [{"id": "other"}, {"id": "target", "labels": [], "properties": {}}]
    }
    g = ProximaDBGraph(client, "gid")
    node = g.get_node_by_id("target")
    assert node.id == "target"


def test_get_node_raw_returns_none_when_not_found():
    client = MagicMock()
    client.get_node.return_value = None
    client.query_nodes.return_value = {"nodes": []}
    g = ProximaDBGraph(client, "gid")
    assert g.get_node_by_id("missing") is None


# ---------------------------------------------------------------------------
# Outgoing / incoming edge helpers
# ---------------------------------------------------------------------------


def test_get_outgoing_edges_direct():
    client = MagicMock()
    client.get_outgoing_edges.return_value = [
        {"id": "e", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"}
    ]
    g = ProximaDBGraph(client, "gid")
    edges = g._get_outgoing_edges_raw("a")
    assert edges[0]["id"] == "e"


def test_get_outgoing_edges_traversal_fallback():
    client = MagicMock()
    client.get_outgoing_edges.side_effect = RuntimeError("unsupported")
    client.traverse_graph.return_value = {
        "edges": [
            {"id": "e", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"},
            {"id": "e2", "from_node_id": "z", "to_node_id": "a", "edge_type": "CALLS"},
        ]
    }
    g = ProximaDBGraph(client, "gid")
    edges = g._get_outgoing_edges_raw("a")
    assert len(edges) == 1
    # traversal fallback returns normalized .to_dict() shape (from_node key)
    assert edges[0]["from_node"] == "a"


def test_get_outgoing_edges_both_paths_fail():
    client = MagicMock()
    client.get_outgoing_edges.side_effect = RuntimeError("x")
    client.traverse_graph.side_effect = RuntimeError("y")
    g = ProximaDBGraph(client, "gid")
    assert g._get_outgoing_edges_raw("a") == []


def test_get_incoming_edges_direct():
    client = MagicMock()
    client.get_incoming_edges.return_value = [
        {"id": "e", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"}
    ]
    g = ProximaDBGraph(client, "gid")
    edges = g._get_incoming_edges_raw("b")
    assert edges[0]["from_node_id"] == "a"


def test_get_incoming_edges_scan_fallback():
    client = MagicMock()
    client.get_incoming_edges.side_effect = RuntimeError("unsupported")
    client.query_nodes.return_value = {
        "nodes": [
            {"id": "a", "labels": [], "properties": {}},
            {"id": "z", "labels": [], "properties": {}},
        ]
    }

    def outgoing(node_id, edge_types=None, graph_id=None):
        if node_id == "a":
            return [
                {"id": "e1", "from_node_id": "a", "to_node_id": "b",
                 "edge_type": "CALLS"}
            ]
        return []

    client.get_outgoing_edges.side_effect = outgoing
    g = ProximaDBGraph(client, "gid")
    edges = g._get_incoming_edges_raw("b")
    assert len(edges) == 1
    # scan fallback returns normalized .to_dict() shape (from_node key)
    assert edges[0]["from_node"] == "a"


# ---------------------------------------------------------------------------
# find_callers
# ---------------------------------------------------------------------------


def test_find_callers_depth_one():
    client = MagicMock()
    client.get_incoming_edges.return_value = [
        {"id": "e", "from_node_id": "caller", "to_node_id": "target",
         "edge_type": "CALLS"}
    ]
    client.get_node.return_value = {
        "id": "caller", "labels": ["Function"], "properties": {"name": "c"}
    }
    g = ProximaDBGraph(client, "gid")
    callers = g.find_callers("target")
    assert len(callers) == 1 and callers[0].id == "caller"


def test_find_callers_multi_depth():
    client = MagicMock()

    def incoming(node_id, edge_types=None, graph_id=None):
        if node_id == "target":
            return [{"id": "e1", "from_node_id": "c1", "to_node_id": "target",
                     "edge_type": "CALLS"}]
        if node_id == "c1":
            return [{"id": "e2", "from_node_id": "c2", "to_node_id": "c1",
                     "edge_type": "CALLS"}]
        return []

    client.get_incoming_edges.side_effect = incoming

    def get_node(node_id, graph_id=None):
        return {"id": node_id, "labels": [], "properties": {}}

    client.get_node.side_effect = get_node
    g = ProximaDBGraph(client, "gid")
    callers = g.find_callers("target", max_depth=2)
    ids = {c.id for c in callers}
    assert ids == {"c1", "c2"}


def test_find_callers_exception_returns_empty():
    client = MagicMock()
    client.get_incoming_edges.side_effect = RuntimeError("boom")
    client.query_nodes.side_effect = RuntimeError("boom2")
    g = ProximaDBGraph(client, "gid")
    assert g.find_callers("target") == []


# ---------------------------------------------------------------------------
# get_all_nodes / paging / internal-label filter
# ---------------------------------------------------------------------------


def test_get_all_nodes_paging_and_internal_filter():
    client = MagicMock()
    page1 = [{"id": f"n{i}", "labels": [], "properties": {}} for i in range(2)]
    page1.append({"id": "internal", "labels": ["__meta"], "properties": {}})
    page2 = [{"id": "n9", "labels": [], "properties": {}}]
    client.query_nodes.side_effect = [
        {"nodes": page1},
        {"nodes": page2},
    ]
    g = ProximaDBGraph(client, "gid")
    nodes = g.get_all_nodes(batch_size=2)
    ids = [n.id for n in nodes]
    assert "internal" not in ids
    assert "n9" in ids


def test_get_all_nodes_include_internal():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [{"id": "internal", "labels": ["__meta"], "properties": {}}]
    }
    g = ProximaDBGraph(client, "gid")
    nodes = g.get_all_nodes(batch_size=1000, include_internal=True)
    assert any(n.id == "internal" for n in nodes)


def test_query_nodes_raw_non_dict_result():
    client = MagicMock()
    client.query_nodes.return_value = ["not", "a", "dict"]
    g = ProximaDBGraph(client, "gid")
    assert g._query_nodes_raw() == []


# ---------------------------------------------------------------------------
# get_nodes_by_file / find_nodes / search_symbols
# ---------------------------------------------------------------------------


def test_get_nodes_by_file():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [
            {"id": "a", "labels": [], "properties": {"file": "x.py"}},
            {"id": "b", "labels": [], "properties": {"path": "y.py"}},
        ]
    }
    g = ProximaDBGraph(client, "gid")
    result = g.get_nodes_by_file("x.py")
    assert [n.id for n in result] == ["a"]


def test_find_nodes_filters():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [
            {"id": "a", "labels": ["Function"],
             "properties": {"name": "main", "file": "x.py"}},
            {"id": "b", "labels": ["Function"],
             "properties": {"name": "main", "file": "other.py"}},
        ]
    }
    g = ProximaDBGraph(client, "gid")
    result = g.find_nodes(name="main", type="Function", file="x.py")
    assert [n.id for n in result] == ["a"]


def test_find_nodes_qualified_name_match():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [
            {"id": "a", "labels": [], "properties": {"qualified_name": "pkg.main"}},
        ]
    }
    g = ProximaDBGraph(client, "gid")
    result = g.find_nodes(name="pkg.main")
    assert [n.id for n in result] == ["a"]


def test_search_symbols_empty_query():
    g = ProximaDBGraph(MagicMock(), "gid")
    assert g.search_symbols("   ") == []


def test_search_symbols_ranking_and_type_filter():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [
            {"id": "exact", "labels": ["Function"],
             "properties": {"name": "parse", "file": "a.py", "line": 1}},
            {"id": "prefix", "labels": ["Function"],
             "properties": {"name": "parser_helper", "file": "b.py", "line": 2}},
            {"id": "sig", "labels": ["Function"],
             "properties": {"name": "x", "signature": "def parse_thing()",
                            "file": "c.py", "line": 3}},
            {"id": "wrongtype", "labels": ["Class"],
             "properties": {"name": "parse", "file": "d.py", "line": 4}},
            {"id": "nohay", "labels": ["Function"], "properties": {}},
        ]
    }
    g = ProximaDBGraph(client, "gid")
    results = g.search_symbols("parse", limit=10, symbol_types=["Function"])
    ids = [n.id for n in results]
    assert "wrongtype" not in ids
    assert ids[0] == "exact"
    assert "prefix" in ids and "sig" in ids


def test_search_symbols_qualified_and_docstring_paths():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [
            {"id": "qn", "labels": ["F"],
             "properties": {"qualified_name": "pkg.parse", "name": "z"}},
            {"id": "doc", "labels": ["F"],
             "properties": {"name": "y", "docstring": "this will parse text"}},
        ]
    }
    g = ProximaDBGraph(client, "gid")
    results = g.search_symbols("parse")
    ids = {n.id for n in results}
    assert "qn" in ids and "doc" in ids


# ---------------------------------------------------------------------------
# get_neighbors / get_all_edges
# ---------------------------------------------------------------------------


def test_get_neighbors_both_directions():
    client = MagicMock()
    client.get_outgoing_edges.return_value = [
        {"id": "out", "from_node_id": "n", "to_node_id": "b", "edge_type": "CALLS"}
    ]
    client.get_incoming_edges.return_value = [
        {"id": "in", "from_node_id": "a", "to_node_id": "n", "edge_type": "CALLS"}
    ]
    g = ProximaDBGraph(client, "gid")
    edges = g.get_neighbors("n", direction="both")
    sigs = {(e.from_node, e.to_node) for e in edges}
    assert ("n", "b") in sigs and ("a", "n") in sigs


def test_get_neighbors_out_only_and_dedup():
    client = MagicMock()
    client.get_outgoing_edges.return_value = [
        {"id": "out", "from_node_id": "n", "to_node_id": "b", "edge_type": "CALLS"},
        {"id": "dup", "from_node_id": "n", "to_node_id": "b", "edge_type": "CALLS"},
    ]
    g = ProximaDBGraph(client, "gid")
    edges = g.get_neighbors("n", edge_types=["CALLS"], direction="out", max_depth=1)
    assert len(edges) == 1


def test_get_all_edges():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [{"id": "a", "labels": [], "properties": {}}]
    }
    client.get_outgoing_edges.return_value = [
        {"id": "e", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"}
    ]
    g = ProximaDBGraph(client, "gid")
    edges = g.get_all_edges(edge_types=["CALLS"])
    assert len(edges) == 1 and edges[0].from_node == "a"


# ---------------------------------------------------------------------------
# import_graph_json
# ---------------------------------------------------------------------------


def test_import_graph_json_from_dict():
    client = MagicMock()
    g = ProximaDBGraph(client, "gid")
    data = {
        "nodes": [{"id": "a", "type": "Function"}],
        "edges": [{"source": "a", "target": "b", "label": "CALLS"}],
    }
    res = g.import_graph_json(data)
    assert res["node_count"] == 1 and res["edge_count"] == 1
    assert res["success"] is True


def test_import_graph_json_nested_graph_key():
    client = MagicMock()
    g = ProximaDBGraph(client, "gid")
    data = {"graph": {"nodes": [{"id": "a"}], "edges": []}}
    res = g.import_graph_json(data)
    assert res["node_count"] == 1 and res["edge_count"] == 0


def test_import_graph_json_from_file(tmp_path):
    client = MagicMock()
    g = ProximaDBGraph(client, "gid")
    payload = {"nodes": [{"id": "a", "type": "F"}], "edges": []}
    p = tmp_path / "graph.json"
    p.write_text(json.dumps(payload))
    res = g.import_graph_json(str(p))
    assert res["node_count"] == 1


# ---------------------------------------------------------------------------
# match_pattern
# ---------------------------------------------------------------------------


def test_match_pattern_node_only():
    client = MagicMock()
    client.query_nodes.return_value = {
        "nodes": [{"id": "a", "labels": ["Function"], "properties": {"name": "main"}}]
    }
    g = ProximaDBGraph(client, "gid")
    matches = g.match_pattern('(f:Function {name: "main"})')
    assert len(matches) == 1
    assert matches[0]["f"].id == "a"


def test_match_pattern_with_relationship_returns_empty():
    g = ProximaDBGraph(MagicMock(), "gid")
    matches = g.match_pattern("(f1:Function)-[r:CALLS]->(f2:Function)")
    assert matches == []


def test_match_pattern_no_nodes():
    g = ProximaDBGraph(MagicMock(), "gid")
    assert g.match_pattern("RETURN nothing") == []


# ---------------------------------------------------------------------------
# get_stats
# ---------------------------------------------------------------------------


def test_get_stats():
    client = MagicMock()
    client.get_graph_stats.return_value = {"node_count": 5, "edge_count": 3}
    g = ProximaDBGraph(client, "gid")
    assert g.get_stats() == {"node_count": 5, "edge_count": 3}
    client.get_graph_stats.assert_called_once_with("gid")


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
