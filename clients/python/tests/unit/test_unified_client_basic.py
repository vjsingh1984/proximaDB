"""
Basic test suite for ProximaDB unified client
"""
import pytest
from unittest.mock import Mock, patch, MagicMock
from proximadb.unified_client import ProximaDBClient
from proximadb.models import SearchResult, VectorOperationResponse, VectorRecord
from proximadb.exceptions import ProximaDBError, CollectionNotFoundError
from proximadb.config import Protocol


class TestProximaDBClientInit:
    """Test ProximaDBClient initialization"""
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_init_default_rest(self, mock_rest_client, mock_load_config):
        """Test client initialization with default REST protocol"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_config.timeout = 30.0
        mock_load_config.return_value = mock_config
        
        client = ProximaDBClient(url="http://localhost:5678")
        
        assert client.config == mock_config
        assert client._rest_client is not None
        assert client._grpc_client is None
        mock_rest_client.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.GrpcSyncClient')
    def test_init_grpc(self, mock_grpc_client, mock_load_config):
        """Test client initialization with gRPC protocol"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = True
        mock_config.get_protocol_url.return_value = "localhost:5679"
        mock_config.timeout = 30.0
        mock_load_config.return_value = mock_config
        
        client = ProximaDBClient(url="grpc://localhost:5679")
        
        assert client.config == mock_config
        assert client._grpc_client is not None
        assert client._rest_client is None
        mock_grpc_client.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')
    def test_init_failure(self, mock_load_config):
        """Test client initialization failure"""
        mock_load_config.side_effect = Exception("Config error")
        
        with pytest.raises(Exception) as exc_info:
            ProximaDBClient(url="http://localhost:5678")
        
        assert "Config error" in str(exc_info.value)


class TestProximaDBClientProperties:
    """Test ProximaDBClient properties"""
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_get_client_rest(self, mock_rest_client, mock_load_config):
        """Test _get_client property for REST"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        active_client = client._get_client()
        
        assert active_client == mock_rest_instance
    
    @patch('proximadb.unified_client.load_config')
    def test_numpy_installed_property(self, mock_load_config):
        """Test numpy_installed property"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_load_config.return_value = mock_config
        
        with patch('proximadb.unified_client.RestSyncClient'):
            client = ProximaDBClient(url="http://localhost:5678")
            # numpy should be installed in test environment
            assert client.numpy_installed is True
    
    @patch('proximadb.unified_client.load_config')
    def test_str_representation(self, mock_load_config):
        """Test string representation"""
        mock_config = Mock()
        mock_config.url = "http://localhost:5678"
        mock_config.protocol = Protocol.REST
        mock_config.should_use_grpc.return_value = False
        mock_load_config.return_value = mock_config
        
        with patch('proximadb.unified_client.RestSyncClient'):
            client = ProximaDBClient(url="http://localhost:5678")
            str_repr = str(client)
            assert "ProximaDBClient" in str_repr
            assert "http://localhost:5678" in str_repr
            assert "rest" in str_repr.lower()


class TestProximaDBClientCollectionMethods:
    """Test ProximaDBClient collection methods"""
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_create_collection(self, mock_rest_client, mock_load_config):
        """Test create_collection method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_instance.create_collection.return_value = {"status": "created"}
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.create_collection(
            name="test_collection",
            dimension=128,
            metric="cosine"
        )
        
        assert result == {"status": "created"}
        mock_rest_instance.create_collection.assert_called_once_with(
            collection_id="test_collection",
            dimension=128,
            metric="cosine"
        )
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_list_collections(self, mock_rest_client, mock_load_config):
        """Test list_collections method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_instance.list_collections.return_value = ["col1", "col2", "col3"]
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.list_collections()
        
        assert result == ["col1", "col2", "col3"]
        mock_rest_instance.list_collections.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_delete_collection(self, mock_rest_client, mock_load_config):
        """Test delete_collection method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_instance.delete_collection.return_value = {"status": "deleted"}
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.delete_collection("test_collection")
        
        assert result == {"status": "deleted"}
        mock_rest_instance.delete_collection.assert_called_once_with("test_collection")
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_get_collection(self, mock_rest_client, mock_load_config):
        """Test get_collection method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_instance.get_collection.return_value = {
            "collection_id": "test_collection",
            "dimension": 128,
            "metric": "cosine"
        }
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.get_collection("test_collection")
        
        assert result["collection_id"] == "test_collection"
        mock_rest_instance.get_collection.assert_called_once_with("test_collection")


class TestProximaDBClientVectorMethods:
    """Test ProximaDBClient vector methods"""
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_upsert_simple(self, mock_rest_client, mock_load_config):
        """Test upsert method with simple inputs"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_response = VectorOperationResponse(
            operation="upsert",
            success=True,
            message="Vectors upserted",
            vector_count=1,
            metrics={}
        )
        mock_rest_instance.insert_vectors.return_value = mock_response
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.upsert(
            collection_name="test_collection",
            vectors=[[0.1, 0.2, 0.3]],
            ids=["vec1"]
        )
        
        assert result == mock_response
        mock_rest_instance.insert_vectors.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_get_vector(self, mock_rest_client, mock_load_config):
        """Test get method for single vector"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_vector = VectorRecord(
            id="vec1",
            vector=[0.1, 0.2, 0.3],
            metadata={"key": "value"}
        )
        mock_rest_instance.get_vector.return_value = mock_vector
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.get(
            collection_name="test_collection",
            ids="vec1"
        )
        
        assert result == mock_vector
        mock_rest_instance.get_vector.assert_called_once_with(
            "test_collection",
            "vec1",
            include_vector=True,
            include_metadata=True
        )
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_delete_vector(self, mock_rest_client, mock_load_config):
        """Test delete method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_response = VectorOperationResponse(
            operation="delete",
            success=True,
            message="Vector deleted",
            vector_count=1,
            metrics={}
        )
        mock_rest_instance.delete_vector.return_value = mock_response
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        result = client.delete(
            collection_name="test_collection",
            ids=["vec1"]
        )
        
        assert result == mock_response


class TestProximaDBClientUtilityMethods:
    """Test ProximaDBClient utility methods"""
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_close(self, mock_rest_client, mock_load_config):
        """Test close method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_client.return_value = mock_rest_instance
        
        client = ProximaDBClient(url="http://localhost:5678")
        client.close()
        
        mock_rest_instance.close.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')
    @patch('proximadb.unified_client.RestSyncClient')
    def test_context_manager(self, mock_rest_client, mock_load_config):
        """Test client as context manager"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_instance = Mock()
        mock_rest_client.return_value = mock_rest_instance
        
        with ProximaDBClient(url="http://localhost:5678") as client:
            assert client is not None
        
        mock_rest_instance.close.assert_called_once()
    
    @patch('proximadb.unified_client.load_config')  
    @patch('proximadb.unified_client.RestSyncClient')
    def test_validate_and_convert_metadata(self, mock_rest_client, mock_load_config):
        """Test _validate_and_convert_metadata method"""
        mock_config = Mock()
        mock_config.should_use_grpc.return_value = False
        mock_config.get_protocol_url.return_value = "http://localhost:5678"
        mock_load_config.return_value = mock_config
        
        mock_rest_client.return_value = Mock()
        
        client = ProximaDBClient(url="http://localhost:5678")
        
        # Test with valid metadata
        metadata = {"key1": "value1", "key2": 123, "key3": 45.6}
        result = client._validate_and_convert_metadata(metadata)
        assert result == {"key1": "value1", "key2": "123", "key3": "45.6"}
        
        # Test with None
        result = client._validate_and_convert_metadata(None)
        assert result == {}