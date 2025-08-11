//! Unified SSTable Reader Architecture
//!
//! This module provides an optimized reader for SSTables with:
//! - Block-level access with caching
//! - Metadata bloom filters for efficient filtering
//! - Index-based range scans
//! - Predicate pushdown to block level
//! - Unified search interface integration

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::io::Read;
use tracing::{debug, warn, info};

// Performance optimizations: import commonly used types and functions for zero-cost abstractions
// use std::hint::likely; // Unstable feature - removed for compilation
use std::ptr;

use crate::core::VectorRecord;
use crate::core::search::{SearchParams, SearchResult, FilterExpression};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::engines::sst::bloom_filter::SstableBloomFilter;
use crate::storage::engines::sst::{SstableHeader, DataBlock, IndexEntry, SstRecord, CompressionAlgorithmSst};
use crate::core::bloom::{BloomFilterConfig, BloomStrategy};
use crate::storage::cache::specialized::{VectorStore, IndexNodeCache, BitmapFilterCache};
use crate::storage::cache::specialized::vector_store::SstBlockKey;

/// Unified SSTable Reader with automatic optimization selection
pub struct UnifiedSstableReader {
    filesystem: Arc<FilesystemFactory>,
    // REPLACED: Using central cache module instead of custom BlockCache
    vector_cache: Arc<VectorStore>,        // For data blocks
    index_node_cache: Arc<IndexNodeCache>, // For SSTable indices
    bloom_cache: Arc<BitmapFilterCache>,   // For bloom filters
    strategy_selector: Arc<ReadingStrategySelector>,
}

impl std::fmt::Debug for UnifiedSstableReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedSstableReader")
            .field("filesystem", &"FilesystemFactory")
            .field("vector_cache", &"VectorStore")
            .field("index_node_cache", &"IndexNodeCache")
            .field("bloom_cache", &"BitmapFilterCache")
            .field("strategy_selector", &self.strategy_selector)
            .finish()
    }
}

/// Block cache for frequently accessed data blocks
#[derive(Debug)]
pub struct BlockCache {
    cache: Arc<tokio::sync::RwLock<lru::LruCache<BlockCacheKey, Arc<DataBlock>>>>,
    max_size: usize,
    hit_rate: Arc<tokio::sync::RwLock<CacheStats>>,
}

/// Optimized Index cache with memory bounds and LRU eviction
#[derive(Debug)]
pub struct IndexCache {
    indices: Arc<moka::future::Cache<String, Arc<SstableIndex>>>,
    bloom_filters: Arc<moka::future::Cache<String, Arc<SstableBloomFilter>>>,
    max_memory_mb: usize,
    metrics: Arc<tokio::sync::RwLock<CacheMetrics>>,
}

/// Cache metrics for monitoring and optimization
#[derive(Debug, Default)]
pub struct CacheMetrics {
    pub memory_usage_bytes: usize,
    pub hit_count: u64,
    pub miss_count: u64,
    pub eviction_count: u64,
    pub memory_pressure_events: u64,
}

/// Enhanced SSTable index with metadata statistics
#[derive(Debug, Clone)]
pub struct SstableIndex {
    pub entries: Vec<IndexEntry>,
    pub metadata_stats: HashMap<String, MetadataStats>,
    pub vector_count: usize,
    pub min_key: String,
    pub max_key: String,
}

/// Metadata statistics for predicate pushdown
#[derive(Debug, Clone)]
pub struct MetadataStats {
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
    pub null_count: usize,
    pub distinct_count: usize,
    pub bloom_filter_offset: Option<u64>,
}

/// Enhanced bloom filter supporting metadata columns

/// Reading strategy for SSTable access
#[derive(Debug, Clone)]
pub enum SstableReadingStrategy {
    /// Full scan for small files or high selectivity
    FullScan {
        use_block_cache: bool,
    },
    /// Index-based range scan
    IndexRangeScan {
        start_block: usize,
        end_block: usize,
        use_bloom_filter: bool,
    },
    /// Metadata-driven block selection
    MetadataFiltered {
        selected_blocks: Vec<usize>,
        skip_bloom_check: bool,
    },
    /// Hybrid approach for complex queries
    Hybrid {
        primary_strategy: Box<SstableReadingStrategy>,
        fallback_blocks: Vec<usize>,
    },
}

/// Strategy selector based on query characteristics
#[derive(Debug)]
pub struct ReadingStrategySelector {
    config: ReaderConfig,
}

/// Configuration for reading strategies
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub block_cache_size: usize,
    pub index_cache_size: usize,
    pub bloom_filter_threshold: f64,
    pub range_scan_threshold: usize,
    pub metadata_selectivity_threshold: f64,
    pub enable_read_ahead: bool,
    pub read_ahead_blocks: usize,
}

/// Block cache key
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct BlockCacheKey {
    pub file_path: String,
    pub block_id: u32,
    pub block_index: usize,
}


/// Cache statistics
#[derive(Debug, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}


/// Collection context for search
pub struct CollectionContext {
    pub file_path: String,
    pub sstable_files: Vec<String>,
    pub total_vectors: usize,
    pub metadata_columns: Vec<String>,
    pub level: usize,
    pub creation_time: chrono::DateTime<chrono::Utc>,
    /// I/O optimization hints for efficient SSTable access
    pub io_optimization_hints: Option<HashMap<String, serde_json::Value>>,
}

impl UnifiedSstableReader {

    /// Ultra-fast metadata comparison using optimized string comparison
    #[inline(always)]
    fn fast_metadata_match(&self, metadata: &[crate::proto::proximadb::MetadataItem], 
                          filter_key: &str, filter_value: &serde_json::Value) -> bool {
        // Early exit if no metadata
        if metadata.is_empty() { return false; }
        
        // Linear search is often faster than HashMap lookup for small metadata sets (< 16 items)
        // which is typical for vector metadata
        for item in metadata.iter() {
            if item.key == filter_key {
                return self.fast_value_comparison(&item.value, filter_value);
            }
        }
        false
    }

