//! SST Unified Search Engine
//!
//! Implements the UnifiedSearchEngine trait for SST storage with:
//! - SSTable-aware search optimization
//! - Bloom filter integration for metadata columns
//! - Block-level caching and prefetching
//! - MVCC-aware result merging

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use crate::core::search::{
    SearchParams, SearchResultSet, UnifiedSearchEngine, UnifiedSearchContext,
    SearchResult, OptimizationHint,
};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::UnifiedQuantizationEngine;

use super::readers::unified_sstable_reader::UnifiedSstableReader;

/// SST-specific I/O optimization hints
#[derive(Debug, Clone)]
enum SstIoHint {
    /// Use bloom filters to skip blocks
    UseBloomFilter {
        enabled: bool,
        expected_false_positive_rate: f64,
    },
    /// Skip blocks based on index entries
    UseBlockIndex {
        enabled: bool,
        skip_distance: usize,
    },
    /// Cache frequently accessed blocks
    EnableBlockCache {
        cache_size_mb: usize,
        ttl_seconds: u32,
    },
    /// Use seek operations for sparse reads
    UseSeekOptimization {
        enabled: bool,
        min_skip_bytes: usize,
    },
    /// Prefetch adjacent blocks
    EnablePrefetch {
        enabled: bool,
        prefetch_count: usize,
    },
    /// Use compression-aware reads
    CompressionAwareRead {
        decompress_parallel: bool,
        cache_decompressed: bool,
    },
}


/// SST-specific search engine configuration
#[derive(Debug, Clone)]
pub struct SstSearchConfig {
    /// Enable bloom filter optimization
    pub enable_bloom_filters: bool,
    /// Enable block caching
    pub enable_block_cache: bool,
    /// Enable MVCC version resolution
    pub enable_mvcc_resolution: bool,
    /// Maximum number of SSTables to search
    pub max_sstables: usize,
    /// Enable compaction hints based on search patterns
    pub enable_compaction_hints: bool,
}

/// SST Unified Search Engine
#[derive(Debug)]
pub struct SstUnifiedSearchEngine {
    sstable_reader: Arc<UnifiedSstableReader>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    config: SstSearchConfig,
    storage_url: String,
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
}

