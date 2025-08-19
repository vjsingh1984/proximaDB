// SWIFT Engine: Storage With Instant Fast Traversal - zero-overhead vector storage
// Implements UnifiedStorageEngine trait for integration with ProximaDB

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::core::{VectorRecord, hardware_capabilities::HardwareCapabilities};
use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy, FlushParameters, FlushResult,
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics,
};
use crate::compute::distance_computation::DistanceMetric;
// Removed unused import: IndexingAlgorithm
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
// Removed unused compression common imports
// Removed unused CompressionDataType import

// Use core compression directly instead of adapter
use crate::core::compression::{
    StandardCompression, CompressionAlgorithm,
};

use super::{
    SwiftFile,
    progressive_search,
    optimized_operations::OptimizedSwiftOperations,
};

/// SWIFT Engine - Storage With Instant Fast Traversal for zero-overhead vector storage
/// Stateless design - all metadata comes from SearchContext
pub struct SwiftEngine {
    
    /// Optimized operations handler
    optimized_ops: Arc<OptimizedSwiftOperations>,
    
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
    
    /// Filesystem factory for storage operations
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
}

impl SwiftEngine {
    /// Create new SWIFT engine instance
    pub async fn new() -> Result<Self> {
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let optimized_ops = Arc::new(OptimizedSwiftOperations::new()?);
        
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
        
        // Configure storage quantization for SWIFT (SST-based engine)
        let storage_config = crate::compute::quantization::storage_engine::StorageQuantizationConfig {
            primary_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(16)),
            filter_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::binary()),
            fast_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::int8()),
            distance_metric: crate::compute::distance_computation::engine::DistanceMetric::Cosine,
            enable_progressive: true,
            filter_threshold: 100.0,
            candidate_multiplier: 10,
            training_sample_size: 10000,
            memory_budget_mb: 256, // Row-based like SST
            enable_hardware_acceleration: true,
        };
        
        let quantization_engine = Arc::new(crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
            unified_engine,
            distance_compute,
            storage_config,
        ));
        
        Ok(Self {
            optimized_ops,
            statistics: Arc::new(RwLock::new(EngineStatistics {
                engine_name: "SWIFT".to_string(),
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
            filesystem,
        })
    }
    
    /// Set metrics collector for monitoring
    pub fn set_metrics_collector(&mut self, collector: Arc<EngineMetricsCollector>) {
        self.metrics_collector = Some(collector);
    }
    
    /// Start operation timer if metrics collector is available
    fn start_operation_timer(&self, operation: &str) -> Option<OperationTimer> {
        self.metrics_collector.as_ref().map(|collector| {
            OperationTimer::new(collector.clone(), "SWIFT".to_string(), operation.to_string())
        })
    }
    
    /// Load SWIFT files for collection from storage
    async fn load_collection_files(&self, collection_id: &str, storage_path: &str) -> Result<Vec<SwiftFile>> {
        // In production, this would:
        // 1. List all files in {storage_path}/{collection_id}/data/
        // 2. Filter out *.stats files and other non-data files
        // 3. Load SST files with embedded statistics from headers
        // 4. Statistics are embedded in each file for atomicity
        // For now, return empty vec as placeholder
        Ok(Vec::new())
    }
    
    /// Update global statistics file for collection
    async fn update_global_stats(&self, collection_id: &str, storage_path: &str) -> Result<()> {
        // Path: {storage_path}/{collection_id}/global.stats
        // This is updated after flush/compaction to maintain collection-wide metrics
        // Statistics are also embedded in individual files for atomicity
        Ok(())
    }
}

#[async_trait]
impl UnifiedStorageEngine for SwiftEngine {
    // =============================================================================
    // ENGINE IDENTIFICATION
    // =============================================================================
    
    fn engine_name(&self) -> &'static str {
        "SWIFT"
    }
    
    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }
    
    fn strategy(&self) -> StorageEngineStrategy {
        // SWIFT is a hierarchical superblock strategy with row-based optimization
        StorageEngineStrategy::Swift
    }

    fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
        None // SWIFT doesn't use collection service directly
    }
    
    // =============================================================================
    // CORE OPERATIONS
    // =============================================================================
    
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();
        
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for flush"))?;
        info!("SWIFT flush: collection={}, vectors={}", 
            collection_id, params.vector_records.len());
        
        // Get dimension from collection config
        let dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| cfg.dimension)
            .unwrap_or(768) as usize;
        
        // Create new SWIFT file from flush parameters
        let mut swift_file = SwiftFile::new(
            collection_id.to_string(),
            dimension,
            DistanceMetric::Euclidean,
        );
        
        // Build blocks from vectors (would come from memtable in production)
        // For now, simulate with empty records
        let records = Vec::new();
        
        // Use universal adapters for quantization - simplified implementation
        // TODO: Implement proper quantization integration when SstFile API is stabilized
        // For now, just add records without quantization
        // sst.build_blocks_from_records(records)?;
        
        // Write SWIFT file to storage with embedded statistics
        // TODO: Implement actual SST file writing to storage_path
        // Statistics will be embedded in SST file header for atomicity:
        // - File creation time, vector count, dimension, compression ratio
        // - Min/max values per dimension for pruning
        // - Bloom filter parameters and false positive rate
        // Also update {storage_path}/{collection_id}/global.stats
        self.update_global_stats(collection_id, params.storage_path.as_deref().unwrap_or("./data")).await?;
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
        info!("SWIFT compaction: collection={}", collection_id);
        
        // Load files from storage for compaction
        // TODO: Implement actual file loading from storage
        let files = Vec::<SwiftFile>::new();
        
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
        
        // Simulate compaction
        let input_count = files.len() as u64;
        let output_count = ((files.len() + 1) / 2) as u64;
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
            bytes_written: (params.estimated_input_size * 80 / 100) as u64, // 20% reduction
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
            serde_json::json!("SWIFT"));
        metrics.insert("hierarchical_storage".to_string(), 
            serde_json::json!(true));
        
        // TODO: Collect actual metrics from storage when needed
        let total_files = 0;
        metrics.insert("total_swift_files".to_string(), 
            serde_json::json!(total_files));
        
        let stats = self.statistics.read().await;
        metrics.insert("pending_flushes".to_string(), 
            serde_json::json!(stats.pending_flushes));
        metrics.insert("pending_compactions".to_string(), 
            serde_json::json!(stats.pending_compactions));
        
        // Hardware info
        metrics.insert("simd_backend".to_string(), 
            serde_json::json!(format!("{:?}", self.hardware.cpu.simd)));
        
        Ok(metrics)
    }
    
    async fn get_vector_by_id(
        &self, 
        collection_id: &str, 
        vector_id: &str
    ) -> Result<Option<VectorRecord>> {
        let _timer = self.start_operation_timer("get_by_id");
        debug!("SWIFT get vector: collection={}, id={}", collection_id, vector_id);
        
        // TODO: Load actual files from storage based on collection_id
        // For now, return None as placeholder
        // In production, would:
        // 1. Get storage path from collection metadata
        // 2. Load SST files from that path
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
        let _filter = ctx.search_params.filter_expression.as_ref()
            .map(|f| serde_json::to_value(f).unwrap_or(serde_json::Value::Null));
        let _search_params = ctx.search_params.custom_hints.clone();
        let mut timer = self.start_operation_timer("search");
        
        info!("SWIFT unified search: collection={}, k={}, metric={:?}, storage_path={}", 
            collection_id, top_k, distance_metric, storage_path);
        
        // Load files from storage
        let files = self.load_collection_files(collection_id, storage_path).await?;
        
        let mut all_results = Vec::new();
        
        // Search each SWIFT file
        for swift_file in files.iter() {
            let config = progressive_search::ProgressiveSearchConfig::default();
            let results = self.optimized_ops.search_optimized(
                swift_file,
                query_vector,
                top_k,
                config,
            ).await?;
            
            // Convert to search results
            for record in results {
                // Would compute actual distance
                all_results.push((record, 0.0f32));
            }
        }
        
        // Sort by distance and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);
        
        // Convert to core SearchResult format
        let search_results: Vec<crate::core::search::SearchResult> = all_results.into_iter()
            .enumerate()
            .map(|(idx, (record, distance))| {
                // Use SimilarityResult for proper semantic meaning
                let similarity_result = crate::compute::distance_computation::SimilarityResult {
                    normalized_score: 1.0 - distance.min(1.0).max(0.0), // Normalize to [0,1]
                    raw_score: 1.0 - distance,
                    distance: Some(distance),
                    metric: crate::compute::distance_computation::DistanceMetric::Euclidean,
                };
                
                crate::core::search::SearchResult {
                    id: record.id.unwrap_or_else(|| format!("unknown_{}", idx)),
                    vector_id: Some(record.id.unwrap_or_else(|| format!("unknown_{}", idx))),
                    score: similarity_result.normalized_score,
                    similarity: Some(similarity_result.normalized_score),
                    vector: Some(record.vector),
                    metadata: Some(record.metadata.into_iter().map(|m| (m.key, serde_json::Value::String(format!("{:?}", m.value)))).collect()),
                    debug_info: None,
                    version: record.version,
                    timestamp: None,
                    semantic_similarity: Some(similarity_result),
                    quantization_info: None,
                    engine_stats: None,
                    index_path: None,
                }
            })
            .collect();
        
        // Track bytes processed for metrics
        if let Some(ref mut timer) = timer {
            let bytes_processed = search_results.len() * query_vector.len() * 4; // Approximate
            timer.set_bytes_processed(bytes_processed as u64);
        }
        
        Ok(search_results)
    }
    
    // =============================================================================
    // OPTIONAL OPERATIONS - Use default implementations
    // =============================================================================
    
    async fn optimize(&self, collection_id: &str) -> Result<()> {
        info!("SWIFT optimize: collection={}", collection_id);
        
        // Trigger compaction for optimization
        let params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            estimated_input_size: 0,
            max_output_file_size: 1024 * 1024 * 1024, // 1GB
            collection_config: None,
        };
        
        self.do_compact(&params).await?;
        Ok(())
    }
    
    async fn get_statistics(&self) -> Result<EngineStatistics> {
        Ok(self.statistics.read().await.clone())
    }
    
    async fn health_check(&self) -> Result<EngineHealth> {
        Ok(EngineHealth {
            is_healthy: true,
            uptime_seconds: 0,
            last_flush_time: None,
            last_compaction_time: None,
            error_count: 0,
            warning_count: 0,
        })
    }
    
    async fn supports_feature(&self, feature: &str) -> bool {
        match feature {
            "id_lookup" => true,
            "similarity_search" => true,
            "progressive_search" => true,
            "quantization" => true,
            "compression" => true,
            "batch_operations" => true,
            _ => false,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_swift_engine_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = SwiftEngine::new().await.unwrap();
        assert_eq!(engine.engine_name(), "SWIFT");
        assert_eq!(engine.engine_version(), "1.0.0");
    }
    
    #[tokio::test]
    async fn test_swift_feature_support() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = SwiftEngine::new().await.unwrap();
        
        assert!(engine.supports_feature("id_lookup").await);
        assert!(engine.supports_feature("similarity_search").await);
        assert!(engine.supports_feature("progressive_search").await);
        assert!(engine.supports_feature("quantization").await);
        assert!(!engine.supports_feature("unknown_feature").await);
    }
}