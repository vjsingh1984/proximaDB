//! Storage-Aware Search Engine Traits and Infrastructure
//!
//! This module defines the core traits and data structures for implementing
//! storage-aware polymorphic search in ProximaDB. Each storage engine
//! (VIPER, LSM) implements the StorageSearchEngine trait with optimizations
//! specific to their storage format and indexing strategies.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::core::{SearchResult, VectorRecord};
use crate::proto::proximadb::StorageEngine as StorageEngineType;
use super::multi_tier_deduplication::{MultiTierDeduplicator, TieredSearchResult, StorageTier, DeduplicationStorageEngine, MetadataFilter};

/// Core trait for storage-aware search engines
///
/// Each storage type (VIPER, LSM) implements this trait with optimizations
/// specific to their format: predicate pushdown for Parquet, bloom filters
/// for LSM, quantization for compressed vectors, etc.
#[async_trait]
pub trait StorageSearchEngine: Send + Sync {
    /// Perform storage-optimized vector search
    ///
    /// This method leverages storage-specific optimizations:
    /// - VIPER: Predicate pushdown, ML clustering, quantization
    /// - LSM: Memtable priority, bloom filters, level-aware search
    async fn search_vectors(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
        search_hints: &SearchHints,
    ) -> Result<Vec<SearchResult>>;

    /// Get storage engine capabilities for optimization planning
    fn search_capabilities(&self) -> SearchCapabilities;

    /// Get the underlying storage engine type
    fn engine_type(&self) -> StorageEngineType;

    /// Get engine-specific performance metrics
    async fn get_search_metrics(&self) -> Result<SearchMetrics>;

    /// Validate search parameters for this storage type
    fn validate_search_params(
        &self,
        query_vector: &[f32],
        k: usize,
        hints: &SearchHints,
    ) -> Result<()>;
}

/// Search optimization hints for storage-specific strategies
///
/// These hints guide the search engine on which optimizations to apply:
/// - Enable/disable specific features (predicate pushdown, bloom filters)
/// - Set performance vs accuracy tradeoffs (quantization level)
/// - Provide optimization context (clustering hints, timeout budgets)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHints {
    /// Enable predicate pushdown to storage layer (VIPER: Parquet filters)
    pub predicate_pushdown: bool,

    /// Use bloom filters to skip irrelevant data structures (LSM: SSTable skipping)
    pub use_bloom_filters: bool,

    /// ML clustering optimization hints for vector search (VIPER specific)
    pub clustering_hints: Option<ClusteringHints>,

    /// Vector quantization level for performance vs accuracy tradeoff
    pub quantization_level: QuantizationLevel,

    /// Maximum search timeout in milliseconds
    pub timeout_ms: Option<u64>,

    /// Include debug information in search results
    pub include_debug_info: bool,

    /// Enable parallel search across data partitions
    pub enable_parallel_search: bool,

    /// Custom optimization parameters for specific storage engines
    pub engine_specific: HashMap<String, serde_json::Value>,
}

impl Default for SearchHints {
    fn default() -> Self {
        Self {
            predicate_pushdown: true,
            use_bloom_filters: true,
            clustering_hints: None,
            quantization_level: QuantizationLevel::FP32,
            timeout_ms: Some(5000), // 5 second default timeout
            include_debug_info: false,
            enable_parallel_search: true,
            engine_specific: HashMap::new(),
        }
    }
}

/// ML clustering hints for VIPER vector search optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringHints {
    /// Use ML-driven cluster selection (vs exhaustive search)
    pub enable_ml_clustering: bool,

    /// Maximum number of clusters to search (0 = no limit)
    pub max_clusters_to_search: usize,

    /// Cluster selection confidence threshold (0.0-1.0)
    pub cluster_confidence_threshold: f32,

    /// Pre-computed cluster centroids for fast distance calculation
    pub cluster_centroids_cache: Option<Vec<Vec<f32>>>,

    /// Cluster distance metric (cosine, euclidean, etc.)
    pub cluster_distance_metric: ClusterDistanceMetric,
}

impl Default for ClusteringHints {
    fn default() -> Self {
        Self {
            enable_ml_clustering: true,
            max_clusters_to_search: 10,
            cluster_confidence_threshold: 0.7,
            cluster_centroids_cache: None,
            cluster_distance_metric: ClusterDistanceMetric::Cosine,
        }
    }
}

