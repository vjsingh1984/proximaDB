"""
Unit tests for graph client methods (no server required).

Tests client-side logic, parameter validation, and data transformation.
"""

import pytest
from unittest.mock import Mock, MagicMock, patch
from typing import Dict, Any

from proximadb import ProximaDBClient
from proximadb.exceptions import ProximaDBError


class TestGraphClientParameterValidation:
    """Test client-side parameter validation and type checking"""

    def test_create_node_validates_node_id_type(self):
        """Test that node_id must be a string"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with pytest.raises((TypeError, ValueError)):
            client.create_node(
                node_id=12345,  # Should be string
                labels=["Test"],
                properties={}
            )

    def test_create_node_validates_labels_type(self):
        """Test that labels must be a list"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with pytest.raises((TypeError, ValueError, AttributeError)):
            client.create_node(
                node_id="test_node",
                labels="NotAList",  # Should be list
                properties={}
            )

    def test_create_edge_validates_edge_id_type(self):
        """Test that edge_id must be a string"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with pytest.raises((TypeError, ValueError)):
            client.create_edge(
                edge_id=12345,  # Should be string
                from_node_id="node1",
                to_node_id="node2",
                edge_type="TEST"
            )

    def test_create_edge_validates_edge_type(self):
        """Test that edge_type must be a string"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with pytest.raises((TypeError, ValueError)):
            client.create_edge(
                edge_id="edge1",
                from_node_id="node1",
                to_node_id="node2",
                edge_type=123  # Should be string
            )

    def test_traverse_graph_validates_max_depth(self):
        """Test that max_depth must be a positive integer"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        # These should raise TypeError or ValueError
        invalid_depths = ["not_an_int", -1, 3.14]

        for invalid_depth in invalid_depths:
            with pytest.raises((TypeError, ValueError)):
                with patch.object(client, '_traverse_graph_rest', return_value={}):
                    client.traverse_graph(
                        start_node_id="test",
                        max_depth=invalid_depth
                    )

    def test_traverse_graph_validates_algorithm(self):
        """Test that algorithm parameter accepts valid values"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        valid_algorithms = ["BFS", "DFS", "bfs", "dfs", "PARALLEL_BFS"]

        for algo in valid_algorithms:
            try:
                with patch.object(client, '_traverse_graph_rest', return_value={}):
                    result = client.traverse_graph(
                        start_node_id="test",
                        algorithm=algo
                    )
                    assert result is not None
            except Exception as e:
                pytest.fail(f"Valid algorithm '{algo}' should not raise exception: {e}")

    def test_query_nodes_validates_limit_type(self):
        """Test that limit must be None or positive integer"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with pytest.raises((TypeError, ValueError)):
            with patch.object(client, '_query_nodes_rest', return_value={}):
                client.query_nodes(
                    labels=["Test"],
                    limit="not_an_int"  # Should be int or None
                )

    def test_query_nodes_validates_offset_type(self):
        """Test that offset must be None or non-negative integer"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with pytest.raises((TypeError, ValueError)):
            with patch.object(client, '_query_nodes_rest', return_value={}):
                client.query_nodes(
                    labels=["Test"],
                    offset="not_an_int"  # Should be int or None
                )


