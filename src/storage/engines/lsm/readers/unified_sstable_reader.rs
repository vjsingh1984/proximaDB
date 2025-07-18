//! Unified SSTable Reader Architecture
//!
//! This module provides an optimized reader for LSM SSTables with:
//! - Block-level access with caching
//! - Metadata bloom filters for efficient filtering
//! - Index-based range scans
//! - Predicate pushdown to block level
//! - Unified search interface integration

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::VectorRecord;
use crate::core::search::{SearchParams, SearchResult, FilterExpression};
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::engines::lsm::bloom_filter::{
    MetadataBloomFilter, SstableBloomFilter, BloomFilterConfig, MetadataBloomFilterBuilder
};
use crate::storage::engines::lsm::{SstableHeader, LsmRecord};

/// Unified SSTable Reader with automatic optimization selection
pub struct UnifiedSstableReader {
    filesystem: Arc<FilesystemFactory>,
    block_cache: Arc<BlockCache>,
    index_cache: Arc<IndexCache>,
    strategy_selector: Arc<ReadingStrategySelector>,
    distance_compute: Arc<UnifiedDistanceCompute>,
}

/// Block cache for frequently accessed data blocks
pub struct BlockCache {
    cache: Arc<tokio::sync::RwLock<lru::LruCache<BlockCacheKey, Arc<DataBlock>>>>,
    max_size: usize,
    hit_rate: Arc<tokio::sync::RwLock<CacheStats>>,
}

/// Index cache for SSTable indices
pub struct IndexCache {
    indices: Arc<tokio::sync::RwLock<HashMap<String, Arc<SstableIndex>>>>,
    bloom_filters: Arc<tokio::sync::RwLock<HashMap<String, Arc<SstableBloomFilter>>>>,
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
    pub block_id: usize,
    pub block_index: usize,
}

/// Data block with vectors and metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataBlock {
    pub block_id: usize,
    pub records: Vec<LsmRecord>,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
}

/// Index entry for block lookup
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexEntry {
    pub block_id: usize,
    pub offset: u64,
    pub size: u64,
    pub first_key: String,
    pub last_key: String,
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
    pub collection_id: String,
    pub file_path: String,
    pub sstable_files: Vec<String>,
    pub total_vectors: usize,
    pub metadata_columns: Vec<String>,
    pub level: usize,
    pub creation_time: chrono::DateTime<chrono::Utc>,
}

