# NOVA Storage Engine Optimization Design

## Executive Summary

This document outlines the comprehensive optimization of the NOVA storage engine to eliminate redundancies and implement advanced columnar analytics capabilities. The optimized design leverages Parquet's native capabilities while adding hierarchical statistics, streaming processing, and progressive search.

## Current Architecture Analysis

### Existing Components
- **NovaEngine**: Main engine with universal compression/quantization adapters
- **ID Index**: Redundant separate indexing layer (TO BE REMOVED)
- **Quantized Columns**: Multi-tier quantization (Binary, INT8, PQ)
- **Columnar Search**: Progressive refinement implementation
- **Parquet Integration**: Basic columnar storage with row groups

### Identified Redundancies
1. **Separate ID Index**: Parquet bloom filters and columnar scanning are sufficient
2. **Manual Row Group Tracking**: Parquet metadata provides this natively
3. **Custom Statistics**: Can leverage Parquet statistics with enhancements
4. **Memory-Heavy Loading**: No streaming support for large datasets

## Optimized Architecture Design

### 1. Hierarchical Statistics Structure

```rust
// SuperBlock: Aggregate of 10 row groups for faster skipping
pub struct SuperBlock {
    pub id: u32,
    pub row_groups: Range<u32>,           // Row group indices 0-9, 10-19, etc.
    pub vector_count: u64,
    pub zone_map: ZoneMap,                // Min/max across all dimensions
    pub quantization_stats: QuantizationStats,
    pub selectivity_hints: SelectivityHints,
}

// Enhanced row group statistics
pub struct EnhancedRowGroupStats {
    pub parquet_metadata: RowGroupMetaData,    // Native Parquet stats
    pub vector_zone_map: ZoneMap,              // Per-dimension min/max
    pub quantized_selectivity: QuantizedSelectivity,
    pub compression_ratio: f32,
    pub search_cost_estimate: SearchCostEstimate,
}

// Zone maps for efficient pruning
pub struct ZoneMap {
    pub min_values: Vec<f32>,             // Per dimension
    pub max_values: Vec<f32>,
    pub centroid: Vec<f32>,               // Average vector
    pub variance: Vec<f32>,               // Per dimension variance
}
```

### 2. Streaming Row Group Processor

```rust
pub struct StreamingRowGroupProcessor {
    // Async streaming with configurable memory limits
    pub memory_limit: usize,              // Max memory per row group
    pub prefetch_queue: usize,            // Number of row groups to prefetch
    pub processing_pipeline: Vec<ProcessingStage>,
}

pub enum ProcessingStage {
    BloomFilter,        // Quick existence check
    ZoneMapPruning,     // Dimensional pruning
    QuantizedFilter,    // Binary/INT8 filtering
    FullPrecision,      // Final reranking
}
```

### 3. Progressive Columnar Search Pipeline

```rust
// Multi-tier progressive search with streaming
pub struct ProgressiveColumnarSearch {
    pub stages: Vec<SearchStage>,
    pub cost_optimizer: CostBasedOptimizer,
    pub streaming_processor: StreamingRowGroupProcessor,
}

pub struct SearchStage {
    pub stage_type: QuantizationLevel,    // Binary, INT8, PQ, FP32
    pub expansion_factor: f32,            // Candidate expansion ratio
    pub memory_budget: usize,             // Memory limit for this stage
    pub parallelism: usize,               // Concurrent row groups
}

// Cost-based optimizer for search strategy
pub struct CostBasedOptimizer {
    pub dataset_size: u64,
    pub memory_available: usize,
    pub latency_target: Duration,
    pub accuracy_target: f32,
}
```

## Key Optimizations

### 1. Remove Redundant ID Indexes

**Current Problem**: Separate `ParquetIdIndex` duplicates Parquet's native capabilities
**Solution**: 
- Remove `id_index.rs` entirely
- Use Parquet bloom filters for existence checks
- Leverage columnar ID scanning with zone maps
- Implement streaming ID lookup with row group parallelism

### 2. Hierarchical Statistics