class TestGraphClientDataTransformation:
    """Test data transformation and conversion methods"""

    def test_convert_property_value_string(self):
        """Test converting string property value"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="grpc")

        prop_value = client._convert_to_property_value("test_string")

        assert prop_value is not None
        assert hasattr(prop_value, 'string_value') or hasattr(prop_value, 'value')

    def test_convert_property_value_int(self):
        """Test converting integer property value"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="grpc")

        prop_value = client._convert_to_property_value(42)

        assert prop_value is not None

    def test_convert_property_value_float(self):
        """Test converting float property value"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="grpc")

        prop_value = client._convert_to_property_value(3.14159)

        assert prop_value is not None

    def test_convert_property_value_bool(self):
        """Test converting boolean property value"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="grpc")

        prop_value_true = client._convert_to_property_value(True)
        prop_value_false = client._convert_to_property_value(False)

        assert prop_value_true is not None
        assert prop_value_false is not None

    def test_convert_property_value_none(self):
        """Test converting None property value"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="grpc")

        prop_value = client._convert_to_property_value(None)

        assert prop_value is not None

    def test_properties_dict_conversion(self):
        """Test converting full properties dictionary"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="grpc")

        properties = {
            "name": "Alice",
            "age": 30,
            "salary": 75000.50,
            "active": True,
            "notes": None
        }

        # Convert all properties
        for key, value in properties.items():
            prop_value = client._convert_to_property_value(value)
            assert prop_value is not None


class TestGraphClientProtocolRouting:
    """Test that client routes calls to correct protocol implementations"""

    def test_create_node_routes_to_grpc(self):
        """Test that gRPC client routes create_node to gRPC implementation"""
        client = ProximaDBClient(url="localhost:5679", protocol="grpc")

        with patch.object(client, '_create_node_grpc', return_value={}) as mock_grpc:
            client.create_node(
                node_id="test",
                labels=["Test"],
                properties={}
            )

            mock_grpc.assert_called_once()

    def test_create_node_routes_to_rest(self):
        """Test that REST client routes create_node to REST implementation"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_node_rest', return_value={}) as mock_rest:
            client.create_node(
                node_id="test",
                labels=["Test"],
                properties={}
            )

            mock_rest.assert_called_once()

    def test_create_edge_routes_to_grpc(self):
        """Test that gRPC client routes create_edge to gRPC implementation"""
        client = ProximaDBClient(url="localhost:5679", protocol="grpc")

        with patch.object(client, '_create_edge_grpc', return_value={}) as mock_grpc:
            client.create_edge(
                edge_id="edge1",
                from_node_id="node1",
                to_node_id="node2",
                edge_type="TEST"
            )

            mock_grpc.assert_called_once()

    def test_create_edge_routes_to_rest(self):
        """Test that REST client routes create_edge to REST implementation"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_edge_rest', return_value={}) as mock_rest:
            client.create_edge(
                edge_id="edge1",
                from_node_id="node1",
                to_node_id="node2",
                edge_type="TEST"
            )

            mock_rest.assert_called_once()

    def test_traverse_graph_routes_to_grpc(self):
        """Test that gRPC client routes traverse_graph to gRPC implementation"""
        client = ProximaDBClient(url="localhost:5679", protocol="grpc")

        with patch.object(client, '_traverse_graph_grpc', return_value={}) as mock_grpc:
            client.traverse_graph(start_node_id="test")

            mock_grpc.assert_called_once()

    def test_traverse_graph_routes_to_rest(self):
        """Test that REST client routes traverse_graph to REST implementation"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_traverse_graph_rest', return_value={}) as mock_rest:
            client.traverse_graph(start_node_id="test")

            mock_rest.assert_called_once()

    def test_query_nodes_routes_to_grpc(self):
        """Test that gRPC client routes query_nodes to gRPC implementation"""
        client = ProximaDBClient(url="localhost:5679", protocol="grpc")

        with patch.object(client, '_query_nodes_grpc', return_value={}) as mock_grpc:
            client.query_nodes(labels=["Test"])

            mock_grpc.assert_called_once()

    def test_query_nodes_routes_to_rest(self):
        """Test that REST client routes query_nodes to REST implementation"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_query_nodes_rest', return_value={}) as mock_rest:
            client.query_nodes(labels=["Test"])

            mock_rest.assert_called_once()


