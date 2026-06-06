"""Offline coverage for proximadb_sdk.protocols.rest_sync.ProximaDBClient.

These tests exercise the REST method bodies (request shaping + response
parsing + the canonical /api/v2 paths) with mocked transports — no server.
Two call styles are covered:
  * methods that go through ``client._make_request(method, path, **kw)``
  * methods that go through ``client._http_client.<verb>(path, json=...)``

Besides driving coverage, the path assertions act as a regression guard for
the API-standardization repath (everything must hit /api/v2/*).
"""

from __future__ import annotations

import pytest

from proximadb_sdk.protocols.rest_sync import ProximaDBClient

# A permissive response body that satisfies the ``.get(...)``-based parsers and
# the light response transforms used across the graph/query/collection methods.
_DEFAULT_BODY = {
    "nodes": [],
    "edges": [],
    "paths": [],
    "results": [],
    "data": {},
    "id": "x",
    "success": True,
    "stats": {},
    "count": 0,
    "node": {"id": "n1"},
    "edge": {"id": "e1"},
    "graph": {"graph_id": "default"},
    "graphs": [],
    "collections": [],
    "rows": [],
    "columns": [],
    "schema": {},
    "status": "ok",
    "total": 0,
    "has_more": False,
}


class FakeResp:
    def __init__(self, data=None, status=200):
        self._d = dict(_DEFAULT_BODY)
        if data:
            self._d.update(data)
        self.status_code = status
        self.headers = {}
        self.text = "{}"
        self.content = b"{}"

    def json(self):
        return self._d

    def raise_for_status(self):
        return None


class FakeHttpClient:
    """Stand-in for the httpx client used by the graph methods."""

    def __init__(self):
        self.calls = []

    def _record(self, verb, path, **kw):
        self.calls.append((verb, path, kw))
        return FakeResp()

    def get(self, path, **kw):
        return self._record("GET", path, **kw)

    def post(self, path, **kw):
        return self._record("POST", path, **kw)

    def put(self, path, **kw):
        return self._record("PUT", path, **kw)

    def delete(self, path, **kw):
        return self._record("DELETE", path, **kw)


@pytest.fixture
def client(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    captured = {"req": [], "http": FakeHttpClient()}

    def fake_make_request(method, endpoint, **kwargs):
        captured["req"].append((method, endpoint, kwargs))
        return FakeResp(kwargs.get("_resp"))

    monkeypatch.setattr(c, "_make_request", fake_make_request)
    monkeypatch.setattr(c, "_http_client", captured["http"])
    c._captured = captured  # type: ignore[attr-defined]
    return c


def _req_paths(client):
    return [p for _, p, _ in client._captured["req"]]


def _http_paths(client):
    return [p for _, p, _ in client._captured["http"].calls]


# ---- query facade (via _make_request) ----------------------------------------


def test_execute_query_hits_v2_query(client):
    out = client.execute_query("SELECT 1", language="uql", collection="c", limit=5)
    assert out["success"] is True
    assert _req_paths(client) == ["/api/v2/query"]


def test_explain_query_hits_v2_query_explain(client):
    client.explain_query("SELECT 1", language="uql", collection="c")
    assert _req_paths(client) == ["/api/v2/query/explain"]


def test_execute_uql_aql_federated_sql(client):
    client.execute_uql("q")
    client.execute_aql("q")
    client.execute_federated("q")
    # all route through execute_query -> /api/v2/query
    assert _req_paths(client) == ["/api/v2/query"] * 3


# ---- graph nodes/edges/traverse (via _http_client) ---------------------------


def test_create_node_path_and_payload(client):
    client.create_node("n1", ["L"], {"k": "v"}, embedding=[0.1, 0.2])
    verb, path, kw = client._captured["http"].calls[-1]
    assert verb == "POST"
    assert path == "/api/v2/graphs/default/nodes"
    assert kw["json"]["node"]["id"] == "n1"
    assert kw["json"]["node"]["embedding"] == [0.1, 0.2]


def test_create_node_custom_graph(client):
    client.create_node("n2", ["L"], graph_id="g7")
    assert _http_paths(client)[-1] == "/api/v2/graphs/g7/nodes"


def test_create_edge_path_and_payload(client):
    client.create_edge("e1", "n1", "n2", "REL", {"p": 1}, weight=2.5)
    verb, path, kw = client._captured["http"].calls[-1]
    assert verb == "POST"
    assert path == "/api/v2/graphs/default/edges"
    assert kw["json"]["edge"]["weight"] == 2.5


def test_traverse_graph_path(client):
    client.traverse_graph("n1", max_depth=2, edge_types=["R"], algorithm="dfs", limit=10)
    assert _http_paths(client)[-1] == "/api/v2/graphs/default/traverse"


def test_query_nodes_and_edges(client):
    client.query_nodes(labels=["L"], graph_id="default")
    client.query_edges(edge_type="R", graph_id="default")
    paths = _http_paths(client)
    assert "/api/v2/graphs/default/query/nodes" in paths
    assert "/api/v2/graphs/default/query/edges" in paths


def test_get_node_and_delete_node(client):
    client.get_node("n1", graph_id="default")
    client.delete_node("n1", graph_id="default")
    paths = _http_paths(client)
    assert any(p == "/api/v2/graphs/default/nodes/n1" for p in paths)


def test_get_outgoing_and_incoming_edges(client):
    # Both delegate to query_edges -> /query/edges with a node filter.
    client.get_outgoing_edges("n1", graph_id="default")
    client.get_incoming_edges("n1", graph_id="default")
    paths = _http_paths(client)
    assert paths and all(p == "/api/v2/graphs/default/query/edges" for p in paths)


# ---- graph collection lifecycle ----------------------------------------------


def test_create_get_list_delete_graph(client):
    client.create_graph("default")
    client.get_graph("default")
    client.list_graphs()
    client.delete_graph("default")
    client.get_graph_stats("default")
    paths = _http_paths(client)
    assert any(p == "/api/v2/graphs" for p in paths)
    assert any(p == "/api/v2/graphs/default" for p in paths)
    assert any(p == "/api/v2/graphs/default/stats" for p in paths)