**SuperBlock Level** (10 row groups):
- Aggregate zone maps across multiple row groups
- Coarse-grained pruning for large datasets
- Memory-efficient navigation

**Row Group Level**:
- Enhanced Parquet statistics with vector-specific metrics
- Per-dimension zone maps for fine-grained pruning
- Quantization selectivity estimates

### 3. Streaming Processing

**Memory-Efficient Scanning**:
- Process row groups asynchronously with bounded memory
- Configurable prefetch queue for optimal throughput
- Streaming decompression with backpressure control

**Progressive Loading**:
- Load quantized columns first for filtering
- Stream full vectors only for top candidates
- Minimize memory footprint for large datasets

### 4. Advanced Zone Maps

**Multi-Dimensional Pruning**:
- Per-dimension min/max values for vector components
- Centroid-based distance bounds
- Variance-based selectivity estimation

**Cost-Based Ordering**:
- Order row groups by estimated search cost
- Prioritize high-selectivity, low-variance groups
- Dynamic reordering based on query patterns

## Implementation Plan

### Phase 1: Infrastructure Setup
1. **Remove ID Index**: Delete `id_index.rs` and update references
2. **Hierarchical Stats**: Implement `SuperBlock` and `EnhancedRowGroupStats`
3. **Zone Maps**: Add vector-aware zone map implementation
4. **Streaming Processor**: Basic async row group processing

### Phase 2: Search Optimization
1. **Progressive Pipeline**: Multi-stage search with streaming
2. **Cost Optimizer**: Implement cost-based row group ordering
3. **Memory Management**: Bounded memory with backpressure
4. **Parallelism**: Concurrent row group processing

### Phase 3: Advanced Features
1. **Adaptive Quantization**: Dynamic quantization based on data characteristics
2. **Query Optimization**: Learning-based search strategy selection
3. **Caching**: Intelligent SuperBlock and statistics caching
4. **Monitoring**: Comprehensive performance metrics

## Performance Expectations

### Memory Efficiency
- **90% Memory Reduction**: Streaming processing vs. full loading
- **Hierarchical Pruning**: 95% reduction in row groups scanned
- **Zone Map Filtering**: 80% reduction in decompression overhead

### Search Performance
- **Progressive Filtering**: 10x faster candidate identification
- **Cost-Based Ordering**: 50% reduction in search latency
- **Streaming Pipeline**: 5x improvement in memory-constrained environments

### Storage Efficiency
- **Remove Redundant Index**: 20-30% storage reduction
- **Enhanced Compression**: Leverage Parquet's native compression
- **Statistics Optimization**: Minimal metadata overhead

## Implementation Guidelines

### Code Organization
```
src/storage/engines/nova/
├── mod.rs                          # Main module exports
├── engine.rs                       # Core NovaEngine implementation
├── hierarchical_stats.rs           # SuperBlock and enhanced statistics
├── streaming_processor.rs          # Async row group processing
├── progressive_search.rs           # Multi-stage search pipeline
├── zone_maps.rs                    # Vector-aware zone maps
├── cost_optimizer.rs               # Cost-based optimization
├── quantized_columns.rs            # Enhanced quantization (existing)
├── columnar_search.rs              # Optimized columnar search (existing)
└── tests/                          # Comprehensive test suite
```

### Integration Points
- **Universal Adapters**: Maintain compatibility with existing compression/quantization
- **Parquet Integration**: Enhanced use of native Parquet features
- **Metrics**: Comprehensive monitoring and observability
- **Hardware Optimization**: SIMD and GPU acceleration where applicable

## Migration Strategy

### Backward Compatibility
- **Gradual Migration**: Phase out ID index over multiple releases
- **Feature Flags**: Enable new optimizations progressively
- **Fallback Support**: Maintain old search paths during transition

### Performance Validation
- **Benchmark Suite**: Comprehensive performance testing
- **Memory Profiling**: Validate streaming memory usage
- **Latency Testing**: Ensure search performance improvements
- **Accuracy Validation**: Maintain search quality with optimizations

This design positions NOVA as a next-generation columnar vector storage engine that leverages advanced analytics techniques while maintaining simplicity and performance.