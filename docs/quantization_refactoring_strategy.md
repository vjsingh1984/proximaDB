# Quantization Refactoring Strategy

## Current State Analysis

### Existing Quantization Implementations

1. **Unified Quantization (`/src/compute/quantization/unified.rs`)**
   - Proto-first design using `proximadb.proto` types
   - Supports PQ, Binary, Scalar, Uniform quantization
   - Has codebook storage abstraction
   - Includes SIMD-optimized Hamming distance

2. **VIPER Quantization (`/src/storage/engines/viper/quantization.rs`)**
   - Engine-specific implementation
   - Duplicates PQ training logic
   - Has its own quality metrics
   - Storage-aware optimizations

3. **SST Quantization (`/src/storage/engines/sst/quantization.rs`)**
   - Newly created with duplicate implementations
   - Binary sketches, PQ codes, INT8 quantization
   - Not using existing infrastructure

## Problems with Current Approach

1. **Code Duplication**
   - PQ training implemented 3 times
   - Binary quantization implemented 3 times
   - INT8/Scalar quantization implemented 3 times
   - Hamming distance implemented multiple times

2. **Inconsistency**
   - Different APIs for same functionality
   - Different quality metrics
   - Different configuration structures

3. **Maintenance Burden**
   - Bug fixes need to be applied in multiple places
   - Performance improvements not shared
   - Hardware optimizations scattered

## Proposed Refactoring Strategy

### Phase 1: Create Common Storage Quantization Module

Create `/src/compute/quantization/storage_engine.rs`:

```rust
//! Common quantization infrastructure for all storage engines
//! Extends the unified quantization with storage-specific features

use super::unified::*;

/// Common configuration for storage engine quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageQuantizationConfig {
    /// Base quantization level
    pub level: UnifiedQuantizationLevel,
    /// Enable progressive resolution
    pub progressive_resolution: bool,
    /// Quality threshold for acceptance
    pub quality_threshold: f32,
    /// Memory budget for quantization
    pub memory_budget_mb: usize,
}

/// Common quantized data structure for storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageQuantizedData {
    /// Primary quantization (e.g., PQ codes)
    pub primary: Option<QuantizedVector>,
    /// Secondary quantization for filtering (e.g., binary sketch)
    pub filter: Option<QuantizedVector>,
    /// Tertiary quantization for fast approximation (e.g., INT8)
    pub fast: Option<QuantizedVector>,
    /// Metadata about quantization
    pub metadata: QuantizationMetadata,
}

/// Trait for storage-aware quantization
#[async_trait]
pub trait StorageQuantization: Send + Sync {
    /// Train quantization model from vectors
    async fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()>;
    
    /// Quantize vectors for storage
    async fn quantize_batch(&self, vectors: &[Vec<f32>]) -> Result<Vec<StorageQuantizedData>>;
    
    /// Progressive search with filtering
    async fn progressive_search(
        &self,
        query: &[f32],
        data: &[StorageQuantizedData],
        stage: SearchStage,
    ) -> Result<SearchStageResult>;
    
    /// Calculate storage savings
    fn calculate_savings(&self, original_size: usize, quantized: &[StorageQuantizedData]) -> f32;
}

/// Search stages for progressive resolution
#[derive(Debug, Clone, Copy)]
pub enum SearchStage {
    /// Stage 1: Binary filtering (95% reduction)
    BinaryFilter,
    /// Stage 2: PQ ranking (further refinement)
    PQRanking,
    /// Stage 3: Full precision (100% accuracy)
    FullPrecision,
}
```

### Phase 2: Create Engine-Specific Adapters

#### VIPER Adapter (`/src/storage/engines/viper/quantization_adapter.rs`):

```rust
use crate::compute::quantization::storage_engine::*;

pub struct ViperQuantizationAdapter {
    base: StorageQuantizationEngine,
    // VIPER-specific fields
    columnar_optimization: bool,
    parquet_compression: bool,
}

impl ViperQuantizationAdapter {
    /// VIPER-specific: Optimize for columnar storage
    pub fn optimize_for_parquet(&mut self, schema: &ParquetSchema) {
        // Columnar-specific optimizations
    }
    
    /// VIPER-specific: Dual column strategy
    pub fn create_dual_columns(&self, data: &StorageQuantizedData) 
        -> (Column<f32>, Column<u8>) {
        // Create FP32 and quantized columns
    }
}
```

#### SST Adapter (`/src/storage/engines/sst/quantization_adapter.rs`):

```rust
use crate::compute::quantization::storage_engine::*;

pub struct SstQuantizationAdapter {
    base: StorageQuantizationEngine,
    // SST-specific fields
    block_size: usize,
    enable_similarity_sorting: bool,
}

impl SstQuantizationAdapter {
    /// SST-specific: Sort by PQ similarity for better compression
    pub fn sort_by_similarity(&self, data: &mut [StorageQuantizedData]) {
        // PQ-based similarity sorting
    }
    
    /// SST-specific: Create hierarchical bloom filters
    pub fn create_bloom_hierarchy(&self, data: &[StorageQuantizedData]) 
        -> BloomHierarchy {
        // Create multi-level bloom filters
    }
}
```