    /// Extract filter information once for high-performance repeated use
    #[inline(always)]
    fn extract_filter<'a>(&self, params: &'a SearchParams) -> Option<(&'a str, &'a serde_json::Value)> {
        match &params.filter_expression {
            Some(FilterExpression::Comparison { field, operator: _, value }) => {
                Some((field.as_str(), value))
            }
            _ => None
        }
    }
    
    #[inline(always)]  
    fn fast_value_comparison(&self, item_value: &Option<crate::proto::proximadb::metadata_item::Value>, 
                           filter_value: &serde_json::Value) -> bool {
        match (item_value, filter_value) {
            // Hot path: string comparisons (most common case)
            (Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)), serde_json::Value::String(filter_s)) => {
                // Use ptr-based comparison first, then memcmp for equality
                ptr::eq(s.as_ptr(), filter_s.as_ptr()) || s == filter_s
            }
            // Hot path: number comparisons
            (Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)), serde_json::Value::Number(filter_n)) => {
                // Direct f64 comparison is very fast
                (*n - filter_n.as_f64().unwrap_or(0.0)).abs() < f64::EPSILON
            }
            // Less common paths
            (Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)), serde_json::Value::Bool(filter_b)) => {
                *b == *filter_b
            }
            (None, serde_json::Value::Null) => true,
            _ => false
        }
    }

    /// Validate SST1 magic marker in a file to ensure it's a valid SSTable
    /// Returns Ok(()) if valid, Err with descriptive message if invalid
    /// This prevents reading non-SSTable files that could cause deserialization errors
    pub async fn validate_sst_file(&self, file_path: &str) -> Result<()> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}:///", scheme))?;
        
        // Check if file exists first
        if !fs.exists(file_path).await? {
            return Err(anyhow::anyhow!("SSTable file does not exist: {}", file_path));
        }
        
        // Read first 4 bytes to check magic marker
        let magic_bytes = fs.read_range(file_path, 0, 4).await
            .map_err(|e| anyhow::anyhow!("Failed to read magic bytes from {}: {}", file_path, e))?;
        
        if magic_bytes.len() < 4 {
            return Err(anyhow::anyhow!(
                "File too small to be valid SSTable: {} has only {} bytes", 
                file_path, magic_bytes.len()
            ));
        }
        
        if &magic_bytes[0..4] != b"SST1" {
            // Log what we actually found for debugging
            let found_magic = std::str::from_utf8(&magic_bytes[0..4])
                .map(|s| s.to_string())
                .unwrap_or_else(|_| format!("bytes: {:?}", &magic_bytes[0..4]));
            
            return Err(anyhow::anyhow!(
                "Invalid SSTable format: expected SST1 magic marker, found '{}' in file {}", 
                found_magic, file_path
            ));
        }
        
        debug!("✅ SST1 magic marker validated for file: {}", file_path);
        Ok(())
    }
    
    /// Create a new unified reader with central cache integration
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        let config = ReaderConfig::default();
        Self::with_cache(
            filesystem,
            Arc::new(VectorStore::new(config.block_cache_size / (1024 * 1024))), // Convert to MB
            Arc::new(IndexNodeCache::new(config.index_cache_size / (1024 * 1024))),
            Arc::new(BitmapFilterCache::new(50)), // 50MB for bloom filters
            config,
        )
    }
    
    /// Create with external cache instances for sharing
    pub fn with_cache(
        filesystem: Arc<FilesystemFactory>,
        vector_cache: Arc<VectorStore>,
        index_node_cache: Arc<IndexNodeCache>,
        bloom_cache: Arc<BitmapFilterCache>,
        config: ReaderConfig,
    ) -> Self {
        Self {
            filesystem,
            vector_cache,
            index_node_cache,
            bloom_cache,
            strategy_selector: Arc::new(ReadingStrategySelector::new(config)),
        }
    }
    
    /// Search vectors using optimized strategies
    pub async fn search_vectors(
        &self,
        params: &SearchParams,
        collection_context: &CollectionContext,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 SSTABLE READER: Starting search with {} files, k={}", 
              collection_context.sstable_files.len(),
              params.top_k.unwrap_or(10));
        
        // CRITICAL: Create distance compute locally per query to avoid cross-query contamination
        // This ensures thread safety and correct distance metric for each query
        let distance_compute = UnifiedDistanceCompute::default();
        
        // Debug: print file paths
        for (i, file_path) in collection_context.sstable_files.iter().enumerate() {
            debug!("📁 SSTable file {}: {}", i, file_path);
        }
        
        // Debug: print filter expression
        if let Some(filter) = &params.filter_expression {
            debug!("🔎 Filter expression: {:?}", filter);
        }
        
        // 1. Select optimal reading strategy
        let strategy = self.strategy_selector.select_strategy(params, collection_context)?;
        debug!("📊 Selected strategy: {:?}", strategy);
        
        // 2. Apply strategy to read relevant blocks
        let relevant_blocks = self.apply_strategy(&strategy, params, collection_context).await?;
        debug!("📦 SSTABLE READER: Loaded {} data blocks total from all files", relevant_blocks.len());
        
        // Debug: print some sample records from blocks
        for (i, block) in relevant_blocks.iter().take(2).enumerate() {
            debug!("  Block {}: {} records", i, block.records.len());
            for (j, record) in block.records.iter().take(3).enumerate() {
                debug!("    Record {}: id={}, metadata={:?}", j, record.id, record.metadata);
            }
        }
        
        // 3. Perform vector search on loaded data
        let results = self.search_in_blocks(params, &relevant_blocks, &distance_compute).await?;
        debug!("🎯 Found {} search results after filtering and scoring", results.len());
        
        // Debug: print sample results
        for (i, result) in results.iter().take(3).enumerate() {
            debug!("  Result {}: id={}, score={}, metadata={:?}", 
                  i, result.id, result.score, result.metadata);
        }
        
        Ok(results)
    }
    
    /// Apply reading strategy to load relevant blocks
    fn apply_strategy<'a>(
        &'a self,
        strategy: &'a SstableReadingStrategy,
        params: &'a SearchParams,
        context: &'a CollectionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<DataBlock>>> + Send + 'a>> {
        Box::pin(async move {
        match strategy {
            SstableReadingStrategy::FullScan { use_block_cache } => {
                self.full_scan_strategy(context, *use_block_cache).await
            }
            SstableReadingStrategy::IndexRangeScan { start_block, end_block, use_bloom_filter } => {
                self.index_range_scan_strategy(context, *start_block, *end_block, *use_bloom_filter).await
            }
            SstableReadingStrategy::MetadataFiltered { selected_blocks, skip_bloom_check } => {
                self.metadata_filtered_strategy(context, params, selected_blocks, *skip_bloom_check).await
            }
            SstableReadingStrategy::Hybrid { primary_strategy, fallback_blocks } => {
                let mut blocks = self.apply_strategy(primary_strategy, params, context).await?;
                let fallback = self.load_specific_blocks(context, fallback_blocks).await?;
                blocks.extend(fallback);
                Ok(blocks)
            }
        }
        })
    }
    
    /// Perform ultra-high-performance vector search in loaded blocks
    /// CRITICAL HOT PATH: This method is called for every search operation
    /// Optimized for maximum throughput and minimum latency
    async fn search_in_blocks(
        &self,
        params: &SearchParams,
        blocks: &[DataBlock],
        distance_compute: &UnifiedDistanceCompute,
    ) -> Result<Vec<SearchResult>> {
        let query_vector = params.first_query_vector()
            .ok_or_else(|| anyhow::anyhow!("Query vector required"))?;
        
        let k = params.top_k.unwrap_or(10);
        let distance_metric = params.distance_metric.unwrap_or(crate::compute::distance_computation::DistanceMetric::Cosine);
        
        debug!("🔍 Searching in {} blocks for top {} results", blocks.len(), k);
        
        // Pre-allocate with exact capacity to avoid reallocations (critical for performance)
        let total_capacity: usize = blocks.iter().map(|b| b.records.len()).sum();
        let mut scored_results = Vec::with_capacity(total_capacity.min(k * 10)); // Pre-allocate for top 10*k candidates
        
        let mut total_records = 0u32; // Use u32 for better cache efficiency
        let mut filtered_out = 0u32;
        let mut tombstones = 0u32;
        
        // Extract filter for fast access (avoid repeated Options checks)
        let filter_info = self.extract_filter(params);
        
        // OPTIMIZED SEARCH LOOP: Use unified distance compute for semantic correctness
        for (block_idx, block) in blocks.iter().enumerate() {
            let block_records = block.records.len() as u32;
            total_records += block_records;
            
            // Skip empty blocks immediately (branch prediction optimization)  
            if block_records == 0 { continue; }
            
            debug!("📊 Processing block {} with {} records", block_idx, block_records);
            
            // Process records with optimized filtering and distance calculation
            for record in &block.records {
                // Fast tombstone check (most common early exit)
                if record.is_tombstone {
                    tombstones += 1;
                    continue;
                }
                
                // Ultra-fast metadata filtering (hot path optimization)
                if let Some((filter_key, filter_value)) = filter_info {
                    if !self.fast_metadata_match(&record.metadata, filter_key, filter_value) {
                        filtered_out += 1;
                        continue; // Skip to next record immediately
                    }
                }
                
                // Calculate similarity using unified distance computation for semantic correctness
                let similarity = distance_compute.calculate_distance(
                    query_vector,
                    &record.vector,
                    &distance_metric,
                );
                
                // Efficient SearchResult creation (minimize allocations)
                scored_results.push(SearchResult {
                    id: record.id.clone(),
                    score: similarity.normalized_score,
                    distance: Some(similarity.raw_value),
                    rank: None,
                    vector: Some(record.vector.clone()),
                    vector_id: Some(record.id.clone()),
                    metadata: self.metadata_items_to_json(&record.metadata),
                    debug_info: None,
                    semantic_distance: Some(similarity), // Use unified distance result
                    created_at: None,
                    engine_stats: None,
                    quantization_info: None,
                    index_path: None,
                    version: record.version,
                    timestamp: Some(record.updated_at.unwrap_or(record.timestamp)),
                });
            }
        }
        
        debug!("📊 Search stats: {} total records, {} tombstones, {} filtered out, {} candidates", 
              total_records, tombstones, filtered_out, scored_results.len());
        
        // Sort and return top-k results (in-place sorting for memory efficiency)
        scored_results.sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Truncate to k results efficiently
        if scored_results.len() > k {
            scored_results.truncate(k);
        }
        
        // Set rankings
        for (rank, result) in scored_results.iter_mut().enumerate() {
            result.rank = Some((rank + 1) as u16);
        }
        
        debug!("🎯 Returning {} final results", scored_results.len());
        Ok(scored_results)
    }
    
    /// Full scan strategy implementation with parallel file processing
    async fn full_scan_strategy(
        &self,
        context: &CollectionContext,
        use_block_cache: bool,
    ) -> Result<Vec<DataBlock>> {
        debug!("🔍 Full scan strategy for {} files (cache={})", context.sstable_files.len(), use_block_cache);
        
        // Use parallel processing for multiple files
        if context.sstable_files.len() > 1 {
            return self.parallel_full_scan(context, use_block_cache).await;
        }
        let mut all_blocks = Vec::new();
        
        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!("📂 Reading file {} of {}: {}", idx + 1, context.sstable_files.len(), file_path);
            
            // Validate SST1 magic marker before attempting to read the file
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue; // Skip this file entirely and move to the next one
                }
            }
            
            let blocks = if use_block_cache {
                self.read_file_with_cache(file_path).await?
            } else {
                self.read_file_direct(file_path).await?
            };
            debug!("  📦 Loaded {} blocks from this file", blocks.len());
            
            // Debug: print sample records from first block
            if let Some(first_block) = blocks.first() {
                debug!("  🔎 First block has {} records", first_block.records.len());
                for (i, record) in first_block.records.iter().take(3).enumerate() {
                    debug!("    Record {}: id={}, metadata={:?}", i, record.id, record.metadata);
                }
            }
            
            all_blocks.extend(blocks);
        }
        
        debug!("✅ Full scan loaded {} total blocks from all files", all_blocks.len());
        Ok(all_blocks)
    }
    
    /// Parallel full scan across multiple SSTable files
    async fn parallel_full_scan(
        &self,
        context: &CollectionContext,
        use_block_cache: bool,
    ) -> Result<Vec<DataBlock>> {
        
        use tokio::sync::Semaphore;
        use std::sync::Arc;
        
        // Limit concurrent file operations to avoid resource exhaustion
        let max_concurrent_files = num_cpus::get().min(8);
        let _semaphore = Arc::new(Semaphore::new(max_concurrent_files));
        
        info!("🚀 Starting parallel SSTable full scan across {} files (max concurrency: {})", 
              context.sstable_files.len(), max_concurrent_files);
        
        // Process files sequentially for now to avoid lifetime issues
        // TODO: Refactor to use Arc<Self> or implement Clone
        let mut all_blocks = Vec::new();
        
        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!("🔄 Reading file {} of {}: {}", 
                   idx + 1, context.sstable_files.len(), file_path);
            
            // Validate SST1 magic marker before attempting to read the file
            match self.validate_sst_file(file_path).await {
                Ok(()) => {
                    debug!("✅ SST1 validation passed for file: {}", file_path);
                }
                Err(e) => {
                    warn!("⚠️ Skipping invalid SSTable file {}: {}", file_path, e);
                    continue; // Skip this file entirely and move to the next one
                }
            }
            
            let start_time = std::time::Instant::now();
            
            let result = if use_block_cache {
                self.read_file_with_cache(file_path).await
            } else {
                self.read_file_direct(file_path).await
            };
            
            let elapsed = start_time.elapsed();
            
            match result {
                Ok(blocks) => {
                    debug!("✅ Loaded {} blocks from {} in {:?}", 
                           blocks.len(), file_path, elapsed);
                    all_blocks.extend(blocks);
                }
                Err(e) => {
                    warn!("❌ Failed to read {}: {}", file_path, e);
                    // Continue with other files instead of failing entirely
                }
            }
        }
        
        info!("🎯 SSTable scan completed: {} total blocks from {} files", 
              all_blocks.len(), context.sstable_files.len());
        
        Ok(all_blocks)
    }
    
    // Placeholder implementations for other strategies
    async fn index_range_scan_strategy(
        &self,
        context: &CollectionContext,
        start_block: usize,
        end_block: usize,
        use_bloom: bool,
    ) -> Result<Vec<DataBlock>> {
        debug!("🔍 Index range scan strategy for {} files (blocks {}-{}, bloom={})", 
                 context.sstable_files.len(), start_block, end_block, use_bloom);
        
        let mut all_blocks = Vec::new();
        
        // For now, just read all files like full scan
        // TODO: Implement proper block-level indexing
        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!("📂 Reading file {} of {}: {}", idx + 1, context.sstable_files.len(), file_path);
            let blocks = self.read_file_direct(file_path).await?;
            debug!("  📦 Loaded {} blocks from this file", blocks.len());
            
            all_blocks.extend(blocks);
        }
        
        debug!("📦 Total blocks loaded: {}", all_blocks.len());
        Ok(all_blocks)
    }
    
    async fn metadata_filtered_strategy(
        &self,
        context: &CollectionContext,
        params: &SearchParams,
        blocks: &[usize],
        skip_bloom: bool,
    ) -> Result<Vec<DataBlock>> {
        debug!("🔍 Using metadata filtered strategy for {} files", context.sstable_files.len());
        
        let mut all_blocks = Vec::new();
        let metadata_conditions = self.extract_metadata_conditions(params);
        debug!("📋 Extracted metadata conditions: {:?}", metadata_conditions);
        
        // Process each SSTable file
        for (file_idx, file_path) in context.sstable_files.iter().enumerate() {
            debug!("📂 Processing SSTable file {} of {}: {}", file_idx + 1, context.sstable_files.len(), file_path);
            
            // Get bloom filter - either from cache or load from disk
            let _bloom_filter: Option<SstableBloomFilter> = if !skip_bloom {
                // First check if we have a cached bloom filter
                if let Some(_cached_result) = self.bloom_cache.get_with_hooks(&file_path.to_string()).await {
                    // We have a cached version, but it's a simplified bitmap
                    // For proper metadata filtering, we need the actual bloom filter
                    // So we'll load it from disk if needed for metadata filtering
                    None // Will be loaded on-demand below if metadata conditions exist
                } else {
                    None
                }
            } else {
                None
            };

            // Get the index from central index cache
            let index = if let Some(cached_index) = self.index_node_cache.get_sstable_index(file_path).await {
                // Convert cached SstIndexEntry back to IndexEntry
                let entries: Vec<IndexEntry> = cached_index.entries.iter().map(|e| {
                    IndexEntry {
                        key: e.key.clone(),
                        offset: e.block_offset,
                        size: e.block_size as u32,
                        block_id: 0, // Would need to be stored in cache
                        block_offset: 0,
                        compressed: false,
                        metadata_min_values: HashMap::new(),
                        metadata_max_values: HashMap::new(),
                        metadata_null_counts: HashMap::new(),
                    }
                }).collect();
                
                // Convert cached metadata stats to local type
                let mut local_metadata_stats = HashMap::new();
                for (key, stats) in cached_index.metadata_stats {
                    local_metadata_stats.insert(key, MetadataStats {
                        min_value: stats.min_value,
                        max_value: stats.max_value,
                        null_count: stats.null_count,
                        distinct_count: stats.distinct_count,
                        bloom_filter_offset: None,
                    });
                }
                
                SstableIndex {
                    entries,
                    metadata_stats: local_metadata_stats,
                    vector_count: cached_index.total_vectors,
                    min_key: String::new(),
                    max_key: String::new(),
                }
            } else {
                // Load and cache the index
                let loaded_index = self.load_index_optimized(file_path).await?;
                // Convert IndexEntry to SstIndexEntry for cache storage
                let cache_entries: Vec<crate::storage::cache::specialized::index_node_cache::SstIndexEntry> = 
                    loaded_index.entries.iter().map(|e| {
                        crate::storage::cache::specialized::index_node_cache::SstIndexEntry {
                            key: e.key.clone(),
                            block_offset: e.offset,
                            block_size: e.size as usize,
                            min_key: e.key.clone(), // Would need to track actual min/max
                            max_key: e.key.clone(),
                            vector_count: 1, // Approximation
                            bloom_filter_offset: None,
                        }
                    }).collect();
                
                // Convert metadata stats for cache
                let mut cache_metadata_stats = std::collections::HashMap::new();
                for (key, stats) in loaded_index.metadata_stats.iter() {
                    cache_metadata_stats.insert(key.clone(), crate::storage::cache::specialized::index_node_cache::MetadataStats {
                        min_value: stats.min_value.clone(),
                        max_value: stats.max_value.clone(),
                        null_count: stats.null_count,
                        distinct_count: stats.distinct_count,
                    });
                }
                
                let sstable_index = crate::storage::cache::specialized::index_node_cache::SstableIndex {
                    file_path: file_path.clone(),
                    entries: cache_entries,
                    total_blocks: loaded_index.entries.len(),
                    total_vectors: loaded_index.entries.len(), // Approximation: one vector per entry
                    metadata_stats: cache_metadata_stats,
                };
                self.index_node_cache.cache_sstable_index(file_path, sstable_index.clone()).await?;
                loaded_index
            };
            debug!("  📊 Loaded index with {} entries", index.entries.len());

            // Check bloom filter for quick rejection when we have metadata conditions
            if !metadata_conditions.is_empty() && !skip_bloom {
                // For metadata filtering, we need the actual bloom filter from disk
                // Check if it's worth loading based on whether we're reading from cache or disk
                let is_cached_data = self.bloom_cache.get_with_hooks(&file_path.to_string()).await.is_some();
                
                if !is_cached_data {
                    // Data is not cached, so we're reading from disk anyway
                    // Load the bloom filter for proper metadata matching
                    match self.load_bloom_filter(file_path).await {
                        Ok(Some(bloom)) => {
                            let mut any_match = false;
                            for (column, value) in &metadata_conditions {
                                // Convert JSON value to MetadataItem for type-safe bloom filter check
                                let metadata_item = crate::core::bloom::json_to_metadata_item(column, value);
                                if bloom.might_match_metadata(column, &metadata_item).unwrap_or(true) {
                                    any_match = true;
                                    break;
                                }
                            }
                            
                            if !any_match {
                                debug!("  ❌ Bloom filter rejected file {} (no metadata matches)", file_path);
                                continue; // Skip this file entirely
                            }
                            debug!("  ✅ Bloom filter indicates potential matches");
                        }
                        Ok(None) => {
                            debug!("  ⚠️ No bloom filter available for {}", file_path);
                        }
                        Err(e) => {
                            debug!("  ⚠️ Failed to load bloom filter for {}: {}", file_path, e);
                        }
                    }
                } else {
                    // Data is cached, bloom filter check less critical
                    debug!("  ℹ️ Skipping bloom filter check for cached data");
                }
            }

            // Use block-level metadata statistics to filter blocks
            let mut selected_blocks = Vec::new();
            let block_list = if blocks.is_empty() {
                // If no specific blocks provided, check all blocks
                (0..index.entries.len()).collect::<Vec<_>>()
            } else {
                blocks.to_vec()
            };
            let total_blocks = block_list.len();

        for block_idx in block_list {
            if block_idx >= index.entries.len() {
                continue;
            }
            
            let entry = &index.entries[block_idx];
            let mut should_include = true;
            
            // Check each metadata condition against block statistics
            for (column, value) in &metadata_conditions {
                // Check if this block might contain the value
                if let Some(min_val) = entry.metadata_min_values.get(column) {
                    if let Some(max_val) = entry.metadata_max_values.get(column) {
                        // Use the centralized comparison function for proper numeric handling
                        // If value is outside the min/max range, skip this block
                        if Self::compare_metadata_values(value, min_val) == std::cmp::Ordering::Less ||
                           Self::compare_metadata_values(value, max_val) == std::cmp::Ordering::Greater {
                            should_include = false;
                            break;
                        }
                    }
                } else {
                    // Column not present in this block, check if there are nulls
                    if entry.metadata_null_counts.get(column).copied().unwrap_or(0) == 0 {
                        // No values for this column in this block
                        should_include = false;
                        break;
                    }
                }
            }
            
            if should_include {
                selected_blocks.push(block_idx);
            }
        }

            debug!("  📦 Selected {} blocks out of {} after metadata filtering for file {}", 
                   selected_blocks.len(), total_blocks, file_path);

            // Load the selected blocks from this file
            for block_idx in selected_blocks {
                debug!("    📄 Loading block {} from file {}", block_idx, file_path);
                // Create a temporary context for this specific file
                let file_context = CollectionContext {
                    file_path: file_path.clone(),
                    sstable_files: vec![file_path.clone()],
                    total_vectors: context.total_vectors,
                    metadata_columns: context.metadata_columns.clone(),
                    level: context.level,
                    creation_time: context.creation_time,
                    io_optimization_hints: context.io_optimization_hints.clone(),
                };
                
                if let Some(block) = self.load_block_with_cache(&file_context, block_idx).await? {
                    // Debug: print first few records from loaded block
                    debug!("    📦 Loaded block {} with {} records from {}", 
                          block_idx, block.records.len(), file_path);
                    for (i, record) in block.records.iter().take(3).enumerate() {
                        debug!("      Record {}: id={}, metadata={:?}", i, record.id, record.metadata);
                    }
                    all_blocks.push(block);
                }
            }
        }

        debug!("Loaded {} blocks total after metadata filtering from {} files", 
              all_blocks.len(), context.sstable_files.len());
        Ok(all_blocks)
    }

    /// Extract metadata conditions from search params
    fn extract_metadata_conditions(&self, params: &SearchParams) -> HashMap<String, serde_json::Value> {
        // Use centralized filter extraction logic
        if let Some(ref filter_expr) = params.filter_expression {
            crate::core::search::filter_extraction::extract_metadata_conditions(filter_expr)
        } else {
            HashMap::new()
        }
    }

    /// Load a specific block with caching
    async fn load_block_with_cache(&self, context: &CollectionContext, block_idx: usize) -> Result<Option<DataBlock>> {
        let _cache_key = BlockCacheKey {
            file_path: context.file_path.clone(),
            block_id: block_idx as u32,
            block_index: block_idx,
        };

        // Use central VectorStore for caching
        let sst_cache_key = SstBlockKey::new(
            context.file_path.clone(),
            block_idx as u64 * 4096, // Assuming 4KB blocks
            4096,
        );

        // Check if we have cached vectors for this block
        let cached_vectors = self.vector_cache.get_block_vectors(&sst_cache_key, 100).await;
        if !cached_vectors.is_empty() {
            // Convert VectorRecord to SstRecord for DataBlock
            let sst_records: Vec<SstRecord> = cached_vectors.into_iter().map(|v| {
                SstRecord {
                    id: v.id.unwrap_or_default(),
                    vector: v.vector,
                    metadata: v.metadata,
                    timestamp: v.timestamp,
                    updated_at: v.updated_at,
                    expires_at: v.expires_at,
                    version: v.version,
                    // SST-specific fields (defaults for cached data)
                    is_tombstone: false,
                    sequence_number: 0,
                    level: 0,
                }
            }).collect();
            
            // Reconstruct DataBlock from cached vectors
            let block = DataBlock {
                block_id: block_idx as u32,
                records: sst_records,
                uncompressed_size: 0, // Would need to calculate
                compression_algorithm: CompressionAlgorithmSst::None,
                compression_ratio: 1.0,
            };
            return Ok(Some(block));
        }

        // Cache miss - load from disk
        let block = self.load_block_from_disk(context, block_idx).await?;
        
        if let Some(block) = block.as_ref() {
            // Cache the block's vectors in central cache
            // Convert SstRecord to VectorRecord for caching
            let vector_records: Vec<VectorRecord> = block.records.iter().map(|r| {
                VectorRecord {
                    id: Some(r.id.clone()),
                    vector: r.vector.clone(),
                    metadata: r.metadata.clone(),
                    timestamp: r.timestamp,
                    updated_at: r.updated_at,
                    expires_at: r.expires_at,
                    version: r.version,
                    rank: None,
                    score: None,
                    distance: None,
                }
            }).collect();
            let _ = self.vector_cache.cache_block_vectors(&sst_cache_key, vector_records).await;
        }

        Ok(block)
    }

    /// Load a block from disk with cloud-optimized range requests
    async fn load_block_from_disk(&self, context: &CollectionContext, block_idx: usize) -> Result<Option<DataBlock>> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if context.file_path.contains("://") {
            context.file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}:///", scheme))?;
        
        // Use optimized cache with proper async-safe loading
        // Get the index from central cache
        let index = if let Some(cached_index) = self.index_node_cache.get_sstable_index(&context.file_path).await {
            // Convert from cache types to SST types
            let entries: Vec<IndexEntry> = cached_index.entries.iter().map(|e| IndexEntry {
                key: e.key.clone(),
                offset: e.block_offset,
                size: e.block_size as u32,
                block_id: 0, // Will be set from block offset if needed
                block_offset: e.block_offset as u32,
                compressed: true, // Default to compressed
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
            }).collect();
            
            let metadata_stats: HashMap<String, MetadataStats> = cached_index.metadata_stats.iter().map(|(k, v)| {
                (k.clone(), MetadataStats {
                    min_value: v.min_value.clone(),
                    max_value: v.max_value.clone(),
                    null_count: v.null_count,
                    distinct_count: v.distinct_count,
                    bloom_filter_offset: None,
                })
            }).collect();
            
            SstableIndex {
                entries,
                metadata_stats,
                vector_count: cached_index.total_vectors,
                min_key: cached_index.entries.first().map(|e| e.min_key.clone()).unwrap_or_default(),
                max_key: cached_index.entries.last().map(|e| e.max_key.clone()).unwrap_or_default(),
            }
        } else {
            let loaded_index = self.load_index_optimized(&context.file_path).await?;
            
            // Convert SST types to cache types for storage
            let cache_entries: Vec<crate::storage::cache::specialized::index_node_cache::SstIndexEntry> = 
                loaded_index.entries.iter().map(|e| crate::storage::cache::specialized::index_node_cache::SstIndexEntry {
                    key: e.key.clone(),
                    block_offset: e.offset,
                    block_size: e.size as usize,
                    min_key: e.key.clone(), // Use key as min for simplicity
                    max_key: e.key.clone(), // Use key as max for simplicity
                    vector_count: 1, // Approximate
                    bloom_filter_offset: None,
                }).collect();
            
            let cache_metadata_stats: HashMap<String, crate::storage::cache::specialized::index_node_cache::MetadataStats> = 
                loaded_index.metadata_stats.iter().map(|(k, v)| {
                    (k.clone(), crate::storage::cache::specialized::index_node_cache::MetadataStats {
                        min_value: v.min_value.clone(),
                        max_value: v.max_value.clone(),
                        null_count: v.null_count,
                        distinct_count: v.distinct_count,
                    })
                }).collect();
            
            let sstable_index = crate::storage::cache::specialized::index_node_cache::SstableIndex {
                file_path: context.file_path.clone(),
                entries: cache_entries,
                total_blocks: loaded_index.entries.len(),
                total_vectors: loaded_index.vector_count,
                metadata_stats: cache_metadata_stats,
            };
            self.index_node_cache.cache_sstable_index(&context.file_path, sstable_index).await?;
            loaded_index
        };
        
        // Check if block exists
        if block_idx >= index.entries.len() {
            return Ok(None);
        }
        
        // To find the block offset, we need to calculate the data section offset
        // Read and verify SST1 magic bytes
        let first_8_bytes = fs.read_range(&context.file_path, 0, 8).await?;
        if &first_8_bytes[0..4] != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SSTable format: missing SST1 magic bytes"));
        }
        
        let header_len = u32::from_le_bytes([
            first_8_bytes[4], first_8_bytes[5], first_8_bytes[6], first_8_bytes[7]
        ]) as u64;
        let header_offset = 8u64; // Skip magic + header_len
        
        // Read bloom filter length to skip it
        let bloom_offset = header_offset + header_len;
        let bloom_len_data = fs.read_range(&context.file_path, bloom_offset, 4).await?;
        let bloom_len = u32::from_le_bytes([
            bloom_len_data[0], bloom_len_data[1], bloom_len_data[2], bloom_len_data[3]
        ]) as u64;
        
        // Read index length to skip it
        let index_offset = bloom_offset + 4 + bloom_len;
        let index_len_data = fs.read_range(&context.file_path, index_offset, 4).await?;
        let index_len = u32::from_le_bytes([
            index_len_data[0], index_len_data[1], index_len_data[2], index_len_data[3]
        ]) as u64;
        
        // Calculate where data blocks start
        let data_section_offset = index_offset + 4 + index_len;
        
        // Now we need to find the specific block offset
        // For efficiency, we should store absolute offsets in the index, but for now
        // we'll read block lengths sequentially (this could be optimized further)
        let mut block_offset = data_section_offset;
        for _i in 0..block_idx {
            // Read block length
            let len_data = fs.read_range(&context.file_path, block_offset, 4).await?;
            let block_len = u32::from_le_bytes([
                len_data[0], len_data[1], len_data[2], len_data[3]
            ]) as u64;
            // Skip this block (length prefix + data)
            block_offset += 4 + block_len;
        }
        
        // Read the target block length
        let block_len_data = fs.read_range(&context.file_path, block_offset, 4).await?;
        let block_len = u32::from_le_bytes([
            block_len_data[0], block_len_data[1], block_len_data[2], block_len_data[3]
        ]) as u64;
        
        // Read the block data
        let block_data = fs.read_range(&context.file_path, block_offset + 4, block_len).await?;
        let block: DataBlock = DataBlock::deserialize(&block_data)?;
        
        debug!("Loaded block {} from SSTable using range request ({} bytes)", block_idx, block_len);
        Ok(Some(block))
    }

    /// Load index with cloud-optimized metadata reading
    async fn load_index_optimized(&self, file_path: &str) -> Result<SstableIndex> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}:///", scheme))?;
        
        // Read and verify SST1 magic bytes
        let first_8_bytes = fs.read_range(file_path, 0, 8).await?;
        if first_8_bytes.len() < 8 {
            return Err(anyhow::anyhow!(
                "SSTable file too small: expected at least 8 bytes, got {}",
                first_8_bytes.len()
            ));
        }
        
        if &first_8_bytes[0..4] != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SSTable format: missing SST1 magic bytes"));
        }
        
        debug!("SST1 format detected");
        let header_len = u32::from_le_bytes([
            first_8_bytes[4], first_8_bytes[5], first_8_bytes[6], first_8_bytes[7]
        ]) as u64;
        let header_offset = 8u64; // Skip magic + header_len
        
        // Read header
        let header_data = fs.read_range(file_path, header_offset, header_len).await?;
        if header_data.len() < header_len as usize {
            return Err(anyhow::anyhow!(
                "Failed to read complete header: expected {} bytes, got {}",
                header_len, header_data.len()
            ));
        }
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        
        // Calculate bloom filter offset and read its length
        let bloom_offset = header_offset + header_len;
        let bloom_len_data = fs.read_range(file_path, bloom_offset, 4).await?;
        if bloom_len_data.len() < 4 {
            return Err(anyhow::anyhow!(
                "Failed to read bloom filter length: expected 4 bytes, got {}",
                bloom_len_data.len()
            ));
        }
        let bloom_len = u32::from_le_bytes([
            bloom_len_data[0], bloom_len_data[1], bloom_len_data[2], bloom_len_data[3]
        ]) as u64;
        
        // Calculate index offset (skip bloom filter)
        let index_offset = bloom_offset + 4 + bloom_len;
        
        // Read index length
        let index_len_data = fs.read_range(file_path, index_offset, 4).await?;
        let index_len = u32::from_le_bytes([
            index_len_data[0], index_len_data[1], index_len_data[2], index_len_data[3]
        ]) as u64;
        
        // Read index data
        let index_data = fs.read_range(file_path, index_offset + 4, index_len).await?;
        
        // Deserialize index entries using custom deserialization
        let mut entries = Vec::new();
        let mut cursor = std::io::Cursor::new(&index_data[..]);
        
        while (cursor.position() as usize) < index_data.len() {
            let mut len_buf = [0u8; 4];
            if cursor.read_exact(&mut len_buf).is_err() {
                break; // End of data
            }
            let entry_len = u32::from_le_bytes(len_buf) as usize;
            
            let current_pos = cursor.position() as usize;
            if current_pos + entry_len > index_data.len() {
                break; // Invalid entry length
            }
            
            let entry_data = &index_data[current_pos..current_pos + entry_len];
            match IndexEntry::deserialize(entry_data) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    warn!("Failed to deserialize index entry: {}", e);
                    break;
                }
            }
            
            cursor.set_position((current_pos + entry_len) as u64);
        }
        
        // Build metadata statistics from index entries
        let mut metadata_stats = HashMap::new();
        
        // Aggregate metadata statistics across all blocks
        for entry in &entries {
            for (column, min_val) in &entry.metadata_min_values {
                let stats = metadata_stats.entry(column.clone()).or_insert(MetadataStats {
                    min_value: min_val.clone(),
                    max_value: min_val.clone(),
                    null_count: 0,
                    distinct_count: 0,
                    bloom_filter_offset: Some(bloom_offset + 4), // Bloom filter location
                });
                
                // Update min value
                if Self::compare_metadata_values(min_val, &stats.min_value) == std::cmp::Ordering::Less {
                    stats.min_value = min_val.clone();
                }
            }
            
            for (column, max_val) in &entry.metadata_max_values {
                let stats = metadata_stats.entry(column.clone()).or_insert(MetadataStats {
                    min_value: max_val.clone(),
                    max_value: max_val.clone(),
                    null_count: 0,
                    distinct_count: 0,
                    bloom_filter_offset: Some(bloom_offset + 4),
                });
                
                // Update max value
                if Self::compare_metadata_values(max_val, &stats.max_value) == std::cmp::Ordering::Greater {
                    stats.max_value = max_val.clone();
                }
            }
            
            // Update null counts
            for (column, null_count) in &entry.metadata_null_counts {
                let stats = metadata_stats.entry(column.clone()).or_insert(MetadataStats {
                    min_value: serde_json::Value::Null,
                    max_value: serde_json::Value::Null,
                    null_count: 0,
                    distinct_count: 0,
                    bloom_filter_offset: Some(bloom_offset + 4),
                });
                stats.null_count += *null_count as usize;
            }
        }
        
        debug!("Built metadata statistics for {} columns", metadata_stats.len());
        
        let index = SstableIndex {
            entries,
            metadata_stats,
            vector_count: header.entry_count as usize,
            min_key: header.min_key,
            max_key: header.max_key,
        };
        
        // Note: We don't cache here anymore as the caller handles caching
        // to avoid double-locking issues
        
        Ok(index)
    }

    /// Simple get operation for single vector retrieval
    /// This provides a lightweight interface for basic get operations
    pub async fn get_vector(&self, file_path: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        debug!("🔍 get_vector: Looking for vector '{}' in file '{}'", vector_id, file_path);
        
        // Check bloom filter for quick rejection using the proper bloom filter
        // The bloom_cache bitmap is just a marker, not the actual bloom filter
        // We need to use might_contain_key which loads and checks the actual bloom filter
        if !self.might_contain_key(file_path, vector_id).await {
            debug!("❌ Bloom filter says vector '{}' not in file", vector_id);
            return Ok(None);
        }
        debug!("✅ Bloom filter says vector '{}' might be in file", vector_id);

        // Create minimal context for the operation
        let context = CollectionContext {
            file_path: file_path.to_string(),
            sstable_files: vec![file_path.to_string()],
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
        };

        // Use full scan strategy for single key lookup
        // TODO: Optimize with index-based lookup
        let strategy = SstableReadingStrategy::FullScan {
            use_block_cache: true,
        };

        // Load blocks and search for the vector
        let blocks = self.apply_strategy(&strategy, &Default::default(), &context).await?;
        debug!("📦 Loaded {} blocks from file", blocks.len());
        
        // Search through blocks for the vector
        for (block_idx, block) in blocks.iter().enumerate() {
            debug!("  Block {}: {} records", block_idx, block.records.len());
            for record in &block.records {
                debug!("    Checking record: id='{}' vs looking for '{}'", record.id, vector_id);
                if record.id == vector_id {
                    // Convert HashMap metadata to Vec<MetadataItem>
                    // Already have metadata items, just clone them
                    let metadata_items = record.metadata.clone();
                    
                    return Ok(Some(VectorRecord {
                        id: Some(record.id.clone()),
                        vector: record.vector.clone(),
                        metadata: metadata_items,
                        timestamp: record.timestamp,
                        updated_at: record.updated_at,
                        expires_at: record.expires_at,
                        version: record.version.map(|v| v as u32),
                        distance: None,
                        score: None,
                        rank: None,
                    
        }));
                }
            }
        }

        Ok(None)
    }

    /// Check if a key might be contained using bloom filter
    pub async fn might_contain_key(&self, file_path: &str, key: &str) -> bool {
        // Try to load the bloom filter if not cached
        match self.load_bloom_filter(file_path).await {
            Ok(Some(bloom_filter)) => {
                // Check if the key might be in the bloom filter
                bloom_filter.might_contain_key(key).unwrap_or(true)
            }
            Ok(None) => {
                // No bloom filter available
                true // Assume it might contain
            }
            Err(_) => {
                // Error loading bloom filter
                true // Assume it might contain
            }
        }
    }

    /// Load just the bloom filter from an SSTable file
    async fn load_bloom_filter(&self, file_path: &str) -> Result<Option<SstableBloomFilter>> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file").to_string()
        } else {
            "file".to_string()
        };
        let fs = self.filesystem.get_filesystem(&scheme)?;
        
        // Read magic bytes and header length
        let header_prefix = fs.read_range(file_path, 0, 8).await?;
        if header_prefix.len() < 8 {
            return Ok(None);
        }
        
        // Check magic bytes "SST1"
        let magic = &header_prefix[0..4];
        if magic != b"SST1" {
            return Ok(None);
        }
        
        let header_len = u32::from_le_bytes([
            header_prefix[4], header_prefix[5], header_prefix[6], header_prefix[7]
        ]) as u64;
        
        // Read header (offset by 8 bytes for magic + header_len)
        let header_data = fs.read_range(file_path, 8, header_len).await?;
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        
        // Read bloom filter if present
        if header.has_bloom_filter {
            let bloom_offset = 8 + header_len; // 8 = magic (4) + header_len (4)
            let bloom_len_data = fs.read_range(file_path, bloom_offset, 4).await?;
            if bloom_len_data.len() < 4 {
                return Ok(None);
            }
            
            let bloom_len = u32::from_le_bytes([
                bloom_len_data[0], bloom_len_data[1], bloom_len_data[2], bloom_len_data[3]
            ]) as u64;
            
            let bloom_data = fs.read_range(file_path, bloom_offset + 4, bloom_len).await?;
            
            match SstableBloomFilter::deserialize(&bloom_data) {
                Ok(bloom) => Ok(Some(bloom)),
                Err(_) => Ok(None)
            }
        } else {
            Ok(None)
        }
    }
    
    /// Load metadata for an SSTable (header and bloom filter)
    pub async fn load_metadata(&self, file_path: &str) -> Result<()> {
        // Extract scheme from file path for proper filesystem selection
        let scheme = if file_path.contains("://") {
            file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}:///", scheme))?;
        
        // First read magic bytes (4 bytes) and header length (4 bytes)
        let header_prefix = fs.read_range(file_path, 0, 8).await?;
        if header_prefix.len() < 8 {
            return Err(anyhow::anyhow!("SSTable file too small: {} bytes", header_prefix.len()));
        }
        
        // Verify magic bytes
        let magic = &header_prefix[0..4];
        if magic != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SSTable magic bytes: {:?}", magic));
        }
        
        let header_len = u32::from_le_bytes([
            header_prefix[4], header_prefix[5], header_prefix[6], header_prefix[7]
        ]) as u64;
        
        debug!("Header length: {} bytes", header_len);
        
        // Read the header data (offset by 8 bytes for magic + header_len)
        let header_data = fs.read_range(file_path, 8, header_len).await?;
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        
        debug!("Header info: version={}, has_bloom={}, entry_count={}", 
               header.version, header.has_bloom_filter, header.entry_count);
        
        // Read bloom filter if present
        if header.has_bloom_filter {
            // Calculate bloom filter offset (after magic + header_len + header)
            let bloom_offset = 8 + header_len;
            
            // Read bloom filter length
            let bloom_len_data = fs.read_range(file_path, bloom_offset, 4).await?;
            if bloom_len_data.len() < 4 {
                return Err(anyhow::anyhow!(
                    "Failed to read bloom filter length: expected 4 bytes, got {}",
                    bloom_len_data.len()
                ));
            }
            let bloom_len = u32::from_le_bytes([
                bloom_len_data[0], bloom_len_data[1], bloom_len_data[2], bloom_len_data[3]
            ]) as u64;
            
            debug!("Reading bloom filter: offset={}, length={}", bloom_offset + 4, bloom_len);
            debug!("Bloom length bytes: {:?}", bloom_len_data);
            
            // Check file size
            let file_metadata = fs.metadata(file_path).await?;
            debug!("File size: {} bytes", file_metadata.size);
            
            // Read bloom filter data
            let bloom_data = fs.read_range(file_path, bloom_offset + 4, bloom_len).await?;
            debug!("Actually read {} bytes of bloom data", bloom_data.len());
            if bloom_data.len() < bloom_len as usize {
                return Err(anyhow::anyhow!(
                    "Failed to read complete bloom filter: expected {} bytes, got {}",
                    bloom_len, bloom_data.len()
                ));
            }
            debug!("Bloom data first 20 bytes: {:?}", &bloom_data[..bloom_data.len().min(20)]);
            
            let _bloom_filter: SstableBloomFilter = match SstableBloomFilter::deserialize(&bloom_data) {
                Ok(bf) => bf,
                Err(e) => {
                    warn!("Deserialization error: {:?}", e);
                    warn!("Expected SstableBloomFilter, got {} bytes", bloom_data.len());
                    
                    // Try to understand what we're actually reading
                    if bloom_data.len() >= 8 {
                        let first_u64 = u64::from_le_bytes(bloom_data[0..8].try_into().unwrap());
                        debug!("First u64 in bloom data: {}", first_u64);
                    }
                    
                    // Log the actual error for debugging
                    tracing::warn!("Failed to deserialize bloom filter, creating empty one: {}", e);
                    
                    // Create an empty bloom filter as fallback
                    // This allows the SSTable to be read even if the bloom filter is corrupted
                    let key_filter_config = BloomFilterConfig {
                        strategy: BloomStrategy::ByteAligned,
                        expected_items: 1000,
                        bits_per_key: 10,
                        enabled: false, // Disable since we couldn't load it
                        ..Default::default()
                    };
                    
                    SstableBloomFilter::new(
                        key_filter_config,
                        vec![],
                        vec![],
                        super::super::bloom_filter::BloomFilterStats::default(),
                    )
                }
            };
            
            // Cache the bloom filter in central cache
            // We need to store the bloom filter somewhere accessible
            // For now, we'll use a simple in-memory cache (should be improved)
            
            // Store a marker in the bitmap cache that bloom filter exists
            let mut bitmap = roaring::RoaringBitmap::new();
            // We'll use a hash of the file path as the marker
            let file_hash = file_path.as_bytes().iter().fold(0u32, |acc, &b| {
                acc.wrapping_mul(31).wrapping_add(b as u32)
            });
            bitmap.insert(file_hash);
            
            let cached_filter = crate::storage::cache::specialized::bitmap_filter_cache::CachedFilterResult {
                bitmap,
                filter_expr: format!("sstable:bloom:{}", file_path),
                cached_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                dependencies: vec![],
            };
            self.bloom_cache.put_with_hooks(file_path.to_string(), cached_filter).await;
            
            debug!("Loaded bloom filter for SSTable: {} ({} bytes)", file_path, bloom_len);
        }
        
        debug!("Loaded metadata for SSTable: {}", file_path);
        Ok(())
    }

    
    async fn load_specific_blocks(
        &self,
        context: &CollectionContext,
        blocks: &[usize],
    ) -> Result<Vec<DataBlock>> {
        let mut loaded_blocks = Vec::new();
        
        for &block_idx in blocks {
            if let Some(block) = self.load_block_with_cache(context, block_idx).await? {
                loaded_blocks.push(block);
            }
        }
        
        Ok(loaded_blocks)
    }
    
    async fn read_file_with_cache(&self, path: &str) -> Result<Vec<DataBlock>> {
        // Use optimized cache with proper LRU eviction
        // Get the index from central cache
        let index = if let Some(cached_index) = self.index_node_cache.get_sstable_index(path).await {
            // Convert from cache types to SST types
            let entries: Vec<IndexEntry> = cached_index.entries.iter().map(|e| IndexEntry {
                key: e.key.clone(),
                offset: e.block_offset,
                size: e.block_size as u32,
                block_id: 0,
                block_offset: e.block_offset as u32,
                compressed: true,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
            }).collect();
            
            let metadata_stats: HashMap<String, MetadataStats> = cached_index.metadata_stats.iter().map(|(k, v)| {
                (k.clone(), MetadataStats {
                    min_value: v.min_value.clone(),
                    max_value: v.max_value.clone(),
                    null_count: v.null_count,
                    distinct_count: v.distinct_count,
                    bloom_filter_offset: None,
                })
            }).collect();
            
            SstableIndex {
                entries,
                metadata_stats,
                vector_count: cached_index.total_vectors,
                min_key: cached_index.entries.first().map(|e| e.min_key.clone()).unwrap_or_default(),
                max_key: cached_index.entries.last().map(|e| e.max_key.clone()).unwrap_or_default(),
            }
        } else {
            let loaded_index = self.load_index_optimized(path).await?;
            
            // Convert SST types to cache types for storage
            let cache_entries: Vec<crate::storage::cache::specialized::index_node_cache::SstIndexEntry> = 
                loaded_index.entries.iter().map(|e| crate::storage::cache::specialized::index_node_cache::SstIndexEntry {
                    key: e.key.clone(),
                    block_offset: e.offset,
                    block_size: e.size as usize,
                    min_key: e.key.clone(),
                    max_key: e.key.clone(),
                    vector_count: 1,
                    bloom_filter_offset: None,
                }).collect();
            
            let cache_metadata_stats: HashMap<String, crate::storage::cache::specialized::index_node_cache::MetadataStats> = 
                loaded_index.metadata_stats.iter().map(|(k, v)| {
                    (k.clone(), crate::storage::cache::specialized::index_node_cache::MetadataStats {
                        min_value: v.min_value.clone(),
                        max_value: v.max_value.clone(),
                        null_count: v.null_count,
                        distinct_count: v.distinct_count,
                    })
                }).collect();
            
            let sstable_index = crate::storage::cache::specialized::index_node_cache::SstableIndex {
                file_path: path.to_string(),
                entries: cache_entries,
                total_blocks: loaded_index.entries.len(),
                total_vectors: loaded_index.vector_count,
                metadata_stats: cache_metadata_stats,
            };
            self.index_node_cache.cache_sstable_index(path, sstable_index).await?;
            loaded_index
        };
        
        let num_blocks = index.entries.len();
        
        // Load all blocks using cache
        let mut blocks = Vec::new();
        let context = CollectionContext {
            file_path: path.to_string(),
            sstable_files: vec![path.to_string()],
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
        };
        
        for block_idx in 0..num_blocks {
            if let Some(block) = self.load_block_with_cache(&context, block_idx).await? {
                blocks.push(block);
            }
        }
        
        Ok(blocks)
    }
    
    async fn read_file_direct(&self, path: &str) -> Result<Vec<DataBlock>> {
        // Load index directly without caching (true direct access)
        let index = Arc::new(self.load_index_optimized(path).await?);
        
        // Extract scheme from path for proper filesystem selection
        let scheme = if path.contains("://") {
            path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}:///", scheme))?;
        
        // Read the full file
        let data = fs.read(path).await?;
        let mut offset = 0usize;
        
        // Verify SST1 magic bytes
        if data.len() < 8 {
            return Ok(vec![]);
        }
        
        if &data[0..4] != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SSTable format: missing SST1 magic bytes"));
        }
        
        offset += 4; // Skip magic
        let header_len = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        offset += 4; // Skip header length field
        offset += header_len;
        
        // Skip bloom filter
        if offset + 4 > data.len() {
            return Ok(vec![]);
        }
        let bloom_len = u32::from_le_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
        ]) as usize;
        offset += 4 + bloom_len;
        
        // Skip index
        if offset + 4 > data.len() {
            return Ok(vec![]);
        }
        let index_len = u32::from_le_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
        ]) as usize;
        offset += 4 + index_len;
        
        // Read all data blocks
        let mut blocks = Vec::new();
        debug!("Starting to read data blocks at offset {}", offset);
        debug!("Total file size: {}, remaining data: {}", data.len(), data.len() - offset);
        
        // Decode header to get block count (header always starts at offset 8 after magic + header_len)
        let header_start = 8; // Always 8: magic (4) + header_len (4)
        let header: SstableHeader = bincode::deserialize(&data[header_start..header_start+header_len])
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header for block count: {}", e))?;
        debug!("Header info: {} blocks expected, {} index entries", header.block_count, index.entries.len());
        
        // Verify we have data blocks section
        if offset >= data.len() {
            warn!("No data blocks section found! File ends at index section.");
            warn!("File structure: header_len={}, bloom_len={}, index_len={}, total_size={}", 
                  header_len, bloom_len, index_len, data.len());
            return Ok(blocks); // Return empty blocks
        }
        
        while offset + 4 <= data.len() {
            let block_len = u32::from_le_bytes([
                data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
            ]) as usize;
            offset += 4;
            
            debug!("Reading block of length {} at offset {}", block_len, offset - 4);
            
            if offset + block_len > data.len() {
                warn!("Not enough data for block (need {}, have {})", block_len, data.len() - offset);
                warn!("Current offset: {}, block_len: {}, total file size: {}", offset, block_len, data.len());
                break;
            }
            
            let block_data = &data[offset..offset + block_len];
            
            // Debug: Check if block data starts with expected magic header
            if block_data.len() >= 4 {
                let magic = &block_data[0..4];
                debug!("Block magic header: {:?} (expecting b\"BLK1\")", 
                       std::str::from_utf8(magic).unwrap_or("invalid"));
            }
            
            debug!("🔍 Deserializing block data of {} bytes at offset {}", block_data.len(), offset - 4);
            match DataBlock::deserialize(block_data) {
                Ok(block) => {
                    debug!("✅ Successfully deserialized block with {} records", block.records.len());
                    // Debug: Print all record IDs for debugging
                    for (i, record) in block.records.iter().enumerate() {
                        debug!("  Record {}: id='{}', vector_len={}, metadata_keys={:?}", 
                               i, record.id, record.vector.len(), 
                               record.metadata.iter().map(|item| &item.key).collect::<Vec<_>>());
                    }
                    blocks.push(block);
                }
                Err(e) => {
                    warn!("Failed to deserialize block at offset {}: {:?}", offset - 4, e);
                    // Debug: Print first few bytes of the problematic block
                    let preview_len = std::cmp::min(block_data.len(), 32);
                    warn!("Block data preview (first {} bytes): {:?}", preview_len, &block_data[..preview_len]);
                    
                    // Try to understand what's in the block
                    if block_data.len() >= 4 {
                        let magic = &block_data[0..4];
                        warn!("Found magic: {:?}, expected: {:?}", magic, b"BLK1");
                        
                        
                        // Check if it might be bincode or JSON
                        if let Ok(s) = std::str::from_utf8(&block_data[..std::cmp::min(block_data.len(), 100)]) {
                            warn!("Block as string (first 100 chars): {}", s);
                        }
                    }
                    
                    // Continue processing other blocks even if one fails
                    warn!("Skipping corrupted block at offset {}", offset - 4);
                }
            }
            offset += block_len;
        }
        
        debug!("Total blocks read: {}", blocks.len());
        
        // Print records in each block
        for (i, block) in blocks.iter().enumerate() {
            debug!("Block {} has {} records", i, block.records.len());
            for (j, record) in block.records.iter().take(3).enumerate() {
                debug!("Record {}: id={}, vector_len={}, tombstone={}", 
                         j, record.id, record.vector.len(), record.is_tombstone);
            }
        }
        
        Ok(blocks)
    }
    
    
    
    fn evaluate_filter(&self, expr: &FilterExpression, metadata: &HashMap<String, serde_json::Value>) -> bool {
        // Use centralized filter evaluation from search module
        crate::core::search::json_comparison::evaluate_filter(expr, metadata)
    }
    
    /// Convert MetadataItem vector to JSON HashMap - optimized for high-performance hot paths
    #[inline(always)]
    fn metadata_items_to_json(&self, items: &[crate::proto::proximadb::MetadataItem]) -> HashMap<String, serde_json::Value> {
        // Pre-allocate HashMap to exact size to avoid reallocations
        let mut map = HashMap::with_capacity(items.len());
        for item in items {
            let value = match &item.value {
                Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => 
                    serde_json::Value::String(s.clone()),
                Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => 
                    serde_json::Number::from_f64(*n)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => 
                    serde_json::Value::Bool(*b),
                None => serde_json::Value::Null,
            };
            map.insert(item.key.clone(), value);
        }
        map
    }
    
    /// Compare metadata values for ordering
    fn compare_metadata_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        crate::core::search::json_comparison::compare_json_values(a, b)
    }
}

