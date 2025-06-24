# ProximaDB Error Handling Guide

This guide covers error handling patterns and best practices for both REST and gRPC APIs.

## Error Response Formats

### REST API Errors

All REST API errors follow a consistent JSON format:

```json
{
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable error message",
    "details": {
      "field": "additional context",
      "suggestion": "how to fix the error"
    },
    "request_id": "req_1234567890abcdef"
  }
}
```

### gRPC API Errors

gRPC errors use standard status codes with details:

```
Status: INVALID_ARGUMENT
Message: Invalid vector dimension
Details: Expected 128 dimensions, got 64
```

## HTTP Status Codes

| Code | Name | Description | When Used |
|------|------|-------------|-----------|
| 200 | OK | Success | Successful requests |
| 201 | Created | Resource created | Collection/vector created |
| 400 | Bad Request | Invalid request | Malformed data, wrong dimensions |
| 401 | Unauthorized | Authentication failed | Missing/invalid API key |
| 403 | Forbidden | Access denied | Insufficient permissions |
| 404 | Not Found | Resource not found | Collection/vector doesn't exist |
| 409 | Conflict | Resource conflict | Collection already exists |
| 422 | Unprocessable Entity | Validation failed | Invalid vector format |
| 429 | Too Many Requests | Rate limited | Exceeded rate limits |
| 500 | Internal Server Error | Server error | Database/system errors |
| 503 | Service Unavailable | Service down | Maintenance mode |

## gRPC Status Codes

| Code | Description | When Used |
|------|-------------|-----------|
| `OK` | Success | Successful operations |
| `INVALID_ARGUMENT` | Invalid request parameters | Bad input data |
| `NOT_FOUND` | Resource not found | Missing collection/vector |
| `ALREADY_EXISTS` | Resource exists | Duplicate collection ID |
| `PERMISSION_DENIED` | Access denied | Insufficient permissions |
| `RESOURCE_EXHAUSTED` | Rate limited | Too many requests |
| `INTERNAL` | Internal error | System/database errors |
| `UNAVAILABLE` | Service unavailable | Server overloaded |
| `UNAUTHENTICATED` | Authentication failed | Invalid API key |

## Error Categories

### 1. Authentication Errors

**Invalid API Key:**
```json
{
  "error": {
    "code": "INVALID_API_KEY",
    "message": "The provided API key is invalid",
    "details": {
      "hint": "Check your API key format and permissions"
    }
  }
}
```

**Missing Authentication:**
```json
{
  "error": {
    "code": "MISSING_AUTHENTICATION", 
    "message": "Authorization header is required",
    "details": {
      "required_header": "Authorization: Bearer your-api-key"
    }
  }
}
```

### 2. Collection Errors

**Collection Not Found:**
```json
{
  "error": {
    "code": "COLLECTION_NOT_FOUND",
    "message": "Collection 'embeddings' does not exist",
    "details": {
      "collection_id": "embeddings",
      "suggestion": "Create the collection first or check the collection name"
    }
  }
}
```

**Collection Already Exists:**
```json
{
  "error": {
    "code": "COLLECTION_ALREADY_EXISTS",
    "message": "Collection 'embeddings' already exists",
    "details": {
      "collection_id": "embeddings",
      "suggestion": "Use a different collection ID or delete the existing collection"
    }
  }
}
```

**Invalid Collection Configuration:**
```json
{
  "error": {
    "code": "INVALID_COLLECTION_CONFIG",
    "message": "Invalid collection configuration",
    "details": {
      "invalid_fields": ["dimension"],
      "dimension_error": "Dimension must be between 1 and 2048"
    }
  }
}
```

### 3. Vector Errors

**Vector Not Found:**
```json
{
  "error": {
    "code": "VECTOR_NOT_FOUND",
    "message": "Vector 'vec_123' not found in collection 'embeddings'",
    "details": {
      "vector_id": "vec_123",
      "collection_id": "embeddings"
    }
  }
}
```

**Invalid Vector Dimension:**
```json
{
  "error": {
    "code": "INVALID_VECTOR_DIMENSION",
    "message": "Vector dimension mismatch",
    "details": {
      "expected_dimension": 128,
      "provided_dimension": 64,
      "collection_id": "embeddings"
    }
  }
}
```

