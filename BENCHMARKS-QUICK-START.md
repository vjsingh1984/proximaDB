# 🚀 ProximaDB Benchmark Suite - Quick Start Guide

Complete industry-standard benchmarking infrastructure for measuring ProximaDB's performance across all three modalities.

## ⚡ Quick Start (5 Minutes)

```bash
# 1. Setup benchmarks (one-time)
cd benches
./scripts/setup_benchmarks.sh

# 2. Start ProximaDB server (in another terminal)
cargo run --bin proximadb-server

# 3. Run all benchmarks
./scripts/run_all_benchmarks.sh

# 4. View results
cat results/latest/MASTER_SUMMARY.txt
```

## 📊 What Gets Measured

### ✅ All Three Metrics You Care About:

| Metric | Vector | Graph | Document |
|--------|--------|-------|----------|
| **Elapsed Time** | ✅ Query execution | ✅ Traversal time | ✅ Operation time |
| **Latency** | ✅ P50/P95/P99 | ✅ Avg/Max | ✅ P50/P95/P99 |
| **Memory** | ✅ Runtime usage | ✅ Operation memory | ✅ JVM heap |

### Plus:
- ✅ **Throughput** (QPS/ops/sec)
- ✅ **Accuracy** (recall@k for vector search)
- ✅ **Scalability** (different data sizes)

## 🎯 Benchmarks Included

### 1. **VectorDBBench** (Vector Databases)
- **Measures**: QPS, latency (P95/P99), memory, recall
- **Datasets**: SIFT-1M, GIST-1M, DEEP-1B
- **Competitors**: Milvus (~12K QPS), Qdrant (~10K QPS), Weaviate (~8K QPS)
- **Status**: ✅ Ready to run

### 2. **LDBC SNB** (Graph Databases)
- **Measures**: Query time, latency, memory, throughput
- **Workloads**: Social Network Benchmark (interactive)
- **Competitors**: Neo4j (~1K ops/sec), TigerGraph (~10K ops/sec)
- **Status**: ✅ Ready to run

### 3. **YCSB** (Document Databases)
- **Measures**: Ops/sec, latency (P95/P99), memory
- **Workloads**: A (read-write), B (read-heavy), C (read-only)
- **Competitors**: MongoDB (~10K ops/sec), PostgreSQL (~8K ops/sec)
- **Status**: ✅ Ready to run

## 📁 Directory Structure

```
benches/
├── README.md                    # This guide
├── configs/                     # Benchmark configurations
├── results/                     # Benchmark results (auto-generated)
├── scripts/                     # Run scripts
│   ├── setup_benchmarks.sh     # One-time setup
│   ├── run_vector_benchmarks.sh
│   ├── run_graph_benchmarks.sh
│   ├── run_document_benchmarks.sh
│   └── run_all_benchmarks.sh    # Master script
└── adapters/                    # ProximaDB integrations
```

## 🔧 Prerequisites

### Required:
- ✅ Python 3.8+
- ✅ Rust/Cargo
- ✅ 16GB RAM minimum
- ✅ 100GB SSD storage

### Optional (for full benchmarks):
- Java 11+ (for LDBC)
- Maven (for LDBC)

## 📖 Detailed Usage

### Setup (One-Time)

```bash
cd benches
./scripts/setup_benchmarks.sh

# This installs:
# - VectorDBBench (vector database benchmark)
# - ANN-Benchmarks (algorithm benchmark)
# - LDBC SNB (graph database benchmark)
# - YCSB (document database benchmark)
```

### Run Individual Benchmarks

```bash
# Vector only
./scripts/run_vector_benchmarks.sh

# Graph only
./scripts/run_graph_benchmarks.sh

# Document only
./scripts/run_document_benchmarks.sh
```

### Run All Benchmarks

```bash
# Run everything
./scripts/run_all_benchmarks.sh

# With custom output directory
./scripts/run_all_benchmarks.sh --output /path/to/results

# CI/CD mode (generates JSON)
./scripts/run_all_benchmarks.sh --ci
```

## 📊 Understanding Results

### Result Files

```
results/latest/
├── MASTER_SUMMARY.txt           # Overall summary
├── vector_benchmarks.log        # Vector benchmark log
├── graph_benchmarks.log         # Graph benchmark log
├── document_benchmarks.log      # Document benchmark log
├── ci_summary.json              # CI/CD JSON summary (if --ci)
└── [detailed results per modality]
```

### Summary Format

```
ProximaDB Comprehensive Benchmark Results
========================================
Timestamp: 20241204_153000
Total Duration: 1200s (20 minutes)

Phase 1: Vector Database Benchmarks
===================================
Duration: 400s (6 minutes)

Key Metrics:
  QPS: 8500
  Latency P95: 15.2 ms
  Latency P99: 28.4 ms
  Memory: 2048 MB
  Recall: 0.97

Phase 2: Graph Database Benchmarks
===================================
Duration: 500s (8 minutes)

Key Metrics:
  Throughput: 1250 ops/sec
  Average latency: 12.5 ms
  Memory: 1536 MB

Phase 3: Document Database Benchmarks
=====================================
Duration: 300s (5 minutes)

Key Metrics:
  Throughput: 9200 ops/sec
  P95 latency: 18.3 ms
  Memory: 1792 MB
```

