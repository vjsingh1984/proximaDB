"""
Test suite for ProximaDB exception classes
"""

from proximadb_sdk import (
    AuthenticationError,
    AuthorizationError,
    BatchError,
    CollectionExistsError,
    CollectionNotFoundError,
    ConfigurationError,
    InvalidVectorError,
    NetworkError,
    ProximaDBError,
    ProximaIndexError,
    QuotaExceededError,
    RateLimitError,
    ServerError,
    StreamingError,
    TimeoutError,
    ValidationError,
    VectorDimensionError,
    VectorNotFoundError,
    WALError,
    map_grpc_error,
    map_http_error,
)


class TestExceptionClasses:
    """Test all exception classes and their properties"""

    def test_base_exception(self):
        """Test base ProximaDBError"""
        exc = ProximaDBError("Test error")
        assert "Test error" in str(exc)
        assert isinstance(exc, Exception)

        # Test with all parameters
        exc = ProximaDBError(
            "Test error",
            error_code="TEST001",
            details={"key": "value"},
            request_id="req-123",
        )
        assert exc.message == "Test error"
        assert exc.error_code == "TEST001"
        assert exc.details == {"key": "value"}
        assert exc.request_id == "req-123"
        assert "Test error" in str(exc)
        assert "TEST001" in str(exc)

    def test_authentication_error(self):
        """Test AuthenticationError"""
        exc = AuthenticationError("Auth failed")
        assert "Auth failed" in str(exc)
        assert "AUTH_FAILED" in str(exc)
        assert isinstance(exc, ProximaDBError)

        # Default message
        exc = AuthenticationError()
        assert "Authentication failed" in str(exc)

    def test_authorization_error(self):
        """Test AuthorizationError"""
        exc = AuthorizationError("Not authorized")
        assert "Not authorized" in str(exc)
        assert "AUTH_INSUFFICIENT" in str(exc)
        assert isinstance(exc, ProximaDBError)

    def test_collection_not_found_error(self):
        """Test CollectionNotFoundError"""
        exc = CollectionNotFoundError("test_collection")
        assert "Collection not found: test_collection" in str(exc)
        assert "COLLECTION_NOT_FOUND" in str(exc)
        assert exc.collection_id == "test_collection"
        assert isinstance(exc, ProximaDBError)

    def test_collection_exists_error(self):
        """Test CollectionExistsError"""
        exc = CollectionExistsError("test_collection")
        assert "Collection already exists: test_collection" in str(exc)
        assert "COLLECTION_EXISTS" in str(exc)
        assert exc.collection_name == "test_collection"
        assert isinstance(exc, ProximaDBError)

    def test_vector_not_found_error(self):
        """Test VectorNotFoundError"""
        exc = VectorNotFoundError("vec_123")
        assert "Vector not found: vec_123" in str(exc)
        assert "VECTOR_NOT_FOUND" in str(exc)
        assert exc.vector_id == "vec_123"
        assert isinstance(exc, ProximaDBError)

    def test_vector_dimension_error(self):
        """Test VectorDimensionError"""
        exc = VectorDimensionError(expected=128, actual=256)
        assert "Vector dimension mismatch: expected 128, got 256" in str(exc)
        assert "DIMENSION_MISMATCH" in str(exc)
        assert exc.expected_dimension == 128
        assert exc.actual_dimension == 256
        assert isinstance(exc, ProximaDBError)

    def test_invalid_vector_error(self):
        """Test InvalidVectorError"""
        exc = InvalidVectorError("Invalid vector format")
        assert "Invalid vector format" in str(exc)
        assert "INVALID_VECTOR" in str(exc)
        assert isinstance(exc, ProximaDBError)

    def test_rate_limit_error(self):
        """Test RateLimitError"""
        exc = RateLimitError("Rate limit exceeded", retry_after=60)
        assert "Rate limit exceeded" in str(exc)
        assert "RATE_LIMIT_EXCEEDED" in str(exc)
        assert exc.retry_after == 60
        assert isinstance(exc, ProximaDBError)

        # Default message
        exc = RateLimitError()
        assert "Rate limit exceeded" in str(exc)

    def test_quota_exceeded_error(self):
        """Test QuotaExceededError"""
        exc = QuotaExceededError("Quota exceeded", quota_type="storage")
        assert "Quota exceeded" in str(exc)
        assert "QUOTA_EXCEEDED" in str(exc)
        assert exc.quota_type == "storage"
        assert isinstance(exc, ProximaDBError)

    def test_validation_error(self):
        """Test ValidationError"""
        exc = ValidationError("Invalid input", field="dimension")
        assert "Invalid input" in str(exc)
        assert "VALIDATION_ERROR" in str(exc)
        assert exc.field == "dimension"
        assert isinstance(exc, ProximaDBError)

    def test_server_error(self):
        """Test ServerError"""
        exc = ServerError("Internal server error", status_code=500)
        assert "Internal server error" in str(exc)
        assert "SERVER_ERROR" in str(exc)
        assert exc.status_code == 500
        assert isinstance(exc, ProximaDBError)

    def test_network_error(self):
        """Test NetworkError"""
        original = Exception("Connection failed")
        exc = NetworkError("Connection failed", original_error=original)
        assert "Connection failed" in str(exc)
        assert "NETWORK_ERROR" in str(exc)
        assert exc.original_error == original
        assert isinstance(exc, ProximaDBError)

    def test_timeout_error(self):
        """Test TimeoutError"""
        exc = TimeoutError("Request timeout", timeout_seconds=30)
        assert "Request timeout" in str(exc)
        assert "TIMEOUT" in str(exc)
        assert exc.timeout_seconds == 30
        assert isinstance(exc, ProximaDBError)

    def test_configuration_error(self):
        """Test ConfigurationError"""
        exc = ConfigurationError("Invalid configuration")
        assert "Invalid configuration" in str(exc)
        assert "CONFIG_ERROR" in str(exc)
        assert isinstance(exc, ProximaDBError)

    def test_index_error(self):
        """Test ProximaIndexError"""
        exc = ProximaIndexError("Index operation failed", index_type="hnsw")
        assert "Index operation failed" in str(exc)
        assert "INDEX_ERROR" in str(exc)
        assert exc.index_type == "hnsw"
        assert isinstance(exc, ProximaDBError)

    def test_batch_error(self):
        """Test BatchError"""
        errors = [{"id": "vec1", "error": "Invalid"}]
        exc = BatchError(
            "Batch operation partially failed",
            successful_count=9,
            failed_count=1,
            errors=errors,
        )
        assert "Batch operation partially failed" in str(exc)
        assert "BATCH_ERROR" in str(exc)
        assert exc.successful_count == 9
        assert exc.failed_count == 1
        assert exc.errors == errors
        assert isinstance(exc, ProximaDBError)

    def test_wal_error(self):
        """Test WALError"""
        exc = WALError("WAL operation failed")
        assert "WAL operation failed" in str(exc)
        assert "WAL_ERROR" in str(exc)
        assert isinstance(exc, ProximaDBError)

    def test_streaming_error(self):
        """Test StreamingError"""
        exc = StreamingError("Stream interrupted")
        assert "Stream interrupted" in str(exc)
        assert "STREAMING_ERROR" in str(exc)
        assert isinstance(exc, ProximaDBError)


