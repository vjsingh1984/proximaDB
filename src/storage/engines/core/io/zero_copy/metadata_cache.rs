// Zero-Copy Metadata Cache Implementation
// Ultra-fast metadata access via memory-mapped files with magic bytes identification

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use memmap2::{Mmap, MmapOptions};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::{debug, info, trace, warn};

use super::MAGIC_BYTES;
use super::traits::{DataRange, EngineMetadata, MetadataSerializer, QueryContext};
use crate::core::error::ProximaDBError;

/// Fixed-size cache file header (bytemuck compatible)
#[repr(C)]
#[derive(Copy, Clone, Debug)]
#[allow(dead_code)]
pub struct CacheFileHeader {
    /// Magic bytes for file identification: b"PXMDCHV1"
    pub magic: [u8; 8],
    /// Cache format version
    pub version: u32,
    /// Engine type hash for validation
    pub engine_hash: u32,
    /// Original source file size
    pub original_file_size: u64,
    /// Size of metadata payload
    pub metadata_size: u32,
    /// Unix timestamp of creation
    pub created_at: u64,
    /// Hash of original file path for verification
    pub file_path_hash: u64,
    /// Compression and feature flags
    pub compression_flags: u32,
    /// Reserved for future expansion
    pub reserved: [u32; 4],
}

#[allow(dead_code)]
impl CacheFileHeader {
    #[allow(dead_code)]
    pub fn new(
        engine_hash: u32,
        original_file_size: u64,
        metadata_size: u32,
        file_path_hash: u64,
        compression_flags: u32,
    ) -> Self {
        Self {
            magic: *MAGIC_BYTES,
            version: 1,
            engine_hash,
            original_file_size,
            metadata_size,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            file_path_hash,
            compression_flags,
            reserved: [0; 4],
        }
    }

    /// Validate magic bytes and version
    #[allow(dead_code)]
    pub fn is_valid(&self) -> bool {
        self.magic == *MAGIC_BYTES && self.version <= 1
    }

    /// Check if the header matches expected file properties
    #[allow(dead_code)]
    pub fn matches_file(&self, engine_hash: u32, file_path_hash: u64) -> bool {
        self.is_valid() && self.engine_hash == engine_hash && self.file_path_hash == file_path_hash
    }
}

/// Memory-mapped metadata with zero-copy access
#[allow(dead_code)]
pub struct MmappedMetadata {
    /// Memory-mapped file
    mmap: Mmap,
    /// Parsed header
    header: CacheFileHeader,
    /// Deserialized metadata (lazy-loaded)
    metadata: RwLock<Option<Arc<Box<dyn EngineMetadata>>>>,
    /// Serializer for this engine type
    serializer: Arc<dyn MetadataSerializer>,
}

#[allow(dead_code)]
impl MmappedMetadata {
    /// Create from memory-mapped file
    #[allow(dead_code)]
    pub fn from_mmap(
        mmap: Mmap,
        serializer: Arc<dyn MetadataSerializer>,
    ) -> Result<Self, ProximaDBError> {
        if mmap.len() < std::mem::size_of::<CacheFileHeader>() {
            return Err(ProximaDBError::InvalidInput(
                "Cache file too small for header".into(),
            ));
        }

        let header = unsafe { std::ptr::read(mmap.as_ptr() as *const CacheFileHeader) };

        if !header.is_valid() {
            return Err(ProximaDBError::InvalidInput(
                "Invalid cache file header".into(),
            ));
        }

        Ok(Self {
            mmap,
            header,
            metadata: RwLock::new(None),
            serializer,
        })
    }

    /// Get cached metadata, deserializing if needed
    #[allow(dead_code)]
    pub fn get_metadata(&self) -> Result<Arc<Box<dyn EngineMetadata>>, ProximaDBError> {
        // Fast path: already deserialized
        {
            let guard = self.metadata.read();
            if let Some(ref metadata) = *guard {
                return Ok(Arc::clone(metadata));
            }
        }

        // Slow path: deserialize from memory
        let mut guard = self.metadata.write();
        if guard.is_none() {
            let header_size = std::mem::size_of::<CacheFileHeader>();
            let payload_start = header_size;
            let payload_end = payload_start + self.header.metadata_size as usize;

            if payload_end > self.mmap.len() {
                return Err(ProximaDBError::InvalidInput(
                    "Cache file payload extends beyond file size".into(),
                ));
            }

            let payload = &self.mmap[payload_start..payload_end];
            let metadata = self.serializer.deserialize_metadata(payload)?;
            let shared_metadata = Arc::new(metadata);
            *guard = Some(Arc::clone(&shared_metadata));

            trace!(
                engine = self.serializer.engine_id(),
                metadata_size = self.header.metadata_size,
                "Deserialized metadata from cache"
            );

            return Ok(shared_metadata);
        }

        if let Some(metadata) = guard.as_ref() {
            Ok(Arc::clone(metadata))
        } else {
            Err(ProximaDBError::Internal(
                "Metadata cache deserialization yielded no value".into(),
            ))
        }
    }