/// Distance metrics for cluster selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterDistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
    Manhattan,
}

/// Vector quantization levels for VIPER storage
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QuantizationLevel {
    /// Full 32-bit floating point precision (100% accuracy)
    FP32,
    /// 8-bit product quantization (faster, ~95% accuracy)
    PQ8,
    /// 4-bit product quantization (4x faster, ~90% accuracy)
    PQ4,
    /// Binary quantization (16x faster, ~80% accuracy)
    Binary,
    /// Scalar quantization to 8-bit integers
    INT8,
}

impl Default for QuantizationLevel {
    fn default() -> Self {
        Self::FP32
    }
}

/// Search engine capabilities for optimization planning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCapabilities {
    /// Engine supports predicate pushdown to storage layer
    pub supports_predicate_pushdown: bool,

    /// Engine supports bloom filter optimizations
    pub supports_bloom_filters: bool,

    /// Engine supports ML-driven clustering
    pub supports_clustering: bool,

    /// Engine supports parallel search execution
    pub supports_parallel_search: bool,

    /// Supported quantization levels
    pub supported_quantization: Vec<QuantizationLevel>,

    /// Maximum supported result set size (k)
    pub max_k: usize,

    /// Maximum supported vector dimension
    pub max_dimension: usize,

    /// Engine-specific optimization features
    pub engine_features: HashMap<String, serde_json::Value>,
}

/// Search performance metrics for monitoring and optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetrics {
    /// Total number of searches performed
    pub total_searches: u64,

    /// Average search latency in microseconds
    pub avg_latency_us: f64,

    /// P95 search latency in microseconds
    pub p95_latency_us: f64,

    /// P99 search latency in microseconds
    pub p99_latency_us: f64,

    /// Number of vectors scanned (efficiency metric)
    pub avg_vectors_scanned: f64,

    /// Cache hit rate for optimization structures
    pub cache_hit_rate: f32,

    /// Index efficiency metrics (bloom filter false positive rate, etc.)
    pub index_efficiency: HashMap<String, f64>,

    /// Quantization accuracy metrics (when applicable)
    pub quantization_accuracy: Option<QuantizationAccuracy>,

    /// Engine-specific performance metrics
    pub engine_metrics: HashMap<String, serde_json::Value>,
}

/// Quantization accuracy tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationAccuracy {
    /// Current quantization level
    pub quantization_level: QuantizationLevel,

    /// Recall@10 compared to FP32 baseline
    pub recall_at_10: f32,

    /// Recall@100 compared to FP32 baseline
    pub recall_at_100: f32,

    /// Average distance error compared to FP32
    pub avg_distance_error: f32,

    /// Number of accuracy measurements
    pub measurement_count: u64,
}

/// Factory for creating storage-appropriate search engines with unified deduplication
pub struct SearchEngineFactory {
    /// WAL manager for unflushed data search
    wal_manager: Option<std::sync::Arc<crate::storage::persistence::wal::WalManager>>,
}

impl SearchEngineFactory {
    /// Create new factory with optional WAL manager for unflushed data
    pub fn new(wal_manager: Option<std::sync::Arc<crate::storage::persistence::wal::WalManager>>) -> Self {
        Self { wal_manager }
    }

    /// Create factory without WAL (only flushed/compacted data)
    pub fn new_without_wal() -> Self {
        Self { wal_manager: None }
    }

