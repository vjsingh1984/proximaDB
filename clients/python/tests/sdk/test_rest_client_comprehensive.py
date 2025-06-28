"""
Comprehensive REST client tests for ProximaDB Python SDK.
Focus on improving rest_client.py coverage.
"""

import pytest
import asyncio
import time
from unittest.mock import Mock, patch, AsyncMock
import httpx
from proximadb.rest_client import ProximaDBProximaDBRestClient
from proximadb.models import (
    CollectionConfig, Collection, DistanceMetric, SearchRequest, 
    SearchResponse, InsertResult, DeleteResult, HealthStatus
)
from proximadb.exceptions import (
    ProximaDBError, NotFoundError, ValidationError, ConnectionError, 
    TimeoutError, ServerError
)


class TestProximaDBRestClientConfiguration:
    """Test REST client configuration and initialization"""
    
    def test_rest_client_init_default(self):
        """Test REST client initialization with defaults"""
        client = ProximaDBRestClient()
        assert hasattr(client, 'base_url')
        assert hasattr(client, 'timeout')
    
    def test_rest_client_init_custom_params(self):
        """Test REST client initialization with custom parameters"""
        client = ProximaDBRestClient(
            base_url="http://custom-host:8080",
            timeout=60.0
        )
        assert client.base_url == "http://custom-host:8080"
        assert client.timeout == 60.0
    
    def test_rest_client_vector_normalization(self):
        """Test vector normalization functionality"""
        client = ProximaDBRestClient()
        
        # Test list input
        vector_list = [0.1, 0.2, 0.3]
        normalized = client._normalize_vector(vector_list)
        assert isinstance(normalized, list)
        assert len(normalized) == 3
        
        # Test numpy array input
        import numpy as np
        vector_np = np.array([0.1, 0.2, 0.3], dtype=np.float32)
        normalized = client._normalize_vector(vector_np)
        assert isinstance(normalized, list)
        assert len(normalized) == 3


class TestProximaDBRestClientErrorHandling:
    """Test REST client error handling and retry logic"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client for testing"""
        return ProximaDBRestClient(base_url="http://test:5678")
    
    @patch('httpx.Client')
    def test_connection_error_handling(self, mock_httpx, mock_client):
        """Test connection error handling"""
        mock_httpx.return_value.__enter__.return_value.get.side_effect = httpx.ConnectError("Connection failed")
        
        with pytest.raises(NetworkError):
            mock_client.health()
    
    @patch('httpx.Client')
    def test_timeout_error_handling(self, mock_httpx, mock_client):
        """Test timeout error handling"""
        mock_httpx.return_value.__enter__.return_value.get.side_effect = httpx.TimeoutException("Request timeout")
        
        with pytest.raises(TimeoutError):
            mock_client.health()
    
    @patch('httpx.Client')
    def test_server_error_handling(self, mock_httpx, mock_client):
        """Test server error response handling"""
        mock_response = Mock()
        mock_response.status_code = 500
        mock_response.text = "Internal server error"
        mock_response.raise_for_status.side_effect = httpx.HTTPStatusError(
            "Server error", request=Mock(), response=mock_response
        )
        mock_httpx.return_value.__enter__.return_value.get.return_value = mock_response
        
        with pytest.raises(ProximaDBError):
            mock_client.health()
    
    @patch('httpx.Client') 
    def test_not_found_error_handling(self, mock_httpx, mock_client):
        """Test 404 error handling"""
        mock_response = Mock()
        mock_response.status_code = 404
        mock_response.text = "Collection not found"
        mock_response.raise_for_status.side_effect = httpx.HTTPStatusError(
            "Not found", request=Mock(), response=mock_response
        )
        mock_httpx.return_value.__enter__.return_value.get.return_value = mock_response
        
        with pytest.raises(ProximaDBError):
            mock_client.get_collection("nonexistent")


