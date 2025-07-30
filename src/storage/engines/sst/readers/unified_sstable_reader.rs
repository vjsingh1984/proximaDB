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
use tracing::{debug, warn};

use crate::core::VectorRecord;
use crate::core::search::{SearchParams, SearchResult, FilterExpression};
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::engines::sst::bloom_filter::SstableBloomFilter;
use crate::storage::engines::sst::{SstableHeader, DataBlock, IndexEntry};
use crate::core::bloom::{BloomFilterConfig, BloomStrategy};

/// Unified SSTable Reader with automatic optimization selection
#[derive(Debug)]
pub struct UnifiedSstableReader {
    filesystem: Arc<FilesystemFactory>,
    block_cache: Arc<BlockCache>,
    index_cache: Arc<IndexCache>,
    strategy_selector: Arc<ReadingStrategySelector>,
    distance_compute: Arc<UnifiedDistanceCompute>,
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
        println!("🔍 LSM Unified Search: {} files, k={}", 
              collection_context.sstable_files.len(),
              params.top_k.unwrap_or(10));
        
        // Debug: print file paths
        for (i, file_path) in collection_context.sstable_files.iter().enumerate() {
            println!("📁 SSTable file {}: {}", i, file_path);
        }
        
        // Debug: print filter expression
        if let Some(filter) = &params.filter_expression {
            println!("🔎 Filter expression: {:?}", filter);
        }
        
        // 1. Select optimal reading strategy
        let strategy = self.strategy_selector.select_strategy(params, collection_context)?;
        println!("📊 Selected strategy: {:?}", strategy);
        
        // 2. Apply strategy to read relevant blocks
        let relevant_blocks = self.apply_strategy(&strategy, params, collection_context).await?;
        println!("📦 Loaded {} data blocks total from all files", relevant_blocks.len());
        
        // Debug: print some sample records from blocks
        for (i, block) in relevant_blocks.iter().take(2).enumerate() {
            println!("  Block {}: {} records", i, block.records.len());
            for (j, record) in block.records.iter().take(3).enumerate() {
                println!("    Record {}: id={}, metadata={:?}", j, record.id, record.metadata);
            }
        }
        
        // 3. Perform vector search on loaded data
        let results = self.search_in_blocks(params, &relevant_blocks, &collection_context.collection_id).await?;
        println!("🎯 Found {} search results after filtering and scoring", results.len());
        
