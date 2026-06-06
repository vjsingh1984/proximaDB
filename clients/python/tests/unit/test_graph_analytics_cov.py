"""Offline unit tests for proximadb_sdk.graph_analytics."""

from unittest.mock import MagicMock

import pytest

from proximadb_sdk.graph_analytics import (
    AlgorithmConfig,
    AlgorithmResult,
    GraphAlgorithm,
    GraphAnalytics,
    GraphPattern,
    PatternElement,
    PatternMatchMode,
    PatternMatchResult,
    RelationshipPattern,
    SemanticTraversalConfig,
    SemanticTraversalResult,
    TraversalDirection,
    node,
    relationship,
)


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeMakeRequestClient:
    """Client exposing _make_request (the preferred internal path)."""

    def __init__(self, response):
        self._base_url = "http://testserver"
        self._response = response
        self.calls = []

    def _make_request(self, method, path, **kwargs):
        self.calls.append((method, path, kwargs))
        return self._response


class BareClient:
    """Client without _make_request — falls back to requests."""

    def __init__(self):
        self.url = "http://testserver"


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------


def test_enum_values():
    assert GraphAlgorithm.PAGERANK.value == "pagerank"
    assert TraversalDirection.BOTH.value == "both"
    assert PatternMatchMode.FUZZY.value == "fuzzy"


# ---------------------------------------------------------------------------
# Config dataclasses
# ---------------------------------------------------------------------------


def test_algorithm_config_defaults_to_dict():
    cfg = AlgorithmConfig()
    d = cfg.to_dict()
    assert d["damping_factor"] == 0.85
    assert d["max_iterations"] == 100
    assert d["normalized"] is True
    assert d["random_seed"] is None
    assert set(d) == {
        "damping_factor",
        "max_iterations",
        "convergence_threshold",
        "resolution",
        "random_seed",
        "normalized",
        "weight_property",
        "max_depth",
    }


def test_algorithm_config_custom():
    cfg = AlgorithmConfig(
        damping_factor=0.5,
        max_iterations=10,
        resolution=2.0,
        random_seed=7,
        normalized=False,
        weight_property="w",
        max_depth=3,
    )
    d = cfg.to_dict()
    assert d["damping_factor"] == 0.5
    assert d["random_seed"] == 7
    assert d["weight_property"] == "w"
    assert d["max_depth"] == 3


def test_semantic_traversal_config_to_dict():
    cfg = SemanticTraversalConfig(
        similarity_threshold=0.9,
        vector_field="vec",
        max_depth=5,
        direction=TraversalDirection.INCOMING,
        edge_types=["A", "B"],
        limit=50,
        include_scores=False,
        include_paths=True,
        node_label_filter=["L"],
        property_filters={"k": 1},
    )
    d = cfg.to_dict()
    assert d["direction"] == "incoming"
    assert d["edge_types"] == ["A", "B"]
    assert d["include_paths"] is True
    assert d["node_label_filter"] == ["L"]
    assert d["property_filters"] == {"k": 1}


def test_semantic_traversal_config_defaults():
    d = SemanticTraversalConfig().to_dict()
    assert d["direction"] == "outgoing"
    assert d["similarity_threshold"] == 0.7
    assert d["vector_field"] == "embedding"


# ---------------------------------------------------------------------------
# Pattern elements / cypher
# ---------------------------------------------------------------------------


def test_pattern_element_to_cypher_variants():
    assert PatternElement("a").to_cypher() == "(a)"
    assert PatternElement("a", "User").to_cypher() == "(a:User)"
    out = PatternElement("a", "User", {"name": "bob"}).to_cypher()
    assert out.startswith("(a:User")
    assert '"bob"' in out
    out2 = PatternElement("x", properties={"n": 1}).to_cypher()
    assert out2.startswith("(x ")


def test_relationship_pattern_outgoing():
    rp = RelationshipPattern(
        source=PatternElement("a"),
        target=PatternElement("b"),
        relationship_type="FOLLOWS",
        relationship_var="r",
    )
    cy = rp.to_cypher()
    assert cy.startswith("(a)-[r:FOLLOWS]-")
    assert cy.endswith(">(b)")