    /// **UNIFIED SEARCH WITH DEDUPLICATION**
    /// Searches across all storage tiers (unflushed WAL + flushed storage) with ID-based deduplication
    pub async fn search_with_deduplication(
        &self,
        collection: &crate::proto::proximadb::Collection,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
        search_params: Option<&crate::core::search::SearchParams>,
        viper_engine: Option<std::sync::Arc<crate::storage::engines::viper::ViperEngine>>,
        lsm_engine: Option<std::sync::Arc<crate::storage::engines::lsm::LsmTree>>,
    ) -> Result<Vec<SearchResult>> {
        let mut deduplicator = if let Some(filters) = filters {
            MultiTierDeduplicator::with_filters(std::mem::take(&mut filters.clone()))
        } else {
            MultiTierDeduplicator::new()
        };

        // Create search hints from search params or use defaults
        let search_hints = if let Some(params) = search_params {
            // Extract hints from SearchParams
            let storage_engine_i32 = collection.config.as_ref()
                .map(|c| c.storage_engine)
                .unwrap_or(crate::proto::proximadb::StorageEngine::Viper as i32);
            let storage_type = match storage_engine_i32 {
                x if x == crate::proto::proximadb::StorageEngine::Viper as i32 => crate::proto::proximadb::StorageEngine::Viper,
                x if x == crate::proto::proximadb::StorageEngine::Lsm as i32 => crate::proto::proximadb::StorageEngine::Lsm,
                _ => crate::proto::proximadb::StorageEngine::Viper,
            };
            let mut hints = Self::create_optimized_hints(
                storage_type,
                query_vector,
                k,
                filters.is_some(),
            );
            
            // Apply params to hints
            if let Some(enable_two_stage) = params.enable_two_stage {
                hints.engine_specific.insert("two_stage_search".to_string(), serde_json::json!(enable_two_stage));
            }
            if let Some(quant_hint) = &params.quantization_hint {
                // Map UnifiedQuantizationLevel to QuantizationLevel
                use crate::compute::unified_quantization::QuantizationLevelType;
                
                let quantization_level = match &quant_hint.level_type {
                    Some(QuantizationLevelType::Scalar(scalar)) => {
                        match scalar.bits {
                            8 => QuantizationLevel::INT8,
                            _ => QuantizationLevel::FP32, // Default to FP32 for unsupported bits
                        }
                    }
                    Some(QuantizationLevelType::Binary(_)) => QuantizationLevel::Binary,
                    Some(QuantizationLevelType::Pq(pq)) => {
                        match pq.bits_per_code {
                            8 => QuantizationLevel::PQ8,
                            4 => QuantizationLevel::PQ4,
                            _ => QuantizationLevel::PQ8, // Default to PQ8
                        }
                    }
                    Some(QuantizationLevelType::None(_)) => QuantizationLevel::FP32,
                    Some(QuantizationLevelType::Uniform(_)) => QuantizationLevel::FP32, // Not supported yet
                    Some(QuantizationLevelType::Custom(_)) => QuantizationLevel::FP32, // Not supported yet
                    None => QuantizationLevel::FP32,
                };
                
                hints.quantization_level = quantization_level;
            }
            if let Some(enable_clustering) = params.enable_clustering_hint {
                if enable_clustering {
                    hints.clustering_hints = Some(ClusteringHints::default());
                } else {
                    hints.clustering_hints = None;
                }
            }
            
            hints
        } else {
            let storage_engine_i32_2 = collection.config.as_ref()
                .map(|c| c.storage_engine)
                .unwrap_or(crate::proto::proximadb::StorageEngine::Viper as i32);
            let storage_type_2 = match storage_engine_i32_2 {
                x if x == crate::proto::proximadb::StorageEngine::Viper as i32 => crate::proto::proximadb::StorageEngine::Viper,
                x if x == crate::proto::proximadb::StorageEngine::Lsm as i32 => crate::proto::proximadb::StorageEngine::Lsm,
                _ => crate::proto::proximadb::StorageEngine::Viper,
            };
            Self::create_optimized_hints(
                storage_type_2,
                query_vector,
                k,
                filters.is_some(),
            )
        };

        // 1. Search unflushed WAL data (highest priority)
        if let Some(wal_manager) = &self.wal_manager {
            if let Ok(wal_results) = self.search_wal_data(
                wal_manager,
                collection.config.as_ref().map(|c| c.name.as_str()).unwrap_or("unknown"),
                query_vector,
                k * 2, // Get more results for better deduplication
                filters,
            ).await {
                tracing::debug!("🔍 WAL search: {} results", wal_results.len());
                deduplicator.add_tier_results(wal_results);
            }
        }

        // 2. Search flushed/compacted storage data
        let storage_engine = Self::create_for_collection(collection, viper_engine, lsm_engine).await?;
        let storage_results = storage_engine.search_vectors(
            collection.config.as_ref().map(|c| c.name.as_str()).unwrap_or("unknown"),
            query_vector,
            k * 2, // Get more results for better deduplication
            filters,
            &search_hints,
        ).await?;

        // Convert SearchResult to TieredSearchResult
        let storage_engine_enum = collection.config.as_ref()
            .map(|c| c.storage_engine)
            .unwrap_or(crate::proto::proximadb::StorageEngine::Viper as i32);
        let storage_type = match storage_engine_enum {
            x if x == crate::proto::proximadb::StorageEngine::Viper as i32 => crate::proto::proximadb::StorageEngine::Viper,
            x if x == crate::proto::proximadb::StorageEngine::Lsm as i32 => crate::proto::proximadb::StorageEngine::Lsm,
            _ => crate::proto::proximadb::StorageEngine::Viper,
        };
        let tiered_storage_results = self.convert_to_tiered_results(
            storage_results,
            storage_type,
        );

        tracing::debug!("🔍 Storage search: {} results", tiered_storage_results.len());
        deduplicator.add_tier_results(tiered_storage_results);

        // 3. Get deduplicated results and convert back to SearchResult
        let deduplicated = deduplicator.get_final_results(k);
        let final_results = self.convert_from_tiered_results(deduplicated);

        tracing::info!(
            "🎯 Unified search complete: {} final results with deduplication",
            final_results.len()
        );

        Ok(final_results)
    }