impl SstUnifiedSearchEngine {
    /// Create new SST search engine
    pub fn new(
        sstable_reader: Arc<UnifiedSstableReader>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        storage_url: String,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Self {
        Self::with_config(
            sstable_reader,
            distance_compute,
            quantization_engine,
            SstSearchConfig::default(),
            storage_url,
            filesystem,
        )
    }
    
    /// Create with custom configuration
    pub fn with_config(
        sstable_reader: Arc<UnifiedSstableReader>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        config: SstSearchConfig,
        storage_url: String,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Self {
        Self {
            sstable_reader,
            distance_compute,
            quantization_engine,
            config,
            storage_url,
            filesystem,
        }
    }
}

#[async_trait::async_trait]
impl UnifiedSearchEngine for SstUnifiedSearchEngine {
    fn engine_id(&self) -> &str {
        "SstUnifiedSearchEngine"
    }
    
    async fn search_unified(
        &self,
        context: &UnifiedSearchContext,
        params: &SearchParams,
        _distance_compute: &UnifiedDistanceCompute,
        _quantization_engine: Option<&UnifiedQuantizationEngine>,
    ) -> Result<SearchResultSet> {
        let start_time = std::time::Instant::now();
        
        debug!("🔍 SST SEARCH: collection={}, k={}", 
              context.collection_id, 
              params.top_k.unwrap_or(10));
        
        // Debug: Print filter expression
        if let Some(filter) = &params.filter_expression {
            debug!("🔎 SST Search Engine: Filter expression = {:?}", filter);
        } else {
            debug!("🔎 SST Search Engine: No filter expression");
        }
        
        // 1. Get SSTable files - use provided paths if available (FAST PATH)
        let sstable_files = if let Some(ref paths) = context.storage_info.file_paths {
            // Files already discovered by engine - use them directly
            debug!("📁 SST: Using {} pre-discovered files from context", paths.len());
            // Using pre-discovered files (FAST PATH)
            paths.clone()
        } else {
            // Fallback: discover files from filesystem (SLOW PATH)
            let collection_storage_url = &self.storage_url;
            debug!("⚠️ SST SEARCH: No files provided, discovering from: {}", collection_storage_url);
            self.discover_sstable_files(context, collection_storage_url).await?
        };
        debug!("📁 SST SEARCH: Processing {} SSTable files", sstable_files.len());
        
        // 2. Generate SST-specific I/O optimization hints
        let io_hints = self.generate_sst_io_hints(&sstable_files, context, params);
        debug!("⚡ SST I/O optimization hints: {:?}", io_hints);
        
        // 3. Apply optimization hints to reduce files to search
        let optimized_files = self.apply_optimization_hints(
            sstable_files,
            context,
            params,
        ).await?;
        
        // 4. Build collection context for reader with I/O hints
        let mut collection_context = super::readers::unified_sstable_reader::CollectionContext {
            file_path: optimized_files.first().cloned().unwrap_or_default(),
            sstable_files: optimized_files,
            total_vectors: context.storage_info.file_count * 1000, // Estimate
            metadata_columns: context.filterable_columns.iter()
                .map(|c| c.name.clone())
                .collect(),
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None, // Will be set by apply_sst_io_hints
        };
        
        // 4.5. Apply I/O hints to collection context
        self.apply_sst_io_hints(&mut collection_context, &io_hints, params);
        
        // 5. Add filterable columns to search params for type-safe filtering
        let mut params_with_columns = params.clone();
        if !context.filterable_columns.is_empty() {
            if params_with_columns.custom_hints.is_none() {
                params_with_columns.custom_hints = Some(HashMap::new());
            }
            
            // Convert filterable columns to JSON array for passing through custom hints
            let columns_json: Vec<serde_json::Value> = context.filterable_columns.iter()
                .map(|col| {
                    serde_json::json!({
                        "name": col.name,
                        "data_type": match col.data_type {
                            crate::core::search::unified_interface::ColumnDataType::String => 
                                crate::proto::proximadb::FilterableDataType::FilterableString as i32,
                            crate::core::search::unified_interface::ColumnDataType::Integer => 
                                crate::proto::proximadb::FilterableDataType::FilterableInteger as i32,
                            crate::core::search::unified_interface::ColumnDataType::Float => 
                                crate::proto::proximadb::FilterableDataType::FilterableFloat as i32,
                            crate::core::search::unified_interface::ColumnDataType::Boolean => 
                                crate::proto::proximadb::FilterableDataType::FilterableBoolean as i32,
                            crate::core::search::unified_interface::ColumnDataType::DateTime => 
                                crate::proto::proximadb::FilterableDataType::FilterableDatetime as i32,
                            crate::core::search::unified_interface::ColumnDataType::Json => 
                                crate::proto::proximadb::FilterableDataType::FilterableString as i32,
                        },
                        "indexed": col.is_indexed,
                        "estimated_cardinality": col.estimated_cardinality,
                    })
                })
                .collect();
            
            params_with_columns.custom_hints.as_mut().unwrap()
                .insert("filterable_columns".to_string(), serde_json::Value::Array(columns_json));
        }
        
        // 4. Use UnifiedSstableReader for optimized search
        debug!("🔍 SST SEARCH: Calling sstable_reader.search_vectors with {} files", 
                collection_context.sstable_files.len());
        let mut search_results = self.sstable_reader.search_vectors(
            &params_with_columns,
            &collection_context,
        ).await?;
        debug!("🔍 SST SEARCH: Got {} results from sstable_reader", search_results.len());
        
        // 5. Apply MVCC resolution if enabled
        if self.config.enable_mvcc_resolution {
            search_results = self.apply_mvcc_resolution(search_results)?;
            debug!("🔍 SST SEARCH: After MVCC resolution: {} results", search_results.len());
        }
        
        let processing_time = start_time.elapsed().as_micros() as u64;
        
        // Log sample results if debug logging enabled
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (i, result) in search_results.iter().take(3).enumerate() {
                debug!("  SST Result {}: id={}, score={}", i, result.id, result.score);
            }
        }
        
        // 6. Build result set with immutable Arc<[SearchResult]>
        let result_count = search_results.len() as u64;
        Ok(SearchResultSet::from_vec(
            search_results,
            result_count,
            None,
            processing_time,
            format!("LSM-{}", 
                if self.config.enable_bloom_filters { "BloomOptimized" } else { "Standard" }),
            HashMap::new(),
        ))
    }
    
    async fn can_handle(&self, _context: &UnifiedSearchContext, _params: &SearchParams) -> bool {
        // LSM can handle all collections in its assigned storage path
        true
    }
    
    async fn optimization_hints(&self, context: &UnifiedSearchContext) -> Vec<OptimizationHint> {
        let mut hints = Vec::new();
        
        if context.storage_info.file_count > 10 {
            hints.push(OptimizationHint::UseMetadataFiltering {
                selectivity_estimate: 0.1,
            });
        }
        
        hints
    }
    
    async fn estimate_cost(
        &self,
        context: &UnifiedSearchContext,
        params: &SearchParams,
    ) -> f64 {
        // Estimate based on:
        // 1. Number of SSTable files
        // 2. Presence of metadata filters (reduces cost)
        // 3. Bloom filter effectiveness
        
        let sstable_count = context.storage_info.file_count;
        let has_filters = params.filter_expression.is_some();
        
        let base_cost = sstable_count as f64;
        let filter_reduction = if has_filters { 0.3 } else { 0.0 };
        let bloom_reduction = if self.config.enable_bloom_filters { 0.2 } else { 0.0 };
        
        base_cost * (1.0 - filter_reduction - bloom_reduction)
    }
}

impl SstUnifiedSearchEngine {
    /// Generate SST-specific I/O optimization hints
    fn generate_sst_io_hints(
        &self,
        file_paths: &[String],
        context: &UnifiedSearchContext,
        params: &SearchParams,
    ) -> Vec<SstIoHint> {
        let mut hints = Vec::new();
        
        let total_files = file_paths.len();
        let is_cloud = context.storage_info.is_cloud_storage;
        let has_filters = params.filter_expression.is_some();
        
        // Bloom filter optimization for metadata filtering
        if self.config.enable_bloom_filters && has_filters {
            hints.push(SstIoHint::UseBloomFilter {
                enabled: true,
                expected_false_positive_rate: 0.01, // 1% FPR
            });
        }
        
        // Block index optimization for SSTable navigation
        hints.push(SstIoHint::UseBlockIndex {
            enabled: true,
            skip_distance: if total_files > 10 { 4 } else { 2 },
        });
        
        // Block caching for frequently accessed data
        if self.config.enable_block_cache {
            hints.push(SstIoHint::EnableBlockCache {
                cache_size_mb: if is_cloud { 256 } else { 128 },
                ttl_seconds: if is_cloud { 600 } else { 300 },
            });
        }
        
        // Seek optimization for sparse reads (especially useful on local SSDs)
        if !is_cloud && total_files < 20 {
            hints.push(SstIoHint::UseSeekOptimization {
                enabled: true,
                min_skip_bytes: 4096, // Skip if gap > 4KB
            });
        }
        
        // Prefetch for sequential access patterns
        if total_files <= 5 && !has_filters {
            hints.push(SstIoHint::EnablePrefetch {
                enabled: true,
                prefetch_count: 2, // Prefetch next 2 blocks
            });
        }
        
        // Compression-aware reading
        hints.push(SstIoHint::CompressionAwareRead {
            decompress_parallel: total_files > 1,
            cache_decompressed: self.config.enable_block_cache,
        });
        
        hints
    }
    
