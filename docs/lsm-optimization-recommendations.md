# LSM Engine Optimization Recommendations

## Executive Summary

The LSM engine should adopt a similar unified reader architecture as VIPER to achieve comparable performance benefits. This document outlines a comprehensive optimization strategy that maintains LSM's row-based SSTable format while adding advanced search capabilities.

## Current State vs. Proposed Architecture

### Current LSM Implementation
- **Direct SSTable Reading**: Reads entire files for search operations
- **Basic Bloom Filters**: Only used for point lookups, not metadata filtering
- **No Caching Layer**: Each search reads from disk
- **Limited Metadata Support**: Filters applied after loading all data
- **Sequential Search**: No block-level or index-based optimization

### Proposed Unified Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    LSM Unified Search API                    │
├─────────────────────────────────────────────────────────────┤
│                  UnifiedSstableReader                        │
│  ┌─────────────┬──────────────┬──────────────┬───────────┐ │
│  │  Strategy   │  Block Cache │  Index Cache │  Bloom    │ │
│  │  Selector   │  (LRU)       │  (Metadata)  │  Filters  │ │
│  └─────────────┴──────────────┴──────────────┴───────────┘ │
├─────────────────────────────────────────────────────────────┤
│                    SSTable Storage Layer                     │
│  ┌──────────┬──────────┬──────────┬──────────┬──────────┐ │
│  │ Level 0  │ Level 1  │ Level 2  │ Level 3  │ Level 4  │ │
│  │ SSTables │ SSTables │ SSTables │ SSTables │ SSTables │ │
│  └──────────┴──────────┴──────────┴──────────┴──────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## Key Optimizations

### 1. UnifiedSstableReader
Similar to VIPER's UnifiedParquetReader, this provides:
- **Automatic Strategy Selection**: Choose optimal reading strategy based on query
- **Block-Level Access**: Read only necessary blocks instead of entire files
- **Unified Interface**: Single API for all search patterns

### 2. Enhanced Bloom Filters
Extend bloom filters beyond key lookup:
```rust
pub struct MetadataBloomFilter {
    pub vector_id_filter: BloomFilter,
    pub metadata_filters: HashMap<String, BloomFilter>,
}
```
- **Per-Column Bloom Filters**: Skip blocks based on metadata values
- **Cardinality Tracking**: Optimize for low-cardinality columns
- **False Positive Rate Tuning**: Adjust based on column characteristics

### 3. Block-Level Caching
Implement LRU cache for frequently accessed blocks:
```rust
pub struct BlockCache {
    cache: LruCache<BlockCacheKey, Arc<DataBlock>>,
    stats: CacheStats,
}
```
- **Smart Eviction**: Consider access patterns and block importance
- **Read-Ahead**: Prefetch adjacent blocks for sequential access
- **Memory Budget**: Configurable cache size with monitoring

### 4. Reading Strategies

#### Full Scan
- For small files or high-selectivity queries
- Benefits from block cache and read-ahead

#### Index Range Scan
- Use SSTable index for targeted block access
- Skip irrelevant blocks based on key range

#### Metadata Filtered
- Use bloom filters to identify candidate blocks
- Apply predicate pushdown at block level

#### Hybrid Strategy
- Combine approaches for complex queries
- Fallback mechanism for edge cases

### 5. MVCC Resolution
Handle multiple versions efficiently:
```rust
fn apply_mvcc_resolution(results: Vec<SearchResult>) -> Vec<SearchResult> {
    // Group by ID, keep latest version
    // Handle tombstones and expired records
}
```

### 6. Search Pipeline Integration

```rust
// Before: Direct SSTable search
let results = search_sstable_for_vectors(&sstable_data, filters)?;

// After: Unified pipeline
let context = CollectionContext { 
    sstable_files, 
    metadata_columns,
    // ... 
};
let results = unified_reader.search_vectors(&params, &context).await?;
```

## Performance Benefits

### Expected Improvements
1. **Reduced I/O**: 60-80% reduction through block-level access
2. **Lower Latency**: 40-60% improvement with caching
3. **Better Scalability**: Handle larger datasets efficiently
4. **Metadata Performance**: 10x improvement for filtered queries

### Measurement Metrics
- Cache hit rate
- Blocks read vs. total blocks
- Bloom filter effectiveness
- Query latency percentiles

## Implementation Phases

### Phase 1: Foundation (Week 1-2)
- [x] Create UnifiedSstableReader structure
- [x] Implement basic strategy selection
- [ ] Add block-level reading capability

### Phase 2: Caching Layer (Week 2-3)
- [ ] Implement BlockCache with LRU eviction
- [ ] Add IndexCache for SSTable indices
- [ ] Integrate cache statistics

### Phase 3: Advanced Features (Week 3-4)
- [ ] Enhanced bloom filters for metadata
- [ ] Predicate pushdown to block level
- [ ] Read-ahead optimization

### Phase 4: Integration (Week 4-5)
- [ ] Update LSM engine to use unified reader
- [ ] Migrate existing search code
- [ ] Performance testing and tuning

## Code Examples

### Using the Unified Reader
```rust
// Create unified reader
let reader = UnifiedSstableReader::new(filesystem);

// Build search context
let context = CollectionContext {
    collection_id: "products".to_string(),
    sstable_files: vec!["level0_001.sst", "level0_002.sst"],
    metadata_columns: vec!["category", "price", "brand"],
};

// Execute search
let results = reader.search_vectors(&search_params, &context).await?;
```

### Strategy Selection
```rust
impl ReadingStrategySelector {
    pub fn select_strategy(&self, params: &SearchParams) -> SstableReadingStrategy {
        if params.has_selective_filters() {
            SstableReadingStrategy::MetadataFiltered { ... }
        } else if self.is_large_dataset() {
            SstableReadingStrategy::IndexRangeScan { ... }
        } else {
            SstableReadingStrategy::FullScan { use_block_cache: true }
        }
    }
}
```

## Testing Strategy

### Unit Tests
- Strategy selection logic
- Cache behavior and eviction
- Bloom filter accuracy

### Integration Tests
- End-to-end search scenarios
- MVCC resolution correctness
- Performance regression tests

### Benchmarks
- Compare with current implementation
- Measure cache effectiveness
- Profile I/O patterns

## Monitoring and Observability

### Key Metrics
```rust
pub struct LsmSearchMetrics {
    pub cache_hit_rate: f64,
    pub blocks_read: u64,
    pub bloom_filter_checks: u64,
    pub false_positives: u64,
    pub search_latency_p50: Duration,
    pub search_latency_p99: Duration,
}
```

### Debug Information
- Strategy selection reasoning
- Cache miss analysis
- Block access patterns

## Future Enhancements

### ML-Driven Optimizations
- Learn access patterns for better caching
- Predict hot blocks based on query history
- Adaptive bloom filter configuration

### Compression Awareness
- Decompress only necessary blocks
- Cache decompressed data when beneficial
- Support multiple compression algorithms

### Distributed Extensions
- Coordinate cache across nodes
- Distributed bloom filters
- Parallel SSTable scanning

## Conclusion

By implementing these optimizations, the LSM engine will achieve performance parity with VIPER while maintaining its strengths in write-heavy workloads and simpler architecture. The unified reader approach provides a clean abstraction that enables future optimizations without changing the API.