impl ReadingStrategySelector {
    pub fn new(config: ReaderConfig) -> Self {
        Self { config }
    }
    
    pub fn select_strategy(
        &self,
        params: &SearchParams,
        context: &CollectionContext,
    ) -> Result<SstableReadingStrategy> {
        // Strategy selection logic based on:
        // 1. Presence of metadata filters
        // 2. File size and count
        // 3. Query selectivity estimate
        
        if params.filter_expression.is_some() {
            // Metadata filtering present - use filtered strategy
            Ok(SstableReadingStrategy::MetadataFiltered {
                selected_blocks: vec![], // Would be populated based on metadata
                skip_bloom_check: false,
            })
        } else if context.sstable_files.len() > self.config.range_scan_threshold {
            // Many files - use index range scan
            Ok(SstableReadingStrategy::IndexRangeScan {
                start_block: 0,
                end_block: 10, // Would be calculated
                use_bloom_filter: true,
            })
        } else {
            // Small dataset - full scan with cache
            Ok(SstableReadingStrategy::FullScan {
                use_block_cache: true,
            })
        }
    }
}

impl BlockCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(tokio::sync::RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(max_size).unwrap()
            ))),
            max_size,
            hit_rate: Arc::new(tokio::sync::RwLock::new(CacheStats::default())),
        }
    }
}