**Invalid Vector Format:**
```json
{
  "error": {
    "code": "INVALID_VECTOR_FORMAT",
    "message": "Vector contains invalid values",
    "details": {
      "invalid_values": ["NaN", "Infinity"],
      "valid_range": "finite floating point numbers"
    }
  }
}
```

### 4. Search Errors

**Invalid Search Parameters:**
```json
{
  "error": {
    "code": "INVALID_SEARCH_PARAMS",
    "message": "Invalid search parameters",
    "details": {
      "k_error": "k must be between 1 and 1000",
      "provided_k": 5000
    }
  }
}
```

**Search Timeout:**
```json
{
  "error": {
    "code": "SEARCH_TIMEOUT",
    "message": "Search operation timed out",
    "details": {
      "timeout_seconds": 30,
      "suggestion": "Try reducing k or simplifying filters"
    }
  }
}
```

### 5. Rate Limiting Errors

**Rate Limit Exceeded:**
```json
{
  "error": {
    "code": "RATE_LIMITED",
    "message": "Too many requests",
    "details": {
      "limit": "100 requests per minute",
      "retry_after_seconds": 60,
      "current_usage": "105/100"
    }
  }
}
```

### 6. Storage Errors

**Storage Full:**
```json
{
  "error": {
    "code": "STORAGE_FULL",
    "message": "Storage capacity exceeded",
    "details": {
      "used_bytes": 10737418240,
      "max_bytes": 10737418240,
      "suggestion": "Delete unused collections or contact administrator"
    }
  }
}
```

**Corruption Error:**
```json
{
  "error": {
    "code": "DATA_CORRUPTION",
    "message": "Data corruption detected",
    "details": {
      "affected_collection": "embeddings",
      "suggestion": "Contact support for data recovery"
    }
  }
}
```

## Client Error Handling

### REST API - Python

```python
import requests
import json
from typing import Dict, Any

class ProximaDBError(Exception):
    def __init__(self, code: str, message: str, details: Dict[str, Any] = None):
        self.code = code
        self.message = message
        self.details = details or {}
        super().__init__(f"{code}: {message}")

def handle_response(response: requests.Response) -> Dict[str, Any]:
    if response.status_code == 200:
        return response.json()
    
    try:
        error_data = response.json()["error"]
    except (json.JSONDecodeError, KeyError):
        error_data = {
            "code": f"HTTP_{response.status_code}",
            "message": response.text or "Unknown error"
        }
    
    # Handle specific error types
    if response.status_code == 401:
        raise ProximaDBError("UNAUTHORIZED", "Check your API key")
    elif response.status_code == 404:
        raise ProximaDBError(
            error_data["code"], 
            error_data["message"],
            error_data.get("details")
        )
    elif response.status_code == 429:
        retry_after = error_data.get("details", {}).get("retry_after_seconds", 60)
        raise ProximaDBError(
            "RATE_LIMITED", 
            f"Rate limited. Retry after {retry_after} seconds",
            {"retry_after": retry_after}
        )
    else:
        raise ProximaDBError(
            error_data["code"],
            error_data["message"], 
            error_data.get("details")
        )

# Usage example
try:
    response = requests.post("http://localhost:5678/collections", 
                           headers=headers, json=data)
    result = handle_response(response)
except ProximaDBError as e:
    if e.code == "COLLECTION_ALREADY_EXISTS":
        print("Collection exists, continuing...")
    elif e.code == "RATE_LIMITED":
        time.sleep(e.details.get("retry_after", 60))
        # Retry the request
    else:
        print(f"Unexpected error: {e}")
        raise
```

### REST API - JavaScript

```javascript
class ProximaDBError extends Error {
  constructor(code, message, details = {}) {
    super(`${code}: ${message}`);
    this.code = code;
    this.details = details;
  }
}

async function handleResponse(response) {
  if (response.ok) {
    return await response.json();
  }

  let errorData;
  try {
    const body = await response.json();
    errorData = body.error;
  } catch {
    errorData = {
      code: `HTTP_${response.status}`,
      message: response.statusText || 'Unknown error'
    };
  }

  // Handle specific errors
  switch (response.status) {
    case 401:
      throw new ProximaDBError('UNAUTHORIZED', 'Check your API key');
    case 404:
      throw new ProximaDBError(errorData.code, errorData.message, errorData.details);
    case 429:
      const retryAfter = errorData.details?.retry_after_seconds || 60;
      throw new ProximaDBError('RATE_LIMITED', 
        `Rate limited. Retry after ${retryAfter} seconds`,
        { retryAfter });
    default:
      throw new ProximaDBError(errorData.code, errorData.message, errorData.details);
  }
}

// Usage with retry logic
async function createCollectionWithRetry(collectionData, maxRetries = 3) {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    try {
      const response = await fetch('http://localhost:5678/collections', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer your-key' },
        body: JSON.stringify(collectionData)
      });
      
      return await handleResponse(response);
    } catch (error) {
      if (error.code === 'RATE_LIMITED' && attempt < maxRetries - 1) {
        await new Promise(resolve => 
          setTimeout(resolve, (error.details.retryAfter || 60) * 1000));
        continue;
      }
      throw error;
    }
  }
}
```