class TestMapHttpError:
    """Test map_http_error function"""

    def test_400_validation_error(self):
        """Test 400 with VALIDATION_ERROR code"""
        exc = map_http_error(
            400, {"error_code": "VALIDATION_ERROR", "message": "Invalid input"}
        )
        assert isinstance(exc, ValidationError)
        assert "Invalid input" in str(exc)

    def test_400_dimension_mismatch(self):
        """Test 400 with DIMENSION_MISMATCH code"""
        exc = map_http_error(
            400,
            {
                "error_code": "DIMENSION_MISMATCH",
                "message": "Dimension mismatch",
                "details": {"expected_dimension": 128, "actual_dimension": 256},
            },
        )
        assert isinstance(exc, VectorDimensionError)
        assert exc.expected_dimension == 128
        assert exc.actual_dimension == 256

    def test_401_unauthorized(self):
        """Test 401 Unauthorized"""
        exc = map_http_error(401, {"message": "Unauthorized"})
        assert isinstance(exc, AuthenticationError)

    def test_403_forbidden(self):
        """Test 403 Forbidden"""
        exc = map_http_error(403, {"message": "Forbidden"})
        assert isinstance(exc, AuthorizationError)

    def test_404_collection_not_found(self):
        """Test 404 with COLLECTION_NOT_FOUND code"""
        exc = map_http_error(
            404,
            {
                "error_code": "COLLECTION_NOT_FOUND",
                "message": "Not found",
                "details": {"collection_id": "test_collection"},
            },
        )
        assert isinstance(exc, CollectionNotFoundError)
        assert exc.collection_id == "test_collection"

    def test_404_vector_not_found(self):
        """Test 404 with VECTOR_NOT_FOUND code"""
        exc = map_http_error(
            404,
            {
                "error_code": "VECTOR_NOT_FOUND",
                "message": "Not found",
                "details": {"vector_id": "vec_123"},
            },
        )
        assert isinstance(exc, VectorNotFoundError)
        assert exc.vector_id == "vec_123"

    def test_404_generic(self):
        """Test 404 generic not found"""
        exc = map_http_error(404, {"message": "Not found"})
        assert isinstance(exc, ProximaDBError)
        assert "Not found" in str(exc)

    def test_409_collection_exists(self):
        """Test 409 with COLLECTION_EXISTS code"""
        exc = map_http_error(
            409,
            {
                "error_code": "COLLECTION_EXISTS",
                "message": "Conflict",
                "details": {"collection_name": "test_collection"},
            },
        )
        assert isinstance(exc, CollectionExistsError)
        assert exc.collection_name == "test_collection"

    def test_413_quota_exceeded(self):
        """Test 413 Payload Too Large"""
        exc = map_http_error(
            413,
            {"message": "Payload too large", "details": {"quota_type": "request_size"}},
        )
        assert isinstance(exc, QuotaExceededError)
        assert exc.quota_type == "request_size"

    def test_429_rate_limit(self):
        """Test 429 Too Many Requests"""
        exc = map_http_error(
            429, {"message": "Rate limit exceeded", "details": {"retry_after": 60}}
        )
        assert isinstance(exc, RateLimitError)
        assert exc.retry_after == 60

    def test_500_server_error(self):
        """Test 500 Internal Server Error"""
        exc = map_http_error(500, {"message": "Internal server error"})
        assert isinstance(exc, ServerError)
        assert exc.status_code == 500

    def test_503_server_error(self):
        """Test 503 Service Unavailable"""
        exc = map_http_error(503, {"message": "Service unavailable"})
        assert isinstance(exc, ServerError)
        assert exc.status_code == 503

    def test_generic_error(self):
        """Test generic error mapping"""
        exc = map_http_error(418, {"message": "I'm a teapot"})
        assert isinstance(exc, ProximaDBError)
        assert "I'm a teapot" in str(exc)


