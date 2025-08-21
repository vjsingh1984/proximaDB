# Integrated Cache System Design Specification

## Executive Summary

The integrated cache system provides intelligent file-level caching for ProximaDB storage engines with automatic management, smart defaults, and proper handling of compaction granularity. The system maintains consistency between cloud storage, disk cache, and memory-mapped files while optimizing for recently flushed and compacted files.

## Core Design Principles

1. **File-Level Granularity**: Cache tracks individual files, not entire collections
2. **Compaction Awareness**: Only removes files being compacted, preserves others
3. **Atomic Operations**: Staging files created in cache directory for atomic moves
4. **Automatic Management**: Smart defaults with optional configuration
5. **Storage-Aware**: Different strategies for cloud vs local storage
6. **Metadata Hierarchy**: File-level and block-level metadata properly separated

## Architecture Overview

```
Cloud Storage (S3/GCS/Azure)
├── {base_url}/{collection_id}/{filename}
└── Atomic upload from staging

Disk Cache (Local)
├── {cache_dir}/{collection_id}/{filename}
├── Staging area for new files
└── Atomic move after flush/compaction

Memory Cache
├── File-level metadata (global)
│   ├── SST: File bloom filter, file index
│   └── Parquet: Footer, file statistics
└── Block-level metadata (per-block)
    ├── SST: Block bloom, block index
    └── Parquet: Page stats, column bloom
```

## Configuration

### Server Configuration (server.toml)

```toml
[cache]
# Enable/disable caching globally
enabled = true  # Default: true

# Base cache directory
cache_dir = "/var/cache/proximadb"  # Default: /tmp/proximadb_cache

# Maximum cache size
max_cache_size_gb = 100  # Default: 10% of disk space

# Memory cache settings
max_memory_cache_mb = 4096  # Default: 512MB per collection
enable_mmap = true  # Default: true for local files

# Disk cache settings
enable_disk_cache = true  # Default: true for cloud storage
compression = "lz4"  # Options: none, lz4, snappy, zstd

# Eviction policy
eviction_policy = "lru"  # Options: lru, lfu, fifo
eviction_threshold = 0.9  # Start evicting at 90% full
```

### Collection Configuration (in Collection metadata)

```rust
pub struct CollectionConfig {
    // ... existing fields ...
    
    /// Cache configuration for this collection
    pub cache_config: Option<CollectionCacheConfig>,
}

pub struct CollectionCacheConfig {
    /// Override global cache enable/disable
    pub enabled: Option<bool>,
    
    /// Maximum memory for this collection's metadata
    pub max_memory_mb: Option<usize>,
    
    /// Maximum disk cache for this collection
    pub max_disk_gb: Option<u64>,
    
    /// Prefer mmap for local files
    pub prefer_mmap: Option<bool>,
}
```

## Directory Structure

### Cloud Storage Layout
```
s3://bucket/proximadb/
├── {collection_id}/
│   ├── L0_001.sst
│   ├── L0_002.sst
│   ├── L1_001.sst
│   └── metadata.json
```

### Disk Cache Layout
```
/var/cache/proximadb/
├── {collection_id}/
│   ├── L0_001.sst          # Cached from cloud
│   ├── L0_002.sst          # Cached from cloud
│   ├── L1_001.sst.staging  # Being written (atomic)
│   └── .cache_index        # Cache metadata
```

### Memory Cache Structure
```rust
pub struct FileMetadataCache {
    /// File-level metadata (global, shared across blocks)
    file_metadata: DashMap<FileCacheKey, FileMetadata>,
    
    /// Block-level metadata (per-block, fine-grained)
    block_metadata: DashMap<BlockCacheKey, BlockMetadata>,
}

pub struct FileCacheKey {
    collection_id: String,
    filename: String,
}

pub struct BlockCacheKey {
    collection_id: String,
    filename: String,
    block_id: u32,  // or offset for Parquet
}
```

## Metadata Hierarchy

### SST Engine Metadata

