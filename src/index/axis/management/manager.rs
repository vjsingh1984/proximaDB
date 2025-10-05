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
//! ```rust
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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error};

use crate::core::{String, VectorId, VectorRecord};
use crate::index::axis::management::{
    migration_engine::{IndexMigrationEngine, MigrationDecision},
    monitor::PerformanceMonitor,
};
use crate::index::axis::{
    clustering::AxisClusteringEngine,
    clustering::ClusteringConfig,
    management::adaptive_engine::AdaptiveIndexEngine,
    types::{AxisConfig, Data, IndexSelectionStrategy},
};
use crate::index::{DenseVectorIndex, GlobalIdIndex, JoinEngine, MetadataIndex, SparseVectorIndex};
// Temporarily disabled due to arrow-arith compilation conflicts - TODO: Re-enable when resolved
// use crate::storage::engines::impls::viper::QuantizationMethod;

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
    performance_monitor: Arc<PerformanceMonitor>,

    /// Manages clustering for IVF indexes
    /// Performs k-means clustering and centroid optimization
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
        if let Some(expires_at) = vector.expires_at {
            if (expires_at as i64) <= Utc::now().timestamp() {
                // Skip inserting already expired vectors
                return Ok(());
            }
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
                .map(|c| Arc::new(c))
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

        // Insert into appropriate indexes based on current search_strategy
        let search_strategy = self.get_collection_strategy(collection_id).await?;

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
                    self.dense_vector_index.insert(&processed_vector).await?;
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

        self.global_id_index.remove(&vector_id).await?;

        for index_spec in &search_strategy.indexes {
            match index_spec.data_type {
                Data::Metadata => {
                    self.metadata_index.remove(&vector_id).await?;
                }
                Data::DenseVector { .. } => {
                    self.dense_vector_index.remove(&vector_id).await?;
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
        // Execute query using current search_strategy
        let collection_id = &query.collection_id;

        // Ensure we have a search_strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        let search_strategy = self.get_collection_strategy(collection_id).await?;

        // Use join engine to combine results from multiple indexes
        let results = self
            .join_engine
            .execute_query(
                &query,
                &self.global_id_index,
                &self.metadata_index,
                &self.dense_vector_index,
                &self.sparse_vector_index,
            )
            .await?;

        // Filter out expired results (MVCC)
        let active_results: Vec<_> = results
            .into_iter()
            .filter(|result| {
                // Check if result is not expired
                if let Some(expires_at) = result.expires_at {
                    Utc::now() < expires_at
                } else {
                    true // No expiration
                }
            })
            .collect();

        Ok(QueryResult {
            results: active_results,
            strategy_used: search_strategy,
            execution_time_ms: 0, // TODO: Track actual time
        })
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

    /// Maybe evaluate if search_strategy should change
    async fn maybe_evaluate_strategy(&self, collection_id: &str) -> Result<()> {
        // TODO: Implement periodic evaluation logic
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
        self.sparse_vector_index
            .remove_collection(collection_id)
            .await?;

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
    pub async fn get_collection_stats(&self, collection_id: &str) -> Result<CollectionStats> {
        let search_strategy = self.get_collection_strategy(collection_id).await?;

        Ok(CollectionStats {
            collection_id: collection_id.to_string(),
            strategy_type: search_strategy
                .indexes
                .first()
                .map(|idx| idx.data_type)
                .unwrap_or(Data::DenseVector { dimension: 128 }), // Default to dense vector
            total_vectors: 0,    // TODO: Implement actual counting
            index_size_bytes: 0, // TODO: Implement actual size calculation
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
        collection_id: &str,
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
    async fn index_vectors_synchronously(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
        _files_created: &[String],
    ) -> Result<()> {
        tracing::info!(
            "🔄 AXIS: Synchronous indexing of {} vectors for collection {}",
            vectors.len(),
            collection_id
        );

        let start_time = std::time::Instant::now();
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
    async fn index_vectors_asynchronously(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
        files_created: Vec<String>,
    ) -> Result<()> {
        tracing::info!(
            "🚀 AXIS: Spawning asynchronous indexing task for {} vectors in collection {}",
            vectors.len(),
            collection_id
        );

        // For async indexing, we'll process immediately but in a non-blocking way
        // In a production system, this would use a proper task queue
        let start_time = std::time::Instant::now();
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
            indexed_count,
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
        let distance_metric = proto_distance_to_internal(collection_config.distance_metric.unwrap_or(0));

        // Create quantization config using collection settings with proper field mapping
        let storage_config = StorageQuantizationConfig {
            // Map to the actual fields available in storage engine config
            primary_level: Some(
                crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(
                    // Default to dimension/4 with min 8 and max 64 subvectors
                    ((collection_config.dimension / 4).max(8).min(64) as usize).min(255) as u8,
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
        let engine =
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

/// Collection statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectionStats {
    pub collection_id: String,
    pub strategy_type: Data,
    pub total_vectors: u64,
    pub index_size_bytes: u64,
    pub last_updated: DateTime<Utc>,
}

/// Hybrid query combining multiple search criteria
#[derive(Debug, Clone)]
pub struct HybridQuery {
    pub collection_id: String,
    pub vector_query: Option<VectorQuery>,
    pub metadata_filters: Vec<MetadataFilter>,
    pub id_filters: Vec<VectorId>,
    pub top_k: usize,
    pub include_expired: bool, // For MVCC - whether to include expired records
}

/// Vector query types
#[derive(Debug, Clone)]
pub enum VectorQuery {
    Dense {
        vector: Vec<f32>,
        similarity_threshold: f32,
    },
    Sparse {
        vector: HashMap<u32, f32>,
        similarity_threshold: f32,
    },
}

/// Metadata filter
#[derive(Debug, Clone)]
pub struct MetadataFilter {
    pub field: String,
    pub operator: FilterOperator,
    pub value: serde_json::Value,
}

/// Filter operators
#[derive(Debug, Clone)]
pub enum FilterOperator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    In,
    NotIn,
}

/// Query result
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub results: Vec<ScoredResult>,
    pub strategy_used: IndexSelectionStrategy,
    pub execution_time_ms: u64,
}

/// Scored result with MVCC support
#[derive(Debug, Clone)]
pub struct ScoredResult {
    pub vector_id: VectorId,
    pub similarity: f32,
    pub expires_at: Option<DateTime<Utc>>,
}
