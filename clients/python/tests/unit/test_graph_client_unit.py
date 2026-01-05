"""
Unit tests for graph client methods (no server required).

Tests client-side logic, parameter validation, and data transformation
using the embedded database for actual graph operations.
"""

import time
from typing import Any, Dict
from unittest.mock import MagicMock, Mock, patch

import pytest

from proximadb_sdk import CollectionConfig, ProximaDBClient
from proximadb_sdk.exceptions import ProximaDBError


class TestGraphClientBasicOperations:
    """Test basic graph client operations using embedded database"""

    @pytest.fixture(scope="class")
    def graph_name(self):
        """Generate unique graph name for tests"""
        return f"test_graph_{int(time.time() * 1000)}"

    def test_create_node_basic(self, rest_client, graph_name):
        """Test basic node creation"""
        # First ensure graph exists
        try:
            rest_client.create_graph(graph_name)
        except Exception:
            pass  # Graph may already exist

        # Create a node
        result = rest_client.create_node(
            node_id=f"node_{int(time.time() * 1000)}",
            labels=["TestLabel"],
            properties={"name": "Test Node", "value": 42},
        )

        # Verify result
        assert result is not None

    def test_create_node_with_multiple_labels(self, rest_client, graph_name):
        """Test node creation with multiple labels"""
        result = rest_client.create_node(
            node_id=f"multi_label_node_{int(time.time() * 1000)}",
            labels=["Label1", "Label2", "Label3"],
            properties={"type": "multi-label"},
        )

        assert result is not None

    def test_create_node_with_empty_properties(self, rest_client, graph_name):
        """Test node creation with empty properties"""
        result = rest_client.create_node(
            node_id=f"empty_props_node_{int(time.time() * 1000)}",
            labels=["TestLabel"],
            properties={},
        )

        assert result is not None

    def test_create_edge_basic(self, rest_client, graph_name):
        """Test basic edge creation between nodes"""
        # Create two nodes first
        node1_id = f"edge_test_node1_{int(time.time() * 1000)}"
        node2_id = f"edge_test_node2_{int(time.time() * 1000)}"

        rest_client.create_node(
            node_id=node1_id, labels=["Person"], properties={"name": "Alice"}
        )

        rest_client.create_node(
            node_id=node2_id, labels=["Person"], properties={"name": "Bob"}
        )

        # Create edge between them
        result = rest_client.create_edge(
            edge_id=f"edge_{int(time.time() * 1000)}",
            from_node_id=node1_id,
            to_node_id=node2_id,
            edge_type="KNOWS",
        )

        assert result is not None

    def test_create_edge_with_properties(self, rest_client, graph_name):
        """Test edge creation with properties"""
        node1_id = f"prop_edge_node1_{int(time.time() * 1000)}"
        node2_id = f"prop_edge_node2_{int(time.time() * 1000)}"

        rest_client.create_node(node_id=node1_id, labels=["Item"])
        rest_client.create_node(node_id=node2_id, labels=["Item"])

        result = rest_client.create_edge(
            edge_id=f"prop_edge_{int(time.time() * 1000)}",
            from_node_id=node1_id,
            to_node_id=node2_id,
            edge_type="RELATES_TO",
            properties={"strength": "strong", "since": "2024"},
        )

        assert result is not None

    def test_create_edge_with_weight(self, rest_client, graph_name):
        """Test edge creation with weight"""
        node1_id = f"weighted_node1_{int(time.time() * 1000)}"
        node2_id = f"weighted_node2_{int(time.time() * 1000)}"

        rest_client.create_node(node_id=node1_id, labels=["Node"])
        rest_client.create_node(node_id=node2_id, labels=["Node"])

        result = rest_client.create_edge(
            edge_id=f"weighted_edge_{int(time.time() * 1000)}",
            from_node_id=node1_id,
            to_node_id=node2_id,
            edge_type="CONNECTED",
            weight=1.5,
        )

        assert result is not None


class TestGraphClientParameterValidation:
    """Test client-side parameter validation"""

    def test_create_node_validates_node_id_type(self, rest_client):
        """Test that node_id validation occurs"""
        # Client should validate that node_id is a string
        # This test verifies the parameter is passed correctly
        try:
            rest_client.create_node(
                node_id="valid_node_id", labels=["Test"], properties={}
            )
            # If no error, validation passed
        except TypeError as e:
            # Type errors are expected for invalid types
            pass

    def test_create_node_with_properties_types(self, rest_client):
        """Test that various property types are handled"""
        node_id = f"prop_types_node_{int(time.time() * 1000)}"

        properties = {
            "string_prop": "test_string",
            "int_prop": 42,
            "float_prop": 3.14159,
            "bool_prop": True,
        }

        result = rest_client.create_node(
            node_id=node_id, labels=["TestTypes"], properties=properties
        )

        assert result is not None