```rust
/// File-level metadata (always in memory)
pub struct SstFileMetadata {
    /// Global bloom filter for entire file
    pub file_bloom_filter: Arc<BloomFilter>,
    
    /// File-level index (points to blocks)
    pub file_index: Arc<Vec<BlockPointer>>,
    
    /// Superblock metadata (for SWIFT)
    pub superblock_index: Option<Arc<SuperBlockIndex>>,
    
    /// File statistics
    pub stats: FileStats,
}

/// Block-level metadata (cached on demand)
pub struct SstBlockMetadata {
    /// Per-block bloom filter
    pub block_bloom: Arc<BloomFilter>,
    
    /// Block index (key ranges)
    pub block_index: Arc<BlockIndex>,
    
    /// Compression dictionary (if used)
    pub compression_dict: Option<Arc<Vec<u8>>>,
    
    /// Block statistics
    pub block_stats: BlockStats,
}
```

### Parquet Engine Metadata

```rust
/// File-level metadata (always in memory)
pub struct ParquetFileMetadata {
    /// Parquet footer (critical)
    pub footer: Arc<ParquetFooter>,
    
    /// File-level statistics
    pub file_stats: Arc<FileStatistics>,
    
    /// Row group metadata
    pub row_groups: Arc<Vec<RowGroupMetadata>>,
}

/// Page-level metadata (cached on demand)
pub struct ParquetPageMetadata {
    /// Column page statistics
    pub page_stats: Arc<PageStatistics>,
    
    /// Column bloom filters
    pub column_blooms: Arc<Vec<BloomFilter>>,
    
    /// Dictionary pages (if present)
    pub dictionaries: Option<Arc<DictionaryPage>>,
    
    /// Page indexes
    pub page_index: Arc<PageIndex>,
}
```

## Flush and Compaction Integration

### Flush Operation Flow

```rust
async fn flush_to_storage(
    collection_id: &str,
    memtable_data: Vec<VectorRecord>,
) -> Result<()> {
    let cache_dir = get_cache_dir();
    let filename = generate_filename("L0", sequence_number);
    
    // Step 1: Create staging file in cache directory (for atomic move)
    let staging_path = format!("{}/{}/{}.staging", 
        cache_dir, collection_id, filename);
    
    // Step 2: Write data to staging file
    write_sst_file(&staging_path, memtable_data)?;
    
    // Step 3: Move staging file to final location (atomic)
    let final_cache_path = format!("{}/{}/{}", 
        cache_dir, collection_id, filename);
    fs::rename(&staging_path, &final_cache_path)?;
    
    // Step 4: Upload to cloud storage if needed
    if is_cloud_storage(base_url) {
        // Upload from cache (don't delete cache file)
        upload_to_cloud(&final_cache_path, &cloud_path).await?;
        
        // Cache file remains for future reads
        update_cache_index(collection_id, &filename, CacheStatus::Fresh);
    } else {
        // For local storage, just use the file directly
        // No separate cache needed
    }
    
    // Step 5: Update metadata cache
    update_metadata_cache(collection_id, &filename, metadata);
    
    Ok(())
}
```

### Compaction Operation Flow

```rust
async fn compact_files(
    collection_id: &str,
    input_files: Vec<String>,  // Files being compacted
    level: CompactionLevel,
) -> Result<()> {
    let cache_dir = get_cache_dir();
    let output_filename = generate_compaction_output_name(level);
    
    // Step 1: Read input files (from cache if available)
    let mut readers = Vec::new();
    for filename in &input_files {
        let reader = get_cached_reader(collection_id, filename).await?;
        readers.push(reader);
    }
    
    // Step 2: Create output file in cache directory
    let staging_path = format!("{}/{}/{}.staging",
        cache_dir, collection_id, output_filename);
    
    // Step 3: Perform compaction
    compact_to_file(&staging_path, readers)?;
    
    // Step 4: Move to final location (atomic)
    let final_cache_path = format!("{}/{}/{}",
        cache_dir, collection_id, output_filename);
    fs::rename(&staging_path, &final_cache_path)?;
    
    // Step 5: Upload to cloud if needed
    if is_cloud_storage(base_url) {
        upload_to_cloud(&final_cache_path, &cloud_path).await?;
    }
    
    // Step 6: Remove ONLY compacted files from cache
    for filename in input_files {
        // Remove from disk cache
        let cache_path = format!("{}/{}/{}",
            cache_dir, collection_id, filename);
        fs::remove_file(&cache_path).ok();
        
        // Remove from memory cache
        evict_file_metadata(collection_id, &filename);
        
        // Remove from cloud storage
        if is_cloud_storage(base_url) {
            delete_from_cloud(collection_id, &filename).await?;
        }
    }
    
    // Step 7: Update cache with new file
    update_cache_index(collection_id, &output_filename, CacheStatus::Fresh);
    
    Ok(())
}
```

