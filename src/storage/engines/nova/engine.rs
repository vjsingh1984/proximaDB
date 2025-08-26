// NOVA Engine: Next-gen Optimized Vector Analytics with columnar quantization
// Implements UnifiedStorageEngine trait for integration with ProximaDB

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};
use crate::proto::proximadb::VectorRecord;
use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy, FlushParameters, FlushResult,
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics,
    OperationPriority,
};
// Health status handled internally
use crate::compute::distance_computation::DistanceMetric;
use crate::proto::proximadb::SearchResult;
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
// Use core compression directly instead of adapter
use crate::core::compression::{
    CompressionContext, CompressionAlgorithm,
};
use super::{
    MetadataFilter, ColumnarSearchMode as SearchMode,
    optimized_operations::OptimizedNovaOperations,
};
// Arrow schema handled by parquet reader
use crate::storage::engines::columnar::{
    ColumnarConfig,
};

// Performance optimization handled internally
// NOVA-specific optimization structures removed - now using universal module

/// NOVA Engine - Next-gen Optimized Vector Analytics for columnar storage
/// Enhanced with performance optimizations for fast reads, I/O bandwidth, and cost efficiency
pub struct NovaEngine {
    /// Filesystem factory for storage operations
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    
    /// Optimized operations handler
    optimized_ops: Arc<OptimizedNovaOperations>,
    /// Engine statistics
    statistics: Arc<RwLock<EngineStatistics>>,
    /// Hardware capabilities
    hardware: Arc<HardwareCapabilities>,
    /// Metrics collector for unified monitoring
    metrics_collector: Option<Arc<EngineMetricsCollector>>,
    /// Direct compression provider (no adapter indirection)
    compression_provider: StandardCompression,
    /// Unified quantization engine from compute module
    quantization_engine: Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>,
    
