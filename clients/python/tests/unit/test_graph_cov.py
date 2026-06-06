"""Offline unit tests for proximadb_sdk.graph.

All transport is mocked via a hand-built fake client; no network/server.
"""

from __future__ import annotations

import json

import pytest

from proximadb_sdk.graph import (
    GraphEdge,
    GraphNode,
    GraphPath,
    GraphQueryResult,
    ProximaDBGraph,
    create_graph_api,
)


class FakeClient:
    """Hand fake backend implementing every method ProximaDBGraph calls."""

    def __init__(self):
        self.nodes = []  # list[dict]
        self.edges = []  # list[dict] with from_node_id/to_node_id
        self.created_nodes = []
        self.created_edges = []
        self.fail_create_node = False
        self.support_get_node = True
        self.support_outgoing = True
        self.support_incoming = True
        self.support_traverse = True

    # ---- creation ----
    def create_node(self, graph_id, node_id, labels, properties):
        if self.fail_create_node:
            raise RuntimeError("boom")
        self.created_nodes.append(
            {"id": node_id, "labels": labels, "properties": properties}
        )

    def create_edge(
        self, graph_id, edge_id, from_node_id, to_node_id, edge_type, properties, weight
    ):
        self.created_edges.append(
            {
                "id": edge_id,
                "from_node_id": from_node_id,
                "to_node_id": to_node_id,
                "edge_type": edge_type,
                "properties": properties,
                "weight": weight,
            }
        )

    # ---- query ----
    def query_nodes(self, graph_id, labels=None, properties=None, limit=None, offset=None):
        offset = offset or 0
        result = []
        for n in self.nodes:
            if labels:
                if not any(lbl in (n.get("labels") or []) for lbl in labels):
                    continue
            if properties:
                if not all(
                    n.get("properties", {}).get(k) == v for k, v in properties.items()
                ):
                    continue
            result.append(n)
        if limit is not None:
            result = result[offset : offset + limit]
        return {"nodes": result}

    def get_node(self, node_id, graph_id):
        if not self.support_get_node:
            raise RuntimeError("not supported")
        for n in self.nodes:
            if n.get("id") == node_id:
                return n
        return None

    def get_outgoing_edges(self, node_id, edge_types, graph_id):
        if not self.support_outgoing:
            raise RuntimeError("not supported")
        out = []
        for e in self.edges:
            if e.get("from_node_id") == node_id:
                if edge_types and e.get("edge_type") not in edge_types:
                    continue
                out.append(dict(e))
        return out

    def get_incoming_edges(self, node_id, edge_types, graph_id):
        if not self.support_incoming:
            raise RuntimeError("not supported")
        out = []
        for e in self.edges:
            if e.get("to_node_id") == node_id:
                if edge_types and e.get("edge_type") not in edge_types:
                    continue
                out.append(dict(e))
        return out

    def traverse_graph(
        self, graph_id, start_node_id, max_depth, edge_types=None, limit=None
    ):
        if not self.support_traverse:
            raise RuntimeError("not supported")
        nodes = []
        edges = []
        for e in self.edges:
            if e.get("from_node_id") == start_node_id:
                if edge_types and e.get("edge_type") not in edge_types:
                    continue
                edges.append(dict(e))
                for n in self.nodes:
                    if n.get("id") == e.get("to_node_id"):
                        nodes.append(n)
        return {"nodes": nodes, "edges": edges}

    def get_graph_stats(self, graph_id):
        return {"node_count": len(self.nodes), "edge_count": len(self.edges)}


def make_graph():
    c = FakeClient()
    return ProximaDBGraph(c, "g1"), c


# ---------------------------------------------------------------------------
# Dataclasses
# ---------------------------------------------------------------------------


def test_graphnode_to_dict():
    n = GraphNode(id="a", labels=["L"], properties={"x": 1})
    assert n.to_dict() == {"id": "a", "labels": ["L"], "properties": {"x": 1}}


def test_graphedge_to_dict_with_and_without_weight():
    e = GraphEdge(id="e1", from_node="a", to_node="b", edge_type="CALLS")
    d = e.to_dict()
    assert "weight" not in d
    e2 = GraphEdge(id="e2", from_node="a", to_node="b", edge_type="CALLS", weight=2.5)
    assert e2.to_dict()["weight"] == 2.5


def test_graphpath_and_queryresult_defaults():
    p = GraphPath()
    assert p.nodes == [] and p.total_weight == 0.0
    r = GraphQueryResult()
    assert r.nodes == [] and r.edges == [] and r.paths == [] and r.stats == {}


