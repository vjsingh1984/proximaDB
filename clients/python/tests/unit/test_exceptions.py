#!/usr/bin/env python3
"""
Unit tests for ProximaDB exceptions module
"""
import pytest
import grpc
from proximadb.exceptions import (
    ProximaDBError,
    AuthenticationError,
    AuthorizationError,
    CollectionNotFoundError,
    CollectionExistsError,
    VectorNotFoundError,
    VectorDimensionError,
    InvalidVectorError,
    RateLimitError,
    QuotaExceededError,
    ValidationError,
    ServerError,
    NetworkError,
    TimeoutError,
    ConfigurationError,
    IndexError,
    BatchError,
    StreamingError,
    map_http_error,
    map_grpc_error
)


class TestProximaDBError:
    """Test base ProximaDBError class"""
    
    def test_base_error_creation(self):
        """Test basic error creation"""
        error = ProximaDBError("Test error")
        assert str(error) == "Test error"
        assert error.message == "Test error"
        assert error.error_code is None
        assert error.details == {}
        assert error.request_id is None
        
    def test_error_with_code(self):
        """Test error with error code"""
        error = ProximaDBError("Test error", error_code="TEST_001")
        assert error.message == "Test error"
        assert error.error_code == "TEST_001"
        assert "Error Code: TEST_001" in str(error)
        
    def test_error_with_details(self):
        """Test error with details"""
        details = {"field": "dimension", "value": "invalid"}
        error = ProximaDBError("Test error", details=details)
        assert error.details["field"] == "dimension"
        assert error.details["value"] == "invalid"
        
    def test_error_with_request_id(self):
        """Test error with request ID"""
        error = ProximaDBError("Test error", request_id="req-123")
        assert error.request_id == "req-123"
        assert "Request ID: req-123" in str(error)
        
    def test_error_full_params(self):
        """Test error with all parameters"""
        error = ProximaDBError(
            "Test error",
            error_code="TEST_001",
            details={"key": "value"},
            request_id="req-456"
        )
        error_str = str(error)
        assert "Test error" in error_str
        assert "Error Code: TEST_001" in error_str
        assert "Request ID: req-456" in error_str
        
    def test_error_inheritance(self):
        """Test error inheritance hierarchy"""
        error = ProximaDBError("Test error")
        assert isinstance(error, Exception)
        assert isinstance(error, ProximaDBError)


class TestAuthenticationError:
    """Test AuthenticationError class"""
    
    def test_authentication_error_default(self):
        """Test default authentication error"""
        error = AuthenticationError()
        assert isinstance(error, ProximaDBError)
        assert "Authentication failed" in str(error)
        assert error.error_code == "AUTH_FAILED"
        
    def test_authentication_error_custom(self):
        """Test custom authentication error"""
        error = AuthenticationError("Invalid API key")
        assert isinstance(error, ProximaDBError)
        assert "Invalid API key" in str(error)
        
    def test_authentication_error_with_details(self):
        """Test authentication error with details"""
        error = AuthenticationError(
            "Invalid token",
            details={"reason": "expired"},
            request_id="req-auth"
        )
        assert error.message == "Invalid token"
        assert error.error_code == "AUTH_FAILED"
        assert error.request_id == "req-auth"
        assert error.details["reason"] == "expired"


class TestAuthorizationError:
    """Test AuthorizationError class"""
    
    def test_authorization_error_default(self):
        """Test default authorization error"""
        error = AuthorizationError()
        assert isinstance(error, ProximaDBError)
        assert "Insufficient permissions" in str(error)
        assert error.error_code == "AUTH_INSUFFICIENT"
        
    def test_authorization_error_custom(self):
        """Test custom authorization error"""
        error = AuthorizationError("Insufficient permissions")
        assert "Insufficient permissions" in str(error)


