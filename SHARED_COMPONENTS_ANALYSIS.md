# Shared Components Analysis: FastLanes and Parquet

## Corrected Engine Storage Format Mapping

### FastLanes-Based Engines (Row-Oriented)
1. **SST** - Uses `FastLanesDataBlock` from `core/formats/fastlanes_blocks`
2. **SWIFT** - Uses `FastLanesDataBlock` from `core/formats/fastlanes_blocks`
3. **HELIX** - Uses `FastLanesDataBlock` from `core/formats/fastlanes_blocks`

### Parquet-Based Engines (Columnar)
1. **VIPER** - Uses Parquet with Arrow integration
2. **NOVA** - Uses Parquet with hierarchical columnar storage

### Hybrid/Custom Formats
1. **RAPTOR** - Uses custom row-group format with `FastLanesScheme` encoding (not `FastLanesDataBlock`)
2. **PRISM** - Uses custom memory-optimized format with multi-resolution quantization

## Shared Component Analysis

### 1. FastLanes Data Blocks (`core/formats/fastlanes_blocks/`)

**Shared by**: SST, SWIFT, HELIX

**Key Components**:
```rust
// core/formats/fastlanes_blocks/block_structures.rs
pub struct FastLanesDataBlock {
    pub encoding_marker: u8,
    pub encoding_metadata: Option<FastLanesMetadata>,
    pub records: Vec<VectorRecord>,
    pub block_id: u32,
    // ...
}
```

**Filesystem Integration Status**:
- **SST**: Mixed - uses both `ZeroCopyIOSystem` and `FilesystemFactory`
- **SWIFT**: Basic - uses only `FilesystemFactory` without caching
- **HELIX**: Basic - uses raw `FileSystem` interface without caching

**Issue**: All three engines sharing FastLanesDataBlock have inconsistent filesystem usage

### 2. Parquet Components (`core/formats/columnar/`)

**Shared by**: VIPER, NOVA

**Key Components**:
```rust
// core/formats/columnar/parquet_io_layer.rs
// core/formats/columnar/parquet_query_engine.rs
// core/formats/columnar/parquet_writer.rs
```

**Filesystem Integration Status**:
- **VIPER**: ✅ Optimal - uses `UnifiedCachingFilesystem` with metadata caching
- **NOVA**: ❌ Suboptimal - uses `FilesystemFactory` without systematic caching

**Issue**: NOVA missing critical Parquet footer caching that VIPER benefits from

### 3. RAPTOR's Unique Position

**Format**: Custom row-group with FastLanes encoding (not FastLanesDataBlock)
- Uses `FastLanesScheme` for encoding but not the shared block structure
- Has its own `RowGroup` and `RowGroupMetadata` structures
- **Filesystem**: ✅ Optimal - uses `UnifiedCachingFilesystem` properly

## Reconciled Findings

### Critical Insight: Format vs Filesystem Orthogonality

The storage format (FastLanes blocks vs Parquet) is **orthogonal** to filesystem caching needs:

1. **FastLanes engines need caching for**:
   - Block metadata and headers
   - Bloom filters per block
   - Index structures
   - Compression dictionaries

2. **Parquet engines need caching for**:
   - Parquet file footers (metadata)
   - Column chunk headers
   - Row group metadata
   - Schema information

### Updated Recommendations

#### Priority 1: Fix FastLanes-Based Engines (SST, SWIFT, HELIX)

Since these engines share `FastLanesDataBlock`, they should have consistent filesystem integration:

```rust
// Standardized for all FastLanes engines
pub struct FastLanesEngine {
    filesystem: Arc<UnifiedCachingFilesystem>,  // Cached access
    block_cache: Arc<FastLanesBlockCache>,      // Shared block cache
    // ...
}
```

**Benefits**:
- Shared block metadata caching across SST/SWIFT/HELIX
- Consistent I/O patterns for FastLanes operations
- Reduced memory usage through shared cache

#### Priority 2: Complete Parquet Engine Optimization (NOVA)