def test_create_graph_api_factory():
    c = FakeClient()
    g = create_graph_api(c, "gx")
    assert isinstance(g, ProximaDBGraph)
    assert g._graph_id == "gx"


# ---------------------------------------------------------------------------
# normalize helpers
# ---------------------------------------------------------------------------


def test_normalize_node_variants():
    assert ProximaDBGraph._normalize_node(None) is None
    gn = GraphNode(id="x")
    assert ProximaDBGraph._normalize_node(gn) is gn
    fromdict = ProximaDBGraph._normalize_node(
        {"id": "d", "labels": ["A"], "properties": {"k": 1}}
    )
    assert fromdict.id == "d" and fromdict.labels == ["A"]

    class Obj:
        id = "o"
        labels = ["B"]
        properties = {"p": 2}

    fromobj = ProximaDBGraph._normalize_node(Obj())
    assert fromobj.id == "o" and fromobj.properties == {"p": 2}


def test_normalize_edge_variants():
    assert ProximaDBGraph._normalize_edge(None) is None
    ge = GraphEdge(id="e", from_node="a", to_node="b", edge_type="T")
    assert ProximaDBGraph._normalize_edge(ge) is ge
    fromdict = ProximaDBGraph._normalize_edge(
        {"id": "e", "from_node_id": "a", "to_node_id": "b", "type": "CALLS", "weight": 1}
    )
    assert fromdict.from_node == "a" and fromdict.edge_type == "CALLS"

    class Obj:
        id = "e2"
        from_node_id = "x"
        to_node_id = "y"
        edge_type = "REL"
        properties = {}
        weight = None

    fromobj = ProximaDBGraph._normalize_edge(Obj())
    assert fromobj.from_node == "x" and fromobj.to_node == "y"


def test_is_internal_label():
    assert ProximaDBGraph._is_internal_label("__meta")
    assert not ProximaDBGraph._is_internal_label("Function")


def test_normalize_json_node():
    assert ProximaDBGraph._normalize_json_node({}) is None
    out = ProximaDBGraph._normalize_json_node(
        {"node_id": "n1", "labels": "Func", "file": "a.py"}
    )
    assert out["id"] == "n1"
    assert out["labels"] == ["Func"]
    assert out["properties"]["file"] == "a.py"
    out2 = ProximaDBGraph._normalize_json_node({"key": "k1", "type": "Class"})
    assert out2["labels"] == ["Class"]
    out3 = ProximaDBGraph._normalize_json_node({"id": "i"})
    assert out3["labels"] == []


def test_normalize_json_edge():
    assert ProximaDBGraph._normalize_json_edge({}, 0) is None
    assert ProximaDBGraph._normalize_json_edge({"from": "a"}, 0) is None
    out = ProximaDBGraph._normalize_json_edge(
        {"source": "a", "target": "b", "line": 9}, 3
    )
    assert out["from_node_id"] == "a" and out["to_node_id"] == "b"
    assert out["edge_type"] == "RELATED_TO"
    assert out["properties"]["line"] == 9
    assert out["id"].startswith("edge_3_")
    out2 = ProximaDBGraph._normalize_json_edge(
        {"id": "E", "src": "a", "dst": "b", "label": "CALLS", "weight": 4}, 0
    )
    assert out2["id"] == "E" and out2["edge_type"] == "CALLS" and out2["weight"] == 4


# ---------------------------------------------------------------------------
# batch create
# ---------------------------------------------------------------------------


def test_batch_create_nodes_empty():
    g, _ = make_graph()
    assert g.batch_create_nodes([]) == {"success": True, "created": 0}


def test_batch_create_nodes_objects_and_dicts():
    g, c = make_graph()
    res = g.batch_create_nodes(
        [
            GraphNode(id="a", labels=["F"]),
            {"id": "b", "labels": ["F"], "properties": {"n": 1}},
        ],
        batch_size=1,
    )
    assert res["success"] is True
    assert res["created"] == 2
    assert len(c.created_nodes) == 2


def test_batch_create_nodes_failure():
    g, c = make_graph()
    c.fail_create_node = True
    res = g.batch_create_nodes([{"id": "a"}], batch_size=1)
    assert res["success"] is False
    assert res["failed"] == 1
    assert res["errors"]


def test_batch_create_edges_empty():
    g, _ = make_graph()
    assert g.batch_create_edges([]) == {"success": True, "created": 0}