class TestCollectionErrors:
    """Test collection-related errors"""
    
    def test_collection_not_found_error(self):
        """Test collection not found error with ID"""
        error = CollectionNotFoundError("test_collection")
        assert isinstance(error, ProximaDBError)
        assert "Collection not found: test_collection" in str(error)
        assert error.collection_id == "test_collection"
        assert error.error_code == "COLLECTION_NOT_FOUND"
        
    def test_collection_not_found_error_with_details(self):
        """Test collection not found error with details"""
        error = CollectionNotFoundError(
            "my_collection",
            details={"user_id": "123"},
            request_id="req-123"
        )
        assert "Collection not found: my_collection" in str(error)
        assert error.collection_id == "my_collection"
        assert error.details["user_id"] == "123"
        
    def test_collection_exists_error(self):
        """Test collection exists error with name"""
        error = CollectionExistsError("test_collection")
        assert isinstance(error, ProximaDBError)
        assert "Collection already exists: test_collection" in str(error)
        assert error.collection_name == "test_collection"
        assert error.error_code == "COLLECTION_EXISTS"
        
    def test_collection_exists_error_with_details(self):
        """Test collection exists error with details"""
        error = CollectionExistsError(
            "existing_collection",
            details={"created_at": "2023-01-01"},
            request_id="req-456"
        )
        assert "Collection already exists: existing_collection" in str(error)
        assert error.collection_name == "existing_collection"
        assert error.details["created_at"] == "2023-01-01"


class TestVectorErrors:
    """Test vector-related errors"""
    
    def test_vector_not_found_error(self):
        """Test vector not found error with ID"""
        error = VectorNotFoundError("vec_123")
        assert isinstance(error, ProximaDBError)
        assert "Vector not found: vec_123" in str(error)
        assert error.vector_id == "vec_123"
        assert error.error_code == "VECTOR_NOT_FOUND"
        
    def test_vector_not_found_error_with_details(self):
        """Test vector not found error with details"""
        error = VectorNotFoundError(
            "vec_123",
            details={"collection": "test_collection"},
            request_id="req-789"
        )
        assert "Vector not found: vec_123" in str(error)
        assert error.vector_id == "vec_123"
        assert error.details["collection"] == "test_collection"
        
    def test_vector_dimension_error_creation(self):
        """Test vector dimension error creation"""
        error = VectorDimensionError(768, 512)
        assert isinstance(error, ProximaDBError)
        assert "expected 768, got 512" in str(error)
        assert error.expected_dimension == 768
        assert error.actual_dimension == 512
        assert error.error_code == "DIMENSION_MISMATCH"
        
    def test_vector_dimension_error_with_details(self):
        """Test vector dimension error with details"""
        error = VectorDimensionError(
            768, 512,
            details={"vector_id": "vec_001"},
            request_id="req-dim"
        )
        error_str = str(error)
        assert "expected 768, got 512" in error_str
        assert error.expected_dimension == 768
        assert error.actual_dimension == 512
        assert error.details["vector_id"] == "vec_001"
        
    def test_invalid_vector_error_default(self):
        """Test default invalid vector error"""
        error = InvalidVectorError()
        assert isinstance(error, ProximaDBError)
        assert "Invalid vector data" in str(error)
        assert error.error_code == "INVALID_VECTOR"
        
    def test_invalid_vector_error_custom(self):
        """Test custom invalid vector error"""
        error = InvalidVectorError("Vector contains NaN values")
        assert "Vector contains NaN values" in str(error)


class TestRateLimitError:
    """Test RateLimitError class"""
    
    def test_rate_limit_error_default(self):
        """Test default rate limit error"""
        error = RateLimitError()
        assert isinstance(error, ProximaDBError)
        assert "Rate limit exceeded" in str(error)
        assert error.error_code == "RATE_LIMIT_EXCEEDED"
        
    def test_rate_limit_error_with_retry_after(self):
        """Test rate limit error with retry after"""
        error = RateLimitError(retry_after=3600)
        error_str = str(error)
        assert "Rate limit exceeded" in error_str
        assert error.retry_after == 3600
        
    def test_rate_limit_error_with_details(self):
        """Test rate limit error with details"""
        error = RateLimitError(
            "Too many requests",
            retry_after=120,
            details={"limit": 100, "remaining": 0}
        )
        error_str = str(error)
        assert "Too many requests" in error_str
        assert error.retry_after == 120
        assert error.details["limit"] == 100


