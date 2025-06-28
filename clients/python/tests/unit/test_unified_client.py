"""
Unit tests for ProximaDB unified client.
Tests protocol selection and method delegation functionality.
"""

import pytest
from unittest.mock import Mock, patch
from proximadb.unified_client import ProximaDBClient, Protocol, connect, connect_grpc, connect_rest
from proximadb.config import ClientConfig
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ConfigurationError, NetworkError


class TestUnifiedClientInit:
    """Test unified client initialization and protocol selection"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_grpc_client')
    def test_auto_protocol_grpc_success(self, mock_create_grpc, mock_load_config):
        """Test auto protocol selection when gRPC is available"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_grpc_client = Mock()
        mock_create_grpc.return_value = mock_grpc_client
        
        client = ProximaDBClient("http://localhost:5678")
        
        assert client._client == mock_grpc_client
        assert client._active_protocol == Protocol.GRPC
        assert client.protocol == Protocol.AUTO
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_grpc_client')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_auto_protocol_fallback_to_rest(self, mock_create_rest, mock_create_grpc, mock_load_config):
        """Test auto protocol fallback to REST when gRPC fails"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_create_grpc.side_effect = ImportError("gRPC not available")
        mock_rest_client = Mock()
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678")
        
        assert client._client == mock_rest_client
        assert client._active_protocol == Protocol.REST
        assert client.protocol == Protocol.AUTO
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_force_rest_protocol(self, mock_create_rest, mock_load_config):
        """Test forcing REST protocol"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        assert client._client == mock_rest_client
        assert client._active_protocol == Protocol.REST
        assert client.protocol == Protocol.REST
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_grpc_client')
    def test_force_grpc_protocol(self, mock_create_grpc, mock_load_config):
        """Test forcing gRPC protocol"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_grpc_client = Mock()
        mock_create_grpc.return_value = mock_grpc_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.GRPC)
        
        assert client._client == mock_grpc_client
        assert client._active_protocol == Protocol.GRPC
        assert client.protocol == Protocol.GRPC
    
    @patch('proximadb.unified_client.load_config')
    def test_string_protocol_conversion(self, mock_load_config):
        """Test string protocol conversion to enum"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        with patch.object(ProximaDBClient, '_create_rest_client') as mock_create_rest:
            mock_rest_client = Mock()
            mock_create_rest.return_value = mock_rest_client
            
            client = ProximaDBClient("http://localhost:5678", protocol="rest")
            
            assert client.protocol == Protocol.REST
            assert isinstance(client.protocol, Protocol)