def test_batch_create_edges_ok():
    g, c = make_graph()
    res = g.batch_create_edges(
        [
            GraphEdge(id="e1", from_node="a", to_node="b", edge_type="CALLS"),
            {"from": "b", "to": "c", "type": "CALLS"},
        ],
        batch_size=10,
    )
    assert res["success"] is True
    assert res["created"] == 2
    assert len(c.created_edges) == 2


def test_batch_create_edges_missing_fields():
    g, _ = make_graph()
    res = g.batch_create_edges([{"from": "a"}], batch_size=10)
    assert res["success"] is False
    assert res["failed"] == 1


# ---------------------------------------------------------------------------
# query_cypher / _parse_cypher / traversal
# ---------------------------------------------------------------------------


def test_parse_cypher_full():
    g, _ = make_graph()
    parsed = g._parse_cypher(
        'MATCH (c:Function {name: "main"})-[r:CALLS]->(f:Function) WHERE c.kind = "x" RETURN c'
    )
    assert parsed["match"] is True
    assert parsed["start_labels"] == ["Function"]
    assert parsed["traversal"]["type"] == "CALLS"
    assert parsed["where"] is True
    assert parsed["start_properties"]["name"] == "main"
    assert parsed["start_properties"]["kind"] == "x"


def test_query_cypher_no_match():
    g, _ = make_graph()
    res = g.query_cypher("RETURN 1")
    assert isinstance(res, GraphQueryResult)
    assert res.nodes == []


def test_query_cypher_match_empty_start():
    g, c = make_graph()
    res = g.query_cypher("MATCH (c:Function) RETURN c")
    assert res.nodes == []


def test_query_cypher_with_traversal():
    g, c = make_graph()
    c.nodes = [
        {"id": "main", "labels": ["Function"], "properties": {"name": "main"}},
        {"id": "parse", "labels": ["Function"], "properties": {"name": "parse"}},
    ]
    c.edges = [
        {"id": "e1", "from_node_id": "main", "to_node_id": "parse", "edge_type": "CALLS"}
    ]
    res = g.query_cypher(
        "MATCH (c:Function)-[r:CALLS]->(f:Function) WHERE c.name = 'main' RETURN c, f"
    )
    assert any(n.id == "parse" for n in res.nodes)
    assert any(e.edge_type == "CALLS" for e in res.edges)


def test_execute_traversal_handles_exception():
    g, c = make_graph()
    c.support_traverse = False
    res = g._execute_traversal(["x"], {"traversal": None})
    assert res.nodes == [] and res.edges == []


# ---------------------------------------------------------------------------
# get_node_by_id / _get_node_raw
# ---------------------------------------------------------------------------


def test_get_node_by_id_direct():
    g, c = make_graph()
    c.nodes = [{"id": "a", "labels": ["F"], "properties": {}}]
    node = g.get_node_by_id("a")
    assert node.id == "a"


def test_get_node_by_id_fallback_scan():
    g, c = make_graph()
    c.support_get_node = False
    c.nodes = [{"id": "z", "labels": [], "properties": {}}]
    node = g.get_node_by_id("z")
    assert node.id == "z"


def test_get_node_by_id_missing():
    g, c = make_graph()
    c.support_get_node = False
    assert g.get_node_by_id("nope") is None


# ---------------------------------------------------------------------------
# edges raw helpers / fallbacks
# ---------------------------------------------------------------------------


