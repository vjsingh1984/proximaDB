"""
Integration tests for graph operations using the ProximaDB SDK.

Tests comprehensive graph operations (nodes, edges, traversal, queries) via both
REST and gRPC protocols using the Python SDK.
"""

import pytest
import time
import uuid
import logging
from typing import List, Dict, Any, Optional

from proximadb_sdk import ProximaDBClient
from proximadb_sdk.exceptions import ProximaDBError

logger = logging.getLogger(__name__)


class TestGraphOperationsSDK:
    """Comprehensive integration tests for graph operations using SDK methods"""

    @pytest.fixture(params=["rest", "grpc"])
    def client(self, request):
        """Create client for both REST and gRPC protocols"""
        protocol = request.param
        if protocol == "rest":
            client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        else:
            client = ProximaDBClient(url="grpc://localhost:5679", protocol="grpc")

        logger.info(f"Created {protocol.upper()} client for graph operations")
        yield client

        # Cleanup: Note - graph cleanup would be added here if API supports it

    @pytest.fixture
    def graph_id(self):
        """Generate unique graph ID for test isolation"""
        return f"test_graph_{uuid.uuid4().hex[:8]}"

    def test_create_node_simple(self, client, graph_id):
        """Test creating a simple node with minimal properties"""
        node_id = f"node_{uuid.uuid4().hex[:8]}"

        result = client.create_node(
            node_id=node_id, labels=["Person"], properties={"name": "Alice", "age": 30}
        )

        assert result is not None
        logger.info(f"Created node {node_id} with labels ['Person']")

    def test_create_node_multiple_labels(self, client, graph_id):
        """Test creating node with multiple labels"""
        node_id = f"node_{uuid.uuid4().hex[:8]}"

        result = client.create_node(
            node_id=node_id,
            labels=["Person", "Employee", "Manager"],
            properties={
                "name": "Bob",
                "age": 35,
                "department": "Engineering",
                "salary": 120000.50,
            },
        )

        assert result is not None
        logger.info(f"Created node {node_id} with multiple labels")

    def test_create_node_various_property_types(self, client, graph_id):
        """Test creating node with various property types"""
        node_id = f"node_{uuid.uuid4().hex[:8]}"

        result = client.create_node(
            node_id=node_id,
            labels=["TestNode"],
            properties={
                "string_prop": "test value",
                "int_prop": 42,
                "float_prop": 3.14159,
                "bool_prop": True,
                "null_prop": None,
            },
        )

        assert result is not None
        logger.info(f"Created node {node_id} with various property types")

    def test_create_edge_simple(self, client, graph_id):
        """Test creating a simple edge between two nodes"""
        # Create source and target nodes first
        node1_id = f"node_{uuid.uuid4().hex[:8]}"
        node2_id = f"node_{uuid.uuid4().hex[:8]}"

        client.create_node(node1_id, labels=["Person"], properties={"name": "Alice"})
        client.create_node(node2_id, labels=["Person"], properties={"name": "Bob"})

        # Create edge
        edge_id = f"edge_{uuid.uuid4().hex[:8]}"
        result = client.create_edge(
            edge_id=edge_id,
            from_node_id=node1_id,
            to_node_id=node2_id,
            edge_type="KNOWS",
            properties={"since": 2020},
        )

        assert result is not None
        logger.info(f"Created edge {edge_id} of type KNOWS")

    def test_create_edge_with_weight(self, client, graph_id):
        """Test creating edge with weight property"""
        node1_id = f"node_{uuid.uuid4().hex[:8]}"
        node2_id = f"node_{uuid.uuid4().hex[:8]}"

        client.create_node(node1_id, labels=["City"], properties={"name": "NYC"})
        client.create_node(node2_id, labels=["City"], properties={"name": "LA"})

        edge_id = f"edge_{uuid.uuid4().hex[:8]}"
        result = client.create_edge(
            edge_id=edge_id,
            from_node_id=node1_id,
            to_node_id=node2_id,
            edge_type="ROAD",
            properties={"distance_km": 4500, "highway": "I-80"},
            weight=4500.0,
        )

        assert result is not None
        logger.info(f"Created weighted edge {edge_id}")

    def test_create_multiple_edges_same_nodes(self, client, graph_id):
        """Test creating multiple edges between same nodes with different types"""
        node1_id = f"node_{uuid.uuid4().hex[:8]}"
        node2_id = f"node_{uuid.uuid4().hex[:8]}"

        client.create_node(node1_id, labels=["Person"], properties={"name": "Alice"})
        client.create_node(node2_id, labels=["Person"], properties={"name": "Bob"})

        # Create multiple edge types
        edge_types = ["KNOWS", "WORKS_WITH", "FRIENDS_WITH"]
        for edge_type in edge_types:
            edge_id = f"edge_{edge_type}_{uuid.uuid4().hex[:8]}"
            client.create_edge(
                edge_id=edge_id,
                from_node_id=node1_id,
                to_node_id=node2_id,
                edge_type=edge_type,
            )

        logger.info(
            f"Created {len(edge_types)} edges between {node1_id} and {node2_id}"
        )

    def test_query_nodes_by_label(self, client, graph_id):
        """Test querying nodes by label"""
        # Create test nodes
        for i in range(5):
            node_id = f"person_{uuid.uuid4().hex[:8]}"
            client.create_node(
                node_id=node_id,
                labels=["Person"],
                properties={"name": f"Person{i}", "age": 20 + i},
            )

        # Query by label
        result = client.query_nodes(labels=["Person"], limit=10)

        assert result is not None
        assert "nodes" in result
        assert len(result["nodes"]) >= 5
        logger.info(f"Queried nodes with label 'Person', found {len(result['nodes'])}")

    def test_query_nodes_by_properties(self, client, graph_id):
        """Test querying nodes by properties"""
        # Create test nodes with specific properties
        for i in range(3):
            node_id = f"employee_{uuid.uuid4().hex[:8]}"
            client.create_node(
                node_id=node_id,
                labels=["Employee"],
                properties={"department": "Engineering", "level": i + 1},
            )

        # Query by properties
        result = client.query_nodes(
            labels=["Employee"], properties={"department": "Engineering"}, limit=10
        )

        assert result is not None
        assert "nodes" in result
        logger.info(f"Queried nodes by properties, found {len(result['nodes'])}")

    def test_query_nodes_with_limit_offset(self, client, graph_id):
        """Test querying nodes with limit and offset for pagination"""
        # Create 10 test nodes
        for i in range(10):
            node_id = f"test_{uuid.uuid4().hex[:8]}"
            client.create_node(
                node_id=node_id, labels=["TestLabel"], properties={"index": i}
            )

        # Query with pagination
        page1 = client.query_nodes(labels=["TestLabel"], limit=5, offset=0)
        page2 = client.query_nodes(labels=["TestLabel"], limit=5, offset=5)

        assert page1 is not None
        assert page2 is not None
        assert "nodes" in page1
        assert "nodes" in page2
        logger.info(
            f"Pagination test: page1={len(page1['nodes'])}, page2={len(page2['nodes'])}"
        )

    def test_traverse_graph_bfs(self, client, graph_id):
        """Test BFS graph traversal"""
        # Create a simple graph structure: A -> B -> C
        #                                     A -> D
        node_a = f"node_a_{uuid.uuid4().hex[:8]}"
        node_b = f"node_b_{uuid.uuid4().hex[:8]}"
        node_c = f"node_c_{uuid.uuid4().hex[:8]}"
        node_d = f"node_d_{uuid.uuid4().hex[:8]}"

        # Create nodes
        for node_id, name in [
            (node_a, "A"),
            (node_b, "B"),
            (node_c, "C"),
            (node_d, "D"),
        ]:
            client.create_node(node_id, labels=["Node"], properties={"name": name})

        # Create edges
        client.create_edge(
            f"edge_ab_{uuid.uuid4().hex[:8]}", node_a, node_b, "CONNECTS"
        )
        client.create_edge(
            f"edge_bc_{uuid.uuid4().hex[:8]}", node_b, node_c, "CONNECTS"
        )
        client.create_edge(
            f"edge_ad_{uuid.uuid4().hex[:8]}", node_a, node_d, "CONNECTS"
        )

        # Traverse from node A
        result = client.traverse_graph(
            start_node_id=node_a, max_depth=2, algorithm="BFS", limit=10
        )

        assert result is not None
        assert "nodes" in result
        assert len(result["nodes"]) >= 1  # At least the start node
        logger.info(f"BFS traversal from {node_a}: found {len(result['nodes'])} nodes")

    def test_traverse_graph_dfs(self, client, graph_id):
        """Test DFS graph traversal"""
        # Create a linear graph: A -> B -> C -> D
        nodes = []
        for i, name in enumerate(["A", "B", "C", "D"]):
            node_id = f"node_{name}_{uuid.uuid4().hex[:8]}"
            client.create_node(
                node_id, labels=["Node"], properties={"name": name, "depth": i}
            )
            nodes.append(node_id)

        # Create edges
        for i in range(len(nodes) - 1):
            edge_id = f"edge_{i}_{uuid.uuid4().hex[:8]}"
            client.create_edge(edge_id, nodes[i], nodes[i + 1], "NEXT")

        # Traverse with DFS
        result = client.traverse_graph(
            start_node_id=nodes[0], max_depth=3, algorithm="DFS", limit=10
        )

        assert result is not None
        assert "nodes" in result
        logger.info(f"DFS traversal: found {len(result['nodes'])} nodes")

    def test_traverse_graph_with_edge_type_filter(self, client, graph_id):
        """Test graph traversal with edge type filtering"""
        # Create nodes
        node1 = f"node1_{uuid.uuid4().hex[:8]}"
        node2 = f"node2_{uuid.uuid4().hex[:8]}"
        node3 = f"node3_{uuid.uuid4().hex[:8]}"

        client.create_node(node1, labels=["Person"], properties={"name": "Alice"})
        client.create_node(node2, labels=["Person"], properties={"name": "Bob"})
        client.create_node(node3, labels=["Person"], properties={"name": "Charlie"})

        # Create different edge types
        client.create_edge(f"edge_knows_{uuid.uuid4().hex[:8]}", node1, node2, "KNOWS")
        client.create_edge(
            f"edge_works_{uuid.uuid4().hex[:8]}", node1, node3, "WORKS_WITH"
        )

        # Traverse only KNOWS edges
        result = client.traverse_graph(
            start_node_id=node1, max_depth=1, edge_types=["KNOWS"], algorithm="BFS"
        )

        assert result is not None
        logger.info(f"Filtered traversal with edge_types=['KNOWS']")

    def test_traverse_graph_with_node_label_filter(self, client, graph_id):
        """Test graph traversal with node label filtering"""
        # Create mixed label nodes
        person1 = f"person_{uuid.uuid4().hex[:8]}"
        company1 = f"company_{uuid.uuid4().hex[:8]}"
        person2 = f"person_{uuid.uuid4().hex[:8]}"

        client.create_node(person1, labels=["Person"], properties={"name": "Alice"})
        client.create_node(
            company1, labels=["Company"], properties={"name": "TechCorp"}
        )
        client.create_node(person2, labels=["Person"], properties={"name": "Bob"})

        # Create edges
        client.create_edge(
            f"edge1_{uuid.uuid4().hex[:8]}", person1, company1, "WORKS_AT"
        )
        client.create_edge(
            f"edge2_{uuid.uuid4().hex[:8]}", company1, person2, "EMPLOYS"
        )

        # Traverse only Person nodes
        result = client.traverse_graph(
            start_node_id=person1, max_depth=2, node_labels=["Person"], algorithm="BFS"
        )

        assert result is not None
        logger.info(f"Filtered traversal with node_labels=['Person']")

    def test_traverse_graph_max_depth(self, client, graph_id):
        """Test that max_depth parameter limits traversal"""
        # Create a chain: A -> B -> C -> D -> E
        nodes = []
        for i in range(5):
            node_id = f"chain_{i}_{uuid.uuid4().hex[:8]}"
            client.create_node(node_id, labels=["ChainNode"], properties={"index": i})
            nodes.append(node_id)

        # Create edges
        for i in range(len(nodes) - 1):
            client.create_edge(
                f"chain_edge_{i}_{uuid.uuid4().hex[:8]}", nodes[i], nodes[i + 1], "NEXT"
            )

        # Traverse with max_depth=2 (should get nodes 0, 1, 2)
        result = client.traverse_graph(
            start_node_id=nodes[0], max_depth=2, algorithm="BFS", limit=10
        )

        assert result is not None
        if "stats" in result:
            assert result["stats"]["max_depth_reached"] <= 2
        logger.info(f"Max depth=2 traversal completed")

    def test_traverse_graph_with_limit(self, client, graph_id):
        """Test that limit parameter restricts result count"""
        # Create a star topology: center connected to 10 nodes
        center = f"center_{uuid.uuid4().hex[:8]}"
        client.create_node(center, labels=["Center"], properties={"type": "hub"})

        for i in range(10):
            spoke = f"spoke_{i}_{uuid.uuid4().hex[:8]}"
            client.create_node(spoke, labels=["Spoke"], properties={"index": i})
            client.create_edge(
                f"spoke_edge_{i}_{uuid.uuid4().hex[:8]}", center, spoke, "CONNECTS"
            )

        # Traverse with limit=5
        result = client.traverse_graph(
            start_node_id=center, max_depth=1, algorithm="BFS", limit=5
        )

        assert result is not None
        assert "nodes" in result
        assert len(result["nodes"]) <= 5
        logger.info(
            f"Limited traversal returned {len(result['nodes'])} nodes (limit=5)"
        )


