# ProximaDB Demo Examples

This directory contains standalone demo scripts and performance tests that showcase ProximaDB's capabilities with the **DirectVectorService architecture**.

## Directory Structure

### 📊 Performance Tests (`performance/`)
Performance benchmarking and optimization demos:

- `grpc_batch_size_test.py` - gRPC batch size optimization testing
- `large_dataset_perf_test.py` - 100K+ vector performance testing 
- `quantization_perf_test.py` - Product quantization performance analysis
- `grpc_perf_test.py` - gRPC protocol performance testing
- `perf_test.py` - General performance benchmarking
- `test_metadata_filtering_performance.py` - Metadata filter performance
- `test_pq_search_performance.py` - Product quantization search performance
- `test_grpc_lsm_insert.py` - gRPC + LSM engine performance
- `test_grpc_viper_insert.py` - gRPC + VIPER engine performance
- `test_rest_lsm_insert.py` - REST + LSM engine performance
- `test_rest_viper_insert.py` - REST + VIPER engine performance
- `test_final_performance_summary.py` - Comprehensive performance summary

### 🔄 Recovery Tests (`recovery/`)
WAL recovery and persistence demonstrations:

- `test_recovery_search.py` - Post-restart recovery and search testing
- `test_simple_wal_recovery.py` - Basic WAL recovery testing
- `test_vector_wal_recovery.py` - Vector-specific WAL recovery
- `test_optimized_wal_sync.py` - Optimized WAL synchronization testing
- `verify_flush_behavior.py` - Flush behavior verification
- `debug_wal_behavior.py` - WAL behavior debugging utilities

### 🌐 Protocol Comparison (`protocol_comparison/`)
REST vs gRPC protocol comparison demos:

- `test_rest_debug.py` - REST protocol debugging
- `test_rest_only.py` - REST-only testing
- `large_batch_test.py` - Large batch operations comparison
- `comprehensive_perf_test.py` - Comprehensive protocol performance
- `run_advanced_performance_tests.py` - Advanced performance test runner
- `run_persistence_tests.py` - Persistence test runner

### 🚀 Basic Demos (`basic_demos/`)
Simple demonstration scripts:

- `enhanced_demo.py` - Enhanced feature demonstration
- `test_viper_flush_demo.py` - VIPER engine flush demonstration
- `test_basic_sdk.py` - Basic SDK usage examples
- `test_unified_simple.py` - Unified client simple examples

### 🛠️ Utilities (`utilities/`)
Helper utilities and tools:

- `perf_summary.py` - Performance results summarization
- `server_restart_helper.py` - Server restart automation
- `update_test_imports.py` - Import path update utility

## Usage

### Prerequisites
1. ProximaDB server running:
   ```bash
   cargo run --release --bin proximadb-server
   ```

2. Python dependencies installed:
   ```bash
   cd clients/python
   pip install -e .
   ```

### Running Performance Tests
```bash
# Run individual performance tests
python examples/demo/performance/grpc_batch_size_test.py
python examples/demo/performance/large_dataset_perf_test.py

# Run comprehensive protocol comparison
python examples/demo/protocol_comparison/comprehensive_perf_test.py
```

### Running Recovery Tests
```bash
# Test WAL recovery after restart
python examples/demo/recovery/test_recovery_search.py

# Test flush behavior
python examples/demo/recovery/verify_flush_behavior.py
```

### Running Basic Demos
```bash
# Enhanced feature demo
python examples/demo/basic_demos/enhanced_demo.py

# Basic SDK usage
python examples/demo/basic_demos/test_basic_sdk.py
```

## Key Performance Metrics (2025)

Based on testing with DirectVectorService architecture:

- **gRPC vs REST**: 3.4x faster for large batch operations
- **Optimal batch size**: 3,000 vectors for gRPC (77,872 vec/s)
- **Search latency**: Sub-100ms for 1M+ vectors
- **Insert throughput**: 100K+ vectors/second with optimized batching

## Integration with pytest

These standalone demos complement the formal pytest test suite in `tests/`. For automated testing and CI/CD integration, use:

```bash
# Run formal test suite
cd clients/python
python -m pytest tests/

# Run specific test categories
python -m pytest tests/performance/
python -m pytest tests/integration/
```

## Contributing

When adding new demo files:
1. Place in appropriate category directory
2. Include clear documentation and comments
3. Add usage examples to this README
4. Ensure compatibility with latest ProximaDB server