    /// Apply SST I/O hints to collection context
    fn apply_sst_io_hints(
        &self,
        context: &mut super::readers::unified_sstable_reader::CollectionContext,
        hints: &[SstIoHint],
        params: &SearchParams,
    ) {
        let mut custom_hints = params.custom_hints.clone().unwrap_or_default();
        
        for hint in hints {
            match hint {
                SstIoHint::UseBloomFilter { enabled, expected_false_positive_rate } => {
                    custom_hints.insert("use_bloom_filter".to_string(), serde_json::json!(*enabled));
                    custom_hints.insert("bloom_fpr".to_string(), serde_json::json!(*expected_false_positive_rate));
                }
                SstIoHint::UseBlockIndex { enabled, skip_distance } => {
                    custom_hints.insert("use_block_index".to_string(), serde_json::json!(*enabled));
                    custom_hints.insert("block_skip_distance".to_string(), serde_json::json!(*skip_distance));
                }
                SstIoHint::EnableBlockCache { cache_size_mb, ttl_seconds } => {
                    custom_hints.insert("block_cache_size_mb".to_string(), serde_json::json!(*cache_size_mb));
                    custom_hints.insert("block_cache_ttl".to_string(), serde_json::json!(*ttl_seconds));
                }
                SstIoHint::UseSeekOptimization { enabled, min_skip_bytes } => {
                    custom_hints.insert("use_seek_optimization".to_string(), serde_json::json!(*enabled));
                    custom_hints.insert("min_seek_skip_bytes".to_string(), serde_json::json!(*min_skip_bytes));
                }
                SstIoHint::EnablePrefetch { enabled, prefetch_count } => {
                    custom_hints.insert("enable_prefetch".to_string(), serde_json::json!(*enabled));
                    custom_hints.insert("prefetch_blocks".to_string(), serde_json::json!(*prefetch_count));
                }
                SstIoHint::CompressionAwareRead { decompress_parallel, cache_decompressed } => {
                    custom_hints.insert("parallel_decompress".to_string(), serde_json::json!(*decompress_parallel));
                    custom_hints.insert("cache_decompressed_blocks".to_string(), serde_json::json!(*cache_decompressed));
                }
            }
        }
        
        // Store hints in context (need to add this field to CollectionContext)
        context.io_optimization_hints = Some(custom_hints);
    }
    
