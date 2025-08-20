# Tiered Caching with Memory Mapping Design

## Key Insights from Discussion

1. **Cloud stores need local disk cache** - Can't rely on memory alone
2. **Selective access patterns** - Bloom filters (4KB) from 1GB SST files, single columns from 100GB Parquet
3. **Memory pressure awareness** - Readers must adapt when memory is scarce
4. **Locality-based caching** - Keep hot data close, cold data on disk

## Revised Architecture: Three-Tier Caching System

```
┌─────────────────────────────────────────────────────┐
│                   Cloud Storage                      │
│               (S3, GCS, Azure - Cold)                │
└────────────────────┬─────────────────────────────────┘
                     │ Download ranges/chunks
                     ▼
┌─────────────────────────────────────────────────────┐
│              Local Disk Cache                        │
│     (/var/cache/proximadb/ - Warm)                  │
│   • Full files for frequently accessed              │
│   • Partial chunks for selective access             │
│   • LRU eviction when disk space low                │
└────────────────────┬─────────────────────────────────┘
                     │ mmap or read
                     ▼
┌─────────────────────────────────────────────────────┐
│          Memory (mmap + buffer pool)                 │
│                  (Hot)                               │
│   • mmap: Read-only file regions                    │
│   • Shared pool: Processing buffers                 │
│   • Pressure-aware eviction                         │
└──────────────────────────────────────────────────────┘
```

## Design: Locality-Aware Tiered Cache

### 1. Local Disk Cache Layer

```rust
pub struct LocalDiskCache {
    cache_dir: PathBuf,           // /var/cache/proximadb/
    max_cache_size: u64,          // e.g., 100GB
    current_size: AtomicU64,      
    
    // Track what we have cached
    cached_files: DashMap<String, CachedFileInfo>,
    // Track partial downloads
    partial_chunks: DashMap<String, Vec<ChunkInfo>>,
}

pub struct CachedFileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub last_access: Instant,
    pub access_count: u32,
    pub cached_ranges: Vec<Range<u64>>, // Which parts are cached
    pub is_complete: bool,              // Full file or partial?
}

impl LocalDiskCache {
    /// Download only what's needed
    async fn ensure_cached_range(
        &self, 
        remote_url: &str, 
        range: Range<u64>
    ) -> Result<PathBuf> {
        // Check if we already have this range
        if let Some(info) = self.cached_files.get(remote_url) {
            if info.is_complete || self.has_range(&info.cached_ranges, &range) {
                return Ok(info.path.clone());
            }
        }
        
        // Download only the needed range
        let local_path = self.cache_path_for(remote_url);
        
        if self.should_download_full_file(remote_url, &range) {
            // Small file or frequently accessed - download completely
            self.download_full_file(remote_url, &local_path).await?;
        } else {
            // Large file, selective access - download only range
            self.download_range(remote_url, range, &local_path).await?;
        }
        
        Ok(local_path)
    }
    
    fn should_download_full_file(&self, url: &str, range: &Range<u64>) -> bool {
        // Heuristics:
        // - File < 100MB: download full
        // - Accessing > 20% of file: download full
        // - File accessed > 10 times: download full
        // - Otherwise: download range only
    }
}
```

### 2. Memory Pressure-Aware mmap Manager

```rust
pub struct MmapManager {
    mmaps: Arc<RwLock<HashMap<String, MmapEntry>>>,
    total_mapped_size: AtomicUsize,
    max_mapped_size: usize,
    memory_pressure_threshold: f32, // e.g., 0.8 = 80% memory used
}

pub struct MmapEntry {
    mmap: Arc<memmap2::Mmap>,
    regions: Vec<MmapRegion>,
    last_access: Instant,
    pin_count: AtomicU32, // Currently in use
}

pub struct MmapRegion {
    offset: u64,
    len: u64,
    access_count: u32,
    heat_score: f32, // Calculated from recency + frequency
}

impl MmapManager {
    /// Get mmap with memory pressure awareness
    async fn get_mmap_for_range(
        &self,
        local_path: &Path,
        range: Range<u64>,
    ) -> Result<MmapHandle> {
        // Check memory pressure
        let pressure = self.get_memory_pressure();
        
        if pressure > self.memory_pressure_threshold {
            // High pressure - be selective
            if !self.is_critical_region(&local_path, &range) {
                // Don't mmap, return None to force read path
                return Ok(MmapHandle::NotMapped);
            }
            
            // Evict cold mappings to make room
            self.evict_cold_mappings().await?;
        }
        
        // Create or get existing mmap
        self.create_or_get_mmap(local_path, range).await
    }
    
    fn is_critical_region(&self, path: &Path, range: &Range<u64>) -> bool {
        // Critical regions that should always be mapped:
        // - SST: Bloom filters (0..4KB), Index blocks (4KB..64KB)
        // - Parquet: Footer (last 8MB), Column indexes
        // - Small files < 1MB
        
        if path.extension() == Some("sst") {
            return range.start < 64 * 1024; // Bloom + index
        }
        
        if path.extension() == Some("parquet") {
            let file_size = std::fs::metadata(path).ok()?.len();
            return range.end > file_size - 8 * 1024 * 1024; // Footer region
        }
        
        false
    }
}
```

