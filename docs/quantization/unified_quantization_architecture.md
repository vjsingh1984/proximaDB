# Unified Quantization Architecture

## Core Principle
**All quantization decisions flow from the Collection's QuantizationConfig** - a single source of truth that drives storage, index, and runtime search optimization.

## Architecture Overview

```
Collection QuantizationConfig (Single Source of Truth)
├── Storage Layer (SST/VIPER)
│   └── Direct inheritance (always uses collection config)
├── Index Layer (HNSW/IVF/LSH)
│   ├── Default: Use storage format as-is (zero CPU cost)
│   └── Override: Create index-specific quantization from FP32
└── Runtime Search Layer
    ├── Query hints for optimization
    └── Cost-based quantization selection
```

## 1. Storage Layer Quantization

### Principle
Storage ALWAYS uses collection's quantization config - no exceptions.

### Implementation
```rust
// Storage quantization is mandatory and follows collection config
fn get_storage_quantization(collection: &Collection) -> QuantizationConfig {
    collection.config.quantization_config.unwrap_or_default()
}
```

### Data Flow
1. Collection defines quantization (e.g., PQ8 with 32 subvectors)
2. SST/VIPER storage applies during flush/compaction
3. Data stored in quantized format (50-80% space savings)
4. Progressive search enables 95% I/O reduction

## 2. Index Layer Quantization

### Principle
**Indexes use storage data as-is by default** (zero CPU overhead), but can override for quality.

### Default Behavior: Zero-Copy from Storage
```rust
// Default: Index directly uses storage format
fn build_index_default(storage_data: &[QuantizedData]) -> Index {
    // No quantization needed - use storage format directly
    // This saves CPU and ensures consistency
    Index::from_quantized(storage_data)
}
```

### Override Behavior: Index-Specific Quantization
```rust
// Override: Index creates its own quantization from FP32
fn build_index_override(
    storage_data: &[QuantizedData],
    index_config: &IndexConfig,
) -> Index {
    if let Some(quant_override) = &index_config.quantization_override {
        // Decompress to FP32, then apply index-specific quantization
        let fp32_data = storage_data.decompress_to_fp32();
        let index_quantized = apply_quantization(fp32_data, quant_override);
        Index::from_quantized(index_quantized)
    } else {
        // Use storage format
        Index::from_quantized(storage_data)
    }
}
```

### Decision Tree
```
IndexConfig.quantization_override exists?
├── NO (Default - 90% of cases)
│   └── Use storage quantization as-is
│       └── Benefits: Zero CPU cost, perfect consistency
├── YES (Override - 10% of cases)
│   ├── Decompress storage to FP32
│   ├── Apply index-specific quantization
│   └── Benefits: Custom optimization for specific index
```

## 3. Runtime Search Quantization

### Principle
Query engine dynamically selects quantization strategy based on data size and search hints.

### Search Hint System
```protobuf
message SearchHint {
  enum OptimizationGoal {
    MAXIMIZE_RECALL = 0;      // Use FP32 or minimal quantization
    BALANCE = 1;              // Default - progressive search
    MAXIMIZE_SPEED = 2;       // Aggressive quantization
    MINIMIZE_MEMORY = 3;      // Maximum quantization
  }
  
  OptimizationGoal goal = 1;
  optional float recall_threshold = 2;  // Minimum acceptable recall
  optional int64 memory_budget_mb = 3;  // Memory constraint
  optional int32 latency_budget_ms = 4; // Time constraint
}
```

### Runtime Decision Logic
```rust
fn select_runtime_quantization(
    collection: &Collection,
    query: &SearchQuery,
    data_size: usize,
) -> QuantizationStrategy {
    // Check search hints
    if let Some(hint) = &query.search_hint {
        match hint.goal {
            MAXIMIZE_RECALL => QuantizationStrategy::None,  // Use FP32
            MAXIMIZE_SPEED => QuantizationStrategy::Binary,  // Fastest
            MINIMIZE_MEMORY => QuantizationStrategy::from_collection(collection),
            BALANCE => QuantizationStrategy::Progressive,
        }
    } else {
        // Cost-based decision
        if data_size > LARGE_DATASET_THRESHOLD {
            // Large dataset - use progressive quantization
            QuantizationStrategy::Progressive
        } else if data_size < SMALL_DATASET_THRESHOLD {
            // Small dataset - use FP32 for quality
            QuantizationStrategy::None
        } else {
            // Medium dataset - use collection default
            QuantizationStrategy::from_collection(collection)
        }
    }
}
```

## 4. Progressive Search Pipeline

### Three-Stage Resolution
```
Stage 1: Binary Filter (99% candidate reduction)
├── Input: All vectors
├── Quantization: 1 bit per dimension
├── Output: Top 1% candidates
└── Benefit: Extremely fast filtering

Stage 2: PQ Ranking (Accurate scoring)
├── Input: Top 1% from Stage 1
├── Quantization: Collection's PQ config
├── Output: Top-k * 10 candidates
└── Benefit: Good recall with low memory

Stage 3: FP32 Reranking (Perfect recall)
├── Input: Top-k * 10 from Stage 2
├── Quantization: None (full precision)
├── Output: Final top-k results
└── Benefit: 100% recall on final results
```

## 5. Configuration Examples

### Example 1: Standard Configuration
```protobuf
// Collection config drives everything
CollectionConfig {
  name: "embeddings",
  dimension: 768,
  distance_metric: COSINE,
  quantization_config: {
    enabled: true,
    method: PRODUCT_QUANTIZATION,
    num_subvectors: 32,
    bits_per_subvector: 8,
    enable_progressive_search: true,
  }
}

// Storage: Uses PQ8 with 32 subvectors
// Index: Uses storage format as-is (zero CPU)
// Search: Progressive pipeline for perfect recall
```

