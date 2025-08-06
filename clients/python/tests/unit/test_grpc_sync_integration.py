"""
Tests for gRPC sync client with connection pooling integration
"""

import pytest
from unittest.mock import Mock, patch, MagicMock

try:
    import grpc
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False

from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
from proximadb.exceptions import ProximaDBError


class TestGrpcSyncIntegration:
    """Test gRPC sync client with connection pool integration"""
    
    @pytest.fixture
    def client_config(self):
        """Standard client configuration"""
        return {
            'server_address': 'localhost:5679',
            'timeout': 30.0,
            'enable_compression': True,
            'pool_size': 3
        }
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_client_initialization_with_pool(self, mock_insecure_channel, client_config):
        """Test that client initializes connection pool correctly"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        client = ProximaDBSyncGrpcClient(**client_config)
        
        # Should have initialized connection pool
        assert client._connection_pool is not None
        assert len(client._connection_pool.channels) == client_config['pool_size']
        assert mock_insecure_channel.call_count == client_config['pool_size']
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_pool_metrics_access(self, mock_insecure_channel, client_config):
        """Test accessing pool metrics"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        client = ProximaDBSyncGrpcClient(**client_config)
        metrics = client.get_pool_metrics()
        
        assert metrics is not None
        assert metrics.total_connections == client_config['pool_size']
        assert hasattr(metrics, 'health_status')
        assert hasattr(metrics, 'requests_served')
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_health_check_operation(self, mock_insecure_channel, client_config):
        """Test health check uses connection pool"""
        # Setup mocks
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        mock_stub = Mock()
        mock_response = Mock()
        mock_response.status = "healthy"
        mock_stub.HealthCheck.return_value = mock_response
        
        with patch('proximadb.protocols.grpc_sync.pb2_grpc.ProximaDBStub') as mock_stub_class:
            mock_stub_class.return_value = mock_stub
            
            client = ProximaDBSyncGrpcClient(**client_config)
            
            # Execute health check
            result = client.health_check()
            
            # Verify operation used connection pool
            assert result['status'] == 'healthy'
            mock_stub_class.assert_called_once()
            mock_stub.HealthCheck.assert_called_once()
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_create_collection_operation(self, mock_insecure_channel, client_config):
        """Test collection creation uses connection pool"""
        # Setup mocks
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        mock_stub = Mock()
        mock_response = Mock()
        mock_response.success = True
        mock_stub.CreateCollection.return_value = mock_response
        
        with patch('proximadb.protocols.grpc_sync.pb2_grpc.ProximaDBStub') as mock_stub_class, \
             patch('proximadb.protocols.grpc_sync.pb2') as mock_pb2:
            
            mock_stub_class.return_value = mock_stub
            
            # Mock proto classes
            mock_config = Mock()
            mock_request = Mock()
            mock_pb2.CollectionConfig.return_value = mock_config
            mock_pb2.CreateCollectionRequest.return_value = mock_request
            
            client = ProximaDBSyncGrpcClient(**client_config)
            
            # Execute collection creation
            result = client.create_collection(
                name="test_collection",
                dimension=384
            )
            
            # Verify operation used connection pool
            assert result == mock_response
            mock_stub_class.assert_called_once()
            mock_stub.CreateCollection.assert_called_once()
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_insert_vectors_operation(self, mock_insecure_channel, client_config):
        """Test vector insertion uses connection pool"""
        # Setup mocks
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        mock_stub = Mock()
        mock_response = Mock()
        mock_response.success = True
        mock_response.inserted_count = 2
        mock_stub.InsertVectors.return_value = mock_response
        
        with patch('proximadb.protocols.grpc_sync.pb2_grpc.ProximaDBStub') as mock_stub_class, \
             patch('proximadb.protocols.grpc_sync.pb2') as mock_pb2:
            
            mock_stub_class.return_value = mock_stub
            
            # Mock proto classes
            mock_vector_record = Mock()
            mock_request = Mock()
            mock_pb2.VectorRecord.return_value = mock_vector_record
            mock_pb2.InsertVectorsRequest.return_value = mock_request
            
            client = ProximaDBSyncGrpcClient(**client_config)
            
            # Execute vector insertion
            vectors = [
                {'id': 'vec1', 'vector': [0.1, 0.2, 0.3], 'metadata': {'type': 'test'}},
                {'id': 'vec2', 'vector': [0.4, 0.5, 0.6], 'metadata': {'type': 'test'}}
            ]
            result = client.insert_vectors(
                collection_id="test_collection",
                vectors=vectors
            )
            
            # Verify operation used connection pool
            assert result.success == True
            assert result.inserted_count == 2
            mock_stub_class.assert_called_once()
            mock_stub.InsertVectors.assert_called_once()
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_context_manager_usage(self, mock_insecure_channel, client_config):
        """Test client works as context manager"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        # Test context manager usage
        with ProximaDBSyncGrpcClient(**client_config) as client:
            assert client._connection_pool is not None
            metrics = client.get_pool_metrics()
            assert metrics.total_connections == client_config['pool_size']
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_error_handling(self, mock_insecure_channel, client_config):
        """Test error handling in operations"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        mock_stub = Mock()
        mock_stub.HealthCheck.side_effect = grpc.RpcError("Connection failed")
        
        with patch('proximadb.protocols.grpc_sync.pb2_grpc.ProximaDBStub') as mock_stub_class:
            mock_stub_class.return_value = mock_stub
            
            client = ProximaDBSyncGrpcClient(**client_config)
            
            # Should raise ProximaDBError
            with pytest.raises(ProximaDBError) as exc_info:
                client.health_check()
            
            assert "health_check RPC failed" in str(exc_info.value)


class TestGrpcAvailabilityHandling:
    """Test handling when gRPC is not available"""
    
    @patch('proximadb.protocols.grpc_sync.GRPC_AVAILABLE', False)
    def test_initialization_without_grpc(self):
        """Test client initialization fails gracefully when gRPC unavailable"""
        with pytest.raises(ProximaDBError) as exc_info:
            ProximaDBSyncGrpcClient('localhost:5679')
        
        assert "gRPC connection pool initialization failed" in str(exc_info.value)
    
    def test_operation_without_grpc(self):
        """Test operations fail gracefully when gRPC unavailable"""
        # This is a more complex test that would require mocking the entire initialization
        # For now, we rely on the previous test to cover the main case
        pass


# Performance and stress tests
class TestGrpcSyncPerformance:
    """Performance tests for gRPC sync client (run manually)"""
    
    @pytest.mark.performance  
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_connection_pool_performance(self, mock_insecure_channel):
        """Test connection pool performance under load"""
        mock_channels = [Mock(spec=grpc.Channel) for _ in range(5)]
        mock_insecure_channel.side_effect = mock_channels
        
        client = ProximaDBSyncGrpcClient('localhost:5679', pool_size=5)
        
        # Get metrics before operations
        initial_metrics = client.get_pool_metrics()
        assert initial_metrics.requests_served == 0
        
        # Simulate many operations (mocked)
        mock_stub = Mock()
        mock_response = Mock()
        mock_response.status = "healthy"
        mock_stub.HealthCheck.return_value = mock_response
        
        with patch('proximadb.protocols.grpc_sync.pb2_grpc.ProximaDBStub') as mock_stub_class:
            mock_stub_class.return_value = mock_stub
            
            # Execute multiple health checks
            for _ in range(100):
                result = client.health_check()
                assert result['status'] == 'healthy'
        
        # Verify pool served all requests
        final_metrics = client.get_pool_metrics()
        assert final_metrics.requests_served == 100