def test_get_outgoing_edges_raw_fallback_to_traverse():
    g, c = make_graph()
    c.support_outgoing = False
    c.nodes = [
        {"id": "a", "labels": [], "properties": {}},
        {"id": "b", "labels": [], "properties": {}},
    ]
    c.edges = [{"id": "e", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"}]
    out = g._get_outgoing_edges_raw("a")
    # traverse fallback returns normalized to_dict() with from_node/to_node keys
    assert any(e.get("to_node") == "b" for e in out)


def test_get_outgoing_edges_raw_both_fail():
    g, c = make_graph()
    c.support_outgoing = False
    c.support_traverse = False
    assert g._get_outgoing_edges_raw("a") == []


def test_get_incoming_edges_raw_fallback():
    g, c = make_graph()
    c.support_incoming = False
    c.nodes = [
        {"id": "a", "labels": [], "properties": {}},
        {"id": "b", "labels": [], "properties": {}},
    ]
    c.edges = [{"id": "e", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"}]
    inc = g._get_incoming_edges_raw("b")
    # fallback path collects normalized edges (to_dict) -> from_node/to_node keys
    assert any(e.get("from_node") == "a" for e in inc)


# ---------------------------------------------------------------------------
# find_callers
# ---------------------------------------------------------------------------


def test_find_callers_depth1():
    g, c = make_graph()
    c.nodes = [
        {"id": "caller", "labels": ["Function"], "properties": {"name": "caller"}},
        {"id": "target", "labels": ["Function"], "properties": {"name": "target"}},
    ]
    c.edges = [
        {"id": "e", "from_node_id": "caller", "to_node_id": "target", "edge_type": "CALLS"}
    ]
    callers = g.find_callers("target")
    assert [n.id for n in callers] == ["caller"]


def test_find_callers_multi_depth():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": ["F"], "properties": {}},
        {"id": "b", "labels": ["F"], "properties": {}},
        {"id": "target", "labels": ["F"], "properties": {}},
    ]
    c.edges = [
        {"id": "e1", "from_node_id": "b", "to_node_id": "target", "edge_type": "CALLS"},
        {"id": "e2", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"},
    ]
    callers = g.find_callers("target", max_depth=2)
    ids = {n.id for n in callers}
    assert "b" in ids and "a" in ids


def test_find_callers_no_edge_type():
    g, c = make_graph()
    c.nodes = [
        {"id": "x", "labels": [], "properties": {}},
        {"id": "y", "labels": [], "properties": {}},
    ]
    c.edges = [{"id": "e", "from_node_id": "x", "to_node_id": "y", "edge_type": "ANY"}]
    callers = g.find_callers("y", edge_type="")
    assert [n.id for n in callers] == ["x"]


# ---------------------------------------------------------------------------
# get_all_nodes / get_nodes_by_file / find_nodes
# ---------------------------------------------------------------------------


def test_get_all_nodes_pagination_and_internal_filter():
    g, c = make_graph()
    c.nodes = [{"id": f"n{i}", "labels": ["F"], "properties": {}} for i in range(3)]
    c.nodes.append({"id": "internal", "labels": ["__sys"], "properties": {}})
    nodes = g.get_all_nodes(batch_size=2)
    ids = {n.id for n in nodes}
    assert "internal" not in ids
    assert len(ids) == 3
    all_nodes = g.get_all_nodes(batch_size=100, include_internal=True)
    assert any(n.id == "internal" for n in all_nodes)


def test_get_nodes_by_file():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": ["F"], "properties": {"file": "x.py"}},
        {"id": "b", "labels": ["F"], "properties": {"path": "y.py"}},
    ]
    res = g.get_nodes_by_file("x.py")
    assert [n.id for n in res] == ["a"]


def test_find_nodes_by_name_type_file():
    g, c = make_graph()
    c.nodes = [
        {
            "id": "a",
            "labels": ["Function"],
            "properties": {"name": "foo", "file": "x.py"},
        },
        {
            "id": "b",
            "labels": ["Function"],
            "properties": {"name": "foo", "file": "y.py"},
        },
    ]
    res = g.find_nodes(name="foo", type="Function", file="x.py")
    assert [n.id for n in res] == ["a"]


def test_find_nodes_no_filters():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": ["F"], "properties": {"qualified_name": "mod.foo"}},
    ]
    res = g.find_nodes()
    assert len(res) == 1


# ---------------------------------------------------------------------------
# search_symbols
# ---------------------------------------------------------------------------


def test_search_symbols_empty_query():
    g, _ = make_graph()
    assert g.search_symbols("  ") == []


def test_search_symbols_ranking():
    g, c = make_graph()
    c.nodes = [
        {"id": "exact", "labels": ["F"], "properties": {"name": "parse"}},
        {"id": "prefix", "labels": ["F"], "properties": {"name": "parser"}},
        {"id": "sub", "labels": ["F"], "properties": {"name": "xparsex"}},
        {"id": "doc", "labels": ["F"], "properties": {"docstring": "this can parse"}},
        {"id": "none", "labels": ["F"], "properties": {"name": "other"}},
    ]
    res = g.search_symbols("parse", limit=10)
    ids = [n.id for n in res]
    assert ids[0] == "exact"
    assert "none" not in ids
    assert "doc" in ids


def test_search_symbols_type_filter():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": ["Function"], "properties": {"name": "parse"}},
        {"id": "b", "labels": ["Class"], "properties": {"name": "parse"}},
    ]
    res = g.search_symbols("parse", symbol_types=["Function"])
    assert [n.id for n in res] == ["a"]


def test_search_symbols_qualified_and_signature():
    g, c = make_graph()
    c.nodes = [
        {"id": "q", "labels": ["F"], "properties": {"qualified_name": "mod.parse"}},
        {"id": "s", "labels": ["F"], "properties": {"signature": "def parse(x)"}},
    ]
    res = g.search_symbols("parse")
    ids = {n.id for n in res}
    assert "q" in ids and "s" in ids


# ---------------------------------------------------------------------------
# get_neighbors / get_all_edges
# ---------------------------------------------------------------------------


def test_get_neighbors_both_directions():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": [], "properties": {}},
        {"id": "b", "labels": [], "properties": {}},
        {"id": "c", "labels": [], "properties": {}},
    ]
    c.edges = [
        {"id": "e1", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"},
        {"id": "e2", "from_node_id": "c", "to_node_id": "a", "edge_type": "CALLS"},
    ]
    edges = g.get_neighbors("a", direction="both")
    sigs = {(e.from_node, e.to_node) for e in edges}
    assert ("a", "b") in sigs and ("c", "a") in sigs


def test_get_neighbors_out_only_multidepth():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": [], "properties": {}},
        {"id": "b", "labels": [], "properties": {}},
        {"id": "d", "labels": [], "properties": {}},
    ]
    c.edges = [
        {"id": "e1", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"},
        {"id": "e2", "from_node_id": "b", "to_node_id": "d", "edge_type": "CALLS"},
    ]
    edges = g.get_neighbors("a", edge_types=["CALLS"], direction="out", max_depth=2)
    sigs = {(e.from_node, e.to_node) for e in edges}
    assert ("a", "b") in sigs and ("b", "d") in sigs


def test_get_neighbors_in_only():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": [], "properties": {}},
        {"id": "b", "labels": [], "properties": {}},
    ]
    c.edges = [
        {"id": "e1", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"},
    ]
    edges = g.get_neighbors("b", direction="in")
    assert [(e.from_node, e.to_node) for e in edges] == [("a", "b")]


def test_get_all_edges():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": ["F"], "properties": {}},
        {"id": "b", "labels": ["F"], "properties": {}},
    ]
    c.edges = [
        {"id": "e1", "from_node_id": "a", "to_node_id": "b", "edge_type": "CALLS"},
    ]
    edges = g.get_all_edges(edge_types=["CALLS"])
    assert len(edges) == 1
    assert edges[0].edge_type == "CALLS"


