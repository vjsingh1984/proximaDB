from proximadb_sdk.protocols import rest_sync


class FakeResponse:
    def __init__(self, data):
        self._data = data

    def json(self):
        return self._data


def test_graph_shortest_path_headers(monkeypatch):
    client = rest_sync.ProximaDBClient(url="http://localhost:5678")

    captured = {}

    def fake_make_request(method, endpoint, **kwargs):
        captured["headers"] = kwargs.get("headers", {})
        assert endpoint == "/api/v1/graph/shortest_path"
        return FakeResponse({"ok": True})

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    resp = client.graph_shortest_path(
        start_node_id="n1",
        target_node_id="n8",
        enable_prefetch=True,
        prefetch_budget=7,
    )
    assert resp["ok"] is True
    assert captured["headers"]["x-graph-prefetch-enabled"] == "true"
    assert captured["headers"]["x-graph-prefetch-budget"] == "7"


def test_graph_traverse_headers(monkeypatch):
    client = rest_sync.ProximaDBClient(url="http://localhost:5678")

    captured = {}

    def fake_make_request(method, endpoint, **kwargs):
        captured["headers"] = kwargs.get("headers", {})
        assert endpoint == "/api/v1/graph/traverse"
        return FakeResponse({"ok": True})

    monkeypatch.setattr(client, "_make_request", fake_make_request)
    resp = client.graph_traverse(
        start_node_id="n1",
        enable_prefetch=False,
        prefetch_budget=10,
    )
    assert resp["ok"] is True
    assert captured["headers"]["x-graph-prefetch-enabled"] == "false"
    assert captured["headers"]["x-graph-prefetch-budget"] == "10"
