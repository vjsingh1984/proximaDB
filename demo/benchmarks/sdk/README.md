# ProximaDB SDK Performance Benchmarks

This directory contains performance benchmarks for the ProximaDB Python SDK that were moved out of the main test suite to keep tests fast and focused.

## Available Benchmarks

### `performance_comprehensive.py`
Comprehensive SDK performance testing including:
- Upsert performance comparison (REST vs gRPC)
- Proto serialization benchmarks  
- Search performance with different k values
- Concurrent operations testing

### `bench_proto_performance.py`
Proto serialization performance testing moved from test suite.

## Running Benchmarks

### Prerequisites
- ProximaDB server running on localhost:5678 (REST) and localhost:5679 (gRPC)
- Python SDK installed: `pip install -e clients/python/`

### Execution
```bash
# Run comprehensive benchmarks
cd demo/benchmarks/sdk/
PROXIMADB_URL=http://localhost:5678 PROXIMADB_GRPC_URL=http://localhost:5679 python performance_comprehensive.py

# Run proto benchmarks only
python bench_proto_performance.py
```

## Benchmark Categories

### 1. Upsert Performance
- Tests batch sizes: 10, 50, 100, 500, 1000 vectors
- Compares REST vs gRPC protocols
- Measures throughput and latency

### 2. Search Performance  
- Tests k values: 1, 10, 50, 100, 500
- Measures query latency and QPS
- Uses 5,000 vector baseline dataset

### 3. Concurrent Operations
- Tests concurrency levels: 1, 2, 4, 8 workers
- Measures parallel operation throughput
- Tests thread safety

### 4. Serialization Performance
- Proto creation and serialization
- Memory usage analysis
- Throughput measurements

## Integration with CI/CD

These benchmarks can be integrated into CI/CD pipelines for:
- Performance regression detection
- Release candidate validation
- Capacity planning data

## Moved from Test Suite

The following tests were moved here to optimize the main test suite:
- `test_performance_comparison` → `benchmark_upsert_performance`
- `test_proto_performance` → `benchmark_proto_serialization`
- Long-running integration tests → `benchmark_search_performance`
- Concurrent stress tests → `benchmark_concurrent_performance`

This separation ensures that:
- ✅ Unit tests run fast (< 2 minutes)
- ✅ Performance analysis is comprehensive
- ✅ CI/CD pipelines remain efficient
- ✅ Performance regression detection is available