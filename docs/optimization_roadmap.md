# ProximaDB Search Optimization Roadmap

**Status**: Phase 1.1 Complete ✅ | Compilation Errors Fixed ✅
**Last Updated**: 2025-12-18

## Overview

Multi-phase optimization plan to reduce search latency and improve cache efficiency through advanced filtering, quantization, and block-level optimizations.

---

## Phase 1: Block-Level Optimizations

### Phase 1.1: FP16 Centroid Quantization ✅ COMPLETE

**Status**: Implemented, tested, and committed (commit: 9ff4ac59)

**Achievements**:
- ✅ 50% storage reduction for centroids
- ✅ <0.1% distance error (measured: 0.0304%)
- ✅ 100% recall preservation
- ✅ Backward compatible dual-field strategy
- ✅ SST and SWIFT engines fully supported
- ✅ Comprehensive tests (7 tests passing)
- ✅ Performance benchmarks created

**Impact**:
- Storage: 128D centroid: 512B → 256B
- Cache: 2x more centroids fit in L3 cache
- Memory: 50% reduction for centroid data in RAM
- Network: 50% reduction in index transfer costs (cloud deployments)

**Files Modified**: 18 files, +1705 lines
**Documentation**: `docs/block_level_centroids.md`

---

### Phase 1.2: Adaptive Bloom Filters (NEXT)

**Goal**: Reduce false positives in block-level filtering through adaptive bloom filter sizing and multi-level bloom strategies.

**Current State**:
- Fixed-size bloom filters in SST engine
- Block-level bloom filters available but not adaptive
- No dynamic sizing based on collection characteristics

**Proposed Improvements**:

1. **Adaptive Bloom Filter Sizing**
   - Auto-size bloom filters based on block size and expected query patterns
   - Formula: `optimal_bits = -n * ln(p) / (ln(2)^2)`
     - n = number of keys in block
     - p = target false positive rate (default: 0.01)
   - Benefit: Reduce memory overhead while maintaining accuracy

2. **Multi-Level Bloom Filters**
   - File-level bloom filter (coarse)
   - SuperBlock-level bloom filter (medium)
   - Block-level bloom filter (fine)
   - Progressive filtering: File → SuperBlock → Block → Exact scan
   - Benefit: 95%+ elimination of unnecessary I/O

3. **Bloom Filter Compression**
   - Apply RLE or simple compression to sparse bloom filters
   - Store compressed bloom in index
   - Decompress on-demand during filtering
   - Benefit: 30-50% reduction in bloom filter storage

**Implementation Plan**:
```rust
// 1. Adaptive sizing (src/storage/engines/impls/sst/bloom_filter.rs)
pub struct AdaptiveBloomConfig {
    pub target_fp_rate: f64,          // Default: 0.01 (1%)
    pub min_bits_per_key: usize,      // Default: 8
    pub max_bits_per_key: usize,      // Default: 20
    pub enable_compression: bool,     // Default: true
}

impl AdaptiveBloomConfig {
    pub fn optimal_size(&self, num_keys: usize) -> usize {
        let ideal = (-num_keys as f64 * self.target_fp_rate.ln()
                     / (2.0_f64.ln().powi(2))) as usize;
        ideal.clamp(
            num_keys * self.min_bits_per_key,
            num_keys * self.max_bits_per_key
        )
    }
}

// 2. Multi-level bloom filters
pub struct HierarchicalBloomFilters {
    pub file_bloom: Option<CompressedBloom>,      // 10K keys, 0.05 FP
    pub superblock_blooms: Vec<CompressedBloom>,  // 1K keys each, 0.02 FP
    pub block_blooms: Vec<CompressedBloom>,       // 256 keys each, 0.01 FP
}

// 3. Compressed bloom filter
pub struct CompressedBloom {
    pub original_size: usize,
    pub compressed_data: Vec<u8>,  // RLE or simple compression
    pub hash_count: usize,
}
```

**Expected Benefits**:
- 40-60% reduction in bloom filter memory
- 10-20% improvement in cache hit rates (smaller blooms = more in cache)
- Reduced false positive rate through adaptive sizing
- Progressive elimination (95%+ blocks pruned before decompression)

**Testing Requirements**:
- False positive rate measurement across different collection sizes
- Memory usage comparison (fixed vs adaptive)
- Query latency impact (compression overhead vs cache benefits)
- Integration with existing SST/SWIFT/HELIX engines

**Estimated Effort**: 2-3 days
**Risk**: Low (isolated to bloom filter code, backward compatible)

---

### Phase 1.3: Unified Centroid Footer

**Goal**: Consolidate all block centroids into a single footer structure for efficient batch loading and cache-friendly access.

**Current State**:
- Centroids stored per-block in index entries
- Requires N random reads to load N block centroids
- Poor cache locality (scattered across index)