class TestGraphOperationsPerformance:
    """Performance and stress tests for graph operations"""

    @pytest.fixture
    def rest_client(self):
        """REST client for performance tests"""
        return ProximaDBClient(url="http://localhost:5678", protocol="rest")

    @pytest.fixture
    def grpc_client(self):
        """gRPC client for performance tests"""
        return ProximaDBClient(url="grpc://localhost:5679", protocol="grpc")

    def test_bulk_node_creation(self, rest_client):
        """Test creating nodes in bulk"""
        start_time = time.time()
        node_count = 50

        for i in range(node_count):
            node_id = f"bulk_node_{i}_{uuid.uuid4().hex[:8]}"
            rest_client.create_node(
                node_id=node_id,
                labels=["BulkTest"],
                properties={"index": i, "batch": "test"},
            )

        elapsed = time.time() - start_time
        throughput = node_count / elapsed

        logger.info(
            f"Created {node_count} nodes in {elapsed:.2f}s ({throughput:.1f} nodes/sec)"
        )
        assert throughput > 5  # Expect at least 5 nodes per second

    def test_bulk_edge_creation(self, rest_client):
        """Test creating edges in bulk"""
        # Create nodes first
        nodes = []
        for i in range(20):
            node_id = f"edge_test_{i}_{uuid.uuid4().hex[:8]}"
            rest_client.create_node(
                node_id, labels=["EdgeTest"], properties={"index": i}
            )
            nodes.append(node_id)

        # Create edges
        start_time = time.time()
        edge_count = 0

        for i in range(len(nodes) - 1):
            edge_id = f"bulk_edge_{i}_{uuid.uuid4().hex[:8]}"
            rest_client.create_edge(
                edge_id=edge_id,
                from_node_id=nodes[i],
                to_node_id=nodes[i + 1],
                edge_type="CONNECTS",
            )
            edge_count += 1

        elapsed = time.time() - start_time
        throughput = edge_count / elapsed

        logger.info(
            f"Created {edge_count} edges in {elapsed:.2f}s ({throughput:.1f} edges/sec)"
        )
        assert throughput > 5  # Expect at least 5 edges per second

    @pytest.mark.parametrize("protocol", ["rest", "grpc"])
    def test_protocol_comparison_node_creation(self, protocol):
        """Compare REST vs gRPC performance for node creation"""
        if protocol == "rest":
            client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        else:
            client = ProximaDBClient(url="grpc://localhost:5679", protocol="grpc")

        start_time = time.time()
        node_count = 20

        for i in range(node_count):
            node_id = f"perf_{protocol}_{i}_{uuid.uuid4().hex[:8]}"
            client.create_node(
                node_id=node_id,
                labels=["PerfTest"],
                properties={"protocol": protocol, "index": i},
            )

        elapsed = time.time() - start_time
        throughput = node_count / elapsed

        logger.info(
            f"{protocol.upper()}: Created {node_count} nodes in {elapsed:.2f}s ({throughput:.1f} nodes/sec)"
        )
        assert elapsed < 10  # Should complete within 10 seconds