class TestQuotaExceededError:
    """Test QuotaExceededError class"""
    
    def test_quota_exceeded_error_default(self):
        """Test default quota exceeded error"""
        error = QuotaExceededError()
        assert isinstance(error, ProximaDBError)
        assert "Usage quota exceeded" in str(error)
        assert error.error_code == "QUOTA_EXCEEDED"
        
    def test_quota_exceeded_error_with_quota_type(self):
        """Test quota exceeded error with quota type"""
        error = QuotaExceededError(quota_type="storage")
        assert "Usage quota exceeded" in str(error)
        assert error.quota_type == "storage"
        
    def test_quota_exceeded_error_with_details(self):
        """Test quota exceeded error with details"""
        error = QuotaExceededError(
            "Vector quota exceeded",
            quota_type="vectors",
            details={"used": 10000, "limit": 10000}
        )
        error_str = str(error)
        assert "Vector quota exceeded" in error_str
        assert error.quota_type == "vectors"
        assert error.details["used"] == 10000


class TestValidationError:
    """Test ValidationError class"""
    
    def test_validation_error_creation(self):
        """Test validation error creation"""
        error = ValidationError("Invalid input")
        assert isinstance(error, ProximaDBError)
        assert "Invalid input" in str(error)
        assert error.error_code == "VALIDATION_ERROR"
        
    def test_validation_error_with_field(self):
        """Test validation error with field"""
        error = ValidationError("Invalid dimension value", field="dimension")
        assert "Invalid dimension value" in str(error)
        assert error.field == "dimension"
        
    def test_validation_error_with_details(self):
        """Test validation error with details"""
        error = ValidationError(
            "Invalid input",
            field="dimension",
            details={"value": -1, "expected": "positive integer"}
        )
        assert error.field == "dimension"
        assert error.details["value"] == -1


class TestServerError:
    """Test ServerError class"""
    
    def test_server_error_default(self):
        """Test default server error"""
        error = ServerError()
        assert isinstance(error, ProximaDBError)
        assert "Internal server error" in str(error)
        assert error.error_code == "SERVER_ERROR"
        
    def test_server_error_with_status_code(self):
        """Test server error with status code"""
        error = ServerError(status_code=500)
        assert "Internal server error" in str(error)
        assert error.status_code == 500
        
    def test_server_error_with_details(self):
        """Test server error with details"""
        error = ServerError(
            "Database connection failed",
            status_code=503,
            details={"service": "database", "retry_after": 30}
        )
        error_str = str(error)
        assert "Database connection failed" in error_str
        assert error.status_code == 503
        assert error.details["service"] == "database"


class TestNetworkError:
    """Test NetworkError class"""
    
    def test_network_error_default(self):
        """Test default network error"""
        error = NetworkError()
        assert isinstance(error, ProximaDBError)
        assert "Network error" in str(error)
        assert error.error_code == "NETWORK_ERROR"
        
    def test_network_error_custom(self):
        """Test custom network error"""
        error = NetworkError("Connection refused")
        assert "Connection refused" in str(error)
        
    def test_network_error_with_original_error(self):
        """Test network error with original error"""
        original = ConnectionError("Connection refused")
        error = NetworkError(
            "Connection timeout",
            original_error=original,
            details={"host": "api.proximadb.com", "port": 443}
        )
        assert error.original_error == original
        assert error.details["host"] == "api.proximadb.com"


class TestTimeoutError:
    """Test TimeoutError class"""
    
    def test_timeout_error_default(self):
        """Test default timeout error"""
        error = TimeoutError()
        assert isinstance(error, ProximaDBError)
        assert "Request timeout" in str(error)
        assert error.error_code == "TIMEOUT"
        
    def test_timeout_error_with_timeout_seconds(self):
        """Test timeout error with timeout value"""
        error = TimeoutError(timeout_seconds=30.0)
        assert "Request timeout" in str(error)
        assert error.timeout_seconds == 30.0
        
    def test_timeout_error_with_details(self):
        """Test timeout error with details"""
        error = TimeoutError(
            "Search timeout",
            timeout_seconds=60.0,
            details={"collection": "documents", "vectors_searched": 1000}
        )
        error_str = str(error)
        assert "Search timeout" in error_str
        assert error.timeout_seconds == 60.0
        assert error.details["collection"] == "documents"