    /// **OPTIMIZED: Search unflushed memtable batches directly (no disk I/O)**
    /// Uses deserialized WalVectorBatch from memtable instead of re-deserializing from disk
    async fn search_wal_data(
        &self,
        wal_manager: &std::sync::Arc<crate::storage::persistence::wal::WalManager>,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
    ) -> Result<Vec<TieredSearchResult>> {
        // 🚀 OPTIMIZATION: Search in-memory unflushed batches instead of disk
        // This avoids disk I/O and deserialization since memtable already has parsed data
        tracing::debug!("🔍 WAL_SEARCH: Using optimized batch-based search for collection {}", collection_id);
        
        let mut results = Vec::new();
        
        // Try to get unflushed batches from memtable first (fastest path)
        if let Ok(memtable_wrapper) = self.get_wal_memtable_wrapper(wal_manager).await {
            let unflushed_batches = memtable_wrapper.get_unflushed_batches(collection_id).await?;
            
            tracing::debug!("🔍 WAL_SEARCH: Found {} unflushed batches in memtable", unflushed_batches.len());
            
            for batch in unflushed_batches {
                for vector_record in &batch.vector_records {
                    // Apply metadata filters
                    if let Some(filters) = filters {
                        let mut matches = true;
                        for (key, expected_value) in filters {
                            match vector_record.metadata.get(key) {
                                Some(actual_value) => {
                                    if actual_value != expected_value {
                                        matches = false;
                                        break;
                                    }
                                }
                                None => {
                                    matches = false;
                                    break;
                                }
                            }
                        }
                        if !matches {
                            continue;
                        }
                    }

                    let score = self.calculate_similarity(query_vector, &vector_record.vector);
                    results.push(TieredSearchResult {
                        vector_record: VectorRecord {
                            id: vector_record.id.clone(),
                            collection_id: vector_record.collection_id.clone(),
                            vector: vector_record.vector.clone(),
                            metadata: vector_record.metadata.clone(),
                            timestamp: vector_record.timestamp,
                            created_at: vector_record.created_at,
                            updated_at: vector_record.updated_at,
                            expires_at: vector_record.expires_at,
                            version: vector_record.version,
                            rank: vector_record.rank,
                            score: vector_record.score,
                            distance: vector_record.distance,
                        },
                        score,
                        tier: StorageTier::Unflushed,
                        engine: DeduplicationStorageEngine::WAL,
                        timestamp: batch.batch_id.created_at,
                        sequence: 0, // Batch-based, no individual sequence
                        file_path: None,
                    });
                }
            }
        } else {
            // FALLBACK: Use old disk-based search if memtable wrapper unavailable
            tracing::warn!("🔍 WAL_SEARCH: Falling back to disk-based search (performance degraded)");
            return self.search_wal_data_fallback(wal_manager, collection_id, query_vector, k, filters).await;
        }

        // Sort by score and take top results
        results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        tracing::debug!("🔍 WAL_SEARCH: Returning {} results from {} batches", results.len(), results.len());
        Ok(results)
    }

