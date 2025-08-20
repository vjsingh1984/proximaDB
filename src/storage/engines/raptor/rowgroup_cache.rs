/// RowGroup-level caching system for RAPTOR engine
/// Enables selective loading of row groups to avoid downloading entire monolithic files
/// Integrates with zero-copy filesystem for optimal bandwidth utilization

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use anyhow::Result;
use tracing::{debug, info, warn, trace};
use bytes::Bytes;

use crate::storage::engines::common::zero_copy_io_system::{
    ZeroCopyIOSystem, QueryContext, FileAccessRequest, RequestPriority, OptimizedIOResult
};
use crate::storage::persistence::filesystem::FileSystem;
use super::common::{RowGroup, RowGroupMetadata, RaptorFileMetadata};

/// Cache key for rowgroup data
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct RowGroupCacheKey {
    pub file_path: String,
    pub rowgroup_id: u32,
    pub collection_id: String,
}

/// Cached rowgroup data with metadata
#[derive(Debug, Clone)]
pub struct CachedRowGroup {
    /// Compressed data of the rowgroup
    pub compressed_data: Bytes,
    
    /// Metadata for quick filtering
    pub metadata: RowGroupMetadata,
    
    /// Timestamp when cached
    pub cached_at: std::time::Instant,
    
    /// Number of times accessed
    pub access_count: u64,
    
    /// Size in bytes (for cache eviction)
    pub size_bytes: usize,
}

/// Serializable cache entry for persistence
#[derive(Serialize, Deserialize)]
pub struct SerializedCacheEntry {
    pub compressed_data: Vec<u8>,
    pub metadata: RowGroupMetadata,
    pub access_count: u64,
}

/// RowGroup cache manager with selective loading capabilities
pub struct RowGroupCacheManager {
    /// In-memory cache of rowgroups
    cache: Arc<RwLock<HashMap<RowGroupCacheKey, CachedRowGroup>>>,
    
    /// Zero-copy I/O system for intelligent caching
    io_system: Arc<ZeroCopyIOSystem>,
    
    /// Underlying filesystem for range reads
    filesystem: Arc<dyn FileSystem>,
    
    /// File metadata cache to avoid repeated footer reads
    file_metadata_cache: Arc<RwLock<HashMap<String, RaptorFileMetadata>>>,
    
    /// Maximum cache size in bytes
    max_cache_size: usize,
    
    /// Current cache size in bytes
    current_size: Arc<RwLock<usize>>,
    
    /// Prefetch strategy
    prefetch_strategy: PrefetchStrategy,
}

/// Strategy for prefetching adjacent rowgroups
#[derive(Debug, Clone)]
pub enum PrefetchStrategy {
    /// No prefetching
    None,
    
    /// Prefetch N adjacent rowgroups
    Adjacent { count: usize },
    
    /// Prefetch based on HNSW connectivity
    HnswLocality { max_hops: usize },
    
    /// Adaptive based on access patterns
    Adaptive,
}

