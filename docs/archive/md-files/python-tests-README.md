# ProximaDB Python Client Tests

This directory contains all tests for the ProximaDB Python client SDK.

## Test Organization

### `/sdk/` - SDK Tests
Tests that specifically validate the Python SDK functionality:
- `test_rest_client.py` - REST client implementation tests
- `test_grpc_collections_comprehensive.py` - gRPC collections SDK tests

### `/integration/` - Integration Tests (23 tests)
End-to-end tests that validate client-server integration:
- `test_vector_coordinator_integration.py` - Vector coordinator integration
- `test_collection_service_e2e.py` - Collection service end-to-end tests
- `test_comprehensive_search.py` - Search functionality integration
- `test_bert_collection_persistence.py` - BERT embeddings persistence
- `test_bert_embeddings.py` - BERT embeddings testing
- `test_bulk_insert_zero_copy.py` - Bulk insert performance tests
- `test_grpc_large_scale_operations.py` - Large scale operations
- `test_grpc_persistence_integrity.py` - Persistence integrity
- `test_grpc_vectors_comprehensive.py` - Vector operations comprehensive
- `test_collection_ops.py` - Collection operations testing
- `test_metadata_unified.py` - Unified metadata testing
- `test_working_operations.py` - Working operations validation

### `/grpc/` - gRPC Protocol Tests (6 tests)
Focused tests for gRPC client functionality:
- `test_end_to_end_grpc.py` - Complete gRPC workflow tests
- `test_new_grpc_api_bert.py` - BERT API via gRPC
- `test_new_grpc_client.py` - New gRPC client implementation
- `test_real_grpc_connection.py` - Real server connection tests
- `test_simple_grpc.py` - Basic gRPC functionality
- `test_verify_grpc_connection.py` - Connection verification

### `/persistence/` - Persistence Tests (9 tests)
Data persistence and recovery validation:
- `test_filestore_persistence.py` - Filestore backend persistence
- `test_filestore_realtime.py` - Real-time persistence testing
- `test_recovery_and_search.py` - Recovery after restart
- `test_vector_persistence.py` - Vector data persistence
- `test_debug_persistence.py` - Persistence debugging
- `test_persistence_step_by_step.py` - Step-by-step persistence
- `test_collection_persistence_only.py` - Collection-only persistence

### `/wal/` - Write-Ahead Log Tests (5 tests)
WAL system functionality validation:
- `test_wal_direct.py` - Direct WAL operations
- `test_wal_comprehensive.py` - Comprehensive WAL testing
- `test_wal_persistence_comprehensive.py` - WAL persistence validation
- `test_wal_cleanup.py` - WAL cleanup operations

### `/debug/` - Debug Tests (1 test)
Debugging and troubleshooting utilities:
- `test_debug_search.py` - Search debugging utilities

### `/unit/` - Unit Tests (3 tests)
Focused unit tests for individual components:
- `test_atomic_operations.py` - Atomic operation unit tests
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