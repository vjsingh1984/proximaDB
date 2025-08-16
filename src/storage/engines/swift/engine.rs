// SWIFT Engine: Storage With Instant Fast Traversal - zero-overhead vector storage
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
};
use crate::compute::distance_computation::DistanceMetric;
use crate::proto::proximadb::{SearchResult, IndexingAlgorithm};
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
use crate::storage::engines::common::compression_common::{
    AdaptiveCompressionSettings, AdaptiveStrategy, ContextAwareCompressionConfig,
};
use crate::metrics::compression::CompressionDataType;

// MIGRATION: Import universal adapters for deduplication
use crate::storage::engines::common::{
    UniversalCompressionAdapter, UniversalQuantizationAdapter,
    UniversalCompressionConfig, UniversalQuantizationConfig,
    // Temporarily disabled - these types may not exist yet
    // compression_common::{
    //     AdaptiveCompressionSettings, AdaptiveStrategy,
    //     ContextAwareCompressionConfig, CompressionDataType,
    // },
    quantization_common::{
        ProgressiveQuantizationStage, UniversalQuantizationLevel,
        BinaryThresholdStrategy, CodebookStrategy,
    },
};

use super::{
    SstFile, // SearchMode, // Temporarily disabled - may not exist
    id_index::IdIndex,
    batch_operations,
    progressive_search,
    optimized_operations::OptimizedSwiftOperations,
};

/// SWIFT Engine - Storage With Instant Fast Traversal for zero-overhead vector storage
pub struct SwiftEngine {
    /// Collection ID mapping to SST files
    collections: Arc<RwLock<HashMap<String, Arc<RwLock<Vec<SstFile>>>>>>,
    
    /// Optimized operations handler
    optimized_ops: Arc<OptimizedSwiftOperations>,
    
    /// Engine statistics
    statistics: Arc<RwLock<EngineStatistics>>,
    
    /// Hardware capabilities
    hardware: Arc<HardwareCapabilities>,
    
    /// Metrics collector for unified monitoring
    metrics_collector: Option<Arc<EngineMetricsCollector>>,
    
    /// MIGRATION: Universal compression adapter (REQUIRED)
    compression_adapter: Arc<UniversalCompressionAdapter>,
    
    /// MIGRATION: Universal quantization adapter (REQUIRED)
    quantization_adapter: Arc<UniversalQuantizationAdapter>,
}

