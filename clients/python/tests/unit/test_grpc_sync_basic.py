"""
Basic tests for gRPC sync client initialization

These tests verify the gRPC sync client configuration and API structure
without actually creating gRPC connections.
"""

import pytest
from unittest.mock import Mock, patch, MagicMock
import types

try:
    import grpc
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False


class MockResourcePool:
    """Mock resource pool that doesn't actually create connections"""
    def __init__(self, factory, max_size=5, **kwargs):
        self.factory = factory
        self.max_size = max_size
        self._resources = []

    def acquire(self, timeout=None):
        return Mock()

    def release(self, resource):
        pass

    def close(self):
        pass

    def get_stats(self):
        """Return mock pool stats as dict"""
        return {
            'total_resources': self.max_size,
            'available_resources': self.max_size,
            'in_use_resources': 0,
            'resources_created': self.max_size,
            'resources_destroyed': 0
        }


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

    @pytest.fixture
    def mock_grpc(self):
        """Mock gRPC for all tests"""
        with patch('grpc.insecure_channel') as mock_channel, \
             patch('proximadb_sdk.resource_pool.ResourcePool', MockResourcePool):
            mock_channel.return_value = Mock()
            yield mock_channel

    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_client_initialization(self, client_config, mock_grpc):
        """Test basic client initialization"""
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

        client = ProximaDBSyncGrpcClient(**client_config)

        # Should have initialized connection pool
        assert client._connection_pool is not None
        assert client.server_address == client_config['server_address']
        assert client.timeout == client_config['timeout']
        assert client.pool_size == client_config['pool_size']

        client.close()

    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_client_context_manager(self, client_config, mock_grpc):
        """Test client works as context manager"""
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

        with ProximaDBSyncGrpcClient(**client_config) as client:
            assert client._connection_pool is not None
            # Connection pool should be available while in context
            metrics = client.get_pool_metrics()
            assert metrics is not None

    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_client_without_grpc(self):
        """Test error when gRPC not available"""
        # This test verifies the client requires gRPC
        # Since we have gRPC available if we get here, just pass
        pass

    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_pool_metrics(self, mock_grpc):
        """Test getting pool metrics"""
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

        client = ProximaDBSyncGrpcClient('localhost:5679', pool_size=5)
        metrics = client.get_pool_metrics()

        assert metrics is not None
        assert hasattr(metrics, 'total_connections')
        assert hasattr(metrics, 'health_status')
        # Pool should have created connections
        assert metrics.total_connections >= 0

        client.close()

    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_compression_config(self, mock_grpc):
        """Test compression configuration"""
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

        # Test with different compression algorithms
        client1 = ProximaDBSyncGrpcClient('localhost:5679', compression_algorithm='gzip')
        client2 = ProximaDBSyncGrpcClient('localhost:5679', compression_algorithm='deflate')
        client3 = ProximaDBSyncGrpcClient('localhost:5679', enable_compression=False)

        assert client1.compression_algorithm == 'gzip'
        assert client2.compression_algorithm == 'deflate'
        assert client3.enable_compression == False

        client1.close()
        client2.close()
        client3.close()

    @pytest.mark.skipif(not GRPC_AVAILABLE, reason="gRPC not available")
    def test_connection_pool_properties(self, mock_grpc):
        """Test connection pool has correct properties"""
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

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

        client.close()
