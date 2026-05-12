// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # AXIS Index Manager - Adaptive eXperimental Index System
//!
//! This module implements the central coordination layer for ProximaDB's adaptive
//! indexing system. AXIS dynamically selects and migrates between different index
//! types based on workload patterns, data characteristics, and query requirements.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │            AXIS Manager                      │
//! ├─────────────────────────────────────────────┤
//! │  Monitoring  →  Analysis  →  Adaptation     │
//! │      ↓            ↓            ↓            │
//! │  Metrics    Workload      Migration         │
//! │  Collection  Patterns      Engine           │
//! ├─────────────────────────────────────────────┤
//! │        Index Selection Strategy             │
//! │    ┌──────┬──────┬──────┬──────┐          │
//! │    │ HNSW │ IVF  │ LSH  │ Flat │          │
//! │    └──────┴──────┴──────┴──────┘          │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! ### 1. **Adaptive Index Selection**
//! - Monitors query patterns and data distribution
//! - Automatically selects optimal index type
//! - Seamless migration between index types
//!
//! ### 2. **Performance Monitoring**
//! - Real-time tracking of index performance
//! - Query latency and throughput metrics
//! - Resource utilization monitoring
//!
//! ### 3. **Index Migration Engine**
//! - Zero-downtime index migrations
//! - Incremental index building
//! - Rollback capabilities
//!
//! ### 4. **Clustering Engine**
//! - Automatic data clustering for IVF indexes
//! - Centroid optimization
//! - Balanced cluster distribution
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::index::axis::AxisManager;
//! use proximadb::index::axis::AxisConfig;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create AXIS manager with adaptive configuration
//! let config = AxisConfig {
//!     enable_adaptive_indexing: true,
//!     migration_threshold: 0.8,
//!     monitoring_interval_ms: 5000,
//!     ..Default::default()
//! };
//!
//! let axis_manager = AxisManager::new(config).await?;
//!
//! // AXIS automatically adapts to workload patterns
//! axis_manager.start_monitoring().await?;
//!
//! // Manual strategy override if needed
//! axis_manager.set_collection_strategy(
//!     "high_traffic_collection",
//!     IndexSelectionStrategy::Hnsw
//! ).await?;
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error};

use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::{String, VectorId, VectorRecord};
use crate::index::axis::management::{
    migration_engine::{IndexMigrationEngine, MigrationDecision},
    monitor::PerformanceMonitor,
};
use crate::index::axis::{
    clustering::AxisClusteringEngine,
    clustering::ClusteringConfig,
    hmgi::{
        DetectionResult, HmgiPartitionKey, HmgiRegistry, HmgiRouter, ModalityDetector,
        ModalityExtractor, VectorRecordSample,
    },
    management::adaptive_engine::AdaptiveIndexEngine,
    types::{AxisConfig, Data, IndexAlgorithm, IndexSelectionStrategy},
};
use crate::index::{DenseVectorIndex, GlobalIdIndex, JoinEngine, MetadataIndex, SparseVectorIndex};
// Temporarily disabled due to arrow-arith compilation conflicts - DEFERRED: Re-enable when resolved
// use crate::storage::engines::viper::QuantizationMethod;

/// Central manager for AXIS with adaptive capabilities
///
/// The `AxisManager` coordinates all indexing operations in ProximaDB, providing
/// a unified interface for vector indexing with automatic adaptation based on
/// workload characteristics.
///
/// # Components
///
/// - **Core Indexes**: Global ID, metadata, dense/sparse vector indexes
/// - **Adaptive Engine**: Monitors and adapts to workload patterns
/// - **Migration Engine**: Handles index type transitions
/// - **Performance Monitor**: Tracks metrics and performance
/// - **Clustering Engine**: Manages IVF clustering operations
///
/// # Thread Safety
///
/// All operations are thread-safe through internal synchronization using
/// `Arc<RwLock>` for shared state and `DashMap` for concurrent collections.
///
/// ## Index Selection Strategy:
///
/// ```text
/// Dataset Size    Dimensions    QPS       → Recommended Index
/// ------------------------------------------------------------
/// < 10K          Any           Any       → Flat (exact)
/// 10K-100K       < 100         < 1000    → HNSW
/// 10K-100K       > 100         < 1000    → IVF + PQ
/// 100K-1M        < 200         < 5000    → HNSW + PQ
/// 100K-1M        > 200         Any       → IVF + PQ
/// > 1M           Any           < 1000    → IVF + PQ
/// > 1M           Any           > 1000    → LSH
/// ```
///
/// ## Migration Triggers:
///
/// AXIS automatically triggers migration when:
/// - Query latency degrades by >20%
/// - Memory usage exceeds threshold
/// - Dataset size crosses boundaries (10K, 100K, 1M)
/// - Query pattern changes significantly
///
/// ## EventLog Integration:
///
/// AXIS receives index update events from the EventLog:
/// ```text
/// Storage Flush → EventLog → AXIS Consumer
///                              ↓
///                        Index Update
///                              ↓
///                        Background Build
/// ```
pub struct AxisManager {
    /// Core index components for different data types

    /// Global ID index for fast ID-based lookups
    /// Maps vector IDs to storage locations across all collections
    global_id_index: Arc<GlobalIdIndex>,

    /// Metadata index for filtered search
    /// Supports range queries, equality, and complex predicates
    metadata_index: Arc<MetadataIndex>,

    /// Dense vector index for similarity search
    /// Supports HNSW, IVF, LSH, Annoy, PQ, Flat algorithms
    dense_vector_index: Arc<DenseVectorIndex>,

    /// Sparse vector index for keyword/document search
    /// Optimized for high-dimensional sparse vectors
    sparse_vector_index: Arc<SparseVectorIndex>,

    /// Join engine for hybrid queries
    /// Combines results from multiple indexes
    #[allow(dead_code)]
    join_engine: Arc<JoinEngine>,

    /// Adaptive intelligence components for workload optimization

    /// Monitors workload patterns and triggers adaptations
    /// Analyzes query distribution, data growth, and access patterns
    adaptive_engine: Arc<AdaptiveIndexEngine>,

    /// Handles zero-downtime index migrations
    /// Builds new index in background, validates, then switches atomically
    migration_engine: Arc<IndexMigrationEngine>,

    /// Tracks performance metrics and anomalies
    /// Monitors latency, throughput, accuracy, and resource usage
    #[allow(dead_code)]
    performance_monitor: Arc<PerformanceMonitor>,

    /// Manages clustering for IVF indexes
    /// Performs k-means clustering and centroid optimization
    #[allow(dead_code)]
    clustering_engine: Arc<AxisClusteringEngine>,

    /// Collection-specific configurations
    /// Maps collection_id → selected index strategy
    /// Can be manually overridden or automatically determined
    collection_strategies: Arc<RwLock<HashMap<String, IndexSelectionStrategy>>>,

    /// Active migrations
    /// Tracks ongoing index migrations for monitoring and rollback
    active_migrations: Arc<RwLock<HashMap<String, MigrationStatus>>>,

    /// Configuration and metrics
    /// Global AXIS configuration (thresholds, intervals, etc.)
    #[allow(dead_code)]
    config: AxisConfig,

    /// Aggregated metrics across all managed indexes
    metrics: Arc<RwLock<AxisMetrics>>,

    /// Collection service for IndexConfig retrieval
    /// Provides access to collection metadata and index configurations
    /// Set via set_collection_service() after initialization
    collection_service: Option<Arc<crate::services::collection::manager::CollectionService>>,

    /// Shared collection cache from VectorOperationsService (read-only access)
    /// This avoids duplicating collection metadata in memory
    /// Collections are cached by VectorOperationsService and shared here
    /// for fast access during index operations
    shared_collection_cache:
        Option<Arc<dashmap::DashMap<String, Arc<crate::proto::proximadb_v1::Collection>>>>,

    /// Real HNSW indexes per collection for vector similarity search
    /// Maps collection_id → AxisHnswIndex instance
    /// These are the actual in-memory HNSW indexes that store and query vectors
    /// NOTE: HNSW has poor recall with incremental indexing - prefer IVF for production
    hnsw_indexes:
        Arc<RwLock<HashMap<String, Arc<crate::index::axis::indexes::hnsw_index::AxisHnswIndex>>>>,

    /// Real IVF indexes per collection for vector similarity search (DEFAULT)
    /// Maps collection_id → UnifiedIvfIndex instance
    /// IVF is better suited for incremental workloads as new vectors are simply
    /// assigned to their nearest cluster without degrading graph quality
    ivf_indexes: Arc<
        RwLock<
            HashMap<
                String,
                Arc<tokio::sync::RwLock<crate::index::axis::indexes::ivf_unified::UnifiedIvfIndex>>,
            >,
        >,
    >,

    /// Pending vectors buffer for IVF training
    /// IVF requires k-means training before vectors can be added
    /// Vectors are buffered here until we have enough for training (min_train_size)
    ivf_pending_vectors: Arc<RwLock<HashMap<String, Vec<(String, Vec<f32>)>>>>,

    /// Exact vector records tracked per collection for correctness-first filtered search.
    /// This is the source of truth for metadata-aware fallback execution and MVCC timestamps.
    collection_vectors: Arc<RwLock<HashMap<String, HashMap<String, VectorRecord>>>>,

    /// HMGI - Hierarchical Multi-modality Graph Indexing components
    /// Registry for managing per-modality HNSW partitions
    hmgi_registry: Option<Arc<HmgiRegistry>>,

    /// Router for directing queries to relevant modality partitions
    hmgi_router: Option<Arc<HmgiRouter>>,

    /// Modality extractor for determining vector modality from metadata
    hmgi_extractor: Option<Arc<ModalityExtractor>>,

    /// Modality detector for auto-enabling HMGI on multi-modality collections
    hmgi_detector: Option<Arc<ModalityDetector>>,

    /// Collections with HMGI enabled
    hmgi_enabled_collections: Arc<RwLock<std::collections::HashSet<String>>>,

    /// Collection OID lookup for HMGI partition key generation
    /// Maps collection_id → oid for partition key creation
    hmgi_collection_oids: Arc<RwLock<HashMap<String, u64>>>,
}

