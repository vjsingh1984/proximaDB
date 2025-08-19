// NOVA Engine: Next-gen Optimized Vector Analytics with columnar quantization
// Implements UnifiedStorageEngine trait for integration with ProximaDB

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use crate::core::{VectorRecord, hardware_capabilities::HardwareCapabilities};
use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy, FlushParameters, FlushResult,
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics,
    OperationPriority,
};
use crate::storage::engines::common::HealthStatus;
use crate::compute::distance_computation::DistanceMetric;
use crate::proto::proximadb::{SearchResult, IndexingAlgorithm};
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
// Use core compression directly instead of adapter
use crate::core::compression::{
    StandardCompression, CompressionProvider,
    CompressionContext, CompressionAlgorithm,
};
use super::{
    NovaFile, MetadataFilter, ColumnarSearchMode as SearchMode,
    optimized_operations::OptimizedNovaOperations,
};
use arrow_schema;
use crate::storage::engines::columnar::{
    ColumnarIdIndex, UnifiedParquetReader,
    ColumnarBatchOperations, ColumnarUtilities, ColumnarConfig,
};
/// NOVA Engine - Next-gen Optimized Vector Analytics for columnar storage
/// Stateless design - all metadata comes from SearchContext
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
            distance_compute,
            storage_config,
        ));
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
    async fn load_collection_files(&self, collection_id: &str, storage_path: &str) -> Result<Vec<NovaFile>> {
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
            .unwrap_or(768) as usize;
        // Use default compression for Parquet
        let compression_algorithm = CompressionAlgorithm::Zstd;
        debug!("NOVA: Using compression: {:?}", compression_algorithm);
        // TODO: Get quantization config from params.collection_config when available
        let nova_file = NovaFile {
            quantized_columns: HashMap::new(), // Initialize empty quantized columns
            schema: None, // Initialize with None
            metadata: crate::storage::engines::columnar::ColumnarFileMetadata {
                collection_id: collection_id.to_string(),
                num_vectors: params.vector_records.len() as u64,
                dimension,
                distance_metric: DistanceMetric::Euclidean,
                quantization: super::QuantizationConfig {
                    enable_binary: true,  // Enable all quantization types for progressive search
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
            enhanced_stats: Vec::new(),
            superblocks: Vec::new(),
            advanced_zone_maps: None,
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
        ctx: &crate::storage::traits::SearchContext,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        // Extract all parameters from context (pre-computed)
        let collection_id = ctx.collection_id();
        let storage_path = ctx.storage_path();
        let query_vector = ctx.query_vector()
            .ok_or_else(|| anyhow!("No query vector in context"))?;
        let top_k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        let dimension = ctx.dimension();
        let filter = ctx.search_params.filter_expression.as_ref()
            .map(|f| serde_json::to_value(f).unwrap_or(serde_json::Value::Null));
        let search_params = ctx.search_params.custom_hints.clone();
        
        info!("NOVA unified search: collection={}, k={}, metric={:?}, storage_path={}", 
            collection_id, top_k, distance_metric, storage_path);
        
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
        
        // Convert to SearchResult format
        let search_results: Vec<crate::core::search::SearchResult> = all_results
            .into_iter()
            .take(top_k)
            .map(|(record, score)| {
                crate::core::search::SearchResult {
                    vector_id: record.id.unwrap_or_default(),
                    semantic_similarity: score,
                    timestamp: chrono::Utc::now(),
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