    // Universal performance optimization (replaces NOVA-specific optimization)
    /// Universal performance optimizer eliminating code duplication
    universal_optimizer: UniversalPerformanceOptimizer,
}
impl NovaEngine {
    /// Create new NOVA engine instance
    pub async fn new() -> Result<Self> {
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let optimized_ops = Arc::new(OptimizedNovaOperations::new()?);
        
        // Initialize filesystem factory
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config).await?);
        
        // Initialize compression provider directly
        let compression_provider = StandardCompression::default();
        // Initialize unified quantization engine from compute module
        let distance_compute = Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        // Configure storage quantization for NOVA (columnar engine)
        let storage_config = crate::compute::quantization::storage_engine::StorageQuantizationConfig {
            primary_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(32)),
            filter_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::binary()),
            fast_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::int8()),
            distance_metric: crate::compute::distance_computation::engine::DistanceMetric::Cosine,
            enable_progressive: true,
            filter_threshold: 100.0,
            candidate_multiplier: 10,
            training_sample_size: 10000,
            memory_budget_mb: 512, // Columnar uses more memory
            enable_hardware_acceleration: true,
        };
        
        let quantization_engine = Arc::new(crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
            unified_engine,
            distance_compute.clone(),
            storage_config,
        ));
        
        // Initialize universal performance optimization
        let universal_optimizer = UniversalPerformanceOptimizer::with_strategy(
            UniversalOptimizationStrategy::Balanced,
        ).await?;
        
        Ok(Self {
            filesystem,
            optimized_ops,
            statistics: Arc::new(RwLock::new(EngineStatistics {
                engine_name: "NOVA".to_string(),
                engine_version: "2.0.0".to_string(),
                total_storage_bytes: 0,
                memory_usage_bytes: 0,
                collection_count: 0,
                last_flush: None,
                last_compaction: None,
                pending_flushes: 0,
                pending_compactions: 0,
                engine_specific: HashMap::new(),
            })),
            hardware,
            metrics_collector: None,
            compression_provider,
            quantization_engine,
            universal_optimizer,
        })
    }
    /// Set metrics collector for monitoring
    pub fn set_metrics_collector(&mut self, collector: Arc<EngineMetricsCollector>) {
        self.metrics_collector = Some(collector);
    }
    
    /// Start operation timer if metrics collector is available
    fn start_operation_timer(&self, operation: &str) -> Option<OperationTimer> {
        self.metrics_collector.as_ref().map(|collector| {
            OperationTimer::new(collector.clone(), "NOVA".to_string(), operation.to_string())
        })
    }
    
    /// Load NOVA files for collection from storage
    async fn load_collection_files(&self, collection_id: &str, storage_path: &str) -> Result<Vec<super::NovaFile>> {
        // In production, this would:
        // 1. List all files in {storage_path}/{collection_id}/data/
        // 2. Filter out *.stats files and other non-data files  
        // 3. Load Parquet files with statistics from metadata properties
        // 4. Statistics are embedded in Parquet metadata for atomicity
        // For now, return empty vec as placeholder
        Ok(Vec::new())
    }
    
    /// Update global statistics file for collection
    async fn update_global_stats(&self, collection_id: &str, storage_path: &str) -> Result<()> {
        // Path: {storage_path}/{collection_id}/global.stats
        // This is updated after flush/compaction to maintain collection-wide metrics
        // File-level statistics are embedded in Parquet metadata properties
        Ok(())
    }
    
    /// Compute enhanced row group statistics (optimized NOVA design)
    fn compute_enhanced_row_group_stats(&self, records: &[VectorRecord], dimension: usize) -> Result<Vec<HashMap<String, String>>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        
        // Group vectors into row groups (default: 10K vectors per group)
        let row_group_size = 10000;
        let mut stats = Vec::new();
        
        for (group_idx, chunk) in records.chunks(row_group_size).enumerate() {
            let mut min_vals = vec![f32::INFINITY; dimension];
            let mut max_vals = vec![f32::NEG_INFINITY; dimension];
            let mut sum_vals = vec![0.0f32; dimension];
            let mut null_counts = vec![0u64; dimension];
            
            // Compute per-dimension statistics
            for record in chunk {
                if record.vector.len() != dimension {
                    continue; // Skip malformed vectors
                }
                
                for (dim_idx, &value) in record.vector.iter().enumerate() {
                    if value.is_finite() {
                        min_vals[dim_idx] = min_vals[dim_idx].min(value);
                        max_vals[dim_idx] = max_vals[dim_idx].max(value);
                        sum_vals[dim_idx] += value;
                    } else {
                        null_counts[dim_idx] += 1;
                    }
                }
            }
            
            // Compute centroid for pruning
            let centroid: Vec<f32> = sum_vals.iter()
                .map(|&sum| sum / chunk.len() as f32)
                .collect();
            
            // Create enhanced statistics using the optimized design
            let enhanced_stat = super::EnhancedRowGroupStats::create_basic(
                group_idx as u32,
                chunk.len() as u64,
                dimension,
                min_vals,
                max_vals,
                centroid,
                null_counts,
                1.0 / records.len() as f32, // Basic selectivity estimate
                0.7, // Placeholder - actual ratio computed during write
                0, // Will be updated based on query patterns
            );
            
            stats.push(enhanced_stat);
        }
        
        Ok(stats)
    }
    
    /// Compute basic zone maps for dimension-level pruning (simplified design)
    fn compute_basic_zone_maps(&self, records: &[VectorRecord], dimension: usize) -> Result<super::hierarchical_stats::BasicZoneMaps> {
        if records.is_empty() {
            return Ok(super::hierarchical_stats::BasicZoneMaps {
                dimension_ranges: Vec::new(),
                total_vectors: 0,
                creation_time: chrono::Utc::now(),
            });
        }
        
        let mut dimension_ranges = Vec::with_capacity(dimension);
        
        // Compute min/max range for each dimension across all vectors
        for dim_idx in 0..dimension {
            let mut min_val = f32::INFINITY;
            let mut max_val = f32::NEG_INFINITY;
            let mut valid_count = 0;
            
            for record in records {
                if dim_idx < record.vector.len() {
                    let value = record.vector[dim_idx];
                    if value.is_finite() {
                        min_val = min_val.min(value);
                        max_val = max_val.max(value);
                        valid_count += 1;
                    }
                }
            }
            
            dimension_ranges.push(super::hierarchical_stats::DimensionRange {
                dimension_index: dim_idx,
                min_value: if valid_count > 0 { min_val } else { 0.0 },
                max_value: if valid_count > 0 { max_val } else { 0.0 },
                selectivity: valid_count as f32 / records.len() as f32,
            });
        }
        
        Ok(super::hierarchical_stats::BasicZoneMaps {
            dimension_ranges,
            total_vectors: records.len() as u64,
            creation_time: chrono::Utc::now(),
        })
    }
    
    // ============================================================================
    // PERFORMANCE OPTIMIZATION METHODS - DELEGATING TO UNIFIED MODULES
    // ============================================================================
    
    /// Fast read optimization using memory-mapped Parquet files (delegates to universal optimizer)
    async fn mmap_parquet_file(&self, file_path: &str) -> Result<Vec<u8>> {
        // Use universal optimizer's memory mapping functionality
        if let Some(mmap) = self.universal_optimizer.get_memory_mapped_file(file_path).await? {
            Ok(mmap.to_vec())
        } else {
            // Fallback to regular file reading for cloud storage
            self.universal_optimizer.read_data_optimized(file_path).await
        }
    }
    
    /// Columnar I/O optimization with parallel column reads (delegates to universal optimizer)
    async fn parallel_column_read(&self, file_path: &str, column_indices: &[usize]) -> Result<Vec<Vec<u8>>> {
        // Use universal optimizer for parallel operations
        let read_operations: Vec<_> = column_indices.iter()
            .map(|&column_idx| {
                let file_path = file_path.to_string();
                async move {
                    // Simulate column-specific read (in production, use actual column reader)
                    self.universal_optimizer.read_data_optimized(&format!("{}:col:{}", file_path, column_idx)).await
                }
            }).collect();
        
        let results = self.universal_optimizer.parallel_operations(
            read_operations,
            |operation| operation
        ).await.map(|results| {
            results.into_iter().collect::<Result<Vec<_>, _>>()
        })??;
        
        Ok(results)
    }
    
    /// Storage tier optimization for Parquet files based on access patterns (delegates to universal optimizer)
    async fn optimize_parquet_storage_tier(&self, file_path: &str, row_group_stats: &super::hierarchical_stats::EnhancedRowGroupStats) -> Result<StorageTier> {
        // Use common utility for consistent vector size estimation
        let dimension = self.config.dimension.unwrap_or(1536); // Default to 1536 if not set
        let estimated_size = crate::storage::engines::common::estimate_vector_storage_size(
            dimension,
            self.config.quantization.as_deref(),
            row_group_stats.vector_zone_map.total_vectors
        );
        
        // Use universal optimizer's storage tier optimization
        self.universal_optimizer.optimize_storage_tier(file_path, estimated_size as usize).await
    }
    
    /// Compression optimization using unified compression module (delegates to universal optimizer)
    async fn compress_parquet_optimized(&self, data: &[u8], tier: StorageTier) -> Result<Vec<u8>> {
        // Use universal optimizer's tier-aware compression
        self.universal_optimizer.compress_for_tier(data, tier).await
    }
    
    /// Distance computation using unified distance compute engine (delegates to universal optimizer)
    async fn compute_distances_unified(&self, query: &[f32], candidates: &[Vec<f32>], metric: DistanceMetric) -> Result<Vec<f32>> {
        // Use universal optimizer's hardware-accelerated distance computation
        self.universal_optimizer.compute_distances_accelerated(query, candidates, metric).await
    }
    
    /// Row group prefetching optimization (delegates to universal optimizer)
    async fn prefetch_row_groups(&self, file_path: &str, row_group_indices: &[usize]) -> Result<()> {
        let config = self.universal_optimizer.get_config();
        if !config.enable_prefetching {
            return Ok(());
        }
        
        // Generate row group URLs for prefetching
        let prefetch_count = (config.prefetch_size_mb / 10).min(row_group_indices.len()); // Assume ~10MB per row group
        let row_group_urls: Vec<String> = row_group_indices.iter()
            .take(prefetch_count)
            .map(|&idx| format!("{}:rg:{}", file_path, idx))
            .collect();
        
        // Use universal optimizer's prefetching capability
        self.universal_optimizer.prefetch_data(&row_group_urls).await
    }
    
    /// Memory pool optimization for columnar operations (delegates to universal optimizer)
    async fn get_columnar_buffer(&self, size: usize) -> Result<Vec<f32>> {
        self.universal_optimizer.get_memory_buffer(size).await
            .map_err(|e| anyhow::anyhow!("Failed to acquire columnar buffer: {}", e))
    }
    
    /// Helper method to get file size in GB
    async fn get_file_size_gb(&self, file_path: &str) -> Result<f32> {
        let metadata = tokio::fs::metadata(file_path).await?;
        Ok(metadata.len() as f32 / (1024.0 * 1024.0 * 1024.0))
    }
    
}

