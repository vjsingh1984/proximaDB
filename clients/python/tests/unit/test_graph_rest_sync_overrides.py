import json
from proximadb_sdk.protocols.rest_sync import ProximaDBClient


class FakeResponse:
    def __init__(self, data):
        self._data = data

    def json(self):
        return self._data


def test_rest_sync_graph_shortest_path_headers_and_body(monkeypatch):
    client = ProximaDBClient(url="http://testserver")
    captured = {}

    def fake_make_request(method, endpoint, **kwargs):
        captured["method"] = method
        captured["path"] = endpoint
        captured["headers"] = kwargs.get("headers", {})
        captured["json"] = kwargs.get("json", {})
        return FakeResponse({"ok": True})

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    resp = client.graph_shortest_path(
        start_node_id="n1",
        target_node_id="n8",
        algorithm="DIJKSTRA",
        max_depth=6,
        enable_prefetch=True,
        prefetch_budget=12,
    )
    assert resp["ok"] is True
    assert captured["path"] == "/api/v1/graph/shortest_path"
    assert captured["headers"]["x-graph-prefetch-enabled"] == "true"
    assert captured["headers"]["x-graph-prefetch-budget"] == "12"
    assert captured["json"]["enable_prefetch"] is True
    assert captured["json"]["prefetch_budget"] == 12


def test_rest_sync_graph_traverse_headers_and_body(monkeypatch):
    client = ProximaDBClient(url="http://testserver")
    captured = {}

    def fake_make_request(method, endpoint, **kwargs):
        captured["method"] = method
        captured["path"] = endpoint
        captured["headers"] = kwargs.get("headers", {})
        captured["json"] = kwargs.get("json", {})
        return FakeResponse({"ok": True})

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    resp = client.graph_traverse(
        start_node_id="n1",
        max_depth=4,
        enable_prefetch=False,
        prefetch_budget=5,
    )
    assert resp["ok"] is True
    assert captured["path"] == "/api/v1/graph/traverse"
    assert captured["headers"]["x-graph-prefetch-enabled"] == "false"
    assert captured["headers"]["x-graph-prefetch-budget"] == "5"
    assert captured["json"]["enable_prefetch"] is False
    assert captured["json"]["prefetch_budget"] == 5