class TestProximaDBRestClientCollectionOperations:
    """Test REST client collection operations"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client for testing"""
        return ProximaDBRestClient(base_url="http://test:5678")
    
    @patch('httpx.Client')
    def test_create_collection_success(self, mock_httpx, mock_client):
        """Test successful collection creation"""
        mock_response = Mock()
        mock_response.status_code = 201
        mock_response.json.return_value = {
            "id": "test_collection_123",
            "name": "test_collection",
            "status": "created"
        }
        mock_httpx.return_value.__enter__.return_value.post.return_value = mock_response
        
        config = CollectionConfig(dimension=768, distance_metric=DistanceMetric.COSINE)
        result = mock_client.create_collection("test_collection", config)
        
        assert result.id == "test_collection_123"
        assert result.name == "test_collection"
    
    @patch('httpx.Client')
    def test_list_collections_success(self, mock_httpx, mock_client):
        """Test successful collection listing"""
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = [
            "collection1", "collection2"
        ]
        mock_httpx.return_value.__enter__.return_value.get.return_value = mock_response
        
        collections = mock_client.list_collections()
        
        assert len(collections) == 2
        assert collections[0] == "collection1"
        assert collections[1] == "collection2"
    
    @patch('httpx.Client')
    def test_get_collection_success(self, mock_httpx, mock_client):
        """Test successful collection retrieval"""
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {
            "id": "test_collection_123",
            "name": "test_collection",
            "dimension": 768,
            "vector_count": 100
        }
        mock_httpx.return_value.__enter__.return_value.get.return_value = mock_response
        
        collection = mock_client.get_collection("test_collection")
        
        assert collection.id == "test_collection_123"
        assert collection.name == "test_collection"
    
    @patch('httpx.Client')
    def test_delete_collection_success(self, mock_httpx, mock_client):
        """Test successful collection deletion"""
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = True
        mock_httpx.return_value.__enter__.return_value.delete.return_value = mock_response
        
        result = mock_client.delete_collection("test_collection")
        
        assert result is True


class TestProximaDBRestClientVectorOperations:
    """Test REST client vector operations"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client for testing"""
        return ProximaDBRestClient(base_url="http://test:5678")
    
    @patch('httpx.AsyncClient')
    async def test_insert_vector_success(self, mock_httpx, mock_client):
        """Test successful vector insertion"""
        mock_response = Mock()
        mock_response.status_code = 201
        mock_response.json.return_value = {
            "count": 1,
            "duration_ms": 45.2,
            "request_id": "req_123"
        }
        mock_httpx.return_value.__aenter__.return_value.post = AsyncMock(return_value=mock_response)
        
        vector_data = [0.1, 0.2, 0.3] + [0.0] * 765  # 768 dimensions
        metadata = {"title": "test document"}
        
        result = await mock_client.insert_vector(
            collection_id="test_collection",
            vector_id="vec_1",
            vector=vector_data,
            metadata=metadata
        )
        
        assert result["count"] == 1
        assert result["duration_ms"] == 45.2
    
    @patch('httpx.AsyncClient')
    async def test_search_vectors_success(self, mock_httpx, mock_client):
        """Test successful vector search"""
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {
            "results": [
                {
                    "id": "vec_1",
                    "score": 0.95,
                    "metadata": {"title": "relevant document"}
                },
                {
                    "id": "vec_2", 
                    "score": 0.87,
                    "metadata": {"title": "another document"}
                }
            ],
            "total_time_ms": 12
        }
        mock_httpx.return_value.__aenter__.return_value.post = AsyncMock(return_value=mock_response)
        
        query_vector = [0.1, 0.2, 0.3] + [0.0] * 765
        
        result = await mock_client.search_vectors(
            collection_id="test_collection",
            query=query_vector,
            k=10
        )
        
        assert len(result["results"]) == 2
        assert result["results"][0]["score"] == 0.95
        assert result["total_time_ms"] == 12
    
    @patch('httpx.AsyncClient')
    async def test_delete_vector_success(self, mock_httpx, mock_client):
        """Test successful vector deletion"""
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {
            "deleted": True,
            "count": 1
        }
        mock_httpx.return_value.__aenter__.return_value.delete = AsyncMock(return_value=mock_response)
        
        result = await mock_client.delete_vector(
            collection_id="test_collection",
            vector_id="vec_1"
        )
        
        assert result["deleted"] is True
        assert result["count"] == 1


class TestProximaDBRestClientHealthAndStatus:
    """Test REST client health and status operations"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client for testing"""
        return ProximaDBRestClient(base_url="http://test:5678")
    
    @patch('httpx.AsyncClient')
    async def test_get_health_success(self, mock_httpx, mock_client):
        """Test successful health check"""
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {
            "status": "healthy",
            "version": "0.1.0",
            "uptime_seconds": 3600
        }
        mock_httpx.return_value.__aenter__.return_value.get = AsyncMock(return_value=mock_response)
        
        health = await mock_client.get_health()
        
        assert health["status"] == "healthy"
        assert health["version"] == "0.1.0"
        assert health["uptime_seconds"] == 3600