### 3. Reader Integration with Adaptive Access

```rust
impl UnifiedParquetReader {
    async fn read_with_memory_awareness(
        &self,
        file_path: &str,
        columns: &[String],
    ) -> Result<Vec<VectorRecord>> {
        let fs = self.filesystem.get_filesystem(file_path)?;
        
        // Step 1: Ensure we have local cache
        let cache_strategy = self.determine_cache_strategy(file_path, columns);
        let local_path = match cache_strategy {
            CacheStrategy::FullFile => {
                fs.ensure_cached_full(file_path).await?
            }
            CacheStrategy::ColumnsOnly(cols) => {
                // Download only needed column chunks
                let ranges = self.calculate_column_ranges(file_path, &cols).await?;
                fs.ensure_cached_ranges(file_path, ranges).await?
            }
            CacheStrategy::FooterOnly => {
                // Just download footer for metadata
                let footer_range = self.calculate_footer_range(file_path).await?;
                fs.ensure_cached_range(file_path, footer_range).await?
            }
        };
        
        // Step 2: Try memory mapping with pressure awareness
        let memory_strategy = self.determine_memory_strategy(&local_path);
        
        match memory_strategy {
            MemoryStrategy::FullMmap => {
                // Low pressure, small file - mmap everything
                let mmap = fs.get_mmap(&local_path).await?;
                self.read_from_mmap(mmap, columns)
            }
            MemoryStrategy::SelectiveMmap(regions) => {
                // Medium pressure - mmap only hot regions
                let mut data = Vec::new();
                for region in regions {
                    if let Some(mmap) = fs.get_regional_mmap(&local_path, region).await? {
                        data.extend(self.read_from_mmap_region(mmap, region));
                    } else {
                        // Fallback to read for this region
                        data.extend(fs.read_range(&local_path, region).await?);
                    }
                }
                Ok(data)
            }
            MemoryStrategy::StreamingRead => {
                // High pressure - stream from disk without mmap
                self.streaming_read_from_disk(&local_path, columns).await
            }
        }
    }
    
    fn determine_memory_strategy(&self, path: &Path) -> MemoryStrategy {
        let file_size = std::fs::metadata(path).ok()?.len();
        let available_memory = self.get_available_memory();
        let memory_pressure = self.get_memory_pressure();
        
        if memory_pressure > 0.9 {
            // Critical pressure - streaming only
            return MemoryStrategy::StreamingRead;
        }
        
        if file_size < 100 * 1024 * 1024 && memory_pressure < 0.5 {
            // Small file, low pressure - full mmap
            return MemoryStrategy::FullMmap;
        }
        
        // Medium pressure or large file - selective mmap
        let hot_regions = self.identify_hot_regions(path);
        MemoryStrategy::SelectiveMmap(hot_regions)
    }
}
```

### 4. SST Reader with Bloom Filter Optimization

```rust
impl SstReader {
    async fn read_with_bloom_optimization(
        &self,
        file_path: &str,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        let fs = self.filesystem.get_filesystem(file_path)?;
        
        // Step 1: Read just bloom filter (4KB) - always cached
        let bloom_range = 0..4096;
        let local_path = fs.ensure_cached_range(file_path, bloom_range).await?;
        
        // Try to mmap bloom filter (tiny, always fits)
        let bloom_data = if let Some(mmap) = fs.get_regional_mmap(&local_path, bloom_range).await? {
            &mmap[0..4096]
        } else {
            // Very high memory pressure - read directly
            &fs.read_range(&local_path, bloom_range).await?
        };
        
        // Check bloom filter
        if !self.bloom_might_contain(bloom_data, key) {
            return Ok(None); // Definitely not in file
        }
        
        // Step 2: Read index block (4KB-64KB) to find data block
        let index_range = 4096..65536;
        let index_data = if let Some(mmap) = fs.get_regional_mmap(&local_path, index_range).await? {
            mmap.as_ref()
        } else {
            &fs.read_range(&local_path, index_range).await?
        };
        
        let data_block_offset = self.find_data_block(index_data, key)?;
        
        // Step 3: Read only the specific data block
        let block_range = data_block_offset..data_block_offset + BLOCK_SIZE;
        
        // For data blocks, prefer streaming read over mmap (usually cold)
        let data = fs.read_range(&local_path, block_range).await?;
        
        self.search_in_block(&data, key)
    }
}
```

