# ProximaDB Benchmark Suite

**Comprehensive industry-standard benchmarks for Graph, Document, and Vector modalities**

## Overview

This benchmark suite measures ProximaDB's performance across all three data modalities using industry-standard benchmarks:

- **Vector**: VectorDBBench, ANN-Benchmarks
- **Graph**: LDBC SNB (Social Network Benchmark)
- **Document**: YCSB (Yahoo! Cloud Serving Benchmark)

## Quick Start

```bash
# Install all benchmarks
./scripts/setup_benchmarks.sh

# Run vector benchmarks
./scripts/run_vector_benchmarks.sh

# Run graph benchmarks
./scripts/run_graph_benchmarks.sh

# Run document benchmarks
./scripts/run_document_benchmarks.sh

# Run all benchmarks
./scripts/run_all_benchmarks.sh
```

## Benchmark Suite

### 1. Vector Database Benchmarks

**Purpose**: Measure vector search performance (QPS, latency, recall, memory)

**Benchmarks**:
- **VectorDBBench**: Full database-level benchmarking
- **ANN-Benchmarks**: Algorithm-level comparison

**Metrics**:
- ✅ Elapsed time (query execution)
- ✅ Latency (P50, P95, P99)
- ✅ Memory usage (runtime)
- ✅ Throughput (QPS)
- ✅ Recall@k (accuracy)

**Competitor Data**: Milvus, Qdrant, Weaviate, Pinecone

### 2. Graph Database Benchmarks

**Purpose**: Measure graph query performance (traversal, analytics, memory)

**Benchmarks**:
- **LDBC SNB Interactive**: Social network workloads
- **LDBC Graphalytics**: Graph analytics (BFS, PageRank, etc.)

**Metrics**:
- ✅ Elapsed time (query execution)
- ✅ Latency (average, maximum)
- ✅ Memory usage (operation)
- ✅ Throughput (transactions/sec)

**Competitor Data**: Neo4j, TigerGraph, Amazon Neptune

### 3. Document Database Benchmarks

**Purpose**: Measure document storage and retrieval performance

**Benchmarks**:
- **YCSB**: Yahoo! Cloud Serving Benchmark
- **Custom document workloads**: JSON document operations

**Metrics**:
- ✅ Elapsed time (operation)
- ✅ Latency (P95, P99)
- ✅ Memory usage (JVM heap, system)
- ✅ Throughput (ops/sec)

**Competitor Data**: MongoDB, CouchDB, PostgreSQL

## Directory Structure

```
benches/
├── README.md                          # This file
├── configs/                           # Benchmark configurations
│   ├── vectordbbench/
│   ├── ann-benchmarks/
│   ├── ldbc/
│   └── ycsb/
├── results/                           # Benchmark results
│   ├── vector/
│   ├── graph/
│   └── document/
├── scripts/                           # Setup and run scripts
│   ├── setup_benchmarks.sh
│   ├── run_vector_benchmarks.sh
│   ├── run_graph_benchmarks.sh
│   ├── run_document_benchmarks.sh
│   └── run_all_benchmarks.sh
└── adapters/                          # ProximaDB adapters
    ├── vectordbbench_adapter.py
    ├── ldbc_adapter/
    └── ycsb_adapter/
```

## System Requirements

### Minimum Requirements
- CPU: 4 cores
- RAM: 16 GB
- Storage: 100 GB SSD
- OS: Linux/macOS

### Recommended Requirements
- CPU: 8+ cores
- RAM: 32+ GB
- Storage: 500 GB NVMe SSD
- OS: Linux

## Metrics Collected

All benchmarks collect:

### Performance Metrics
- **Elapsed Time**: Total execution time for operations
- **Latency**: P50, P95, P99 percentiles
- **Throughput**: Operations/queries per second

### Resource Metrics
- **Memory**: Runtime memory usage
- **CPU**: CPU utilization
- **I/O**: Disk read/write operations

### Quality Metrics
- **Recall**: For vector search (recall@k)
- **Correctness**: Functional validation

## Running Benchmarks

### Individual Benchmarks

```bash
# Vector benchmarks
cd benches/vectordbbench
python run.py --config config/proximadb_sift.yaml

# Graph benchmarks
cd benches/ldbc
./run_ldbc.sh --scale SF1

# Document benchmarks
cd benches/ycsb
./run_ycsb.sh --workload A
```

### All Benchmarks

```bash
./scripts/run_all_benchmarks.sh --output results/$(date +%Y%m%d)
```

## Analyzing Results

Results are saved in `results/` with timestamps:

```bash
# View latest results
cat results/latest/summary.txt

# Compare with competitors
python scripts/compare_results.py --baseline results/competitor.json

# Generate report
python scripts/generate_report.py --results results/latest
```

## Competitor Comparison

Public benchmark results are available for comparison:

- **Vector**: Milvus (~12K QPS), Qdrant (~10K QPS), Weaviate (~8K QPS)
- **Graph**: Neo4j SNB results, TigerGraph analytics results
- **Document**: MongoDB YCSB results, PostgreSQL results

See `competitors/` directory for detailed comparison data.

## Continuous Benchmarking

To integrate with CI/CD:

```bash
# Run in CI
./scripts/run_all_benchmarks.sh --ci --output results/ci

# Check for regressions
python scripts/check_regression.py --baseline results/main --current results/ci
```

## Troubleshooting

### Common Issues

1. **Out of Memory**: Reduce dataset size or increase available RAM
2. **Slow Performance**: Ensure using NVMe SSD, not HDD
3. **Benchmark Failures**: Check ProximaDB server is running

### Debug Mode

```bash
# Enable verbose output
./scripts/run_vector_benchmarks.sh --verbose --debug

# Check logs
tail -f logs/benchmark.log
```

## Contributing

To add new benchmarks:

1. Create adapter in `adapters/`
2. Add configuration in `configs/`
3. Add run script in `scripts/`
4. Update this README

## References

- [VectorDBBench GitHub](https://github.com/zilliztech/VectorDBBench)
- [ANN-Benchmarks GitHub](https://github.com/erikbern/ann-benchmarks)
- [LDBC Website](https://ldbcouncil.org/benchmarks/)
- [YCSB GitHub](https://github.com/brianfrankcooper/YCSB)

## License

Apache 2.0 (same as ProximaDB)

## Support

For issues or questions:
- GitHub Issues: [ProximaDB/issues](https://github.com/your-org/proximadb/issues)
- Documentation: `docs/benchmarks/`