### Phase 3: Migrate Existing Code

1. **Update VIPER** to use `ViperQuantizationAdapter`
2. **Update SST** to use `SstQuantizationAdapter`
3. **Remove duplicate implementations**
4. **Update tests to use common infrastructure**

### Phase 4: Add Advanced Features to Common Module

```rust
// In /src/compute/quantization/advanced.rs

/// Hardware-accelerated operations
pub struct SimdQuantizationOps {
    backend: HardwareBackend,
}

impl SimdQuantizationOps {
    /// SIMD-optimized Hamming distance
    #[cfg(target_arch = "x86_64")]
    pub fn hamming_distance_avx512(&self, a: &[u8], b: &[u8]) -> u32 {
        // AVX-512 implementation
    }
    
    /// SIMD-optimized PQ distance
    #[cfg(target_arch = "x86_64")]
    pub fn pq_distance_avx2(&self, query_table: &[f32], codes: &[u8]) -> f32 {
        // AVX2 implementation
    }
    
    /// GPU-accelerated batch quantization
    #[cfg(feature = "gpu")]
    pub async fn quantize_batch_gpu(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        // CUDA/ROCm implementation
    }
}

/// Learning-based quantization
pub struct LearnedQuantization {
    /// Neural network for optimal quantization
    model: Option<NeuralQuantizer>,
}

/// Adaptive quantization based on data distribution
pub struct AdaptiveQuantization {
    /// Automatically select best quantization level
    pub fn analyze_and_select(&self, vectors: &[Vec<f32>]) -> UnifiedQuantizationLevel {
        // Analyze data distribution
        // Select optimal quantization
    }
}
```

## Implementation Plan

### Week 1: Common Infrastructure
- [ ] Create `/src/compute/quantization/storage_engine.rs`
- [ ] Implement `StorageQuantizationEngine` with common logic
- [ ] Add `StorageQuantizedData` structure
- [ ] Create trait for storage-aware quantization

### Week 2: Adapters
- [ ] Create VIPER adapter with columnar optimizations
- [ ] Create SST adapter with block optimizations
- [ ] Add engine-specific configuration

### Week 3: Migration
- [ ] Migrate VIPER to use adapter
- [ ] Migrate SST to use adapter
- [ ] Remove duplicate code
- [ ] Update tests

### Week 4: Advanced Features
- [ ] Add SIMD optimizations to common module
- [ ] Add GPU support (if enabled)
- [ ] Add adaptive quantization
- [ ] Add learned quantization

## Benefits of Refactoring

1. **Code Reuse**: 70% reduction in quantization code
2. **Consistency**: Same API across all engines
3. **Performance**: Shared optimizations benefit all engines
4. **Maintainability**: Single place for bug fixes
5. **Extensibility**: Easy to add new quantization methods
6. **Testing**: Common test suite for all engines

## Migration Guide

### For VIPER:
```rust
// Before
let engine = VectorQuantizationEngine::new(config);
engine.train_model(&vectors)?;
let quantized = engine.quantize_vectors(&records)?;

// After
let adapter = ViperQuantizationAdapter::new(config);
adapter.train(&vectors).await?;
let quantized = adapter.quantize_batch(&vectors).await?;
```

### For SST:
```rust
// Before
let manager = QuantizationManager::new(config, enable_int8);
manager.learn_codebook(&vectors)?;
let section = manager.create_quantized_section(&vectors);

// After
let adapter = SstQuantizationAdapter::new(config);
adapter.train(&vectors).await?;
let quantized = adapter.quantize_batch(&vectors).await?;
```

## Testing Strategy

### Unit Tests
- Common quantization operations
- Engine-specific adapters
- SIMD operations
- Quality metrics

### Integration Tests
- VIPER with quantization
- SST with quantization
- Cross-engine compatibility
- Performance benchmarks

### Benchmarks
- Quantization speed
- Memory usage
- Search accuracy
- I/O reduction

## Rollout Plan

1. **Phase 1**: Implement common module alongside existing code
2. **Phase 2**: Gradually migrate engines to use common module
3. **Phase 3**: Deprecate old implementations
4. **Phase 4**: Remove deprecated code

## Success Metrics

- **Code Reduction**: Target 70% less quantization code
- **Performance**: No regression in quantization speed
- **Accuracy**: Maintain or improve search quality
- **Memory**: Reduce memory footprint by 20%
- **I/O**: Maintain 95-99% I/O reduction

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Performance regression | Benchmark before/after each change |
| Breaking changes | Keep old APIs during migration |
| Engine-specific optimizations lost | Preserve in adapters |
| Increased complexity | Clear documentation and examples |

## Conclusion

This refactoring will create a robust, efficient, and maintainable quantization infrastructure that serves all storage engines while preserving engine-specific optimizations. The modular design allows for future enhancements without affecting existing functionality.