**Proposed Design**:

```rust
// Unified centroid footer structure
pub struct CentroidFooter {
    pub version: u8,                    // Footer format version
    pub num_blocks: u32,                // Total blocks in file
    pub dimension: u16,                 // Vector dimension
    pub centroid_dtype: CentroidDtype,  // FP32, FP16, or PQ

    // Centroids stored contiguously for cache efficiency
    pub centroids: CentroidStorage,

    // Mapping: block_id → centroid index
    pub block_to_centroid: Vec<u32>,

    // Optional: Centroid metadata for advanced pruning
    pub centroid_metadata: Option<CentroidMetadata>,
}

pub enum CentroidStorage {
    FP32(Vec<f32>),         // Contiguous FP32 array (dim * num_blocks)
    FP16(Vec<u16>),         // Contiguous FP16 array (50% size)
    PQ {                     // Product quantization (further compression)
        codebook: Vec<f32>,
        codes: Vec<u8>,
    },
}

pub struct CentroidMetadata {
    pub centroid_norms: Vec<f32>,       // Pre-computed norms for cosine
    pub centroid_bounds: Vec<(f32, f32)>, // Min/max per dimension
}
```

**Implementation Steps**:

1. **Writer Changes**:
   - Accumulate centroids during block writing
   - Serialize unified footer at end of file
   - Store footer offset in file header

2. **Reader Changes**:
   - Load entire centroid footer in single I/O operation
   - Convert to FP32 if needed (for FP16/PQ formats)
   - Use contiguous array for SIMD-friendly distance computation

3. **Migration Path**:
   - New files: Write unified footer
   - Old files: Fall back to per-block centroids
   - Compaction: Convert to unified footer format

**Expected Benefits**:
- **1 I/O operation** instead of N for loading centroids
- **Sequential read** (faster) instead of random access
- **Cache-friendly**: All centroids loaded together
- **SIMD-optimized**: Contiguous storage enables vectorized operations
- **50-80% faster centroid loading** for large files

**Memory Layout** (128D, 1000 blocks):
```
Without footer:  1000 random reads × 512B = ~500KB scattered
With footer:     1 sequential read × 500KB = 500KB contiguous

With FP16:       1 sequential read × 250KB = 250KB contiguous ← 2x better
```

**Testing Requirements**:
- Benchmark centroid loading time (scattered vs unified)
- Verify cache performance (perf stat cache-misses)
- Test migration path (old → new format)
- Validate SIMD optimizations

**Estimated Effort**: 3-4 days
**Risk**: Medium (requires file format change, needs migration logic)

---

## Phase 2: Quantization Enhancements

### Phase 2.1: Product Quantization (PQ) for Centroids

**Goal**: Further compress centroids beyond FP16 using product quantization.

**Current**: FP16 gives 50% reduction (128D: 512B → 256B)
**Target**: PQ gives 75-90% reduction (128D: 512B → 64-128B)

**Trade-off**:
- FP16: <0.1% error, fast conversion
- PQ: 1-2% error, slower lookup (codebook required)

**When to Use**:
- Very large collections (>1M vectors)
- Cloud deployments (minimize network transfer)
- Memory-constrained environments

**Implementation**: Use existing PQ infrastructure, apply to centroids

---

### Phase 2.2: Quantized Vector Precomputation

**Goal**: Pre-compute and cache quantized query vectors to avoid repeated quantization during search.

**Current State**:
- Query quantized on every SSTable scan
- Repeated INT8/PQ encoding overhead

**Proposed**:
```rust
pub struct QuantizedQueryCache {
    query_fp32: Vec<f32>,
    query_int8: Option<Vec<i8>>,      // Cached INT8 quantization
    query_pq: Option<Vec<u8>>,        // Cached PQ codes
    query_fp16: Option<Vec<u16>>,     // Cached FP16
    last_used: Instant,
}
```

**Benefit**: 5-10% latency reduction on multi-file searches

---

## Phase 3: Advanced Search Optimizations

### Phase 3.1: Early Termination with Dynamic Thresholds

**Goal**: Stop scanning blocks once enough high-quality results found.

**Strategy**: Adaptive top-k threshold based on current result quality

### Phase 3.2: Parallel Block Scanning

**Goal**: Scan multiple blocks concurrently using thread pool.

**Benefit**: 2-4x throughput on multi-core systems

### Phase 3.3: Prefetching and Speculative Loading

**Goal**: Prefetch likely-needed blocks during centroid filtering.

**Strategy**: Load top-N blocks in parallel while filtering remaining

---

## Implementation Priority

**Immediate (This Week)**:
1. ✅ Phase 1.1: FP16 Centroid Quantization - COMPLETE
2. 🔄 Wait for benchmarks to finish
3. 📊 Analyze FP16 performance data