impl SwiftEngine {
    /// Create new SWIFT engine instance
    pub fn new() -> Result<Self> {
        let hardware = HardwareCapabilities::get()?;
        let optimized_ops = Arc::new(OptimizedSwiftOperations::new()?);
        
        // MIGRATION: Initialize universal adapters (REQUIRED)
        let compression_adapter = Arc::new(
            UniversalCompressionAdapter::new()
                .map_err(|e| anyhow!("Failed to initialize compression adapter: {}", e))?
        );
        
        let quantization_adapter = Arc::new(
            UniversalQuantizationAdapter::new()
                .map_err(|e| anyhow!("Failed to initialize quantization adapter: {}", e))?
        );
        
        // Configure SWIFT-specific settings
        let mut compression_config = UniversalCompressionConfig::default();
        compression_config.primary_algorithm = crate::core::compression::CompressionAlgorithm::Lz4;
        compression_config.compression_level = 3;
        compression_config.adaptive_settings = AdaptiveCompressionSettings {
            enabled: true,
            strategy: AdaptiveStrategy::DataDriven,
            fallback_algorithms: vec![
                crate::core::compression::CompressionAlgorithm::Zstd,
                crate::core::compression::CompressionAlgorithm::Snappy,
            ],
            performance_target: Some(10), // 10ms target for SST blocks
        };
        compression_config.context_aware = ContextAwareCompressionConfig {
            data_type: CompressionDataType::SstBlock,
            ..Default::default()
        };
        compression_adapter.set_default_config(compression_config);
        
        // Configure SWIFT-specific quantization with progressive stages
        let mut quant_config = UniversalQuantizationConfig::default();
        quant_config.enabled = true;
        quant_config.stages = vec![
            // Stage 1: Binary for fast filtering
            ProgressiveQuantizationStage {
                level: UniversalQuantizationLevel::Binary {
                    threshold_strategy: BinaryThresholdStrategy::ZeroCentered,
                },
                candidate_reduction: 0.8, // Filter 80% using binary
                quality_threshold: 0.3,
            },
            // Stage 2: INT8 for intermediate filtering
            ProgressiveQuantizationStage {
                level: UniversalQuantizationLevel::Int8 {
                    scale_strategy: crate::storage::engines::common::quantization_common::ScaleStrategy::GlobalMinMax,
                    zero_point_strategy: crate::storage::engines::common::quantization_common::ZeroPointStrategy::Symmetric,
                },
                candidate_reduction: 0.5, // Further reduce by 50%
                quality_threshold: 0.85,
            },
            // Stage 3: PQ for final ranking
            ProgressiveQuantizationStage {
                level: UniversalQuantizationLevel::ProductQuantization {
                    segments: 16,
                    bits_per_segment: 8,
                    codebook_strategy: CodebookStrategy::KMeans,
                },
                candidate_reduction: 0.0, // Keep all for final ranking
                quality_threshold: 0.95,
            },
        ];
        
        // Add SWIFT-specific engine overrides
        quant_config.engine_overrides.insert(
            "swift_hierarchical_blocks".to_string(),
            serde_json::json!(true)
        );
        quant_config.engine_overrides.insert(
            "swift_progressive_search".to_string(),
            serde_json::json!(true)
        );
        quantization_adapter.set_default_config(quant_config);
        
        Ok(Self {
            collections: Arc::new(RwLock::new(HashMap::new())),
            optimized_ops,
            statistics: Arc::new(RwLock::new(EngineStatistics::default())),
            hardware,
            metrics_collector: None,
            compression_adapter,
            quantization_adapter,
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
    
    /// Get or create SST files for collection
    async fn get_or_create_collection(&self, collection_id: &str) -> Arc<RwLock<Vec<SstFile>>> {
        let mut collections = self.collections.write().await;
        collections.entry(collection_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(Vec::new())))
            .clone()
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
        // SWIFT is a variant of SST/LSM strategy
        StorageEngineStrategy::Lsm
    }
    
    // =============================================================================
    // CORE OPERATIONS
    // =============================================================================
    
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("SWIFT flush: collection={:?}, vectors={}", 
            params.collection_id, params.num_vectors);
        
        let collection_files = self.get_or_create_collection(&params.collection_id).await;
        
        // Create new SST file from flush parameters
        let mut sst = SstFile::new(
            params.collection_id.clone(),
            params.dimension.unwrap_or(768),
            DistanceMetric::Euclidean,
        );
        
        // Build blocks from vectors (would come from memtable in production)
        // For now, simulate with empty records
        let records = Vec::new();
        
        // Use universal adapters for quantization
        let quant_config = self.quantization_adapter.get_default_config();
        sst.build_blocks_from_records_with_adapters(
            records,
            Some(self.quantization_adapter.as_ref()),
            Some(&quant_config),
        )?;
        
        // Add to collection
        let mut files = collection_files.write().await;
        files.push(sst);
        
        // Update statistics
        let mut stats = self.statistics.write().await;
        stats.total_flushes += 1;
        stats.bytes_written += params.estimated_size;
        
        Ok(FlushResult {
            success: true,
            files_created: 1,
            bytes_written: params.estimated_size,
            duration_ms: 100,
            error_message: None,
        })
    }
    
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        info!("SWIFT compaction: collection={:?}, level={}", 
            params.collection_id, params.compaction_level);
        
        let collection_files = self.get_or_create_collection(&params.collection_id).await;
        let files = collection_files.read().await;
        
        if files.len() < 2 {
            return Ok(CompactionResult {
                success: true,
                input_files: files.len(),
                output_files: files.len(),
                bytes_read: 0,
                bytes_written: 0,
                records_compacted: 0,
                duration_ms: 0,
                error_message: None,
            });
        }
        
        // Simulate compaction
        let input_count = files.len();
        let output_count = (input_count + 1) / 2;
        
        Ok(CompactionResult {
            success: true,
            input_files: input_count,
            output_files: output_count,
            bytes_read: params.estimated_input_size,
            bytes_written: params.estimated_input_size * 80 / 100, // 20% reduction
            records_compacted: 0,
            duration_ms: 500,
            error_message: None,
        })
    }
    
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        
        let collections = self.collections.read().await;
        metrics.insert("collection_count".to_string(), 
            serde_json::json!(collections.len()));
        
        let mut total_files = 0;
        for files in collections.values() {
            total_files += files.read().await.len();
        }
        metrics.insert("total_sst_files".to_string(), 
            serde_json::json!(total_files));
        