### Delete Operation Flow

```rust
async fn delete_file(
    collection_id: &str,
    filename: &str,
) -> Result<()> {
    // Step 1: Delete from cloud storage
    if is_cloud_storage(base_url) {
        delete_from_cloud(collection_id, filename).await?;
    }
    
    // Step 2: Delete from disk cache
    let cache_path = format!("{}/{}/{}",
        get_cache_dir(), collection_id, filename);
    fs::remove_file(&cache_path).ok();
    
    // Step 3: Remove from memory cache
    evict_file_metadata(collection_id, filename);
    evict_block_metadata(collection_id, filename);
    
    // Step 4: Remove mmap if exists
    unmap_file(collection_id, filename);
    
    Ok(())
}
```

## Memory Mapping Strategy

### When to Use mmap

```rust
fn should_use_mmap(file_path: &str, file_size: u64) -> bool {
    // Don't mmap cloud files
    if is_cloud_url(file_path) {
        return false;
    }
    
    // Don't mmap tiny files (< 1MB)
    if file_size < 1024 * 1024 {
        return false;
    }
    
    // Don't mmap huge files (> 1GB) unless enough memory
    if file_size > 1024 * 1024 * 1024 {
        let available_memory = get_available_memory();
        if available_memory < file_size * 2 {
            return false;
        }
    }
    
    // Check if file is on fast local storage
    if is_on_ssd(file_path) || is_on_nvme(file_path) {
        return true;
    }
    
    // For EBS, only mmap if frequently accessed
    if is_on_ebs(file_path) {
        return is_hot_file(file_path);
    }
    
    false
}
```

### mmap Cache Structure

```rust
pub struct MmapCache {
    /// Partition by collection for efficient lookup
    collections: DashMap<String, CollectionMmapCache>,
}

pub struct CollectionMmapCache {
    /// File-level mmaps
    files: DashMap<String, Arc<MmappedFile>>,
    
    /// Total memory used by this collection
    memory_used: AtomicUsize,
}

pub struct MmappedFile {
    /// File path
    path: String,
    
    /// Memory map
    mmap: Arc<Mmap>,
    
    /// File metadata (cached)
    file_metadata: Arc<FileMetadata>,
    
    /// Block metadata (lazy loaded)
    block_metadata: DashMap<u32, Arc<BlockMetadata>>,
    
    /// Access tracking
    last_access: AtomicU64,
    access_count: AtomicU32,
}
```

## Cache Eviction Strategy

### File-Level Eviction

```rust
async fn evict_files_if_needed(
    collection_id: &str,
    required_space: u64,
) -> Result<()> {
    let cache_stats = get_cache_stats(collection_id);
    
    if cache_stats.used_space + required_space > cache_stats.max_space {
        // Get eviction candidates (exclude recently flushed/compacted)
        let candidates = get_eviction_candidates(collection_id);
        
        // Sort by eviction priority
        let sorted = match eviction_policy {
            EvictionPolicy::LRU => sort_by_last_access(candidates),
            EvictionPolicy::LFU => sort_by_access_count(candidates),
            EvictionPolicy::FIFO => sort_by_creation_time(candidates),
        };
        
        // Evict until enough space
        let mut freed = 0u64;
        for file in sorted {
            // Skip if recently created (< 5 minutes)
            if file.age() < Duration::from_secs(300) {
                continue;
            }
            
            evict_file(collection_id, &file.name).await?;
            freed += file.size;
            
            if freed >= required_space {
                break;
            }
        }
    }
    
    Ok(())
}
```

