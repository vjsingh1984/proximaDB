"""Offline unit coverage for proximadb_sdk.exceptions.

Pure module: instantiate/raise every exception, exercise message formatting,
and cover both map_http_error envelope shapes + every status branch and the
map_grpc_error status-code mapping (with a fake grpc error) plus the
ImportError fallback.
"""

import sys
import types

import pytest

from proximadb_sdk import exceptions as ex
from proximadb_sdk.exceptions import (
    AuthenticationError,
    AuthorizationError,
    BatchError,
    CollectionExistsError,
    CollectionNotFoundError,
    ConfigurationError,
    IndexError,
    InvalidVectorError,
    NetworkError,
    ProximaDBError,
    QuotaExceededError,
    RateLimitError,
    ServerError,
    StreamingError,
    TimeoutError,
    TransportError,
    ValidationError,
    VectorDimensionError,
    VectorNotFoundError,
    WALError,
    map_grpc_error,
    map_http_error,
)


# --------------------------------------------------------------------------
# Base exception + __str__ formatting
# --------------------------------------------------------------------------
def test_base_minimal():
    e = ProximaDBError("boom")
    assert e.message == "boom"
    assert e.error_code is None
    assert e.details == {}
    assert e.request_id is None
    assert e.retryable is False
    assert str(e) == "boom"
    assert isinstance(e, Exception)


def test_base_full_str_formatting():
    e = ProximaDBError(
        "boom",
        error_code="CODE",
        details={"k": "v"},
        request_id="req-1",
        retryable=True,
    )
    assert e.details == {"k": "v"}
    assert e.retryable is True
    s = str(e)
    assert "boom" in s
    assert "Error Code: CODE" in s
    assert "Request ID: req-1" in s
    assert s.count("|") == 2


def test_base_str_only_error_code():
    e = ProximaDBError("boom", error_code="CODE")
    assert str(e) == "boom | Error Code: CODE"


def test_base_str_only_request_id():
    e = ProximaDBError("boom", request_id="req-9")
    assert str(e) == "boom | Request ID: req-9"


def test_can_raise_and_catch_as_base():
    with pytest.raises(ProximaDBError):
        raise ValidationError("bad field")


# --------------------------------------------------------------------------
# Each concrete exception type
# --------------------------------------------------------------------------
def test_authentication_error_defaults():
    e = AuthenticationError()
    assert e.message == "Authentication failed"
    assert e.error_code == "AUTH_FAILED"


def test_authentication_error_custom():
    e = AuthenticationError("nope", request_id="r")
    assert e.message == "nope"
    assert e.request_id == "r"


def test_authorization_error():
    e = AuthorizationError()
    assert e.error_code == "AUTH_INSUFFICIENT"
    assert e.message == "Insufficient permissions"


def test_collection_not_found_error():
    e = CollectionNotFoundError("col-1", request_id="r")
    assert e.collection_id == "col-1"
    assert e.error_code == "COLLECTION_NOT_FOUND"
    assert "col-1" in e.message
    assert e.request_id == "r"


def test_collection_exists_error():
    e = CollectionExistsError("mycol")
    assert e.collection_name == "mycol"
    assert e.error_code == "COLLECTION_EXISTS"
    assert "mycol" in e.message


def test_vector_not_found_error():
    e = VectorNotFoundError("v-7")
    assert e.vector_id == "v-7"
    assert e.error_code == "VECTOR_NOT_FOUND"
    assert "v-7" in e.message


def test_vector_dimension_error():
    e = VectorDimensionError(128, 256)
    assert e.expected_dimension == 128
    assert e.actual_dimension == 256
    assert e.error_code == "DIMENSION_MISMATCH"
    assert "128" in e.message and "256" in e.message


def test_invalid_vector_error():
    e = InvalidVectorError()
    assert e.error_code == "INVALID_VECTOR"
    assert e.message == "Invalid vector data"
    e2 = InvalidVectorError("custom")
    assert e2.message == "custom"


def test_rate_limit_error():
    e = RateLimitError(retry_after=30)
    assert e.retry_after == 30
    assert e.error_code == "RATE_LIMIT_EXCEEDED"
    assert e.message == "Rate limit exceeded"
    assert RateLimitError().retry_after is None