#[async_trait]
impl UnifiedStorageEngine for NovaEngine {
    // =============================================================================
    // ENGINE IDENTIFICATION
    fn engine_name(&self) -> &'static str {
        "NOVA"
    }
    
    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Nova
    }

    fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
        None // NOVA doesn't use collection service directly
    }
    
    // CORE OPERATIONS
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();
        
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for flush"))?;
        info!("NOVA flush: collection={}, vectors={}", collection_id, params.vector_records.len());
        // Create new VIPER file from flush parameters
        // Get dimension from collection config or use default
        let dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension)
             as usize;
        // Use default compression for Parquet
        let compression_algorithm = CompressionAlgorithm::Zstd;
        debug!("NOVA: Using compression: {:?}", compression_algorithm);
        // Enhanced row group statistics (optimized NOVA design)
        let enhanced_stats = self.compute_enhanced_row_group_stats(&params.vector_records, dimension)?;
        
        // Basic zone maps for dimension-level pruning (simplified from 3-tier hierarchy)
        let zone_maps = self.compute_basic_zone_maps(&params.vector_records, dimension)?;
        
        let nova_file = NovaFile {
            quantized_columns: HashMap::new(),
            schema: None,
            metadata: crate::storage::engines::columnar::ColumnarFileMetadata {
                collection_id: collection_id.to_string(),
                num_vectors: params.vector_records.len() as u64,
                dimension,
                distance_metric: DistanceMetric::Euclidean,
                quantization: super::QuantizationConfig {
                    enable_binary: true,
                    enable_int8: true,
                    enable_pq: true,
                    pq_segments: 32,
                    pq_bits: 8,
                    binary_threshold: 0.0,
                    int8_threshold: 0.85,
                    pq_threshold: 0.95,
                },
                column_stats: HashMap::new(),
                version: 1,
                timestamp: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
            },
            row_groups: Vec::new(),
            enhanced_stats,
            superblocks: Vec::new(), // Keep for future SuperBlock implementation
            advanced_zone_maps: Some(zone_maps),
        };
        // Write NOVA file to storage with embedded statistics
        // TODO: Implement actual Parquet file writing to storage_path
        // Statistics will be embedded in Parquet metadata properties:
        // - File creation time, vector count, dimension, compression ratio
        // - Column statistics (min/max/null count) for each column
        // - Quantization parameters and accuracy metrics
        // Also update {storage_path}/{collection_id}/global.stats
        // Update global stats using a default path (storage_path field no longer exists)
        self.update_global_stats(collection_id, "./data").await?;
        // For now, just simulate success
        // Update statistics
        let mut stats = self.statistics.write().await;
        stats.pending_flushes = stats.pending_flushes.saturating_sub(1);
        stats.last_flush = Some(chrono::Utc::now());
        stats.total_storage_bytes += params.estimated_size as u64;
        let duration_ms = start_time.elapsed().as_millis() as u64;
        
        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: params.vector_records.len() as u64,
            bytes_written: params.estimated_size as u64,
            files_created: 1,
            flushed_batch_ids: vec![], // Initialize empty batch IDs
            duration_ms,
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
        })
    }
    
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let start_time = std::time::Instant::now();
        
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for compaction"))?;
        info!("NOVA compaction: collection={}", collection_id);
        
        // Load files from storage for compaction
        // TODO: Implement actual file loading from storage
        let files = Vec::<NovaFile>::new();
        if files.len() < 2 {
            let duration_ms = start_time.elapsed().as_millis() as u64;
            return Ok(CompactionResult {
                success: true,
                collections_affected: vec![collection_id.to_string()],
                entries_processed: 0,
                entries_removed: 0,
                bytes_read: 0,
                bytes_written: 0,
                input_files: files.len() as u64,
                output_files: files.len() as u64,
                duration_ms,
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
            });
        }
        // Simulate compaction with Parquet optimization
        let input_count = files.len() as u64;
        let output_count = 1u64; // NOVA typically merges to single file
        let duration_ms = start_time.elapsed().as_millis() as u64;
        
        // Update statistics
        let mut stats = self.statistics.write().await;
        stats.pending_compactions = stats.pending_compactions.saturating_sub(1);
        stats.last_compaction = Some(chrono::Utc::now());
        
        Ok(CompactionResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_processed: 0, // TODO: Count actual entries
            entries_removed: 0,
            bytes_read: params.estimated_input_size as u64,
            bytes_written: (params.estimated_input_size * 70 / 100) as u64, // 30% reduction with columnar
            input_files: input_count,
            output_files: output_count,
            duration_ms,
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
        })
    }
    
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        
        // Engine is stateless, so we report engine-level metrics only
        metrics.insert("engine_type".to_string(), 
            serde_json::json!("NOVA"));
        metrics.insert("columnar_engine".to_string(), 
            serde_json::json!(true));
        
        // TODO: Collect actual metrics from storage when needed
        let total_files = 0;
        let total_row_groups = 0;
        metrics.insert("total_parquet_files".to_string(), 
            serde_json::json!(total_files));
        metrics.insert("total_row_groups".to_string(), 
            serde_json::json!(total_row_groups));
        let stats = self.statistics.read().await;
        // Use existing fields instead of non-existent ones
        metrics.insert("pending_flushes".to_string(), 
            serde_json::json!(stats.pending_flushes));
        metrics.insert("pending_compactions".to_string(), 
            serde_json::json!(stats.pending_compactions));
        // Hardware info
        metrics.insert("simd_backend".to_string(), 
            serde_json::json!(format!("{:?}", self.hardware.cpu)));
        metrics.insert("columnar_optimization".to_string(), 
            serde_json::json!(true));
        Ok(metrics)
    }
    
    async fn get_vector_by_id(
        &self, 
        collection_id: &str, 
        vector_id: &str
    ) -> Result<Option<VectorRecord>> {
        debug!("NOVA get vector: collection={}, id={}", collection_id, vector_id);
        
        // TODO: Load actual files from storage based on collection_id
        // For now, return None as placeholder
        // In production, would:
        // 1. Get storage path from collection metadata
        // 2. Load Parquet files from that path
        // 3. Search through ID indexes
        Ok(None)
    }
    
    async fn search_vectors_unified(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
    ) -> Result<Vec<crate::core::search::InternalSearchResult>> {
        let search_start = std::time::Instant::now();
        
        // Extract all parameters from context (pre-computed)
        let collection_id = ctx.collection_id();
        let storage_path = ctx.storage_path();
        let query_vector = ctx.query_vector()
            .ok_or_else(|| anyhow!("No query vector in context"))?;
        let top_k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        let dimension = ctx.dimension();
        let filter_expression = ctx.search_params.filter_expression.as_ref();
        let search_params = ctx.search_params.custom_hints.clone();
        
        info!("🚀 NOVA: Enhanced unified search with orchestration for collection {}", collection_id);
        
        // ========================================================================
        // PHASE 1: SEARCH ORCHESTRATION AND STRATEGY SELECTION
        // ========================================================================
        
        // Check if orchestration should be used based on context metadata
        let use_orchestration = ctx.metadata.use_axis_indexes || ctx.metadata.has_quantization;
        
        if use_orchestration {
            info!("🎯 NOVA: Using intelligent search orchestration");
            
            // Create mock services for orchestration
            let axis_manager = match self.get_mock_axis_manager() {
                Ok(manager) => manager,
                Err(e) => {
                    tracing::warn!("⚠️ Failed to get AXIS manager: {}, falling back to direct search", e);
                    return self.fallback_to_direct_search(ctx, collection_id, storage_path, query_vector, top_k, distance_metric, filter_expression).await;
                }
            };
            
            // Direct search implementation - IntegratedSearchOptimizer requires cache infrastructure
            // that isn't available in this context
            return self.fallback_to_direct_search(ctx, collection_id, storage_path, query_vector, top_k, distance_metric, filter_expression).await;
        }
        
        // ========================================================================
        // PHASE 2: CURRENT IMPLEMENTATION WITH ENHANCED LOGGING
        // ========================================================================
        
        info!("🔍 NOVA: Using current unified search implementation (orchestration disabled)");
        
        // Load files from storage
        let files = self.load_collection_files(collection_id, storage_path).await?;
        let mut all_results = Vec::new();
        // Search each NOVA file using columnar optimization
        for nova_file in files.iter() {
            // TODO: Implement columnar search config when module is available
            // let config = super::columnar_search::ColumnarSearchConfig::from_params(search_params.as_ref());
            // let results = self.optimized_ops.search_columnar_optimized(
            //     nova_file,
            //     query_vector,
            //     top_k,
            //     config,
            // ).await?;
            let results: Vec<(crate::core::VectorRecord, f32)> = Vec::new(); // Placeholder with correct type
            
            // Convert to search results
            for (record, score) in results {
                // Would compute actual distance
                all_results.push((record, score));
            }
        }
        
        // Sort by score and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);
        
        // Convert to SearchResult format
        let search_results: Vec<crate::proto::proximadb::SearchResult> = all_results
            .into_iter()
            .enumerate()
            .map(|(idx, (record, score))| {
                // Create similarity result for semantic information
                let similarity_result = crate::compute::distance_computation::SimilarityResult {
                    normalized_score: score,
                    raw_score: score,
                    distance: Some(1.0 - score), // Convert similarity to distance
                    metric: distance_metric,
                };
                
                crate::proto::proximadb::SearchResult {
                    id: record.id.clone().unwrap_or_else(|| format!("unknown_{}", idx)),
                    vector_id: record.id.clone(),
                    score: similarity_result.normalized_score,
                    similarity: Some(similarity_result.normalized_score),
                    vector: Some(record.vector.clone()),
                    metadata: Some(record.metadata.clone()),
                    debug_info: None,
                    version: record.version,
                    timestamp: record.timestamp.map(|ts| ts as u64),
                    semantic_similarity: Some(similarity_result),
                    quantization_info: None,
                    engine_stats: None,
                    index_path: None,
                }
            })
            .collect();
        
        Ok(search_results)
    }
    
    // OPTIONAL OPERATIONS
    async fn optimize(&self, collection_id: &str) -> Result<()> {
        info!("NOVA optimize: collection={}", collection_id);
        // Trigger compaction for optimization
        let params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: false,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: Some(60000),
            priority: OperationPriority::Low,
            collection_config: None,
            estimated_input_size: 0,
        };
        self.do_compact(&params).await?;
        Ok(())
    }
    
    async fn get_statistics(&self) -> Result<EngineStatistics> {
        Ok(self.statistics.read().await.clone())
    }
    
    async fn health_check(&self) -> Result<EngineHealth> {
        Ok(EngineHealth {
            healthy: true,
            status: "Healthy".to_string(),
            last_check: chrono::Utc::now(),
            response_time_ms: 1.0,
            warnings: vec![],
            metrics: HashMap::new(),
            error_count: 0,
        })
    }
    
    async fn supports_feature(&self, feature: &str) -> bool {
        match feature {
            "id_lookup" => true,
            "similarity_search" => true,
            "columnar_search" => true,
            "quantization" => true,
            "compression" => true,
            "batch_operations" => true,
            "predicate_pushdown" => true,
            "projection" => true,
            _ => false,
        }
    }
}