def test_relationship_pattern_incoming():
    rp = RelationshipPattern(
        source=PatternElement("a"),
        target=PatternElement("b"),
        direction=TraversalDirection.INCOMING,
    )
    cy = rp.to_cypher()
    assert cy.startswith("<(a)")
    assert cy.endswith("(b)")


def test_relationship_pattern_both_direction():
    rp = RelationshipPattern(
        source=PatternElement("a"),
        target=PatternElement("b"),
        direction=TraversalDirection.BOTH,
    )
    cy = rp.to_cypher()
    assert "(a)" in cy and "(b)" in cy
    assert not cy.endswith(">(b)")


def test_relationship_pattern_hops_equal():
    rp = RelationshipPattern(
        source=PatternElement("a"),
        target=PatternElement("b"),
        min_hops=2,
        max_hops=2,
    )
    assert "*2" in rp.to_cypher()


def test_relationship_pattern_hops_range_and_props():
    rp = RelationshipPattern(
        source=PatternElement("a"),
        target=PatternElement("b"),
        min_hops=1,
        max_hops=3,
        properties={"since": 2020},
    )
    cy = rp.to_cypher()
    assert "*1..3" in cy
    assert "since" in cy


# ---------------------------------------------------------------------------
# GraphPattern builder
# ---------------------------------------------------------------------------


def test_graph_pattern_builder_chain():
    p = (
        GraphPattern()
        .match(PatternElement("u", "User"))
        .relationship(
            PatternElement("a"),
            PatternElement("b"),
            rel_type="FOLLOWS",
            direction=TraversalDirection.OUTGOING,
            min_hops=1,
            max_hops=2,
        )
        .where("a.age > 18")
        .returns("a", "b")
        .order("a.name")
        .with_limit(5)
    )
    d = p.to_dict()
    assert len(d["patterns"]) == 2
    assert d["where"] == ["a.age > 18"]
    assert d["return"] == ["a", "b"]
    assert d["order_by"] == "a.name"
    assert d["limit"] == 5


def test_graph_pattern_to_dict_str_fallback():
    p = GraphPattern()
    p.patterns.append("raw_string_pattern")
    d = p.to_dict()
    assert d["patterns"] == ["raw_string_pattern"]


# ---------------------------------------------------------------------------
# Result dataclasses
# ---------------------------------------------------------------------------


def test_algorithm_result_from_dict_full():
    data = {
        "algorithm": "louvain",
        "node_scores": {"a": 1.0},
        "communities": {"a": 0},
        "paths": [["a", "b"]],
        "components": [["a"]],
        "statistics": {"global_coefficient": 0.5},
        "execution_time_ms": 12.5,
    }
    r = AlgorithmResult.from_dict(data)
    assert r.algorithm == GraphAlgorithm.LOUVAIN
    assert r.node_scores == {"a": 1.0}
    assert r.execution_time_ms == 12.5


def test_algorithm_result_from_dict_defaults():
    r = AlgorithmResult.from_dict({})
    assert r.algorithm == GraphAlgorithm.PAGERANK
    assert r.node_scores is None
    assert r.execution_time_ms == 0


def test_semantic_and_pattern_result_construction():
    s = SemanticTraversalResult(nodes=[{"id": "n"}])
    assert s.total_count == 0
    pm = PatternMatchResult(matches=[{"a": 1}], total_count=1)
    assert pm.matches == [{"a": 1}]


# ---------------------------------------------------------------------------
# Convenience functions
# ---------------------------------------------------------------------------


def test_node_helper():
    el = node("u", "User", active=True)
    assert el.variable == "u"
    assert el.label == "User"
    assert el.properties == {"active": True}
    assert node("u").properties is None


def test_relationship_helper():
    rel = relationship(node("a"), node("b"), "FOLLOWS", weight=5)
    assert rel.relationship_type == "FOLLOWS"
    assert rel.properties == {"weight": 5}
    assert relationship(node("a"), node("b")).properties is None


