# Benchmark Deduplication Analysis

## Current Benchmark Files and Their Purpose

### Core Performance (Keep Separate)
- **bench_01_core_distance.rs**: Distance computation fundamentals
- **bench_02_hardware_simd.rs**: SIMD hardware acceleration
- **bench_03_memory_vector.rs**: Memory optimization patterns

### Storage Engine Benchmarks (DUPLICATED - Need Consolidation)
- **bench_04_storage_comparison.rs**: Engine insertion/search comparison ✅
- **bench_05_storage_lifecycle.rs**: Engine creation, flush, compression ✅ 
- **bench_06_storage_sparsity.rs**: Sparsity + compression (DUPLICATE of 05)
- **bench_07_storage_flush.rs**: Flush optimization (DUPLICATE of 05)
- **bench_11_cross_engine.rs**: Engine creation/flush/memory (DUPLICATE of 04/05)

### Specialized Benchmarks (Keep Separate)
- **bench_08_quantization_sst.rs**: SST-specific quantization
- **bench_09_columnar_viper.rs**: VIPER-specific columnar operations
- **bench_10_query_progressive.rs**: Progressive search testing

### System-Wide Benchmarks (Need Review)
- **bench_12_system_optimization.rs**: General optimizations
- **bench_13_complete_suite.rs**: Complete unified suite (COMBINES ALL)
- **bench_14_graph_operations.rs**: Graph database operations

### Utility (Not Benchmarks)
- **engine_performance_reporter.rs**: CSV report generator (NOT A BENCHMARK)

## Recommended Consolidation Plan

### 1. Merge Storage Engine Benchmarks
Combine bench_04, bench_05, bench_06, bench_07, bench_11 into:
- **bench_storage_engines.rs**: All engine comparisons
  - Creation overhead
  - Insertion performance  
  - Search performance
  - Compression ratios (all algorithms)
  - Sparsity effects
  - Memory efficiency

### 2. Keep Specialized Engine Benchmarks
- **bench_quantization.rs**: Rename from bench_08 (SST + unified quantization)
- **bench_columnar.rs**: Rename from bench_09 (VIPER + columnar formats)
- **bench_progressive_search.rs**: Rename from bench_10

### 3. Core Performance Suite
- **bench_distance_compute.rs**: Rename from bench_01
- **bench_hardware_simd.rs**: Keep bench_02
- **bench_memory_patterns.rs**: Rename from bench_03

### 4. System Benchmarks
- **bench_system_suite.rs**: Merge bench_12 and bench_13
- **bench_graph_ops.rs**: Rename from bench_14

## Issues to Fix

1. **std::fs Usage**: Replace with filesystem API for cloud storage testing
2. **Duplicate Measurements**: Same operations measured in multiple files
3. **Missing Compression Details**: No algorithm names or ratios reported
4. **No Cloud Storage Testing**: All using /tmp local paths