        // Debug: print sample results
        for (i, result) in results.iter().take(3).enumerate() {
            println!("  Result {}: id={}, score={}, metadata={:?}", 
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
        
        println!("🔍 Searching in {} blocks for top {} results", blocks.len(), k);
        
        // Compute distances for all vectors
        let mut scored_results = Vec::new();
        let mut total_records = 0;
        let mut filtered_out = 0;
        let mut tombstones = 0;
        
        for (block_idx, block) in blocks.iter().enumerate() {
            let block_records = block.records.len();
            total_records += block_records;
            println!("📊 Processing block {} with {} records", block_idx, block_records);
            
            for (record_idx, record) in block.records.iter().enumerate() {
                // Debug first few records from each block
                if record_idx < 3 {
                    println!("  🔍 Block {} Record {}: id={}, metadata={:?}", block_idx, record_idx, record.id, record.metadata);
                }
                
                if record.is_tombstone {
                    tombstones += 1;
                    continue;
                }
                
                // Apply metadata filters
                if let Some(filter_expr) = &params.filter_expression {
                    println!("  🔍 Checking filter for record: id={}, metadata={:?}", record.id, record.metadata);
                    if !self.evaluate_filter(filter_expr, &record.metadata) {
                        filtered_out += 1;
                        // Debug first few filtered records
                        if filtered_out <= 5 {
                            println!("  ❌ Filtered out record: id={}, metadata={:?}", record.id, record.metadata);
                            // Debug the filter evaluation
                            if let FilterExpression::Comparison { field, operator: _, value } = filter_expr {
                                if let Some(field_val) = record.metadata.get(field) {
                                    println!("    📊 Filter comparison: field_val={:?} vs filter_value={:?} (equal={})", 
                                             field_val, value, field_val == value);
                                    // Try numeric comparison
                                    if let (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) = (field_val, value) {
                                        println!("    📊 Numeric values: {} vs {} (f64: {} vs {})",
                                                 n1, n2, n1.as_f64().unwrap_or(0.0), n2.as_f64().unwrap_or(0.0));
                                    }
                                }
                            }
                        }
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
            
            if block_idx == 0 || block_idx == blocks.len() - 1 {
                println!("  Block {}: {} records processed", block_idx, block_records);
            }
        }
        
        println!("📊 Search stats: {} total records, {} tombstones, {} filtered out, {} candidates", 
              total_records, tombstones, filtered_out, scored_results.len());
        
        // Apply diversity-aware selection: group by ID, take best from each group, then sort
        let mut id_groups: std::collections::HashMap<String, Vec<SearchResult>> = 
            std::collections::HashMap::new();
        
        // Group results by ID
        for result in scored_results {
            id_groups.entry(result.id.clone()).or_default().push(result);
        }
        
        // Select best representative from each ID group
        let mut diverse_results = Vec::new();
        for (_id, mut group) in id_groups {
            // Sort group by score and take the best one
            group.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            if let Some(best) = group.into_iter().next() {
                diverse_results.push(best);
            }
        }
        
        // Sort diverse results by score and take top k
        diverse_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        diverse_results.truncate(k);
        
        debug!("🔍 Diversity-aware selection: {} unique IDs from {} candidates", 
               diverse_results.len(), total_records - tombstones - filtered_out);
        
        Ok(diverse_results)
    }
    
    /// Full scan strategy implementation
    async fn full_scan_strategy(
        &self,
        context: &CollectionContext,
        use_block_cache: bool,
    ) -> Result<Vec<DataBlock>> {
        println!("🔍 Full scan strategy for {} files (cache={})", context.sstable_files.len(), use_block_cache);
        let mut all_blocks = Vec::new();
        
        for (idx, file_path) in context.sstable_files.iter().enumerate() {
            println!("📂 Reading file {} of {}: {}", idx + 1, context.sstable_files.len(), file_path);
            let blocks = if use_block_cache {
                self.read_file_with_cache(file_path).await?
            } else {
                self.read_file_direct(file_path).await?
            };
            println!("  📦 Loaded {} blocks from this file", blocks.len());
            
            // Debug: print sample records from first block
            if let Some(first_block) = blocks.first() {
                println!("  🔎 First block has {} records", first_block.records.len());
                for (i, record) in first_block.records.iter().take(3).enumerate() {
                    println!("    Record {}: id={}, metadata={:?}", i, record.id, record.metadata);
                }
            }
            
            all_blocks.extend(blocks);
        }
        
        println!("✅ Full scan loaded {} total blocks from all files", all_blocks.len());
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
        println!("🔍 Using metadata filtered strategy for {} files", context.sstable_files.len());
        
        let mut all_blocks = Vec::new();
        let metadata_conditions = self.extract_metadata_conditions(params);
        println!("📋 Extracted metadata conditions: {:?}", metadata_conditions);
        
        // Process each SSTable file
        for (file_idx, file_path) in context.sstable_files.iter().enumerate() {
            println!("📂 Processing SSTable file {} of {}: {}", file_idx + 1, context.sstable_files.len(), file_path);
            
            // Get bloom filter from cache
            let bloom_filter = if !skip_bloom {
                self.index_cache.bloom_filters.get(file_path).await
            } else {
                None
            };

            // Get the index using the new cache API
            let index = self.index_cache.get_or_load_index(file_path, || async {
                self.load_index_optimized(file_path).await
            }).await?;
            println!("  📊 Loaded index with {} entries", index.entries.len());

            // First check bloom filter for quick rejection
            if let Some(bloom_filter) = bloom_filter {
                let mut any_match = false;
                for (column, value) in &metadata_conditions {
                    // Convert JSON value to MetadataItem for type-safe bloom filter check
                    let metadata_item = crate::core::bloom::json_to_metadata_item(column, value);
                    if bloom_filter.might_match_metadata(column, &metadata_item).unwrap_or(true) {
                        any_match = true;
                        break;
                    }
                }
                
                if !any_match {
                    println!("  ❌ Bloom filter rejected file {} (no metadata matches)", file_path);
                    continue; // Skip this file entirely
                }
                println!("  ✅ Bloom filter indicates potential matches");
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

            println!("  📦 Selected {} blocks out of {} after metadata filtering for file {}", 
                   selected_blocks.len(), total_blocks, file_path);

            // Load the selected blocks from this file
            for block_idx in selected_blocks {
                println!("    📄 Loading block {} from file {}", block_idx, file_path);
                // Create a temporary context for this specific file
                let file_context = CollectionContext {
                    collection_id: context.collection_id.clone(),
                    file_path: file_path.clone(),
                    sstable_files: vec![file_path.clone()],
                    total_vectors: context.total_vectors,
                    metadata_columns: context.metadata_columns.clone(),
                    level: context.level,
                    creation_time: context.creation_time,
                };
                
                if let Some(block) = self.load_block_with_cache(&file_context, block_idx).await? {
                    // Debug: print first few records from loaded block
                    println!("    📦 Loaded block {} with {} records from {}", 
                          block_idx, block.records.len(), file_path);
                    for (i, record) in block.records.iter().take(3).enumerate() {
                        println!("      Record {}: id={}, metadata={:?}", i, record.id, record.metadata);
                    }
                    all_blocks.push(block);
                }
            }
        }

        println!("Loaded {} blocks total after metadata filtering from {} files", 
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
        let cache_key = BlockCacheKey {
            file_path: context.file_path.clone(),
            block_id: block_idx as u32,
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
        // Extract scheme from file path for proper filesystem selection
        let scheme = if context.file_path.contains("://") {
            context.file_path.split("://").next().unwrap_or("file")
        } else {
            "file"
        };
        let fs = self.filesystem.get_filesystem(&format!("{}:///", scheme))?;
        
        // Use optimized cache with proper async-safe loading
        let index = self.index_cache.get_or_load_index(&context.file_path, || async {
            self.load_index_optimized(&context.file_path).await
        }).await?;
        
        // Check if block exists
        if block_idx >= index.entries.len() {
            return Ok(None);
        }
        
        // To find the block offset, we need to calculate the data section offset
        // Read header length to calculate offsets
        let header_len_data = fs.read_range(&context.file_path, 0, 4).await?;
        let header_len = u32::from_le_bytes([
            header_len_data[0], header_len_data[1], header_len_data[2], header_len_data[3]
        ]) as u64;
        
        // Read bloom filter length to skip it
        let bloom_offset = 4 + header_len;
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
        
        // Read header length
        let header_len_data = fs.read_range(file_path, 0, 4).await?;
        if header_len_data.len() < 4 {
            return Err(anyhow::anyhow!(
                "SSTable file too small: expected at least 4 bytes for header length, got {}",
                header_len_data.len()
            ));
        }
        let header_len = u32::from_le_bytes([
            header_len_data[0], header_len_data[1], header_len_data[2], header_len_data[3]
        ]) as u64;
        
        // Read header
        let header_data = fs.read_range(file_path, 4, header_len).await?;
        if header_data.len() < header_len as usize {
            return Err(anyhow::anyhow!(
                "Failed to read complete header: expected {} bytes, got {}",
                header_len, header_data.len()
            ));
        }
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        
        // Calculate bloom filter offset and read its length
        let bloom_offset = 4 + header_len;
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
        // Check bloom filter first using new cache API
        if let Some(bloom_filter) = self.index_cache.bloom_filters.get(file_path).await {
            if !bloom_filter.might_contain_key(vector_id).unwrap_or(true) {
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

        // Use full scan strategy for single key lookup
        // TODO: Optimize with index-based lookup
        let strategy = SstableReadingStrategy::FullScan {
            use_block_cache: true,
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
        if let Some(bloom_filter) = self.index_cache.bloom_filters.get(file_path).await {
            bloom_filter.might_contain_key(key).unwrap_or(true)
        } else {
            true // No bloom filter, assume it might contain
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
        
        // First read just the header length (4 bytes)
        let header_len_data = fs.read_range(file_path, 0, 4).await?;
        if header_len_data.len() < 4 {
            return Err(anyhow::anyhow!("SSTable file too small: {} bytes", header_len_data.len()));
        }
        let header_len = u32::from_le_bytes([
            header_len_data[0], header_len_data[1], header_len_data[2], header_len_data[3]
        ]) as u64;
        
        debug!("Header length: {} bytes", header_len);
        
        // Read the header data
        let header_data = fs.read_range(file_path, 4, header_len).await?;
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize header: {}", e))?;
        
        debug!("Header info: version={}, has_bloom={}, entry_count={}", 
               header.version, header.has_bloom_filter, header.entry_count);
        
        // Read bloom filter if present
        if header.has_bloom_filter {
            // Calculate bloom filter offset (after header_len + header)
            let bloom_offset = 4 + header_len;
            
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
            
            let bloom_filter: SstableBloomFilter = match SstableBloomFilter::deserialize(&bloom_data) {
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
            
            // Cache the bloom filter using new API
            self.index_cache.bloom_filters.insert(file_path.to_string(), Arc::new(bloom_filter)).await;
            
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
        let index = self.index_cache.get_or_load_index(path, || async {
            self.load_index_optimized(path).await
        }).await?;
        
        let num_blocks = index.entries.len();
        
        // Load all blocks using cache
        let mut blocks = Vec::new();
        let context = CollectionContext {
            collection_id: "temp".to_string(),
            file_path: path.to_string(),
            sstable_files: vec![path.to_string()],
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: chrono::Utc::now(),
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
        
        // Skip header
        if data.len() < 4 {
            return Ok(vec![]);
        }
        let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        offset += 4 + header_len;
        
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
        debug!("Index said {} entries, header said {} blocks", index.entries.len(), 
            if let Ok(header) = bincode::deserialize::<SstableHeader>(&data[4..4+header_len]) {
                header.block_count
            } else { 0 });
        
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
            match DataBlock::deserialize(block_data) {
                Ok(block) => {
                    debug!("Successfully deserialized block with {} records", block.records.len());
                    blocks.push(block);
                }
                Err(e) => {
                    warn!("Failed to deserialize block: {:?}", e);
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