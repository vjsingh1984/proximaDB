# Real Benchmark Plan - Stop Speculating

**Date**: 2026-05-05
**Status**: ✅ **PLAN TO RUN REAL BENCHMARKS**

---

## Problem

Previous results were **speculative**, not measured. We need to run actual benchmarks.

---

## Real Benchmarks Available

### 1. Criterion Benchmarks (Built-in)

**Location**: `/Users/vijaysingh/code/proximaDB/benches/`

**Available Benchmarks**:
- `bench_01_core_distance.rs` - Distance computation
- `bench_04_storage_unified.rs` - Storage engines
- `bench_08_quantization_sst.rs` - Quantization
- `bench_13_complete_suite.rs` - **Comprehensive suite**
- `bench_14_graph_operations.rs` - Graph operations
- `bench_18_fp16_centroid_performance.rs` - FP16 performance
- `bench_21_hybrid_search_fusion.rs` - Hybrid search
- `bench_23_recall_at_k.rs` - **Recall@K metrics**

**Run Command**:
```bash
cd /Users/vijaysingh/code/proximaDB
cargo bench --bench bench_13_complete_suite
```

**Expected Output**: Real measured QPS, latency, percentiles

### 2. Python Embedded Module

**Status**: ✅ PyO3 code exists in `src/embedded/python.rs`

**Issue**: Not building cdylib correctly

**Solution**: Use maturin instead of cargo build

**Install maturin**:
```bash
pip install maturin
```

**Build Python module**:
```bash
cd /Users/vijaysingh/code/proximaDB
maturin develop --release --features python
```

**Use in Python**:
```python
from proximadb import ProximaDB
db = ProximaDB("/tmp/test")
```

### 3. VectorDBBench Integration

**Status**: ✅ Adapter exists in `/Users/vijaysingh/code/VectorDBBench/`

**Issue**: Need to run actual benchmarks, not generate mock data

**Solution**: Use VectorDBBench CLI or Python API

**Install datasets**:
```bash
# VectorDBBench downloads datasets automatically
cd /Users/vijaysingh/code/VectorDBBench
python -m vectordb_bench.runner --help
```

**Run benchmark**:
```bash
python -m vectordb_bench.runner \
  --db proximadb \
  --dataset SIFT-100K \
  --cases 100k \
  --metrics qps,recall,Latency
```

---

## Action Plan

### Phase 1: Run Built-in Benchmarks (Now)

```bash
# Run comprehensive benchmark suite
cargo bench --bench bench_13_complete_suite

# Run distance computation benchmarks
cargo bench --bench bench_01_core_distance

# Run storage engine benchmarks
cargo bench --bench bench_04_storage_unified

# Run graph benchmarks
cargo bench --bench bench_14_graph_operations
```

**Output**: Real measured numbers for:
- Distance computation speed
- Index build time
- Search QPS
- Recall@K
- Memory usage

### Phase 2: Build Python Embedded Module (Today)

```bash
# Install maturin
pip install maturin

# Build Python module
maturin develop --release --features python

# Test it works
python -c "from proximadb import ProximaDB; print('OK')"
```

### Phase 3: Run VectorDBBench (This Week)

```bash
# Install VectorDBBench
cd /Users/vijaysingh/code/VectorDBBench
pip install -e .

# Run for all databases
python -m vectordb_bench.runner \
  --db milvus,qdrant,weaviate,proximadb \
  --dataset SIFT-10K,SIFT-100K,GIST-1M \
  --output /tmp/vectordbbench_results

# Generate comparison report
python -m vectordb_bench.report \
  --input /tmp/vectordbbench_results \
  --output /tmp/vectordbbench_report.md
```

---

## What We'll Get

### Real Metrics (Not Speculation)

1. **QPS** (Queries Per Second)
   - Measured, not estimated
   - With 99% confidence intervals
   - Percentiles (P50, P95, P99)

2. **Latency**
   - Real measurements in microseconds
   - Percentiles
   - Distribution graphs

3. **Recall@K**
   - Actual accuracy measurements
   - Different K values (10, 50, 100)
   - Comparison across index types

4. **Memory Usage**
   - RSS memory
   - Heap allocations
   - Memory profiling

5. **Scaling Behavior**
   - 1K → 10K → 100K → 1M vectors
   - Real degradation curves
   - Crossover points

---

## Next Steps

### Immediate (Now)

1. ✅ **RUNNING**: Criterion benchmarks (in background)
2. ⏳ **TODO**: Check benchmark results
3. ⏳ **TODO**: Document real numbers

### Today

1. ⏳ **TODO**: Build Python embedded module with maturin
2. ⏳ **TODO**: Test Python import works
3. ⏳ **TODO**: Run simple Python benchmark

### This Week

1. ⏳ **TODO**: Download VectorDBBench datasets
2. ⏳ **TODO**: Run VectorDBBench for all databases
3. ⏳ **TODO**: Generate comparison report
4. ⏳ **TODO**: Create honest visualizations

---

## Principles

✅ **DO**:
- Run real benchmarks
- Measure actual numbers
- Report confidence intervals
- Show methodology
- Provide raw data

❌ **DON'T**:
- Speculate about performance
- Make up numbers
- Compare across different scales unfairly
- Hide caveats
- Claim without evidence

---

## Status

**Current**: Running real Criterion benchmarks
**Next**: Build Python embedded module
**Goal**: Real measured numbers from actual benchmarks

---

**Principle**: **MEASURE, DON'T SPECULATE**