def test_quota_exceeded_error():
    e = QuotaExceededError(quota_type="vectors")
    assert e.quota_type == "vectors"
    assert e.error_code == "QUOTA_EXCEEDED"
    assert e.message == "Usage quota exceeded"


def test_validation_error():
    e = ValidationError("bad", field="name")
    assert e.field == "name"
    assert e.error_code == "VALIDATION_ERROR"
    assert e.message == "bad"
    assert ValidationError("x").field is None


def test_server_error():
    e = ServerError(status_code=503)
    assert e.status_code == 503
    assert e.error_code == "SERVER_ERROR"
    assert e.message == "Internal server error"


def test_network_error():
    orig = ValueError("cause")
    e = NetworkError("net down", original_error=orig)
    assert e.original_error is orig
    assert e.error_code == "NETWORK_ERROR"
    assert e.message == "net down"
    assert NetworkError().message == "Network error"


def test_transport_error():
    orig = RuntimeError("x")
    e = TransportError("t", transport_type="grpc", original_error=orig)
    assert e.transport_type == "grpc"
    assert e.original_error is orig
    assert e.error_code == "TRANSPORT_ERROR"
    assert TransportError().message == "Transport error"


def test_timeout_error():
    e = TimeoutError(timeout_seconds=5.0)
    assert e.timeout_seconds == 5.0
    assert e.error_code == "TIMEOUT"
    assert e.message == "Request timeout"


def test_configuration_error():
    e = ConfigurationError("bad config")
    assert e.error_code == "CONFIG_ERROR"
    assert e.message == "bad config"


def test_index_error():
    e = IndexError("idx fail", index_type="hnsw")
    assert e.index_type == "hnsw"
    assert e.error_code == "INDEX_ERROR"
    assert IndexError("x").index_type is None


def test_batch_error():
    errs = [{"id": 1}]
    e = BatchError("partial", successful_count=3, failed_count=2, errors=errs)
    assert e.successful_count == 3
    assert e.failed_count == 2
    assert e.errors == errs
    assert e.error_code == "BATCH_ERROR"
    assert BatchError("x").errors == []


def test_wal_error():
    e = WALError()
    assert e.error_code == "WAL_ERROR"
    assert e.message == "WAL operation failed"
    assert WALError("custom").message == "custom"


def test_streaming_error():
    e = StreamingError("stream broke")
    assert e.error_code == "STREAMING_ERROR"
    assert e.message == "stream broke"


# --------------------------------------------------------------------------
# map_http_error — nested canonical envelope
# --------------------------------------------------------------------------
def test_map_http_nested_validation_400():
    data = {"error": {"type": "validation_error", "message": "bad", "request_id": "r1"}}
    e = map_http_error(400, data)
    assert isinstance(e, ValidationError)
    assert e.message == "bad"
    assert e.request_id == "r1"


def test_map_http_nested_dimension_mismatch_400():
    data = {
        "error": {
            "type": "dimension_mismatch",
            "message": "dim",
            "details": {"expected_dimension": 4, "actual_dimension": 8},
        }
    }
    e = map_http_error(400, data)
    assert isinstance(e, VectorDimensionError)
    assert e.expected_dimension == 4
    assert e.actual_dimension == 8


def test_map_http_dimension_mismatch_missing_details_falls_through_400():
    # expected/actual missing -> falls through to generic ProximaDBError
    data = {"error": {"type": "dimension_mismatch", "message": "dim", "details": {}}}
    e = map_http_error(400, data)
    assert type(e) is ProximaDBError
    assert e.error_code == "DIMENSION_MISMATCH"


def test_map_http_generic_400():
    data = {"error": {"type": "other", "message": "m"}}
    e = map_http_error(400, data)
    assert type(e) is ProximaDBError
    assert e.error_code == "OTHER"
    assert e.retryable is False


def test_map_http_401():
    e = map_http_error(401, {"error": {"type": "auth_failed", "message": "no"}})
    assert isinstance(e, AuthenticationError)
    assert e.message == "no"


