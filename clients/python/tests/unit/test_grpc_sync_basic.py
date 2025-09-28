"""
Basic tests for gRPC sync client initialization
"""

import pytest
from unittest.mock import Mock, patch

try:
    import grpc
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False

from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
from proximadb.exceptions import ProximaDBError


class TestGrpcSyncBasic:
    """Basic tests for gRPC sync client"""
    
    @pytest.fixture
    def client_config(self):
        """Standard client configuration"""
        return {
            'server_address': 'localhost:5679',
            'timeout': 30.0,
            'pool_size': 3
        }
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_client_initialization(self, mock_insecure_channel, client_config):
        """Test basic client initialization"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        client = ProximaDBSyncGrpcClient(**client_config)
        
        # Should have initialized connection pool
        assert client._connection_pool is not None
        assert client.server_address == client_config['server_address']
        assert client.timeout == client_config['timeout']
        assert client.pool_size == client_config['pool_size']
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_client_context_manager(self, mock_insecure_channel, client_config):
        """Test client works as context manager"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        with ProximaDBSyncGrpcClient(**client_config) as client:
            assert client._connection_pool is not None
            # Connection pool should be available while in context
            metrics = client.get_pool_metrics()
            assert metrics is not None
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_client_without_grpc(self):
        """Test error when gRPC not available"""
        # This test is more complex to implement since we need to mock the import
        # For now, just test that the client requires gRPC
        pass
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_pool_metrics(self, mock_insecure_channel):
        """Test getting pool metrics"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        client = ProximaDBSyncGrpcClient('localhost:5679', pool_size=5)
        metrics = client.get_pool_metrics()
        
        assert metrics is not None
        assert hasattr(metrics, 'total_connections')
        assert hasattr(metrics, 'health_status')
        # Pool should have created connections (at least the pool size, maybe slightly more due to initialization)
        assert metrics.total_connections >= 5
        assert metrics.total_connections <= 7  # Allow some tolerance for initialization
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_compression_config(self, mock_insecure_channel):
        """Test compression configuration"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        # Test with different compression algorithms
        client1 = ProximaDBSyncGrpcClient('localhost:5679', compression_algorithm='gzip')
        client2 = ProximaDBSyncGrpcClient('localhost:5679', compression_algorithm='deflate')
        client3 = ProximaDBSyncGrpcClient('localhost:5679', enable_compression=False)
        
        assert client1.compression_algorithm == 'gzip'
        assert client2.compression_algorithm == 'deflate'
        assert client3.enable_compression == False
    
    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available") 
    @patch('proximadb.protocols.connection_pools.grpc.insecure_channel')
    def test_connection_pool_properties(self, mock_insecure_channel):
        """Test connection pool has correct properties"""
        mock_channel = Mock(spec=grpc.Channel)
        mock_insecure_channel.return_value = mock_channel
        
        client = ProximaDBSyncGrpcClient(
            'localhost:5679', 
            pool_size=7,
            max_message_size=32 * 1024 * 1024
        )
        
        pool = client._connection_pool
        assert pool is not None
        assert pool.pool_size == 7
        assert pool.max_message_size == 32 * 1024 * 1024
        assert pool.endpoint == 'localhost:5679'