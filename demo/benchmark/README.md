# ProximaDB Search Optimizations Benchmark Suite

This benchmark suite tests the performance impact of three key search optimizations in ProximaDB:

1. **Bloom Filters for Memtable** - Skip 95%+ of irrelevant memtable batches
2. **Parallel SSTable Reading** - 3-5x speedup for multi-file searches  
3. **Early Termination** - Stop searching once k results found (for unordered queries)

## Prerequisites

- Running ProximaDB server on localhost (REST: 5678, gRPC: 5679)
- Python 3.8+ with ProximaDB Python client

## Quick Start

```bash
# Quick test with small dataset
./run_quick_test.sh

# Full benchmark
python3 comprehensive_search_benchmark.py --vectors 100000 --queries 100
```

## Benchmark Scenarios

The comprehensive benchmark tests all optimization combinations across:

### REST API
- Basic search (no filters)
- Single metadata filter
- Complex metadata filters

### gRPC API  
- Basic search (no filters)
- Single metadata filter
- Brand-specific filter

### SQL Queries
- **Ordered (no early termination)**
  - Basic similarity search with ORDER BY
  - Filtered search with ORDER BY
  - Complex multi-condition search with ORDER BY
  
- **Unordered (early termination enabled)**
  - Simple metadata query
  - Multi-filter query
  - OR condition query

## Understanding Results

### Optimization Codes
- **BF**: Bloom Filter enabled
- **PS**: Parallel SSTable reading enabled
- **ET**: Early Termination enabled

### Key Metrics
- **Avg latency**: Average query response time
- **P50/P95/P99**: Latency percentiles
- **QPS**: Queries per second throughput

### Expected Improvements

1. **Bloom Filter Impact** (metadata filtering)
   - 30-50% latency reduction for filtered searches
   - Higher improvement with selective filters

2. **Early Termination** (unordered SQL)
   - 40-60% latency reduction vs ordered queries
   - Scales with result set size

3. **Parallel SSTable Reading**
   - 3-5x throughput improvement
   - Most noticeable with multiple SST files

## Running Custom Benchmarks

```python
# Example: Test with different vector counts
python3 comprehensive_search_benchmark.py --vectors 50000 --queries 200

# Example: Test against remote server
python3 comprehensive_search_benchmark.py \
    --rest-url http://remote-server:5678 \
    --grpc-url http://remote-server:5679
```

## Output Files

Results are saved to timestamped JSON files:
- `comprehensive_search_results_YYYYMMDD_HHMMSS.json`

These contain detailed metrics for each scenario including:
- Query latencies
- Success/failure counts
- Optimization configurations
- Timestamps

## Interpreting Results

Look for:
1. **Bloom Filter Effectiveness**: Compare filtered vs non-filtered searches
2. **Early Termination Gains**: Compare SQL with/without ORDER BY
3. **Protocol Efficiency**: Compare REST vs gRPC performance
4. **Optimization Combinations**: Best performance with all optimizations active

## Troubleshooting

- **"Server not running"**: Start ProximaDB server first
- **"Collection not found"**: Benchmark creates/deletes collections automatically
- **High latencies**: Ensure server has warmed up, run benchmark multiple times
- **Failed queries**: Check server logs for errors