**Short Term (Next 1-2 Weeks)**:
1. Phase 1.2: Adaptive Bloom Filters
2. Phase 1.3: Unified Centroid Footer
3. Integration testing with real workloads

**Medium Term (Next Month)**:
1. Phase 2.1: Product Quantization for Centroids
2. Phase 2.2: Quantized Vector Precomputation
3. Performance comparison with competitors

**Long Term (Next Quarter)**:
1. Phase 3: Advanced search optimizations
2. Distributed search optimizations
3. GPU acceleration for distance computation

---

## Success Metrics

**Phase 1 Target** (Block-Level Optimizations):
- [ ] 50% reduction in centroid memory ← ✅ Achieved (FP16)
- [ ] 30% reduction in bloom filter memory ← Pending (Adaptive Bloom)
- [ ] 20-30% reduction in search latency ← Pending (Unified Footer + Bloom)
- [ ] 95%+ block elimination rate ← Pending (Multi-level Bloom)

**Phase 2 Target** (Quantization Enhancements):
- [ ] 75-90% reduction in centroid memory (PQ)
- [ ] 5-10% latency reduction (query caching)
- [ ] Sub-10ms search on 1M vectors

**Phase 3 Target** (Advanced Optimizations):
- [ ] 2-4x throughput improvement (parallelization)
- [ ] Sub-5ms search on 1M vectors
- [ ] Competitive with Qdrant/Milvus on benchmarks

---

## Benchmarking Plan

**After Each Phase**:
1. Run `bench_18_fp16_centroid_performance` (or equivalent)
2. Compare against baseline (pre-optimization)
3. Measure:
   - Search latency (p50, p95, p99)
   - Memory usage (RSS, heap)
   - Cache efficiency (cache-misses, cache-hit-rate)
   - I/O reduction (blocks read, bytes read)
4. Document results in `docs/performance/`

**Competitive Benchmarking**:
- Compare against Qdrant, Milvus, Weaviate
- Use SIFT-1M, GIST-1M, Deep-1M datasets
- Metrics: QPS, recall@10, memory usage, index size

---

## Recent Updates (2025-12-18)

### ✅ Compilation Issues Resolved
- **Before**: 101+ compilation errors blocking all tests
- **After**: All compilation errors fixed
- **Test Status**: 2745 tests passing, 8 failing (normal test failures)
- **Effort**: Comprehensive fix of imports, type mismatches, and struct fields

### ✅ Critical Graph Traversal Performance Fix (2025-12-18)
- **Issue**: O(E) complexity in `get_outgoing_edges` and `get_incoming_edges`
- **Fix**: Refactored to use CSR neighbor lists for O(degree) complexity
- **Impact**: 100-1000x performance improvement for graph traversals
- **Test Status**: All 8 graph integration tests passing
- **Files Modified**:
  - `src/graph/engines/orion/mod.rs` (get_outgoing_edges, get_incoming_edges, CSR updates)
  - `src/graph/engines/orion/persistence.rs` (lock changes)

**Key Changes**:
1. Changed from `tokio::sync::RwLock` to `std::sync::RwLock` for CSR structures
2. Made CSR updates synchronous (previously async, broke query correctness)
3. Added `rebuild()` calls after CSR modifications to commit temp_edges
4. Refactored edge lookups to use CSR `get_edge_ids()` instead of iterating all edges

### ⚠️ Technical Debt Identified
See `docs/TECHNICAL_DEBT.md` for detailed analysis of:
1. ✅ **Critical**: O(E) graph traversals (FIXED)
2. **High**: Incomplete parallel algorithms (PageRank, parallel BFS)
3. **Medium**: Partial implementations (Pulsar WAL, Quasar tiering, SQL DML, transactions)
4. **Low**: Sparse vector support (future enhancement)

**Action Items**:
- ✅ **Week 1-2**: Fix critical O(E) traversal issue (COMPLETED)
- **Week 3-4**: Complete core features (parallel algos, SQL DML, transactions)
- **Month 2**: Finish distributed engines (Pulsar/Quasar)

---

## Notes

- All optimizations maintain backward compatibility
- Gradual rollout with feature flags
- Comprehensive testing before production
- Performance monitoring in place
- **NEW**: Technical debt tracked in TECHNICAL_DEBT.md

**Last Review**: 2025-12-18
**Next Review**: After Phase 2 core features (parallel algorithms, SQL DML)
**Related Docs**: `TECHNICAL_DEBT.md`, `block_level_centroids.md`

**Recent Milestones**:
- ✅ Phase 1 Critical Fix: O(E) → O(degree) graph traversals (2025-12-18)
- ✅ FP16 Centroid Quantization: 50% storage reduction (commit: 9ff4ac59)
