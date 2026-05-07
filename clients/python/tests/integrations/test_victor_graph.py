from __future__ import annotations

from typing import Any, Dict, List

import pytest

from proximadb_sdk.graph import ProximaDBGraph
from proximadb_sdk.integrations.victor_graph import (
    ProximaDBGraphStore,
    VictorGraphEdge,
    VictorGraphNode,
)


class FakeGraphClient:
    def __init__(self) -> None:
        self.graphs: Dict[str, Dict[str, Any]] = {}
        self.fail_on_traverse = False

    def _graph(self, graph_id: str) -> Dict[str, Any]:
        return self.graphs.setdefault(graph_id, {"nodes": {}, "edges": {}})

    def create_graph(self, graph_id: str, *args, **kwargs) -> Dict[str, Any]:
        self._graph(graph_id)
        return {"success": True, "graph_id": graph_id}

    def delete_graph(self, graph_id: str) -> Dict[str, Any]:
        self.graphs.pop(graph_id, None)
        return {"success": True, "graph_id": graph_id}

    def create_node(
        self,
        *,
        graph_id: str,
        node_id: str,
        labels: List[str],
        properties: Dict[str, Any] | None = None,
        **kwargs,
    ) -> Dict[str, Any]:
        graph = self._graph(graph_id)
        graph["nodes"][node_id] = {
            "id": node_id,
            "labels": list(labels or []),
            "properties": dict(properties or {}),
        }
        return graph["nodes"][node_id]

    def create_edge(
        self,
        *,
        graph_id: str,
        edge_id: str,
        from_node_id: str,
        to_node_id: str,
        edge_type: str,
        properties: Dict[str, Any] | None = None,
        weight: float | None = None,
        **kwargs,
    ) -> Dict[str, Any]:
        graph = self._graph(graph_id)
        graph["edges"][edge_id] = {
            "id": edge_id,
            "from_node_id": from_node_id,
            "to_node_id": to_node_id,
            "edge_type": edge_type,
            "properties": dict(properties or {}),
            "weight": weight,
        }
        return graph["edges"][edge_id]

    def query_nodes(
        self,
        *,
        graph_id: str,
        labels: List[str] | None = None,
        properties: Dict[str, Any] | None = None,
        limit: int | None = None,
        offset: int | None = None,
        **kwargs,
    ) -> Dict[str, Any]:
        graph = self._graph(graph_id)
        nodes = list(graph["nodes"].values())
        if labels:
            nodes = [
                node
                for node in nodes
                if all(label in node.get("labels", []) for label in labels)
            ]
        if properties:
            nodes = [
                node
                for node in nodes
                if all(node.get("properties", {}).get(key) == value for key, value in properties.items())
            ]
        start = offset or 0
        end = None if limit is None else start + limit
        page = nodes[start:end]
        return {"nodes": page, "total_count": len(nodes)}

    def get_node(self, *, graph_id: str, node_id: str, **kwargs) -> Dict[str, Any] | None:
        return self._graph(graph_id)["nodes"].get(node_id)

    def get_outgoing_edges(
        self,
        *,
        graph_id: str,
        node_id: str,
        edge_types: List[str] | None = None,
        **kwargs,
    ) -> List[Dict[str, Any]]:
        edges = [
            edge
            for edge in self._graph(graph_id)["edges"].values()
            if edge["from_node_id"] == node_id
        ]
        if edge_types:
            edges = [edge for edge in edges if edge["edge_type"] in edge_types]
        return edges

    def get_incoming_edges(
        self,
        *,
        graph_id: str,
        node_id: str,
        edge_types: List[str] | None = None,
        **kwargs,
    ) -> List[Dict[str, Any]]:
        edges = [
            edge
            for edge in self._graph(graph_id)["edges"].values()
            if edge["to_node_id"] == node_id
        ]
        if edge_types:
            edges = [edge for edge in edges if edge["edge_type"] in edge_types]
        return edges

    def delete_node(self, *, graph_id: str, node_id: str, **kwargs) -> bool:
        graph = self._graph(graph_id)
        existed = graph["nodes"].pop(node_id, None) is not None
        graph["edges"] = {
            edge_id: edge
            for edge_id, edge in graph["edges"].items()
            if edge["from_node_id"] != node_id and edge["to_node_id"] != node_id
        }
        return existed

    def traverse_graph(
        self,
        *,
        graph_id: str,
        start_node_id: str,
        max_depth: int = 1,
        edge_types: List[str] | None = None,
        **kwargs,
    ) -> Dict[str, Any]:
        if self.fail_on_traverse:
            raise AssertionError("traverse_graph should not be used for incoming-edge lookups")

        graph = self._graph(graph_id)
        frontier = {start_node_id}
        visited = {start_node_id}
        nodes = []
        edges = []

        for _ in range(max_depth):
            next_frontier = set()
            for node_id in frontier:
                node = graph["nodes"].get(node_id)
                if node is not None:
                    nodes.append(node)
                for edge in self.get_outgoing_edges(
                    graph_id=graph_id,
                    node_id=node_id,
                    edge_types=edge_types,
                ):
                    edges.append(edge)
                    target = edge["to_node_id"]
                    if target not in visited:
                        visited.add(target)
                        next_frontier.add(target)
            frontier = next_frontier
            if not frontier:
                break

        for node_id in frontier:
            node = graph["nodes"].get(node_id)
            if node is not None:
                nodes.append(node)

        deduped_nodes = {node["id"]: node for node in nodes}
        deduped_edges = {edge["id"]: edge for edge in edges}
        return {
            "nodes": list(deduped_nodes.values()),
            "edges": list(deduped_edges.values()),
            "paths": [],
            "stats": {},
        }

    def get_graph_stats(self, graph_id: str) -> Dict[str, Any]:
        graph = self._graph(graph_id)
        return {
            "total_nodes": len(graph["nodes"]),
            "total_edges": len(graph["edges"]),
        }

    def close(self) -> None:
        return None