class TestGraphClientMethodSignatures:
    """Test that method signatures are correct and handle optional parameters"""

    def test_create_node_minimal_params(self):
        """Test create_node with minimal required parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_node_rest', return_value={}) as mock:
            client.create_node(
                node_id="test",
                labels=["Test"]
                # properties is optional
            )

            mock.assert_called_once()

    def test_create_node_with_all_params(self):
        """Test create_node with all parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_node_rest', return_value={}) as mock:
            client.create_node(
                node_id="test",
                labels=["Test", "Multiple"],
                properties={"key": "value"},
                embedding=[0.1, 0.2, 0.3]
            )

            mock.assert_called_once()

    def test_create_edge_minimal_params(self):
        """Test create_edge with minimal required parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_edge_rest', return_value={}) as mock:
            client.create_edge(
                edge_id="edge1",
                from_node_id="node1",
                to_node_id="node2",
                edge_type="CONNECTS"
                # properties and weight are optional
            )

            mock.assert_called_once()

    def test_create_edge_with_all_params(self):
        """Test create_edge with all parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_edge_rest', return_value={}) as mock:
            client.create_edge(
                edge_id="edge1",
                from_node_id="node1",
                to_node_id="node2",
                edge_type="CONNECTS",
                properties={"strength": "strong"},
                weight=1.5
            )

            mock.assert_called_once()

    def test_traverse_graph_minimal_params(self):
        """Test traverse_graph with minimal required parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_traverse_graph_rest', return_value={}) as mock:
            client.traverse_graph(start_node_id="test")

            mock.assert_called_once()

    def test_traverse_graph_with_all_params(self):
        """Test traverse_graph with all parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_traverse_graph_rest', return_value={}) as mock:
            client.traverse_graph(
                start_node_id="test",
                max_depth=5,
                edge_types=["KNOWS", "WORKS_WITH"],
                node_labels=["Person", "Company"],
                algorithm="DFS",
                limit=100
            )

            mock.assert_called_once()

    def test_query_nodes_minimal_params(self):
        """Test query_nodes with no parameters (returns all nodes)"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_query_nodes_rest', return_value={}) as mock:
            client.query_nodes()

            mock.assert_called_once()

    def test_query_nodes_with_all_params(self):
        """Test query_nodes with all parameters"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_query_nodes_rest', return_value={}) as mock:
            client.query_nodes(
                labels=["Person"],
                properties={"age": 30},
                limit=50,
                offset=10
            )

            mock.assert_called_once()


class TestGraphClientEdgeCases:
    """Test edge cases and boundary conditions"""

    def test_create_node_with_empty_label_list(self):
        """Test behavior with empty labels list"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_node_rest', return_value={}) as mock:
            client.create_node(
                node_id="test",
                labels=[],  # Empty list
                properties={}
            )

            mock.assert_called_once()

    def test_create_node_with_empty_properties(self):
        """Test creating node with empty properties dict"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_create_node_rest', return_value={}) as mock:
            client.create_node(
                node_id="test",
                labels=["Test"],
                properties={}  # Empty dict
            )

            mock.assert_called_once()

    def test_traverse_graph_with_empty_edge_types_list(self):
        """Test traversal with empty edge_types list"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_traverse_graph_rest', return_value={}) as mock:
            client.traverse_graph(
                start_node_id="test",
                edge_types=[]  # Empty list (no filtering)
            )

            mock.assert_called_once()

    def test_traverse_graph_with_empty_node_labels_list(self):
        """Test traversal with empty node_labels list"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_traverse_graph_rest', return_value={}) as mock:
            client.traverse_graph(
                start_node_id="test",
                node_labels=[]  # Empty list (no filtering)
            )

            mock.assert_called_once()

    def test_query_nodes_with_none_labels(self):
        """Test querying with None labels (should query all nodes)"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_query_nodes_rest', return_value={}) as mock:
            client.query_nodes(labels=None)

            mock.assert_called_once()

    def test_query_nodes_with_none_properties(self):
        """Test querying with None properties (no property filtering)"""
        client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

        with patch.object(client, '_query_nodes_rest', return_value={}) as mock:
            client.query_nodes(
                labels=["Test"],
                properties=None
            )

            mock.assert_called_once()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
