"""Victor-compatible graph store backed by ProximaDB graph collections.

This adapter reuses the existing ProximaDB graph APIs instead of introducing a
parallel store. It is intended for Victor's code graph / Graph RAG workloads
and can also ingest Graphify-style ``graph.json`` artifacts.
"""

from __future__ import annotations

import hashlib
import json
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, AsyncIterator, Dict, Iterable, List, Literal, Optional, Sequence

from ..graph import GraphEdge as SDKGraphEdge
from ..graph import GraphNode as SDKGraphNode
from ..graph import ProximaDBGraph
from ..unified_client import ProximaDBClient

try:
    from victor.storage.graph.protocol import (
        GraphEdge as VictorGraphEdge,
        GraphNode as VictorGraphNode,
        GraphQueryResult as VictorGraphQueryResult,
        GraphStoreProtocol,
        GraphTraversalDirection,
        Subgraph,
    )
except ImportError:
    GraphTraversalDirection = Literal["out", "in", "both"]
    GraphStoreProtocol = object

    @dataclass
    class VictorGraphNode:
        node_id: str
        type: str
        name: str
        file: str
        line: int | None = None
        end_line: int | None = None
        lang: str | None = None
        signature: str | None = None
        docstring: str | None = None
        parent_id: str | None = None
        embedding_ref: str | None = None
        metadata: Dict[str, Any] = field(default_factory=dict)
        ast_kind: str | None = None
        scope_id: str | None = None
        statement_type: str | None = None
        requirement_id: str | None = None
        visibility: str | None = None

    @dataclass
    class VictorGraphEdge:
        src: str
        dst: str
        type: str
        weight: float | None = None
        metadata: Dict[str, Any] = field(default_factory=dict)

    @dataclass
    class Subgraph:
        subgraph_id: str
        anchor_node_id: str
        radius: int
        edge_types: List[str]
        node_ids: List[str]
        edges: List[VictorGraphEdge]
        node_count: int = 0
        computed_at: str | None = None

    @dataclass
    class VictorGraphQueryResult:
        nodes: List[VictorGraphNode]
        edges: List[VictorGraphEdge]
        subgraphs: List[Subgraph] = field(default_factory=list)
        query: str = ""
        execution_time_ms: float = 0.0
        metadata: Dict[str, Any] = field(default_factory=dict)


_FILE_STATE_LABEL = "__FileState"
_SUBGRAPH_CACHE_LABEL = "__SubgraphCache"
_INTERNAL_LABELS = {_FILE_STATE_LABEL, _SUBGRAPH_CACHE_LABEL}
_NODE_CORE_PROPERTIES = {
    "type",
    "name",
    "file",
    "file_path",
    "line",
    "line_start",
    "end_line",
    "line_end",
    "lang",
    "signature",
    "docstring",
    "parent_id",
    "embedding_ref",
    "ast_kind",
    "scope_id",
    "statement_type",
    "requirement_id",
    "visibility",
    "metadata_json",
    "qualified_name",
}
_EDGE_CORE_PROPERTIES = {
    "metadata_json",
    "line",
    "line_number",
}


def _is_scalar(value: Any) -> bool:
    return value is None or isinstance(value, (str, int, float, bool))


def _safe_json_loads(value: Any) -> Dict[str, Any]:
    if not value:
        return {}
    if isinstance(value, dict):
        return dict(value)
    try:
        loaded = json.loads(str(value))
    except (TypeError, ValueError):
        return {}
    return loaded if isinstance(loaded, dict) else {}


def _coerce_int(value: Any) -> int | None:
    if value is None or value == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _coerce_float(value: Any) -> float | None:
    if value is None or value == "":
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