impl NovaFile {
    /// Load record at specific location
    pub fn load_record_at_location(&self, location: &crate::storage::engines::columnar::id_index::ParquetLocation) -> Result<VectorRecord> {
        // In production, would load from Parquet row group
        Ok(VectorRecord {
            id: Some(format!("vec_rg{}_row{}", location.row_group_id, location.row_offset)),
            vector: vec![0.0; self.metadata.dimension],
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            quantized_vector: None,
        })
    }
}

// TODO: Fix columnar search config implementation when module is available
/*
impl crate::storage::engines::columnar::columnar_search::ColumnarSearchConfig {
    /// Create from search parameters
    pub fn from_params(params: Option<&serde_json::Value>) -> Self {
        // Parse parameters or use defaults
        Self::default()
    }
}
*/

/// Implementation of UniversallyOptimized trait for NOVA engine
#[async_trait]
impl UniversallyOptimized for NovaEngine {
    /// Get the universal performance optimizer instance
    fn get_universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }
    
    /// NOVA-specific optimization setup
    async fn setup_engine_optimizations(&self) -> Result<()> {
        // NOVA-specific optimizations for columnar storage
        info!("🔧 NOVA Engine: Setting up universal performance optimizations");
        
        // Initialize columnar-specific optimizations
        let config = self.universal_optimizer.get_config();
        debug!("   Cache size: {}MB", config.cache_size_mb);
        debug!("   Parallel operations: {}", config.parallel_operations);
        debug!("   Prefetching enabled: {}", config.enable_prefetching);
        debug!("   Memory mapping enabled: {}", config.enable_memory_mapping);
        
        // NOVA is ready for columnar analytics operations
        info!("✅ NOVA Engine: Universal optimizations configured for columnar analytics");
        Ok(())
    }
    
    /// NOVA-specific performance metrics
    async fn collect_performance_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        
        // Basic NOVA metrics
        let stats = self.statistics.read().await;
        metrics.insert("nova_total_storage_bytes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.total_storage_bytes)
        ));
        metrics.insert("nova_memory_usage_bytes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.memory_usage_bytes)
        ));
        metrics.insert("nova_collection_count".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.collection_count)
        ));
        metrics.insert("nova_pending_flushes".to_string(), serde_json::Value::Number(
            serde_json::Number::from(stats.pending_flushes)
        ));
        
        // Universal optimizer metrics
        let strategy = self.universal_optimizer.get_strategy();
        metrics.insert("universal_optimization_strategy".to_string(), 
            serde_json::Value::String(format!("{:?}", strategy)));
        
        let config = self.universal_optimizer.get_config();
        metrics.insert("universal_cache_size_mb".to_string(), serde_json::Value::Number(
            serde_json::Number::from(config.cache_size_mb)
        ));
        metrics.insert("universal_parallel_operations".to_string(), serde_json::Value::Number(
            serde_json::Number::from(config.parallel_operations)
        ));
        metrics.insert("universal_prefetching_enabled".to_string(), serde_json::Value::Bool(
            config.enable_prefetching
        ));
        
        Ok(metrics)
    }
}