# ---------------------------------------------------------------------------
# GraphAnalytics construction
# ---------------------------------------------------------------------------


def test_init_base_url_from_make_request_client():
    c = FakeMakeRequestClient({})
    ga = GraphAnalytics(c)
    assert ga._base_url == "http://testserver"


def test_init_base_url_from_url_attr():
    ga = GraphAnalytics(BareClient())
    assert ga._base_url == "http://testserver"


def test_init_base_url_default():
    class NoAttr:
        pass

    ga = GraphAnalytics(NoAttr())
    assert ga._base_url == "http://localhost:5678"


# ---------------------------------------------------------------------------
# run_algorithm + wrappers (via _make_request)
# ---------------------------------------------------------------------------


def test_run_algorithm_make_request():
    c = FakeMakeRequestClient(
        {"algorithm": "pagerank", "node_scores": {"a": 0.5, "b": 0.3}}
    )
    ga = GraphAnalytics(c)
    r = ga.run_algorithm("g1", GraphAlgorithm.PAGERANK, AlgorithmConfig(), ["a", "b"])
    assert r.node_scores == {"a": 0.5, "b": 0.3}
    method, path, kwargs = c.calls[0]
    assert method == "POST"
    assert path == "/v1/graphs/g1/algorithms/pagerank"
    assert kwargs["json"]["node_subset"] == ["a", "b"]


def test_run_algorithm_fallback_requests(monkeypatch):
    captured = {}

    class FakeResp:
        def json(self):
            return {"algorithm": "pagerank", "node_scores": {"x": 1.0}}

    def fake_post(url, json=None):
        captured["url"] = url
        captured["json"] = json
        return FakeResp()

    import requests

    monkeypatch.setattr(requests, "post", fake_post)
    ga = GraphAnalytics(BareClient())
    r = ga.run_algorithm("g1", GraphAlgorithm.PAGERANK)
    assert r.node_scores == {"x": 1.0}
    assert captured["url"].endswith("/v1/graphs/g1/algorithms/pagerank")


def test_pagerank_wrapper():
    c = FakeMakeRequestClient({"algorithm": "pagerank", "node_scores": {"a": 0.9}})
    ga = GraphAnalytics(c)
    scores = ga.pagerank("g", damping_factor=0.7, max_iterations=5)
    assert scores == {"a": 0.9}


def test_pagerank_empty_scores():
    c = FakeMakeRequestClient({"algorithm": "pagerank"})
    ga = GraphAnalytics(c)
    assert ga.pagerank("g") == {}


@pytest.mark.parametrize(
    "ctype,expected_algo",
    [
        ("betweenness", "betweenness_centrality"),
        ("closeness", "closeness_centrality"),
        ("degree", "degree_centrality"),
        ("eigenvector", "eigenvector_centrality"),
    ],
)
def test_centrality_types(ctype, expected_algo):
    c = FakeMakeRequestClient({"algorithm": expected_algo, "node_scores": {"a": 1.0}})
    ga = GraphAnalytics(c)
    scores = ga.centrality("g", ctype, normalized=False, weight_property="w")
    assert scores == {"a": 1.0}
    _, path, _ = c.calls[0]
    assert path.endswith(expected_algo)


def test_centrality_unknown_raises():
    ga = GraphAnalytics(FakeMakeRequestClient({}))
    with pytest.raises(ValueError, match="Unknown centrality type"):
        ga.centrality("g", "bogus")


def test_centrality_empty():
    c = FakeMakeRequestClient({"algorithm": "degree_centrality"})
    ga = GraphAnalytics(c)
    assert ga.centrality("g", "degree") == {}


def test_community_detection_louvain():
    c = FakeMakeRequestClient({"algorithm": "louvain", "communities": {"a": 0, "b": 1}})
    ga = GraphAnalytics(c)
    out = ga.community_detection("g", "louvain", resolution=1.5, random_seed=42)
    assert out == {"a": 0, "b": 1}


