"""
ProximaDB Python Client - Asynchronous REST Client (Graph subset)

Provides async REST methods for graph operations with per-call prefetch overrides.
"""

from typing import Any

import httpx


class ProximaDBAsyncClient:
    def __init__(self, url: str = "http://localhost:5678", timeout: float = 60.0):
        self._base_url = url.rstrip("/")
        self._timeout = timeout
        self._client = httpx.AsyncClient(base_url=self._base_url, timeout=self._timeout)

    async def aclose(self):
        await self._client.aclose()

    async def graph_shortest_path(
        self,
        start_node_id: str,
        target_node_id: str,
        max_depth: int | None = None,
        edge_types: list[str] | None = None,
        algorithm: str = "DIJKSTRA",
        k: int | None = None,
        enable_prefetch: bool | None = None,
        prefetch_budget: int | None = None,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "start_node_id": start_node_id,
            "target_node_id": target_node_id,
            "algorithm": algorithm,
        }
        if max_depth is not None:
            body["max_depth"] = max_depth
        if edge_types:
            body["edge_types"] = edge_types
        if k is not None:
            body["k"] = k

        headers: dict[str, str] = {"Content-Type": "application/json"}
        if enable_prefetch is not None:
            headers["x-graph-prefetch-enabled"] = "true" if enable_prefetch else "false"
        if prefetch_budget is not None:
            headers["x-graph-prefetch-budget"] = str(prefetch_budget)

        # Also include overrides in body for compatibility with endpoints that accept JSON fields
        if enable_prefetch is not None:
            body["enable_prefetch"] = bool(enable_prefetch)
        if prefetch_budget is not None:
            body["prefetch_budget"] = int(prefetch_budget)

        resp = await self._client.post(
            "/api/v1/graph/shortest_path", json=body, headers=headers
        )
        resp.raise_for_status()
        return resp.json()

    async def graph_traverse(
        self,
        start_node_id: str,
        max_depth: int = 3,
        edge_types: list[str] | None = None,
        algorithm: str = "BFS",
        limit: int | None = None,
        timeout_ms: int | None = None,
        max_frontier: int | None = None,
        enable_prefetch: bool | None = None,
        prefetch_budget: int | None = None,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "start_node_id": start_node_id,
            "max_depth": max_depth,
            "algorithm": algorithm,
        }
        if edge_types:
            body["edge_types"] = edge_types
        if limit is not None:
            body["limit"] = limit
        if timeout_ms is not None:
            body["timeout_ms"] = timeout_ms
        if max_frontier is not None:
            body["max_frontier"] = max_frontier

        headers: dict[str, str] = {"Content-Type": "application/json"}
        if enable_prefetch is not None:
            headers["x-graph-prefetch-enabled"] = "true" if enable_prefetch else "false"
        if prefetch_budget is not None:
            headers["x-graph-prefetch-budget"] = str(prefetch_budget)

        # Also include overrides in body for compatibility
        if enable_prefetch is not None:
            body["enable_prefetch"] = bool(enable_prefetch)
        if prefetch_budget is not None:
            body["prefetch_budget"] = int(prefetch_budget)

        resp = await self._client.post(
            "/api/v1/graph/traverse", json=body, headers=headers
        )
        resp.raise_for_status()
        return resp.json()