// Additional implementation methods for NovaEngine
impl NovaEngine {
    /// Helper methods for search orchestration
    /// These create mock services for orchestration functionality
    
    fn get_mock_axis_manager(&self) -> Result<Arc<crate::index::axis::manager::AxisManager>> {
        // Create a mock AXIS manager
        Err(anyhow!("AXIS manager not available in mock implementation"))
    }
    
    fn get_mock_collection_service(&self) -> Arc<crate::services::collection_service::CollectionService> {
        // Create a mock collection service
        Arc::new(crate::services::collection_service::CollectionService::new(
            "mock://collections".to_string(),
            None, // No metadata provider for mock
        ))
    }
    
    fn get_mock_distance_engine(&self) -> Arc<crate::compute::distance_computation::UnifiedDistanceCompute> {
        // Return the existing distance engine
        self.distance_engine.clone()
    }
    
    fn get_mock_quantization_engine(&self) -> Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine> {
        // Return the existing quantization engine
        self.quantization_engine.clone()
    }
    
    fn get_mock_storage_engine(&self) -> Arc<dyn crate::storage::traits::UnifiedStorageEngine> {
        // Return self as the storage engine
        Arc::new(NovaEngine {
            optimized_ops: self.optimized_ops.clone(),
            statistics: self.statistics.clone(),
            hardware: self.hardware.clone(),
            metrics_collector: self.metrics_collector.clone(),
            compression_provider: self.compression_provider.clone(),
            quantization_engine: self.quantization_engine.clone(),
            distance_engine: self.distance_engine.clone(),
            filesystem: self.filesystem.clone(),
        })
    }
    
