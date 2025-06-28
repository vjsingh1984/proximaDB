"""
Simple REST client tests for ProximaDB Python SDK.
Focus on improving rest_client.py coverage with basic functionality tests.
"""

import pytest
import numpy as np
from unittest.mock import Mock, patch
import httpx
from proximadb.rest_client import ProximaDBRestClient
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import NetworkError, TimeoutError, ProximaDBError


class TestRestClientBasics:
    """Test basic REST client functionality"""
    
    def test_client_initialization(self):
        """Test REST client initialization"""
        client = ProximaDBRestClient()
        assert hasattr(client, 'base_url')
        assert hasattr(client, 'timeout')
        
        # Test with custom parameters
        client = ProximaDBRestClient(
            base_url="http://custom:8080",
            timeout=60.0
        )
        assert client.base_url == "http://custom:8080"
        assert client.timeout == 60.0
    
    def test_vector_normalization(self):
        """Test vector normalization methods"""
        client = ProximaDBRestClient()
        
        # Test _normalize_vector with list
        vector_list = [0.1, 0.2, 0.3]
        result = client._normalize_vector(vector_list)
        assert isinstance(result, list)
        assert result == vector_list
        
        # Test _normalize_vector with numpy array
        vector_np = np.array([0.1, 0.2, 0.3], dtype=np.float32)
        result = client._normalize_vector(vector_np)
        assert isinstance(result, list)
        assert len(result) == 3
        
        # Test _normalize_vectors with multiple vectors
        vectors = [[0.1, 0.2], [0.3, 0.4]]
        result = client._normalize_vectors(vectors)
        assert isinstance(result, list)
        assert len(result) == 2
        assert all(isinstance(v, list) for v in result)


class TestRestClientMockedRequests:
    """Test REST client with mocked HTTP requests"""
    
    @pytest.fixture
    def client(self):
        return ProximaDBRestClient(base_url="http://test:5678")
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_health_endpoint(self, mock_request, client):
        """Test health endpoint call"""
        mock_response = Mock()
        mock_response.json.return_value = {
            "status": "healthy",
            "version": "0.1.0"
        }
        mock_request.return_value = mock_response
        
        result = client.health()
        
        mock_request.assert_called_once_with('GET', '/health')
        assert result.status == "healthy"
        assert result.version == "0.1.0"
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_list_collections(self, mock_request, client):
        """Test list collections endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = ["collection1", "collection2"]
        mock_request.return_value = mock_response
        
        result = client.list_collections()
        
        mock_request.assert_called_once_with('GET', '/collections')
        assert isinstance(result, list)
        assert len(result) == 2
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_create_collection(self, mock_request, client):
        """Test create collection endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = {
            "id": "test_collection_123",
            "name": "test_collection",
            "dimension": 768,
            "status": "active"
        }
        mock_request.return_value = mock_response
        
        config = CollectionConfig(dimension=768, distance_metric=DistanceMetric.COSINE)
        result = client.create_collection("test_collection", config)
        
        mock_request.assert_called_once()
        assert result.id == "test_collection_123"
        assert result.name == "test_collection"
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_get_collection(self, mock_request, client):
        """Test get collection endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = {
            "id": "test_collection",
            "name": "test_collection",
            "dimension": 768,
            "vector_count": 100,
            "status": "active"
        }
        mock_request.return_value = mock_response
        
        result = client.get_collection("test_collection")
        
        mock_request.assert_called_once_with('GET', '/collections/test_collection')
        assert result.id == "test_collection"
        assert result.vector_count == 100
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_delete_collection(self, mock_request, client):
        """Test delete collection endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = True
        mock_request.return_value = mock_response
        
        result = client.delete_collection("test_collection")
        
        mock_request.assert_called_once_with('DELETE', '/collections/test_collection')
        assert result is True
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_insert_vector(self, mock_request, client):
        """Test insert vector endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = {
            "count": 1,
            "duration_ms": 25.5,
            "failed_count": 0
        }
        mock_request.return_value = mock_response
        
        vector = [0.1, 0.2, 0.3] + [0.0] * 765  # 768 dimensions
        metadata = {"title": "test"}
        
        result = client.insert_vector(
            collection_id="test_collection",
            vector_id="vec_1",
            vector=vector,
            metadata=metadata
        )
        
        mock_request.assert_called_once()
        assert result.count == 1
        assert result.duration_ms == 25.5
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_search_vectors(self, mock_request, client):
        """Test search vectors endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = {
            "results": [
                {
                    "id": "vec_1",
                    "score": 0.95,
                    "metadata": {"title": "relevant"}
                }
            ],
            "total_time_ms": 15
        }
        mock_request.return_value = mock_response
        
        query_vector = [0.1, 0.2, 0.3] + [0.0] * 765
        
        result = client.search(
            collection_id="test_collection",
            query=query_vector,
            k=10
        )
        
        mock_request.assert_called_once()
        assert len(result.results) == 1
        assert result.results[0].score == 0.95
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_delete_vector(self, mock_request, client):
        """Test delete vector endpoint"""
        mock_response = Mock()
        mock_response.json.return_value = {
            "deleted_count": 1,
            "duration_ms": 10.0
        }
        mock_request.return_value = mock_response
        
        result = client.delete_vector("test_collection", "vec_1")
        
        mock_request.assert_called_once_with('DELETE', '/collections/test_collection/vectors/vec_1')
        assert result.deleted_count == 1