    /// Check if file can be skipped for given query
    #[allow(dead_code)]
    pub fn can_skip_file(&self, query_context: &QueryContext) -> Result<bool, ProximaDBError> {
        let metadata = self.get_metadata()?;
        Ok(self
            .serializer
            .can_skip_file(metadata.as_ref().as_ref(), query_context))
    }

    /// Get required data ranges for selective reading
    #[allow(dead_code)]
    pub fn get_required_ranges(
        &self,
        query_context: &QueryContext,
    ) -> Result<Option<Vec<DataRange>>, ProximaDBError> {
        let metadata = self.get_metadata()?;
        Ok(self
            .serializer
            .get_required_ranges(metadata.as_ref().as_ref(), query_context))
    }

    /// Get cache file header
    #[allow(dead_code)]
    pub fn header(&self) -> &CacheFileHeader {
        &self.header
    }

    /// Get memory footprint
    #[allow(dead_code)]
    pub fn memory_footprint(&self) -> usize {
        self.mmap.len() + std::mem::size_of::<Self>()
    }
}

/// Cache statistics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStatistics {
    pub hits: u64,
    pub misses: u64,
    pub files_skipped: u64,
    pub bytes_saved_by_skipping: u64,
    pub total_entries: usize,
    pub memory_usage_bytes: usize,
    pub serialization_time_total_ms: f64,
    pub deserialization_time_total_ms: f64,
    pub evictions: u64,
    pub invalidations: u64,
}

impl CacheStatistics {
    #[allow(dead_code)]
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    #[allow(dead_code)]
    pub fn avg_serialization_time_ms(&self) -> f64 {
        if self.hits + self.misses == 0 {
            0.0
        } else {
            self.serialization_time_total_ms / (self.hits + self.misses) as f64
        }
    }
}

/// Zero-copy metadata cache with mmap-based storage
#[allow(dead_code)]
pub struct ZeroCopyMetadataCache {
    /// Cache directory for metadata files
    cache_dir: PathBuf,
    /// Engine-specific serializers
    serializers: DashMap<String, Arc<dyn MetadataSerializer>>,
    /// Memory-mapped metadata cache
    mmap_cache: DashMap<String, Arc<MmappedMetadata>>,
    /// Cache statistics
    stats: RwLock<CacheStatistics>,
    /// Cache configuration
    max_memory_bytes: usize,
    max_entries: usize,
    enable_compression: bool,
}

#[allow(dead_code)]
impl ZeroCopyMetadataCache {
    /// Create new cache with configuration
    pub async fn new(
        cache_dir: PathBuf,
        max_memory_bytes: usize,
        max_entries: usize,
        enable_compression: bool,
    ) -> Result<Self, ProximaDBError> {
        // Ensure cache directory exists
        fs::create_dir_all(&cache_dir).await.map_err(|e| {
            ProximaDBError::Internal(format!("Failed to create cache directory: {}", e))
        })?;

        Ok(Self {
            cache_dir,
            serializers: DashMap::new(),
            mmap_cache: DashMap::new(),
            stats: RwLock::new(CacheStatistics::default()),
            max_memory_bytes,
            max_entries,
            enable_compression,
        })
    }

    /// Register engine-specific metadata serializer
    pub fn register_serializer(&self, serializer: Arc<dyn MetadataSerializer>) {
        let engine_id = serializer.engine_id().to_string();
        debug!(
            engine_id = engine_id.as_str(),
            "Registered metadata serializer"
        );
        self.serializers.insert(engine_id, serializer);
    }