impl UnifiedSstableReader {
    /// Create a new unified reader
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        let config = ReaderConfig::default();
        Self {
            filesystem,
            block_cache: Arc::new(BlockCache::new(config.block_cache_size)),
            index_cache: Arc::new(IndexCache::new()),
            strategy_selector: Arc::new(ReadingStrategySelector::new(config)),
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
        }
    }
    
    /// Search vectors using optimized strategies
    pub async fn search_vectors(
        &self,
        params: &SearchParams,
        collection_context: &CollectionContext,
    ) -> Result<Vec<SearchResult>> {
        info!("🔍 LSM Unified Search: {} files, k={}", 
              collection_context.sstable_files.len(),
              params.top_k.unwrap_or(10));
        
        // 1. Select optimal reading strategy
        let strategy = self.strategy_selector.select_strategy(params, collection_context)?;
        debug!("📊 Selected strategy: {:?}", strategy);
        
        // 2. Apply strategy to read relevant blocks
        let relevant_blocks = self.apply_strategy(&strategy, params, collection_context).await?;
        
        // 3. Perform vector search on loaded data
        let results = self.search_in_blocks(params, &relevant_blocks, &collection_context.collection_id).await?;
        
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
    
    /// Perform vector search in loaded blocks
    async fn search_in_blocks(
        &self,
        params: &SearchParams,
        blocks: &[DataBlock],
        collection_id: &str,
    ) -> Result<Vec<SearchResult>> {
        let query_vector = params.first_query_vector()
            .ok_or_else(|| anyhow::anyhow!("Query vector required"))?;
        
        let k = params.top_k.unwrap_or(10);
        let distance_metric = params.distance_metric.unwrap_or(crate::compute::distance::DistanceMetric::Cosine);
        
        // Compute distances for all vectors
        let mut scored_results = Vec::new();
        
        for block in blocks {
            for record in &block.records {
                if record.is_tombstone {
                    continue;
                }
                
                // Apply metadata filters
                if let Some(filter_expr) = &params.filter_expression {
                    if !self.evaluate_filter(filter_expr, &record.metadata) {
                        continue;
                    }
                }
                
                // Compute distance
                let similarity = self.distance_compute.calculate_distance(
                    query_vector,
                    &record.vector,
                    &distance_metric,
                );
                
                scored_results.push(SearchResult {
                    id: record.id.clone(),
                    score: similarity.normalized_score,
                    distance: Some(similarity.raw_value),
                    rank: None,
                    vector: Some(record.vector.clone()),
                    vector_id: None,
                    metadata: record.metadata.clone(),
                    debug_info: None,
                    semantic_distance: Some(similarity),
                    collection_id: None,
                    created_at: None,
                    engine_stats: None,
                    quantization_info: None,
                    index_path: None,
                });
            }
        }
        
        // Sort by score (descending) and take top k
        scored_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        scored_results.truncate(k);
        
        Ok(scored_results)
    }
    
    /// Full scan strategy implementation
    async fn full_scan_strategy(
        &self,
        context: &CollectionContext,
        use_block_cache: bool,
    ) -> Result<Vec<DataBlock>> {
        let mut all_blocks = Vec::new();
        
        for file_path in &context.sstable_files {
            let blocks = if use_block_cache {
                self.read_file_with_cache(file_path).await?
            } else {
                self.read_file_direct(file_path).await?
            };
            all_blocks.extend(blocks);
        }
        
        Ok(all_blocks)
    }
    
    // Placeholder implementations for other strategies
    async fn index_range_scan_strategy(
        &self,
        _context: &CollectionContext,
        _start: usize,
        _end: usize,
        _use_bloom: bool,
    ) -> Result<Vec<DataBlock>> {
        Ok(Vec::new())
    }
    
    async fn metadata_filtered_strategy(
        &self,
        context: &CollectionContext,
        params: &SearchParams,
        blocks: &[usize],
        skip_bloom: bool,
    ) -> Result<Vec<DataBlock>> {
        debug!("Using metadata filtered strategy for {} blocks", blocks.len());

        // Get bloom filter for this SSTable
        let bloom_filter = if !skip_bloom {
            let bloom_filters = self.index_cache.bloom_filters.read().await;
            bloom_filters.get(&context.file_path).cloned()
        } else {
            None
        };

        // Extract metadata filters from search params
        let metadata_conditions = self.extract_metadata_conditions(params);

        // Filter blocks using bloom filter if available
        let filtered_blocks = if let Some(bloom_filter) = bloom_filter {
            let mut filtered = Vec::new();
            // Check metadata conditions against bloom filter
            let mut conditions_match = true;
            for (column, value) in &metadata_conditions {
                if !bloom_filter.metadata_filter.might_match_metadata(column, value) {
                    conditions_match = false;
                    break;
                }
            }
            
            if conditions_match {
                // If metadata might match, include all blocks
                filtered.extend_from_slice(blocks);
            } else {
                debug!("Bloom filter skipped all blocks (metadata mismatch)");
            }
            filtered
        } else {
            blocks.to_vec()
        };

        // Load the filtered blocks
        let mut result_blocks = Vec::new();
        for block_idx in filtered_blocks {
            if let Some(block) = self.load_block_with_cache(context, block_idx).await? {
                result_blocks.push(block);
            }
        }

        info!("Loaded {} blocks after metadata filtering", result_blocks.len());
        Ok(result_blocks)
    }

    /// Extract metadata conditions from search params
    fn extract_metadata_conditions(&self, params: &SearchParams) -> HashMap<String, String> {
        let mut conditions = HashMap::new();
        
        // Extract from simple filters
        if let Some(ref filters) = params.filters {
            for (key, value) in filters {
                if let Some(string_value) = value.as_str() {
                    conditions.insert(key.clone(), string_value.to_string());
                }
            }
        }

        // Extract from complex filter expressions
        if let Some(ref filter_expr) = params.filter_expression {
            self.extract_from_filter_expression(filter_expr, &mut conditions);
        }

        conditions
    }

    /// Extract metadata conditions from complex filter expressions
    fn extract_from_filter_expression(&self, expr: &FilterExpression, conditions: &mut HashMap<String, String>) {
        match expr {
            FilterExpression::And(exprs) | FilterExpression::Or(exprs) => {
                for expr in exprs {
                    self.extract_from_filter_expression(expr, conditions);
                }
            }
            FilterExpression::Not(expr) => {
                self.extract_from_filter_expression(expr, conditions);
            }
            FilterExpression::Comparison { field, value, .. } => {
                if let Some(string_value) = value.as_str() {
                    conditions.insert(field.clone(), string_value.to_string());
                }
            }
        }
    }

    /// Load a specific block with caching
    async fn load_block_with_cache(&self, context: &CollectionContext, block_idx: usize) -> Result<Option<DataBlock>> {
        let cache_key = BlockCacheKey {
            file_path: context.file_path.clone(),
            block_id: block_idx,
            block_index: block_idx,
        };

        // Check cache first
        {
            let mut cache = self.block_cache.cache.write().await;
            if let Some(block) = cache.get(&cache_key) {
                // Update cache stats
                let mut stats = self.block_cache.hit_rate.write().await;
                stats.hits += 1;
                return Ok(Some((**block).clone()));
            }
        }

        // Cache miss - load from disk
        let block = self.load_block_from_disk(context, block_idx).await?;
        
        if let Some(block) = block.as_ref() {
            // Cache the block
            let mut cache = self.block_cache.cache.write().await;
            cache.put(cache_key, Arc::new(block.clone()));
        }

        // Update cache stats
        let mut stats = self.block_cache.hit_rate.write().await;
        stats.misses += 1;

        Ok(block)
    }

    /// Load a block from disk with cloud-optimized range requests
    async fn load_block_from_disk(&self, context: &CollectionContext, block_idx: usize) -> Result<Option<DataBlock>> {
        // Get block offset and size from index
        let indices = self.index_cache.indices.read().await;
        let index = indices.get(&context.file_path).ok_or_else(|| {
            anyhow::anyhow!("No index found for file: {}", context.file_path)
        })?;
        
        if block_idx >= index.entries.len() {
            return Ok(None);
        }
        
        let entry = &index.entries[block_idx];
        let block_offset = entry.offset;
        let block_size = entry.size as u64;
        
        // Use cloud-optimized range request
        let fs = self.filesystem.get_filesystem("file:///")?;
        let block_data = match fs.read_range(&context.file_path, block_offset, block_size).await {
            Ok(data) => {
                debug!("Loading block {} with range request: bytes={}-{}", block_idx, block_offset, block_offset + block_size - 1);
                data
            }
            Err(_) => {
                // Fallback to full file read with seeking
                debug!("Loading block {} with full file read (range request failed)", block_idx);
                let data = fs.read(&context.file_path).await?;
                if data.len() < (block_offset + block_size) as usize {
                    return Err(anyhow::anyhow!("Block extends beyond file size"));
                }
                data[block_offset as usize..(block_offset + block_size) as usize].to_vec()
            }
        };
        
        // Deserialize the block
        let block: DataBlock = bincode::deserialize(&block_data)?;
        Ok(Some(block))
    }

    /// Load index with cloud-optimized metadata reading
    async fn load_index_optimized(&self, file_path: &str) -> Result<SstableIndex> {
        let fs = self.filesystem.get_filesystem("file:///")?;
        
        // Read only the header first to get index offset
        let header_data = match fs.read_range(file_path, 0, 4).await {
            Ok(header_len_data) => {
                let header_len = u32::from_le_bytes([header_len_data[0], header_len_data[1], header_len_data[2], header_len_data[3]]) as u64;
                
                // Now read the actual header
                match fs.read_range(file_path, 4, header_len).await {
                    Ok(data) => data,
                    Err(_) => {
                        // Fallback to full file read
                        let data = fs.read(file_path).await?;
                        let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
                        data[4..4 + header_len].to_vec()
                    }
                }
            }
            Err(_) => {
                // Fallback to full file read
                let data = fs.read(file_path).await?;
                let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
                data[4..4 + header_len].to_vec()
            }
        };
        
        let header: SstableHeader = bincode::deserialize(&header_data)?;
        
        // Calculate index offset (header_len + bloom_filter_len + header + bloom_filter)
        let index_offset = 4 + header_data.len() as u64 + 4 + header.index_size as u64;
        
        // Read index with range request
        let index_data = match fs.read_range(file_path, index_offset, header.index_size as u64).await {
            Ok(data) => {
                debug!("Loading index with range request: bytes={}-{}", index_offset, index_offset + header.index_size as u64 - 1);
                data
            }
            Err(_) => {
                let data = fs.read(file_path).await?;
                data[index_offset as usize..(index_offset + header.index_size as u64) as usize].to_vec()
            }
        };
        
        let entries: Vec<IndexEntry> = bincode::deserialize(&index_data)?;
        
        // Build metadata statistics from index entries
        let mut metadata_stats = HashMap::new();
        for entry in &entries {
            // TODO: Extract metadata statistics from index entries
            // This would require storing metadata min/max values in the index
        }
        
        Ok(SstableIndex {
            entries,
            metadata_stats,
            vector_count: header.entry_count as usize,
            min_key: header.min_key,
            max_key: header.max_key,
        })
    }

    /// Simple get operation for single vector retrieval
    /// This provides a lightweight interface for basic get operations
    pub async fn get_vector(&self, file_path: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        // Get bloom filter for this SSTable
        let bloom_filters = self.index_cache.bloom_filters.read().await;
        if let Some(bloom_filter) = bloom_filters.get(file_path) {
            // Check bloom filter first
            if !bloom_filter.key_filter.might_contain(vector_id) {
                return Ok(None);
            }
        }

        // Create minimal context for the operation
        let context = CollectionContext {
            collection_id: "temp".to_string(),
            file_path: file_path.to_string(),
            sstable_files: vec![file_path.to_string()],
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
        };

        // Use single-key search strategy
        let strategy = SstableReadingStrategy::MetadataFiltered {
            selected_blocks: vec![],
            skip_bloom_check: false,
        };

        // Load blocks and search for the vector
        let blocks = self.apply_strategy(&strategy, &Default::default(), &context).await?;
        
        // Search through blocks for the vector
        for block in blocks {
            for record in block.records {
                if record.id == vector_id {
                    // Convert HashMap metadata to Vec<MetadataItem>
                    let metadata_items = crate::core::proto_metadata_helper::json_metadata_to_proto(&record.metadata);
                    
                    return Ok(Some(VectorRecord {
                        id: Some(record.id),
                        vector: record.vector,
                        metadata: metadata_items,
                        timestamp: record.timestamp,
                        created_at: record.created_at,
                        updated_at: record.updated_at,
                        expires_at: record.expires_at,
                        version: record.version,
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
        let bloom_filters = self.index_cache.bloom_filters.read().await;
        if let Some(bloom_filter) = bloom_filters.get(file_path) {
            bloom_filter.key_filter.might_contain(key)
        } else {
            true // No bloom filter, assume it might contain
        }
    }

    /// Load metadata for an SSTable (header and bloom filter)
    pub async fn load_metadata(&self, file_path: &str) -> Result<()> {
        let fs = self.filesystem.get_filesystem("file:///")?;
        
        // Read file
        let data = fs.read(file_path).await?;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read header
        let mut header_len_bytes = [0u8; 4];
        std::io::Read::read_exact(&mut cursor, &mut header_len_bytes)?;
        let header_len = u32::from_le_bytes(header_len_bytes) as usize;
        
        let mut header_data = vec![0u8; header_len];
        std::io::Read::read_exact(&mut cursor, &mut header_data)?;
        let header: SstableHeader = bincode::deserialize(&header_data)?;
        
        // Read bloom filter if present
        if header.has_bloom_filter {
            let mut bloom_len_bytes = [0u8; 4];
            std::io::Read::read_exact(&mut cursor, &mut bloom_len_bytes)?;
            let bloom_len = u32::from_le_bytes(bloom_len_bytes) as usize;
            
            let mut bloom_data = vec![0u8; bloom_len];
            std::io::Read::read_exact(&mut cursor, &mut bloom_data)?;
            let bloom_filter: SstableBloomFilter = bincode::deserialize(&bloom_data)?;
            
            // Cache the bloom filter
            let mut bloom_filters = self.index_cache.bloom_filters.write().await;
            bloom_filters.insert(file_path.to_string(), Arc::new(bloom_filter));
        }
        
        debug!("Loaded metadata for SSTable: {}", file_path);
        Ok(())
    }

    
    async fn load_specific_blocks(
        &self,
        _context: &CollectionContext,
        _blocks: &[usize],
    ) -> Result<Vec<DataBlock>> {
        Ok(Vec::new())
    }
    
    async fn read_file_with_cache(&self, _path: &str) -> Result<Vec<DataBlock>> {
        Ok(Vec::new())
    }
    
    async fn read_file_direct(&self, _path: &str) -> Result<Vec<DataBlock>> {
        Ok(Vec::new())
    }
    
    fn evaluate_filter(&self, _expr: &FilterExpression, _metadata: &HashMap<String, serde_json::Value>) -> bool {
        true // Placeholder
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
        
        if params.filter_expression.is_some() || params.filters.is_some() {
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
        Self {
            indices: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            bloom_filters: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
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