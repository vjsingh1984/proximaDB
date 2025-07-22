//! LSM Unified Search Engine
//!
//! Implements the UnifiedSearchEngine trait for LSM storage with:
//! - SSTable-aware search optimization
//! - Bloom filter integration for metadata columns
//! - Block-level caching and prefetching
//! - MVCC-aware result merging

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::search::{
    SearchParams, SearchResultSet, UnifiedSearchEngine, UnifiedSearchContext,
    SearchResult, OptimizationHint,
};
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::compute::unified_quantization::UnifiedQuantizationEngine;

use super::readers::unified_sstable_reader::UnifiedSstableReader;


/// LSM-specific search engine configuration
#[derive(Debug, Clone)]
pub struct LsmSearchConfig {
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

/// LSM Unified Search Engine
pub struct LsmUnifiedSearchEngine {
    sstable_reader: Arc<UnifiedSstableReader>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    config: LsmSearchConfig,
}

impl LsmUnifiedSearchEngine {
    /// Create new LSM search engine
    pub fn new(
        sstable_reader: Arc<UnifiedSstableReader>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        Self::with_config(
            sstable_reader,
            distance_compute,
            quantization_engine,
            LsmSearchConfig::default(),
        )
    }
    
    /// Create with custom configuration
    pub fn with_config(
        sstable_reader: Arc<UnifiedSstableReader>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        config: LsmSearchConfig,
    ) -> Self {
        Self {
            sstable_reader,
            distance_compute,
            quantization_engine,
            config,
        }
    }
}

#[async_trait::async_trait]
impl UnifiedSearchEngine for LsmUnifiedSearchEngine {
    fn engine_id(&self) -> &str {
        "LsmUnifiedSearchEngine"
    }
    
    async fn search_unified(
        &self,
        context: &UnifiedSearchContext,
        params: &SearchParams,
        distance_compute: &UnifiedDistanceCompute,
        quantization_engine: Option<&UnifiedQuantizationEngine>,
    ) -> Result<SearchResultSet> {
        let start_time = std::time::Instant::now();
        
        info!("🔍 LSM Search: collection={}, k={}", 
              context.collection_id, 
              params.top_k.unwrap_or(10));
        
        // 1. Build SSTable file paths
        let sstable_files = self.discover_sstable_files(context).await?;
        debug!("📁 Found {} SSTable files", sstable_files.len());
        
        // 2. Apply optimization hints
        let optimized_files = self.apply_optimization_hints(
            sstable_files,
            context,
            params,
        ).await?;
        
        // 3. Build collection context for reader
        let collection_context = super::readers::unified_sstable_reader::CollectionContext {
            collection_id: context.collection_id.clone(),
            file_path: optimized_files.first().cloned().unwrap_or_default(),
            sstable_files: optimized_files,
            total_vectors: context.storage_info.file_count * 1000, // Estimate
            metadata_columns: context.filterable_columns.iter()
                .map(|c| c.name.clone())
                .collect(),
            level: 0,
            creation_time: chrono::Utc::now(),
        };
        
        // 4. Use UnifiedSstableReader for optimized search
        let mut search_results = self.sstable_reader.search_vectors(
            params,
            &collection_context,
        ).await?;
        
        // 5. Apply MVCC resolution if enabled
        if self.config.enable_mvcc_resolution {
            search_results = self.apply_mvcc_resolution(search_results)?;
        }
        
        let processing_time = start_time.elapsed().as_micros() as u64;
        
        // 6. Build result set
        Ok(SearchResultSet {
            results: search_results.clone(),
            total_count: search_results.len() as u64,
            query_id: None,
            processing_time_us: processing_time,
            algorithm: format!("LSM-{}", 
                if self.config.enable_bloom_filters { "BloomOptimized" } else { "Standard" }),
            metadata: HashMap::new(),
        })
    }
    
    async fn can_handle(&self, context: &UnifiedSearchContext, params: &SearchParams) -> bool {
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
        let has_filters = params.filter_expression.is_some() || params.filters.is_some();
        
        let base_cost = sstable_count as f64;
        let filter_reduction = if has_filters { 0.3 } else { 0.0 };
        let bloom_reduction = if self.config.enable_bloom_filters { 0.2 } else { 0.0 };
        
        base_cost * (1.0 - filter_reduction - bloom_reduction)
    }
}

impl LsmUnifiedSearchEngine {
    /// Discover SSTable files for collection
    async fn discover_sstable_files(
        &self,
        context: &UnifiedSearchContext,
    ) -> Result<Vec<String>> {
        // In production, this would list actual SSTable files
        // For now, return placeholder paths
        Ok(vec![
            format!("{}/level0_001.sst", context.collection_id),
            format!("{}/level0_002.sst", context.collection_id),
            format!("{}/level1_001.sst", context.collection_id),
        ])
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
    fn apply_mvcc_resolution(
        &self,
        results: Vec<SearchResult>,
    ) -> Result<Vec<SearchResult>> {
        // Group by ID and keep only latest version (based on metadata version field)
        let mut latest_versions: std::collections::HashMap<String, SearchResult> = 
            std::collections::HashMap::new();
        
        let initial_count = results.len();
        
        for result in results {
            // Extract version from metadata if available
            let version = result.metadata.get("_version")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            
            match latest_versions.get(&result.id) {
                Some(existing) => {
                    let existing_version = existing.metadata.get("_version")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if existing_version >= version {
                        // Keep existing (newer or same version)
                        continue;
                    }
                }
                None => {}
            }
            
            // Insert new or replace with newer version
            latest_versions.insert(result.id.clone(), result);
        }
        
        // Convert back to vector and sort by score
        let mut resolved: Vec<SearchResult> = latest_versions.into_values().collect();
        resolved.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        
        debug!("🔄 MVCC resolution: {} results -> {} unique", initial_count, resolved.len());
        
        Ok(resolved)
    }
}

impl Default for LsmSearchConfig {
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