//! PRISM Engine Implementation with Universal Adapter Integration
//! Progressive Retrieval through Indexed Storage Management

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use serde::{Serialize, Deserialize};
use uuid::Uuid;
use tracing::{debug, info, warn};

use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy, FlushParameters, FlushResult,
    CompactionParameters, CompactionResult,
};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::services::collection_service::CollectionService;
use crate::storage::engines::universal::{
    UniversalDistanceAdapter, DistanceComputationRequest, StorageFormat, EngineType
};
use crate::storage::engines::CandidateVector;
use crate::compute::distance_computation::DistanceMetric;
use crate::core::VectorRecord;

/// Configuration for PRISM engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub base_dir: String,
    pub storage_url: String,
    pub memory_cache_size_mb: usize,
    pub compression: bool,
    pub enable_progressive_quantization: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_dir: "/tmp/prism".to_string(),
            storage_url: "s3://prism-bucket".to_string(),
            memory_cache_size_mb: 3072,
            compression: true,
            enable_progressive_quantization: true,
        }
    }
}

/// PRISM Engine - Memory-First Progressive Retrieval Storage Engine with Universal Adapter
pub struct PrismEngine {
    config: Arc<Config>,
    filesystem_factory: Arc<FilesystemFactory>,
    universal_adapter: Option<Arc<UniversalDistanceAdapter>>,
    /// Unified quantization engine from compute module (optional - only if quantization is enabled)
    quantization_engine: Option<Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>>,
}

impl PrismEngine {
    /// Create a new PRISM engine (async initialization)
    pub async fn new(config: Config) -> Result<Self> {
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem_factory = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        
        // Initialize quantization engine if enabled in config
        let quantization_engine = if config.enable_progressive_quantization {
            // Initialize unified quantization engine from compute module
            let distance_compute = Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
            let codebook_store = Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
            let unified_engine = Arc::new(crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ));
            
            // Configure storage quantization for PRISM (memory-first engine)
            let storage_config = crate::compute::quantization::storage_engine::StorageQuantizationConfig {
                primary_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(16)),
                filter_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::binary()),
                fast_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::int8()),
                distance_metric: crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                enable_progressive: true,
                filter_threshold: 100.0,
                candidate_multiplier: 10,
                training_sample_size: 10000,
                memory_budget_mb: config.memory_cache_size_mb,
                enable_hardware_acceleration: true,
            };
            
            Some(Arc::new(crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                storage_config,
            )))
        } else {
            None
        };
        
        Ok(Self {
            config: Arc::new(config),
            filesystem_factory,
            universal_adapter: None,
            quantization_engine,
        })
    }
    
    /// Create a new PRISM engine with universal adapter integration
    pub async fn new_with_universal_adapter(config: Config) -> Result<Self> {
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem_factory = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        
        // Initialize universal adapter for PRISM engine
        let universal_adapter = UniversalDistanceAdapter::new().await
            .map_err(|e| anyhow!("Failed to initialize universal adapter: {}", e))?;
        
        Ok(Self {
            config: Arc::new(config),
            filesystem_factory,
            universal_adapter: Some(Arc::new(universal_adapter)),
        })
    }
    
    /// Perform vector search using universal adapter
    pub async fn search_with_universal_adapter(
        &self,
        collection_id: Uuid,
        query_vector: Vec<f32>,
        distance_metric: DistanceMetric,
        max_results: usize,
        storage_format: Option<StorageFormat>,
    ) -> Result<Vec<(Uuid, f32)>> {
        let adapter = self.universal_adapter.as_ref()
            .ok_or_else(|| anyhow!("Universal adapter not initialized. Use new_with_universal_adapter()"))?;
        
        // In a real implementation, this would load candidate vectors from PRISM storage
        // For now, create dummy candidates
        let candidates = self.load_candidate_vectors(collection_id).await?;
        
        let request = DistanceComputationRequest {
            query_vector,
            candidates,
            distance_metric,
            storage_format: storage_format.unwrap_or(StorageFormat::QuantizedPQ { segments: 8, bits: 8 }),
            refinement_config: None,
            max_results,
            enable_acceleration: true,
            // quality_threshold removed -  Some(0.85),
            collection_id,
            engine_type: EngineType::PRISM,
        };
        
        let result = adapter.compute_progressive_distance(request).await
            .map_err(|e| anyhow!("Universal adapter search failed: {}", e))?;
        
        // Convert results to expected format
        let search_results = result.vector_ids.into_iter()
            .zip(result.results.into_iter())
            .map(|(id, sim_result)| (id, sim_result.rank_value))
            .collect();
        
        Ok(search_results)
    }
    
    /// Load candidate vectors from PRISM storage (placeholder implementation)
    async fn load_candidate_vectors(&self, _collection_id: Uuid) -> Result<Vec<CandidateVector>> {
        use crate::storage::engines::universal::CandidateVector;
        
        // Placeholder implementation - in practice would load from PRISM storage
        let mut candidates = Vec::new();
        for i in 0..1000 {
            candidates.push(CandidateVector {
                id: Uuid::new_v4(),
                data: (0..512).map(|j| ((i + j) % 256) as u8).collect(), // 128 dimensions as FP32
                original_vector: Some((0..128).map(|j| (i + j) as f32 * 0.01).collect()),
                metadata: Some(HashMap::new()),
                quality_score: Some(0.8 + (i as f32 * 0.0001)),
            });
        }
        Ok(candidates)
    }
    
    /// Get optimal storage format for given parameters
    pub async fn get_optimal_storage_format(
        &self,
        vector_dimension: usize,
        dataset_size: usize,
        target_recall: f32,
    ) -> Result<StorageFormat> {
        if let Some(adapter) = &self.universal_adapter {
            adapter.get_optimal_format(&EngineType::PRISM, vector_dimension, dataset_size, target_recall).await
                .map_err(|e| anyhow!("Failed to get optimal format: {}", e))
        } else {
            // Default PRISM format selection
            Ok(if dataset_size > 1_000_000 && target_recall <= 0.9 {
                StorageFormat::QuantizedPQ { segments: 8, bits: 8 }
            } else if target_recall > 0.95 {
                StorageFormat::FP32
            } else {
                StorageFormat::QuantizedINT8 { scale: 1.0, zero_point: 0 }
            })
        }
    }
}

