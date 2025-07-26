"""
Test suite for ProximaDB synchronous gRPC client
"""
import pytest
from unittest.mock import Mock, patch, MagicMock
from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient, sync_method
from proximadb.models import SearchResult, VectorOperationResponse, VectorRecord
from proximadb.exceptions import ProximaDBError, NetworkError


class TestSyncMethod:
    """Test the sync_method decorator"""
    
    def test_sync_method_decorator(self):
        """Test sync_method decorator functionality"""
        # Create a mock async client
        mock_async_client = Mock()
        mock_async_client.test_method = Mock(return_value="test_result")
        
        # Create a class using the decorator
        class TestClient:
            def __init__(self):
                self._async_client = mock_async_client
            
            @sync_method
            def test_method(self, arg1, arg2, kwarg1=None):
                pass
        
        # Test the decorated method
        client = TestClient()
        result = client.test_method("arg1", "arg2", kwarg1="value1")
        
        assert result == "test_result"
        mock_async_client.test_method.assert_called_once_with("arg1", "arg2", kwarg1="value1")


class TestProximaDBSyncGrpcClient:
    """Test ProximaDBSyncGrpcClient class"""
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_init(self, mock_async_client_class):
        """Test client initialization"""
        mock_instance = Mock()
        mock_async_client_class.return_value = mock_instance
        
        client = ProximaDBSyncGrpcClient("localhost:5679", timeout=30.0)
        
        assert client.server_address == "localhost:5679"
        assert client.timeout == 30.0
        assert client._async_client == mock_instance
        mock_async_client_class.assert_called_once_with(endpoint="localhost:5679", timeout=30.0)
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_init_failure(self, mock_async_client_class):
        """Test client initialization failure"""
        mock_async_client_class.side_effect = Exception("Connection failed")
        
        with pytest.raises(NetworkError) as exc_info:
            ProximaDBSyncGrpcClient("localhost:5679")
        
        assert "Failed to initialize gRPC client" in str(exc_info.value)
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_close(self, mock_async_client_class):
        """Test client close method"""
        mock_instance = Mock()
        mock_instance.close = Mock()
        mock_async_client_class.return_value = mock_instance
        
        client = ProximaDBSyncGrpcClient("localhost:5679")
        client.close()
        
        mock_instance.close.assert_called_once()
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_context_manager(self, mock_async_client_class):
        """Test client as context manager"""
        mock_instance = Mock()
        mock_instance.close = Mock()
        mock_async_client_class.return_value = mock_instance
        
        with ProximaDBSyncGrpcClient("localhost:5679") as client:
            assert client._async_client == mock_instance
        
        mock_instance.close.assert_called_once()
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_search_vectors(self, mock_async_client_class):
        """Test search_vectors method"""
        # Setup mock
        mock_instance = Mock()
        mock_result = SearchResult(
            results=[],
            query_id="query_123",
            took_ms=10.5
        )
        mock_instance.search_vectors = Mock(return_value=mock_result)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.search_vectors(
            collection_id="test_collection",
            vector=[0.1, 0.2, 0.3],
            top_k=5
        )
        
        assert result == mock_result
        mock_instance.search_vectors.assert_called_once_with(
            collection_id="test_collection",
            vector=[0.1, 0.2, 0.3],
            top_k=5
        )
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_insert_vectors(self, mock_async_client_class):
        """Test insert_vectors method"""
        # Setup mock
        mock_instance = Mock()
        mock_response = VectorOperationResponse(
            success=True,
            message="Inserted 2 vectors",
            vector_count=2
        )
        mock_instance.insert_vectors = Mock(return_value=mock_response)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        vectors = [
            {"id": "vec1", "vector": [0.1, 0.2]},
            {"id": "vec2", "vector": [0.3, 0.4]}
        ]
        result = client.insert_vectors("test_collection", vectors)
        
        assert result == mock_response
        mock_instance.insert_vectors.assert_called_once_with("test_collection", vectors)
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_get_vector(self, mock_async_client_class):
        """Test get_vector method"""
        # Setup mock
        mock_instance = Mock()
        mock_vector = VectorRecord(
            id="vec1",
            vector=[0.1, 0.2, 0.3],
            metadata={"key": "value"}
        )
        mock_instance.get_vector = Mock(return_value=mock_vector)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.get_vector("test_collection", "vec1")
        
        assert result == mock_vector
        mock_instance.get_vector.assert_called_once_with(
            "test_collection", 
            "vec1", 
            include_vector=True, 
            include_metadata=True
        )
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_delete_vector(self, mock_async_client_class):
        """Test delete_vector method"""
        # Setup mock
        mock_instance = Mock()
        mock_response = VectorOperationResponse(
            success=True,
            message="Deleted vector",
            vector_count=1
        )
        mock_instance.delete_vector = Mock(return_value=mock_response)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.delete_vector("test_collection", "vec1")
        
        assert result == mock_response
        mock_instance.delete_vector.assert_called_once_with("test_collection", "vec1")
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_create_collection(self, mock_async_client_class):
        """Test create_collection method"""
        # Setup mock
        mock_instance = Mock()
        mock_instance.create_collection = Mock(return_value=True)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.create_collection(
            collection_id="test_collection",
            dimension=128,
            metric="cosine"
        )
        
        assert result is True
        mock_instance.create_collection.assert_called_once_with(
            collection_id="test_collection",
            dimension=128,
            metric="cosine"
        )
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_list_collections(self, mock_async_client_class):
        """Test list_collections method"""
        # Setup mock
        mock_instance = Mock()
        mock_collections = ["collection1", "collection2", "collection3"]
        mock_instance.list_collections = Mock(return_value=mock_collections)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.list_collections()
        
        assert result == mock_collections
        mock_instance.list_collections.assert_called_once()
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_delete_collection(self, mock_async_client_class):
        """Test delete_collection method"""
        # Setup mock
        mock_instance = Mock()
        mock_instance.delete_collection = Mock(return_value=True)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.delete_collection("test_collection")
        
        assert result is True
        mock_instance.delete_collection.assert_called_once_with("test_collection")
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_batch_operations(self, mock_async_client_class):
        """Test batch insert/update/delete operations"""
        # Setup mock
        mock_instance = Mock()
        mock_response = VectorOperationResponse(
            success=True,
            message="Batch operation completed",
            vector_count=10
        )
        mock_instance.batch_insert = Mock(return_value=mock_response)
        mock_instance.batch_update = Mock(return_value=mock_response)
        mock_instance.batch_delete = Mock(return_value=mock_response)
        mock_async_client_class.return_value = mock_instance
        
        # Test batch operations
        client = ProximaDBSyncGrpcClient("localhost:5679")
        
        # Test batch insert
        vectors = [{"id": f"vec{i}", "vector": [0.1] * 128} for i in range(10)]
        result = client.batch_insert("test_collection", vectors)
        assert result == mock_response
        
        # Test batch update  
        result = client.batch_update("test_collection", vectors)
        assert result == mock_response
        
        # Test batch delete
        vector_ids = [f"vec{i}" for i in range(10)]
        result = client.batch_delete("test_collection", vector_ids)
        assert result == mock_response
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_get_collection_info(self, mock_async_client_class):
        """Test get_collection_info method"""
        # Setup mock
        mock_instance = Mock()
        mock_info = {
            "collection_id": "test_collection",
            "dimension": 128,
            "metric": "cosine",
            "vector_count": 1000
        }
        mock_instance.get_collection_info = Mock(return_value=mock_info)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.get_collection_info("test_collection")
        
        assert result == mock_info
        mock_instance.get_collection_info.assert_called_once_with("test_collection")
    
    @patch('proximadb.protocols.grpc_sync.AsyncGrpcClient')
    def test_health_check(self, mock_async_client_class):
        """Test health_check method"""
        # Setup mock
        mock_instance = Mock()
        mock_health = {"status": "healthy", "version": "1.0.0"}
        mock_instance.health_check = Mock(return_value=mock_health)
        mock_async_client_class.return_value = mock_instance
        
        # Test
        client = ProximaDBSyncGrpcClient("localhost:5679")
        result = client.health_check()
        
        assert result == mock_health
        mock_instance.health_check.assert_called_once()