## 🆚 Competitor Comparison

### Vector (SIFT-1M, 97% recall):
```
Industry:
  Milvus:     ~12,000 QPS
  Qdrant:     ~10,000 QPS
  Weaviate:   ~8,000 QPS
  Pinecone:   ~15,000 QPS

ProximaDB:   [Your results here]
```

### Graph (LDBC SNB SF1):
```
Industry:
  Neo4j:        ~1,000 ops/sec
  TigerGraph:   ~10,000 ops/sec (analytical)
  Neptune:      ~800 ops/sec

ProximaDB:     [Your results here]
```

### Document (YCSB Workload A):
```
Industry:
  MongoDB:     ~10,000 ops/sec
  PostgreSQL:  ~8,000 ops/sec
  CouchDB:     ~5,000 ops/sec

ProximaDB:    [Your results here]
```

## 🔄 Continuous Benchmarking

### For CI/CD Integration

```bash
# Run in CI pipeline
./scripts/run_all_benchmarks.sh --ci --output results/ci

# Check for regressions
python scripts/check_regression.py \
    --baseline results/main \
    --current results/ci

# Fail on regression
if [ $? -ne 0 ]; then
    echo "Performance regression detected!"
    exit 1
fi
```

### Automated Scheduling

```bash
# Run weekly benchmarks
crontab -e
# Add: 0 2 * * 0 cd /path/to/proximadb/benches && ./scripts/run_all_benchmarks.sh

# Run daily quick benchmarks
0 2 * * * cd /path/to/proximadb/benches && ./scripts/run_vector_benchmarks.sh
```

## 📈 Performance Tracking

### Historical Results

```bash
# List all benchmark runs
ls -la results/

# Compare two runs
python scripts/compare_results.py \
    --baseline results/master_20241201 \
    --current results/master_20241204

# Generate performance report
python scripts/generate_report.py \
    --results results/latest \
    --output report.html
```

### Regression Detection

```bash
# Check for performance regressions
python scripts/check_regression.py \
    --baseline results/main \
    --threshold 10  # Alert on 10% degradation
```

## 🛠️ Troubleshooting

### Common Issues

**1. Out of Memory**
```bash
# Reduce dataset size
# Edit configs/vectordbbench/proximadb_sift.yaml
# Change dataset from sift-1m to sift-100k

# Or increase available RAM
# Ensure at least 16GB available
```

**2. Slow Performance**
```bash
# Check disk type (must be SSD, not HDD)
df -Th | grep /tmp/proximadb

# Check ProximaDB server logs
tail -f /tmp/proximadb/*.log
```

**3. Benchmark Failures**
```bash
# Check server is running
curl http://localhost:5678/health

# Enable debug output
./scripts/run_vector_benchmarks.sh --verbose --debug
```

## 📚 Additional Resources

### Documentation
- [VectorDBBench GitHub](https://github.com/zilliztech/VectorDBBench)
- [ANN-Benchmarks GitHub](https://github.com/erikbern/ann-benchmarks)
- [LDBC Website](https://ldbcouncil.org/benchmarks/)
- [YCSB GitHub](https://github.com/brianfrankcooper/YCSB)

### ProximaDB-Specific
- `docs/benchmarks/` - Detailed benchmarking guide
- `docs/performance/` - Performance tuning guide
- `CLAUDE.md` - Development instructions

## 🎯 Success Criteria

### What Makes a Good Result?

**Vector (SIFT-1M, 97% recall):**
- ✅ QPS > 8,000 (competitive with Weaviate)
- ✅ P95 latency < 20ms
- ✅ Memory < 4GB

**Graph (LDBC SNB SF1):**
- ✅ Throughput > 1,000 ops/sec (competitive with Neo4j)
- ✅ Average latency < 20ms
- ✅ Memory < 4GB

**Document (YCSB Workload A):**
- ✅ Throughput > 8,000 ops/sec (competitive with PostgreSQL)
- ✅ P95 latency < 25ms
- ✅ Memory < 4GB

## 🚀 Next Steps

1. **Run First Benchmark**
   ```bash
   ./scripts/run_all_benchmarks.sh
   ```

2. **Review Results**
   ```bash
   cat results/latest/MASTER_SUMMARY.txt
   ```

3. **Compare with Competitors**
   - Check if ProximaDB meets success criteria
   - Identify performance bottlenecks
   - Plan optimizations

4. **Establish Baseline**
   ```bash
   cp -r results/latest results/baseline
   ```

5. **Track Performance Over Time**
   - Run benchmarks weekly
   - Monitor for regressions
   - Publish results

## 💡 Tips

- **First run**: Use smaller datasets (SIFT-100K instead of SIFT-1M)
- **Consistency**: Run on same hardware for fair comparison
- **Monitoring**: Watch memory usage during benchmarks
- **Validation**: Verify results match expected patterns

## 🆘 Support

For issues or questions:
- GitHub Issues: [ProximaDB/issues](https://github.com/your-org/proximadb/issues)
- Documentation: `docs/benchmarks/`
- Troubleshooting: See Troubleshooting section above

---

**Ready to benchmark? Run `./scripts/setup_benchmarks.sh` to get started!** 🚀