def test_map_http_403():
    e = map_http_error(403, {"error": {"type": "forbidden", "message": "no"}})
    assert isinstance(e, AuthorizationError)


def test_map_http_404_collection():
    data = {
        "error": {
            "type": "collection_not_found",
            "message": "m",
            "details": {"collection_id": "c9"},
        }
    }
    e = map_http_error(404, data)
    assert isinstance(e, CollectionNotFoundError)
    assert e.collection_id == "c9"


def test_map_http_404_collection_default_id():
    data = {"error": {"type": "collection_not_found", "message": "m"}}
    e = map_http_error(404, data)
    assert isinstance(e, CollectionNotFoundError)
    assert e.collection_id == "unknown"


def test_map_http_404_vector():
    data = {
        "error": {
            "type": "vector_not_found",
            "message": "m",
            "details": {"vector_id": "v3"},
        }
    }
    e = map_http_error(404, data)
    assert isinstance(e, VectorNotFoundError)
    assert e.vector_id == "v3"


def test_map_http_404_generic():
    e = map_http_error(404, {"error": {"type": "other", "message": "m"}})
    assert type(e) is ProximaDBError
    assert e.error_code == "OTHER"


def test_map_http_409_exists():
    data = {
        "error": {
            "type": "collection_exists",
            "message": "m",
            "details": {"collection_name": "cn"},
        }
    }
    e = map_http_error(409, data)
    assert isinstance(e, CollectionExistsError)
    assert e.collection_name == "cn"


def test_map_http_409_generic():
    e = map_http_error(409, {"error": {"type": "conflict", "message": "m"}})
    assert type(e) is ProximaDBError


def test_map_http_429():
    data = {"error": {"type": "rate", "message": "m", "details": {"retry_after": 12}}}
    e = map_http_error(429, data)
    assert isinstance(e, RateLimitError)
    assert e.retry_after == 12


def test_map_http_413():
    data = {"error": {"type": "quota", "message": "m", "details": {"quota_type": "vec"}}}
    e = map_http_error(413, data)
    assert isinstance(e, QuotaExceededError)
    assert e.quota_type == "vec"


def test_map_http_500():
    e = map_http_error(500, {"error": {"type": "server", "message": "m"}})
    assert isinstance(e, ServerError)
    assert e.status_code == 500


def test_map_http_503():
    e = map_http_error(503, {"error": {"type": "server", "message": "m"}})
    assert isinstance(e, ServerError)
    assert e.status_code == 503


def test_map_http_unknown_status():
    e = map_http_error(418, {"error": {"type": "teapot", "message": "m"}})
    assert type(e) is ProximaDBError
    assert e.error_code == "TEAPOT"


# --------------------------------------------------------------------------
# map_http_error — legacy flat shape + bare string + defaults
# --------------------------------------------------------------------------
def test_map_http_legacy_flat():
    data = {
        "error_code": "validation_error",
        "message": "flat",
        "request_id": "rf",
        "details": {"x": 1},
    }
    e = map_http_error(400, data)
    assert isinstance(e, ValidationError)
    assert e.message == "flat"
    assert e.request_id == "rf"


def test_map_http_bare_string_error():
    data = {"error": "something_bad", "message": "m"}
    e = map_http_error(400, data)
    assert type(e) is ProximaDBError
    assert e.error_code == "SOMETHING_BAD"


def test_map_http_empty_uses_defaults_and_message():
    e = map_http_error(400, {})
    assert type(e) is ProximaDBError
    assert e.error_code == "UNKNOWN"
    assert e.message == "HTTP 400 error"


def test_map_http_nested_default_message():
    e = map_http_error(404, {"error": {"type": "x"}})
    assert e.message == "HTTP 404 error"


def test_map_http_request_id_from_headers():
    data = {"error": {"type": "validation_error", "message": "m"}}
    e = map_http_error(400, data, headers={"x-request-id": "hdr-1"})
    assert e.request_id == "hdr-1"


def test_map_http_request_id_from_headers_capitalized():
    data = {"error": {"type": "validation_error", "message": "m"}}
    e = map_http_error(400, data, headers={"X-Request-ID": "hdr-2"})
    assert e.request_id == "hdr-2"