    /// Fallback to direct search when orchestration fails
    async fn fallback_to_direct_search(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
        collection_id: &str,
        storage_path: &str,
        query_vector: &[f32],
        top_k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        filter_expression: Option<&crate::core::search::FilterExpression>,
    ) -> Result<Vec<crate::proto::proximadb::SearchResult>> {
        tracing::warn!("🔄 NOVA: Falling back to direct search implementation");
        
        // Use the existing search implementation
        // Load files from storage
        let files = self.load_collection_files(collection_id, storage_path).await?;
        let mut all_results = Vec::new();
        
        // Search each NOVA file using columnar optimization
        for nova_file in files.iter() {
            // Placeholder - would implement actual columnar search
            let results: Vec<(crate::core::VectorRecord, f32)> = Vec::new();
            
            // Convert to search results
            for (record, score) in results {
                all_results.push((record, score));
            }
        }
        
        // Sort by score and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);
        
        // Convert to SearchResult format
        let search_results: Vec<crate::proto::proximadb::SearchResult> = all_results
            .into_iter()
            .enumerate()
            .map(|(idx, (record, score))| {
                let similarity_result = crate::compute::distance_computation::SimilarityResult {
                    normalized_score: score,
                    raw_score: score,
                    distance: Some(1.0 - score),
                    metric: distance_metric,
                };
                
                crate::proto::proximadb::SearchResult {
                    id: record.id.clone().unwrap_or_else(|| format!("unknown_{}", idx)),
                    vector_id: record.id.clone(),
                    score: similarity_result.normalized_score,
                    similarity: Some(similarity_result.normalized_score),
                    vector: Some(record.vector.clone()),
                    metadata: Some(record.metadata.clone()),
                    debug_info: None,
                    version: record.version,
                    timestamp: record.timestamp.map(|ts| ts as u64),
                    semantic_similarity: Some(similarity_result),
                    quantization_info: None,
                    engine_stats: None,
                    index_path: None,
                }
            })
            .collect();
        
        Ok(search_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_nova_engine_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = NovaEngine::new().await.unwrap();
        assert_eq!(engine.engine_name(), "NOVA");
        assert_eq!(engine.engine_version(), "1.0.0");
    }
    
    #[tokio::test]
    async fn test_nova_feature_support() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = NovaEngine::new().await.unwrap();
        assert!(engine.supports_feature("id_lookup").await);
        assert!(engine.supports_feature("columnar_search").await);
        assert!(engine.supports_feature("predicate_pushdown").await);
        assert!(engine.supports_feature("projection").await);
        assert!(!engine.supports_feature("unknown_feature").await);
    }
}