class TestUnifiedClientMethods:
    """Test unified client method delegation"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_health_delegation(self, mock_create_rest, mock_load_config):
        """Test health method delegation"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_health_result = Mock()
        mock_rest_client.health.return_value = mock_health_result
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        result = client.health()
        
        mock_rest_client.health.assert_called_once()
        assert result == mock_health_result
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_list_collections_delegation(self, mock_create_rest, mock_load_config):
        """Test list_collections delegation"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collections = ["collection1", "collection2"]
        mock_rest_client.list_collections.return_value = mock_collections
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        result = client.list_collections()
        
        mock_rest_client.list_collections.assert_called_once()
        assert result == mock_collections
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_create_collection_delegation(self, mock_create_rest, mock_load_config):
        """Test create_collection delegation"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_collection = Mock()
        mock_rest_client.create_collection.return_value = mock_collection
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        config = CollectionConfig(dimension=768, distance_metric=DistanceMetric.COSINE)
        result = client.create_collection("test_collection", config)
        
        mock_rest_client.create_collection.assert_called_once_with("test_collection", config)
        assert result == mock_collection
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_search_delegation(self, mock_create_rest, mock_load_config):
        """Test search method delegation"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_results = [Mock(), Mock()]
        mock_rest_client.search.return_value = mock_results
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        query = [0.1, 0.2, 0.3]
        result = client.search("test_collection", query, k=10)
        
        mock_rest_client.search.assert_called_once_with(
            "test_collection", query, 10, None, False, True, None, False, None
        )
        assert result == mock_results


class TestUnifiedClientProperties:
    """Test unified client properties and utilities"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_grpc_client')
    def test_active_protocol_property(self, mock_create_grpc, mock_load_config):
        """Test active_protocol property"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_grpc_client = Mock()
        mock_create_grpc.return_value = mock_grpc_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.GRPC)
        
        assert client.active_protocol == Protocol.GRPC
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_grpc_client')
    def test_performance_info_grpc(self, mock_create_grpc, mock_load_config):
        """Test performance info for gRPC protocol"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_grpc_client = Mock()
        mock_create_grpc.return_value = mock_grpc_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.GRPC)
        perf_info = client.get_performance_info()
        
        assert perf_info["protocol"] == "gRPC"
        assert "Binary Protocol Buffers" in perf_info["serialization"]
        assert "HTTP/2" in perf_info["transport"]
        assert "advantages" in perf_info
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_performance_info_rest(self, mock_create_rest, mock_load_config):
        """Test performance info for REST protocol"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        perf_info = client.get_performance_info()
        
        assert perf_info["protocol"] == "REST"
        assert "JSON" in perf_info["serialization"]
        assert "HTTP/1.1" in perf_info["transport"]
        assert "advantages" in perf_info


class TestUnifiedClientContextManager:
    """Test unified client context manager functionality"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_context_manager(self, mock_create_rest, mock_load_config):
        """Test context manager functionality"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.close = Mock()
        mock_create_rest.return_value = mock_rest_client
        
        with ProximaDBClient("http://localhost:5678", protocol=Protocol.REST) as client:
            assert client._client == mock_rest_client
        
        # Close should be called on exit
        mock_rest_client.close.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_close_method(self, mock_create_rest, mock_load_config):
        """Test explicit close method"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.close = Mock()
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        client.close()
        
        mock_rest_client.close.assert_called_once()


class TestConvenienceFunctions:
    """Test convenience connection functions"""
    
    @patch('proximadb.unified_client.ProximaDBClient')
    def test_connect_function(self, mock_client_class):
        """Test connect convenience function"""
        mock_client = Mock()
        mock_client_class.return_value = mock_client
        
        result = connect("http://localhost:5678", api_key="test-key-1234567890")
        
        mock_client_class.assert_called_once_with(
            url="http://localhost:5678",
            api_key="test-key-1234567890",
            protocol=Protocol.AUTO
        )
        assert result == mock_client
    
    @patch('proximadb.unified_client.ProximaDBClient')
    def test_connect_grpc_function(self, mock_client_class):
        """Test connect_grpc convenience function"""
        mock_client = Mock()
        mock_client_class.return_value = mock_client
        
        result = connect_grpc("http://localhost:5678")
        
        mock_client_class.assert_called_once_with(
            url="http://localhost:5678",
            api_key=None,
            protocol=Protocol.GRPC
        )
        assert result == mock_client
    
    @patch('proximadb.unified_client.ProximaDBClient')
    def test_connect_rest_function(self, mock_client_class):
        """Test connect_rest convenience function"""
        mock_client = Mock()
        mock_client_class.return_value = mock_client
        
        result = connect_rest("http://localhost:5678")
        
        mock_client_class.assert_called_once_with(
            url="http://localhost:5678",
            api_key=None,
            protocol=Protocol.REST
        )
        assert result == mock_client


class TestErrorHandling:
    """Test error handling scenarios"""
    
    @patch('proximadb.unified_client.load_config')
    @patch.object(ProximaDBClient, '_create_rest_client')
    def test_error_propagation(self, mock_create_rest, mock_load_config):
        """Test that underlying client errors are propagated"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        mock_rest_client = Mock()
        mock_rest_client.list_collections.side_effect = NetworkError("Connection failed")
        mock_create_rest.return_value = mock_rest_client
        
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        with pytest.raises(NetworkError, match="Connection failed"):
            client.list_collections()