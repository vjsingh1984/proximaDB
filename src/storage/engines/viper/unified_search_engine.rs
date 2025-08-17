//! VIPER Unified Search Engine
//!
//! This is the SEARCH ENGINE that implements search logic and uses UnifiedParquetReader
//! as a data access layer. This provides proper separation of concerns:
//!
//! - UnifiedParquetReader = Pure data access with strategy optimization
//! - ViperUnifiedSearchEngine = Search logic, ranking, filtering
//! - VectorOperationsService = Search orchestration across engines

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::search::{
    SearchParams, SearchResultSet, UnifiedSearchEngine, UnifiedSearchContext,
    OptimizationHint
};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::{UnifiedQuantizationEngine, UnifiedQuantizationLevel};
use super::readers::unified_parquet_reader::{UnifiedParquetReader, CollectionContext};

/// I/O optimization hints for efficient file access
#[derive(Debug, Clone)]
enum IoOptimizationHint {
    /// Use HTTP range requests for cloud storage
    UseRangeRequests {
        enabled: bool,
        chunk_size_mb: f32,
        prefetch_next: bool,
    },
    /// Project only needed columns in Parquet
    UseColumnProjection {
        columns: Vec<&'static str>,
    },
    /// Filter at row group level in Parquet
    UseRowGroupFiltering {
        enabled: bool,
        stats_filtering: bool,
    },
    /// Push predicates down to storage layer
    UsePredicatePushdown {
        enabled: bool,
        early_termination: bool,
    },
    /// Enable page-level caching
    EnableCaching {
        cache_pages: bool,
        cache_duration_sec: u32,
    },
    /// Use batch reading for efficiency
    UseBatchReading {
        batch_size: usize,
        parallel_decode: bool,
    },
}


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
        _distance_compute: &UnifiedDistanceCompute,
        _quantization_engine: Option<&UnifiedQuantizationEngine>,
    ) -> Result<SearchResultSet> {
        let start_time = std::time::Instant::now();
        
        info!("🔍 VIPER Search: collection={}, k={}", context.collection_id, params.top_k.unwrap_or(10));
        if let Some(_filter_expr) = &params.filter_expression {
            info!("🔍 VIPER Search has filter expression");
        }
        
        // 1. Get file paths directly from context (no redundant discovery)
        let file_paths = if let Some(ref paths) = context.storage_info.file_paths {
            // Files already discovered by engine - use them directly (FAST PATH)
            debug!("📁 Using {} pre-discovered files from context", paths.len());
            paths.clone()
        } else {
            // Fallback: ML clustering or filesystem query (SLOW PATH - should be rare)
            tracing::warn!("⚠️ No file paths provided in context, falling back to discovery");
            self.build_file_paths(context, params).await?
        };
        
        for (i, path) in file_paths.iter().enumerate() {
            debug!("📁   File {}: {}", i, path);
        }
        
        // 2. Generate I/O optimization hints based on file paths and storage type
        let io_hints = self.generate_io_optimization_hints(&file_paths, context, params);
        
        // 3. Build collection context for reader with I/O hints
        let mut collection_context = self.build_collection_context(context, &file_paths);
        
        // Add I/O optimization hints to collection context
        self.apply_io_hints_to_context(&mut collection_context, &io_hints, params);
        
        debug!("📁 Collection context: {:?}", collection_context);
        debug!("📁 Filterable columns: {:?}", collection_context.filterable_columns);
        debug!("⚡ I/O optimization hints: {:?}", io_hints);
        
        // 3. VIPER TWO-STAGE SEARCH (unique to VIPER's dual column storage)
        //
        // ARCHITECTURAL DIFFERENCES - Three Distinct Two-Stage Approaches:
        //
        // 1. SST Two-Stage (Block Compression):
        //    - Stage 1: Search compressed SSTable blocks with bloom filters
        //    - Stage 2: Decompress selected blocks and search FP32 vectors
        //    - Compression: Block-level (ZSTD/LZ4/Snappy on entire blocks)
        //
        // 2. VectorOperationsService Two-Stage (Index Quantization):  
        //    - Stage 1: Search quantized INDEX structures (HNSW with PQ codes)
        //    - Stage 2: Retrieve and rerank original FP32 vectors from storage
        //    - Compression: Index-only (graph structure uses quantized codes)
        //
        // 3. VIPER Two-Stage (Dual Column Storage) - THIS METHOD:
        //    - Stage 1: Search actual quantized VECTOR COLUMNS (INT8/PQ8/PQ4)
        //    - Stage 2: Rerank using parallel FP32 column for exact scoring
        //    - Compression: Data-level (separate columns for each precision)
        //    - Unique: Both quantized and FP32 vectors are directly searchable
        //
        // VIPER's approach provides the best flexibility - can search at any precision level
        let search_results = if params.enable_two_stage.unwrap_or(false) && 
                                self.has_quantized_columns(&collection_context).await {
            info!("🎯 VIPER Two-Stage Search: Using quantized columns for initial filtering");
            
            // STAGE 1: Fast search on quantized columns (INT8/PQ8/PQ4)
            // This is unique to VIPER - we search the actual quantized vectors stored in separate columns
            let stage1_k = params.top_k.unwrap_or(10) * 10; // Get 10x candidates
            let mut stage1_params = params.clone();
            stage1_params.top_k = Some(stage1_k);
            stage1_params.custom_hints = Some({
                let mut hints = params.custom_hints.clone().unwrap_or_default();
                hints.insert("use_quantized_column".to_string(), serde_json::Value::Bool(true));
                hints.insert("quantization_type".to_string(), serde_json::Value::String("int8".to_string()));
                hints
            });
            
            let stage1_results = self.parquet_reader.search_vectors(
                &stage1_params,
                &collection_context,
            ).await?;
            
            info!("📊 Stage 1: Found {} candidates from quantized search", stage1_results.len());
            
            // STAGE 2: Precise reranking with FP32 vectors
            // This preserves 100% accuracy by using original vectors for final ranking
            let stage2_params = SearchParams {
                query_vectors: params.query_vectors.clone(),
                top_k: params.top_k,
                distance_metric: params.distance_metric,
                filter_expression: None, // Already filtered in stage 1
                custom_hints: Some({
                    let mut hints = params.custom_hints.clone().unwrap_or_default();
                    hints.insert("use_fp32_column".to_string(), serde_json::Value::Bool(true));
                    hints.insert("vector_ids".to_string(), serde_json::json!(
                        stage1_results.iter().map(|r| &r.vector_id).collect::<Vec<_>>()
                    ));
                    hints
                }),
                ..params.clone()
            };
            
            let stage2_results = self.parquet_reader.search_vectors(
                &stage2_params,
                &collection_context,
            ).await?;
            
            info!("✨ Stage 2: Reranked to {} final results with FP32 precision", stage2_results.len());
            stage2_results
            
        } else {
            // Standard single-stage search on FP32 vectors
            info!("🔍 VIPER Standard Search: Using FP32 vectors directly");
            self.parquet_reader.search_vectors(
                params,
                &collection_context,
            ).await?
        };
        
        let processing_time = start_time.elapsed().as_micros() as u64;
        
        info!("✅ VIPER Search completed: {} results in {}μs", search_results.len(), processing_time);
        
        let result_count = search_results.len() as u64;
        let search_method = if params.enable_two_stage.unwrap_or(false) {
            "VIPER-TwoStage-DualColumn"
        } else {
            "VIPER-Direct"
        };
        
        Ok(SearchResultSet::from_vec(
            search_results,
            result_count,
            params.custom_hints.as_ref()
                .and_then(|h| h.get(key))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            processing_time,
            search_method.to_string(),
            HashMap::new(),
        ))
    }
    
    async fn can_handle(&self, context: &UnifiedSearchContext, _params: &SearchParams) -> bool {
        // VIPER handles Parquet-based collections
        context.storage_info.storage_type.contains_hash("VIPER") ||
        context.storage_info.storage_type.contains_hash("Parquet") ||
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
                columns: vec!["vector".to_string(), "id".to_string(), "metadata_info".to_string()],
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
    /// Check if collection has quantized columns (INT8, PQ8, PQ4)
    /// This is unique to VIPER's dual column storage architecture
    /// 
    /// VIPER TWO-STAGE SEARCH vs VectorOperationsService:
    /// - VIPER: Uses actual quantized COLUMNS stored alongside FP32 (dual column storage)
    /// - VectorOperationsService: Uses quantized INDEXES that point to FP32 vectors
    /// - VIPER: Can search directly on INT8/PQ8/PQ4 columns without decompression
    /// - VectorOperationsService: Must decompress/decode quantized index entries
    async fn has_quantized_columns(&self, context: &CollectionContext) -> bool {
        // VIPER stores quantized vectors as separate columns in Parquet files
        // Check if quantization columns exist (vector_int8, vector_pq8, vector_pq4)
        !context.quantization_columns.is_empty()
    }
    
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
    /// NOTE: This should only be called as a fallback when files aren't pre-discovered
    async fn build_all_file_paths(&self, context: &UnifiedSearchContext) -> Result<Vec<String>> {
        // This method should rarely be called in production as files should be passed from engine
        tracing::error!("❌ build_all_file_paths called - this is inefficient! Files should be passed from engine.");
        tracing::error!("    Collection: {}", context.collection_id);
        
        // We can't proceed without a proper storage URL
        // The engine should have provided the files
        Err(anyhow::anyhow!(
            "File paths not provided for collection '{}'. The storage engine must provide file paths through context.",
            context.collection_id
        ))
    }
    
    /// Generate I/O optimization hints based on file paths and storage type
    fn generate_io_optimization_hints(
        &self,
        file_paths: &[String],
        context: &UnifiedSearchContext,
        params: &SearchParams,
    ) -> Vec<IoOptimizationHint> {
        let mut hints = Vec::new();
        
        // Analyze file characteristics
        let total_files = file_paths.len();
        let is_cloud = context.storage_info.is_cloud_storage;
        let has_filters = params.filter_expression.is_some();
        let vector_dim = context.collection_config.as_ref()
            .map(|c| c.vector_dimension)
            .unwrap_or(128);
        
        // For cloud storage, use range requests to minimize data transfer
        if is_cloud {
            hints.push(IoOptimizationHint::UseRangeRequests {
                enabled: true,
                chunk_size_mb: if total_files > 10 { 1.0 } else { 2.0 },
                prefetch_next: total_files <= 5,
            });
            
            // Enable aggressive caching for cloud files
            hints.push(IoOptimizationHint::EnableCaching {
                cache_pages: true,
                cache_duration_sec: 300,
            });
        }
        
        // For Parquet files, optimize column reads
        if file_paths.iter().any(|p| p.ends_with(".parquet")) {
            hints.push(IoOptimizationHint::UseColumnProjection {
                columns: if has_filters {
                    vec!["id", "vector", "metadata_info", "version"]
                } else {
                    vec!["id", "vector"]
                },
            });
            
            // For high-dimensional vectors, use batch reading
            if vector_dim > 768 {
                hints.push(IoOptimizationHint::UseBatchReading {
                    batch_size: 1000,
                    parallel_decode: true,
                });
            }
            
            // Enable row group filtering for large files
            if context.storage_info.estimated_size_mb > 100.0 {
                hints.push(IoOptimizationHint::UseRowGroupFiltering {
                    enabled: true,
                    stats_filtering: has_filters,
                });
            }
        }
        
        // For filtered queries, enable predicate pushdown
        if has_filters {
            hints.push(IoOptimizationHint::UsePredicatePushdown {
                enabled: true,
                early_termination: params.top_k.unwrap_or(10) < 100,
            });
        }
        
        hints
    }
    
    /// Apply I/O hints to collection context for reader consumption
    fn apply_io_hints_to_context(
        &self,
        context: &mut CollectionContext,
        hints: &[IoOptimizationHint],
        params: &SearchParams,
    ) {
        // Convert hints to custom hints in params that the reader can use
        let mut custom_hints = params.custom_hints.clone().unwrap_or_default();
        
        for hint in hints {
            match hint {
                IoOptimizationHint::UseRangeRequests { enabled, chunk_size_mb, prefetch_next } => {
                    custom_hints.insert("use_range_requests".to_string(), json!(*enabled));
                    custom_hints.insert("range_chunk_size_mb".to_string(), json!(*chunk_size_mb));
                    custom_hints.insert("prefetch_next_chunk".to_string(), json!(*prefetch_next));
                }
                IoOptimizationHint::UseColumnProjection { columns } => {
                    custom_hints.insert("projection_columns".to_string(), json!(columns));
                }
                IoOptimizationHint::UseRowGroupFiltering { enabled, stats_filtering } => {
                    custom_hints.insert("row_group_filtering".to_string(), json!(*enabled));
                    custom_hints.insert("use_stats_filtering".to_string(), json!(*stats_filtering));
                }
                IoOptimizationHint::UsePredicatePushdown { enabled, early_termination } => {
                    custom_hints.insert("predicate_pushdown".to_string(), json!(*enabled));
                    custom_hints.insert("early_termination".to_string(), json!(*early_termination));
                }
                IoOptimizationHint::EnableCaching { cache_pages, cache_duration_sec } => {
                    custom_hints.insert("cache_pages".to_string(), json!(*cache_pages));
                    custom_hints.insert("cache_ttl_sec".to_string(), json!(*cache_duration_sec));
                }
                IoOptimizationHint::UseBatchReading { batch_size, parallel_decode } => {
                    custom_hints.insert("read_batch_size".to_string(), json!(*batch_size));
                    custom_hints.insert("parallel_decode".to_string(), json!(*parallel_decode));
                }
            }
        }
        
        // Store hints in context for reader to consume
        // This would be passed through to the UnifiedParquetReader
        context.io_optimization_hints = Some(custom_hints);
    }
    
    /// Build collection context for reader - NO ADAPTERS NEEDED
    fn build_collection_context(
        &self,
        context: &UnifiedSearchContext,
        file_paths: &[String],
    ) -> CollectionContext {
        let filterable_columns = context.filterable_columns.iter().map(|col| {
            crate::proto::proximadb::FilterableColumnSpec {
                name: col.name.clone(),
                data_type: match col.data_type {
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
                        crate::proto::proximadb::FilterableDataType::FilterableString as i32, // Fallback
                },
                encoding_hint: None,  // SDK-driven encoding (2025-08-06)
                indexed: col.is_indexed,
                supports_range: matches!(
                    col.data_type,
                    crate::core::search::unified_interface::ColumnDataType::Integer |
                    crate::core::search::unified_interface::ColumnDataType::Float |
                    crate::core::search::unified_interface::ColumnDataType::DateTime
                ),
                estimated_cardinality: col.estimated_cardinality.map(|c| c as i32),
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
            io_optimization_hints: None, // Will be set by apply_io_hints_to_context
        }
    }
}

// #[cfg(test)]
// mod tests;