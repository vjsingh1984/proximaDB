# Memory Mapping Integration Design Analysis

## Problem Statement
How should we integrate memory mapping (mmap) into ProximaDB's storage layer to optimize I/O performance while maintaining clean abstractions and efficient resource utilization?

## Key Considerations

### 1. Current Architecture
- **FileSystem API**: Abstract interface supporting local/S3/GCS/Azure
- **Multiple Readers**: Parquet, SST, columnar engines each with different access patterns
- **Range Reads**: Critical for query pruning and selective data access
- **Cloud Storage**: Must handle files that need downloading before mmap

### 2. Access Patterns Analysis

#### Parquet Reader
- **Row Group Access**: Reads specific row groups (50-100MB chunks)
- **Column Projection**: Often reads only specific columns
- **Footer Metadata**: Small, frequently accessed (ideal for mmap)
- **Page-level Pruning**: Needs byte-range access within row groups

#### SST Reader
- **Block-based**: Reads 4KB-64KB blocks
- **Bloom Filters**: Small, hot data at file start
- **Index Blocks**: Frequently accessed, good mmap candidates
- **Data Blocks**: Selective access based on search

#### Cloud Storage Considerations
- **Download Cost**: $0.09/GB egress
- **Latency**: 50-200ms per request
- **Caching**: Local copies after download
- **Partial Reads**: Range requests save bandwidth

## Design Options

### Option 1: Transparent mmap in FileSystem API

```rust
impl FileSystem {
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        // Transparently use mmap when beneficial
        if self.should_use_mmap(path).await? {
            let mmap = self.get_or_create_mmap(path).await?;
            Ok(mmap.to_vec())
        } else {
            self.read_traditional(path).await
        }
    }
    
    async fn read_range(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if let Some(mmap) = self.get_cached_mmap(path) {
            // Zero-copy slice from mmap
            Ok(mmap[offset..offset+len].to_vec())
        } else {
            // Traditional range read or download
            self.read_range_traditional(path, offset, len).await
        }
    }
}
```

**Pros:**
- Simple reader implementation
- Automatic optimization
- Consistent behavior across engines
- Centralized cache management

**Cons:**
- Less control for specialized access patterns
- May mmap entire files unnecessarily
- Memory pressure from large mappings
- Difficult to implement partial mmap for large files

### Option 2: Explicit mmap Control in Readers

```rust
impl ParquetReader {
    async fn read_with_strategy(&self, path: &str) -> Result<Data> {
        let fs = self.filesystem.get_filesystem(path)?;
        
        if fs.supports_mmap() && self.should_mmap_file(path) {
            // Reader decides what to mmap
            let footer_mmap = fs.get_mmap_range(path, footer_offset, footer_size).await?;
            let index_mmap = fs.get_mmap_range(path, index_offset, index_size).await?;
            
            // Read data blocks normally (not mmap)
            let data = fs.read_range(path, data_offset, data_size).await?;
        }
    }
}
```

**Pros:**
- Fine-grained control per access pattern
- Can mmap only hot regions (footer, index)
- Memory efficient
- Optimized for each engine's needs

**Cons:**
- Complex reader implementations
- Code duplication across engines
- Maintenance burden
- Risk of inconsistent behavior

### Option 3: Hybrid Smart FileSystem (RECOMMENDED)

```rust
pub struct SmartFileSystem {
    // Configurable strategies per file type
    mmap_strategies: HashMap<FileType, MmapStrategy>,
    // LRU cache of mmaps with size limits
    mmap_cache: Arc<MmapCache>,
    // Track access patterns
    access_tracker: Arc<AccessPatternTracker>,
}

pub enum MmapStrategy {
    Never,                    // Never use mmap
    Always,                   // Always mmap entire file
    Adaptive {                // Smart decision based on:
        min_file_size: u64,   // Don't mmap tiny files
        max_file_size: u64,   // Don't mmap huge files
        hot_regions: Vec<Range<u64>>, // Regions to prioritize
    },
    RangeOnly {              // Only mmap specific ranges
        ranges: Vec<Range<u64>>,
    },
}

impl FileSystem for SmartFileSystem {
    // Smart read with access pattern tracking
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.track_access(path, 0, None);
        
        match self.get_mmap_strategy(path) {
            MmapStrategy::Always => self.read_via_mmap(path).await,
            MmapStrategy::Never => self.read_traditional(path).await,
            MmapStrategy::Adaptive { .. } => self.read_adaptive(path).await,
            _ => self.read_traditional(path).await,
        }
    }
    
    // Intelligent range read
    async fn read_range(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.track_access(path, offset, Some(len));
        
        // Check if this range is in a hot region
        if self.is_hot_region(path, offset, len) {
            // Try to use cached mmap
            if let Some(data) = self.read_from_mmap_cache(path, offset, len) {
                return Ok(data);
            }
            
            // Consider creating mmap for this region
            if self.should_create_regional_mmap(path, offset, len) {
                self.create_regional_mmap(path, offset, len).await?;
            }
        }
        
        // Fallback to traditional read
        self.read_range_traditional(path, offset, len).await
    }
    
    // New method for explicit partial mmap (for advanced users)
    async fn get_regional_mmap(&self, path: &str, region: Range<u64>) -> Result<Option<Arc<Mmap>>> {
        // Implementation for selective mmap
    }
}
```