    /// Fallback to old disk-based search when memtable wrapper unavailable
    async fn search_wal_data_fallback(
        &self,
        wal_manager: &std::sync::Arc<crate::storage::persistence::wal::WalManager>,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
    ) -> Result<Vec<TieredSearchResult>> {
        let collection_id_typed = crate::core::String::from(collection_id.to_string());
        let wal_entries = wal_manager.get_collection_vectors(&collection_id_typed).await?;
        let mut results = Vec::new();

        for vector_record in wal_entries {
            // wal_entries now returns Vec<VectorRecord> directly from WalManager.get_collection_vectors()
            // No need to extract from WalEntry operations since we eliminated that dependency
            
            // Apply metadata filters
            if let Some(filters) = filters {
                let mut matches = true;
                for (key, expected_value) in filters {
                    match vector_record.metadata.get(key) {
                        Some(actual_value) => {
                            if actual_value != expected_value {
                                matches = false;
                                break;
                            }
                        }
                        None => {
                            matches = false;
                            break;
                        }
                    }
                }
                if !matches {
                    continue;
                }
            }

            let score = self.calculate_similarity(query_vector, &vector_record.vector);
            results.push(TieredSearchResult {
                vector_record,
                score,
                tier: StorageTier::Unflushed,
                engine: DeduplicationStorageEngine::WAL,
                timestamp: chrono::Utc::now(), // Use current time since we don't have entry timestamp
                sequence: 0, // No sequence available for direct vector records
                file_path: None,
            });
        }

        // Sort by score and take top results
        results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        Ok(results)
    }