class TestProximaDBRestClientValidation:
    """Test REST client input validation"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client for testing"""
        return ProximaDBRestClient(base_url="http://test:5678")
    
    async def test_invalid_collection_name_validation(self, mock_client):
        """Test validation of invalid collection names"""
        config = CollectionConfig(dimension=768)
        
        with pytest.raises(ValidationError):
            await mock_client.create_collection("", config)
        
        with pytest.raises(ValidationError):
            await mock_client.create_collection(None, config)
    
    async def test_invalid_vector_dimension_validation(self, mock_client):
        """Test validation of vector dimensions"""
        with pytest.raises(ValidationError):
            await mock_client.insert_vector(
                collection_id="test",
                vector_id="vec_1",
                vector=[],  # Empty vector
                metadata={}
            )
    
    async def test_invalid_search_parameters_validation(self, mock_client):
        """Test validation of search parameters"""
        query_vector = [0.1, 0.2, 0.3]
        
        with pytest.raises(ValidationError):
            await mock_client.search_vectors(
                collection_id="test",
                query=query_vector,
                k=0  # Invalid k value
            )
        
        with pytest.raises(ValidationError):
            await mock_client.search_vectors(
                collection_id="test",
                query=[],  # Empty query vector
                k=10
            )


class TestProximaDBRestClientRetryLogic:
    """Test REST client retry logic and resilience"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client with custom retry settings"""
        return ProximaDBRestClient(base_url="http://test:5678", max_retries=2)
    
    @patch('httpx.AsyncClient')
    async def test_retry_on_server_error(self, mock_httpx, mock_client):
        """Test retry logic on server errors"""
        # First call fails, second succeeds
        mock_response_fail = Mock()
        mock_response_fail.status_code = 500
        mock_response_fail.raise_for_status.side_effect = httpx.HTTPStatusError(
            "Server error", request=Mock(), response=mock_response_fail
        )
        
        mock_response_success = Mock()
        mock_response_success.status_code = 200
        mock_response_success.json.return_value = {"status": "healthy"}
        
        mock_httpx.return_value.__aenter__.return_value.get = AsyncMock(
            side_effect=[mock_response_fail, mock_response_success]
        )
        
        # Should succeed after retry
        result = await mock_client.get_health()
        assert result["status"] == "healthy"
    
    @patch('httpx.AsyncClient')
    async def test_max_retries_exceeded(self, mock_httpx, mock_client):
        """Test behavior when max retries are exceeded"""
        mock_response = Mock()
        mock_response.status_code = 500
        mock_response.text = "Server error"
        mock_response.raise_for_status.side_effect = httpx.HTTPStatusError(
            "Server error", request=Mock(), response=mock_response
        )
        mock_httpx.return_value.__aenter__.return_value.get = AsyncMock(return_value=mock_response)
        
        with pytest.raises(ServerError):
            await mock_client.get_health()


class TestProximaDBRestClientRequestFormatting:
    """Test REST client request formatting and serialization"""
    
    @pytest.fixture
    def mock_client(self):
        """Create a mock REST client for testing"""
        return ProximaDBRestClient(base_url="http://test:5678")
    
    def test_url_construction(self, mock_client):
        """Test URL construction for different endpoints"""
        assert mock_client._build_url("/health") == "http://test:5678/health"
        assert mock_client._build_url("collections") == "http://test:5678/collections"
        assert mock_client._build_url("collections/test") == "http://test:5678/collections/test"
    
    def test_collection_config_serialization(self, mock_client):
        """Test collection config serialization"""
        config = CollectionConfig(
            dimension=768,
            distance_metric=DistanceMetric.COSINE,
            description="Test collection"
        )
        
        serialized = mock_client._serialize_collection_config(config)
        
        assert serialized["dimension"] == 768
        assert serialized["distance_metric"] == "cosine"
        assert serialized["description"] == "Test collection"
    
    def test_vector_data_formatting(self, mock_client):
        """Test vector data formatting for requests"""
        vector = [0.1, 0.2, 0.3, 0.4]
        metadata = {"title": "test", "category": "document"}
        
        formatted = mock_client._format_vector_data("vec_1", vector, metadata)
        
        assert formatted["vector_id"] == "vec_1"
        assert formatted["vector"] == vector
        assert formatted["metadata"] == metadata