def test_community_detection_label_propagation_empty():
    c = FakeMakeRequestClient({"algorithm": "label_propagation"})
    ga = GraphAnalytics(c)
    assert ga.community_detection("g", "label_propagation") == {}
    _, path, _ = c.calls[0]
    assert path.endswith("label_propagation")


def test_connected_components_weak():
    c = FakeMakeRequestClient(
        {"algorithm": "connected_components", "components": [["a", "b"], ["c"]]}
    )
    ga = GraphAnalytics(c)
    comps = ga.connected_components("g")
    assert comps == [["a", "b"], ["c"]]


def test_connected_components_strong_empty():
    c = FakeMakeRequestClient({"algorithm": "strongly_connected"})
    ga = GraphAnalytics(c)
    assert ga.connected_components("g", strongly_connected=True) == []
    _, path, _ = c.calls[0]
    assert path.endswith("strongly_connected")


# ---------------------------------------------------------------------------
# shortest_path
# ---------------------------------------------------------------------------


def test_shortest_path_uses_client_method():
    client = MagicMock()
    client.graph_shortest_path.return_value = {"path": ["a", "x", "b"]}
    ga = GraphAnalytics(client)
    path = ga.shortest_path("g", "a", "b", edge_types=["E"])
    assert path == ["a", "x", "b"]
    client.graph_shortest_path.assert_called_once_with(
        graph_id="g", start_node="a", end_node="b", edge_types=["E"]
    )


def test_shortest_path_client_method_returns_none():
    client = MagicMock()
    client.graph_shortest_path.return_value = None
    ga = GraphAnalytics(client)
    assert ga.shortest_path("g", "a", "b") is None


def test_shortest_path_fallback_algorithm():
    c = FakeMakeRequestClient(
        {"algorithm": "shortest_path", "paths": [["a", "b", "c"]]}
    )
    ga = GraphAnalytics(c)  # no graph_shortest_path attr
    assert ga.shortest_path("g", "a", "c") == ["a", "b", "c"]


def test_shortest_path_fallback_no_path():
    c = FakeMakeRequestClient({"algorithm": "shortest_path"})
    ga = GraphAnalytics(c)
    assert ga.shortest_path("g", "a", "c") is None


# ---------------------------------------------------------------------------
# semantic_traverse / semantic_neighbors
# ---------------------------------------------------------------------------


def test_semantic_traverse_make_request():
    c = FakeMakeRequestClient(
        {
            "nodes": [{"id": "n1"}],
            "edges": [{"s": "n1"}],
            "paths": [["n1"]],
            "scores": {"n1": 0.9},
            "total_count": 1,
            "execution_time_ms": 3.0,
        }
    )
    ga = GraphAnalytics(c)
    res = ga.semantic_traverse(
        "g",
        "n1",
        [0.1, 0.2],
        SemanticTraversalConfig(max_depth=2),
        collection_id="col",
    )
    assert isinstance(res, SemanticTraversalResult)
    assert res.nodes == [{"id": "n1"}]
    assert res.total_count == 1
    _, path, kwargs = c.calls[0]
    assert path == "/v1/graphs/g/semantic-traverse"
    assert kwargs["json"]["collection_id"] == "col"


def test_semantic_traverse_defaults_and_no_collection():
    c = FakeMakeRequestClient({"nodes": []})
    ga = GraphAnalytics(c)
    res = ga.semantic_traverse("g", "n1", [0.1])
    assert res.nodes == []
    assert res.edges is None
    _, _, kwargs = c.calls[0]
    assert "collection_id" not in kwargs["json"]


def test_semantic_traverse_fallback_requests(monkeypatch):
    class FakeResp:
        def json(self):
            return {"nodes": [{"id": "z"}], "total_count": 1}

    import requests

    monkeypatch.setattr(requests, "post", lambda url, json=None: FakeResp())
    ga = GraphAnalytics(BareClient())
    res = ga.semantic_traverse("g", "n1", [0.5])
    assert res.nodes == [{"id": "z"}]


