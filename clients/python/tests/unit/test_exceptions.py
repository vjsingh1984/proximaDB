"""
Unit tests for ProximaDB exceptions module.
Tests exception hierarchy and error mapping functionality.
"""

import pytest
from proximadb.exceptions import (
    ProximaDBError, NetworkError, TimeoutError, ValidationError,
    CollectionNotFoundError, CollectionExistsError, VectorNotFoundError,
    AuthenticationError, AuthorizationError, QuotaExceededError,
    ServerError, RateLimitError, VectorDimensionError, InvalidVectorError,
    ConfigurationError, IndexError, BatchError, StreamingError,
    map_http_error
)
# No imports needed for these tests


class TestProximaDBExceptions:
    """Test ProximaDB exception classes"""
    
    def test_base_exception(self):
        """Test base ProximaDBError"""
        error = ProximaDBError("Test error")
        assert str(error) == "Test error"
        assert isinstance(error, Exception)
    
    def test_network_error(self):
        """Test NetworkError"""
        error = NetworkError("Connection failed")
        assert "Connection failed" in str(error)
        assert "NETWORK_ERROR" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_timeout_error(self):
        """Test TimeoutError"""
        error = TimeoutError("Request timed out")
        assert "Request timed out" in str(error)
        assert "TIMEOUT" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_validation_error(self):
        """Test ValidationError"""
        error = ValidationError("Invalid input")
        assert "Invalid input" in str(error)
        assert "VALIDATION_ERROR" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_collection_not_found_error(self):
        """Test CollectionNotFoundError"""
        error = CollectionNotFoundError("test_collection")
        assert "Collection not found: test_collection" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_collection_exists_error(self):
        """Test CollectionExistsError"""
        error = CollectionExistsError("test_collection")
        assert "Collection already exists: test_collection" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_authentication_error(self):
        """Test AuthenticationError"""
        error = AuthenticationError("Access denied")
        assert "Access denied" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_authorization_error(self):
        """Test AuthorizationError"""
        error = AuthorizationError("Authorization failed")
        assert "Authorization failed" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_quota_exceeded_error(self):
        """Test QuotaExceededError"""
        error = QuotaExceededError("Storage quota exceeded")
        assert "Storage quota exceeded" in str(error)
        assert "QUOTA_EXCEEDED" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_server_error(self):
        """Test ServerError"""
        error = ServerError("Internal server error")
        assert "Internal server error" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_rate_limit_error(self):
        """Test RateLimitError"""
        error = RateLimitError("Rate limit exceeded")
        assert "Rate limit exceeded" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_vector_dimension_error(self):
        """Test VectorDimensionError"""
        error = VectorDimensionError(expected=768, actual=512)
        assert "Vector dimension mismatch" in str(error)
        assert "768" in str(error)
        assert "512" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_invalid_vector_error(self):
        """Test InvalidVectorError"""
        error = InvalidVectorError("Invalid vector format")
        assert "Invalid vector format" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_configuration_error(self):
        """Test ConfigurationError"""
        error = ConfigurationError("Invalid configuration")
        assert "Invalid configuration" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_batch_error(self):
        """Test BatchError"""
        error = BatchError("Batch operation failed")
        assert "Batch operation failed" in str(error)
        assert isinstance(error, ProximaDBError)
    
    def test_streaming_error(self):
        """Test StreamingError"""
        error = StreamingError("Streaming failed")
        assert "Streaming failed" in str(error)
        assert isinstance(error, ProximaDBError)


class TestErrorMapping:
    """Test error mapping functions"""
    
    def test_map_http_error_400(self):
        """Test mapping HTTP 400 error"""
        response_data = {"message": "Bad request", "error_code": "VALIDATION_ERROR"}
        error = map_http_error(400, response_data)
        assert isinstance(error, ValidationError)
        assert "Bad request" in str(error)
    
    def test_map_http_error_404(self):
        """Test mapping HTTP 404 error"""
        response_data = {"message": "Not found", "error_code": "NOT_FOUND"}
        error = map_http_error(404, response_data)
        assert isinstance(error, ProximaDBError)
        assert "Not found" in str(error)
    
    def test_map_http_error_500(self):
        """Test mapping HTTP 500 error"""
        response_data = {"message": "Server error", "error_code": "SERVER_ERROR"}
        error = map_http_error(500, response_data)
        assert isinstance(error, ServerError)
        assert "Server error" in str(error)


class TestExceptionFeatures:
    """Test exception features and properties"""
    
    def test_exception_with_error_code(self):
        """Test exception with error code"""
        error = ProximaDBError("Test error", error_code="E001")
        assert "Test error" in str(error)
        assert "E001" in str(error)
    
    def test_exception_with_request_id(self):
        """Test exception with request ID"""
        error = ProximaDBError("Test error", request_id="req_123")
        assert "Test error" in str(error)
        assert "req_123" in str(error)
    
    def test_exception_with_details(self):
        """Test exception with details"""
        details = {"field": "dimension", "value": -1}
        error = ValidationError("Invalid input", details=details)
        assert error.details == details
        assert error.details["field"] == "dimension"


# Additional tests for special exception features and edge cases