    /// Get metadata for file, using cache or creating new entry
    pub async fn get_metadata(
        &self,
        file_path: &str,
        collection_id: &str,
        engine_type: &str,
    ) -> Result<Arc<MmappedMetadata>, ProximaDBError> {
        let cache_key = self.create_cache_key(file_path, collection_id, engine_type);

        // Check if already in memory cache
        if let Some(cached) = self.mmap_cache.get(&cache_key) {
            self.record_hit();
            trace!(file_path, collection_id, engine_type, "Cache hit");
            return Ok(Arc::clone(&cached));
        }

        self.record_miss();

        // Get serializer for this engine
        let serializer = self.serializers.get(engine_type).ok_or_else(|| {
            ProximaDBError::Config(format!(
                "No serializer registered for engine: {}",
                engine_type
            ))
        })?;
        let serializer = Arc::clone(&serializer);

        // Check disk cache
        let cache_file_path = self.get_cache_file_path(&cache_key);
        if let Ok(mmap_metadata) = self
            .load_from_disk(&cache_file_path, Arc::clone(&serializer))
            .await
        {
            // Validate cache file matches current file
            let file_path_hash = self.hash_string(file_path);
            let engine_hash = self.hash_string(engine_type);

            if mmap_metadata
                .header()
                .matches_file(engine_hash as u32, file_path_hash)
            {
                let cached = Arc::new(mmap_metadata);
                self.mmap_cache.insert(cache_key, Arc::clone(&cached));
                trace!(
                    file_path,
                    collection_id, engine_type, "Loaded from disk cache"
                );
                return Ok(cached);
            } else {
                debug!(file_path, "Cache file invalid, regenerating");
            }
        }

        // Create new cache entry
        let metadata = self
            .create_cache_entry(
                file_path,
                collection_id,
                engine_type,
                &cache_key,
                Arc::clone(&serializer),
            )
            .await?;

        let cached = Arc::new(metadata);
        self.mmap_cache.insert(cache_key, Arc::clone(&cached));

        // Check if we need to evict entries
        self.enforce_cache_limits().await;

        Ok(cached)
    }

    /// Check if entire file can be skipped based on cached metadata
    pub async fn can_skip_file(
        &self,
        file_path: &str,
        collection_id: &str,
        engine_type: &str,
        query_context: &QueryContext,
    ) -> Result<bool, ProximaDBError> {
        let metadata = self
            .get_metadata(file_path, collection_id, engine_type)
            .await?;
        let can_skip = metadata.can_skip_file(query_context)?;

        if can_skip {
            let mut stats = self.stats.write();
            stats.files_skipped += 1;
            // Estimate bytes saved (would need actual file size)
            stats.bytes_saved_by_skipping += metadata.header().original_file_size;
        }

        Ok(can_skip)
    }

    /// Get required data ranges for selective reading
    pub async fn get_required_ranges(
        &self,
        file_path: &str,
        collection_id: &str,
        engine_type: &str,
        query_context: &QueryContext,
    ) -> Result<Option<Vec<DataRange>>, ProximaDBError> {
        let metadata = self
            .get_metadata(file_path, collection_id, engine_type)
            .await?;
        metadata.get_required_ranges(query_context)
    }

