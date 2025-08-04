# ProximaDB Python SDK - Performance Benchmarks

This directory contains standalone performance benchmarks extracted from the test suite for focused performance analysis.

## Available Benchmarks

### 1. Search Performance Benchmark
**File**: `search_performance_benchmark.py`
**Purpose**: Measure search latency and throughput across REST and gRPC protocols

**Metrics**:
- Search latency (mean, median, P95, P99)
- Search throughput (queries per second)
- Protocol comparison (REST vs gRPC)

**Usage**:
```bash
cd demo/benchmark/python-sdk/
python search_performance_benchmark.py
```

### 2. Bulk Operations Benchmark
**File**: `bulk_operations_benchmark.py`
**Purpose**: Measure bulk insert/upsert performance and storage engine comparison

**Metrics**:
- Bulk insert rate (vectors per second)
- Upsert performance
- Storage engine comparison (VIPER vs SST)
- Protocol comparison for bulk operations

**Usage**:
```bash
cd demo/benchmark/python-sdk/
python bulk_operations_benchmark.py
```

## Prerequisites

1. **ProximaDB Server Running**:
   ```bash
   # From project root
   cargo run --release --bin proximadb-server
   # Server should be running on ports 5678 (REST) and 5679 (gRPC)
   ```

2. **Python Dependencies**:
   ```bash
   pip install numpy proximadb
   ```

## Expected Output

### Search Performance Benchmark
```
🚀 ProximaDB Python SDK - Search Performance Benchmark
============================================================
✅ Populated 1000 vectors in 2.34s (427 vectors/s)

🔍 Search Latency Benchmark (50 queries)
📊 Search Latency Results:
REST API:
  Mean: 15.23ms
  Median: 14.85ms
  P95: 18.67ms
  P99: 22.14ms

gRPC API:
  Mean: 8.91ms
  Median: 8.72ms
  P95: 11.45ms
  P99: 13.28ms

🚀 gRPC is 1.71x faster than REST (mean latency)

⚡ Search Throughput Benchmark (15s duration)
📈 Throughput Results:
  REST: 58.3 queries/second
  gRPC: 89.7 queries/second
  gRPC throughput advantage: 1.54x
```

### Bulk Operations Benchmark
```
🚀 ProximaDB Python SDK - Bulk Operations Benchmark
============================================================
📥 Bulk Insert Benchmark - REST
  Successfully inserted: 2000/2000 vectors
  Average rate: 1245 vectors/second

📥 Bulk Insert Benchmark - GRPC
  Successfully inserted: 2000/2000 vectors
  Average rate: 1687 vectors/second

🏗️ Storage Engine Performance Comparison
  VIPER:
    Insert: 1203 vec/s
    Search: 12.45ms
  SST:
    Insert: 1287 vec/s
    Search: 11.89ms
```

## Benchmark Configuration

All benchmarks use realistic test parameters:

- **Vector Dimensions**: 384D (BERT-like), 512D (large embeddings)
- **Dataset Sizes**: 1000-2000 vectors for manageable benchmark times
- **Batch Sizes**: 100-200 vectors per batch (optimal for ProximaDB)
- **Distance Metric**: Cosine similarity (most common for embeddings)
- **Protocols**: Both REST and gRPC for comparison

## Integration with CI/CD

These benchmarks can be integrated into CI/CD pipelines for performance regression detection:

```bash
# Example CI usage
python search_performance_benchmark.py > search_perf_results.txt
python bulk_operations_benchmark.py > bulk_ops_results.txt
# Parse results and compare against baselines
```

## Notes

- Benchmarks use random vectors for reproducible performance testing
- Each benchmark includes warm-up phases to ensure stable measurements
- Results may vary based on system resources and server configuration
- Benchmarks automatically clean up test collections after completion