def test_find_callers_prefers_incoming_edge_lookup() -> None:
    client = FakeGraphClient()
    graph = ProximaDBGraph(client, "code")

    graph.batch_create_nodes(
        [
            {"id": "fn:a", "labels": ["Function"], "properties": {"name": "a"}},
            {"id": "fn:b", "labels": ["Function"], "properties": {"name": "b"}},
            {"id": "fn:c", "labels": ["Function"], "properties": {"name": "c"}},
        ]
    )
    graph.batch_create_edges(
        [
            {"id": "e1", "from": "fn:a", "to": "fn:c", "type": "CALLS"},
            {"id": "e2", "from": "fn:b", "to": "fn:c", "type": "CALLS"},
        ]
    )

    client.fail_on_traverse = True
    callers = graph.find_callers("fn:c", edge_type="CALLS")

    assert sorted(node.id for node in callers) == ["fn:a", "fn:b"]


def test_import_graph_json_supports_graphify_shape() -> None:
    client = FakeGraphClient()
    graph = ProximaDBGraph(client, "graphify")

    result = graph.import_graph_json(
        {
            "nodes": [
                {"id": "file:app.py", "type": "File", "name": "app.py", "file": "app.py"},
                {"id": "fn:main", "type": "Function", "name": "main", "file": "app.py"},
            ],
            "edges": [
                {"source": "file:app.py", "target": "fn:main", "label": "CONTAINS"},
            ],
        }
    )

    assert result["success"] is True
    assert result["node_count"] == 2
    assert result["edge_count"] == 1
    assert client.get_node(graph_id="graphify", node_id="fn:main")["properties"]["name"] == "main"
    outgoing = client.get_outgoing_edges(graph_id="graphify", node_id="file:app.py")
    assert outgoing[0]["edge_type"] == "CONTAINS"


@pytest.mark.asyncio
async def test_victor_graph_store_supports_victor_shape_workflows() -> None:
    client = FakeGraphClient()
    store = ProximaDBGraphStore(client=client, graph_id="victor")

    await store.initialize()
    await store.upsert_nodes(
        [
            VictorGraphNode(
                node_id="fn:main",
                type="Function",
                name="main",
                file="app.py",
                line=1,
                metadata={"qualified_name": "app.main"},
            ),
            VictorGraphNode(
                node_id="fn:helper",
                type="Function",
                name="helper",
                file="lib.py",
                line=10,
                metadata={"qualified_name": "lib.helper"},
            ),
        ]
    )
    await store.upsert_edges(
        [
            VictorGraphEdge(
                src="fn:main",
                dst="fn:helper",
                type="CALLS",
                metadata={"line_number": 3},
            )
        ]
    )

    await store.update_file_mtime("app.py", 100.0)

    search_results = await store.search_symbols("main", limit=5)
    neighbors = await store.get_neighbors("fn:main", edge_types=["CALLS"], direction="out")
    stale_files = await store.get_stale_files({"app.py": 101.0, "lib.py": 50.0})

    assert [node.node_id for node in search_results] == ["fn:main"]
    assert [(edge.src, edge.dst, edge.type) for edge in neighbors] == [
        ("fn:main", "fn:helper", "CALLS")
    ]
    assert stale_files == ["app.py", "lib.py"]

    await store.delete_by_file("app.py")
    remaining = await store.get_nodes_by_file("app.py")
    assert remaining == []