class TestConfigurationError:
    """Test ConfigurationError class"""
    
    def test_configuration_error_creation(self):
        """Test configuration error creation"""
        error = ConfigurationError("Invalid URL format")
        assert isinstance(error, ProximaDBError)
        assert "Invalid URL format" in str(error)
        assert error.error_code == "CONFIG_ERROR"
        
    def test_configuration_error_with_details(self):
        """Test configuration error with details"""
        error = ConfigurationError(
            "Invalid config file",
            details={"file": "config.toml", "line": 15}
        )
        assert "Invalid config file" in str(error)
        assert error.details["file"] == "config.toml"


class TestIndexError:
    """Test IndexError class"""
    
    def test_index_error_creation(self):
        """Test index error creation"""
        error = IndexError("Index not found")
        assert isinstance(error, ProximaDBError)
        assert "Index not found" in str(error)
        assert error.error_code == "INDEX_ERROR"
        
    def test_index_error_with_type(self):
        """Test index error with index type"""
        error = IndexError("HNSW index failed", index_type="hnsw")
        assert "HNSW index failed" in str(error)
        assert error.index_type == "hnsw"


class TestBatchError:
    """Test BatchError class"""
    
    def test_batch_error_creation(self):
        """Test batch error creation"""
        error = BatchError("Batch failed")
        assert isinstance(error, ProximaDBError)
        assert "Batch failed" in str(error)
        assert error.error_code == "BATCH_ERROR"
        
    def test_batch_error_with_counts(self):
        """Test batch error with success/fail counts"""
        errors = [{"id": "vec_1", "error": "Invalid dimension"}]
        error = BatchError(
            "Partial batch failure",
            successful_count=80,
            failed_count=20,
            errors=errors
        )
        assert "Partial batch failure" in str(error)
        assert error.successful_count == 80
        assert error.failed_count == 20
        assert len(error.errors) == 1


class TestStreamingError:
    """Test StreamingError class"""
    
    def test_streaming_error_creation(self):
        """Test streaming error creation"""
        error = StreamingError("Stream disconnected")
        assert isinstance(error, ProximaDBError)
        assert "Stream disconnected" in str(error)
        assert error.error_code == "STREAMING_ERROR"
        
    def test_streaming_error_with_details(self):
        """Test streaming error with details"""
        error = StreamingError(
            "Stream disconnected",
            details={"stream_id": "stream_123", "duration": 45}
        )
        assert "Stream disconnected" in str(error)
        assert error.details["stream_id"] == "stream_123"


class TestErrorHierarchy:
    """Test error class hierarchy and relationships"""
    
    def test_all_errors_inherit_from_base(self):
        """Test that all errors inherit from ProximaDBError"""
        # Test with required parameters for each error type
        errors = [
            AuthenticationError("Auth failed"),
            AuthorizationError("Access denied"),
            CollectionNotFoundError("test_collection"),
            CollectionExistsError("test_collection"),
            VectorNotFoundError("vec_123"),
            VectorDimensionError(768, 512),
            InvalidVectorError("Invalid vector"),
            RateLimitError("Rate limit exceeded"),
            QuotaExceededError("Quota exceeded"),
            ValidationError("Validation failed"),
            ServerError("Server error"),
            NetworkError("Network error"),
            TimeoutError("Timeout"),
            ConfigurationError("Config error"),
            IndexError("Index error"),
            BatchError("Batch error"),
            StreamingError("Streaming error")
        ]
        
        for error in errors:
            assert isinstance(error, ProximaDBError)
            assert isinstance(error, Exception)
        
    def test_error_attributes_preserved(self):
        """Test that error attributes are preserved across inheritance"""
        error = VectorDimensionError(
            768, 512,
            details={"vector_id": "vec_001"},
            request_id="req-test"
        )
        
        assert "expected 768, got 512" in error.message
        assert error.error_code == "DIMENSION_MISMATCH"
        assert error.details["vector_id"] == "vec_001" 
        assert error.request_id == "req-test"
        assert isinstance(error, ProximaDBError)
        assert isinstance(error, VectorDimensionError)