class TestRestClientErrorHandling:
    """Test REST client error handling"""
    
    @pytest.fixture
    def client(self):
        return ProximaDBRestClient(base_url="http://test:5678")
    
    @patch('httpx.Client')
    def test_connection_error(self, mock_httpx, client):
        """Test connection error handling"""
        mock_httpx.return_value.__enter__.return_value.request.side_effect = httpx.ConnectError("Connection failed")
        
        with pytest.raises(NetworkError):
            client.health()
    
    @patch('httpx.Client')
    def test_timeout_error(self, mock_httpx, client):
        """Test timeout error handling"""
        mock_httpx.return_value.__enter__.return_value.request.side_effect = httpx.TimeoutException("Timeout")
        
        with pytest.raises(TimeoutError):
            client.health()
    
    @patch('proximadb.rest_client.ProximaDBRestClient._make_request')
    def test_error_response_handling(self, mock_request, client):
        """Test error response handling"""
        mock_response = Mock()
        mock_response.status_code = 404
        mock_response.text = "Collection not found"
        mock_response.raise_for_status.side_effect = httpx.HTTPStatusError(
            "Not found", request=Mock(), response=mock_response
        )
        mock_request.side_effect = httpx.HTTPStatusError(
            "Not found", request=Mock(), response=mock_response
        )
        
        with pytest.raises(httpx.HTTPStatusError):
            client.get_collection("nonexistent")


class TestRestClientInputValidation:
    """Test REST client input validation and edge cases"""
    
    @pytest.fixture
    def client(self):
        return ProximaDBRestClient()
    
    def test_empty_vector_handling(self, client):
        """Test handling of empty vectors"""
        # Empty list
        result = client._normalize_vector([])
        assert result == []
        
        # Empty numpy array
        empty_np = np.array([], dtype=np.float32)
        result = client._normalize_vector(empty_np)
        assert result == []
    
    def test_vector_type_conversion(self, client):
        """Test vector type conversion"""
        # Integer list to float
        int_vector = [1, 2, 3]
        result = client._normalize_vector(int_vector)
        assert all(isinstance(x, (int, float)) for x in result)
        
        # Different numpy dtypes
        float64_vector = np.array([1.0, 2.0, 3.0], dtype=np.float64)
        result = client._normalize_vector(float64_vector)
        assert isinstance(result, list)
        assert len(result) == 3
    
    def test_multiple_vectors_normalization(self, client):
        """Test normalization of multiple vectors"""
        vectors = [
            [1.0, 2.0],
            np.array([3.0, 4.0]),
            [5, 6]  # integers
        ]
        result = client._normalize_vectors(vectors)
        
        assert len(result) == 3
        assert all(isinstance(v, list) for v in result)
        assert all(len(v) == 2 for v in result)


class TestRestClientConfiguration:
    """Test REST client configuration options"""
    
    def test_default_configuration(self):
        """Test default configuration values"""
        client = ProximaDBRestClient()
        
        # Should have sensible defaults
        assert hasattr(client, 'base_url')
        assert hasattr(client, 'timeout')
        assert client.base_url.startswith('http')
        assert isinstance(client.timeout, (int, float))
        assert client.timeout > 0
    
    def test_custom_configuration(self):
        """Test custom configuration"""
        client = ProximaDBRestClient(
            base_url="https://custom-host:9090",
            timeout=120.0
        )
        
        assert client.base_url == "https://custom-host:9090"
        assert client.timeout == 120.0
    
    def test_http_client_creation(self):
        """Test HTTP client creation and configuration"""
        client = ProximaDBRestClient(timeout=30.0)
        
        # Should create proper HTTP client
        http_client = client._create_http_client()
        assert http_client is not None
        assert hasattr(http_client, 'timeout')