impl IndexCache {
    pub fn new() -> Self {
        Self::with_config(100) // Default 100MB limit
    }
    
    pub fn with_config(max_memory_mb: usize) -> Self {
        use std::time::Duration;
        
        // Calculate max entries based on estimated size per index (~1MB each)
        let max_indices = (max_memory_mb * 1024 * 1024) / (1024 * 1024); // ~1MB per index
        let max_bloom_filters = (max_memory_mb * 1024 * 1024) / (50 * 1024); // ~50KB per bloom filter
        
        let indices = moka::future::Cache::builder()
            .max_capacity(max_indices as u64)
            .time_to_live(Duration::from_secs(3600)) // 1 hour TTL
            .eviction_listener(|_key, _value, _cause| {
                tracing::debug!("Evicted index cache entry");
            })
            .build();
            
        let bloom_filters = moka::future::Cache::builder()
            .max_capacity(max_bloom_filters as u64)
            .time_to_live(Duration::from_secs(3600))
            .eviction_listener(|_key, _value, _cause| {
                tracing::debug!("Evicted bloom filter cache entry");
            })
            .build();
        
        Self {
            indices: Arc::new(indices),
            bloom_filters: Arc::new(bloom_filters),
            max_memory_mb,
            metrics: Arc::new(tokio::sync::RwLock::new(CacheMetrics::default())),
        }
    }
    