    /// Discover SSTable files for collection by scanning directory recursively
    async fn discover_sstable_files(
        &self,
        context: &UnifiedSearchContext,
        collection_storage_url: &str,
    ) -> Result<Vec<String>> {
        debug!("🔍 SST: Discovering SSTable files for collection {} by recursive directory scan", context.collection_id);
        
        let mut sstable_files = Vec::new();
        
        debug!("🔍 SST Search Engine: Using collection storage URL: {}", collection_storage_url);
        
        // Use the search engine's filesystem instance with the provided URL
        let fs = self.filesystem.get_filesystem(collection_storage_url)?;
        
        // Use iterative approach to avoid async recursion issues
        let mut directories_to_scan = vec![(collection_storage_url.to_string(), 0u32)];
        
        while let Some((directory_url, depth)) = directories_to_scan.pop() {
            // Prevent infinite recursion
            if depth > 5 {
                debug!("⚠️ Max recursion depth reached, stopping scan at {}", directory_url);
                continue;
            }

            // List all files in the current directory
            let files = fs.list(&directory_url).await?;
            debug!("📁 Scanning directory {} (depth={}) - found {} items", directory_url, depth, files.len());
            debug!("🔍 SST SCAN: Directory {} (depth={}) - found {} items", directory_url, depth, files.len());
            
            for file_info in files {
                let full_path = &file_info.metadata.path;
                let filename = full_path.split('/').last().unwrap_or("");
                
                debug!("🔍 SST SCAN: Item: path={}, name={}, is_dir={}", 
                         full_path, filename, file_info.metadata.is_directory);
                
                if file_info.metadata.is_directory {
                    // Skip temp directories but add other directories to scan queue
                    if !filename.starts_with("___") {
                        debug!("🔍 SST SCAN: Adding directory to scan queue: {}", full_path);
                        directories_to_scan.push((full_path.clone(), depth + 1));
                    } else {
                        debug!("🔍 SST SCAN: Skipping temp directory: {}", filename);
                    }
                } else {
                    // Check if it's an SSTable file for this collection
                    debug!("🔍 SST SCAN: Checking file: '{}'", filename);
                    // 🔴 UNUSED - belongs_to_collection method doesn't exist
                    // if super::SstFilenameGenerator::belongs_to_collection(filename, &context.collection_id) {
                    if filename.contains(&context.collection_id) {  // Simple check instead
                        debug!("🔍 SST SCAN: ✅ MATCH - filename '{}' matches pattern", filename);
                        
                        // Extract level from filename using centralized utility
                        let level = super::SstFilenameGenerator::parse_level_from_filename(filename).unwrap_or(0);
                        
                        debug!("  ✅ SSTable: {} (level={}, size={} bytes)", filename, level, file_info.metadata.size);
                        sstable_files.push(full_path.clone());
                    } else {
                        debug!("🔍 SST SCAN: ❌ NO MATCH - filename '{}' doesn't match pattern", filename);
                    }
                }
            }
        }
        
        // Sort by filename to ensure consistent ordering (older files first)
        sstable_files.sort();
        
        debug!("📊 SST: Found {} SSTable files for collection {}", sstable_files.len(), context.collection_id);
        
        // Debug: print all discovered files
        for (i, file) in sstable_files.iter().enumerate() {
            debug!("🔍 DEBUG SST File {}: {}", i, file);
        }
        
        Ok(sstable_files)
    }
    