# ---------------------------------------------------------------------------
# import_graph_json
# ---------------------------------------------------------------------------


def test_import_graph_json_dict():
    g, c = make_graph()
    payload = {
        "nodes": [{"id": "n1", "labels": ["F"]}],
        "edges": [{"source": "n1", "target": "n2", "type": "CALLS"}],
    }
    res = g.import_graph_json(payload)
    assert res["node_count"] == 1
    assert res["edge_count"] == 1
    assert res["success"] is True


def test_import_graph_json_nested_graph_key():
    g, _ = make_graph()
    payload = {"graph": {"nodes": [{"id": "x"}], "edges": []}}
    res = g.import_graph_json(payload)
    assert res["node_count"] == 1
    assert res["edge_count"] == 0


def test_import_graph_json_from_file(tmp_path):
    g, _ = make_graph()
    p = tmp_path / "graph.json"
    p.write_text(json.dumps({"nodes": [{"id": "f1", "type": "Func"}], "edges": []}))
    res = g.import_graph_json(p)
    assert res["node_count"] == 1


# ---------------------------------------------------------------------------
# match_pattern
# ---------------------------------------------------------------------------


def test_match_pattern_node_only():
    g, c = make_graph()
    c.nodes = [
        {"id": "a", "labels": ["Function"], "properties": {"name": "main"}},
    ]
    res = g.match_pattern('(f:Function {name: "main"})')
    assert len(res) == 1
    assert "f" in res[0]
    assert res[0]["f"].id == "a"


def test_match_pattern_with_relationship_returns_empty():
    g, _ = make_graph()
    res = g.match_pattern("(a:Function)-[r:CALLS]->(b:Function)")
    assert res == []


def test_match_pattern_no_nodes():
    g, _ = make_graph()
    assert g.match_pattern("") == []


# ---------------------------------------------------------------------------
# stats
# ---------------------------------------------------------------------------


def test_get_stats():
    g, c = make_graph()
    c.nodes = [{"id": "a", "labels": [], "properties": {}}]
    stats = g.get_stats()
    assert stats["node_count"] == 1


if __name__ == "__main__":
    pytest.main([__file__, "-q"])