class TestMapGrpcError:
    """Test map_grpc_error function"""

    def test_grpc_not_found(self):
        """Test gRPC NOT_FOUND status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.NOT_FOUND

            def details(self):
                return "Resource not found"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, ProximaDBError)
        assert "Resource not found" in str(exc)
        assert exc.error_code == "NOT_FOUND"

    def test_grpc_already_exists(self):
        """Test gRPC ALREADY_EXISTS status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.ALREADY_EXISTS

            def details(self):
                return "Resource already exists"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, ProximaDBError)
        assert "Resource already exists" in str(exc)
        assert exc.error_code == "ALREADY_EXISTS"

    def test_grpc_permission_denied(self):
        """Test gRPC PERMISSION_DENIED status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.PERMISSION_DENIED

            def details(self):
                return "Permission denied"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, AuthorizationError)

    def test_grpc_unauthenticated(self):
        """Test gRPC UNAUTHENTICATED status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.UNAUTHENTICATED

            def details(self):
                return "Unauthenticated"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, AuthenticationError)

    def test_grpc_resource_exhausted(self):
        """Test gRPC RESOURCE_EXHAUSTED status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.RESOURCE_EXHAUSTED

            def details(self):
                return "Resource exhausted"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, RateLimitError)

    def test_grpc_invalid_argument(self):
        """Test gRPC INVALID_ARGUMENT status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.INVALID_ARGUMENT

            def details(self):
                return "Invalid argument"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, ValidationError)

    def test_grpc_internal(self):
        """Test gRPC INTERNAL status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.INTERNAL

            def details(self):
                return "Internal error"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, ServerError)

    def test_grpc_unavailable(self):
        """Test gRPC UNAVAILABLE status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.UNAVAILABLE

            def details(self):
                return "Service unavailable"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, NetworkError)

    def test_grpc_deadline_exceeded(self):
        """Test gRPC DEADLINE_EXCEEDED status"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.DEADLINE_EXCEEDED

            def details(self):
                return "Deadline exceeded"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, TimeoutError)

    def test_grpc_unknown_error(self):
        """Test gRPC with unknown status code"""
        import grpc

        class MockGrpcError:
            def code(self):
                return grpc.StatusCode.DATA_LOSS

            def details(self):
                return "Data loss"

        exc = map_grpc_error(MockGrpcError())
        assert isinstance(exc, ProximaDBError)
        assert "Data loss" in str(exc)


class TestCanonicalErrorEnvelope:
    """The server emits {"error":{"type,message,code,request_id}}; map_http_error
    must normalise it (lowercase type -> typed exception) and pull request_id
    from the body or the X-Request-ID header."""

    def test_nested_envelope_maps_to_typed_exception(self):
        from proximadb_sdk.exceptions import map_http_error

        exc = map_http_error(
            404,
            {
                "error": {
                    "type": "collection_not_found",
                    "message": "Collection not found: c1",
                    "code": 404,
                }
            },
            headers={"x-request-id": "rid-42"},
        )
        assert isinstance(exc, CollectionNotFoundError)
        assert exc.request_id == "rid-42"  # from header (not in body)

    def test_nested_envelope_request_id_from_body(self):
        from proximadb_sdk.exceptions import map_http_error

        exc = map_http_error(
            400,
            {
                "error": {
                    "type": "validation_error",
                    "message": "bad",
                    "code": 400,
                    "request_id": "b-7",
                }
            },
        )
        assert isinstance(exc, ValidationError)
        assert exc.request_id == "b-7"

    def test_legacy_flat_shape_still_supported(self):
        from proximadb_sdk.exceptions import map_http_error

        exc = map_http_error(500, {"error_code": "INTERNAL", "message": "boom"})
        assert isinstance(exc, ServerError)

    def test_bare_string_error_does_not_crash(self):
        from proximadb_sdk.exceptions import map_http_error

        exc = map_http_error(409, {"error": "already_exists", "message": "dup"})
        assert isinstance(exc, ProximaDBError)
