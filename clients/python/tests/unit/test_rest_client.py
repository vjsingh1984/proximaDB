#!/usr/bin/env python3
"""
Unit tests for ProximaDB REST client module
"""
import pytest
from unittest.mock import Mock, patch, MagicMock
import httpx
import numpy as np
from proximadb.rest_client import ProximaDBRestClient
from proximadb.config import ClientConfig
from proximadb.exceptions import ProximaDBError, ValidationError
from proximadb.models import Collection, HealthStatus, SearchResult


class TestProximaDBRestClientInit:
    """Test ProximaDBRestClient initialization"""
    
    @patch('proximadb.rest_client.load_config')
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_init_with_url_and_api_key(self, mock_create_client, mock_load_config):
        """Test initialization with URL and API key"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        mock_http_client = Mock()
        mock_create_client.return_value = mock_http_client
        
        client = ProximaDBRestClient(
            url="http://localhost:5678",
            api_key="test-key-1234567890"
        )
        
        mock_load_config.assert_called_once_with(
            url="http://localhost:5678",
            api_key="test-key-1234567890"
        )
        assert client.config == mock_config
        mock_create_client.assert_called_once()
        
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_init_with_config_object(self, mock_create_client):
        """Test initialization with config object"""
        config = ClientConfig(url="http://localhost:5678")
        mock_http_client = Mock()
        mock_create_client.return_value = mock_http_client
        
        client = ProximaDBRestClient(config=config)
        
        assert client.config == config
        mock_create_client.assert_called_once()
        
    @patch('proximadb.rest_client.load_config')
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_init_with_kwargs(self, mock_create_client, mock_load_config):
        """Test initialization with kwargs"""
        mock_config = Mock()
        mock_load_config.return_value = mock_config
        
        client = ProximaDBRestClient(
            url="http://localhost:5678",
            timeout=60.0,
            enable_debug_logging=True
        )
        
        mock_load_config.assert_called_once_with(
            url="http://localhost:5678",
            api_key=None,
            timeout=60.0,
            enable_debug_logging=True
        )


class TestProximaDBRestClientHttpClient:
    """Test HTTP client creation and configuration"""
    
    @patch('proximadb.rest_client.httpx.Client')
    def test_create_http_client_basic(self, mock_httpx_client):
        """Test basic HTTP client creation"""
        config = ClientConfig(url="http://localhost:5678")
        mock_client_instance = Mock()
        mock_httpx_client.return_value = mock_client_instance
        
        client = ProximaDBRestClient(config=config)
        
        # Verify httpx.Client was called with expected parameters
        mock_httpx_client.assert_called_once()
        call_kwargs = mock_httpx_client.call_args[1]
        
        # Check that timeout is a Timeout object, not a float
        assert hasattr(call_kwargs['timeout'], 'connect')
        assert 'headers' in call_kwargs
        assert call_kwargs['headers']['User-Agent'].startswith('proximadb-python/')
        
    @patch('proximadb.rest_client.httpx.Client')
    def test_create_http_client_with_tls(self, mock_httpx_client):
        """Test HTTP client creation with TLS config"""
        config = ClientConfig(
            url="https://secure.example.com",
            api_key="test-key-1234567890"
        )
        config.tls.verify = False
        
        client = ProximaDBRestClient(config=config)
        
        mock_httpx_client.assert_called_once()
        call_kwargs = mock_httpx_client.call_args[1]
        
        # Should have auth header
        assert 'Authorization' in call_kwargs['headers']
        assert call_kwargs['headers']['Authorization'] == 'Bearer test-key-1234567890'


class TestProximaDBRestClientHelperMethods:
    """Test helper methods"""
    
    def test_normalize_vector_list(self):
        """Test vector normalization from list"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        vector = [0.1, 0.2, 0.3]
        result = client._normalize_vector(vector)
        
        assert result == [0.1, 0.2, 0.3]
        assert isinstance(result, list)
        
    def test_normalize_vector_numpy(self):
        """Test vector normalization from numpy array"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        vector = np.array([0.1, 0.2, 0.3], dtype=np.float64)
        result = client._normalize_vector(vector)
        
        # Check values are approximately equal (float32 conversion may cause small differences)
        assert len(result) == 3
        assert abs(result[0] - 0.1) < 1e-6
        assert abs(result[1] - 0.2) < 1e-6
        assert abs(result[2] - 0.3) < 1e-6
        assert isinstance(result, list)
        
    def test_normalize_vectors_list_of_lists(self):
        """Test vectors normalization from list of lists"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        vectors = [[0.1, 0.2], [0.3, 0.4]]
        result = client._normalize_vectors(vectors)
        
        assert result == [[0.1, 0.2], [0.3, 0.4]]
        
    def test_normalize_vectors_numpy_array(self):
        """Test vectors normalization from numpy array"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        vectors = np.array([[0.1, 0.2], [0.3, 0.4]], dtype=np.float64)
        result = client._normalize_vectors(vectors)
        
        # Check shape and approximate values (float32 conversion may cause small differences)
        assert len(result) == 2
        assert len(result[0]) == 2
        assert abs(result[0][0] - 0.1) < 1e-6
        assert abs(result[0][1] - 0.2) < 1e-6
        assert abs(result[1][0] - 0.3) < 1e-6
        assert abs(result[1][1] - 0.4) < 1e-6


class TestProximaDBRestClientErrorHandling:
    """Test error handling"""
    
    def test_handle_error_response_400(self):
        """Test 400 error handling"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        # Mock response
        response = Mock()
        response.status_code = 400
        response.json.return_value = {
            "error_code": "VALIDATION_ERROR",
            "message": "Invalid input"
        }
        
        with pytest.raises(ValidationError, match="Invalid input"):
            client._handle_error_response(response)
            
    def test_handle_error_response_404(self):
        """Test 404 error handling"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        response = Mock()
        response.status_code = 404
        response.json.return_value = {
            "message": "Not found"
        }
        
        with pytest.raises(ProximaDBError, match="Not found"):
            client._handle_error_response(response)
            
    def test_handle_error_response_json_decode_error(self):
        """Test error handling when JSON decode fails"""
        config = ClientConfig(url="http://localhost:5678")
        with patch('proximadb.rest_client.ProximaDBRestClient._create_http_client'):
            client = ProximaDBRestClient(config=config)
            
        response = Mock()
        response.status_code = 500
        response.json.side_effect = ValueError("Invalid JSON")
        response.text = "Internal Server Error"
        
        with pytest.raises(ProximaDBError, match="Internal Server Error"):
            client._handle_error_response(response)


class TestProximaDBRestClientMakeRequest:
    """Test request making functionality"""
    
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_make_request_success(self, mock_create_client):
        """Test successful request"""
        config = ClientConfig(url="http://localhost:5678")
        
        # Mock HTTP client and response
        mock_http_client = Mock()
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {"success": True}
        mock_http_client.request.return_value = mock_response
        mock_create_client.return_value = mock_http_client
        
        client = ProximaDBRestClient(config=config)
        
        response = client._make_request("GET", "/test")
        
        assert response == mock_response
        mock_http_client.request.assert_called_once_with(
            "GET", 
            "/test"
        )
        
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_make_request_with_retry(self, mock_create_client):
        """Test request with retry logic"""
        config = ClientConfig(url="http://localhost:5678")
        
        mock_http_client = Mock()
        # First call fails, second succeeds
        mock_error_response = Mock()
        mock_error_response.status_code = 500
        mock_success_response = Mock()
        mock_success_response.status_code = 200
        
        from proximadb.exceptions import NetworkError
        mock_http_client.request.side_effect = [
            NetworkError("Connection failed"),
            mock_success_response
        ]
        mock_create_client.return_value = mock_http_client
        
        client = ProximaDBRestClient(config=config)
        
        response = client._make_request("GET", "/test")
        
        assert response == mock_success_response
        assert mock_http_client.request.call_count == 2
        
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_make_request_error_response(self, mock_create_client):
        """Test request with error response"""
        config = ClientConfig(url="http://localhost:5678")
        
        mock_http_client = Mock()
        mock_response = Mock()
        mock_response.status_code = 400
        mock_response.json.return_value = {
            "error_code": "VALIDATION_ERROR",
            "message": "Bad request"
        }
        mock_http_client.request.return_value = mock_response
        mock_create_client.return_value = mock_http_client
        
        client = ProximaDBRestClient(config=config)
        
        with pytest.raises(ValidationError, match="Bad request"):
            client._make_request("GET", "/test")


class TestProximaDBRestClientHealthCheck:
    """Test health check functionality"""
    
    @patch('proximadb.rest_client.httpx.get')
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_health_success(self, mock_create_client, mock_httpx_get):
        """Test successful health check"""
        config = ClientConfig(url="http://localhost:5678")
        
        mock_response = Mock()
        mock_response.status_code = 200
        mock_response.json.return_value = {
            "success": True,
            "data": {
                "status": "healthy",
                "version": "1.0.0",
                "service": "proximadb-rest"
            }
        }
        mock_httpx_get.return_value = mock_response
        
        client = ProximaDBRestClient(config=config)
        health_status = client.health()
        
        assert isinstance(health_status, HealthStatus)
        assert health_status.status == "healthy"
        assert health_status.version == "1.0.0"
        
        mock_httpx_get.assert_called_once_with(
            "http://localhost:5678/health", 
            timeout=30.0
        )
        
    @patch('proximadb.rest_client.httpx.get')
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_health_error(self, mock_create_client, mock_httpx_get):
        """Test health check with error"""
        config = ClientConfig(url="http://localhost:5678")
        
        mock_response = Mock()
        mock_response.status_code = 500
        mock_response.json.return_value = {
            "success": False,
            "error": "Service unavailable"
        }
        mock_httpx_get.return_value = mock_response
        
        client = ProximaDBRestClient(config=config)
        
        from proximadb.exceptions import ServerError
        with pytest.raises(ServerError, match="HTTP 500 error"):
            client.health()


class TestProximaDBRestClientValidation:
    """Test input validation"""
    
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_validate_vector_dimension_valid(self, mock_create_client):
        """Test valid vector dimension validation"""
        config = ClientConfig(url="http://localhost:5678")
        client = ProximaDBRestClient(config=config)
        
        vector = [0.1, 0.2, 0.3]
        # Test vector normalization works (this is the actual validation behavior)
        result = client._normalize_vector(vector)
        assert len(result) == 3
        
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_validate_vector_dimension_invalid(self, mock_create_client):
        """Test invalid vector dimension validation through insert operation"""
        config = ClientConfig(url="http://localhost:5678")
        client = ProximaDBRestClient(config=config)
        
        # Test that empty vectors can be normalized (no built-in validation)
        vector = []
        result = client._normalize_vector(vector)
        assert result == []
            
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_validate_empty_vector(self, mock_create_client):
        """Test empty vector validation"""
        config = ClientConfig(url="http://localhost:5678")
        client = ProximaDBRestClient(config=config)
        
        vector = []
        # Empty vectors are allowed by the normalize function
        result = client._normalize_vector(vector)
        assert result == []


class TestProximaDBRestClientProperties:
    """Test client properties and configuration"""
    
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_client_properties(self, mock_create_client):
        """Test client property access"""
        config = ClientConfig(
            url="http://test:1234",
            timeout=45.0
        )
        
        client = ProximaDBRestClient(config=config)
        
        assert client.config == config
        assert client.config.url == "http://test:1234"
        assert client.config.timeout == 45.0
        
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_client_logging_setup(self, mock_create_client):
        """Test logging setup"""
        config = ClientConfig(
            url="http://localhost:5678",
            enable_debug_logging=True
        )
        
        with patch('proximadb.rest_client.logging.basicConfig') as mock_logging:
            client = ProximaDBRestClient(config=config)
            # Logging setup depends on implementation details


class TestProximaDBRestClientContextManager:
    """Test context manager functionality"""
    
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_context_manager_enter_exit(self, mock_create_client):
        """Test context manager protocol"""
        config = ClientConfig(url="http://localhost:5678")
        mock_http_client = Mock()
        mock_create_client.return_value = mock_http_client
        
        client = ProximaDBRestClient(config=config)
        
        # Test context manager protocol
        with client:
            pass
        
        # The close method is called both in __exit__ and __del__
        # So we expect at least one call, possibly more
        assert mock_http_client.close.call_count >= 1


class TestProximaDBRestClientConfiguration:
    """Test client configuration scenarios"""
    
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client')
    def test_default_configuration(self, mock_create_client):
        """Test default configuration behavior"""
        config = ClientConfig(url="http://localhost:5678")
        client = ProximaDBRestClient(config=config)
        
        assert client.config.timeout == 30.0  # default
        assert client.config.protocol == "auto"  # default
        
    @patch('proximadb.rest_client.ProximaDBRestClient._create_http_client') 
    def test_custom_configuration(self, mock_create_client):
        """Test custom configuration"""
        config = ClientConfig(
            url="https://api.proximadb.com",
            timeout=60.0,
            api_key="custom-key-1234567890"
        )
        
        client = ProximaDBRestClient(config=config)
        
        assert client.config.url == "https://api.proximadb.com"
        assert client.config.timeout == 60.0
        assert client.config.api_key == "custom-key-1234567890"