    /// Apply optimization hints to reduce files to search
    async fn apply_optimization_hints(
        &self,
        files: Vec<String>,
        _context: &UnifiedSearchContext,
        _params: &SearchParams,
    ) -> Result<Vec<String>> {
        // Optimization strategies:
        // 1. Skip older levels if we have enough recent data
        // 2. Use metadata bloom filters to skip files
        // 3. Limit to max_sstables if configured
        
        let mut optimized = files;
        
        // Limit files if configured
        if optimized.len() > self.config.max_sstables {
            optimized.truncate(self.config.max_sstables);
            debug!("📊 Limited to {} SSTables", self.config.max_sstables);
        }
        
        Ok(optimized)
    }
    
    /// Apply MVCC resolution to handle multiple versions
    /// This implements the same version continuity rules as VIPER engine
    fn apply_mvcc_resolution(
        &self,
        results: Vec<SearchResult>,
    ) -> Result<Vec<SearchResult>> {
        use std::collections::BTreeMap;
use tracing::debug;
        
        // Log MVCC processing if debug enabled
        debug!("🔍 MVCC: Processing {} results", results.len());
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (i, result) in results.iter().take(3).enumerate() {
                debug!("  MVCC Input {}: id={}, version={:?}, timestamp={:?}", 
                         i, result.id, result.version, result.timestamp);
            }
        }
        
        // Pre-allocate capacity for efficiency
        let estimated_unique_ids = results.len() / 2; // Conservative estimate
        
        // Group results by ID - using BTreeMap for better cache locality
        let mut id_groups: BTreeMap<String, Vec<SearchResult>> = BTreeMap::new();
        let mut results_without_id = Vec::with_capacity(results.len() / 4);
        
        for result in results {
            if result.id.is_empty() {
                // Vectors without IDs are append-only, no deduplication
                results_without_id.push(result);
            } else {
                id_groups.entry(result.id.clone()).or_insert_with(Vec::new).push(result);
            }
        }
        
        // Process each ID group - pre-allocate with estimated capacity
        let mut deduplicated = Vec::with_capacity(estimated_unique_ids + results_without_id.len());
        
        for (id, mut versions) in id_groups {
            // Sort by version (treating None as 1), then timestamp (earliest first for same version)
            // Use unstable sort for better performance
            versions.sort_unstable_by(|a, b| {
                // Treat None/null as version 1
                let version_a = a.version.unwrap_or(1);
                let version_b = b.version.unwrap_or(1);
                
                version_a.cmp(&version_b)
                    .then_with(|| {
                        // For same version, earliest timestamp wins
                        let ts_a = a.timestamp.unwrap_or(u32::MAX);
                        let ts_b = b.timestamp.unwrap_or(u32::MAX);
                        ts_a.cmp(&ts_b)
                    })
            });
            
            // Validate version continuity and find the latest valid version
            let mut expected_version = 1;
            let mut last_valid: Option<SearchResult> = None;
            
            for result in versions {
                // Treat None/null as version 1
                let version = result.version.unwrap_or(1);
                
                if version == expected_version {
                    // Check for duplicate version - keep earliest timestamp
                    if let Some(ref existing) = last_valid {
                        let existing_version = existing.version.unwrap_or(1);
                        if existing_version == version {
                            let existing_ts = existing.timestamp.unwrap_or(u32::MAX);
                            let current_ts = result.timestamp.unwrap_or(u32::MAX);
                            if current_ts < existing_ts {
                                last_valid = Some(result);
                            }
                            continue;
                        }
                    }
                    last_valid = Some(result);
                    expected_version += 1;
                } else if version > expected_version {
                    // Version gap detected - stop processing this ID
                    debug!("Version gap detected for {}: expected {}, found {}", id, expected_version, version);
                    break;
                }
                // Skip older versions
            }
            
            if let Some(result) = last_valid {
                deduplicated.push(result);
            }
        }
        
        // Add back results without IDs (no deduplication for append-only vectors)
        deduplicated.extend(results_without_id);
        
        // Sort by score - use unstable sort for better performance
        deduplicated.sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
        debug!("🔄 MVCC resolution: {} unique results after deduplication", deduplicated.len());
        
        Ok(deduplicated)
    }
}

impl Default for SstSearchConfig {
    fn default() -> Self {
        Self {
            enable_bloom_filters: true,
            enable_block_cache: true,
            enable_mvcc_resolution: true,
            max_sstables: 100,
            enable_compaction_hints: true,
        }
    }
}

// #[cfg(test)]
// mod tests;