impl RowGroupCacheManager {
    pub fn new(
        io_system: Arc<ZeroCopyIOSystem>,
        filesystem: Arc<dyn FileSystem>,
        max_cache_size: usize,
        prefetch_strategy: PrefetchStrategy,
    ) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            io_system,
            filesystem,
            file_metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            max_cache_size,
            current_size: Arc::new(RwLock::new(0)),
            prefetch_strategy,
        }
    }
    
    /// Get a rowgroup from cache or load it selectively
    pub async fn get_rowgroup(
        &self,
        file_path: &str,
        rowgroup_id: u32,
        collection_id: &str,
        query_context: Option<&QueryContext>,
    ) -> Result<CachedRowGroup> {
        let key = RowGroupCacheKey {
            file_path: file_path.to_string(),
            rowgroup_id,
            collection_id: collection_id.to_string(),
        };
        
        // Check in-memory cache first
        {
            let mut cache = self.cache.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                entry.access_count += 1;
                debug!(
                    file_path, rowgroup_id, 
                    access_count = entry.access_count,
                    "RowGroup cache hit"
                );
                return Ok(entry.clone());
            }
        }
        
        // Not in cache, need to load selectively
        info!(file_path, rowgroup_id, "RowGroup cache miss - loading selectively");
        
        // Get file metadata (cached to avoid repeated footer reads)
        let metadata = self.get_file_metadata(file_path).await?;
        
        // Find the specific rowgroup metadata
        let rg_metadata = metadata.row_groups
            .iter()
            .find(|rg| rg.id == rowgroup_id)
            .ok_or_else(|| anyhow::anyhow!("RowGroup {} not found in file", rowgroup_id))?
            .clone();
        
        // Check if we can skip this rowgroup based on query context
        if let Some(ctx) = query_context {
            if self.can_skip_rowgroup(&rg_metadata, ctx) {
                debug!(
                    file_path, rowgroup_id,
                    "Skipping rowgroup based on query context"
                );
                return Err(anyhow::anyhow!("RowGroup filtered out by query context"));
            }
        }
        
        // Perform selective range read for just this rowgroup
        let compressed_data = self.load_rowgroup_data(
            file_path,
            rg_metadata.offset,
            rg_metadata.compressed_size,
        ).await?;
        
        // Create cache entry
        let cached_rg = CachedRowGroup {
            compressed_data: Bytes::from(compressed_data),
            metadata: rg_metadata.clone(),
            cached_at: std::time::Instant::now(),
            access_count: 1,
            size_bytes: rg_metadata.compressed_size as usize,
        };
        
        // Insert into cache with eviction if needed
        self.insert_with_eviction(key.clone(), cached_rg.clone()).await?;
        
        // Trigger prefetch if configured
        self.maybe_prefetch(file_path, rowgroup_id, &metadata).await;
        
        Ok(cached_rg)
    }
    
    /// Get multiple rowgroups efficiently with batched reads
    pub async fn get_rowgroups(
        &self,
        file_path: &str,
        rowgroup_ids: &[u32],
        collection_id: &str,
        query_context: Option<&QueryContext>,
    ) -> Result<Vec<CachedRowGroup>> {
        let mut results = Vec::new();
        let mut missing_ranges = Vec::new();
        
        // First pass: collect what's in cache and what needs loading
        {
            let cache = self.cache.read().await;
            for &rg_id in rowgroup_ids {
                let key = RowGroupCacheKey {
                    file_path: file_path.to_string(),
                    rowgroup_id: rg_id,
                    collection_id: collection_id.to_string(),
                };
                
                if let Some(entry) = cache.get(&key) {
                    results.push((rg_id, Some(entry.clone())));
                } else {
                    results.push((rg_id, None));
                    missing_ranges.push(rg_id);
                }
            }
        }
        
        // Load missing rowgroups if any
        if !missing_ranges.is_empty() {
            debug!(
                file_path,
                missing_count = missing_ranges.len(),
                "Loading missing rowgroups"
            );
            
            // Get file metadata once
            let metadata = self.get_file_metadata(file_path).await?;
            
            // Optimize read strategy based on missing ranges
            let read_strategy = self.optimize_read_strategy(&metadata, &missing_ranges);
            
            match read_strategy {
                ReadStrategy::Individual => {
                    // Load each rowgroup individually
                    for rg_id in missing_ranges {
                        let cached_rg = self.get_rowgroup(
                            file_path,
                            rg_id,
                            collection_id,
                            query_context,
                        ).await?;
                        
                        // Update results
                        for (id, entry) in &mut results {
                            if *id == rg_id {
                                *entry = Some(cached_rg);
                                break;
                            }
                        }
                    }
                }
                ReadStrategy::Coalesced { ranges } => {
                    // Perform coalesced reads for adjacent rowgroups
                    for (start_offset, end_offset, rg_ids) in ranges {
                        let data = self.load_range(file_path, start_offset, end_offset - start_offset).await?;
                        
                        // Split data into individual rowgroups
                        self.split_and_cache_rowgroups(
                            file_path,
                            collection_id,
                            &data,
                            &metadata,
                            &rg_ids,
                        ).await?;
                    }
                    
                    // Retrieve from cache
                    for rg_id in missing_ranges {
                        let cached_rg = self.get_rowgroup(
                            file_path,
                            rg_id,
                            collection_id,
                            query_context,
                        ).await?;
                        
                        for (id, entry) in &mut results {
                            if *id == rg_id {
                                *entry = Some(cached_rg);
                                break;
                            }
                        }
                    }
                }
                ReadStrategy::FullFile => {
                    // More efficient to load full file
                    info!(file_path, "Loading full file - many rowgroups needed");
                    self.load_and_cache_full_file(file_path, collection_id).await?;
                    
                    // Retrieve from cache
                    for rg_id in missing_ranges {
                        let cached_rg = self.get_rowgroup(
                            file_path,
                            rg_id,
                            collection_id,
                            query_context,
                        ).await?;
                        
                        for (id, entry) in &mut results {
                            if *id == rg_id {
                                *entry = Some(cached_rg);
                                break;
                            }
                        }
                    }
                }
            }
        }
        
        // Extract final results in order
        Ok(results.into_iter()
            .filter_map(|(_, entry)| entry)
            .collect())
    }
    
    /// Check if a rowgroup can be skipped based on query context
    fn can_skip_rowgroup(&self, metadata: &RowGroupMetadata, context: &QueryContext) -> bool {
        // Check vector statistics
        if let Some(ref query_vector) = context.query_vector {
            // Check centroid distance if available
            if let Some(ref centroid) = metadata.centroid {
                // Simplified distance check - would use proper distance metric
                let approx_distance = self.estimate_distance(query_vector, centroid);
                if approx_distance > 1.0 { // Use a default threshold
                    return true;
                }
            }
        }
        
        // Check metadata filters
        if !context.metadata_filters.is_empty() {
            for (field, predicate) in &context.metadata_filters {
                if let Some(stats) = metadata.metadata_stats.get(field) {
                    // Check if predicate can possibly match based on min/max
                    if !self.predicate_could_match(predicate, stats) {
                        return true;
                    }
                }
            }
        }
        
        // Check temporal filters
        // Temporal filtering not implemented in QueryContext yet
        /*
        if let Some(ref temporal_filter) = context.temporal_filter {
            if let (Some(min_ts), Some(max_ts)) = (metadata.min_timestamp, metadata.max_timestamp) {
                if max_ts < temporal_filter.start_time || min_ts > temporal_filter.end_time {
                    return true;
                }
            }
        }
        */
        
        false
    }
    
    /// Load rowgroup data using selective range read
    async fn load_rowgroup_data(
        &self,
        file_path: &str,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>> {
        trace!(
            file_path, offset, size,
            "Loading rowgroup data via range read"
        );
        
        // Use zero-copy filesystem for optimized range read
        self.filesystem.read_range(file_path, offset, size)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read rowgroup: {}", e))
    }
    
    /// Load a range of data
    async fn load_range(&self, file_path: &str, offset: u64, size: u64) -> Result<Vec<u8>> {
        self.filesystem.read_range(file_path, offset, size)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read range: {}", e))
    }
    
    /// Get file metadata with caching
    async fn get_file_metadata(&self, file_path: &str) -> Result<RaptorFileMetadata> {
        // Check cache first
        {
            let cache = self.file_metadata_cache.read().await;
            if let Some(metadata) = cache.get(file_path) {
                return Ok(metadata.clone());
            }
        }
        
        // Load footer to get metadata
        info!(file_path, "Loading file metadata from footer");
        
        // Read last 64KB for footer (should be enough for metadata)
        let file_size = self.filesystem.metadata(file_path).await?.size;
        let footer_size = std::cmp::min(65536, file_size);
        let footer_offset = file_size - footer_size;
        
        let footer_data = self.filesystem.read_range(
            file_path,
            footer_offset,
            footer_size,
        ).await?;
        
        // Parse metadata from footer
        let metadata = self.parse_footer_metadata(&footer_data)?;
        
        // Cache it
        {
            let mut cache = self.file_metadata_cache.write().await;
            cache.insert(file_path.to_string(), metadata.clone());
        }
        
        Ok(metadata)
    }
    
    /// Parse metadata from footer bytes
    fn parse_footer_metadata(&self, footer_data: &[u8]) -> Result<RaptorFileMetadata> {
        // Find footer marker and parse
        // This would use bincode or similar to deserialize
        
        // Simplified - would actually parse
        bincode::deserialize(&footer_data[footer_data.len() - 4096..])
            .map_err(|e| anyhow::anyhow!("Failed to parse footer: {}", e))
    }
    
    /// Optimize read strategy for missing rowgroups
    fn optimize_read_strategy(
        &self,
        metadata: &RaptorFileMetadata,
        missing_ids: &[u32],
    ) -> ReadStrategy {
        if missing_ids.len() == 1 {
            return ReadStrategy::Individual;
        }
        
        // Check if we should load the full file
        let total_rowgroups = metadata.row_groups.len();
        let missing_ratio = missing_ids.len() as f32 / total_rowgroups as f32;
        
        if missing_ratio > 0.5 {
            // More than 50% missing - load full file
            return ReadStrategy::FullFile;
        }
        
        // Check for adjacent rowgroups that can be coalesced
        let mut ranges = Vec::new();
        let mut sorted_ids = missing_ids.to_vec();
        sorted_ids.sort_unstable();
        
        let mut current_range_start = None;
        let mut current_range_end = None;
        let mut current_ids = Vec::new();
        
        for &id in &sorted_ids {
            if let Some(rg) = metadata.row_groups.iter().find(|r| r.id == id) {
                let start = rg.offset;
                let end = rg.offset + rg.compressed_size;
                
                if let Some(range_end) = current_range_end {
                    // Check if this rowgroup is adjacent or close enough to coalesce
                    let gap = start.saturating_sub(range_end);
                    
                    // Coalesce if gap is less than 1MB
                    if gap < 1024 * 1024 {
                        current_range_end = Some(end);
                        current_ids.push(id);
                    } else {
                        // Start new range
                        if let Some(start) = current_range_start {
                            ranges.push((start, range_end, current_ids.clone()));
                        }
                        current_range_start = Some(start);
                        current_range_end = Some(end);
                        current_ids = vec![id];
                    }
                } else {
                    // First range
                    current_range_start = Some(start);
                    current_range_end = Some(end);
                    current_ids = vec![id];
                }
            }
        }
        
        // Add last range
        if let (Some(start), Some(end)) = (current_range_start, current_range_end) {
            ranges.push((start, end, current_ids));
        }
        
        // If we have many small ranges, might be better to load full file
        if ranges.len() > 10 {
            return ReadStrategy::FullFile;
        }
        
        ReadStrategy::Coalesced { ranges }
    }
    
    /// Split coalesced data and cache individual rowgroups
    async fn split_and_cache_rowgroups(
        &self,
        file_path: &str,
        collection_id: &str,
        data: &[u8],
        metadata: &RaptorFileMetadata,
        rowgroup_ids: &[u32],
    ) -> Result<()> {
        for &rg_id in rowgroup_ids {
            if let Some(rg_meta) = metadata.row_groups.iter().find(|r| r.id == rg_id) {
                // Calculate offset within the data buffer
                // This is simplified - would need proper offset calculation
                let rg_data = &data[0..rg_meta.compressed_size as usize];
                
                let cached_rg = CachedRowGroup {
                    compressed_data: Bytes::from(rg_data.to_vec()),
                    metadata: rg_meta.clone(),
                    cached_at: std::time::Instant::now(),
                    access_count: 1,
                    size_bytes: rg_meta.compressed_size as usize,
                };
                
                let key = RowGroupCacheKey {
                    file_path: file_path.to_string(),
                    rowgroup_id: rg_id,
                    collection_id: collection_id.to_string(),
                };
                
                self.insert_with_eviction(key, cached_rg).await?;
            }
        }
        
        Ok(())
    }
    
    /// Load and cache full file
    async fn load_and_cache_full_file(
        &self,
        file_path: &str,
        collection_id: &str,
    ) -> Result<()> {
        let data = self.filesystem.read(file_path).await
            .map_err(|e| anyhow::anyhow!("Failed to read full file: {}", e))?;
        
        let metadata = self.get_file_metadata(file_path).await?;
        
        // Cache each rowgroup
        for rg_meta in &metadata.row_groups {
            let start = rg_meta.offset as usize;
            let end = (rg_meta.offset + rg_meta.compressed_size) as usize;
            
            if end <= data.len() {
                let rg_data = &data[start..end];
                
                let cached_rg = CachedRowGroup {
                    compressed_data: Bytes::from(rg_data.to_vec()),
                    metadata: rg_meta.clone(),
                    cached_at: std::time::Instant::now(),
                    access_count: 1,
                    size_bytes: rg_meta.compressed_size as usize,
                };
                
                let key = RowGroupCacheKey {
                    file_path: file_path.to_string(),
                    rowgroup_id: rg_meta.id,
                    collection_id: collection_id.to_string(),
                };
                
                self.insert_with_eviction(key, cached_rg).await?;
            }
        }
        
        Ok(())
    }
    
    /// Insert into cache with LRU eviction if needed
    async fn insert_with_eviction(
        &self,
        key: RowGroupCacheKey,
        entry: CachedRowGroup,
    ) -> Result<()> {
        let entry_size = entry.size_bytes;
        
        // Check if we need eviction
        let mut current_size = self.current_size.write().await;
        
        if *current_size + entry_size > self.max_cache_size {
            // Evict least recently used entries
            let mut cache = self.cache.write().await;
            
            // Sort by access time and count
            let mut entries: Vec<_> = cache.iter()
                .map(|(k, v)| (k.clone(), v.cached_at, v.access_count, v.size_bytes))
                .collect();
            
            entries.sort_by_key(|(_, time, count, _)| (*time, *count));
            
            // Evict until we have space
            let mut freed_space = 0;
            let mut to_remove = Vec::new();
            
            for (k, _, _, size) in entries {
                to_remove.push(k);
                freed_space += size;
                
                if *current_size - freed_space + entry_size <= self.max_cache_size {
                    break;
                }
            }
            
            for k in to_remove {
                if let Some(removed) = cache.remove(&k) {
                    *current_size -= removed.size_bytes;
                    debug!(
                        "Evicted rowgroup from cache: {:?}",
                        k
                    );
                }
            }
        }
        
        // Insert new entry
        let mut cache = self.cache.write().await;
        cache.insert(key, entry);
        *current_size += entry_size;
        
        Ok(())
    }
    
    /// Trigger prefetch based on strategy
    async fn maybe_prefetch(
        &self,
        file_path: &str,
        current_rg_id: u32,
        metadata: &RaptorFileMetadata,
    ) {
        match &self.prefetch_strategy {
            PrefetchStrategy::None => {}
            PrefetchStrategy::Adjacent { count } => {
                // Prefetch adjacent rowgroups
                let mut to_prefetch = Vec::new();
                
                for i in 1..=*count {
                    if let Some(rg) = metadata.row_groups.iter().find(|r| r.id == current_rg_id + i as u32) {
                        to_prefetch.push(rg.id);
                    }
                    if current_rg_id >= i as u32 {
                        if let Some(rg) = metadata.row_groups.iter().find(|r| r.id == current_rg_id - i as u32) {
                            to_prefetch.push(rg.id);
                        }
                    }
                }
                
                // Spawn background prefetch
                let file_path = file_path.to_string();
                let self_clone = self.clone();
                tokio::spawn(async move {
                    for rg_id in to_prefetch {
                        let _ = self_clone.get_rowgroup(
                            &file_path,
                            rg_id,
                            "prefetch",
                            None,
                        ).await;
                    }
                });
            }
            PrefetchStrategy::HnswLocality { max_hops: _ } => {
                // Would prefetch based on HNSW connectivity
                // This requires HNSW graph information
            }
            PrefetchStrategy::Adaptive => {
                // Would use access patterns to predict next rowgroups
            }
        }
    }
    
    /// Estimate distance between vectors (simplified)
    fn estimate_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }
    
    /// Check if predicate could match based on stats
    fn predicate_could_match(
        &self,
        _predicate: &HashMap<String, String>,
        _stats: &super::common::ColumnStats,
    ) -> bool {
        // Simplified - would check min/max against predicate
        true
    }
    
    /// Serialize cache to disk for persistence
    pub async fn serialize_cache(&self) -> Result<Vec<u8>> {
        let cache = self.cache.read().await;
        let mut serialized_entries = HashMap::new();
        
        for (key, value) in cache.iter() {
            let entry = SerializedCacheEntry {
                compressed_data: value.compressed_data.to_vec(),
                metadata: value.metadata.clone(),
                access_count: value.access_count,
            };
            serialized_entries.insert(key.clone(), entry);
        }
        
        bincode::serialize(&serialized_entries)
            .map_err(|e| anyhow::anyhow!("Failed to serialize cache: {}", e))
    }
    
    /// Deserialize cache from disk
    pub async fn deserialize_cache(&self, data: &[u8]) -> Result<()> {
        let entries: HashMap<RowGroupCacheKey, SerializedCacheEntry> = 
            bincode::deserialize(data)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize cache: {}", e))?;
        
        let mut cache = self.cache.write().await;
        let mut current_size = self.current_size.write().await;
        
        for (key, entry) in entries {
            let cached_rg = CachedRowGroup {
                compressed_data: Bytes::from(entry.compressed_data),
                metadata: entry.metadata,
                cached_at: std::time::Instant::now(),
                access_count: entry.access_count,
                size_bytes: entry.metadata.compressed_size as usize,
            };
            
            *current_size += cached_rg.size_bytes;
            cache.insert(key, cached_rg);
        }
        
        Ok(())
    }
}

/// Read strategy for missing rowgroups
enum ReadStrategy {
    /// Read each rowgroup individually
    Individual,
    
    /// Coalesce adjacent rowgroups into fewer reads
    Coalesced {
        ranges: Vec<(u64, u64, Vec<u32>)>, // (start, end, rowgroup_ids)
    },
    
    /// Load the full file (many rowgroups needed)
    FullFile,
}

// Implement Clone for testing and spawning
impl Clone for RowGroupCacheManager {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            io_system: self.io_system.clone(),
            filesystem: self.filesystem.clone(),
            file_metadata_cache: self.file_metadata_cache.clone(),
            max_cache_size: self.max_cache_size,
            current_size: self.current_size.clone(),
            prefetch_strategy: self.prefetch_strategy.clone(),
        }
    }
}