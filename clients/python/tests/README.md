# ProximaDB Python Client Tests

This directory contains all tests for the ProximaDB Python client SDK.

## Test Organization

### `/sdk/` - SDK Tests
Tests that specifically validate the Python SDK functionality:
- `test_rest_client.py` - REST client implementation tests
- `test_grpc_collections_comprehensive.py` - gRPC collections SDK tests

### `/integration/` - Integration Tests
End-to-end tests that validate client-server integration:
- `test_vector_coordinator_integration.py` - Vector coordinator integration
- `test_collection_service_e2e.py` - Collection service end-to-end tests
- `test_comprehensive_search.py` - Search functionality integration
- `test_bert_collection_persistence.py` - BERT embeddings persistence
- `test_grpc_large_scale_operations.py` - Large scale operations
- `test_grpc_persistence_integrity.py` - Persistence integrity
- `test_grpc_vectors_comprehensive.py` - Vector operations comprehensive
- `test_persistence_comprehensive.py` - Comprehensive persistence tests

### `/unit/` - Unit Tests
Focused unit tests for individual components:
- `test_connection_quick.py` - Connection unit tests
- `test_health_check.py` - Health check unit tests

## Running Tests

### Run all tests:
```bash
cd /home/vsingh/code/proximadb/clients/python
python -m pytest tests/
```

### Run specific test categories:
```bash
# SDK tests only
python -m pytest tests/sdk/

# Integration tests only  
python -m pytest tests/integration/

# Unit tests only
python -m pytest tests/unit/
```

### Run specific test files:
```bash
# Test vector coordinator integration
python tests/integration/test_vector_coordinator_integration.py

# Test REST client SDK
python tests/sdk/test_rest_client.py
```

## Test Development Guidelines

1. **SDK Tests** - Focus on client library functionality, mocking server responses
2. **Integration Tests** - Require running ProximaDB server, test full workflows
3. **Unit Tests** - Fast, isolated tests for individual functions/classes

## Prerequisites

- ProximaDB server running (for integration tests)
- Python dependencies installed: `pip install -r requirements.txt`
- ProximaDB Python SDK installed: `pip install -e .`