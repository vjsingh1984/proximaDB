# ProximaDB Python SDK Test Suite

This directory contains the comprehensive test suite for the ProximaDB Python SDK, organized by functional components and covering all major features.

## Test Organization

The test suite is organized into the following modules:

### Core Test Modules

1. **`test_client_sdk.py`** - Client creation, configuration, and SDK features
   - Client creation with auto-detection and explicit protocols
   - Configuration handling and validation
   - Health checks and metrics collection
   - Error handling and exception management
   - Async support and context managers
   - Protocol interoperability testing

2. **`test_collection_operations.py`** - Collection CRUD operations and configuration
   - Collection lifecycle (create, read, update, delete)
   - Advanced configuration options (index, storage, flush configs)
   - Distance metrics and compression types
   - Cross-protocol collection operations
   - Configuration validation and persistence

3. **`test_vector_operations.py`** - Vector CRUD operations and batch processing
   - Single vector operations (insert, get, update, delete)
   - Batch vector operations and large-scale insertions
   - Cross-protocol vector operations
   - UUID-based operations for improved performance
   - Vector validation and error handling

4. **`test_search_operations.py`** - Search functionality and similarity operations
   - ID-based search and retrieval
   - Metadata filtering (client-side and server-side)
   - Proximity/similarity search with BERT embeddings
   - Document-to-document similarity
   - Cross-protocol search operations
   - Search edge cases and performance testing

5. **`test_persistence_wal.py`** - Data persistence and WAL operations
   - Collection and vector persistence across operations
   - Write-Ahead Log (WAL) operations and durability
   - Cross-protocol persistence verification
   - Concurrent WAL operations
   - Recovery mechanisms and consistency checks

6. **`test_storage_layouts.py`** - Storage engine testing with flush and compaction
   - VIPER storage layout with flush operations
   - LSM storage layout with compaction triggers
   - Large-scale data operations (20MB+ datasets)
   - Storage layout performance comparison
   - Unified search across WAL, flushed, and compacted data

## Test Configuration

### Prerequisites

Before running tests, ensure:

1. **ProximaDB Server Running**: The test suite requires a running ProximaDB server
   ```bash
   # Start ProximaDB server
   cargo run --bin proximadb-server
   ```

2. **Python Dependencies**: Install test dependencies
   ```bash
   cd clients/python
   pip install -r requirements.txt
   pip install pytest pytest-cov sentence-transformers
   ```

### Running Tests

#### Basic Test Execution

```bash
# Run all tests
pytest tests/

# Run specific test module
pytest tests/test_collection_operations.py

# Run with verbose output
pytest tests/ -v

# Run with coverage reporting
pytest tests/ --cov=proximadb --cov-report=html
```

#### Test Filtering

```bash
# Run only fast tests (skip large data operations)
pytest tests/ --fast

# Run specific test markers
pytest tests/ -m "not slow"           # Skip slow tests
pytest tests/ -m "search"             # Only search tests  
pytest tests/ -m "storage"            # Only storage tests
pytest tests/ -m "integration"        # Only integration tests

# Run specific test patterns
pytest tests/ -k "collection"         # Tests with 'collection' in name
pytest tests/ -k "search and metadata" # Tests matching both terms
```

#### Custom Configuration

```bash
# Use custom endpoints
pytest tests/ --rest-endpoint=http://localhost:8678 --grpc-endpoint=http://localhost:8679

# Run with custom log level
pytest tests/ --log-level=DEBUG
```

### Test Markers

The test suite uses the following markers to categorize tests:

- `@pytest.mark.slow` - Tests that may take >10 seconds (large data operations)
- `@pytest.mark.integration` - Integration tests requiring running server (default for most tests)
- `@pytest.mark.storage` - Tests related to storage layer functionality
- `@pytest.mark.search` - Tests related to search and similarity operations
- `@pytest.mark.large_data` - Tests that work with large datasets (>1MB)

### Performance Testing

#### Storage Layout Performance Tests

The storage layout tests (`test_storage_layouts.py`) include comprehensive performance testing:

```bash
# Run VIPER storage tests with large datasets
pytest tests/test_storage_layouts.py::TestVIPERStorageLayout -v -s

# Run LSM storage tests with compaction triggers  
pytest tests/test_storage_layouts.py::TestLSMStorageLayout -v -s

# Compare VIPER vs LSM performance
pytest tests/test_storage_layouts.py::TestStorageLayoutComparison -v -s
```

These tests:
- Insert 15,000-20,000 vectors (20MB+ data) to trigger multiple flushes
- Test search across WAL, flushed, and compacted data layers
- Verify ranking consistency across storage layers
- Measure insert and search performance characteristics

#### Search Performance Tests

