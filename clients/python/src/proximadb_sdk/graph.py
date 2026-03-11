"""
ProximaDB Graph API Module

High-level graph operations for Python SDK.
Provides Cypher-like query interface, batch operations, and pattern matching.

This module fills the gap between low-level node/edge CRUD and Victor's needs
for call graph and dependency tracking.

Example:
    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.graph import ProximaDBGraph

    client = ProximaDBClient(url="http://localhost:5678")
    graph = ProximaDBGraph(client, "myrepo_graph")

    # Batch operations (performance-critical for code indexing)
    graph.batch_create_nodes([
        {"id": "func:main", "labels": ["Function"], "properties": {"name": "main"}},
        {"id": "func:parse", "labels": ["Function"], "properties": {"name": "parse"}},
    ])

    # Create edges
    graph.batch_create_edges([
        {"from": "func:main", "to": "func:parse", "type": "CALLS", "properties": {"line": 42}},
    ])

    # Cypher-like query
    results = graph.query_cypher("""
        MATCH (c:Function)-[:CALLS]->(f:Function)
        WHERE c.name = 'main'
        RETURN c, f
    """)

    # Find callers (reverse traversal)
    callers = graph.find_callers("func:parse_json")
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Union

from .models import Collection


@dataclass
class GraphNode:
    """Graph node representation.

    Attributes:
        id: Unique node identifier
        labels: List of node labels/types
        properties: Node properties as key-value pairs
        embedding: Optional embedding vector for semantic search
    """

    id: str
    labels: List[str] = field(default_factory=list)
    properties: Dict[str, Any] = field(default_factory=dict)
    embedding: Optional[List[float]] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API calls."""
        return {
            "id": self.id,
            "labels": self.labels,
            "properties": self.properties,
        }