### 5. Filesystem API Extensions

```rust
#[async_trait]
pub trait FileSystem {
    // Existing methods
    async fn read(&self, path: &str) -> Result<Vec<u8>>;
    async fn read_range(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>>;
    
    // New cache-aware methods
    
    /// Ensure file is in local cache (download if needed)
    async fn ensure_cached_full(&self, path: &str) -> Result<PathBuf>;
    
    /// Ensure specific range is cached locally
    async fn ensure_cached_range(&self, path: &str, range: Range<u64>) -> Result<PathBuf>;
    
    /// Ensure multiple ranges are cached (optimize download)
    async fn ensure_cached_ranges(&self, path: &str, ranges: Vec<Range<u64>>) -> Result<PathBuf>;
    
    /// Get mmap with memory pressure awareness
    async fn get_mmap(&self, path: &str) -> Result<Option<Arc<Mmap>>>;
    
    /// Get mmap for specific region only
    async fn get_regional_mmap(&self, path: &str, range: Range<u64>) -> Result<Option<Arc<Mmap>>>;
    
    /// Get current memory pressure (0.0 = no pressure, 1.0 = critical)
    fn get_memory_pressure(&self) -> f32;
    
    /// Hint about upcoming access pattern
    async fn hint_access_pattern(&self, path: &str, pattern: AccessPattern);
}

pub enum AccessPattern {
    Sequential,           // Will read whole file
    RandomAccess,        // Will jump around
    HotRegions(Vec<Range<u64>>), // Will repeatedly access these regions
    OnceOnly,           // One-time access, don't cache aggressively
}
```

## Configuration

```toml
[cache]
# Local disk cache settings
disk_cache_dir = "/var/cache/proximadb"
max_disk_cache_size = "100GB"
disk_cache_eviction_threshold = 0.9  # Start evicting at 90% full

# Memory mapping settings
max_mmap_size = "10GB"  # Total mmap size limit
mmap_pressure_threshold = 0.8  # Start being selective at 80% memory
critical_regions_always_map = true  # Always map bloom filters, indexes

# Cloud download optimization
min_chunk_size = "1MB"  # Don't download smaller chunks than this
max_concurrent_downloads = 4
download_full_threshold = "100MB"  # Download full file if smaller than this

# Per-engine settings
[cache.sst]
always_cache_ranges = ["0..4KB", "4KB..64KB"]  # Bloom + index
mmap_strategy = "selective"

[cache.parquet]
always_cache_footer = true
footer_size = "8MB"
cache_column_indexes = true
mmap_strategy = "adaptive"
```

## Key Benefits of This Design

1. **Cloud Cost Optimization**
   - Download once, reuse many times
   - Selective range downloads for large files
   - LRU eviction prevents unbounded disk usage

2. **Memory Pressure Resilience**
   - Graceful degradation under pressure
   - Critical regions prioritized
   - Fallback to streaming reads

3. **Access Pattern Optimization**
   - Hot regions stay in memory
   - Cold data streams from disk
   - Predictive prefetching based on patterns

4. **Reader Simplicity**
   - Readers focus on logic, not caching
   - Filesystem handles complexity
   - Clean fallback paths

## Implementation Phases

### Phase 1: Local Disk Cache (Week 1)
- Implement LocalDiskCache
- Add range-based downloads
- LRU eviction policy

### Phase 2: Memory Pressure Awareness (Week 2)
- Implement MmapManager
- Add pressure detection
- Selective region mapping

### Phase 3: Reader Integration (Week 3)
- Update Parquet reader
- Update SST reader
- Add access pattern hints

### Phase 4: Production Hardening (Week 4)
- Monitoring and metrics
- Performance tuning
- Documentation

## Open Questions Resolved

1. **Q: How to handle 100GB Parquet files?**
   A: Download only needed columns/ranges to local cache, then selectively mmap

2. **Q: What about memory pressure?**
   A: Three-tier degradation: Full mmap → Selective mmap → Streaming read

3. **Q: Cloud egress costs?**
   A: Local disk cache with LRU ensures download once, use many times

4. **Q: Bloom filter access from 1GB SST?**
   A: Download just first 4KB, keep permanently cached and mapped

5. **Q: Windows compatibility?**
   A: Disk cache works everywhere, mmap has Windows equivalent API