## Recommended Design: Hybrid Smart FileSystem

### Key Features

1. **Automatic Optimization with Override Capability**
   - FileSystem makes smart decisions by default
   - Readers can provide hints via FileOptions
   - Access pattern learning improves over time

2. **Regional Memory Mapping**
   - Map only frequently accessed regions
   - Examples: Parquet footer, SST bloom filters, index blocks
   - LRU eviction when memory pressure detected

3. **Cloud Storage Integration**
   ```rust
   // For S3/GCS/Azure files:
   1. First access: Download to local cache
   2. Subsequent accesses: Use local cached file with mmap
   3. Smart eviction based on access patterns and available disk space
   ```

4. **Configuration per File Type**
   ```toml
   [filesystem.mmap]
   # Parquet files
   parquet.strategy = "adaptive"
   parquet.min_size = "1MB"
   parquet.max_size = "1GB"
   parquet.hot_regions = ["footer", "column_index"]
   
   # SST files  
   sst.strategy = "range_only"
   sst.ranges = ["0..4KB", "4KB..20KB"]  # Bloom filter and index
   
   # Small metadata files
   metadata.strategy = "always"
   metadata.max_size = "10MB"
   ```

5. **Resource Management**
   ```rust
   pub struct MmapCache {
       max_memory: usize,        // Total memory limit for mmaps
       max_files: usize,         // Max number of mapped files
       eviction_policy: EvictionPolicy,
       // Integration with shared memory pool
       shared_pool: Arc<VectorMemoryPool>,
   }
   ```

### Implementation Strategy

#### Phase 1: Foundation (Week 1)
- Extend FileSystem trait with regional mmap support
- Implement basic MmapCache with LRU eviction
- Add access pattern tracking

#### Phase 2: Smart Strategies (Week 2)
- Implement adaptive mmap strategies
- Add configuration system
- Integrate with existing memory pool

#### Phase 3: Engine Integration (Week 3)
- Update Parquet reader for footer/index mmap
- Update SST reader for bloom/index mmap
- Add FileOptions hints from readers

#### Phase 4: Cloud Optimization (Week 4)
- Implement local caching for cloud files
- Add smart prefetching based on access patterns
- Optimize for cost (minimize downloads)

### API Examples

```rust
// Simple usage - FileSystem handles everything
let data = fs.read("file.parquet").await?;

// Range read with automatic optimization
let chunk = fs.read_range("file.sst", 4096, 8192).await?;

// Advanced usage with hints
let options = FileOptions {
    mmap_hint: Some(MmapHint::HotRegions(vec![0..4096, 1024*1024..1024*1024+8192])),
    access_pattern: AccessPattern::Random,
    ..Default::default()
};
let data = fs.read_with_options("file.parquet", options).await?;

// Explicit control when needed
if let Some(mmap) = fs.get_regional_mmap("file.sst", 0..4096).await? {
    // Direct memory access for hot bloom filter
    let bloom_data = &mmap[0..4096];
}
```

## Tradeoff Analysis

### Complexity
- **Transparent**: Low reader complexity, moderate filesystem complexity
- **Explicit**: High reader complexity, low filesystem complexity  
- **Hybrid**: Moderate both sides, but clean interfaces

### Performance
- **Transparent**: Good average case, suboptimal for specialized patterns
- **Explicit**: Best possible per-engine optimization
- **Hybrid**: Near-optimal with learning, excellent with hints

### Resource Utilization
- **Transparent**: Risk of over-mapping
- **Explicit**: Precise control but risk of fragmentation
- **Hybrid**: Intelligent limits with shared resource pool

### Maintenance
- **Transparent**: Single point of optimization
- **Explicit**: Scattered optimization logic
- **Hybrid**: Centralized smart defaults with escape hatches

## Recommendation

Implement the **Hybrid Smart FileSystem** approach because:

1. **Best of Both Worlds**: Automatic optimization with manual override capability
2. **Future Proof**: Can evolve strategies without changing reader code
3. **Cloud Ready**: Elegant handling of download-then-mmap pattern
4. **Resource Efficient**: Regional mapping prevents memory waste
5. **Performance**: Access pattern learning improves over time
6. **Maintainable**: Central logic with clean extension points

## Next Steps

1. Create detailed technical specification
2. Implement Phase 1 foundation
3. Benchmark with production workloads
4. Iterate based on performance data
5. Document best practices for each engine

## Open Questions

1. Should we use huge pages (2MB) for large sequential reads?
2. How to handle mmap on Windows (different API)?
3. Should regional mmaps be aligned to page boundaries?
4. Integration with existing buffer pool for memory accounting?
5. Telemetry for mmap hit/miss rates?