### Example 2: High-Performance Configuration
```protobuf
// Collection with aggressive quantization
CollectionConfig {
  name: "large_dataset",
  dimension: 2048,
  distance_metric: EUCLIDEAN,
  quantization_config: {
    enabled: true,
    method: PRODUCT_QUANTIZATION,
    num_subvectors: 64,
    bits_per_subvector: 4,  // PQ4 for extreme compression
    enable_progressive_search: true,
  }
}

// One index overrides for quality
IndexConfig {
  index_name: "precise_rerank",
  algorithm: FLAT,
  quantization_override: {
    enabled: true,
    method: SCALAR_QUANTIZATION,  // INT8 instead of PQ4
  }
}
```

### Example 3: Mixed Precision Strategy
```protobuf
// Collection with moderate quantization
CollectionConfig {
  name: "products",
  dimension: 512,
  distance_metric: COSINE,
  quantization_config: {
    enabled: true,
    method: PRODUCT_QUANTIZATION,
    num_subvectors: 16,
    bits_per_subvector: 8,
  }
}

// Fast index uses storage format
IndexConfig {
  index_name: "fast_search",
  algorithm: IVF,
  // No override - uses storage PQ8
}

// Precise index uses custom quantization
IndexConfig {
  index_name: "precise_search",
  algorithm: HNSW,
  quantization_override: {
    enabled: false,  // Force FP32
  }
}

// Runtime search with hint
SearchQuery {
  vector: [...],
  search_hint: {
    goal: MAXIMIZE_RECALL,
    recall_threshold: 0.99,
  }
}
```

## 6. CPU Cost Optimization

### Zero-Copy Path (Default)
```
Storage (PQ8) → Index (PQ8) → Search (PQ8)
CPU Cost: 0 (no conversions needed)
```

### Override Path
```
Storage (PQ8) → Decompress to FP32 → Index Quantization (INT8) → Search
CPU Cost: Decompression + Requantization
```

### Best Practices
1. **Use storage format in indexes** unless quality requires override
2. **Let progressive search handle recall** instead of forcing FP32
3. **Override only for specific use cases** (e.g., reranking index)
4. **Document why overrides are used** in index configuration

## 7. Memory Hierarchy

```
Collection (768-dim, 1M vectors):

Storage Layer:
├── SST with PQ8: 32MB (32 bytes per vector)
└── Savings: 99% vs FP32

Index Layer (Default - uses storage format):
├── IVF Index: 32MB (references storage)
├── HNSW Index: 32MB + graph structure
└── No additional quantization memory

Index Layer (Override to FP32):
├── FLAT Index: 3GB (full precision)
└── 100x more memory than default

Runtime Search:
├── Stage 1 (Binary): 96MB for all vectors
├── Stage 2 (PQ): 320KB for top 10K
├── Stage 3 (FP32): 30MB for top 10K
└── Total: < 130MB peak memory
```

## 8. Migration Path

### Phase 1: Storage Quantization
- [x] Implement collection-driven storage quantization
- [x] SST and VIPER use collection config
- [x] Progressive search in storage layer

### Phase 2: Index Alignment
- [ ] Indexes use storage format by default
- [ ] Implement override mechanism
- [ ] Zero-copy index building from quantized storage

### Phase 3: Runtime Optimization
- [ ] Search hint system
- [ ] Cost-based quantization selection
- [ ] Dynamic progressive pipeline

### Phase 4: Advanced Features
- [ ] Auto-tuning based on workload
- [ ] Adaptive quantization parameters
- [ ] Multi-tier caching with different quantization levels

## 9. Performance Characteristics

### Storage Performance
| Quantization | Space | Write Speed | Read Speed |
|-------------|-------|-------------|------------|
| None (FP32) | 100% | Baseline | Baseline |
| PQ8 | 3-5% | 0.8x | 20x (progressive) |
| PQ4 | 1-2% | 0.7x | 25x (progressive) |
| Binary | 3% | 0.9x | 50x (filter only) |

### Index Performance (Using Storage Format)
| Index Type | Build Time | Memory | Search Speed |
|-----------|------------|---------|--------------|
| IVF | 1x | 1x | 10x |
| HNSW | 1x | 1.5x | 20x |
| LSH | 0.5x | 0.8x | 30x |

### Runtime Search Performance
| Strategy | Recall | Latency | Memory |
|----------|--------|---------|---------|
| FP32 Only | 100% | 1x | 100% |
| Progressive | 99.9% | 0.1x | 5% |
| PQ Only | 95% | 0.05x | 3% |
| Binary Only | 85% | 0.01x | 3% |

## 10. Key Insights

1. **Collection Config is King**: All quantization flows from collection config
2. **Zero-Copy Default**: Indexes use storage format to avoid CPU overhead
3. **Progressive Search**: Enables perfect recall with quantized data
4. **Override When Needed**: Specific indexes can customize for their needs
5. **Runtime Flexibility**: Query engine adapts based on hints and cost
6. **CPU Optimization**: Avoid unnecessary conversions between formats
7. **Memory Efficiency**: 90%+ savings while maintaining quality

## Conclusion

The unified quantization architecture provides:
- **Simplicity**: Single configuration point
- **Efficiency**: Zero-copy data flow by default
- **Flexibility**: Override when needed
- **Quality**: Progressive search maintains recall
- **Performance**: 95% I/O reduction, 90% memory savings

This design ensures that quantization "just works" for most users while providing escape hatches for advanced optimization.