## Smart Defaults

### Memory Cache Defaults

```rust
fn get_default_memory_cache_size() -> usize {
    let total_memory = get_system_memory();
    
    // Use 5% of system memory for metadata cache
    let metadata_cache = (total_memory as f64 * 0.05) as usize;
    
    // Minimum 256MB, maximum 4GB
    metadata_cache.max(256 * 1024 * 1024)
                  .min(4 * 1024 * 1024 * 1024)
}
```

### Disk Cache Defaults

```rust
fn get_default_disk_cache_size() -> u64 {
    let cache_dir = get_cache_dir();
    let available_space = get_available_disk_space(&cache_dir);
    
    // Use 10% of available disk space
    let cache_size = (available_space as f64 * 0.10) as u64;
    
    // Minimum 1GB, maximum 100GB
    cache_size.max(1024 * 1024 * 1024)
              .min(100 * 1024 * 1024 * 1024)
}
```

## Implementation Checklist

### Phase 1: Core Infrastructure
- [ ] Implement file-level cache tracking
- [ ] Add staging directory support
- [ ] Implement atomic file operations
- [ ] Add cache index persistence

### Phase 2: Compaction Integration
- [ ] Track compaction input files
- [ ] Implement selective file eviction
- [ ] Add output file caching
- [ ] Update delete operations

### Phase 3: Memory Management
- [ ] Implement two-level metadata cache
- [ ] Add mmap management
- [ ] Implement memory pressure handling
- [ ] Add access tracking

### Phase 4: Optimization
- [ ] Add prefetching for sequential access
- [ ] Implement adaptive eviction
- [ ] Add compression for cold data
- [ ] Optimize for different storage types

## Benefits

1. **Consistency**: Cache always reflects actual storage state
2. **Efficiency**: Recently created files stay cached
3. **Atomic Operations**: No partial states during flush/compaction
4. **Granular Control**: File-level tracking allows precise eviction
5. **Smart Defaults**: Works well out-of-the-box
6. **Storage Aware**: Optimizes for cloud vs local storage

## Example Usage

### Basic Configuration

```toml
# server.toml
[cache]
enabled = true
cache_dir = "/mnt/fast-ssd/cache"
max_cache_size_gb = 50
```

### Collection Override

```rust
let collection = Collection {
    id: "vectors_2024",
    cache_config: Some(CollectionCacheConfig {
        enabled: Some(true),
        max_memory_mb: Some(1024),
        max_disk_gb: Some(20),
        prefer_mmap: Some(true),
    }),
    // ... other fields
};
```

### Programmatic Control

```rust
// Get cache handle
let cache = IntegratedCache::new(config)?;

// Manually evict files if needed
cache.evict_files("collection_1", vec!["L0_001.sst"])?;

// Prefetch files for upcoming operation
cache.prefetch_files("collection_1", vec!["L1_001.sst"])?;

// Get cache statistics
let stats = cache.get_stats("collection_1")?;
println!("Cache hit rate: {:.2}%", stats.hit_rate * 100.0);
```

## Monitoring and Metrics

### Key Metrics to Track

```rust
pub struct CacheMetrics {
    // Hit rates
    pub metadata_hit_rate: f64,
    pub block_hit_rate: f64,
    pub disk_cache_hit_rate: f64,
    
    // Space usage
    pub memory_used_mb: usize,
    pub disk_used_gb: u64,
    pub mmap_count: usize,
    
    // Operations
    pub files_evicted: u64,
    pub files_cached: u64,
    pub staging_operations: u64,
    
    // Performance
    pub avg_read_latency_ms: f64,
    pub cache_miss_latency_ms: f64,
}
```

## Conclusion

This integrated cache design provides:
- Automatic cache management with smart defaults
- Proper handling of compaction granularity
- Atomic operations for consistency
- Efficient memory and disk usage
- Storage-aware optimization
- Simple configuration with powerful overrides

The system ensures that recently flushed and compacted files remain cached while properly cleaning up only the files that are actually deleted during compaction, maintaining perfect consistency between cloud storage and local caches.