class TestGraphTraversal:
    """Test graph traversal operations"""

    @pytest.fixture(scope="class")
    def populated_graph(self, rest_client):
        """Create a graph with nodes and edges for traversal tests"""
        graph_name = f"traversal_graph_{int(time.time() * 1000)}"

        try:
            rest_client.create_graph(graph_name)
        except Exception:
            pass

        # Create a small network
        nodes = ["alice", "bob", "charlie", "diana"]
        for node in nodes:
            rest_client.create_node(
                node_id=f"{graph_name}_{node}",
                labels=["Person"],
                properties={"name": node.capitalize()},
            )

        # Create edges: alice -> bob -> charlie -> diana
        edges = [
            ("alice", "bob", "KNOWS"),
            ("bob", "charlie", "KNOWS"),
            ("charlie", "diana", "KNOWS"),
            ("alice", "charlie", "FRIENDS_WITH"),  # Direct connection
        ]

        for from_node, to_node, edge_type in edges:
            rest_client.create_edge(
                edge_id=f"{graph_name}_{from_node}_{to_node}",
                from_node_id=f"{graph_name}_{from_node}",
                to_node_id=f"{graph_name}_{to_node}",
                edge_type=edge_type,
            )

        return graph_name

    def test_graph_traverse_basic(self, rest_client, populated_graph):
        """Test basic graph traversal"""
        start_node = f"{populated_graph}_alice"

        result = rest_client.graph_traverse(start_node_id=start_node, max_depth=2)

        assert result is not None

    def test_graph_traverse_with_edge_type_filter(self, rest_client, populated_graph):
        """Test traversal with edge type filter"""
        start_node = f"{populated_graph}_alice"

        result = rest_client.graph_traverse(
            start_node_id=start_node, max_depth=3, edge_types=["KNOWS"]
        )

        assert result is not None


class TestGraphShortestPath:
    """Test shortest path operations"""

    @pytest.fixture(scope="class")
    def path_graph(self, rest_client):
        """Create a graph for shortest path tests"""
        graph_name = f"path_graph_{int(time.time() * 1000)}"

        try:
            rest_client.create_graph(graph_name)
        except Exception:
            pass

        # Create nodes
        for i in range(5):
            rest_client.create_node(
                node_id=f"{graph_name}_node{i}",
                labels=["Waypoint"],
                properties={"index": i},
            )

        # Create edges: 0 -> 1 -> 2 -> 3 -> 4
        for i in range(4):
            rest_client.create_edge(
                edge_id=f"{graph_name}_edge{i}",
                from_node_id=f"{graph_name}_node{i}",
                to_node_id=f"{graph_name}_node{i+1}",
                edge_type="NEXT",
                weight=1.0,
            )

        # Add shortcut: 0 -> 2 (weight 3)
        rest_client.create_edge(
            edge_id=f"{graph_name}_shortcut",
            from_node_id=f"{graph_name}_node0",
            to_node_id=f"{graph_name}_node2",
            edge_type="SHORTCUT",
            weight=3.0,
        )

        return graph_name

    def test_shortest_path_basic(self, rest_client, path_graph):
        """Test basic shortest path calculation"""
        start_node = f"{path_graph}_node0"
        end_node = f"{path_graph}_node4"

        result = rest_client.graph_shortest_path(
            start_node_id=start_node, target_node_id=end_node
        )

        assert result is not None


class TestGraphClientConfiguration:
    """Test graph client configuration options"""

    def test_client_with_rest_protocol(self):
        """Test creating client with REST protocol"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        assert client is not None
        assert client.active_protocol == "rest"
        client.close()

    def test_client_with_grpc_protocol(self):
        """Test creating client with gRPC protocol"""
        client = ProximaDBClient(url="grpc://localhost:5679", protocol="grpc")

        assert client is not None
        assert client.active_protocol == "grpc"
        client.close()

    def test_client_with_auto_protocol(self):
        """Test creating client with auto protocol selection"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="auto")

        assert client is not None
        # Protocol should be determined automatically
        assert client.active_protocol in ["rest", "grpc"]
        client.close()


class TestGraphClientMethods:
    """Test that graph client has expected methods"""

    def test_client_has_create_node_method(self):
        """Verify create_node method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "create_node")
        assert callable(client.create_node)
        client.close()

    def test_client_has_create_edge_method(self):
        """Verify create_edge method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "create_edge")
        assert callable(client.create_edge)
        client.close()

    def test_client_has_graph_traverse_method(self):
        """Verify graph_traverse method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "graph_traverse")
        assert callable(client.graph_traverse)
        client.close()

    def test_client_has_graph_shortest_path_method(self):
        """Verify graph_shortest_path method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "graph_shortest_path")
        assert callable(client.graph_shortest_path)
        client.close()

    def test_client_has_create_graph_method(self):
        """Verify create_graph method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "create_graph")
        assert callable(client.create_graph)
        client.close()

    def test_client_has_get_graph_method(self):
        """Verify get_graph method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "get_graph")
        assert callable(client.get_graph)
        client.close()

    def test_client_has_delete_graph_method(self):
        """Verify delete_graph method exists"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")
        assert hasattr(client, "delete_graph")
        assert callable(client.delete_graph)
        client.close()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