### gRPC API - Python

```python
import grpc
from typing import Any

class ProximaDBGrpcError(Exception):
    def __init__(self, status_code: grpc.StatusCode, details: str):
        self.status_code = status_code
        self.details = details
        super().__init__(f"{status_code.name}: {details}")

def handle_grpc_error(rpc_error: grpc.RpcError) -> None:
    """Convert gRPC errors to ProximaDB errors"""
    status_code = rpc_error.code()
    details = rpc_error.details()
    
    # Map gRPC status codes to user-friendly errors
    error_map = {
        grpc.StatusCode.INVALID_ARGUMENT: "Invalid request parameters",
        grpc.StatusCode.NOT_FOUND: "Resource not found", 
        grpc.StatusCode.ALREADY_EXISTS: "Resource already exists",
        grpc.StatusCode.UNAUTHENTICATED: "Authentication failed",
        grpc.StatusCode.PERMISSION_DENIED: "Access denied",
        grpc.StatusCode.RESOURCE_EXHAUSTED: "Rate limited",
        grpc.StatusCode.INTERNAL: "Internal server error",
        grpc.StatusCode.UNAVAILABLE: "Service unavailable"
    }
    
    user_message = error_map.get(status_code, "Unknown error")
    raise ProximaDBGrpcError(status_code, f"{user_message}: {details}")

# Usage example
try:
    response = stub.CreateCollection(request, metadata=metadata)
except grpc.RpcError as e:
    try:
        handle_grpc_error(e)
    except ProximaDBGrpcError as pe:
        if pe.status_code == grpc.StatusCode.ALREADY_EXISTS:
            print("Collection already exists")
        elif pe.status_code == grpc.StatusCode.RESOURCE_EXHAUSTED:
            time.sleep(60)  # Wait before retry
        else:
            print(f"Error: {pe}")
            raise
```

### gRPC API - Go

```go
package main

import (
    "context"
    "fmt"
    "time"
    
    "google.golang.org/grpc/codes"
    "google.golang.org/grpc/status"
)

type ProximaDBError struct {
    Code    codes.Code
    Message string
}

func (e *ProximaDBError) Error() string {
    return fmt.Sprintf("%s: %s", e.Code, e.Message)
}

func handleGrpcError(err error) error {
    if st, ok := status.FromError(err); ok {
        switch st.Code() {
        case codes.InvalidArgument:
            return &ProximaDBError{codes.InvalidArgument, "Invalid request parameters"}
        case codes.NotFound:
            return &ProximaDBError{codes.NotFound, "Resource not found"}
        case codes.AlreadyExists:
            return &ProximaDBError{codes.AlreadyExists, "Resource already exists"}
        case codes.Unauthenticated:
            return &ProximaDBError{codes.Unauthenticated, "Authentication failed"}
        case codes.ResourceExhausted:
            return &ProximaDBError{codes.ResourceExhausted, "Rate limited"}
        default:
            return &ProximaDBError{st.Code(), st.Message()}
        }
    }
    return err
}

// Usage with retry
func createCollectionWithRetry(client VectorDBClient, req *CreateCollectionRequest) error {
    maxRetries := 3
    for attempt := 0; attempt < maxRetries; attempt++ {
        _, err := client.CreateCollection(context.Background(), req)
        if err == nil {
            return nil
        }
        
        if proxErr := handleGrpcError(err); proxErr != nil {
            if proxErr.(*ProximaDBError).Code == codes.ResourceExhausted && attempt < maxRetries-1 {
                time.Sleep(60 * time.Second)
                continue
            }
            return proxErr
        }
    }
    return fmt.Errorf("max retries exceeded")
}
```

## Best Practices

### 1. Retry Logic

Implement exponential backoff for transient errors:

```python
import time
import random

def exponential_backoff_retry(func, max_retries=3, base_delay=1):
    for attempt in range(max_retries):
        try:
            return func()
        except ProximaDBError as e:
            if e.code in ['RATE_LIMITED', 'SERVICE_UNAVAILABLE'] and attempt < max_retries - 1:
                delay = base_delay * (2 ** attempt) + random.uniform(0, 1)
                time.sleep(delay)
                continue
            raise
```

### 2. Error Classification

Classify errors by recoverability:

```python
RETRYABLE_ERRORS = {
    'RATE_LIMITED',
    'SERVICE_UNAVAILABLE', 
    'TIMEOUT',
    'INTERNAL_SERVER_ERROR'
}

PERMANENT_ERRORS = {
    'INVALID_API_KEY',
    'COLLECTION_NOT_FOUND',
    'INVALID_VECTOR_DIMENSION'
}

def is_retryable(error_code: str) -> bool:
    return error_code in RETRYABLE_ERRORS
```

### 3. Circuit Breaker

Implement circuit breaker pattern for failing services:

```python
from enum import Enum
import time

class CircuitState(Enum):
    CLOSED = "closed"
    OPEN = "open" 
    HALF_OPEN = "half_open"

class CircuitBreaker:
    def __init__(self, failure_threshold=5, timeout=60):
        self.failure_threshold = failure_threshold
        self.timeout = timeout
        self.failure_count = 0
        self.last_failure_time = None
        self.state = CircuitState.CLOSED
    
    def call(self, func):
        if self.state == CircuitState.OPEN:
            if time.time() - self.last_failure_time > self.timeout:
                self.state = CircuitState.HALF_OPEN
            else:
                raise ProximaDBError("CIRCUIT_OPEN", "Circuit breaker is open")
        
        try:
            result = func()
            if self.state == CircuitState.HALF_OPEN:
                self.state = CircuitState.CLOSED
                self.failure_count = 0
            return result
        except Exception as e:
            self.failure_count += 1
            self.last_failure_time = time.time()
            
            if self.failure_count >= self.failure_threshold:
                self.state = CircuitState.OPEN
            raise
```

### 4. Logging and Monitoring

Log errors with context for debugging:

```python
import logging

logger = logging.getLogger(__name__)

def log_error(error: ProximaDBError, context: dict = None):
    logger.error(
        "ProximaDB operation failed",
        extra={
            "error_code": error.code,
            "error_message": error.message,
            "error_details": error.details,
            "context": context or {}
        }
    )

# Usage
try:
    client.create_collection("test", 128)
except ProximaDBError as e:
    log_error(e, {
        "operation": "create_collection", 
        "collection_id": "test",
        "user_id": "user_123"
    })
    raise
```

## Testing Error Scenarios

### Unit Tests

```python
import pytest
from unittest.mock import Mock, patch

def test_collection_not_found_error():
    mock_response = Mock()
    mock_response.status_code = 404
    mock_response.json.return_value = {
        "error": {
            "code": "COLLECTION_NOT_FOUND",
            "message": "Collection 'test' not found"
        }
    }
    
    with patch('requests.get', return_value=mock_response):
        with pytest.raises(ProximaDBError) as exc_info:
            client.get_collection("test")
        
        assert exc_info.value.code == "COLLECTION_NOT_FOUND"

def test_rate_limiting_retry():
    # First call fails with rate limit
    mock_response_1 = Mock()
    mock_response_1.status_code = 429
    mock_response_1.json.return_value = {
        "error": {
            "code": "RATE_LIMITED",
            "details": {"retry_after_seconds": 1}
        }
    }
    
    # Second call succeeds
    mock_response_2 = Mock()
    mock_response_2.status_code = 200
    mock_response_2.json.return_value = {"success": True}
    
    with patch('requests.post', side_effect=[mock_response_1, mock_response_2]):
        with patch('time.sleep'):
            result = client_with_retry.create_collection("test", 128)
            assert result["success"] is True
```

### Integration Tests

```python
def test_error_handling_integration():
    # Test with real server
    client = ProximaDBClient(api_key="invalid_key")
    
    with pytest.raises(ProximaDBError) as exc_info:
        client.list_collections()
    
    assert exc_info.value.code == "UNAUTHORIZED"
```

This comprehensive error handling guide provides the foundation for robust client applications that can gracefully handle all types of errors from ProximaDB APIs.