/// Status of ongoing migrations
///
/// ## Migration Lifecycle:
///
/// 1. **Triggered**: Migration decision made
/// 2. **Building**: New index being constructed (0-90%)
/// 3. **Validating**: Comparing accuracy (90-95%)
/// 4. **Switching**: Atomic switchover (95-99%)
/// 5. **Cleanup**: Old index removal (100%)
///
/// Progress tracking enables:
/// - User visibility into long-running migrations
/// - Cancellation/rollback capabilities
/// - Resource planning (CPU/memory allocation)
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    /// Unique identifier for tracking
    pub migration_id: crate::utils::uuid::Uuid,

    /// Source index strategy
    pub from_strategy: IndexSelectionStrategy,

    /// Target index strategy
    pub to_strategy: IndexSelectionStrategy,

    /// When migration started
    pub start_time: DateTime<Utc>,

    /// Current progress (0.0 to 100.0)
    pub progress_percentage: f64,

    /// Estimated completion based on current rate
    pub estimated_completion: Option<DateTime<Utc>>,
}

/// AXIS metrics
///
/// ## Key Metrics:
///
/// - **Migration Success Rate**: successful/total migrations
/// - **Average Migration Time**: Indicates system adaptation speed
/// - **Rebuild Frequency**: High rebuilds may indicate instability
/// - **Vector Growth Rate**: Helps predict future resource needs
///
/// These metrics feed into:
/// - Adaptive decision making
/// - Capacity planning
/// - Performance monitoring dashboards
#[derive(Debug, Clone, Default)]
pub struct AxisMetrics {
    /// Total migration attempts
    pub total_migrations: u64,

    /// Successfully completed migrations
    pub successful_migrations: u64,

    /// Failed migrations (rolled back)
    pub failed_migrations: u64,

    /// Average time to complete migration
    pub average_migration_time_ms: u64,

    /// Number of collections under management
    pub total_collections_managed: u64,

    /// Total vectors across all indexes
    pub total_vectors_indexed: u64,

    /// Full index rebuilds (usually after corruption)
    pub total_rebuilds: u64,
}