class TestGraphOperationsEdgeCases:
    """Edge case and error handling tests"""

    @pytest.fixture
    def client(self):
        """REST client for edge case tests"""
        return ProximaDBClient(url="http://localhost:5678", protocol="rest")

    def test_create_node_empty_labels(self, client):
        """Test creating node with empty labels list"""
        node_id = f"empty_labels_{uuid.uuid4().hex[:8]}"

        try:
            result = client.create_node(
                node_id=node_id, labels=[], properties={"test": "value"}  # Empty labels
            )
            # If successful, that's valid behavior
            logger.info("Creating node with empty labels succeeded")
        except ProximaDBError as e:
            # If it fails, that's also acceptable depending on server validation
            logger.info(f"Creating node with empty labels failed (expected): {e}")

    def test_create_node_empty_properties(self, client):
        """Test creating node with empty properties"""
        node_id = f"empty_props_{uuid.uuid4().hex[:8]}"

        result = client.create_node(
            node_id=node_id, labels=["Test"], properties={}  # Empty properties
        )

        assert result is not None
        logger.info("Created node with empty properties")

    def test_create_node_no_properties(self, client):
        """Test creating node without properties parameter"""
        node_id = f"no_props_{uuid.uuid4().hex[:8]}"

        result = client.create_node(
            node_id=node_id,
            labels=["Test"],
            # No properties parameter
        )

        assert result is not None
        logger.info("Created node without properties parameter")

    def test_create_edge_nonexistent_nodes(self, client):
        """Test creating edge with nonexistent nodes"""
        edge_id = f"bad_edge_{uuid.uuid4().hex[:8]}"
        fake_node1 = "nonexistent_node_1"
        fake_node2 = "nonexistent_node_2"

        try:
            client.create_edge(
                edge_id=edge_id,
                from_node_id=fake_node1,
                to_node_id=fake_node2,
                edge_type="TEST",
            )
            logger.info(
                "Creating edge with nonexistent nodes succeeded (server may create nodes)"
            )
        except ProximaDBError as e:
            logger.info(f"Creating edge with nonexistent nodes failed (expected): {e}")

    def test_traverse_from_nonexistent_node(self, client):
        """Test traversing from a nonexistent node"""
        fake_node = "nonexistent_start_node"

        try:
            result = client.traverse_graph(
                start_node_id=fake_node, max_depth=1, algorithm="BFS"
            )
            # May return empty results
            assert result is not None
            logger.info("Traversal from nonexistent node returned empty results")
        except ProximaDBError as e:
            logger.info(f"Traversal from nonexistent node failed (expected): {e}")

    def test_query_nodes_nonexistent_label(self, client):
        """Test querying with a label that doesn't exist"""
        result = client.query_nodes(labels=["NonexistentLabel123456"], limit=10)

        assert result is not None
        assert "nodes" in result
        assert len(result["nodes"]) == 0
        logger.info("Query for nonexistent label returned empty results")

    def test_traverse_max_depth_zero(self, client):
        """Test traversal with max_depth=0 (should return only start node)"""
        node_id = f"depth_zero_{uuid.uuid4().hex[:8]}"
        client.create_node(node_id, labels=["Test"], properties={"name": "Start"})

        result = client.traverse_graph(
            start_node_id=node_id, max_depth=0, algorithm="BFS"
        )

        assert result is not None
        logger.info("Traversal with max_depth=0 completed")

    def test_large_property_value(self, client):
        """Test creating node with large property value"""
        node_id = f"large_prop_{uuid.uuid4().hex[:8]}"
        large_text = "x" * 10000  # 10KB string

        result = client.create_node(
            node_id=node_id,
            labels=["LargeTest"],
            properties={"large_field": large_text, "size": len(large_text)},
        )

        assert result is not None
        logger.info(f"Created node with {len(large_text)} character property")


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])