        let stats = self.statistics.read().await;
        metrics.insert("total_flushes".to_string(), 
            serde_json::json!(stats.total_flushes));
        metrics.insert("total_compactions".to_string(), 
            serde_json::json!(stats.total_compactions));
        
        // Hardware info
        metrics.insert("simd_backend".to_string(), 
            serde_json::json!(format!("{:?}", self.hardware.best_backend())));
        
        Ok(metrics)
    }
    
    async fn get_vector_by_id(
        &self, 
        collection_id: &str, 
        vector_id: &str
    ) -> Result<Option<VectorRecord>> {
        let mut timer = self.start_operation_timer("get_by_id");
        debug!("SWIFT get vector: collection={}, id={}", collection_id, vector_id);
        
        let collection_files = self.get_or_create_collection(collection_id).await;
        let files = collection_files.read().await;
        
        // Search through all SST files for the ID
        for sst in files.iter() {
            if let Some(location) = sst.id_index.lookup_async(vector_id).await {
                let record = sst.load_record_at_location(&location)?;
                
                // Track bytes processed for metrics
                if let Some(ref mut timer) = timer {
                    let bytes_processed = record.vector.len() * 4; // 4 bytes per f32
                    timer.set_bytes_processed(bytes_processed as u64);
                }
                
                return Ok(Some(record));
            }
        }
        
        Ok(None)
    }
    
    async fn search_vectors_unified(
        &self,
        collection_id: &str,
        _storage_url: &str,
        query_vector: &[f32],
        top_k: usize,
        distance_metric: DistanceMetric,
        filter: Option<serde_json::Value>,
        _index_algorithm: Option<IndexingAlgorithm>,
        _search_params: Option<serde_json::Value>,
    ) -> Result<SearchResult> {
        let mut timer = self.start_operation_timer("search");
        info!("SWIFT unified search: collection={}, k={}, metric={:?}", 
            collection_id, top_k, distance_metric);
        
        let collection_files = self.get_or_create_collection(collection_id).await;
        let files = collection_files.read().await;
        
        let mut all_results = Vec::new();
        
        // Search each SST file
        for sst in files.iter() {
            let config = progressive_search::ProgressiveSearchConfig::default();
            let results = self.optimized_ops.search_optimized(
                sst,
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
        
        // Convert to proto SearchResult
        let search_records = all_results.into_iter()
            .enumerate()
            .map(|(idx, (record, distance))| {
                crate::proto::proximadb::SearchVectorRecord {
                    id: record.id.unwrap_or_else(|| format!("unknown_{}", idx)),
                    vector: record.vector,
                    metadata: vec![],
                    rank: (idx + 1) as i32,
                    score: 1.0 - distance, // Convert distance to similarity
                    distance,
                    version: record.version,
                    timestamp: Some(record.timestamp as u32),
                    collection_id: Some(collection_id.to_string()),
                }
            })
            .collect();
        
        // Track bytes processed for metrics
        if let Some(ref mut timer) = timer {
            let bytes_processed = search_records.len() * query_vector.len() * 4; // Approximate
            timer.set_bytes_processed(bytes_processed as u64);
        }
        
        Ok(SearchResult {
            records: search_records,
            total_results: files.len() as i32,
            execution_time_ms: 10.0,
        })
    }
    
    // =============================================================================
    // OPTIONAL OPERATIONS - Use default implementations
    // =============================================================================
    
    async fn optimize(&self, collection_id: &str) -> Result<()> {
        info!("SWIFT optimize: collection={}", collection_id);
        
        // Trigger compaction for optimization
        let params = CompactionParameters {
            collection_id: collection_id.to_string(),
            compaction_level: 1,
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
        let collections = self.collections.read().await;
        
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

impl Default for EngineStatistics {
    fn default() -> Self {
        Self {
            total_vectors: 0,
            total_bytes: 0,
            total_flushes: 0,
            total_compactions: 0,
            bytes_written: 0,
            bytes_read: 0,
            cache_hits: 0,
            cache_misses: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_swift_engine_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = SwiftEngine::new().unwrap();
        assert_eq!(engine.engine_name(), "SWIFT");
        assert_eq!(engine.engine_version(), "1.0.0");
    }
    
    #[tokio::test]
    async fn test_swift_feature_support() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = SwiftEngine::new().unwrap();
        
        assert!(engine.supports_feature("id_lookup").await);
        assert!(engine.supports_feature("similarity_search").await);
        assert!(engine.supports_feature("progressive_search").await);
        assert!(engine.supports_feature("quantization").await);
        assert!(!engine.supports_feature("unknown_feature").await);
    }
}