```bash
# Test search across large datasets
pytest tests/test_search_operations.py::TestSearchOperations::test_proximity_similarity_search -v

# Test cross-storage search performance
pytest tests/test_storage_layouts.py::TestCrossStorageSearch -v -s
```

## Test Data and Fixtures

### Shared Fixtures (`conftest.py`)

- `rest_client` / `grpc_client` - Pre-configured client instances
- `test_collection` - Automatically managed test collections with cleanup
- `collection_manager` - Helper for creating and managing multiple collections
- `performance_monitor` - Performance measurement utilities
- `unique_collection_name` - Generates unique collection names per test

### Test Data Generation

- **BERT Embeddings**: Tests use `sentence-transformers` for realistic embeddings
- **Diverse Content**: Test documents span multiple categories (technology, healthcare, science, etc.)
- **Metadata Variety**: Rich metadata for testing filtering and search functionality

### Example Test Data

```python
# Realistic test documents with BERT embeddings
documents = [
    {
        "id": "tech_001",
        "text": "Artificial intelligence and machine learning are revolutionizing software development",
        "category": "technology",
        "importance": 9,
        "author": "Dr. Sarah Chen"
    }
    # ... more documents
]
```

## Coverage Goals

The test suite aims for comprehensive coverage:

### Functional Coverage
- ✅ Collection CRUD operations (create, read, update, delete)
- ✅ Vector CRUD operations with metadata
- ✅ Search operations (ID-based, metadata filtering, similarity)
- ✅ Batch operations and large-scale data handling
- ✅ Cross-protocol operations (REST ↔ gRPC)
- ✅ Error handling and edge cases

### Performance Coverage
- ✅ Large dataset operations (20MB+ data)
- ✅ Storage layout performance (VIPER vs LSM)
- ✅ Flush and compaction trigger testing
- ✅ Search performance across storage layers
- ✅ Concurrent operation handling

### Integration Coverage
- ✅ End-to-end workflows
- ✅ Data persistence across operations
- ✅ WAL durability and recovery
- ✅ Cross-storage search ranking
- ✅ Multi-client concurrent operations

## Troubleshooting

### Common Issues

1. **Server Not Running**
   ```
   Error: ProximaDB server not accessible at http://localhost:5678
   ```
   - Ensure ProximaDB server is running on the expected ports
   - Check server logs for any startup issues

2. **Test Timeouts**
   ```
   Error: Test timed out after 30 seconds
   ```
   - Large data tests may take longer on slower systems
   - Use `--fast` flag to skip slow tests during development

3. **Collection Conflicts**
   ```
   Error: Collection already exists
   ```
   - Tests use unique names with timestamps
   - Ensure proper cleanup in test teardown methods

4. **Memory Issues with Large Tests**
   ```
   MemoryError: Unable to allocate array
   ```
   - Large data tests require sufficient RAM (8GB+ recommended)
   - Consider reducing dataset sizes for resource-constrained environments

### Debug Mode

Enable debug logging for detailed test execution:

```bash
pytest tests/ --log-level=DEBUG -s
```

### Performance Monitoring

Monitor test performance with the performance fixture:

```python
def test_my_operation(performance_monitor):
    performance_monitor.start_timer("operation")
    # ... perform operation
    duration = performance_monitor.end_timer("operation")
    performance_monitor.assert_performance("operation", max_seconds=1.0)
```

## Contributing

When adding new tests:

1. **Follow Naming Conventions**: Use descriptive test names with `test_` prefix
2. **Use Appropriate Markers**: Add relevant pytest markers for categorization
3. **Include Cleanup**: Ensure test resources are properly cleaned up
4. **Document Large Tests**: Add comments explaining complex test scenarios
5. **Performance Awareness**: Consider test execution time and mark slow tests appropriately

### Test Structure Template

```python
class TestNewFeature:
    """Test new feature functionality"""
    
    @pytest.fixture
    def setup_data(self):
        """Setup test-specific data"""
        # ... setup code
        yield data
        # ... cleanup code
    
    def test_basic_functionality(self, rest_client, setup_data):
        """Test basic functionality"""
        # Arrange
        # Act  
        # Assert
        
    @pytest.mark.slow
    def test_large_scale_operations(self, grpc_client, setup_data):
        """Test with large datasets"""
        # ... large data test
```

## Continuous Integration

The test suite is designed for CI/CD integration:

- **Fast Mode**: Use `--fast` flag for quick feedback in CI
- **Coverage Reporting**: Generate coverage reports for quality metrics  
- **Parallel Execution**: Tests are designed to run safely in parallel
- **Resource Management**: Automatic cleanup prevents resource leaks

Example CI configuration:
```yaml
# Fast feedback stage
- pytest tests/ --fast --cov=proximadb

# Full test stage  
- pytest tests/ --cov=proximadb --cov-report=xml
```