class ProximaDBGraphStore(GraphStoreProtocol):
    """Victor-compatible graph store using a single ProximaDB graph collection."""

    def __init__(
        self,
        client: Optional[Any] = None,
        *,
        graph_id: str = "victor_code_graph",
        url: Optional[str] = None,
        create_if_missing: bool = True,
    ) -> None:
        self._client = client or ProximaDBClient(url=url or "embedded://local")
        self._graph_id = graph_id
        self._graph = ProximaDBGraph(self._client, graph_id)
        self._create_if_missing = create_if_missing
        self._initialized = False

    async def initialize(self) -> None:
        if self._initialized:
            return
        if self._create_if_missing:
            try:
                self._client.create_graph(self._graph_id)
            except Exception:
                pass
        self._initialized = True

    async def close(self) -> None:
        close = getattr(self._client, "close", None)
        if callable(close):
            close()

    @staticmethod
    def _file_state_node_id(file_path: str) -> str:
        digest = hashlib.sha1(file_path.encode("utf-8")).hexdigest()
        return f"{_FILE_STATE_LABEL}:{digest}"

    @staticmethod
    def _subgraph_cache_node_id(subgraph_id: str) -> str:
        return f"{_SUBGRAPH_CACHE_LABEL}:{subgraph_id}"

    @staticmethod
    def _subgraph_cache_key(
        anchor_node_id: str,
        radius: int,
        edge_types: Sequence[str] | None,
    ) -> str:
        payload = json.dumps(
            {
                "anchor_node_id": anchor_node_id,
                "radius": radius,
                "edge_types": sorted(edge_types or []),
            },
            sort_keys=True,
        )
        return hashlib.sha1(payload.encode("utf-8")).hexdigest()

    @staticmethod
    def _sdk_node_from_victor(node: VictorGraphNode) -> Dict[str, Any]:
        properties: Dict[str, Any] = {
            "type": node.type,
            "name": node.name,
            "file": node.file,
            "line": node.line,
            "end_line": node.end_line,
            "lang": node.lang,
            "signature": node.signature,
            "docstring": node.docstring,
            "parent_id": node.parent_id,
            "embedding_ref": node.embedding_ref,
            "ast_kind": node.ast_kind,
            "scope_id": node.scope_id,
            "statement_type": node.statement_type,
            "requirement_id": node.requirement_id,
            "visibility": node.visibility,
            "metadata_json": json.dumps(node.metadata or {}, sort_keys=True),
        }
        for key, value in (node.metadata or {}).items():
            if key not in properties and _is_scalar(value):
                properties[key] = value

        labels = [node.type]
        extra_labels = node.metadata.get("labels") if isinstance(node.metadata, dict) else None
        if isinstance(extra_labels, list):
            labels.extend(str(label) for label in extra_labels)

        return {
            "id": node.node_id,
            "labels": list(dict.fromkeys(label for label in labels if label)),
            "properties": {key: value for key, value in properties.items() if value is not None},
        }

    @staticmethod
    def _sdk_edge_from_victor(edge: VictorGraphEdge) -> Dict[str, Any]:
        edge_id = edge.metadata.get("id") or f"{edge.type}:{edge.src}:{edge.dst}"
        properties: Dict[str, Any] = {
            "metadata_json": json.dumps(edge.metadata or {}, sort_keys=True),
        }
        for key, value in (edge.metadata or {}).items():
            if key not in properties and _is_scalar(value):
                properties[key] = value

        payload = {
            "id": str(edge_id),
            "from_node_id": edge.src,
            "to_node_id": edge.dst,
            "edge_type": edge.type,
            "properties": properties,
        }
        if edge.weight is not None:
            payload["weight"] = edge.weight
        return payload

    @staticmethod
    def _victor_node_from_sdk(node: SDKGraphNode) -> VictorGraphNode:
        properties = dict(node.properties or {})
        metadata = _safe_json_loads(properties.get("metadata_json"))
        for key, value in properties.items():
            if key not in _NODE_CORE_PROPERTIES and key not in metadata:
                metadata[key] = value

        node_type = properties.get("type")
        if not node_type:
            for label in node.labels:
                if label not in _INTERNAL_LABELS:
                    node_type = label
                    break

        return VictorGraphNode(
            node_id=node.id,
            type=str(node_type or "node"),
            name=str(properties.get("name") or node.id),
            file=str(
                properties.get("file")
                or properties.get("file_path")
                or properties.get("path")
                or ""
            ),
            line=_coerce_int(properties.get("line", properties.get("line_start"))),
            end_line=_coerce_int(
                properties.get("end_line", properties.get("line_end"))
            ),
            lang=properties.get("lang"),
            signature=properties.get("signature"),
            docstring=properties.get("docstring"),
            parent_id=properties.get("parent_id"),
            embedding_ref=properties.get("embedding_ref"),
            metadata=metadata,
            ast_kind=properties.get("ast_kind"),
            scope_id=properties.get("scope_id"),
            statement_type=properties.get("statement_type"),
            requirement_id=properties.get("requirement_id"),
            visibility=properties.get("visibility"),
        )

    @staticmethod
    def _victor_edge_from_sdk(edge: SDKGraphEdge) -> VictorGraphEdge:
        properties = dict(edge.properties or {})
        metadata = _safe_json_loads(properties.get("metadata_json"))
        for key, value in properties.items():
            if key not in _EDGE_CORE_PROPERTIES and key not in metadata:
                metadata[key] = value
        if edge.id:
            metadata.setdefault("id", edge.id)
        return VictorGraphEdge(
            src=edge.from_node,
            dst=edge.to_node,
            type=edge.edge_type,
            weight=edge.weight,
            metadata=metadata,
        )

    async def upsert_nodes(self, nodes: Iterable[VictorGraphNode]) -> None:
        await self.initialize()
        node_list = list(nodes)
        if not node_list:
            return

        for node in node_list:
            existing = self._graph.get_node_by_id(node.node_id)
            if existing is not None:
                try:
                    self._client.delete_node(node_id=node.node_id, graph_id=self._graph_id)
                except Exception:
                    pass

        self._graph.batch_create_nodes(
            [self._sdk_node_from_victor(node) for node in node_list]
        )

    async def upsert_edges(self, edges: Iterable[VictorGraphEdge]) -> None:
        await self.initialize()
        edge_list = list(edges)
        if not edge_list:
            return
        self._graph.batch_create_edges(
            [self._sdk_edge_from_victor(edge) for edge in edge_list]
        )

    async def get_neighbors(
        self,
        node_id: str,
        edge_types: Iterable[str] | None = None,
        *,
        direction: GraphTraversalDirection = "both",
        max_depth: int = 1,
    ) -> List[VictorGraphEdge]:
        await self.initialize()
        edges = self._graph.get_neighbors(
            node_id,
            edge_types=edge_types,
            direction=direction,
            max_depth=max_depth,
        )
        return [self._victor_edge_from_sdk(edge) for edge in edges]

    async def find_nodes(
        self,
        *,
        name: str | None = None,
        type: str | None = None,
        file: str | None = None,
    ) -> List[VictorGraphNode]:
        await self.initialize()
        nodes = self._graph.find_nodes(name=name, type=type, file=file)
        return [self._victor_node_from_sdk(node) for node in nodes]

    async def search_symbols(
        self,
        query: str,
        *,
        limit: int = 20,
        symbol_types: Iterable[str] | None = None,
    ) -> List[VictorGraphNode]:
        await self.initialize()
        nodes = self._graph.search_symbols(
            query,
            limit=limit,
            symbol_types=symbol_types,
        )
        return [self._victor_node_from_sdk(node) for node in nodes]

    async def get_node_by_id(self, node_id: str) -> VictorGraphNode | None:
        await self.initialize()
        node = self._graph.get_node_by_id(node_id)
        if node is None:
            return None
        return self._victor_node_from_sdk(node)

    async def get_all_nodes(self) -> List[VictorGraphNode]:
        await self.initialize()
        return [
            self._victor_node_from_sdk(node)
            for node in self._graph.get_all_nodes(include_internal=False)
        ]

    async def get_nodes_by_file(self, file: str) -> List[VictorGraphNode]:
        await self.initialize()
        return [
            self._victor_node_from_sdk(node)
            for node in self._graph.get_nodes_by_file(file)
        ]

    async def update_file_mtime(self, file: str, mtime: float) -> None:
        await self.initialize()
        state_node = VictorGraphNode(
            node_id=self._file_state_node_id(file),
            type=_FILE_STATE_LABEL,
            name=file,
            file=file,
            metadata={
                "mtime": mtime,
                "indexed_at": time.time(),
            },
        )
        await self.upsert_nodes([state_node])

    async def get_stale_files(self, file_mtimes: Dict[str, float]) -> List[str]:
        await self.initialize()
        stale_files: List[str] = []
        state_nodes = self._graph.get_all_nodes(
            labels=[_FILE_STATE_LABEL],
            include_internal=True,
        )
        indexed_mtimes = {
            node.properties.get("file", node.properties.get("name", "")): _coerce_float(
                _safe_json_loads(node.properties.get("metadata_json")).get("mtime")
                or node.properties.get("mtime")
            )
            for node in state_nodes
        }

        for file_path, current_mtime in file_mtimes.items():
            stored = indexed_mtimes.get(file_path)
            if stored is None or float(current_mtime) > float(stored):
                stale_files.append(file_path)
        return stale_files

    async def delete_by_file(self, file: str) -> None:
        await self.initialize()
        for node in self._graph.get_nodes_by_file(file):
            try:
                self._client.delete_node(node_id=node.id, graph_id=self._graph_id)
            except Exception:
                continue
        try:
            self._client.delete_node(
                node_id=self._file_state_node_id(file),
                graph_id=self._graph_id,
            )
        except Exception:
            pass

    async def delete_by_repo(self) -> None:
        await self.initialize()
        try:
            self._client.delete_graph(self._graph_id)
        finally:
            if self._create_if_missing:
                try:
                    self._client.create_graph(self._graph_id)
                except Exception:
                    pass

    async def stats(self) -> Dict[str, Any]:
        await self.initialize()
        stats = self._graph.get_stats()
        data = stats.get("data", stats) if isinstance(stats, dict) else {}
        if isinstance(data, dict):
            return data
        return {}

    async def get_all_edges(self) -> List[VictorGraphEdge]:
        await self.initialize()
        return [
            self._victor_edge_from_sdk(edge)
            for edge in self._graph.get_all_edges()
        ]

    async def get_nodes_by_statement_type(
        self,
        statement_type: str,
        *,
        file: str | None = None,
    ) -> List[VictorGraphNode]:
        await self.initialize()
        nodes = [
            node
            for node in self._graph.get_all_nodes()
            if node.properties.get("statement_type") == statement_type
        ]
        if file is not None:
            nodes = [
                node
                for node in nodes
                if (
                    node.properties.get("file")
                    or node.properties.get("file_path")
                )
                == file
            ]
        return [self._victor_node_from_sdk(node) for node in nodes]

    async def get_nodes_by_requirement(self, requirement_id: str) -> List[VictorGraphNode]:
        await self.initialize()
        nodes = [
            node
            for node in self._graph.get_all_nodes()
            if node.properties.get("requirement_id") == requirement_id
        ]
        return [self._victor_node_from_sdk(node) for node in nodes]

    async def get_subgraph(
        self,
        anchor_node_id: str,
        radius: int = 2,
        edge_types: Iterable[str] | None = None,
    ) -> Subgraph:
        await self.initialize()
        edge_types_list = list(edge_types or [])
        subgraph_id = self._subgraph_cache_key(anchor_node_id, radius, edge_types_list)
        cache_node = self._graph.get_node_by_id(self._subgraph_cache_node_id(subgraph_id))
        if cache_node is not None:
            cached = _safe_json_loads(cache_node.properties.get("metadata_json")).get(
                "payload",
                {},
            )
            if cached:
                edges = [
                    VictorGraphEdge(**edge_payload)
                    for edge_payload in cached.get("edges", [])
                ]
                return Subgraph(
                    subgraph_id=subgraph_id,
                    anchor_node_id=anchor_node_id,
                    radius=radius,
                    edge_types=edge_types_list,
                    node_ids=list(cached.get("node_ids", [])),
                    edges=edges,
                    node_count=int(cached.get("node_count", len(cached.get("node_ids", [])))),
                    computed_at=cached.get("computed_at"),
                )

        edges = await self.get_neighbors(
            anchor_node_id,
            edge_types=edge_types_list,
            direction="both",
            max_depth=radius,
        )
        node_ids = {anchor_node_id}
        for edge in edges:
            node_ids.add(edge.src)
            node_ids.add(edge.dst)
        subgraph = Subgraph(
            subgraph_id=subgraph_id,
            anchor_node_id=anchor_node_id,
            radius=radius,
            edge_types=edge_types_list,
            node_ids=sorted(node_ids),
            edges=edges,
            node_count=len(node_ids),
            computed_at=str(time.time()),
        )
        await self.cache_subgraph(subgraph)
        return subgraph

    async def cache_subgraph(self, subgraph: Subgraph) -> None:
        await self.initialize()
        cache_node = VictorGraphNode(
            node_id=self._subgraph_cache_node_id(subgraph.subgraph_id),
            type=_SUBGRAPH_CACHE_LABEL,
            name=subgraph.subgraph_id,
            file="",
            metadata={
                "payload": {
                    "node_ids": list(subgraph.node_ids),
                    "edges": [
                        {
                            "src": edge.src,
                            "dst": edge.dst,
                            "type": edge.type,
                            "weight": edge.weight,
                            "metadata": edge.metadata,
                        }
                        for edge in subgraph.edges
                    ],
                    "node_count": subgraph.node_count,
                    "computed_at": subgraph.computed_at,
                }
            },
        )
        await self.upsert_nodes([cache_node])

    async def invalidate_subgraph(self, subgraph_id: str) -> None:
        await self.initialize()
        try:
            self._client.delete_node(
                node_id=self._subgraph_cache_node_id(subgraph_id),
                graph_id=self._graph_id,
            )
        except Exception:
            pass

    async def get_nodes_by_scope(self, scope_id: str) -> List[VictorGraphNode]:
        await self.initialize()
        nodes = [
            node
            for node in self._graph.get_all_nodes()
            if node.properties.get("scope_id") == scope_id
        ]
        return [self._victor_node_from_sdk(node) for node in nodes]

    async def multi_hop_traverse(
        self,
        start_node_ids: List[str],
        max_hops: int = 2,
        edge_types: Iterable[str] | None = None,
        max_nodes: int = 100,
    ) -> VictorGraphQueryResult:
        await self.initialize()
        edge_types_list = list(edge_types or [])
        node_map: Dict[str, VictorGraphNode] = {}
        edge_map: Dict[tuple[str, str, str], VictorGraphEdge] = {}

        for start_node_id in start_node_ids:
            node = await self.get_node_by_id(start_node_id)
            if node is not None:
                node_map[node.node_id] = node

            subgraph = await self.get_subgraph(
                start_node_id,
                radius=max_hops,
                edge_types=edge_types_list,
            )
            for edge in subgraph.edges:
                signature = (edge.src, edge.dst, edge.type)
                edge_map[signature] = edge
                if len(node_map) >= max_nodes:
                    continue
                for node_id in (edge.src, edge.dst):
                    if node_id in node_map:
                        continue
                    neighbor = await self.get_node_by_id(node_id)
                    if neighbor is not None:
                        node_map[node_id] = neighbor

        return VictorGraphQueryResult(
            nodes=list(node_map.values())[:max_nodes],
            edges=list(edge_map.values()),
            query="multi_hop_traverse",
            metadata={
                "start_node_ids": start_node_ids,
                "max_hops": max_hops,
            },
        )

    async def iter_nodes(
        self,
        *,
        batch_size: int = 100,
        name: str | None = None,
        type: str | None = None,
        file: str | None = None,
    ) -> AsyncIterator[List[VictorGraphNode]]:
        await self.initialize()
        nodes = await self.find_nodes(name=name, type=type, file=file)
        for index in range(0, len(nodes), batch_size):
            yield nodes[index : index + batch_size]

    async def import_graphify_graph(
        self,
        graph_data: Dict[str, Any] | str | Path,
    ) -> Dict[str, Any]:
        """Import a Graphify-style ``graph.json`` payload into the graph store."""
        await self.initialize()
        return self._graph.import_graph_json(graph_data)


__all__ = ["ProximaDBGraphStore"]
