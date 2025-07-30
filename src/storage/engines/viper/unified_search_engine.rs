//! VIPER Unified Search Engine
//!
//! This is the SEARCH ENGINE that implements search logic and uses UnifiedParquetReader
//! as a data access layer. This provides proper separation of concerns:
//!
//! - UnifiedParquetReader = Pure data access with strategy optimization
//! - ViperUnifiedSearchEngine = Search logic, ranking, filtering
//! - DirectVectorService = Search orchestration across engines

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::search::{
    SearchParams, SearchResultSet, UnifiedSearchEngine, UnifiedSearchContext,
    OptimizationHint
};
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::compute::unified_quantization::{UnifiedQuantizationEngine, UnifiedQuantizationLevel};
use super::readers::unified_parquet_reader::{UnifiedParquetReader, CollectionContext, FilterableColumnSpec};


/// VIPER Unified Search Engine - implements search logic using UnifiedParquetReader for data access
#[derive(Debug)]
pub struct ViperUnifiedSearchEngine {
    /// Data access layer (pure reader)
    parquet_reader: Arc<UnifiedParquetReader>,
    /// Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Quantization engine for optimization
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    /// Engine configuration
    config: ViperSearchConfig,
}

#[derive(Debug, Clone)]
pub struct ViperSearchConfig {
    /// Enable quantized search optimization
    pub enable_quantization: bool,
    /// Enable metadata filtering optimization  
    pub enable_metadata_filtering: bool,
    /// Enable ML clustering file selection
    pub enable_clustering: bool,
    /// Maximum files to process in parallel
    pub max_parallel_files: usize,
    /// Cache size for frequent queries
    pub cache_size: usize,
}

impl Default for ViperSearchConfig {
    fn default() -> Self {
        Self {
            enable_quantization: true,
            enable_metadata_filtering: true,
            enable_clustering: true,
            max_parallel_files: 8,
            cache_size: 1000,
        }
    }
}

impl ViperUnifiedSearchEngine {
    /// Create new VIPER search engine with data access layer
    pub fn new(
        parquet_reader: Arc<UnifiedParquetReader>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        Self::with_config(
            parquet_reader,
            distance_compute,
            quantization_engine,
            ViperSearchConfig::default(),
        )
    }
    
    /// Create with custom configuration
    pub fn with_config(
        parquet_reader: Arc<UnifiedParquetReader>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        config: ViperSearchConfig,
    ) -> Self {
        Self {
            parquet_reader,
            distance_compute,
            quantization_engine,
            config,
        }
    }
}

#[async_trait]
impl UnifiedSearchEngine for ViperUnifiedSearchEngine {
    fn engine_id(&self) -> &str {
        "ViperUnifiedSearchEngine"
    }
    