#[async_trait]
impl UnifiedStorageEngine for PrismEngine {
    fn engine_name(&self) -> &'static str {
        "PRISM"
    }

    fn engine_version(&self) -> &'static str {
        "0.1.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Prism
    }

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for flush"))?;
        let start_time = std::time::Instant::now();
        
        info!("PRISM flush: collection={}, vectors={}", collection_id, params.vector_records.len());
        
        // PRISM is memory-first, so flush to in-memory cache
        // TODO: Implement actual memory cache storage
        let bytes_written = params.vector_records.len() * params.dimension.unwrap_or(768) * 4;
        
        Ok(FlushResult {
            success: true,
            files_created: 0, // Memory-first, no files
            bytes_written: bytes_written as u64,
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }

    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for compaction"))?;
        let start_time = std::time::Instant::now();
        
        info!("PRISM compaction: collection={}, level={}", collection_id, params.compaction_level);
        
        // PRISM uses memory-first approach, compaction reorganizes in-memory structures
        // TODO: Implement actual memory reorganization
        
        Ok(CompactionResult {
            success: true,
            input_files: 0, // Memory-based, no files
            output_files: 0,
            bytes_read: params.estimated_input_size,
            bytes_written: (params.estimated_input_size * 90) / 100, // 10% reduction
            records_compacted: 0,
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        metrics.insert("engine_name".to_string(), serde_json::Value::String("PRISM".to_string()));
        metrics.insert("engine_type".to_string(), serde_json::Value::String("memory_first".to_string()));
        metrics.insert("memory_cache_size_mb".to_string(), serde_json::json!(self.config.memory_cache_size_mb));
        metrics.insert("progressive_quantization".to_string(), serde_json::json!(self.config.enable_progressive_quantization));
        metrics.insert("healthy".to_string(), serde_json::Value::Bool(true));
        Ok(metrics)
    }

    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<crate::core::VectorRecord>> {
        debug!("PRISM get vector: collection={}, id={}", collection_id, vector_id);
        
        // TODO: Implement actual lookup from memory cache
        // For now, return None as placeholder
        // In production, would:
        // 1. Check memory cache first
        // 2. Fall back to storage if not in cache
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
        let performance_tier = ctx.performance_tier();
        
        info!("PRISM search: collection={}, k={}, metric={:?}, tier={:?}", 
            collection_id, top_k, distance_metric, performance_tier);
        
        // PRISM uses progressive retrieval with memory-first approach
        // TODO: Implement actual search logic
        // For now, return empty results
        Ok(vec![])
    }

    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        &self.filesystem_factory
    }

    fn get_collection_service(&self) -> Option<&CollectionService> {
        None
    }

    async fn get_collection_storage_url(&self, collection_id: &str) -> Result<String> {
        Ok(format!("{}/collections/{}", self.config.storage_url, collection_id))
    }

    async fn get_base_storage_url(&self, _collection_id: &str) -> Result<String> {
        Ok(self.config.storage_url.clone())
    }
}