NOVA should match VIPER's caching strategy for Parquet operations:

```rust
// NOVA should match VIPER's pattern
pub struct NovaEngine {
    filesystem: Arc<UnifiedCachingFilesystem>,  // Like VIPER
    parquet_cache: Arc<ParquetMetadataCache>,   // Like VIPER
    // ...
}
```

**Benefits**:
- 80% reduction in Parquet footer parsing
- Shared column statistics caching
- Consistent with VIPER's proven approach

### Filesystem Caching Requirements by Format

#### FastLanes Block Engines (SST, SWIFT, HELIX)
**Cache Requirements**:
- Block headers (small, frequently accessed)
- Bloom filters (medium size, accessed on queries)
- Index nodes (small, hot path)
- Compression dictionaries (if applicable)

**Recommended Cache Size**: 10-50MB per collection
**Cache Policy**: LRU with priority for headers/indices

#### Parquet Engines (VIPER, NOVA)
**Cache Requirements**:
- File footers (can be large with many columns)
- Column chunk metadata (accessed per query)
- Row group statistics (for pruning)
- Schema definitions (small, frequently needed)

**Recommended Cache Size**: 50-200MB per collection
**Cache Policy**: TTL-based with refresh on access

#### RAPTOR (Custom Format)
**Already Optimal**: Uses UnifiedCachingFilesystem appropriately
- Caches row group metadata
- Caches centroid matrices
- Caches bloom filters

## Implementation Priority Matrix

| Engine | Format | Current FS | Target FS | Impact | Effort |
|--------|--------|------------|-----------|---------|---------|
| SST | FastLanesDataBlock | Mixed/Legacy | UnifiedCaching | High | Medium |
| SWIFT | FastLanesDataBlock | Basic | UnifiedCaching | High | Low |
| HELIX | FastLanesDataBlock | Basic | UnifiedCaching | Medium | Low |
| NOVA | Parquet | Basic | UnifiedCaching | High | Low |
| VIPER | Parquet | ✅ Optimal | No change | - | None |
| RAPTOR | Custom+FastLanes | ✅ Optimal | No change | - | None |
| PRISM | Custom | Basic | UnifiedCaching | Low | Low |

## Shared Infrastructure Improvements

### 1. Create Format-Specific Cache Managers

```rust
// For FastLanes engines
pub struct FastLanesBlockCacheManager {
    block_headers: LruCache<BlockId, FastLanesBlockHeader>,
    bloom_filters: LruCache<BlockId, BloomFilter>,
    indices: LruCache<BlockId, BlockIndex>,
}

// For Parquet engines
pub struct ParquetMetadataCacheManager {
    footers: LruCache<FileId, ParquetMetadata>,
    row_groups: LruCache<(FileId, usize), RowGroupMetadata>,
    schemas: LruCache<FileId, Schema>,
}
```

### 2. Standardize Cache Configuration

```toml
[storage.cache.fastlanes]
max_size_mb = 50
ttl_seconds = 3600
policy = "lru"

[storage.cache.parquet]
max_size_mb = 200
ttl_seconds = 7200
policy = "ttl_with_refresh"
```

### 3. Unified Metrics Collection

All engines should report:
- Cache hit rates by data type
- Memory usage by cache component
- I/O reduction metrics
- Cloud API call savings

## Conclusion

The analysis reveals that:

1. **Format grouping matters**: Engines sharing formats (FastLanes or Parquet) should have consistent caching strategies
2. **SST's legacy code** affects all FastLanes-based engines' potential performance
3. **NOVA is missing critical optimizations** that VIPER already has for Parquet
4. **RAPTOR and VIPER are exemplars** of proper UnifiedCachingFilesystem usage
5. **Shared caching infrastructure** could benefit engines using the same format

The priority should be:
1. Fix SST's mixed filesystem usage (impacts all FastLanes engines)
2. Add caching to NOVA (quick win for Parquet operations)
3. Standardize SWIFT and HELIX (complete FastLanes consistency)
4. Consider shared cache managers for format-specific optimizations