    /// Get current memory usage in MB
    pub async fn memory_usage_mb(&self) -> f64 {
        let metrics = self.metrics.read().await;
        metrics.memory_usage_bytes as f64 / (1024.0 * 1024.0)
    }
    
    /// Get cache hit rate
    pub async fn hit_rate(&self) -> f64 {
        let metrics = self.metrics.read().await;
        let total_requests = metrics.hit_count + metrics.miss_count;
        if total_requests == 0 {
            0.0
        } else {
            metrics.hit_count as f64 / total_requests as f64
        }
    }
    
    /// Handle memory pressure by clearing least recently used entries
    pub async fn handle_memory_pressure(&self, pressure_level: MemoryPressure) {
        let mut metrics = self.metrics.write().await;
        metrics.memory_pressure_events += 1;
        
        match pressure_level {
            MemoryPressure::Low => {
                // Reduce TTL to encourage natural eviction
                // This would require rebuilding cache - for now just invalidate 10%
                let current_size = self.indices.entry_count();
                let to_remove = (current_size as f64 * 0.1) as usize;
                for _ in 0..to_remove {
                    self.indices.invalidate_all();
                    break; // Simplified - in production would remove specific entries
                }
            },
            MemoryPressure::Medium => {
                // More aggressive cleanup - remove 25%
                let current_size = self.indices.entry_count() + self.bloom_filters.entry_count();
                if current_size > 0 {
                    self.indices.invalidate_all();
                }
            },
            MemoryPressure::High => {
                // Emergency cleanup - clear all caches
                self.indices.invalidate_all();
                self.bloom_filters.invalidate_all();
                tracing::warn!("Emergency cache cleanup due to high memory pressure");
            }
        }
    }
    
