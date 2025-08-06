# SST Block Size Optimization Guide

## Current Analysis

### Vector Size Calculations
```
Per Vector Storage Requirements:
- Vector data: dimension × 4 bytes (FP32)
- Vector ID: ~50 bytes average
- Metadata: ~200-400 bytes average
- Total overhead: ~20-25% of vector size

Examples:
- 384D: 1.5KB vector + 0.4KB overhead = ~2KB/vector
- 768D: 3KB vector + 0.4KB overhead = ~3.5KB/vector
- 1536D: 6KB vector + 0.4KB overhead = ~6.5KB/vector
```

### SST Storage Logic (Discovered)
1. **Primary Sort**: By Vector ID (alphabetical BTreeMap)
2. **Secondary Sort**: By metadata similarity (for compression)
3. **Block Creation**: Sequential packing until size limit
4. **Compression**: Applied to entire sorted blocks

## Recommended Block Sizes

### By Vector Dimension

| Dimension | Vector Size | Block Size | Vectors/Block | Rationale |
|-----------|-------------|------------|---------------|-----------|
| 128D | 0.5KB + 0.4KB = 0.9KB | **2MB** | ~2,200 | Small vectors, high count |
| 384D | 1.5KB + 0.4KB = 1.9KB | **4MB** | ~2,100 | Common embedding size |
| 768D | 3KB + 0.4KB = 3.4KB | **8MB** | ~2,350 | BERT-base embeddings |
| 1024D | 4KB + 0.4KB = 4.4KB | **8-12MB** | ~2,000 | Large language models |
| 1536D | 6KB + 0.4KB = 6.4KB | **12-16MB** | ~2,000 | GPT-3 ada embeddings |
| 2048D | 8KB + 0.4KB = 8.4KB | **16MB** | ~1,900 | Very large models |
| 3072D | 12KB + 0.4KB = 12.4KB | **24-32MB** | ~2,000 | Specialized models |

### Target: 2000-2500 Vectors per Block
**Rationale**: 
- Optimal for ZSTD compression (finds patterns across vectors)
- Good memory footprint for decompression
- Efficient I/O operations
- Balanced random access performance

## Configuration Examples

### Small Models (≤384D)
```toml
[storage.sst_config]
block_size_kb = 4096  # 4MB blocks
compression_level = 3
target_vectors_per_block = 2000
```

### Medium Models (768D)
```toml
[storage.sst_config]
block_size_kb = 8192  # 8MB blocks (current default)
compression_level = 3
target_vectors_per_block = 2350
```

### Large Models (1536D - GPT-3 ada)
```toml
[storage.sst_config]
block_size_kb = 12288  # 12MB blocks
compression_level = 3
target_vectors_per_block = 2000
```

### Very Large Models (≥2048D)
```toml
[storage.sst_config]
block_size_kb = 16384  # 16MB blocks
compression_level = 3
target_vectors_per_block = 2000
```

## Optimization Insights

### Metadata-Aware Sorting
SST sorts by metadata similarity before compression:
- Groups similar metadata values together
- Improves ZSTD compression by 10-15%
- Enables efficient predicate pushdown

### ID-Based Primary Sorting
- BTreeMap ensures alphabetical ID ordering
- Enables binary search within blocks
- Bloom filters for O(1) existence checks

### Compression Sweet Spots
```
Block Size vs Compression Ratio:
< 1MB:   Poor compression (not enough data)
2-4MB:   Good compression (15-25%)
8-16MB:  Best compression (20-40%)
> 32MB:  Diminishing returns, high memory
```

## Performance Trade-offs

| Block Size | Compression | Memory | Random Access | Sequential |
|------------|-------------|--------|---------------|------------|
| 1-2MB | 15-20% | Low | Fast | Good |
| 4-8MB | 20-30% | Medium | Good | Better |
| 8-16MB | 25-40% | High | Slower | Best |
| 16-32MB | 30-45% | Very High | Slowest | Best |

## Recommendations

### Default Change: 4MB → 8MB ✓
**Already implemented** in current codebase (`src/core/config.rs`)
- Good choice for 384-768D vectors
- Maintains ~2000 vectors per block
- Optimal ZSTD compression

### Dynamic Block Sizing (Future)
```rust
fn calculate_optimal_block_size(dimension: usize) -> usize {
    let vector_size = dimension * 4 + 400; // FP32 + overhead
    let target_vectors = 2000;
    let block_size = vector_size * target_vectors;
    
    // Round to nearest MB, min 2MB, max 32MB
    let block_mb = (block_size / 1_000_000).max(2).min(32);
    block_mb * 1_000_000
}
```

### Environment-Specific Tuning

#### Cloud Storage (S3/GCS)
- Use larger blocks (16-32MB)
- Reduces API calls
- Better for bandwidth-optimized transfers

#### Local NVMe
- Use moderate blocks (4-8MB)
- Better cache utilization
- Lower memory pressure

#### Memory-Constrained
- Use smaller blocks (1-2MB)
- Trade compression for memory
- More frequent I/O acceptable

## Sparse Vector Optimization

For sparse vectors (>70% zeros):
- Larger blocks capture more sparsity patterns
- Additional 20-30% compression bonus
- Recommended: 2x normal block size

```toml
[storage.sst_config]
block_size_kb = 16384  # Double for sparse
sparse_optimization = true
```

## Summary

1. **Current 8MB default is good** for most use cases (384-768D)
2. **Larger models need larger blocks** (12-16MB for 1536D+)
3. **Target 2000-2500 vectors per block** for optimal compression
4. **Metadata sorting provides 10-15% bonus** compression
5. **User-configurable based on deployment** environment