class TestHttpErrorMapping:
    """Test HTTP error mapping function"""
    
    def test_map_http_error_400_validation(self):
        """Test mapping 400 validation error"""
        response_data = {
            "error_code": "VALIDATION_ERROR",
            "message": "Invalid input",
            "request_id": "req-123"
        }
        error = map_http_error(400, response_data)
        assert isinstance(error, ValidationError)
        assert error.message == "Invalid input"
        assert error.request_id == "req-123"
        
    def test_map_http_error_400_dimension_mismatch(self):
        """Test mapping 400 dimension mismatch error"""
        response_data = {
            "error_code": "DIMENSION_MISMATCH",
            "message": "Dimension mismatch",
            "details": {"expected_dimension": 768, "actual_dimension": 512}
        }
        error = map_http_error(400, response_data)
        assert isinstance(error, VectorDimensionError)
        assert error.expected_dimension == 768
        assert error.actual_dimension == 512
        
    def test_map_http_error_401(self):
        """Test mapping 401 authentication error"""
        response_data = {"message": "Invalid token"}
        error = map_http_error(401, response_data)
        assert isinstance(error, AuthenticationError)
        assert error.message == "Invalid token"
        
    def test_map_http_error_403(self):
        """Test mapping 403 authorization error"""
        response_data = {"message": "Access denied"}
        error = map_http_error(403, response_data)
        assert isinstance(error, AuthorizationError)
        assert error.message == "Access denied"
        
    def test_map_http_error_404_collection(self):
        """Test mapping 404 collection not found error"""
        response_data = {
            "error_code": "COLLECTION_NOT_FOUND",
            "details": {"collection_id": "test_collection"}
        }
        error = map_http_error(404, response_data)
        assert isinstance(error, CollectionNotFoundError)
        assert error.collection_id == "test_collection"
        
    def test_map_http_error_404_vector(self):
        """Test mapping 404 vector not found error"""
        response_data = {
            "error_code": "VECTOR_NOT_FOUND",
            "details": {"vector_id": "vec_123"}
        }
        error = map_http_error(404, response_data)
        assert isinstance(error, VectorNotFoundError)
        assert error.vector_id == "vec_123"
        
    def test_map_http_error_409(self):
        """Test mapping 409 collection exists error"""
        response_data = {
            "error_code": "COLLECTION_EXISTS",
            "details": {"collection_name": "existing_collection"}
        }
        error = map_http_error(409, response_data)
        assert isinstance(error, CollectionExistsError)
        assert error.collection_name == "existing_collection"
        
    def test_map_http_error_429(self):
        """Test mapping 429 rate limit error"""
        response_data = {
            "message": "Rate limit exceeded",
            "details": {"retry_after": 60}
        }
        error = map_http_error(429, response_data)
        assert isinstance(error, RateLimitError)
        assert error.retry_after == 60
        
    def test_map_http_error_413(self):
        """Test mapping 413 quota exceeded error"""
        response_data = {
            "message": "Quota exceeded",
            "details": {"quota_type": "storage"}
        }
        error = map_http_error(413, response_data)
        assert isinstance(error, QuotaExceededError)
        assert error.quota_type == "storage"
        
    def test_map_http_error_500(self):
        """Test mapping 500 server error"""
        response_data = {"message": "Internal server error"}
        error = map_http_error(500, response_data)
        assert isinstance(error, ServerError)
        assert error.status_code == 500
        
    def test_map_http_error_unknown(self):
        """Test mapping unknown status code"""
        response_data = {"message": "Unknown error"}
        error = map_http_error(418, response_data)
        assert isinstance(error, ProximaDBError)
        assert error.message == "Unknown error"


class TestGrpcErrorMapping:
    """Test gRPC error mapping function"""
    
    def test_map_grpc_error_unauthenticated(self):
        """Test mapping gRPC unauthenticated error"""
        try:
            import grpc
            
            class MockError:
                def code(self):
                    return grpc.StatusCode.UNAUTHENTICATED
                    
                def details(self):
                    return "Authentication failed"
            
            error = map_grpc_error(MockError())
            assert isinstance(error, AuthenticationError)
            assert "Authentication failed" in str(error)
        except ImportError:
            # Skip test if grpc not available
            pytest.skip("grpc module not available")