    /// Get index from cache or load if not present
    pub async fn get_or_load_index<F, Fut>(&self, key: &str, loader: F) -> Result<Arc<SstableIndex>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<SstableIndex>>,
    {
        // Try cache first
        if let Some(index) = self.indices.get(key).await {
            let mut metrics = self.metrics.write().await;
            metrics.hit_count += 1;
            return Ok(index);
        }
        
        // Cache miss - load and store
        let mut metrics = self.metrics.write().await;
        metrics.miss_count += 1;
        drop(metrics); // Release lock before async operation
        
        let index = Arc::new(loader().await?);
        
        // Update memory usage estimate
        let estimated_size = std::mem::size_of::<SstableIndex>() + 
                            index.entries.len() * std::mem::size_of::<IndexEntry>();
        
        self.indices.insert(key.to_string(), index.clone()).await;
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.memory_usage_bytes += estimated_size;
        
        Ok(index)
    }
}

/// Memory pressure levels for adaptive cache management
#[derive(Debug, Clone, Copy)]
pub enum MemoryPressure {
    Low,    // 70-80% of limit
    Medium, // 80-90% of limit  
    High,   // >90% of limit
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            block_cache_size: 1000,
            index_cache_size: 100,
            bloom_filter_threshold: 0.01,
            range_scan_threshold: 10,
            metadata_selectivity_threshold: 0.1,
            enable_read_ahead: true,
            read_ahead_blocks: 5,
        }
    }
}