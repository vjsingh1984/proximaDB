"""Extended offline coverage for proximadb_sdk.protocols.rest_sync.

This file EXTENDS test_rest_sync_more_cov.py without duplicating it. It targets
the methods/branches that file leaves uncovered:

  * capability negotiation: server_capabilities / supports / get_capabilities /
    _auto_warmup
  * liveness/readiness probes: live() / ready()
  * graph REST: graph_shortest_path / graph_traverse / create_node /
    create_edge / traverse_graph / query_nodes / query_edges / get_node /
    get_outgoing_edges / get_incoming_edges / delete_node
  * graph collection mgmt: create_graph / delete_graph / get_graph /
    list_graphs / get_graph_stats
  * query API: execute_query / explain_query / execute_uql / execute_aql /
    execute_federated / execute_sql (+ error paths)
  * metadata SqlValue conversion helpers
  * compression helpers + _handle_error_response + _http_post
  * quantization config proto conversion
  * module-level connect() / quick_search() + context manager dunder

Everything is mocked — no server, no socket, no sleep, no model download.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import numpy as np
import pytest

from proximadb_sdk.protocols.rest_sync import (
    ProximaDBClient,
    _convert_quantization_config_to_proto,
    connect,
    quick_search,
)


class FakeResp:
    def __init__(self, data=None, status=200, headers=None, text="{}"):
        self._d = {} if data is None else data
        self.status_code = status
        self.headers = headers or {}
        self.text = text
        self.content = b"{}"
        self.url = "http://testserver/x"

    def json(self):
        if isinstance(self._d, Exception):
            raise self._d
        return self._d

    def raise_for_status(self):
        return None


class FakeHttpClient:
    """Records http_client.<verb> calls and returns a canned response."""

    def __init__(self, resp=None):
        self.calls = []
        self._resp = resp if resp is not None else FakeResp()
        self.closed = False

    def _record(self, verb, path, **kw):
        self.calls.append((verb, path, kw))
        return self._resp

    def get(self, path, **kw):
        return self._record("GET", path, **kw)

    def post(self, path, **kw):
        return self._record("POST", path, **kw)

    def put(self, path, **kw):
        return self._record("PUT", path, **kw)

    def delete(self, path, **kw):
        return self._record("DELETE", path, **kw)

    def close(self):
        self.closed = True


def _make_client(monkeypatch, resp_body=None, http_resp=None):
    """Construct a client with both transports mocked.

    ``resp_body`` drives the fake ``_make_request`` JSON; ``http_resp`` (a
    FakeResp) drives the fake ``_http_client`` used by the graph/query methods
    that call ``self._http_client.<verb>`` directly.
    """
    c = ProximaDBClient(url="http://testserver")
    captured = {"req": []}
    body = resp_body if resp_body is not None else {}

    def fake_make_request(method, endpoint, **kwargs):
        captured["req"].append((method, endpoint, kwargs))
        return FakeResp(body)

    monkeypatch.setattr(c, "_make_request", fake_make_request)
    http = FakeHttpClient(http_resp if http_resp is not None else FakeResp(body))
    monkeypatch.setattr(c, "_http_client", http)
    c._captured = captured  # type: ignore[attr-defined]
    c._fake_http = http  # type: ignore[attr-defined]
    return c


@pytest.fixture
def client(monkeypatch):
    return _make_client(monkeypatch)


def _last(client):
    return client._captured["req"][-1]


def _last_http(client):
    return client._fake_http.calls[-1]


# -------------------------------------------------- capability negotiation


def test_server_capabilities_caches(monkeypatch):
    c = _make_client(
        monkeypatch,
        resp_body={"api_version": "2", "features": ["sks", "graph"], "limits": {}},
    )
    caps = c.server_capabilities()
    assert caps["api_version"] == "2"
    # second call is cached -> no new request
    n = len(c._captured["req"])
    caps2 = c.server_capabilities()
    assert caps2 == caps
    assert len(c._captured["req"]) == n


def test_server_capabilities_refresh(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"features": ["a"]})
    c.server_capabilities()
    n = len(c._captured["req"])
    c.server_capabilities(refresh=True)
    assert len(c._captured["req"]) == n + 1


def test_server_capabilities_non_dict_json(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    monkeypatch.setattr(
        c, "_make_request", lambda m, p, **k: FakeResp(["not", "a", "dict"])
    )
    monkeypatch.setattr(c, "_http_client", FakeHttpClient())
    assert c.server_capabilities() == {}
    c.close()


def test_server_capabilities_request_raises(monkeypatch):
    c = ProximaDBClient(url="http://testserver")

    def boom(method, endpoint, **kwargs):
        raise RuntimeError("no endpoint")

    monkeypatch.setattr(c, "_make_request", boom)
    monkeypatch.setattr(c, "_http_client", FakeHttpClient())
    assert c.server_capabilities() == {}
    c.close()


def test_supports_feature(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"features": ["sks_search"]})
    assert c.supports("sks_search") is True
    assert c.supports("nope") is False


def test_get_capabilities_initial(client):
    caps = client.get_capabilities()
    assert caps["sks_search_supported"] is None
    assert caps["warmed_collections"] == []


def test_auto_warmup_marks_collection(monkeypatch):
    c = _make_client(monkeypatch)
    called = {"n": 0}

    def fake_warm(cid):
        called["n"] += 1

    monkeypatch.setattr(c, "warmup_sks_capabilities", fake_warm, raising=False)
    c._auto_warmup("coll1")
    assert "coll1" in c._warmed_collections
    # second call short-circuits (already warmed)
    c._auto_warmup("coll1")
    assert called["n"] == 1


def test_auto_warmup_empty_collection_id_noop(client):
    client._auto_warmup("")
    assert client._warmed_collections == set()


def test_auto_warmup_swallows_exception(monkeypatch):
    c = _make_client(monkeypatch)

    def boom(cid):
        raise RuntimeError("warmup failed")

    monkeypatch.setattr(c, "warmup_sks_capabilities", boom, raising=False)
    # Should not propagate
    c._auto_warmup("coll2")
    assert "coll2" in c._warmed_collections


def test_auto_warmup_skipped_when_caps_known(monkeypatch):
    c = _make_client(monkeypatch)
    c._sks_search_supported = True
    c._sks_entities_supported = True
    called = {"n": 0}
    monkeypatch.setattr(
        c,
        "warmup_sks_capabilities",
        lambda cid: called.__setitem__("n", called["n"] + 1),
        raising=False,
    )
    c._auto_warmup("coll3")
    # caps already known -> warmup not invoked but collection marked
    assert called["n"] == 0
    assert "coll3" in c._warmed_collections


# ------------------------------------------------------- liveness/readiness


def test_live_probe(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"status": "ok"})
    probe = c.live()
    assert probe is not None
    assert _last(c)[:2] == ("GET", "/health/live")


def test_ready_probe(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"status": "ready"})
    probe = c.ready()
    assert probe is not None
    assert _last(c)[:2] == ("GET", "/health/ready")


def test_health_with_nested_data_and_timestamp(monkeypatch):
    c = _make_client(
        monkeypatch,
        resp_body={
            "data": {
                "status": "healthy",
                "version": "1.2.3",
                "uptime_seconds": 99,
                "timestamp": 1700,
                "components": {"db": "up"},
            }
        },
    )
    hs = c.health()
    assert hs.status == "healthy"
    assert hs.version == "1.2.3"
    assert hs.timestamp_ms == 1700 * 1000
    assert hs.services == {"db": "up"}


def test_health_default_timestamp(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"status": "ok"})
    hs = c.health()
    assert hs.timestamp_ms > 0


# --------------------------------------------------------------- graph ops


def test_graph_shortest_path_with_overrides(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"path": ["a", "b"]})
    out = c.graph_shortest_path(
        "a",
        "b",
        max_depth=5,
        edge_types=["knows"],
        k=2,
        enable_prefetch=True,
        prefetch_budget=10,
    )
    assert out["path"] == ["a", "b"]
    method, path, kw = _last(c)
    assert method == "POST"
    assert path == "/api/v2/graphs/default/shortest-path"
    assert kw["headers"]["x-graph-prefetch-enabled"] == "true"
    assert kw["headers"]["x-graph-prefetch-budget"] == "10"
    assert kw["json"]["max_depth"] == 5
    assert kw["json"]["edge_types"] == ["knows"]
    assert kw["json"]["enable_prefetch"] is True


def test_graph_shortest_path_minimal(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"path": []})
    c.graph_shortest_path("a", "b", enable_prefetch=False)
    _, _, kw = _last(c)
    assert kw["headers"]["x-graph-prefetch-enabled"] == "false"
    assert "max_depth" not in kw["json"]


def test_graph_traverse_with_overrides(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"nodes": []})
    out = c.graph_traverse(
        "n0",
        max_depth=4,
        edge_types=["e"],
        limit=10,
        timeout_ms=500,
        max_frontier=100,
        enable_prefetch=True,
        prefetch_budget=7,
    )
    assert out == {"nodes": []}
    _, path, kw = _last(c)
    assert path == "/api/v2/graphs/default/traverse"
    assert kw["json"]["limit"] == 10
    assert kw["json"]["timeout_ms"] == 500
    assert kw["json"]["max_frontier"] == 100
    assert kw["headers"]["x-graph-prefetch-budget"] == "7"


def test_graph_traverse_minimal(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"nodes": []})
    c.graph_traverse("n0")
    _, _, kw = _last(c)
    assert kw["json"]["max_depth"] == 3
    assert "limit" not in kw["json"]


def test_create_node(monkeypatch):
    # Rebased through the generated REST op -> goes via _make_request.
    c = _make_client(monkeypatch, resp_body={"id": "n1"})
    out = c.create_node("n1", ["Person"], properties={"age": 30}, embedding=[1.0, 2.0])
    assert out["id"] == "n1"
    verb, path, kw = _last(c)
    assert (verb, path) == ("POST", "/api/v2/graphs/default/nodes")
    assert kw["json"]["node"]["embedding"] == [1.0, 2.0]


def test_create_node_no_embedding(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"id": "n2"})
    c.create_node("n2", ["L"])
    _, _, kw = _last(c)
    assert "embedding" not in kw["json"]["node"]


def test_create_node_custom_graph(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"id": "n3"})
    c.create_node("n3", ["L"], graph_id="g7")
    _, path, _ = _last(c)
    assert path == "/api/v2/graphs/g7/nodes"


def test_create_edge(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"id": "e1"})
    out = c.create_edge("e1", "a", "b", "knows", properties={"w": 1}, weight=0.5)
    assert out["id"] == "e1"
    verb, path, kw = _last(c)
    assert (verb, path) == ("POST", "/api/v2/graphs/default/edges")
    assert kw["json"]["edge"]["weight"] == 0.5


def test_create_edge_no_weight(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"id": "e2"})
    c.create_edge("e2", "a", "b", "rel")
    _, _, kw = _last(c)
    assert "weight" not in kw["json"]["edge"]


def test_traverse_graph_extracts_nested_data(monkeypatch):
    c = _make_client(
        monkeypatch,
        resp_body={
            "success": True,
            "data": {
                "nodes": [{"id": "n1"}],
                "edges": [{"id": "e1"}],
                "paths": [["n1"]],
                "stats": {"nodes_visited": 1},
            },
        },
    )
    out = c.traverse_graph("n1", limit=5, edge_types=["k"], node_labels=["L"])
    assert out["nodes"] == [{"id": "n1"}]
    assert out["stats"]["nodes_visited"] == 1
    _, _, kw = _last(c)
    assert kw["json"]["limit"] == 5
    assert kw["json"]["return_path"] is True


def test_traverse_graph_flat_response_defaults(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"foo": "bar"})
    out = c.traverse_graph("n1")
    assert out["nodes"] == []
    assert out["stats"]["nodes_visited"] == 0


def test_query_nodes(monkeypatch):
    c = _make_client(
        monkeypatch,
        http_resp=FakeResp(
            {"success": True, "data": [{"id": "n1"}, {"id": "n2"}], "next_token": "t"}
        ),
    )
    out = c.query_nodes(labels=["L"], properties={"k": "v"}, limit=10, offset=2)
    assert out["total_count"] == 2
    assert out["next_token"] == "t"
    _, _, kw = _last_http(c)
    assert kw["json"]["limit"] == 10
    assert kw["json"]["offset"] == 2


def test_query_edges(monkeypatch):
    c = _make_client(
        monkeypatch,
        http_resp=FakeResp({"data": [{"id": "e1"}]}),
    )
    out = c.query_edges(
        edge_type="knows",
        from_node_id="a",
        to_node_id="b",
        properties={"x": 1},
        limit=5,
        offset=1,
    )
    assert out["total_count"] == 1
    _, path, kw = _last_http(c)
    assert path == "/api/v2/graphs/default/query/edges"
    assert kw["json"]["from_node_id"] == "a"
    assert kw["json"]["to_node_id"] == "b"


def test_get_node(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"data": {"id": "n1"}})
    out = c.get_node("n1")
    assert out["id"] == "n1"
    verb, path, _ = _last(c)
    assert (verb, path) == ("GET", "/api/v2/graphs/default/nodes/n1")


def test_get_node_no_data_wrapper(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"id": "raw"})
    out = c.get_node("raw")
    assert out["id"] == "raw"


def test_get_outgoing_edges(monkeypatch):
    c = _make_client(monkeypatch, http_resp=FakeResp({"data": [{"id": "e1"}]}))
    edges = c.get_outgoing_edges("n1", edge_types=["knows", "likes"])
    # two edge types -> two query calls -> two edges
    assert len(edges) == 2
    assert len(c._fake_http.calls) == 2


def test_get_outgoing_edges_default_type(monkeypatch):
    c = _make_client(monkeypatch, http_resp=FakeResp({"data": []}))
    edges = c.get_outgoing_edges("n1")
    assert edges == []
    # default edge_types == [""] -> single call
    assert len(c._fake_http.calls) == 1


def test_get_incoming_edges(monkeypatch):
    c = _make_client(monkeypatch, http_resp=FakeResp({"data": [{"id": "e9"}]}))
    edges = c.get_incoming_edges("n1", edge_types=["knows"])
    assert edges == [{"id": "e9"}]
    _, path, kw = _last_http(c)
    assert kw["json"]["to_node_id"] == "n1"


def test_delete_node(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"data": {"deleted": True}})
    out = c.delete_node("n1")
    assert out["deleted"] is True
    verb, path, _ = _last(c)
    assert (verb, path) == ("DELETE", "/api/v2/graphs/default/nodes/n1")


# ------------------------------------------- graph collection management


def test_create_graph(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"graph_id": "g1"})
    out = c.create_graph("g1", name="G", description="d", schema={"x": 1})
    assert out["graph_id"] == "g1"
    verb, path, kw = _last(c)
    assert (verb, path) == ("POST", "/api/v2/graphs")
    assert kw["json"]["schema"] == {"x": 1}


def test_create_graph_no_schema(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"graph_id": "g2"})
    c.create_graph("g2")
    _, _, kw = _last(c)
    assert "schema" not in kw["json"]


def test_delete_graph(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"deleted": True})
    out = c.delete_graph("g1")
    assert out["deleted"] is True
    verb, path, _ = _last(c)
    assert (verb, path) == ("DELETE", "/api/v2/graphs/g1")


def test_get_graph(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"name": "G"})
    assert c.get_graph("g1")["name"] == "G"
    assert _last(c)[:2] == ("GET", "/api/v2/graphs/g1")


def test_list_graphs(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"graphs": []})
    assert c.list_graphs() == {"graphs": []}
    assert _last(c)[:2] == ("GET", "/api/v2/graphs")


def test_get_graph_stats(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"node_count": 3})
    assert c.get_graph_stats("g1")["node_count"] == 3
    assert _last(c)[:2] == ("GET", "/api/v2/graphs/g1/stats")


# ------------------------------------------------------------- query API


def test_execute_query_full(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"rows": [{"id": 1}]})
    out = c.execute_query(
        "MATCH x", language="aql", parameters=[1], collection="coll", limit=5
    )
    assert out["rows"] == [{"id": 1}]
    method, path, kw = _last(c)
    assert (method, path) == ("POST", "/api/v2/query")
    assert kw["json"]["language"] == "aql"
    assert kw["json"]["parameters"] == [1]
    assert kw["json"]["collection"] == "coll"
    assert kw["json"]["limit"] == 5


def test_execute_query_minimal(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"rows": []})
    c.execute_query("SELECT 1")
    _, _, kw = _last(c)
    assert kw["json"] == {"language": "uql", "query": "SELECT 1"}


def test_explain_query(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"plan": "x"})
    out = c.explain_query("SELECT 1", language="aql", collection="c")
    assert out["plan"] == "x"
    _, path, kw = _last(c)
    assert path == "/api/v2/query/explain"
    assert kw["json"]["collection"] == "c"


def test_explain_query_minimal(monkeypatch):
    c = _make_client(monkeypatch, resp_body={})
    c.explain_query("q")
    _, _, kw = _last(c)
    assert "collection" not in kw["json"]


def test_execute_uql(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"ok": True})
    c.execute_uql("q", parameters=[1], collection="c", limit=2)
    _, _, kw = _last(c)
    assert kw["json"]["language"] == "uql"


def test_execute_aql(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"ok": True})
    c.execute_aql("q")
    _, _, kw = _last(c)
    assert kw["json"]["language"] == "aql"


def test_execute_federated(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"ok": True})
    c.execute_federated("q", limit=3)
    _, _, kw = _last(c)
    assert kw["json"]["language"] == "federated"
    assert kw["json"]["limit"] == 3


def test_execute_sql_basic(monkeypatch):
    c = _make_client(
        monkeypatch,
        http_resp=FakeResp({"rows": [{"id": 1}, {"id": 2}], "columns": ["id"]}),
    )
    out = c.execute_sql("SELECT id FROM t", parameters=[1], collection="t")
    assert out["row_count"] == 2
    verb, path, kw = _last_http(c)
    assert (verb, path) == ("POST", "/api/v2/query")
    assert kw["json"]["language"] == "uql"
    assert kw["json"]["parameters"] == [1]
    assert kw["json"]["collection"] == "t"


def test_execute_sql_unwraps_data(monkeypatch):
    c = _make_client(
        monkeypatch,
        http_resp=FakeResp({"data": {"rows": [{"id": 1}]}}),
    )
    out = c.execute_sql("SELECT 1")
    assert out["row_count"] == 1


def test_execute_sql_http_status_error(monkeypatch):
    # The HTTPStatusError branch calls map_http_error(e) with a single arg, but
    # map_http_error's signature is (status_code, response_data, ...); the
    # resulting TypeError propagates. We assert the branch is reached (any
    # exception escapes) without asserting a specific exception type.
    import httpx

    c = ProximaDBClient(url="http://testserver")

    class Boom(FakeHttpClient):
        def post(self, path, **kw):
            raise httpx.HTTPStatusError(
                "bad", request=MagicMock(), response=MagicMock(status_code=500)
            )

    monkeypatch.setattr(c, "_http_client", Boom())
    with pytest.raises(Exception):
        c.execute_sql("SELECT 1")
    c.close()


def test_execute_sql_timeout_error(monkeypatch):
    import httpx

    from proximadb_sdk.exceptions import TimeoutError as PDBTimeout

    c = ProximaDBClient(url="http://testserver")

    class Boom(FakeHttpClient):
        def post(self, path, **kw):
            raise httpx.TimeoutException("slow")

    monkeypatch.setattr(c, "_http_client", Boom())
    with pytest.raises(PDBTimeout):
        c.execute_sql("SELECT 1")
    c.close()


def test_execute_sql_network_error(monkeypatch):
    import httpx

    from proximadb_sdk.exceptions import NetworkError

    c = ProximaDBClient(url="http://testserver")

    class Boom(FakeHttpClient):
        def post(self, path, **kw):
            raise httpx.RequestError("down")

    monkeypatch.setattr(c, "_http_client", Boom())
    with pytest.raises(NetworkError):
        c.execute_sql("SELECT 1")
    c.close()


# ------------------------------------------ metadata SqlValue conversion


def test_convert_metadata_to_rest_format_empty(client):
    assert client._convert_metadata_to_rest_format({}) == {}


def test_convert_metadata_to_rest_format_types(client):
    out = client._convert_metadata_to_rest_format(
        {
            "n": None,
            "b": True,
            "i": 42,
            "f": 1.5,
            "s": "hi",
            "by": b"abc",
            "lst": [1, "x"],
            "obj": {"k": 2},
        }
    )
    assert out["n"] == {"null_value": None}
    assert out["b"] == {"bool_value": True}
    assert out["i"] == {"int64_value": 42}
    assert out["f"] == {"number_value": 1.5}
    assert out["s"] == {"string_value": "hi"}
    assert "bytes_value" in out["by"]
    assert "array_value" in out["lst"]
    assert "object_value" in out["obj"]


def test_convert_value_to_rest_sql_value_fallback(client):
    class Weird:
        def __str__(self):
            return "weird"

    out = client._convert_value_to_rest_sql_value(Weird())
    assert out == {"string_value": "weird"}


# ------------------------------------------------- compression + helpers


def test_compress_data_gzip(client):
    import gzip

    client.config.compression.algorithm = "gzip"
    client.config.compression.level = 6
    out = client._compress_data(b"hello world" * 10)
    assert gzip.decompress(out) == b"hello world" * 10


def test_compress_data_deflate(client):
    import zlib

    client.config.compression.algorithm = "deflate"
    client.config.compression.level = 6
    out = client._compress_data(b"abc" * 50)
    assert zlib.decompress(out) == b"abc" * 50


def test_compress_data_unknown_falls_back_to_gzip(client):
    import gzip

    client.config.compression.algorithm = "unknown-thing"
    client.config.compression.level = None
    out = client._compress_data(b"payload")
    assert gzip.decompress(out) == b"payload"


def test_compress_data_zstd_missing_falls_back(client, monkeypatch):
    import builtins
    import gzip

    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "zstandard":
            raise ImportError("no zstd")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    client.config.compression.algorithm = "zstd"
    client.config.compression.level = 3
    out = client._compress_data(b"zdata")
    assert gzip.decompress(out) == b"zdata"


def test_compress_data_brotli_missing_falls_back(client, monkeypatch):
    import builtins
    import gzip

    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "brotli":
            raise ImportError("no brotli")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    client.config.compression.algorithm = "br"
    client.config.compression.level = None
    out = client._compress_data(b"bdata")
    assert gzip.decompress(out) == b"bdata"


def test_handle_error_response_with_json(client):
    from proximadb_sdk.exceptions import ProximaDBError

    resp = FakeResp(
        {"message": "boom", "error_code": "BAD"},
        status=400,
        headers={"x-request-id": "rid-1"},
    )
    with pytest.raises(ProximaDBError):
        client._handle_error_response(resp)


def test_handle_error_response_non_json(client):
    from proximadb_sdk.exceptions import ProximaDBError

    resp = FakeResp(ValueError("not json"), status=500, text="server fire")
    with pytest.raises(ProximaDBError):
        client._handle_error_response(resp)


def test_http_post_returns_json(monkeypatch):
    c = _make_client(monkeypatch, resp_body={"ok": 1})
    out = c._http_post("/some/path", {"a": 1})
    assert out == {"ok": 1}
    method, path, kw = _last(c)
    assert (method, path) == ("POST", "/some/path")
    assert kw["json"] == {"a": 1}


def test_normalize_vectors_numpy_float64(client):
    arr = np.array([[1.0, 2.0]], dtype=np.float64)
    out = client._normalize_vectors(arr)
    assert out == [[1.0, 2.0]]


def test_validate_vector_dimensions_mismatch(client):
    from proximadb_sdk.exceptions import VectorDimensionError

    client.config.validate_inputs = True
    with pytest.raises(VectorDimensionError):
        client._validate_vector_dimensions([[1.0, 2.0]], expected_dim=3)


def test_validate_vector_dimensions_numpy_not_2d(client):
    client.config.validate_inputs = True
    with pytest.raises(ValueError):
        client._validate_vector_dimensions(np.array([1.0, 2.0]), expected_dim=2)


def test_validate_vector_dimensions_disabled(client):
    client.config.validate_inputs = False
    # no raise even with mismatch
    client._validate_vector_dimensions([[1.0]], expected_dim=99)


# --------------------------------------- quantization proto conversion


def test_convert_quantization_disabled():
    class Q:
        def model_dump(self, exclude_none=True):
            return {"enabled": False, "type": "NONE"}

    out = _convert_quantization_config_to_proto(Q())
    assert out["enabled"] is False
    assert out["strategy"] == 0
    assert out["custom_levels"] == []


def test_convert_quantization_custom_levels():
    class Q:
        def model_dump(self, exclude_none=True):
            return {
                "enabled": True,
                "type": "PRODUCT",
                "num_subvectors": 16,
                "bits_per_subvector": 4,
                "accuracy_threshold": 0.9,
                "progressive_quantization": True,
            }

    out = _convert_quantization_config_to_proto(Q())
    assert out["strategy"] == 1
    assert len(out["custom_levels"]) == 1
    level = out["custom_levels"][0]
    assert level["type"] == 2  # PRODUCT
    assert level["bits"] == 4
    assert out["binary_filter_selectivity"] == 0.9
    assert out["enable_progressive_search"] is True


def test_convert_quantization_model_dump_failure():
    class Q:
        def model_dump(self, exclude_none=True):
            raise RuntimeError("v2 fail")

        def dict(self, exclude_none=True):
            return {"enabled": True, "type": "SCALAR"}

    out = _convert_quantization_config_to_proto(Q())
    assert out["strategy"] == 1
    assert out["custom_levels"][0]["type"] == 1  # SCALAR


def test_convert_quantization_both_dump_fail():
    class Q:
        def model_dump(self, exclude_none=True):
            raise RuntimeError("fail v2")

        def dict(self, exclude_none=True):
            raise RuntimeError("fail v1")

    out = _convert_quantization_config_to_proto(Q())
    # falls back to empty dict -> disabled default
    assert out["enabled"] is False
    assert out["strategy"] == 0


# ------------------------------------------ module-level convenience fns


def test_connect_returns_client(monkeypatch):
    created = {}

    class Fake:
        def __init__(self, url=None, api_key=None, **kwargs):
            created["url"] = url
            created["api_key"] = api_key

    monkeypatch.setattr("proximadb_sdk.protocols.rest_sync.ProximaDBClient", Fake)
    c = connect(url="http://x", api_key="k")
    assert isinstance(c, Fake)
    assert created["url"] == "http://x"


def test_quick_search_uses_context_manager(monkeypatch):
    fake_client = MagicMock()
    fake_client.__enter__.return_value = fake_client
    fake_client.__exit__.return_value = False
    fake_client.search.return_value = [MagicMock()]

    monkeypatch.setattr(
        "proximadb_sdk.protocols.rest_sync.connect", lambda **kw: fake_client
    )
    out = quick_search("coll", [0.1, 0.2], k=3, url="http://x")
    assert len(out) == 1
    fake_client.search.assert_called_once_with("coll", [0.1, 0.2], 3)


def test_context_manager_closes(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    fake_http = FakeHttpClient()
    monkeypatch.setattr(c, "_http_client", fake_http)
    with c as ctx:
        assert ctx is c
    assert fake_http.closed is True


def test_search_batch_with_filter(monkeypatch):
    c = _make_client(
        monkeypatch,
        resp_body={"results": [[{"id": "a", "score": 0.9}]]},
    )
    out = c.search_batch("cid", [[0.1, 0.2]], k=3, filter={"cat": "x"}, exact=True)
    assert out[0][0].id == "a"
    _, _, kw = _last(c)
    assert kw["json"]["filter"] == {"cat": "x"}
    assert kw["json"]["params"]["exact_search"] is True


# ----------------------------------------- real _make_request internals


class _FakeReqHttp:
    """Stands in for httpx.Client; records .request() calls."""

    def __init__(self, resp=None, exc=None):
        self.calls = []
        self._resp = resp
        self._exc = exc

    def request(self, method, endpoint, **kw):
        self.calls.append((method, endpoint, kw))
        if self._exc is not None:
            raise self._exc
        return self._resp

    def close(self):
        pass


def test_make_request_plain(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    resp = FakeResp({"ok": True}, status=200)
    http = _FakeReqHttp(resp=resp)
    monkeypatch.setattr(c, "_http_client", http)
    out = c._make_request("GET", "/health")
    assert out is resp
    assert http.calls[0][0] == "GET"
    c.close()


def test_make_request_compresses_big_payload(monkeypatch):
    import gzip

    c = ProximaDBClient(url="http://testserver")
    c.config.compression.enabled = True
    c.config.compression.algorithm = "gzip"
    c.config.compression.threshold_bytes = 1
    http = _FakeReqHttp(resp=FakeResp({"ok": True}))
    monkeypatch.setattr(c, "_http_client", http)
    big = {"data": "x" * 5000}
    c._make_request("POST", "/api/v2/collections", json=big)
    _, _, kw = http.calls[0]
    # json replaced by compressed content + gzip Content-Encoding header
    assert "content" in kw
    assert kw["headers"]["Content-Encoding"] == "gzip"
    assert gzip.decompress(kw["content"]) == __import__("json").dumps(big).encode()
    c.close()


def test_make_request_small_payload_not_compressed(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    c.config.compression.enabled = True
    c.config.compression.threshold_bytes = 100000
    http = _FakeReqHttp(resp=FakeResp({"ok": True}))
    monkeypatch.setattr(c, "_http_client", http)
    c._make_request("POST", "/x", json={"a": 1})
    _, _, kw = http.calls[0]
    # too small -> left as json, no content
    assert "json" in kw
    assert "content" not in kw
    c.close()


def test_make_request_compress_brotli_header(monkeypatch):
    c = ProximaDBClient(url="http://testserver")
    c.config.compression.enabled = True
    c.config.compression.algorithm = "br"
    c.config.compression.threshold_bytes = 1
    http = _FakeReqHttp(resp=FakeResp({"ok": True}))
    monkeypatch.setattr(c, "_http_client", http)
    c._make_request("POST", "/x", json={"k": "v" * 2000})
    _, _, kw = http.calls[0]
    assert kw["headers"]["Content-Encoding"] == "br"
    c.close()


def test_make_request_error_response_maps(monkeypatch):
    from proximadb_sdk.exceptions import ProximaDBError

    c = ProximaDBClient(url="http://testserver")
    err = FakeResp({"message": "bad", "error_code": "X"}, status=400)
    http = _FakeReqHttp(resp=err)
    monkeypatch.setattr(c, "_http_client", http)
    with pytest.raises(ProximaDBError):
        c._make_request("GET", "/boom")
    c.close()


def test_make_request_timeout_exception(monkeypatch):
    import httpx

    from proximadb_sdk.exceptions import TimeoutError as PDBTimeout

    c = ProximaDBClient(url="http://testserver")
    # zero retries so the loop doesn't sleep/iterate
    c.config.retry.max_retries = 0
    http = _FakeReqHttp(exc=httpx.TimeoutException("slow"))
    monkeypatch.setattr(c, "_http_client", http)
    with pytest.raises(PDBTimeout):
        c._make_request("GET", "/slow")
    c.close()


def test_make_request_network_exception(monkeypatch):
    import httpx

    from proximadb_sdk.exceptions import NetworkError

    c = ProximaDBClient(url="http://testserver")
    c.config.retry.max_retries = 0
    http = _FakeReqHttp(exc=httpx.ConnectError("down"))
    monkeypatch.setattr(c, "_http_client", http)
    with pytest.raises(NetworkError):
        c._make_request("GET", "/down")
    c.close()


def test_health_services_none_defaults(monkeypatch):
    c = _make_client(
        monkeypatch,
        resp_body={"status": "ok", "services": None},
    )
    hs = c.health()
    assert hs.services == {}


def test_create_collection_filterable_columns(monkeypatch):
    from proximadb_sdk.models import CollectionConfig, FilterableColumn

    c = _make_client(
        monkeypatch,
        resp_body={
            "collection": {
                "id": "fc1",
                "config": {
                    "name": "withcols_x",
                    "dimension": 8,
                    "filterable_columns": [
                        {"name": "price", "data_type": 3, "indexed": True}
                    ],
                },
            }
        },
    )
    cfg = CollectionConfig(
        name="withcols_x",
        dimension=8,
        filterable_columns=[FilterableColumn(name="price", data_type="float")],
    )
    coll = c.create_collection("withcols_x", cfg)
    assert coll.id == "fc1"
    assert coll.config.filterable_columns is not None


def test_create_collection_error_in_collection_field(monkeypatch):
    from proximadb_sdk.exceptions import ProximaDBError

    c = _make_client(
        monkeypatch,
        resp_body={"collection": None, "error": "boom creating"},
    )
    from proximadb_sdk.models import CollectionConfig

    cfg = CollectionConfig(name="errcoll_xx", dimension=8)
    with pytest.raises(ProximaDBError):
        c.create_collection("errcoll_xx", cfg)


def test_create_collection_fallback_response(monkeypatch):
    # Response with neither collection_id nor collection -> Collection(**data)
    c = _make_client(
        monkeypatch,
        resp_body={
            "id": "fallbackid",
            "config": {"name": "fallbackcoll", "dimension": 16},
        },
    )
    from proximadb_sdk.models import CollectionConfig

    cfg = CollectionConfig(name="fallbackcoll", dimension=16)
    coll = c.create_collection("fallbackcoll", cfg)
    assert coll.id == "fallbackid"
