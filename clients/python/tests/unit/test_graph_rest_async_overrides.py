import json

import httpx
import pytest

from proximadb_sdk.protocols.rest_async import ProximaDBAsyncClient


@pytest.mark.asyncio
async def test_async_graph_shortest_path_headers_and_body():
    captured = {}

    async def handler(request: httpx.Request) -> httpx.Response:
        captured["path"] = request.url.path
        captured["headers"] = dict(request.headers)
        body = json.loads(request.content.decode() or "{}")
        captured["body"] = body
        return httpx.Response(200, json={"ok": True})

    transport = httpx.MockTransport(handler)
    client = ProximaDBAsyncClient(url="http://testserver")
    # Inject mock transport
    client._client = httpx.AsyncClient(
        base_url="http://testserver", transport=transport
    )

    try:
        resp = await client.graph_shortest_path(
            start_node_id="n1",
            target_node_id="n8",
            algorithm="DIJKSTRA",
            max_depth=5,
            enable_prefetch=True,
            prefetch_budget=11,
        )
        assert resp["ok"] is True
        assert captured["path"] == "/api/v1/graph/shortest_path"
        # Headers set
        assert captured["headers"]["x-graph-prefetch-enabled"] == "true"
        assert captured["headers"]["x-graph-prefetch-budget"] == "11"
        # Body fields set for compatibility
        assert captured["body"]["enable_prefetch"] is True
        assert captured["body"]["prefetch_budget"] == 11
    finally:
        await client.aclose()


@pytest.mark.asyncio
async def test_async_graph_traverse_headers_and_body():
    captured = {}

    async def handler(request: httpx.Request) -> httpx.Response:
        captured["path"] = request.url.path
        captured["headers"] = dict(request.headers)
        body = json.loads(request.content.decode() or "{}")
        captured["body"] = body
        return httpx.Response(200, json={"ok": True})

    transport = httpx.MockTransport(handler)
    client = ProximaDBAsyncClient(url="http://testserver")
    client._client = httpx.AsyncClient(
        base_url="http://testserver", transport=transport
    )

    try:
        resp = await client.graph_traverse(
            start_node_id="n1",
            max_depth=3,
            algorithm="BFS",
            enable_prefetch=False,
            prefetch_budget=7,
        )
        assert resp["ok"] is True
        assert captured["path"] == "/api/v1/graph/traverse"
        # Headers set
        assert captured["headers"]["x-graph-prefetch-enabled"] == "false"
        assert captured["headers"]["x-graph-prefetch-budget"] == "7"
        # Body fields set
        assert captured["body"]["enable_prefetch"] is False
        assert captured["body"]["prefetch_budget"] == 7
    finally:
        await client.aclose()