def test_semantic_neighbors_with_vector():
    c = FakeMakeRequestClient({"nodes": [{"id": "nb1"}]})
    ga = GraphAnalytics(c)
    out = ga.semantic_neighbors(
        "g",
        "n",
        query_vector=[0.1],
        similarity_threshold=0.8,
        max_neighbors=3,
        edge_types=["E"],
    )
    assert out == [{"id": "nb1"}]
    _, _, kwargs = c.calls[0]
    assert kwargs["json"]["query_vector"] == [0.1]
    assert kwargs["json"]["config"]["limit"] == 3


def test_semantic_neighbors_without_vector():
    c = FakeMakeRequestClient({"nodes": []})
    ga = GraphAnalytics(c)
    out = ga.semantic_neighbors("g", "n")
    assert out == []
    _, _, kwargs = c.calls[0]
    assert kwargs["json"]["query_vector"] == []


# ---------------------------------------------------------------------------
# pattern_match
# ---------------------------------------------------------------------------


def test_pattern_match_make_request():
    c = FakeMakeRequestClient(
        {"matches": [{"a": 1}], "total_count": 1, "execution_time_ms": 2.0}
    )
    ga = GraphAnalytics(c)
    pattern = GraphPattern().match(node("u", "User")).returns("u")
    res = ga.pattern_match("g", pattern, mode=PatternMatchMode.PARTIAL, limit=10)
    assert isinstance(res, PatternMatchResult)
    assert res.matches == [{"a": 1}]
    assert pattern.limit == 10
    _, path, kwargs = c.calls[0]
    assert path == "/v1/graphs/g/pattern-match"
    assert kwargs["json"]["mode"] == "partial"


def test_pattern_match_fallback_requests(monkeypatch):
    class FakeResp:
        def json(self):
            return {"matches": [{"x": 2}]}

    import requests

    monkeypatch.setattr(requests, "post", lambda url, json=None: FakeResp())
    ga = GraphAnalytics(BareClient())
    res = ga.pattern_match("g", GraphPattern())
    assert res.matches == [{"x": 2}]


# ---------------------------------------------------------------------------
# find_triangles / clustering_coefficient
# ---------------------------------------------------------------------------


def test_find_triangles_all():
    c = FakeMakeRequestClient(
        {"algorithm": "triangle_count", "paths": [["a", "b", "c"]]}
    )
    ga = GraphAnalytics(c)
    assert ga.find_triangles("g") == [["a", "b", "c"]]
    _, _, kwargs = c.calls[0]
    assert "node_subset" not in kwargs["json"]


def test_find_triangles_node_subset():
    c = FakeMakeRequestClient({"algorithm": "triangle_count"})
    ga = GraphAnalytics(c)
    assert ga.find_triangles("g", node_id="a") == []
    _, _, kwargs = c.calls[0]
    assert kwargs["json"]["node_subset"] == ["a"]


def test_clustering_coefficient_global():
    c = FakeMakeRequestClient(
        {
            "algorithm": "clustering_coefficient",
            "statistics": {"global_coefficient": 0.42},
        }
    )
    ga = GraphAnalytics(c)
    assert ga.clustering_coefficient("g") == 0.42


def test_clustering_coefficient_global_no_stats():
    c = FakeMakeRequestClient({"algorithm": "clustering_coefficient"})
    ga = GraphAnalytics(c)
    assert ga.clustering_coefficient("g") == 0.0


def test_clustering_coefficient_node():
    c = FakeMakeRequestClient(
        {"algorithm": "clustering_coefficient", "node_scores": {"a": 0.75}}
    )
    ga = GraphAnalytics(c)
    assert ga.clustering_coefficient("g", node_id="a") == 0.75


def test_clustering_coefficient_node_no_scores():
    c = FakeMakeRequestClient({"algorithm": "clustering_coefficient"})
    ga = GraphAnalytics(c)
    assert ga.clustering_coefficient("g", node_id="a") == 0.0