    async fn search_unified(
        &self,
        context: &UnifiedSearchContext,
        params: &SearchParams,
        distance_compute: &UnifiedDistanceCompute,
        quantization_engine: Option<&UnifiedQuantizationEngine>,
    ) -> Result<SearchResultSet> {
        let start_time = std::time::Instant::now();
        
        info!("🔍 VIPER Search: collection={}, k={}", context.collection_id, params.top_k.unwrap_or(10));
        if let Some(filter_expr) = &params.filter_expression {
            info!("🔍 VIPER Search has filter expression");
        }
        
        // 1. Build file paths using collection context and ML clustering
        let file_paths = self.build_file_paths(context, params).await?;
        debug!("📁 Selected {} files for search", file_paths.len());
        for (i, path) in file_paths.iter().enumerate() {
            debug!("📁   File {}: {}", i, path);
        }
        
        // 2. Build collection context for reader
        let collection_context = self.build_collection_context(context, &file_paths);
        debug!("📁 Collection context: {:?}", collection_context);
        debug!("📁 Filterable columns: {:?}", collection_context.filterable_columns);
        
        // 3. Use UnifiedParquetReader direct search - NO ADAPTERS!
        let search_results = self.parquet_reader.search_vectors(
            params,
            &collection_context,
        ).await?;
        
        let processing_time = start_time.elapsed().as_micros() as u64;
        
        info!("✅ VIPER Search completed: {} results in {}μs", search_results.len(), processing_time);
        
        Ok(SearchResultSet {
            results: search_results.clone(),
            total_count: search_results.len() as u64,
            query_id: params.custom_hints.as_ref()
                .and_then(|h| h.get("query_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            processing_time_us: processing_time,
            algorithm: "VIPER-Direct".to_string(),
            metadata: HashMap::new(),
        })
    }
    
    async fn can_handle(&self, context: &UnifiedSearchContext, _params: &SearchParams) -> bool {
        // VIPER handles Parquet-based collections
        context.storage_info.storage_type.contains("VIPER") ||
        context.storage_info.storage_type.contains("Parquet") ||
        context.storage_info.file_count > 0
    }
    
    async fn optimization_hints(&self, context: &UnifiedSearchContext) -> Vec<OptimizationHint> {
        let mut hints = Vec::new();
        
        // Quantization optimization for large collections
        if self.config.enable_quantization && 
           context.collection_config.as_ref().map(|c| c.estimated_document_count).unwrap_or(0) > 50000 {
            hints.push(OptimizationHint::UseQuantization {
                method: UnifiedQuantizationLevel::pq8(8),
                expected_speedup: 4.0,
            });
        }
        
        // Metadata filtering for collections with filterable columns
        if self.config.enable_metadata_filtering && !context.filterable_columns.is_empty() {
            hints.push(OptimizationHint::UseMetadataFiltering {
                selectivity_estimate: 0.2, // Conservative estimate
            });
        }
        
        // Cloud optimization for cloud storage
        if context.storage_info.is_cloud_storage {
            hints.push(OptimizationHint::UseRangeRequests {
                chunk_size_mb: 2.0, // Optimized for VIPER
            });
            
            hints.push(OptimizationHint::UseCaching {
                cache_key: format!("viper_{}", context.collection_id),
            });
        }
        
        // Column projection for high-dimensional vectors
        let vector_dim = context.collection_config.as_ref().map(|c| c.vector_dimension).unwrap_or(128);
        if vector_dim > 768 {
            hints.push(OptimizationHint::UseColumnProjection {
                columns: vec!["vector".to_string(), "id".to_string(), "metadata".to_string()],
            });
        }
        
        hints
    }
    
    async fn estimate_cost(&self, context: &UnifiedSearchContext, params: &SearchParams) -> f64 {
        let base_cost = 10.0; // VIPER has higher base cost due to Parquet overhead
        
        // File count impact
        let file_count_factor = (context.storage_info.file_count as f64).log2().max(1.0);
        
        // Size impact  
        let size_factor = (context.storage_info.estimated_size_mb / 1000.0).sqrt();
        
        // Query complexity
        let k_factor = (params.top_k.unwrap_or(10) as f64).sqrt();
        let filter_factor = if params.filter_expression.is_some() { 0.5 } else { 1.0 }; // Filters reduce cost
        let quantization_factor = if params.quantization_hint.is_some() { 0.3 } else { 1.0 }; // Quantization reduces cost
        
        // Storage type impact
        let storage_factor = if context.storage_info.is_cloud_storage { 2.0 } else { 1.0 };
        
        base_cost * file_count_factor * size_factor * k_factor * filter_factor * quantization_factor * storage_factor
    }
}

impl ViperUnifiedSearchEngine {
    /// Build file paths using collection context and ML clustering
    async fn build_file_paths(
        &self,
        context: &UnifiedSearchContext,
        params: &SearchParams,
    ) -> Result<Vec<String>> {
        if self.config.enable_clustering {
            // Use ML clustering for file selection if available
            self.build_clustered_file_paths(context, params).await
        } else {
            // Fallback to all collection files
            self.build_all_file_paths(context).await
        }
    }
    
    /// Build file paths using ML clustering optimization
    async fn build_clustered_file_paths(
        &self,
        context: &UnifiedSearchContext,
        _params: &SearchParams,
    ) -> Result<Vec<String>> {
        // For now, fall back to getting all files until ML clustering is implemented
        // In the future, this would integrate with ML clustering service to select
        // only the most relevant parquet files based on the query vector
        self.build_all_file_paths(context).await
    }
    
    /// Build all file paths for collection
    async fn build_all_file_paths(&self, context: &UnifiedSearchContext) -> Result<Vec<String>> {
        use crate::storage::assignment_service::get_assignment_service;
        
        debug!("📁 Building file paths for collection: {}", context.collection_id);
        
        // Get storage assignment for the collection
        let assignment_service = get_assignment_service();
        let storage_assignment = assignment_service
            .get_assignment(&context.collection_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("No storage assignment found for collection {}", context.collection_id))?;
        
        let storage_url = &storage_assignment.data_url;
        debug!("📁 Storage URL for collection {}: {}", context.collection_id, storage_url);
        
        // Use the parquet reader's filesystem (which was injected)
        let fs = self.parquet_reader.filesystem();
        let filesystem = fs.get_filesystem(storage_url)?;
        
        // List files in the data directory (storage_url already includes collection_id)
        let entries = match filesystem.list(storage_url).await {
            Ok(entries) => entries,
            Err(e) => {
                debug!("📁 Failed to list files in {}: {}", storage_url, e);
                return Ok(vec![]);
            }
        };
        
        // Find all .parquet files
        let mut files = Vec::new();
        for entry in entries {
            // Skip staging directories (start with __) and hidden files (start with .)
            if entry.name.starts_with("__") || entry.name.starts_with(".") {
                debug!("📁 Skipping staging/hidden entry: {}", entry.name);
                continue;
            }
            
            if entry.name.ends_with(".parquet") && !entry.metadata.is_directory {
                // In stateless design, DirEntry.url already contains full URL
                debug!("📁 Found parquet file: {}", entry.url);
                files.push(entry.url);
            }
        }
        
        // Sort files for consistent ordering
        files.sort();
        
        info!("📁 Found {} parquet files for collection {}", files.len(), context.collection_id);
        Ok(files)
    }
    
    /// Build collection context for reader - NO ADAPTERS NEEDED
    fn build_collection_context(
        &self,
        context: &UnifiedSearchContext,
        file_paths: &[String],
    ) -> CollectionContext {
        let filterable_columns = context.filterable_columns.iter().map(|col| {
            FilterableColumnSpec {
                name: col.name.clone(),
                data_type: format!("{:?}", col.data_type),
                is_indexed: col.is_indexed,
                estimated_cardinality: col.estimated_cardinality,
            }
        }).collect();
        
        CollectionContext {
            collection_id: context.collection_id.clone(),
            file_paths: file_paths.to_vec(),
            filterable_columns,
            quantization_columns: vec!["vector_pq8".to_string(), "vector_pq4".to_string()],
            estimated_size_mb: context.storage_info.estimated_size_mb,
            estimated_document_count: context.collection_config.as_ref()
                .map(|c| c.estimated_document_count)
                .unwrap_or(1000),
            is_cloud_storage: context.storage_info.is_cloud_storage,
        }
    }
}

// #[cfg(test)]
// mod tests;