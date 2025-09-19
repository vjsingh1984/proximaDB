# Benchmark Migration Analysis

## Tests that are actually benchmarks (should be moved or removed):

### 1. **progressive_search_performance_test.rs**
   - **Type**: Performance benchmark
   - **Existing benchmark**: `progressive_search_bench.rs` already exists
   - **Action**: REMOVE (duplicate functionality)

### 2. **pq_compaction_benchmark_test.rs**
   - **Type**: Compaction strategy benchmark
   - **Existing benchmark**: None specific to PQ compaction
   - **Action**: MOVE to benches/pq_compaction_bench.rs

### 3. **engine_compression_comparison_test.rs**
   - **Type**: Compression comparison benchmark
   - **Existing benchmark**: `engine_sparsity_compression_bench.rs` covers similar ground
   - **Action**: REMOVE (duplicate functionality)

### 4. **write_buffer_optimization_integration_test.rs**
   - **Type**: Likely a performance test for write buffer
   - **Existing benchmark**: `flush_optimization_bench.rs` may cover this
   - **Action**: CHECK and possibly REMOVE

### 5. **sst_quantization_comprehensive_test.rs**
   - **Type**: Quantization performance test
   - **Existing benchmark**: `sst_quantization_bench.rs` exists
   - **Action**: CHECK for unique functionality, likely REMOVE

### 6. **optimization_e2e_test.rs**
   - **Type**: End-to-end optimization test
   - **Existing benchmark**: `optimization_benchmarks.rs` exists
   - **Action**: CHECK and possibly REMOVE

### 7. **bloom_filter_optimization_tests.rs**
   - **Type**: Bloom filter performance optimization test
   - **Action**: MOVE to benches/bloom_filter_bench.rs (no existing benchmark)

## Summary Actions:

1. **REMOVE these duplicate benchmarks:** ✅ DONE
   - progressive_search_performance_test.rs (duplicate of progressive_search_bench.rs) ✅
   - engine_compression_comparison_test.rs (duplicate of engine_sparsity_compression_bench.rs) ✅

2. **MOVE these unique benchmarks:**
   - pq_compaction_benchmark_test.rs → ✅ REMOVED (was not referenced, dead code)
   - bloom_filter_optimization_tests.rs → ✅ REMOVED (was not referenced, dead code)

3. **INVESTIGATE further:**
   - write_buffer_optimization_integration_test.rs ✅ REMOVED (was not referenced, dead code)
   - sst_quantization_comprehensive_test.rs ⚠️ EXISTS but NOT REFERENCED (dead code, should be removed or added to mod.rs)
   - optimization_e2e_test.rs ✅ KEPT (legitimate integration test, referenced in mod.rs)

## Final Status:

### Completed Actions:
1. **Removed 5 benchmark/dead files:**
   - progressive_search_performance_test.rs
   - engine_compression_comparison_test.rs
   - pq_compaction_benchmark_test.rs
   - bloom_filter_optimization_tests.rs
   - write_buffer_optimization_integration_test.rs

2. **Moved 2 benchmarks to benches/:**
   - engine_sparsity_compression_benchmark.rs → benches/engine_sparsity_compression_bench.rs
   - comprehensive_engine_benchmark_report.rs → benches/comprehensive_engine_report.rs

### All Issues Resolved:
- `sst_quantization_comprehensive_test.rs` ✅ REMOVED (was not referenced, dead code)