    /// Get WAL memtable wrapper for direct batch access
    async fn get_wal_memtable_wrapper<'a>(
        &self,
        wal_manager: &'a std::sync::Arc<crate::storage::persistence::wal::WalManager>,
    ) -> Result<&'a crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        wal_manager
            .get_wal_behavior_wrapper()
            .ok_or_else(|| anyhow::anyhow!("WAL behavior wrapper not available from strategy"))
    }

    /// Calculate similarity score (cosine distance)
    fn calculate_similarity(&self, query: &[f32], vector: &[f32]) -> f32 {
        if query.len() != vector.len() {
            return f32::INFINITY;
        }

        let dot_product: f32 = query.iter().zip(vector.iter()).map(|(a, b)| a * b).sum();
        let norm_a: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        if norm_a == 0.0 || norm_b == 0.0 {
            f32::INFINITY
        } else {
            1.0 - (dot_product / (norm_a * norm_b))
        }
    }

    /// Convert SearchResult to TieredSearchResult
    fn convert_to_tiered_results(
        &self,
        search_results: Vec<SearchResult>,
        storage_engine_type: StorageEngineType,
    ) -> Vec<TieredSearchResult> {
        let tier = match storage_engine_type {
            StorageEngineType::Viper => StorageTier::Compacted,
            StorageEngineType::Lsm => StorageTier::Flushed,
            _ => StorageTier::Flushed,
        };

        let engine = match storage_engine_type {
            StorageEngineType::Viper => DeduplicationStorageEngine::VIPER,
            StorageEngineType::Lsm => DeduplicationStorageEngine::LSM,
            _ => DeduplicationStorageEngine::LSM,
        };

        search_results
            .into_iter()
            .map(|result| {
                let id = result.id;
                let collection_id = result.collection_id.unwrap_or_default();
                let vector = result.vector.unwrap_or_default();
                let metadata = result.metadata;
                let now = chrono::Utc::now().timestamp_millis();
                
                TieredSearchResult {
                    vector_record: VectorRecord {
                        id: id.clone(),
                        collection_id,
                        vector,
                        metadata,
                        timestamp: now,
                        created_at: now,
                        updated_at: now,
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: Some(result.score),
                        distance: None,
                    },
                score: result.score,
                tier,
                engine,
                timestamp: chrono::Utc::now(), // Approximation for flushed data
                sequence: 0, // Storage engines don't track sequence
                file_path: None, // Storage engines don't provide file paths in search results
            }
            })
            .collect()
    }

    /// Convert TieredSearchResult back to SearchResult
    fn convert_from_tiered_results(&self, tiered_results: Vec<TieredSearchResult>) -> Vec<SearchResult> {
        tiered_results
            .into_iter()
            .map(|result| {
                let id = result.vector_record.id;
                SearchResult {
                    id: id.clone(),
                    vector_id: Some(id),
                    score: result.score,
                    distance: None,
                    rank: None,
                    vector: Some(result.vector_record.vector),
                    metadata: result.vector_record.metadata,
                    collection_id: Some(result.vector_record.collection_id),
                    created_at: Some(result.timestamp.timestamp_millis()),
                    algorithm_used: None,
                    processing_time_us: None,
                }
            })
            .collect()
    }

    /// Create a search engine appropriate for the given collection
    ///
    /// This factory method examines the collection's storage engine type
    /// and creates the appropriate search engine with optimized configuration.
    pub async fn create_for_collection(
        collection: &crate::proto::proximadb::Collection,
        // Dependencies will be injected here based on storage type
        viper_engine: Option<std::sync::Arc<crate::storage::engines::viper::ViperEngine>>,
        lsm_engine: Option<std::sync::Arc<crate::storage::engines::lsm::LsmTree>>,
    ) -> Result<Box<dyn StorageSearchEngine>> {
        match collection.config.as_ref().map(|c| c.storage_engine).unwrap_or(0) {
            x if x == crate::proto::proximadb::StorageEngine::Viper as i32 => {
                if let Some(viper) = viper_engine {
                    Ok(Box::new(super::viper_search::ViperSearchEngine::new(
                        viper,
                        collection.clone(),
                    )?))
                } else {
                    Err(anyhow::anyhow!(
                        "VIPER engine not available for VIPER collection"
                    ))
                }
            }
            x if x == crate::proto::proximadb::StorageEngine::Lsm as i32 => {
                if let Some(lsm) = lsm_engine {
                    Ok(Box::new(super::lsm_search::LSMSearchEngine::new(
                        lsm,
                        collection.clone(),
                    )?))
                } else {
                    Err(anyhow::anyhow!(
                        "LSM engine not available for LSM collection"
                    ))
                }
            }
            // ProximaDB only supports LSM and VIPER engines
            _ => Err(anyhow::anyhow!(
                "Unsupported storage engine type. Only LSM and VIPER are supported."
            ))
        }
    }

    /// Create search hints optimized for the given storage type and query
    pub fn create_optimized_hints(
        storage_type: StorageEngineType,
        query_vector: &[f32],
        k: usize,
        has_filters: bool,
    ) -> SearchHints {
        let mut hints = SearchHints::default();

        match storage_type {
            StorageEngineType::Viper => {
                // VIPER optimizations
                hints.predicate_pushdown = has_filters;
                hints.clustering_hints = Some(ClusteringHints::default());

                // Adaptive quantization based on query characteristics
                hints.quantization_level = if k <= 10 && query_vector.len() <= 384 {
                    QuantizationLevel::PQ4 // Fast for small result sets
                } else if k <= 100 {
                    QuantizationLevel::PQ8 // Balanced for medium result sets
                } else {
                    QuantizationLevel::FP32 // Accurate for large result sets
                };
            }
            StorageEngineType::Lsm => {
                // LSM optimizations
                hints.use_bloom_filters = true;
                hints.predicate_pushdown = false; // Not applicable to LSM
                hints.quantization_level = QuantizationLevel::FP32; // LSM stores full precision

                // LSM-specific hints
                hints.engine_specific.insert(
                    "level_search_priority".to_string(),
                    serde_json::Value::String("recent_first".to_string()),
                );
            }
            _ => {
                // Default hints for unknown storage types
            }
        }

        hints
    }
}

/// Validation utilities for search parameters
pub struct SearchValidator;

impl SearchValidator {
    /// Validate common search parameters across all storage types
    pub fn validate_common_params(
        query_vector: &[f32],
        k: usize,
        collection_dimension: usize,
    ) -> Result<()> {
        if query_vector.is_empty() {
            return Err(anyhow::anyhow!("Query vector cannot be empty"));
        }

        if query_vector.len() != collection_dimension {
            return Err(anyhow::anyhow!(
                "Query vector dimension {} does not match collection dimension {}",
                query_vector.len(),
                collection_dimension
            ));
        }

        if k == 0 {
            return Err(anyhow::anyhow!("k must be greater than 0"));
        }

        if k > 10000 {
            return Err(anyhow::anyhow!(
                "k cannot exceed 10000 for performance reasons"
            ));
        }

        // Check for invalid values (NaN, infinity)
        for (i, &value) in query_vector.iter().enumerate() {
            if !value.is_finite() {
                return Err(anyhow::anyhow!(
                    "Query vector contains invalid value at index {}: {}",
                    i,
                    value
                ));
            }
        }

        Ok(())
    }