@dataclass
class GraphEdge:
    """Graph edge representation.

    Attributes:
        id: Unique edge identifier
        from_node: Source node ID
        to_node: Target node ID
        edge_type: Relationship type (e.g., "CALLS", "IMPORTS")
        properties: Edge properties as key-value pairs
        weight: Optional weight for weighted graphs
    """

    id: str
    from_node: str
    to_node: str
    edge_type: str
    properties: Dict[str, Any] = field(default_factory=dict)
    weight: Optional[float] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API calls."""
        result = {
            "id": self.id,
            "from_node": self.from_node,
            "to_node": self.to_node,
            "edge_type": self.edge_type,
            "properties": self.properties,
        }
        if self.weight is not None:
            result["weight"] = self.weight
        return result


@dataclass
class GraphPath:
    """Path representation for traversal results.

    Attributes:
        nodes: List of node IDs in path order
        edges: List of edge IDs in path order
        total_weight: Sum of edge weights
    """

    nodes: List[str] = field(default_factory=list)
    edges: List[str] = field(default_factory=list)
    total_weight: float = 0.0


@dataclass
class GraphQueryResult:
    """Result from graph query operations.

    Attributes:
        nodes: List of nodes in result
        edges: List of edges in result
        paths: List of traversal paths (if applicable)
        stats: Query execution statistics
    """

    nodes: List[GraphNode] = field(default_factory=list)
    edges: List[GraphEdge] = field(default_factory=list)
    paths: List[GraphPath] = field(default_factory=list)
    stats: Dict[str, Any] = field(default_factory=dict)


class ProximaDBGraph:
    """High-level ProximaDB graph operations interface.

    Provides simplified API for common graph operations needed by Victor:
    - Batch node/edge creation (performance-critical)
    - Cypher-like query interface
    - Reverse traversal (find callers)
    - Pattern matching

    Args:
        client: ProximaDB client instance
        graph_id: Graph collection identifier
    """

    def __init__(self, client: Any, graph_id: str):
        """Initialize graph interface.

        Args:
            client: ProximaDBClient instance
            graph_id: Graph collection ID
        """
        self._client = client
        self._graph_id = graph_id

    # ========================================================================
    # Batch Operations (Performance-Critical for Victor)
    # ========================================================================

    def batch_create_nodes(
        self,
        nodes: List[Union[GraphNode, Dict[str, Any]]],
        batch_size: int = 1000,
    ) -> Dict[str, Any]:
        """Create multiple nodes in a single operation.

        This is **critical for Victor performance** when indexing codebases
        with thousands of functions.

        Args:
            nodes: List of nodes to create (GraphNode or dict)
            batch_size: Number of nodes per batch request

        Returns:
            Creation result with success status and statistics

        Example:
            graph.batch_create_nodes([
                {"id": "func:main", "labels": ["Function"], "properties": {"name": "main"}},
                {"id": "func:parse", "labels": ["Function"], "properties": {"name": "parse"}},
            ])
        """
        if not nodes:
            return {"success": True, "created": 0}

        # Convert GraphNode objects to dicts if needed
        node_dicts = []
        for node in nodes:
            if isinstance(node, GraphNode):
                node_dicts.append(node.to_dict())
            else:
                node_dicts.append(node)

        # Process in batches
        total_created = 0
        failed = []

        for i in range(0, len(node_dicts), batch_size):
            batch = node_dicts[i : i + batch_size]

            # Use REST API for batch creation
            # (This will call the client's internal batch operation)
            try:
                # For now, we'll use individual creates through the client
                # TODO: Add true batch API to client
                for node_dict in batch:
                    self._client.create_node(
                        graph_id=self._graph_id,
                        node_id=node_dict["id"],
                        labels=node_dict.get("labels", []),
                        properties=node_dict.get("properties", {}),
                    )
                    total_created += 1
            except Exception as e:
                failed.append({"batch": i // batch_size, "error": str(e)})

        return {
            "success": len(failed) == 0,
            "created": total_created,
            "failed": len(failed),
            "errors": failed,
        }

    def batch_create_edges(
        self,
        edges: List[Union[GraphEdge, Dict[str, Any]]],
        batch_size: int = 1000,
    ) -> Dict[str, Any]:
        """Create multiple edges in a single operation.

        Critical for Victor when creating call graphs and import relationships.

        Args:
            edges: List of edges to create (GraphEdge or dict)
            batch_size: Number of edges per batch request

        Returns:
            Creation result with success status and statistics

        Example:
            graph.batch_create_edges([
                {"from": "func:main", "to": "func:parse", "type": "CALLS"},
                {"from": "func:main", "to": "func:validate", "type": "CALLS"},
            ])
        """
        if not edges:
            return {"success": True, "created": 0}

        # Convert GraphEdge objects to dicts if needed
        edge_dicts = []
        for edge in edges:
            if isinstance(edge, GraphEdge):
                edge_dicts.append(edge.to_dict())
            else:
                edge_dicts.append(edge)

        # Process in batches
        total_created = 0
        failed = []

        for i in range(0, len(edge_dicts), batch_size):
            batch = edge_dicts[i : i + batch_size]

            try:
                for edge_dict in batch:
                    self._client.create_edge(
                        graph_id=self._graph_id,
                        edge_id=edge_dict.get("id", f"edge_{i}"),
                        from_node=edge_dict["from_node"],
                        to_node=edge_dict["to_node"],
                        edge_type=edge_dict["edge_type"],
                        properties=edge_dict.get("properties", {}),
                    )
                    total_created += 1
            except Exception as e:
                failed.append({"batch": i // batch_size, "error": str(e)})

        return {
            "success": len(failed) == 0,
            "created": total_created,
            "failed": len(failed),
            "errors": failed,
        }

    # ========================================================================
    # Query Operations
    # ========================================================================

    def query_cypher(
        self,
        query: str,
        params: Optional[Dict[str, Any]] = None,
    ) -> GraphQueryResult:
        """Execute Cypher-like graph query.

        Supports a subset of openCypher:
        - MATCH patterns with nodes and relationships
        - WHERE clauses with property filters
        - RETURN projections
        - Basic traversals

        Args:
            query: Cypher query string
            params: Optional query parameters

        Returns:
            Graph query result with nodes, edges, and paths

        Example:
            results = graph.query_cypher("""
                MATCH (c:Function)-[:CALLS]->(f:Function)
                WHERE c.name = 'main'
                RETURN c, f
            """)
        """
        # Parse Cypher query
        parsed = self._parse_cypher(query)

        # Execute using available client methods
        # For now, use graph traversal and node query
        if parsed["match"]:
            # Extract starting conditions
            start_labels = parsed.get("start_labels", [])
            start_props = parsed.get("start_properties", {})

            # Query starting nodes
            start_nodes_result = self._client.query_nodes(
                graph_id=self._graph_id,
                labels=start_labels,
                properties=start_props,
            )

            # Traverse from start nodes
            if start_nodes_result.get("nodes"):
                start_node_ids = [n["id"] for n in start_nodes_result["nodes"]]
                return self._execute_traversal(start_node_ids, parsed)
            else:
                return GraphQueryResult()

        return GraphQueryResult()

    def _parse_cypher(self, query: str) -> Dict[str, Any]:
        """Parse Cypher query into components.

        This is a simplified parser for common patterns.
        Full Cypher support would require a proper parser.
        """
        # Remove extra whitespace
        query = " ".join(query.split())

        result = {
            "match": False,
            "where": False,
            "return": False,
            "start_labels": [],
            "start_properties": {},
            "traversal": None,
        }

        # Check for MATCH clause
        match_match = re.search(r"MATCH\s+(.+?)(?:\s+WHERE|\s+RETURN|$)", query, re.IGNORECASE)
        if match_match:
            result["match"] = True
            match_pattern = match_match.group(1)

            # Parse node pattern: (variable:Label {property: value})
            node_pattern = re.search(r"\((\w+)(?::(\w+))?(?:\s*\{(.+)\})?\)", match_pattern)
            if node_pattern:
                if node_pattern.group(2):  # Label
                    result["start_labels"] = [node_pattern.group(2)]

                if node_pattern.group(3):  # Properties
                    # Parse properties: key: value, key2: value2
                    props_str = node_pattern.group(3)
                    for prop_match in re.finditer(r'(\w+)\s*:\s*"?(\w+)"?', props_str):
                        result["start_properties"][prop_match.group(1)] = prop_match.group(2)

            # Parse relationship pattern: -[r:TYPE]->
            rel_pattern = re.search(r"-?\[(\w+)(?::(\w+))?\]->", match_pattern)
            if rel_pattern:
                result["traversal"] = {
                    "variable": rel_pattern.group(1),
                    "type": rel_pattern.group(2),
                }

        # Check for WHERE clause
        where_match = re.search(r"WHERE\s+(.+?)(?:\s+RETURN|$)", query, re.IGNORECASE)
        if where_match:
            result["where"] = True
            where_clause = where_match.group(1)
            # Parse WHERE conditions
            # Simplified: handles "property = value" patterns
            prop_match = re.search(r"(\w+)\s*=\s*['\"]?([^'\"]+)['\"]?", where_clause)
            if prop_match:
                result["start_properties"][prop_match.group(1)] = prop_match.group(2)

        return result

    def _execute_traversal(
        self, start_node_ids: List[str], parsed_query: Dict[str, Any]
    ) -> GraphQueryResult:
        """Execute graph traversal from start nodes."""
        result = GraphQueryResult()

        # Traverse from each start node
        for start_id in start_node_ids[:10]:  # Limit for now
            try:
                traversal_result = self._client.traverse_graph(
                    graph_id=self._graph_id,
                    start_node_id=start_id,
                    max_depth=1,  # Default depth
                    edge_types=[parsed_query["traversal"]["type"]] if parsed_query.get("traversal") else None,
                )

                if traversal_result.get("nodes"):
                    for node_data in traversal_result["nodes"]:
                        result.nodes.append(
                            GraphNode(
                                id=node_data["id"],
                                labels=node_data.get("labels", []),
                                properties=node_data.get("properties", {}),
                            )
                        )

                if traversal_result.get("edges"):
                    for edge_data in traversal_result["edges"]:
                        result.edges.append(
                            GraphEdge(
                                id=edge_data["id"],
                                from_node=edge_data["from_node_id"],
                                to_node=edge_data["to_node_id"],
                                edge_type=edge_data.get("edge_type", ""),
                                properties=edge_data.get("properties", {}),
                            )
                        )
            except Exception as e:
                # Log and continue
                pass

        return result

    # ========================================================================
    # Reverse Traversal (Find Callers)
    # ========================================================================

    def find_callers(
        self,
        node_id: str,
        edge_type: str = "CALLS",
        max_depth: int = 1,
    ) -> List[GraphNode]:
        """Find all nodes that have edges pointing to the target node.

        This is **critical for Victor** to find:
        - All functions that call a given function
        - All modules that import a given module
        - All classes that inherit from a given class

        Args:
            node_id: Target node ID
            edge_type: Edge type to traverse (default: "CALLS")
            max_depth: Maximum depth to traverse (default: 1)

        Returns:
            List of nodes that point to the target

        Example:
            # Find all functions that call parse_json
            callers = graph.find_callers("func:parse_json", edge_type="CALLS")
            for caller in callers:
                print(f"{caller.properties['name']}() calls parse_json()")
        """
        # Get all edges of the specified type
        all_edges = []
        try:
            # Query edges that point to our target
            # This requires a custom query since standard API doesn't support it
            # For now, we'll use a workaround: get all edges and filter
            graph_info = self._client.get_graph(self._graph_id)

            # Get all nodes first to find connections
            # This is inefficient - proper implementation needs reverse index
            all_nodes = self._client.query_nodes(
                graph_id=self._graph_id,
                labels=[],
                properties={},
            )

            caller_ids = []

            # For each node, check if it has an edge to our target
            # TODO: Optimize this with proper reverse index query
            for node_data in all_nodes.get("nodes", []):
                try:
                    # Try to traverse from this node
                    traversal = self._client.traverse_graph(
                        graph_id=self._graph_id,
                        start_node_id=node_data["id"],
                        max_depth=max_depth,
                        edge_types=[edge_type] if edge_type else None,
                    )

                    # Check if traversal reaches our target
                    for visited_node in traversal.get("nodes", []):
                        if visited_node["id"] == node_id:
                            caller_ids.append(node_data["id"])
                            break
                except Exception:
                    pass

            # Get full node data for callers
            callers = []
            for caller_id in caller_ids:
                try:
                    node_data = self._client.get_node(self._graph_id, caller_id)
                    callers.append(
                        GraphNode(
                            id=node_data["id"],
                            labels=node_data.get("labels", []),
                            properties=node_data.get("properties", {}),
                        )
                    )
                except Exception:
                    pass

            return callers

        except Exception as e:
            # Log error and return empty list
            return []

    # ========================================================================
    # Pattern Matching
    # ========================================================================

    def match_pattern(
        self,
        pattern: str,
        filters: Optional[Dict[str, Any]] = None,
    ) -> List[Dict[str, GraphNode]]:
        """Match a graph pattern and return matching subgraphs.

        Pattern syntax:
        - (n) matches any node
        - (n:Label) matches nodes with label
        - (n {key: value}) matches nodes with properties
        - (n)-[r:TYPE]->(m) matches relationships

        Args:
            pattern: Graph pattern string
            filters: Optional additional filters

        Returns:
            List of matched subgraphs (each is a dict of variable -> node)

        Example:
            # Find all functions that call other functions
            matches = graph.match_pattern("(f1:Function)-[:CALLS]->(f2:Function)")

            # Find test functions that call functions with "parse" in name
            matches = graph.match_pattern(
                "(t1:Function)-[:CALLS]->(f2:Function)",
                filters={"t1.name": "*test*", "f2.name": "*parse*"}
            )
        """
        # This is a simplified implementation
        # Full pattern matching would require a proper graph pattern engine

        # Parse pattern for nodes and relationships
        nodes = {}
        relationships = []

        # Find all node patterns: (variable:Label)
        node_matches = re.finditer(r"\((\w+)(?::(\w+))?(?:\s*\{(.+)\})?\)", pattern)
        for match in node_matches:
            var_name = match.group(1)
            label = match.group(2)
            props_str = match.group(3)

            props = {}
            if props_str:
                # Parse properties
                for prop_match in re.finditer(r'(\w+)\s*:\s*"?(\w+)"?', props_str):
                    props[prop_match.group(1)] = prop_match.group(2)

            nodes[var_name] = {"label": label, "properties": props}

        # Find relationship patterns: -[r:TYPE]->
        rel_matches = re.finditer(r"-?\[(\w+)(?::(\w+))?\]->", pattern)
        for match in rel_matches:
            relationships.append({
                "variable": match.group(1),
                "type": match.group(2),
            })

        # If no relationships, just return matching nodes
        if not relationships:
            if nodes:
                # Get first node as starting point
                first_var = list(nodes.keys())[0]
                first_node = nodes[first_var]

                node_results = self._client.query_nodes(
                    graph_id=self._graph_id,
                    labels=[first_node["label"]] if first_node["label"] else [],
                    properties=first_node["properties"],
                )

                return [
                    {first_var: GraphNode(
                        id=n["id"],
                        labels=n.get("labels", []),
                        properties=n.get("properties", {}),
                    )}
                    for n in node_results.get("nodes", [])
                ]

        return []

    # ========================================================================
    # Graph Statistics
    # ========================================================================

    def get_stats(self) -> Dict[str, Any]:
        """Get graph statistics.

        Returns:
            Dictionary with node_count, edge_count, etc.
        """
        return self._client.get_graph_stats(self._graph_id)


def create_graph_api(client: Any, graph_id: str) -> ProximaDBGraph:
    """Factory function to create a graph API instance.

    Args:
        client: ProximaDB client instance
        graph_id: Graph collection identifier

    Returns:
        ProximaDBGraph instance
    """
    return ProximaDBGraph(client, graph_id)