def test_map_http_request_id_prefers_body_over_header():
    data = {"error": {"type": "validation_error", "message": "m", "request_id": "body"}}
    e = map_http_error(400, data, headers={"x-request-id": "hdr"})
    assert e.request_id == "body"


def test_map_http_request_id_body_top_level_fallback():
    # nested envelope without request_id falls back to top-level request_id
    data = {"error": {"type": "x", "message": "m"}, "request_id": "top"}
    e = map_http_error(404, data)
    assert e.request_id == "top"


# --------------------------------------------------------------------------
# map_grpc_error — real grpc available, fake error object per status code
# --------------------------------------------------------------------------
class _FakeGrpcError:
    def __init__(self, code, details="grpc msg"):
        self._code = code
        self._details = details

    def code(self):
        return self._code

    def details(self):
        return self._details

    def __str__(self):
        return f"FakeGrpcError({self._details})"


@pytest.mark.parametrize(
    "status_name,exc_type,error_code",
    [
        ("UNAUTHENTICATED", AuthenticationError, "AUTH_FAILED"),
        ("PERMISSION_DENIED", AuthorizationError, "AUTH_INSUFFICIENT"),
        ("INVALID_ARGUMENT", ValidationError, "VALIDATION_ERROR"),
        ("RESOURCE_EXHAUSTED", RateLimitError, "RATE_LIMIT_EXCEEDED"),
        ("DEADLINE_EXCEEDED", TimeoutError, "TIMEOUT"),
        ("UNAVAILABLE", NetworkError, "NETWORK_ERROR"),
        ("INTERNAL", ServerError, "SERVER_ERROR"),
    ],
)
def test_map_grpc_typed(status_name, exc_type, error_code):
    import grpc

    err = _FakeGrpcError(getattr(grpc.StatusCode, status_name))
    e = map_grpc_error(err)
    assert isinstance(e, exc_type)
    assert e.error_code == error_code


def test_map_grpc_not_found():
    import grpc

    e = map_grpc_error(_FakeGrpcError(grpc.StatusCode.NOT_FOUND))
    assert type(e) is ProximaDBError
    assert e.error_code == "NOT_FOUND"


def test_map_grpc_already_exists():
    import grpc

    e = map_grpc_error(_FakeGrpcError(grpc.StatusCode.ALREADY_EXISTS))
    assert type(e) is ProximaDBError
    assert e.error_code == "ALREADY_EXISTS"


def test_map_grpc_unavailable_keeps_original():
    import grpc

    err = _FakeGrpcError(grpc.StatusCode.UNAVAILABLE)
    e = map_grpc_error(err)
    assert isinstance(e, NetworkError)
    assert e.original_error is err


def test_map_grpc_fallback_other_code():
    import grpc

    err = _FakeGrpcError(grpc.StatusCode.CANCELLED)
    e = map_grpc_error(err)
    assert type(e) is ProximaDBError
    assert e.error_code == "CANCELLED"
    assert "grpc_error" in e.details


def test_map_grpc_import_error_fallback(monkeypatch):
    # Simulate grpc import failing inside map_grpc_error.
    real_grpc = sys.modules.get("grpc")
    sys.modules["grpc"] = None  # forces ImportError on `import grpc`
    try:
        e = map_grpc_error(_FakeGrpcError(None, details="boom"))
        assert type(e) is ProximaDBError
        assert e.error_code == "GRPC_ERROR"
        assert "boom" in str(e)
    finally:
        if real_grpc is not None:
            sys.modules["grpc"] = real_grpc
        else:
            sys.modules.pop("grpc", None)


def test_map_grpc_runtime_error_path_triggers_except(monkeypatch):
    # A grpc_error whose .code() raises AttributeError isn't caught (only
    # ImportError is), so confirm typed mapping still works with a proper code.
    import grpc

    e = map_grpc_error(_FakeGrpcError(grpc.StatusCode.INTERNAL, details="srv"))
    assert isinstance(e, ServerError)
    assert e.message == "srv"
