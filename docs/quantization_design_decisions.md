# Quantization Design Decisions

## Executive Summary
ProximaDB implements quantization as a **default-enabled** feature with smart defaults, providing 95% I/O reduction and 50-80% storage savings with minimal configuration.

## Key Design Decisions

### 1. Quantization is ON by Default
**Decision**: Quantization is enabled by default for all collections unless explicitly disabled.

**Rationale**:
- **95% I/O Reduction**: Progressive filtering (Binary → PQ → Full precision) dramatically reduces disk reads
- **50-80% Storage Savings**: Significant cost reduction for large-scale deployments
- **Minimal CPU Overhead**: SIMD acceleration (AVX-512, AVX2, SSE, NEON) makes quantization fast
- **Hierarchical SST Layout**: Enables efficient progressive search without loading full vectors

**When to Disable**:
- Very small dimensions (< 32)
- Already compressed data
- Extreme precision requirements

### 2. Single Distance Metric for Search and Quantization
**Decision**: Quantization uses the same distance metric as configured in `CollectionConfig.distance_metric`.

**Rationale**:
- **Consistency**: PQ codes are most effective when trained with the actual search metric
- **Simplicity**: One metric to configure, less confusion
- **Correctness**: Ensures quantized approximations align with exact search behavior

### 3. Smart Defaults Based on Dimension
**Decision**: Automatic selection of quantization parameters based on vector dimension.

**Defaults**:
```
dimension >= 128: Product Quantization (PQ)
  - num_subvectors = dimension / 4 (min: 8, max: 64)
  - bits_per_subvector = 8

dimension < 128: Scalar Quantization (INT8)
  - Direct INT8 quantization
  - Better for low dimensions

All cases:
  - enable_progressive_search = true
  - binary_filter_threshold = 0.3
  - training_sample_size = 10000
```

## Proto Structure

### Collection Creation Example
```protobuf
// Create collection with default quantization (recommended)
CollectionRequest {
  operation: CREATE,
  collection_config: {
    name: "embeddings",
    dimension: 768,
    distance_metric: COSINE,
    storage_engine: SST,
    // quantization_config is optional - smart defaults applied
  }
}

// Create collection with custom quantization
CollectionRequest {
  operation: CREATE,
  collection_config: {
    name: "embeddings",
    dimension: 768,
    distance_metric: COSINE,
    storage_engine: SST,
    quantization_config: {
      enabled: true,  // Default anyway
      method: PRODUCT_QUANTIZATION,
      num_subvectors: 32,  // Override default of 768/4=192
      bits_per_subvector: 6,  // Use 6-bit for more compression
      training_sample_size: 50000,  // More samples for better quality
    }
  }
}

// Disable quantization (not recommended)
CollectionRequest {
  operation: CREATE,
  collection_config: {
    name: "small_vectors",
    dimension: 16,
    distance_metric: EUCLIDEAN,
    storage_engine: SST,
    quantization_config: {
      enabled: false  // Only for specific use cases
    }
  }
}
```

### Collection Update Example
```protobuf
// Update quantization settings
CollectionRequest {
  operation: UPDATE,
  collection_id: "embeddings",
  collection_config: {
    quantization_config: {
      enabled: true,
      method: ADAPTIVE,  // Let system choose based on data
      quality_threshold: 0.98,  // Higher quality requirement
    }
  }
}
```

## Implementation Details

### SST Storage Engine
1. **DataBlock Structure**: Always includes `quantized_section`
2. **SstableWriter**: Automatically creates quantization adapter when enabled
3. **Flush Operation**: Uses collection's distance metric for PQ training
4. **Compaction**: Preserves quantization through merge operations

### Collection Service
1. **Default Behavior**: If `quantization_config` not provided, defaults to enabled
2. **Validation**: Ensures sensible parameters (e.g., num_subvectors <= dimension)
3. **Migration**: Existing collections get quantization on next flush/compaction

### Performance Impact
```
Without Quantization:
- Full vector loads: 768 * 4 bytes = 3KB per vector
- 1M vectors = 3GB I/O for full scan

With Quantization (PQ8, 32 subvectors):
- Stage 1 (Binary): 768 bits = 96 bytes per vector
- Stage 2 (PQ): 32 bytes per vector  
- Stage 3 (Full): Only top-k loaded
- 1M vectors = 96MB (stage 1) + 32MB (stage 2) + 3MB (top-1000)
- Total: ~131MB vs 3GB = 95.6% reduction
```

## Migration Strategy

### For New Collections
- Quantization enabled by default
- Smart defaults based on dimension
- No action required

### For Existing Collections
- Quantization applied on next flush/compaction
- No data migration required
- Backward compatible

### Rollback Plan
- Set `quantization_config.enabled = false`
- Next compaction removes quantized sections
- Full vectors remain accessible

## Monitoring and Observability

### Metrics to Track
- `quantization_compression_ratio`: Actual compression achieved
- `quantization_training_time_ms`: Time to train codebooks
- `progressive_search_stages`: Number of filtering stages used
- `progressive_search_candidates`: Candidates at each stage

### Logs
```
INFO: 🎯 Quantization enabled for collection 'embeddings' (method: PQ, subvectors: 32)
INFO: 📊 Quantization training complete: 10000 samples, quality: 0.96
INFO: ✅ Progressive search: Binary(1M) → PQ(10K) → Full(100) candidates
INFO: 💾 Storage savings: 72% (3GB → 840MB)
```

## Future Enhancements

1. **Adaptive Method Selection**: Automatically choose PQ vs INT8 based on data
2. **Dynamic Retraining**: Retrain codebooks as data distribution changes
3. **GPU Acceleration**: Use GPU for PQ training and search
4. **Tiered Quantization**: Different quantization levels for hot/warm/cold data

## Conclusion

ProximaDB's quantization design prioritizes **simplicity** and **performance** with smart defaults that work for 95% of use cases. The system is designed to "just work" out of the box while providing flexibility for advanced users who need custom configurations.

**Key Takeaway**: Just create your collection normally - quantization happens automatically and gives you 95% I/O reduction and 50-80% storage savings for free.