impl AxisManager {
    /// Create a new AXIS index manager
    pub async fn new(config: AxisConfig) -> Result<Self> {
        // Initialize core index components
        let global_id_index = Arc::new(GlobalIdIndex::new().await?);
        let metadata_index = Arc::new(MetadataIndex::new().await?);
        let dense_vector_index = Arc::new(DenseVectorIndex::new().await?);
        let sparse_vector_index = Arc::new(SparseVectorIndex::new().await?);
        let join_engine = Arc::new(JoinEngine::new().await?);

        // Initialize adaptive components
        let adaptive_engine = Arc::new(AdaptiveIndexEngine::new(config.clone()).await?);
        let migration_engine = Arc::new(IndexMigrationEngine::new(config.clone()).await?);
        let performance_monitor = Arc::new(PerformanceMonitor::new(config.clone()).await?);

        // Initialize clustering engine with default config
        let clustering_config = ClusteringConfig::default();
        let clustering_engine = Arc::new(AxisClusteringEngine::new(clustering_config));

        // Initialize HMGI components with the default modality field. Collections are still
        // opted in only after explicit enablement or auto-detection of multiple modalities.
        let hmgi_registry = Arc::new(HmgiRegistry::new());
        let hmgi_extractor = Arc::new(ModalityExtractor::new());
        let hmgi_detector = Arc::new(ModalityDetector::default_config());
        let hmgi_router = Arc::new(HmgiRouter::new(
            hmgi_registry.clone(),
            hmgi_extractor.clone(),
        ));

        Ok(Self {
            global_id_index,
            metadata_index,
            dense_vector_index,
            sparse_vector_index,
            join_engine,
            adaptive_engine,
            migration_engine,
            performance_monitor,
            clustering_engine,
            collection_strategies: Arc::new(RwLock::new(HashMap::new())),
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
            config,
            metrics: Arc::new(RwLock::new(AxisMetrics::default())),
            collection_service: None, // Will be set later via set_collection_service
            shared_collection_cache: None, // Will be set via set_shared_collection_cache
            hnsw_indexes: Arc::new(RwLock::new(HashMap::new())), // Real HNSW indexes per collection
            ivf_indexes: Arc::new(RwLock::new(HashMap::new())), // Real IVF indexes per collection (DEFAULT)
            ivf_pending_vectors: Arc::new(RwLock::new(HashMap::new())), // Buffer for IVF training
            collection_vectors: Arc::new(RwLock::new(HashMap::new())),
            // HMGI components initialized up front; collections are enabled on demand.
            hmgi_registry: Some(hmgi_registry),
            hmgi_router: Some(hmgi_router),
            hmgi_extractor: Some(hmgi_extractor),
            hmgi_detector: Some(hmgi_detector),
            hmgi_enabled_collections: Arc::new(RwLock::new(std::collections::HashSet::new())),
            hmgi_collection_oids: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Set the collection service for IndexConfig retrieval
    pub fn set_collection_service(
        &mut self,
        collection_service: Arc<crate::services::collection::manager::CollectionService>,
    ) {
        self.collection_service = Some(collection_service);
        tracing::info!("🔗 AXIS: Collection service set for IndexConfig retrieval");
    }

    /// Set shared collection cache from VectorOperationsService
    pub fn set_shared_collection_cache(
        &mut self,
        cache: Arc<dashmap::DashMap<String, Arc<crate::proto::proximadb_v1::Collection>>>,
    ) {
        self.shared_collection_cache = Some(cache);
        tracing::info!("🔗 AXIS: Shared collection cache set for read-only access");
    }

    /// Get collection's IndexConfig from collection service for index build decisions
    pub async fn get_native_index_config(
        &self,
        collection_id: &str,
    ) -> Result<crate::index::config::IndexConfig> {
        if let Some(collection_service) = &self.collection_service {
            match collection_service.native_index_config(collection_id).await {
                Ok(Some(config)) => {
                    tracing::debug!(
                        "📋 AXIS: Retrieved IndexConfig for collection: {}",
                        collection_id
                    );
                    Ok(config)
                }
                Ok(None) => {
                    tracing::warn!(
                        "⚠️ AXIS: Collection not found for IndexConfig: {}",
                        collection_id
                    );
                    // Return default IndexConfig as fallback
                    Ok(crate::index::config::IndexConfig::default())
                }
                Err(e) => {
                    tracing::error!(
                        "❌ AXIS: Failed to retrieve IndexConfig for collection {}: {}",
                        collection_id,
                        e
                    );
                    // Return default IndexConfig as fallback
                    Ok(crate::index::config::IndexConfig::default())
                }
            }
        } else {
            tracing::warn!("⚠️ AXIS: Collection service not available, using default IndexConfig");
            // Default implementation: return default IndexConfig
            Ok(crate::index::config::IndexConfig::default())
        }
    }

    /// Insert a vector with adaptive indexing and quantization support
    pub async fn insert(&self, collection_id: &str, vector: &VectorRecord) -> Result<()> {
        // Ensure we have a search_strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        // Check if vector is expired (MVCC support) - direct field access
        if let Some(expires_at) = vector.expires_at
            && expires_at <= Utc::now().timestamp()
        {
            // Skip inserting already expired vectors
            return Ok(());
        }

        // Get collection config for quantization settings
        // First try shared cache, then fall back to collection service
        let collection = if let Some(cache) = &self.shared_collection_cache {
            cache.get(collection_id).map(|r| r.clone())
        } else if let Some(collection_service) = &self.collection_service {
            collection_service
                .collection(collection_id)
                .await
                .ok()
                .flatten()
                .map(Arc::new)
        } else {
            None
        };

        // Prepare vector for insertion (with potential quantization)
        let processed_vector = if let Some(collection) = &collection {
            // Check if quantization is enabled for this collection
            if let Some(config) = &collection.config {
                if let Some(quant_config) = &config.quantization {
                    if quant_config.enabled.unwrap_or(false) {
                        // Quantize vector for in-memory index using collection settings
                        // This reuses our existing quantization infrastructure
                        self.quantize_for_index(vector, quant_config, config)
                            .await?
                    } else {
                        vector.clone()
                    }
                } else {
                    vector.clone()
                }
            } else {
                vector.clone()
            }
        } else {
            vector.clone()
        };

        if !vector.id.is_empty() {
            let mut collection_vectors = self.collection_vectors.write().await;
            collection_vectors
                .entry(collection_id.to_string())
                .or_default()
                .insert(vector.id.clone(), vector.clone());
        }

        // Insert into appropriate indexes based on current search_strategy
        let search_strategy = self.get_collection_strategy(collection_id).await?;
        let has_dense_vector_index = search_strategy
            .indexes
            .iter()
            .any(|index_spec| matches!(index_spec.data_type, Data::DenseVector { .. }));

        if has_dense_vector_index
            && !processed_vector.id.is_empty()
            && !processed_vector.vector.is_empty()
        {
            self.ensure_hmgi_collection_enabled(collection_id).await?;
        }

        // Insert into global ID index if ID is present
        if !processed_vector.id.is_empty() {
            self.global_id_index
                .insert(
                    processed_vector.id.clone(),
                    collection_id,
                    &processed_vector,
                )
                .await?;
        }

        // Insert into other indexes based on search_strategy
        for index_spec in &search_strategy.indexes {
            match index_spec.data_type {
                Data::Metadata => {
                    self.metadata_index.insert(&processed_vector).await?;
                }
                Data::DenseVector { .. } => {
                    self.insert_dense_vector_index(
                        collection_id,
                        &processed_vector,
                        &index_spec.algorithm,
                    )
                    .await?;
                }
                Data::SparseVector { .. } => {
                    self.sparse_vector_index.insert(&processed_vector).await?;
                }
                _ => {} // Handle other data types
            }
        }

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_vectors_indexed += 1;

        // Check if we should evaluate search_strategy change
        self.maybe_evaluate_strategy(collection_id).await?;

        Ok(())
    }

    /// Delete a vector (soft delete with expires_at)
    pub async fn delete(&self, collection_id: &str, vector_id: VectorId) -> Result<()> {
        // For MVCC, we don't actually delete - we set expires_at to now
        // This is handled by the storage layer creating a tombstone

        // Skip if vector_id is empty
        if vector_id.is_empty() {
            return Ok(());
        }

        // Ensure we have a search_strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        // Remove from indexes
        let search_strategy = self.get_collection_strategy(collection_id).await?;

        {
            let mut collection_vectors = self.collection_vectors.write().await;
            if let Some(vectors) = collection_vectors.get_mut(collection_id) {
                vectors.remove(&vector_id);
            }
            let should_remove_collection = collection_vectors
                .get(collection_id)
                .map(|vectors| vectors.is_empty())
                .unwrap_or(false);
            if should_remove_collection {
                collection_vectors.remove(collection_id);
            }
        }

        self.global_id_index.remove(&vector_id).await?;

        for index_spec in &search_strategy.indexes {
            match index_spec.data_type {
                Data::Metadata => {
                    self.metadata_index.remove(&vector_id).await?;
                }
                Data::DenseVector { .. } => {
                    self.remove_dense_vector_index(collection_id, &vector_id)
                        .await?;
                }
                Data::SparseVector { .. } => {
                    self.sparse_vector_index.remove(&vector_id).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Query vectors using adaptive indexes
    pub async fn query(&self, query: HybridQuery) -> Result<QueryResult> {
        let start = std::time::Instant::now();

        // Execute query using current search_strategy
        let collection_id = &query.collection_id;

        // Ensure we have a search_strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        let search_strategy = self.get_collection_strategy(collection_id).await?;

        let hmgi_query_safe = self.is_hmgi_routable_query(&query);

        // Query using the appropriate index based on strategy algorithm
        // HNSW: O(log N) search - best for search latency
        // IVF: O(√N) search - acceptable if insert-optimized
        let results = if self.is_hmgi_enabled(collection_id).await && hmgi_query_safe {
            self.search_hmgi(collection_id, &query, query.top_k).await?
        } else if !query.metadata_filters.is_empty() || !query.id_filters.is_empty() {
            self.execute_exact_filtered_query(collection_id, &query)
                .await?
        } else {
            // Find the dense vector index spec to determine algorithm
            let use_ivf = search_strategy.indexes.iter().any(|spec| {
                matches!(spec.data_type, Data::DenseVector { .. })
                    && matches!(
                        spec.algorithm,
                        IndexAlgorithm::IVF { .. } | IndexAlgorithm::PQ { .. }
                    )
            });

            if use_ivf {
                self.query_ivf(collection_id, &query).await?
            } else {
                self.query_hnsw(collection_id, &query).await?
            }
        };

        // Filter out expired results (MVCC)
        let active_results: Vec<_> = results
            .into_iter()
            .filter(|result| {
                if query.include_expired {
                    return true;
                }
                // Check if result is not expired
                if let Some(expires_at) = result.expires_at.as_ref() {
                    Utc::now() < *expires_at
                } else {
                    true // No expiration
                }
            })
            .collect();

        Ok(QueryResult {
            results: active_results,
            strategy_used: search_strategy,
            execution_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Insert dense vectors through the canonical HMGI path.
    ///
    /// Legacy collection-scoped HNSW/IVF remains as a fallback for collections
    /// that have not been initialized for HMGI, but normal AXIS dense indexing is
    /// per-modality so insert and query semantics match.
    async fn insert_dense_vector_index(
        &self,
        collection_id: &str,
        vector: &VectorRecord,
        algorithm: &IndexAlgorithm,
    ) -> Result<()> {
        if self.is_hmgi_enabled(collection_id).await {
            self.insert_hmgi(collection_id, vector.clone()).await?;
            return Ok(());
        }

        match algorithm {
            IndexAlgorithm::HNSW { .. } => self.insert_into_hnsw(collection_id, vector).await,
            IndexAlgorithm::IVF { .. } | IndexAlgorithm::PQ { .. } => {
                self.insert_into_ivf(collection_id, vector).await
            }
            _ => self.insert_into_hnsw(collection_id, vector).await,
        }
    }

    /// Remove dense vectors through the same layout used for dense inserts.
    async fn remove_dense_vector_index(
        &self,
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<()> {
        if self.is_hmgi_enabled(collection_id).await {
            self.remove_hmgi_vector(collection_id, vector_id).await?;
            return Ok(());
        }

        use crate::index::axis::index_factory::AxisVectorIndex;

        if let Some(index) = self.hnsw_indexes.read().await.get(collection_id).cloned() {
            index.remove(vector_id).await?;
        }

        if let Some(index) = self.ivf_indexes.read().await.get(collection_id).cloned() {
            index.read().await.remove(vector_id).await?;
        }

        {
            let mut pending = self.ivf_pending_vectors.write().await;
            if let Some(vectors) = pending.get_mut(collection_id) {
                vectors.retain(|(id, _)| id != vector_id);
            }
        }

        self.dense_vector_index.remove(vector_id).await
    }

    /// Insert a vector into the real HNSW index for a collection
    async fn insert_into_hnsw(&self, collection_id: &str, vector: &VectorRecord) -> Result<()> {
        use crate::compute::distance_computation::DistanceMetric;
        use crate::index::axis::index_factory::AxisVectorIndex;
        use crate::index::axis::indexes::hnsw_index::{AxisHnswConfig, AxisHnswIndex};

        // Get or create HNSW index for this collection
        let dimension = vector.vector.len();
        if dimension == 0 || vector.id.is_empty() {
            return Ok(()); // Skip empty vectors or missing IDs
        }

        // Check if index exists, if not create it
        {
            let indexes = self.hnsw_indexes.read().await;
            if !indexes.contains_key(collection_id) {
                drop(indexes);

                // Get collection's distance metric from its config
                // This ensures HNSW uses the same metric as the collection
                let distance_metric = self
                    .get_collection_distance_metric(collection_id)
                    .await
                    .unwrap_or(DistanceMetric::DotProduct); // Default to DotProduct for compatibility with FAISS/benchmarks

                // Create HNSW config with collection's distance metric
                let config = AxisHnswConfig {
                    distance_metric,
                    ..Default::default()
                };

                tracing::info!(
                    "🔗 AXIS: Creating HNSW index for collection {} with metric {:?}",
                    collection_id,
                    distance_metric
                );

                let index = AxisHnswIndex::new_with_collection(
                    Some(collection_id.to_string()),
                    config,
                    dimension,
                )?;
                let mut indexes = self.hnsw_indexes.write().await;
                indexes.insert(collection_id.to_string(), Arc::new(index));
                tracing::debug!(
                    "🔗 AXIS: Created new HNSW index for collection {} (dimension={}, metric={:?})",
                    collection_id,
                    dimension,
                    distance_metric
                );
            }
        }

        // Insert into the index using the AxisVectorIndex trait
        let indexes = self.hnsw_indexes.read().await;
        if let Some(index) = indexes.get(collection_id) {
            index.add(vector.id.clone(), vector.vector.clone()).await?;
        }

        Ok(())
    }

    /// Query vectors from the real HNSW index
    async fn query_hnsw(
        &self,
        collection_id: &str,
        query: &HybridQuery,
    ) -> Result<Vec<ScoredResult>> {
        use crate::index::axis::index_factory::AxisVectorIndex;

        let indexes = self.hnsw_indexes.read().await;
        if let Some(index) = indexes.get(collection_id) {
            // Extract query vector
            if let Some(VectorQuery::Dense { vector, .. }) = &query.vector_query {
                let results = index.search(vector, query.top_k, None).await?;
                return Ok(results
                    .into_iter()
                    .map(|(id, score)| {
                        let expires_at = self.lookup_record_expiration(collection_id, &id);
                        ScoredResult {
                            vector_id: id,
                            similarity: score,
                            expires_at,
                        }
                    })
                    .collect());
            }
        }

        // Return empty if no index or no dense vector query
        Ok(Vec::new())
    }

    /// Insert a vector into the IVF index for a collection (DEFAULT for incremental workloads)
    /// Insert vector into IVF index (DEFAULT)
    ///
    /// IVF requires k-means training before vectors can be added:
    /// 1. Buffer vectors until we have min_train_size (100 vectors)
    /// 2. Train index with buffered vectors to build centroids
    /// 3. Add all buffered vectors to trained index
    /// 4. Future inserts go directly to trained index
    async fn insert_into_ivf(&self, collection_id: &str, vector: &VectorRecord) -> Result<()> {
        use crate::compute::distance_computation::DistanceMetric;
        use crate::index::axis::indexes::ivf_unified::{UnifiedIvfConfig, UnifiedIvfIndex};

        let dimension = vector.vector.len();
        if dimension == 0 || vector.id.is_empty() {
            return Ok(()); // Skip empty vectors or missing IDs
        }

        const MIN_TRAIN_SIZE: usize = 100; // Minimum vectors needed for k-means training

        // Check if index exists and is trained
        let index_exists_and_trained = {
            let indexes = self.ivf_indexes.read().await;
            if let Some(index) = indexes.get(collection_id) {
                let idx = index.read().await;
                idx.is_trained()
            } else {
                false
            }
        };

        if index_exists_and_trained {
            // Index is trained, add vector directly
            let indexes = self.ivf_indexes.read().await;
            if let Some(index) = indexes.get(collection_id) {
                let idx = index.read().await;
                idx.add_vector(vector.id.clone(), vector.vector.clone(), None)
                    .await?;
            }
            return Ok(());
        }

        // Buffer the vector for training
        {
            let mut pending = self.ivf_pending_vectors.write().await;
            let buffer = pending.entry(collection_id.to_string()).or_default();
            buffer.push((vector.id.clone(), vector.vector.clone()));

            // Check if we have enough vectors to train
            if buffer.len() >= MIN_TRAIN_SIZE {
                tracing::info!(
                    "🎯 AXIS: IVF training triggered for collection {} with {} vectors",
                    collection_id,
                    buffer.len()
                );

                // Take ownership of buffered vectors
                let training_vectors: Vec<(String, Vec<f32>)> = std::mem::take(buffer);
                drop(pending); // Release the lock

                // Calculate number of clusters: Use sqrt-based with min/max clamping
                // Similar to block pruning mechanism - scales with data size
                // n_clusters = clamp(sqrt(N) * 2, min=16, max=256)
                let n_clusters = {
                    let sqrt_based = (training_vectors.len() as f32).sqrt() as usize * 2;
                    const MIN_CLUSTERS: usize = 16;
                    const MAX_CLUSTERS: usize = 256;
                    sqrt_based.clamp(MIN_CLUSTERS, MAX_CLUSTERS)
                };

                // Calculate n_probe: For incremental indexing where we train early with
                // limited samples, we need to search more clusters. Use 50% of clusters
                // minimum with sqrt-based scaling for larger cluster counts.
                // Formula: max(n_clusters/2, sqrt(n_clusters)*3), clamped to n_clusters
                let n_probe = {
                    let half_clusters = n_clusters / 2;
                    let sqrt_based = ((n_clusters as f32).sqrt() * 3.0) as usize;
                    std::cmp::max(half_clusters, sqrt_based).min(n_clusters)
                };

                tracing::debug!(
                    "🔧 AXIS: IVF config for collection {} - clusters: {}, n_probe: {} (sqrt-based)",
                    collection_id,
                    n_clusters,
                    n_probe
                );

                let config = UnifiedIvfConfig {
                    n_clusters,
                    n_probe,
                    dimension,
                    distance_metric: DistanceMetric::Cosine,
                    min_train_size: MIN_TRAIN_SIZE,
                    ..Default::default()
                };

                let mut index = UnifiedIvfIndex::new(collection_id.to_string(), config)?;

                // Train with just the vector data (not IDs)
                let vector_data: Vec<Vec<f32>> =
                    training_vectors.iter().map(|(_, v)| v.clone()).collect();
                index.train(vector_data).await?;

                tracing::info!(
                    "✅ AXIS: IVF index trained for collection {} with {} clusters",
                    collection_id,
                    n_clusters
                );

                // Add all buffered vectors to the trained index
                for (id, vec) in &training_vectors {
                    index.add_vector(id.clone(), vec.clone(), None).await?;
                }

                tracing::info!(
                    "✅ AXIS: Added {} vectors to IVF index for collection {}",
                    training_vectors.len(),
                    collection_id
                );

                // Store the trained index
                let mut indexes = self.ivf_indexes.write().await;
                indexes.insert(
                    collection_id.to_string(),
                    Arc::new(tokio::sync::RwLock::new(index)),
                );
            }
        }

        Ok(())
    }

    /// Query vectors from the IVF index (DEFAULT)
    async fn query_ivf(
        &self,
        collection_id: &str,
        query: &HybridQuery,
    ) -> Result<Vec<ScoredResult>> {
        let indexes = self.ivf_indexes.read().await;
        if let Some(index_lock) = indexes.get(collection_id) {
            let index = index_lock.read().await;

            // Check if index is trained
            if !index.is_trained() {
                tracing::debug!(
                    "🔍 AXIS: IVF index for collection {} not yet trained, returning empty results",
                    collection_id
                );
                return Ok(Vec::new());
            }

            if let Some(VectorQuery::Dense { vector, .. }) = &query.vector_query {
                let start = std::time::Instant::now();
                let results = index.search(vector, query.top_k, None).await?;
                let search_time = start.elapsed();

                tracing::info!(
                    "🔍 AXIS: IVF search completed for collection {} - {} results in {:?} (top_k={})",
                    collection_id,
                    results.len(),
                    search_time,
                    query.top_k
                );

                return Ok(results
                    .into_iter()
                    .map(|(id, score)| {
                        let expires_at = self.lookup_record_expiration(collection_id, &id);
                        ScoredResult {
                            vector_id: id,
                            similarity: score,
                            expires_at,
                        }
                    })
                    .collect());
            }
        } else {
            tracing::debug!(
                "🔍 AXIS: No IVF index found for collection {}, falling back to storage engine search",
                collection_id
            );
        }

        // Return empty if no index or no dense vector query
        Ok(Vec::new())
    }

    /// Analyze collection and trigger migration if beneficial
    pub async fn analyze_and_optimize(&self, collection_id: &str) -> Result<()> {
        // Check if migration is already in progress
        let migrations = self.active_migrations.read().await;
        if migrations.contains_key(collection_id) {
            return Ok(()); // Migration already in progress
        }
        drop(migrations);

        // Analyze collection characteristics
        let characteristics = self
            .adaptive_engine
            .analyze_collection(collection_id)
            .await?;

        // Determine if migration is beneficial
        let decision = self
            .adaptive_engine
            .should_migrate(collection_id, &characteristics)
            .await?;

        match decision {
            MigrationDecision::Migrate {
                from,
                to,
                estimated_improvement,
                ..
            } => {
                debug!(
                    "AXIS: Initiating migration for collection {} from {} to {} indexes (estimated improvement: {:.2}%)",
                    collection_id,
                    from.indexes.len(),
                    to.indexes.len(),
                    estimated_improvement * 100.0
                );

                // Start migration
                self.start_migration(collection_id, from, to).await?;
            }
            MigrationDecision::Stay { reason } => {
                debug!(
                    "AXIS: Collection {} staying with current // search_strategy removed -  {}",
                    collection_id, reason
                );
            }
        }

        Ok(())
    }

    /// Start migration to new indexing search_strategy
    async fn start_migration(
        &self,
        collection_id: &str,
        from: IndexSelectionStrategy,
        to: IndexSelectionStrategy,
    ) -> Result<()> {
        let migration_id = crate::utils::uuid::Uuid::new_v4();

        // Record migration start
        let mut migrations = self.active_migrations.write().await;
        migrations.insert(
            collection_id.to_string(),
            MigrationStatus {
                migration_id,
                from_strategy: from.clone(),
                to_strategy: to.clone(),
                start_time: Utc::now(),
                progress_percentage: 0.0,
                estimated_completion: None,
            },
        );
        drop(migrations);

        // Execute migration in background
        let migration_engine = self.migration_engine.clone();
        let collection_id = collection_id.to_string();
        let active_migrations = self.active_migrations.clone();
        let collection_strategies = self.collection_strategies.clone();
        let metrics = self.metrics.clone();

        tokio::spawn(async move {
            let result = migration_engine
                .execute_migration(&collection_id, from, to)
                .await;

            // Update status
            let mut migrations = active_migrations.write().await;
            migrations.remove(&collection_id);

            match result {
                Ok(migration_result) => {
                    // Update search_strategy
                    let mut strategies = collection_strategies.write().await;
                    strategies.insert(collection_id.to_string(), migration_result.new_strategy);

                    // Update metrics
                    let mut metrics = metrics.write().await;
                    metrics.total_migrations += 1;
                    metrics.successful_migrations += 1;
                    metrics.average_migration_time_ms = (metrics.average_migration_time_ms
                        * (metrics.total_migrations - 1)
                        + migration_result.duration_ms)
                        / metrics.total_migrations;

                    debug!(
                        "AXIS: Migration completed for collection {} in {}ms",
                        collection_id, migration_result.duration_ms
                    );
                }
                Err(e) => {
                    let mut metrics = metrics.write().await;
                    metrics.total_migrations += 1;
                    metrics.failed_migrations += 1;

                    error!(
                        "AXIS: Migration failed for collection {}: {}",
                        collection_id, e
                    );
                }
            }
        });

        Ok(())
    }

    /// Ensure collection has an indexing search_strategy
    pub async fn ensure_collection_strategy(&self, collection_id: &str) -> Result<()> {
        let strategies = self.collection_strategies.read().await;
        if strategies.contains_key(collection_id) {
            return Ok(());
        }
        drop(strategies);

        // Analyze collection and select initial search_strategy
        let characteristics = self
            .adaptive_engine
            .analyze_collection(collection_id)
            .await?;
        let search_strategy = self
            .adaptive_engine
            .recommend_strategy(&characteristics)
            .await?;

        let mut strategies = self.collection_strategies.write().await;
        strategies.insert(collection_id.to_string(), search_strategy);

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_collections_managed += 1;

        Ok(())
    }

    /// Get current search_strategy for collection
    pub async fn get_collection_strategy(
        &self,
        collection_id: &str,
    ) -> Result<IndexSelectionStrategy> {
        let strategies = self.collection_strategies.read().await;
        strategies.get(collection_id).cloned().ok_or_else(|| {
            anyhow::anyhow!("No search_strategy found for collection {}", collection_id)
        })
    }

    /// Update collection search_strategy
    pub async fn update_collection_strategy(
        &self,
        collection_id: &str,
        search_strategy: IndexSelectionStrategy,
    ) -> Result<()> {
        let mut strategies = self.collection_strategies.write().await;
        strategies.insert(collection_id.to_string(), search_strategy);
        Ok(())
    }

    /// Get the distance metric configured for a collection
    /// This ensures indexes use the same metric as the collection's stored config
    async fn get_collection_distance_metric(
        &self,
        collection_id: &str,
    ) -> Option<crate::compute::distance_computation::DistanceMetric> {
        use crate::compute::distance_computation::conversion::proto_distance_to_internal;

        // Try to get from shared cache first
        if let Some(cache) = &self.shared_collection_cache
            && let Some(collection) = cache.get(collection_id)
            && let Some(config) = &collection.config
        {
            let metric_code = config
                .distance_metric
                .unwrap_or(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32);
            return Some(proto_distance_to_internal(metric_code));
        }

        // Fall back to collection service
        if let Some(collection_service) = &self.collection_service
            && let Ok(Some(collection)) = collection_service.collection(collection_id).await
            && let Some(config) = &collection.config
        {
            let metric_code = config
                .distance_metric
                .unwrap_or(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32);
            return Some(proto_distance_to_internal(metric_code));
        }

        None
    }

    /// Maybe evaluate if search_strategy should change
    async fn maybe_evaluate_strategy(&self, _collection_id: &str) -> Result<()> {
        // Deferred: Implement periodic evaluation logic
        // For now, we'll rely on explicit analyze_and_optimize calls
        Ok(())
    }

    /// Get migration status for a collection
    pub async fn get_migration_status(&self, collection_id: &str) -> Option<MigrationStatus> {
        let migrations = self.active_migrations.read().await;
        migrations.get(collection_id).cloned()
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> AxisMetrics {
        self.metrics.read().await.clone()
    }

    /// Drop all indexes for a collection (used during collection deletion)
    pub async fn drop_collection(&self, collection_id: &str) -> Result<()> {
        tracing::info!(
            "🗑️ Dropping all AXIS indexes for collection: {}",
            collection_id
        );

        // Remove from collection strategies
        let mut strategies = self.collection_strategies.write().await;
        strategies.remove(collection_id);
        drop(strategies);

        // Clean up from all indexes
        self.global_id_index
            .remove_collection(collection_id)
            .await?;
        self.metadata_index.remove_collection(collection_id).await?;
        self.dense_vector_index
            .remove_collection(collection_id)
            .await?;
        if let Some(registry) = &self.hmgi_registry {
            registry.drop_collection_partitions(collection_id).await?;
        }
        {
            let mut enabled = self.hmgi_enabled_collections.write().await;
            enabled.remove(collection_id);
        }
        {
            let mut oids = self.hmgi_collection_oids.write().await;
            oids.remove(collection_id);
        }
        {
            let mut hnsw_indexes = self.hnsw_indexes.write().await;
            hnsw_indexes.remove(collection_id);
        }
        {
            let mut ivf_indexes = self.ivf_indexes.write().await;
            ivf_indexes.remove(collection_id);
        }
        {
            let mut pending = self.ivf_pending_vectors.write().await;
            pending.remove(collection_id);
        }
        self.sparse_vector_index
            .remove_collection(collection_id)
            .await?;

        {
            let mut collection_vectors = self.collection_vectors.write().await;
            collection_vectors.remove(collection_id);
        }

        // Update metrics
        let mut metrics = self.metrics.write().await;
        if metrics.total_collections_managed > 0 {
            metrics.total_collections_managed -= 1;
        }

        tracing::info!(
            "✅ Successfully dropped all indexes for collection: {}",
            collection_id
        );
        Ok(())
    }

    /// Get collection statistics
    pub async fn get_collection_stats(&self, collection_id: &str) -> Result<IndexCollectionStats> {
        let search_strategy = self.get_collection_strategy(collection_id).await?;

        Ok(IndexCollectionStats {
            collection_id: collection_id.to_string(),
            strategy_type: search_strategy
                .indexes
                .first()
                .map_or(Data::DenseVector { dimension: 128 }, |idx| idx.data_type), // Default to dense vector
            total_vectors: 0,    // Deferred: Implement actual counting
            index_size_bytes: 0, // Deferred: Implement actual size calculation
            last_updated: Utc::now(),
        })
    }

    /// Update vector file reference after flush/compaction
    /// This ensures AXIS indexes point to the correct on-disk files
    pub async fn update_vector_file_reference(
        &self,
        vector_id: &VectorId,
        collection_id: &str,
        file_path: &str,
    ) -> Result<()> {
        // Skip if vector_id is empty
        if vector_id.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            "🗂️ AXIS: Updating file reference for vector {} → {}",
            vector_id,
            file_path
        );

        // Ensure we have a search_strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        // Update file reference in global ID index
        self.global_id_index
            .update_file_reference(vector_id, file_path)
            .await?;

        // Update file references in secondary indexes based on search_strategy
        let search_strategy = self.get_collection_strategy(collection_id).await?;
        for index_spec in &search_strategy.indexes {
            match index_spec.data_type {
                Data::Metadata => {
                    self.metadata_index
                        .update_file_reference(vector_id, file_path)
                        .await?;
                }
                Data::DenseVector { .. } => {
                    self.dense_vector_index
                        .update_file_reference(vector_id, file_path)
                        .await?;
                }
                Data::SparseVector { .. } => {
                    self.sparse_vector_index
                        .update_file_reference(vector_id, file_path)
                        .await?;
                }
                _ => {}
            }
        }

        tracing::debug!(
            "✅ AXIS: Updated file reference for vector {} in all indexes",
            vector_id
        );
        Ok(())
    }

    /// Rebuild indexes after compaction
    /// This is called when storage files are merged/compacted
    pub async fn rebuild_indexes_after_compaction(
        &self,
        collection_id: &str,
        old_files: &[String],
        new_files: &[String],
    ) -> Result<()> {
        tracing::info!(
            "🔄 AXIS: Rebuilding indexes after compaction for collection {}",
            collection_id
        );
        tracing::debug!(
            "🔄 AXIS: Old files: {:?} → New files: {:?}",
            old_files,
            new_files
        );

        // Ensure we have a search_strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        // For now, we'll do a simple file reference update
        // In a production system, this would involve:
        // 1. Reading vector data from new_files
        // 2. Rebuilding the affected index segments
        // 3. Updating file references atomically

        let rebuild_start = Utc::now();

        // Update file references for all affected vectors
        // This is a simplified implementation - production would be more sophisticated
        for old_file in old_files {
            for new_file in new_files {
                tracing::debug!("🔄 AXIS: Mapping vectors from {} to {}", old_file, new_file);
                // In reality, we'd need to map specific vectors from old to new files
                // For now, we'll let the natural indexing process handle this
            }
        }

        let rebuild_duration = Utc::now().signed_duration_since(rebuild_start);
        tracing::info!(
            "✅ AXIS: Completed index rebuild for collection {} in {}ms",
            collection_id,
            rebuild_duration.num_milliseconds()
        );

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_migrations += 1; // Count rebuilds as migrations
        metrics.successful_migrations += 1;

        Ok(())
    }

    /// Get native index config for a collection
    pub async fn native_index_config(
        &self,
        _collection_id: &str,
    ) -> Result<crate::index::config::IndexConfig> {
        // Return default config for now
        // In production, this would look up collection-specific configuration
        Ok(crate::index::config::IndexConfig::default())
    }

    /// Notify AXIS about newly flushed vectors that need indexing
    /// This method is called by the flush coordinator after successful storage flush
    pub async fn handle_flushed_vectors(
        &self,
        collection_id: &str,
        flushed_vectors: Vec<VectorRecord>,
        files_created: Vec<String>,
    ) -> Result<()> {
        if flushed_vectors.is_empty() {
            tracing::debug!(
                "🔄 AXIS: No vectors to index for collection {}",
                collection_id
            );
            return Ok(());
        }

        tracing::info!(
            "🚀 AXIS: Processing {} newly flushed vectors for collection {} from {} files",
            flushed_vectors.len(),
            collection_id,
            files_created.len()
        );

        // Get IndexConfig for this collection to determine indexing behavior
        let index_config = match self.native_index_config(collection_id).await {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(
                    "⚠️ AXIS: Failed to get IndexConfig for collection {}: {}. Using default sync mode.",
                    collection_id,
                    e
                );
                // Use default synchronous indexing if config retrieval fails
                crate::index::config::IndexConfig::default()
            }
        };

        tracing::debug!(
            "🎯 AXIS: Using IndexConfig with update_mode: {:?} for collection {}",
            index_config.update_mode,
            collection_id
        );

        // Handle indexing based on update mode
        match index_config.update_mode {
            crate::index::config::IndexUpdateMode::Synchronous => {
                self.index_vectors_synchronously(collection_id, flushed_vectors, &files_created)
                    .await?;
            }
            crate::index::config::IndexUpdateMode::Asynchronous => {
                self.index_vectors_asynchronously(collection_id, flushed_vectors, files_created)
                    .await?;
            }
            crate::index::config::IndexUpdateMode::Hybrid => {
                self.index_vectors_hybrid(
                    collection_id,
                    flushed_vectors,
                    files_created,
                    &index_config,
                )
                .await?;
            }
        }

        tracing::info!(
            "✅ AXIS: Completed indexing notification for collection {}",
            collection_id
        );

        Ok(())
    }

    /// Index vectors synchronously (blocking the flush completion)
    ///
    /// For batches >= 500 vectors, uses batch-aware IVF training to ensure
    /// proper cluster count for better recall.
    async fn index_vectors_synchronously(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
        _files_created: &[String],
    ) -> Result<()> {
        let batch_size = vectors.len();
        tracing::info!(
            "🔄 AXIS: Synchronous indexing of {} vectors for collection {}",
            batch_size,
            collection_id
        );

        let start_time = std::time::Instant::now();

        // For medium-sized batches, still use batch training for better recall
        if batch_size >= 500
            && let Err(e) = self.train_ivf_for_batch(collection_id, &vectors).await
        {
            tracing::warn!(
                "⚠️ AXIS: Batch IVF training failed for collection {}: {}, using incremental",
                collection_id,
                e
            );
        }

        for vector in vectors {
            self.insert(collection_id, &vector).await?;
        }
        let duration = start_time.elapsed();

        tracing::info!(
            "✅ AXIS: Synchronous indexing completed in {}ms for collection {}",
            duration.as_millis(),
            collection_id
        );

        Ok(())
    }

    /// Index vectors asynchronously (non-blocking)
    ///
    /// For large batches (>1000 vectors), this method uses batch-aware IVF training
    /// to ensure proper cluster count based on total collection size. This fixes
    /// the recall degradation issue at 50K vectors where IVF was trained with only
    /// 100 vectors, resulting in too few clusters (20 instead of 256).
    async fn index_vectors_asynchronously(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
        files_created: Vec<String>,
    ) -> Result<()> {
        let batch_size = vectors.len();
        tracing::info!(
            "🚀 AXIS: Spawning asynchronous indexing task for {} vectors in collection {}",
            batch_size,
            collection_id
        );

        let start_time = std::time::Instant::now();

        // For large batches, use batch-aware IVF training for better recall
        // This trains the IVF index with proper cluster count based on total batch size
        if batch_size >= 1000 {
            match self.train_ivf_for_batch(collection_id, &vectors).await {
                Ok(()) => {
                    tracing::info!(
                        "✅ AXIS: Batch IVF training completed for {} vectors in collection {}",
                        batch_size,
                        collection_id
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️ AXIS: Batch IVF training failed for collection {}: {}, falling back to incremental",
                        collection_id,
                        e
                    );
                }
            }
        }

        // Insert all vectors (will go to trained IVF index directly if batch training succeeded)
        let mut indexed_count = 0;
        for vector in vectors {
            match self.insert(collection_id, &vector).await {
                Ok(()) => indexed_count += 1,
                Err(e) => {
                    tracing::error!(
                        "❌ AXIS: Failed to index vector in collection {}: {}",
                        collection_id,
                        e
                    );
                }
            }
        }

        let duration = start_time.elapsed();
        tracing::info!(
            "✅ AXIS: Asynchronous indexing completed - {}/{} vectors indexed in {}ms for collection {} (files: {:?})",
            indexed_count,
            batch_size,
            duration.as_millis(),
            collection_id,
            files_created
        );

        tracing::debug!(
            "🚀 AXIS: Asynchronous indexing completed for collection {}",
            collection_id
        );
        Ok(())
    }

    /// Train IVF index with proper cluster count for a large batch
    ///
    /// This method trains the IVF index with the optimal number of clusters
    /// based on the total batch size, avoiding the issue of undertrained indexes
    /// when vectors are inserted one-by-one (which trains at only 100 vectors).
    ///
    /// For N vectors:
    /// - n_clusters = clamp(sqrt(N) * 2, 16, 256)
    /// - n_probe = max(n_clusters/2, sqrt(n_clusters)*3)
    async fn train_ivf_for_batch(
        &self,
        collection_id: &str,
        vectors: &[VectorRecord],
    ) -> Result<()> {
        use crate::compute::distance_computation::DistanceMetric;
        use crate::index::axis::indexes::ivf_unified::{UnifiedIvfConfig, UnifiedIvfIndex};

        if vectors.is_empty() {
            return Ok(());
        }

        let dimension = vectors[0].vector.len();
        if dimension == 0 {
            return Ok(());
        }

        // Check if IVF index already exists and is trained
        {
            let indexes = self.ivf_indexes.read().await;
            if let Some(index) = indexes.get(collection_id) {
                let idx = index.read().await;
                if idx.is_trained() {
                    tracing::debug!(
                        "🔍 AXIS: IVF index for collection {} already trained, skipping batch training",
                        collection_id
                    );
                    return Ok(());
                }
            }
        }

        let batch_size = vectors.len();

        // Calculate optimal cluster count based on batch size
        // n_clusters = clamp(sqrt(N) * 2, 16, 256)
        let n_clusters = {
            let sqrt_based = (batch_size as f32).sqrt() as usize * 2;
            const MIN_CLUSTERS: usize = 16;
            const MAX_CLUSTERS: usize = 256;
            sqrt_based.clamp(MIN_CLUSTERS, MAX_CLUSTERS)
        };

        // Calculate n_probe: max(n_clusters/2, sqrt(n_clusters)*3), clamped to n_clusters
        let n_probe = {
            let half_clusters = n_clusters / 2;
            let sqrt_based = ((n_clusters as f32).sqrt() * 3.0) as usize;
            std::cmp::max(half_clusters, sqrt_based).min(n_clusters)
        };

        tracing::info!(
            "🎯 AXIS: Batch IVF training for collection {} - {} vectors, {} clusters, n_probe={}",
            collection_id,
            batch_size,
            n_clusters,
            n_probe
        );

        let config = UnifiedIvfConfig {
            n_clusters,
            n_probe,
            dimension,
            distance_metric: DistanceMetric::Cosine,
            min_train_size: 100, // Lower since we're batch training
            ..Default::default()
        };

        let mut index = UnifiedIvfIndex::new(collection_id.to_string(), config)?;

        // Train with all vectors (or sample if too large)
        let training_vectors: Vec<Vec<f32>> = if batch_size > 50000 {
            // Sample for very large batches to avoid memory issues
            let sample_size = 50000;
            let step = batch_size / sample_size;
            vectors
                .iter()
                .step_by(step.max(1))
                .take(sample_size)
                .map(|v| v.vector.clone())
                .collect()
        } else {
            vectors.iter().map(|v| v.vector.clone()).collect()
        };

        index.train(training_vectors).await?;

        tracing::info!(
            "✅ AXIS: Batch IVF training complete for collection {} with {} clusters",
            collection_id,
            n_clusters
        );

        // Store the trained index
        let mut indexes = self.ivf_indexes.write().await;
        indexes.insert(
            collection_id.to_string(),
            Arc::new(tokio::sync::RwLock::new(index)),
        );

        // Clear pending vectors buffer since we've trained
        {
            let mut pending = self.ivf_pending_vectors.write().await;
            pending.remove(collection_id);
        }

        Ok(())
    }

    /// Index vectors using hybrid mode (adaptive based on batch size)
    pub async fn index_vectors_hybrid(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
        files_created: Vec<String>,
        index_config: &crate::index::config::IndexConfig,
    ) -> Result<()> {
        let batch_size_threshold = index_config.async_update_batch_size.unwrap_or(1000);

        tracing::info!(
            "🎯 AXIS: Hybrid indexing for {} vectors (threshold: {}) in collection {}",
            vectors.len(),
            batch_size_threshold,
            collection_id
        );

        if vectors.len() <= batch_size_threshold {
            tracing::debug!(
                "🔄 AXIS: Small batch - using synchronous indexing for collection {}",
                collection_id
            );
            self.index_vectors_synchronously(collection_id, vectors, &files_created)
                .await
        } else {
            tracing::debug!(
                "🚀 AXIS: Large batch - using asynchronous indexing for collection {}",
                collection_id
            );
            self.index_vectors_asynchronously(collection_id, vectors, files_created)
                .await
        }
    }

    /// Get all indexes for a collection (required for compaction integration)
    /// Return the live HNSW index for a collection if one exists (Phase C).
    pub async fn get_hnsw_index(
        &self,
        collection_id: &str,
    ) -> Option<Arc<crate::index::axis::indexes::hnsw_index::AxisHnswIndex>> {
        self.hnsw_indexes.read().await.get(collection_id).cloned()
    }

    pub async fn get_collection_indexes(
        &self,
        collection_id: &str,
    ) -> Result<Vec<(String, Arc<dyn crate::index::axis::AxisVectorIndex>)>> {
        tracing::debug!("🔍 AXIS: Getting indexes for collection {}", collection_id);

        // For now, return empty list - this will be properly implemented when compaction integration is complete
        // In the full implementation, this would:
        // 1. Check collection_strategies for active indexes
        // 2. Return references to global_id_index, metadata_index, dense_vector_index, sparse_vector_index
        // 3. Include any dynamic indexes created by the adaptive engine

        Ok(Vec::new())
    }

    /// Rebuild a specific index by name (required for compaction integration)
    pub async fn rebuild_index(&self, collection_id: &str, index_name: &str) -> Result<()> {
        tracing::info!(
            "🔄 AXIS: Rebuilding index '{}' for collection {}",
            index_name,
            collection_id
        );

        // For now, delegate to the full rebuild method
        // In the full implementation, this would:
        // 1. Identify the specific index component by name
        // 2. Rebuild only that index while keeping others intact
        // 3. Update internal tracking structures

        self.rebuild_indexes_after_compaction(collection_id, &[], &[])
            .await
    }

    /// Quantize vector for in-memory index using collection's quantization settings
    /// This reuses our existing modular quantization infrastructure
    async fn quantize_for_index(
        &self,
        vector: &VectorRecord,
        quant_config: &crate::proto::proximadb_v1::QuantizationConfig,
        collection_config: &crate::proto::proximadb_v1::CollectionConfig,
    ) -> Result<VectorRecord> {
        use crate::compute::distance_computation::conversion::proto_distance_to_internal;
        use crate::compute::quantization::storage_engine::{
            StorageQuantizationConfig, StorageQuantizationEngine,
        };

        // Extract the vector data
        let vector_data = &vector.vector;
        if vector_data.is_empty() {
            return Ok(vector.clone());
        }

        // Use helper function for distance metric conversion
        let distance_metric =
            proto_distance_to_internal(collection_config.distance_metric.unwrap_or(0));

        // Create quantization config using collection settings with proper field mapping
        let storage_config = StorageQuantizationConfig {
            // Map to the actual fields available in storage engine config
            primary_level: Some(
                crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(
                    // Default to dimension/4 with min 8 and max 64 subvectors
                    (collection_config.dimension / 4).clamp(8, 64).min(255) as u8,
                ),
            ),
            filter_level: Some(
                crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
            ),
            fast_level: Some(
                crate::compute::quantization::unified::UnifiedQuantizationLevel::int8(),
            ),
            distance_metric,
            enable_progressive: true,
            filter_threshold: 0.8,
            candidate_multiplier: 10,
            // quality_threshold removed -  0.95,
            training_sample_size: quant_config.training_sample_size.unwrap_or(10000) as usize,
            memory_budget_mb: 512,
            enable_hardware_acceleration: true,
        };

        // Create required components for quantization engine
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                distance_metric,
            ),
        );
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ),
        );

        // Create quantization engine
        let _engine =
            StorageQuantizationEngine::new(unified_engine, distance_compute, storage_config);

        // For indexes, we don't actually quantize the vector data in the VectorRecord
        // Instead, indexes maintain their own quantized representation internally
        // This is just a placeholder that marks the vector as quantized for the index
        // The actual quantization happens inside the index implementations

        // Return the original vector marked for quantization
        // The index will handle the actual quantization internally
        Ok(vector.clone())
    }
}

/// AXIS index-level collection statistics
///
/// Distinct from `storage::traits::CollectionStats` — carries index-specific
/// metadata (strategy_type, last_updated timestamp).
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexCollectionStats {
    /// Unique identifier of the collection.
    pub collection_id: String,
    /// Data type strategy used for this collection's index.
    pub strategy_type: Data,
    /// Total number of vectors indexed.
    pub total_vectors: u64,
    /// Size of the index on disk in bytes.
    pub index_size_bytes: u64,
    /// Timestamp of the last index update.
    pub last_updated: DateTime<Utc>,
}

/// Hybrid query combining multiple search criteria
#[derive(Debug, Clone)]
pub struct HybridQuery {
    /// Target collection for the query.
    pub collection_id: String,
    /// Optional vector similarity query component.
    pub vector_query: Option<VectorQuery>,
    /// Metadata field filter predicates.
    pub metadata_filters: Vec<MetadataFilter>,
    /// Exact vector ID filters for point lookups.
    pub id_filters: Vec<VectorId>,
    /// Maximum number of results to return.
    pub top_k: usize,
    /// Whether to include MVCC-expired records in results.
    pub include_expired: bool,
}

/// Vector query types
#[derive(Debug, Clone)]
pub enum VectorQuery {
    /// Dense vector similarity query.
    Dense {
        /// Query vector in full-precision f32 format.
        vector: Vec<f32>,
        /// Minimum similarity score threshold for results.
        similarity_threshold: f32,
    },
    /// Sparse vector similarity query.
    Sparse {
        /// Sparse query vector as dimension-index to value mapping.
        vector: HashMap<u32, f32>,
        /// Minimum similarity score threshold for results.
        similarity_threshold: f32,
    },
}

/// Metadata filter
#[derive(Debug, Clone)]
pub struct MetadataFilter {
    /// Name of the metadata field to filter on.
    pub field: String,
    /// Comparison operator for the filter.
    pub operator: FilterOperator,
    /// Value to compare against.
    pub value: serde_json::Value,
}

/// Filter operators
#[derive(Debug, Clone, PartialEq)]
pub enum FilterOperator {
    /// Exact equality match.
    Equals,
    /// Not equal comparison.
    NotEquals,
    /// Greater than comparison.
    GreaterThan,
    /// Greater than or equal comparison.
    GreaterThanOrEqual,
    /// Less than comparison.
    LessThan,
    /// Less than or equal comparison.
    LessThanOrEqual,
    /// Membership in a set of values.
    In,
    /// Exclusion from a set of values.
    NotIn,
    /// String contains substring.
    Contains,
    /// String starts with prefix.
    StartsWith,
    /// String ends with suffix.
    EndsWith,
    /// SQL-style LIKE pattern matching.
    Like,
    /// Inclusive range comparison with `[lower, upper]`.
    Between,
    /// Value is null or missing.
    IsNull,
    /// Value is present and non-null.
    IsNotNull,
}

/// Query result
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Scored results ordered by relevance.
    pub results: Vec<ScoredResult>,
    /// Index selection strategy that was used to execute the query.
    pub strategy_used: IndexSelectionStrategy,
    /// Total execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Scored result with MVCC support
#[derive(Debug, Clone)]
pub struct ScoredResult {
    /// Identifier of the matching vector.
    pub vector_id: VectorId,
    /// Similarity score between the query and this result.
    pub similarity: f32,
    /// MVCC expiration timestamp, if the record has a TTL.
    pub expires_at: Option<DateTime<Utc>>,
}

impl AxisManager {
    async fn execute_exact_filtered_query(
        &self,
        collection_id: &str,
        query: &HybridQuery,
    ) -> Result<Vec<ScoredResult>> {
        let records = {
            let collection_vectors = self.collection_vectors.read().await;
            collection_vectors
                .get(collection_id)
                .map(|vectors| vectors.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };

        if records.is_empty() {
            return Ok(Vec::new());
        }

        let metric = self
            .get_collection_distance_metric(collection_id)
            .await
            .unwrap_or(crate::compute::distance_computation::DistanceMetric::DotProduct);
        let compute = UnifiedDistanceCompute::new(metric);
        let metadata_expression = if query.metadata_filters.is_empty() {
            None
        } else {
            Some(self.metadata_filters_to_expression(&query.metadata_filters))
        };

        let mut results = Vec::new();

        for record in records {
            if !query.id_filters.is_empty() && !query.id_filters.contains(&record.id) {
                continue;
            }

            let metadata = self.record_filter_metadata(&record);
            if let Some(expr) = &metadata_expression
                && !crate::core::search::json_comparison::evaluate_filter(expr, &metadata)
            {
                continue;
            }

            let similarity = match &query.vector_query {
                Some(VectorQuery::Dense {
                    vector,
                    similarity_threshold,
                }) => {
                    let result = compute.similarity(vector, &record.vector, Some(metric));
                    if result.normalized_score < *similarity_threshold {
                        continue;
                    }
                    result.normalized_score
                }
                Some(VectorQuery::Sparse { .. }) => continue,
                None => 1.0,
            };

            let expires_at = record
                .expires_at
                .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0));

            if !query.include_expired
                && let Some(expiration) = expires_at.as_ref()
                && Utc::now() >= *expiration
            {
                continue;
            }

            results.push(ScoredResult {
                vector_id: record.id,
                similarity,
                expires_at,
            });
        }

        results.sort_by(|left, right| {
            right
                .similarity
                .partial_cmp(&left.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.vector_id.cmp(&right.vector_id))
        });
        results.truncate(query.top_k);

        Ok(results)
    }

    fn metadata_filters_to_expression(
        &self,
        filters: &[MetadataFilter],
    ) -> crate::core::search::FilterExpression {
        crate::core::search::FilterExpression::And(
            filters
                .iter()
                .map(|filter| crate::core::search::FilterExpression::Comparison {
                    field: filter.field.clone(),
                    operator: self.axis_filter_operator_to_comparison(&filter.operator),
                    value: filter.value.clone(),
                })
                .collect(),
        )
    }

    fn axis_filter_operator_to_comparison(
        &self,
        operator: &FilterOperator,
    ) -> crate::core::search::ComparisonOperator {
        match operator {
            FilterOperator::Equals => crate::core::search::ComparisonOperator::Equals,
            FilterOperator::NotEquals => crate::core::search::ComparisonOperator::NotEquals,
            FilterOperator::GreaterThan => crate::core::search::ComparisonOperator::GreaterThan,
            FilterOperator::GreaterThanOrEqual => {
                crate::core::search::ComparisonOperator::GreaterThanOrEqual
            }
            FilterOperator::LessThan => crate::core::search::ComparisonOperator::LessThan,
            FilterOperator::LessThanOrEqual => {
                crate::core::search::ComparisonOperator::LessThanOrEqual
            }
            FilterOperator::In => crate::core::search::ComparisonOperator::In,
            FilterOperator::NotIn => crate::core::search::ComparisonOperator::NotIn,
            FilterOperator::Contains => crate::core::search::ComparisonOperator::Contains,
            FilterOperator::StartsWith => crate::core::search::ComparisonOperator::StartsWith,
            FilterOperator::EndsWith => crate::core::search::ComparisonOperator::EndsWith,
            FilterOperator::Like => crate::core::search::ComparisonOperator::Like,
            FilterOperator::Between => crate::core::search::ComparisonOperator::Between,
            FilterOperator::IsNull => crate::core::search::ComparisonOperator::IsNull,
            FilterOperator::IsNotNull => crate::core::search::ComparisonOperator::IsNotNull,
        }
    }

    fn record_filter_metadata(&self, record: &VectorRecord) -> HashMap<String, Value> {
        let mut metadata =
            crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&record.metadata);
        metadata.insert("id".to_string(), Value::String(record.id.clone()));
        metadata
    }

    fn lookup_record_expiration(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Option<DateTime<Utc>> {
        self.collection_vectors
            .try_read()
            .ok()
            .and_then(|collections| collections.get(collection_id).cloned())
            .and_then(|vectors| vectors.get(vector_id).cloned())
            .and_then(|record| {
                record
                    .expires_at
                    .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
            })
    }
}

/// HMGI (Hierarchical Multi-modality Graph Indexing) extensions for AxisManager
///
/// HMGI provides per-modality HNSW partitioning for multi-modality collections,
/// achieving 70% search space reduction compared to monolithic HNSW.
///
/// ## Usage
///
/// ```rust,ignore
/// // Enable HMGI for a collection
/// axis_manager.enable_hmgi("my_collection", Some("_modality")).await?;
///
/// // Insert vectors (automatically routed to modality partitions)
/// axis_manager.insert_hmgi("my_collection", vector_record, oid).await?;
///
/// // Search with automatic partition routing
/// let results = axis_manager.search_hmgi("my_collection", query, top_k).await?;
/// ```
impl AxisManager {
    /// Enable HMGI for a collection
    ///
    /// ## Arguments
    ///
    /// - `collection_id`: Collection to enable HMGI for
    /// - `modality_field`: Field name containing modality tag (default: "_modality")
    /// - `oid`: Entity type ID for partition key generation
    ///
    /// ## Process
    ///
    /// 1. Initialize HMGI components if not already done
    /// 2. Register collection as HMGI-enabled
    /// 3. Migrate existing vectors to modality partitions
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// axis_manager.enable_hmgi("documents", Some("_modality"), 123).await?;
    /// ```
    pub async fn enable_hmgi(
        &self,
        collection_id: &str,
        modality_field: Option<String>,
        oid: u64,
    ) -> Result<()> {
        // Store the field for logging before moving
        let field_display = modality_field.clone();

        // Initialize HMGI components if not already done
        self.ensure_hmgi_initialized(modality_field).await?;

        // Register collection as HMGI-enabled
        {
            let mut enabled = self.hmgi_enabled_collections.write().await;
            enabled.insert(collection_id.to_string());
        }

        // Store OID for partition key generation
        {
            let mut oids = self.hmgi_collection_oids.write().await;
            oids.insert(collection_id.to_string(), oid);
        }

        tracing::info!(
            "✅ HMGI enabled for collection '{}' with modality field '{:?}'",
            collection_id,
            field_display
        );

        // Migrate existing vectors to HMGI partitions
        self.migrate_collection_to_hmgi(collection_id).await?;

        Ok(())
    }

    /// Disable HMGI for a collection
    ///
    /// Merges all modality partitions back into a single index.
    pub async fn disable_hmgi(&self, collection_id: &str) -> Result<()> {
        let registry = self
            .hmgi_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI not initialized"))?;

        // Drop all partitions for this collection
        registry.drop_collection_partitions(collection_id).await?;

        // Remove from enabled set
        {
            let mut enabled = self.hmgi_enabled_collections.write().await;
            enabled.remove(collection_id);
        }

        // Remove OID mapping
        {
            let mut oids = self.hmgi_collection_oids.write().await;
            oids.remove(collection_id);
        }

        tracing::info!("❌ HMGI disabled for collection '{}'", collection_id);
        Ok(())
    }

    /// Check if HMGI is enabled for a collection
    pub async fn is_hmgi_enabled(&self, collection_id: &str) -> bool {
        let enabled = self.hmgi_enabled_collections.read().await;
        enabled.contains(collection_id)
    }

    /// Analyze indexed collection records and auto-enable HMGI when multiple modalities appear.
    pub async fn maybe_auto_enable_hmgi(
        &self,
        collection_id: &str,
    ) -> Result<Option<DetectionResult>> {
        if self.is_hmgi_enabled(collection_id).await {
            return Ok(None);
        }

        let detector = match self.hmgi_detector.as_ref() {
            Some(detector) => detector,
            None => return Ok(None),
        };

        let samples = self.hmgi_detection_samples(collection_id).await;
        let detection = detector.detect_modalities(collection_id, &samples).await;

        if detection.should_enable_hmgi {
            let oid = self.hmgi_oid_for_collection(collection_id).await;
            self.enable_hmgi(collection_id, None, oid).await?;
        }

        Ok(Some(detection))
    }

    /// Ensure a collection has HMGI partitioning before dense vector indexing.
    async fn ensure_hmgi_collection_enabled(&self, collection_id: &str) -> Result<()> {
        if self.is_hmgi_enabled(collection_id).await {
            return Ok(());
        }

        let oid = self.hmgi_oid_for_collection(collection_id).await;
        self.enable_hmgi(collection_id, None, oid).await
    }

    fn is_hmgi_routable_query(&self, query: &HybridQuery) -> bool {
        if !matches!(query.vector_query, Some(VectorQuery::Dense { .. }))
            || !query.id_filters.is_empty()
        {
            return false;
        }

        let modality_field = self
            .hmgi_extractor
            .as_ref()
            .map(|extractor| extractor.modality_field())
            .unwrap_or("_modality");

        query
            .metadata_filters
            .iter()
            .all(|filter| filter.field == modality_field)
    }

    /// Insert a vector with HMGI partitioning
    ///
    /// Routes the vector to the appropriate modality partition based on metadata.
    ///
    /// ## Arguments
    ///
    /// - `collection_id`: Collection to insert into
    /// - `record`: Vector record with metadata containing modality tag
    ///
    /// ## Returns
    ///
    /// The partition key the vector was inserted into
    pub async fn insert_hmgi(
        &self,
        collection_id: &str,
        record: VectorRecord,
    ) -> Result<HmgiPartitionKey> {
        if !self.is_hmgi_enabled(collection_id).await {
            return Err(anyhow::anyhow!(
                "HMGI not enabled for collection '{}'",
                collection_id
            ));
        }

        let registry = self
            .hmgi_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI not initialized"))?;

        let extractor = self
            .hmgi_extractor
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI extractor not initialized"))?;

        // Get OID for this collection
        let oid = {
            let oids = self.hmgi_collection_oids.read().await;
            *oids
                .get(collection_id)
                .ok_or_else(|| anyhow::anyhow!("No OID found for collection '{}'", collection_id))?
        };

        // Extract modality from metadata
        let metadata =
            crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&record.metadata);
        let modality_tag = extractor.extract_modality(&metadata);

        // Create partition key
        let partition_key = HmgiPartitionKey::new(oid, 1, modality_tag, None);

        // Get vector dimension from the record
        let dimension = if record.vector.is_empty() {
            128
        } else {
            record.vector.len()
        };

        // Get or create partition with default config
        let config = crate::index::axis::indexes::hnsw_index::AxisHnswConfig::default();
        let index = registry
            .get_or_create_partition(partition_key.clone(), config, dimension)
            .await?;
        registry
            .register_collection_partition(collection_id, partition_key.clone())
            .await;

        use crate::index::axis::index_factory::AxisVectorIndex;
        if !record.id.is_empty() && !record.vector.is_empty() {
            index.add(record.id.clone(), record.vector.clone()).await?;
        }

        tracing::debug!(
            "Inserting vector '{}' into HMGI partition '{}'",
            record.id,
            partition_key
        );

        Ok(partition_key)
    }

    /// Remove a vector from every HMGI partition registered for the collection.
    async fn remove_hmgi_vector(&self, collection_id: &str, vector_id: &VectorId) -> Result<()> {
        let registry = self
            .hmgi_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI registry not initialized"))?;

        let partitions = registry.get_partitions_for_collection(collection_id).await;

        use crate::index::axis::index_factory::AxisVectorIndex;
        for partition in partitions {
            if let Some(index) = registry.get_partition(&partition).await {
                index.remove(vector_id).await?;
            }
        }

        Ok(())
    }

    /// Search with HMGI partition routing
    ///
    /// Routes queries to relevant modality partitions based on filters.
    ///
    /// ## Arguments
    ///
    /// - `collection_id`: Collection to search
    /// - `query`: Hybrid query with optional metadata filters
    /// - `top_k`: Number of results to return
    ///
    /// ## Returns
    ///
    /// Top-k results from relevant partitions
    pub async fn search_hmgi(
        &self,
        collection_id: &str,
        query: &HybridQuery,
        _top_k: usize,
    ) -> Result<Vec<ScoredResult>> {
        if !self.is_hmgi_enabled(collection_id).await {
            // Fall back to non-HMGI search
            return self
                .execute_exact_filtered_query(collection_id, query)
                .await;
        }

        let router = self
            .hmgi_router
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI router not initialized"))?;

        // Get all partitions for this collection
        let registry = self
            .hmgi_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI registry not initialized"))?;
        let all_partitions = registry.get_partitions_for_collection(collection_id).await;

        // Create PartitionSet from all partitions
        use crate::index::axis::hmgi::PartitionSet;
        let mut partition_set = PartitionSet::new();
        for p in all_partitions {
            partition_set.insert(p);
        }

        // Route query to relevant partitions
        let routed = router
            .route_query(collection_id, query, partition_set)
            .await?;

        if routed.is_empty() {
            tracing::warn!(
                "No HMGI partitions found for collection '{}'",
                collection_id
            );
            return Ok(Vec::new());
        }

        // Convert PartitionSet to Vec<HmgiPartitionKey>
        let partitions: Vec<HmgiPartitionKey> = routed.iter().cloned().collect();

        tracing::debug!(
            "HMGI search routing to {} partitions for collection '{}'",
            partitions.len(),
            collection_id
        );

        // Search across routed partitions
        let results = router.search_partitions(partitions, query).await?;

        Ok(results)
    }

    /// Initialize HMGI components if not already done
    async fn ensure_hmgi_initialized(&self, _modality_field: Option<String>) -> Result<()> {
        // Already initialized
        if self.hmgi_registry.is_some() {
            return Ok(());
        }

        // This is a self-referential issue - we can't initialize through &self
        // In production, this would be done through interior mutability or
        // the components would be passed in during construction
        tracing::warn!("HMGI initialization requires mutable access - use enable_hmgi_init");
        Err(anyhow::anyhow!(
            "HMGI components not initialized - call enable_hmgi_init first"
        ))
    }

    /// Migrate existing collection vectors to HMGI partitions
    async fn migrate_collection_to_hmgi(&self, collection_id: &str) -> Result<()> {
        let extractor = self
            .hmgi_extractor
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI extractor not initialized"))?;

        let registry = self
            .hmgi_registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HMGI registry not initialized"))?;

        // Get OID for this collection
        let oid = {
            let oids = self.hmgi_collection_oids.read().await;
            *oids
                .get(collection_id)
                .ok_or_else(|| anyhow::anyhow!("No OID found for collection '{}'", collection_id))?
        };

        // Get existing vectors
        let vectors = {
            let collection_vectors = self.collection_vectors.read().await;
            collection_vectors
                .get(collection_id)
                .map(|v| v.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };

        tracing::info!(
            "Migrating {} vectors from collection '{}' to HMGI partitions",
            vectors.len(),
            collection_id
        );

        let mut migrated = 0;
        for record in vectors {
            // Extract modality
            let metadata =
                crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&record.metadata);
            let modality_tag = extractor.extract_modality(&metadata);

            // Create partition key
            let partition_key = HmgiPartitionKey::new(oid, 1, modality_tag, None);

            // Get or create partition with default config
            let dimension = if record.vector.is_empty() {
                128
            } else {
                record.vector.len()
            };
            let config = crate::index::axis::indexes::hnsw_index::AxisHnswConfig::default();
            let _index = registry
                .get_or_create_partition(partition_key.clone(), config, dimension)
                .await?;
            registry
                .register_collection_partition(collection_id, partition_key)
                .await;
            use crate::index::axis::index_factory::AxisVectorIndex;
            if !record.id.is_empty() && !record.vector.is_empty() {
                _index.add(record.id.clone(), record.vector.clone()).await?;
            }
            migrated += 1;
        }

        tracing::info!("Migrated {} vectors to HMGI partitions", migrated);
        Ok(())
    }

    async fn hmgi_detection_samples(&self, collection_id: &str) -> Vec<VectorRecordSample> {
        let collection_vectors = self.collection_vectors.read().await;
        collection_vectors
            .get(collection_id)
            .map(|records| {
                records
                    .values()
                    .map(|record| {
                        VectorRecordSample::new(
                            crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(
                                &record.metadata,
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn hmgi_oid_for_collection(&self, collection_id: &str) -> u64 {
        {
            let oids = self.hmgi_collection_oids.read().await;
            if let Some(oid) = oids.get(collection_id) {
                return *oid;
            }
        }

        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        collection_id.hash(&mut hasher);
        hasher.finish()
    }
}

/// Mutable HMGI initialization for AxisManager
///
/// This trait provides mutable methods for initializing HMGI components.
/// In production, this would use interior mutability (RwLock) instead.
impl AxisManager {
    /// Initialize HMGI components (mutable version)
    ///
    /// Call this before enabling HMGI for collections if components
    /// weren't initialized during construction.
    pub fn init_hmgi(&mut self, modality_field: Option<String>) -> Result<()> {
        let field = modality_field.unwrap_or_else(|| "_modality".to_string());

        self.hmgi_registry = Some(Arc::new(HmgiRegistry::new()));
        self.hmgi_extractor = Some(Arc::new(ModalityExtractor::with_config(
            field.clone(),
            "default".to_string(),
        )));
        self.hmgi_detector = Some(Arc::new(ModalityDetector::default_config()));

        // Router requires registry and extractor
        let registry = self.hmgi_registry.clone().unwrap();
        let extractor = self.hmgi_extractor.clone().unwrap();
        self.hmgi_router = Some(Arc::new(HmgiRouter::new(registry, extractor)));

        tracing::info!(
            "🔧 HMGI components initialized with modality field '{}'",
            field
        );
        Ok(())
    }

    /// Get HMGI registry (for testing/diagnostics)
    pub fn hmgi_registry(&self) -> Option<&Arc<HmgiRegistry>> {
        self.hmgi_registry.as_ref()
    }

    /// Get HMGI router (for testing/diagnostics)
    pub fn hmgi_router(&self) -> Option<&Arc<HmgiRouter>> {
        self.hmgi_router.as_ref()
    }

    /// Get modality extractor (for testing/diagnostics)
    pub fn hmgi_extractor(&self) -> Option<&Arc<ModalityExtractor>> {
        self.hmgi_extractor.as_ref()
    }

    /// Get modality detector (for testing/diagnostics)
    pub fn hmgi_detector(&self) -> Option<&Arc<ModalityDetector>> {
        self.hmgi_detector.as_ref()
    }
}