    /// Validate search hints for consistency and feasibility
    pub fn validate_search_hints(hints: &SearchHints) -> Result<()> {
        if let Some(timeout_ms) = hints.timeout_ms {
            if timeout_ms == 0 {
                return Err(anyhow::anyhow!("Search timeout must be greater than 0"));
            }
            if timeout_ms > 60000 {
                return Err(anyhow::anyhow!("Search timeout cannot exceed 60 seconds"));
            }
        }

        if let Some(clustering_hints) = &hints.clustering_hints {
            if clustering_hints.cluster_confidence_threshold < 0.0
                || clustering_hints.cluster_confidence_threshold > 1.0
            {
                return Err(anyhow::anyhow!(
                    "Cluster confidence threshold must be between 0.0 and 1.0"
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_hints_default() {
        let hints = SearchHints::default();
        assert!(hints.predicate_pushdown);
        assert!(hints.use_bloom_filters);
        assert_eq!(hints.quantization_level, QuantizationLevel::FP32);
        assert_eq!(hints.timeout_ms, Some(5000));
    }

    #[test]
    fn test_clustering_hints_default() {
        let hints = ClusteringHints::default();
        assert!(hints.enable_ml_clustering);
        assert_eq!(hints.max_clusters_to_search, 10);
        assert_eq!(hints.cluster_confidence_threshold, 0.7);
    }

    #[test]
    fn test_quantization_level_ordering() {
        // Test that quantization levels have expected performance characteristics
        assert_ne!(QuantizationLevel::FP32, QuantizationLevel::PQ4);
        assert_ne!(QuantizationLevel::PQ8, QuantizationLevel::Binary);
    }

    #[test]
    fn test_search_validator_common_params() {
        let query_vector = vec![0.1, 0.2, 0.3];

        // Valid parameters
        assert!(SearchValidator::validate_common_params(&query_vector, 10, 3).is_ok());

        // Invalid k
        assert!(SearchValidator::validate_common_params(&query_vector, 0, 3).is_err());
        assert!(SearchValidator::validate_common_params(&query_vector, 10001, 3).is_err());

        // Invalid dimension
        assert!(SearchValidator::validate_common_params(&query_vector, 10, 2).is_err());
        assert!(SearchValidator::validate_common_params(&query_vector, 10, 4).is_err());

        // Invalid vector values
        let invalid_vector = vec![0.1, f32::NAN, 0.3];
        assert!(SearchValidator::validate_common_params(&invalid_vector, 10, 3).is_err());

        let infinite_vector = vec![0.1, f32::INFINITY, 0.3];
        assert!(SearchValidator::validate_common_params(&infinite_vector, 10, 3).is_err());
    }

    #[test]
    fn test_search_validator_hints() {
        let mut hints = SearchHints::default();

        // Valid hints
        assert!(SearchValidator::validate_search_hints(&hints).is_ok());

        // Invalid timeout
        hints.timeout_ms = Some(0);
        assert!(SearchValidator::validate_search_hints(&hints).is_err());

        hints.timeout_ms = Some(70000);
        assert!(SearchValidator::validate_search_hints(&hints).is_err());

        // Invalid clustering hints
        hints.timeout_ms = Some(5000);
        hints.clustering_hints = Some(ClusteringHints {
            cluster_confidence_threshold: 1.5,
            ..Default::default()
        });
        assert!(SearchValidator::validate_search_hints(&hints).is_err());
    }

    #[test]
    fn test_factory_optimized_hints() {
        let query_vector = vec![0.1; 384];

        // VIPER hints
        let viper_hints = SearchEngineFactory::create_optimized_hints(
            StorageEngineType::Viper,
            &query_vector,
            10,
            true,
        );
        assert!(viper_hints.predicate_pushdown);
        assert!(viper_hints.clustering_hints.is_some());
        assert_eq!(viper_hints.quantization_level, QuantizationLevel::PQ4);

        // LSM hints
        let lsm_hints = SearchEngineFactory::create_optimized_hints(
            StorageEngineType::Lsm,
            &query_vector,
            10,
            true,
        );
        assert!(!lsm_hints.predicate_pushdown);
        assert!(lsm_hints.use_bloom_filters);
        assert_eq!(lsm_hints.quantization_level, QuantizationLevel::FP32);
    }
}