    /// Invalidate all cache entries for a collection
    pub async fn invalidate_collection(&self, collection_id: &str) -> Result<u64, ProximaDBError> {
        let mut invalidated = 0u64;
        let collection_prefix = format!("{}:", collection_id);

        // Remove from memory cache
        self.mmap_cache.retain(|key, _| {
            if key.contains(&collection_prefix) {
                invalidated += 1;
                false
            } else {
                true
            }
        });

        // Remove from disk cache
        let mut read_dir = fs::read_dir(&self.cache_dir).await.map_err(|e| {
            ProximaDBError::Internal(format!("Failed to read cache directory: {}", e))
        })?;

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            ProximaDBError::Internal(format!("Failed to read directory entry: {}", e))
        })? {
            let file_name = entry.file_name();
            if let Some(name_str) = file_name.to_str() {
                if name_str.contains(&collection_prefix) {
                    if let Err(e) = fs::remove_file(entry.path()).await {
                        warn!(path = ?entry.path(), error = %e, "Failed to remove cache file");
                    }
                }
            }
        }

        {
            let mut stats = self.stats.write();
            stats.invalidations += invalidated;
        }

        info!(collection_id, invalidated, "Invalidated collection cache");
        Ok(invalidated)
    }

    /// Get cache statistics
    pub fn get_statistics(&self) -> CacheStatistics {
        let mut stats = self.stats.read().clone();
        stats.total_entries = self.mmap_cache.len();
        stats.memory_usage_bytes = self.calculate_memory_usage();
        stats
    }

    /// Create cache key from file path, collection, and engine
    fn create_cache_key(&self, file_path: &str, collection_id: &str, engine_type: &str) -> String {
        format!(
            "{}:{}:{}",
            collection_id,
            engine_type,
            self.hash_string(file_path)
        )
    }

    /// Get cache file path for key
    fn get_cache_file_path(&self, cache_key: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.cache", cache_key))
    }

    /// Hash string for cache keys
    fn hash_string(&self, s: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    /// Load metadata from disk cache
    async fn load_from_disk(
        &self,
        cache_file_path: &Path,
        serializer: Arc<dyn MetadataSerializer>,
    ) -> Result<MmappedMetadata, ProximaDBError> {
        let file = File::open(cache_file_path).map_err(|_| {
            ProximaDBError::Storage(crate::core::error::StorageError::NotFound(
                "Cache file not found".into(),
            ))
        })?;

        let mmap = unsafe {
            MmapOptions::new().map(&file).map_err(|e| {
                ProximaDBError::Internal(format!("Failed to mmap cache file: {}", e))
            })?
        };

        MmappedMetadata::from_mmap(mmap, serializer)
    }

    /// Create new cache entry
    async fn create_cache_entry(
        &self,
        file_path: &str,
        collection_id: &str,
        engine_type: &str,
        cache_key: &str,
        serializer: Arc<dyn MetadataSerializer>,
    ) -> Result<MmappedMetadata, ProximaDBError> {
        let start_time = std::time::Instant::now();

        // Serialize metadata
        let metadata_bytes = serializer.serialize_metadata(file_path, collection_id)?;

        let serialization_time = start_time.elapsed().as_secs_f64() * 1000.0;
        {
            let mut stats = self.stats.write();
            stats.serialization_time_total_ms += serialization_time;
        }

        // Create cache file header
        let file_path_hash = self.hash_string(file_path);
        let engine_hash = self.hash_string(engine_type);
        let compression_flags = if self.enable_compression { 1 } else { 0 };

        let header = CacheFileHeader::new(
            engine_hash as u32,
            0, // Would need actual file size from filesystem
            metadata_bytes.len() as u32,
            file_path_hash,
            compression_flags,
        );

        // Write cache file
        let cache_file_path = self.get_cache_file_path(cache_key);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&cache_file_path)
            .map_err(|e| ProximaDBError::Internal(format!("Failed to create cache file: {}", e)))?;

        // Write header manually (serialize each field)
        file.write_all(&header.magic).map_err(|e| {
            ProximaDBError::Internal(format!("Failed to write cache header magic: {}", e))
        })?;
        file.write_all(&header.version.to_le_bytes()).map_err(|e| {
            ProximaDBError::Internal(format!("Failed to write cache header version: {}", e))
        })?;
        file.write_all(&header.engine_hash.to_le_bytes())
            .map_err(|e| {
                ProximaDBError::Internal(format!("Failed to write cache header engine_hash: {}", e))
            })?;
        file.write_all(&header.original_file_size.to_le_bytes())
            .map_err(|e| {
                ProximaDBError::Internal(format!("Failed to write cache header file_size: {}", e))
            })?;
        file.write_all(&header.metadata_size.to_le_bytes())
            .map_err(|e| {
                ProximaDBError::Internal(format!(
                    "Failed to write cache header metadata_size: {}",
                    e
                ))
            })?;
        file.write_all(&header.created_at.to_le_bytes())
            .map_err(|e| {
                ProximaDBError::Internal(format!("Failed to write cache header created_at: {}", e))
            })?;
        file.write_all(&header.file_path_hash.to_le_bytes())
            .map_err(|e| {
                ProximaDBError::Internal(format!(
                    "Failed to write cache header file_path_hash: {}",
                    e
                ))
            })?;
        file.write_all(&header.compression_flags.to_le_bytes())
            .map_err(|e| {
                ProximaDBError::Internal(format!(
                    "Failed to write cache header compression_flags: {}",
                    e
                ))
            })?;
        // Write reserved fields
        for reserved_val in &header.reserved {
            file.write_all(&reserved_val.to_le_bytes()).map_err(|e| {
                ProximaDBError::Internal(format!("Failed to write cache header reserved: {}", e))
            })?;
        }

        // Write metadata payload
        file.write_all(&metadata_bytes).map_err(|e| {
            ProximaDBError::Internal(format!("Failed to write cache payload: {}", e))
        })?;

        file.sync_all()
            .map_err(|e| ProximaDBError::Internal(format!("Failed to sync cache file: {}", e)))?;

        // Memory map the file
        let file = File::open(&cache_file_path)
            .map_err(|e| ProximaDBError::Internal(format!("Failed to reopen cache file: {}", e)))?;

        let mmap = unsafe {
            MmapOptions::new().map(&file).map_err(|e| {
                ProximaDBError::Internal(format!("Failed to mmap new cache file: {}", e))
            })?
        };

        debug!(
            file_path,
            collection_id,
            engine_type,
            cache_size = metadata_bytes.len(),
            serialization_time_ms = serialization_time,
            "Created new cache entry"
        );

        MmappedMetadata::from_mmap(mmap, serializer)
    }

    /// Record cache hit
    fn record_hit(&self) {
        self.stats.write().hits += 1;
    }

    /// Record cache miss
    fn record_miss(&self) {
        self.stats.write().misses += 1;
    }

    /// Calculate total memory usage
    fn calculate_memory_usage(&self) -> usize {
        self.mmap_cache
            .iter()
            .map(|entry| entry.value().memory_footprint())
            .sum()
    }

    /// Enforce cache size and entry limits
    async fn enforce_cache_limits(&self) {
        let current_memory = self.calculate_memory_usage();
        let current_entries = self.mmap_cache.len();

        if current_memory <= self.max_memory_bytes && current_entries <= self.max_entries {
            return;
        }

        // Simple LRU eviction - remove oldest entries
        // In a full implementation, this would use access time tracking
        let entries_to_remove = if current_entries > self.max_entries {
            current_entries - self.max_entries
        } else {
            // Estimate entries to remove based on memory
            (current_entries * 20) / 100 // Remove 20%
        };

        let mut removed = 0;
        let mut keys_to_remove = Vec::new();

        for entry in self.mmap_cache.iter().take(entries_to_remove) {
            keys_to_remove.push(entry.key().clone());
        }

        for key in keys_to_remove {
            if self.mmap_cache.remove(&key).is_some() {
                removed += 1;
            }
        }

        if removed > 0 {
            let mut stats = self.stats.write();
            stats.evictions += removed as u64;

            info!(
                removed,
                current_memory, current_entries, "Evicted cache entries due to limits"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cache_file_header() {
        let header = CacheFileHeader::new(123, 1000, 500, 456, 0);
        assert!(header.is_valid());
        assert_eq!(header.engine_hash, 123);
        assert_eq!(header.original_file_size, 1000);
        assert_eq!(header.metadata_size, 500);
        assert!(header.matches_file(123, 456));
        assert!(!header.matches_file(124, 456));
    }

    #[tokio::test]
    async fn test_cache_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache = ZeroCopyMetadataCache::new(
            temp_dir.path().to_path_buf(),
            1024 * 1024, // 1MB
            1000,
            false,
        )
        .await
        .unwrap();

        assert_eq!(cache.get_statistics().total_entries, 0);
        assert_eq!(cache.get_statistics().hits, 0);
        assert_eq!(cache.get_statistics().misses, 0);
    }

    #[test]
    fn test_cache_key_generation() {
        let temp_dir = TempDir::new().unwrap();
        let cache = ZeroCopyMetadataCache {
            cache_dir: temp_dir.path().to_path_buf(),
            serializers: DashMap::new(),
            mmap_cache: DashMap::new(),
            stats: RwLock::new(CacheStatistics::default()),
            max_memory_bytes: 1024,
            max_entries: 100,
            enable_compression: false,
        };

        let key1 = cache.create_cache_key("/path/to/file1.sst", "collection1", "SST");
        let key2 = cache.create_cache_key("/path/to/file2.sst", "collection1", "SST");
        let key3 = cache.create_cache_key("/path/to/file1.sst", "collection2", "SST");

        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
        assert!(key1.contains("collection1"));
        assert!(key3.contains("collection2"));
    }
}
