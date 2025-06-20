// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AXIS Index Manager - Central coordinator for adaptive indexing

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{
    AdaptiveIndexEngine, AxisConfig, IndexMigrationEngine, IndexStrategy, IndexType,
    MigrationDecision, PerformanceMonitor,
};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::index::{DenseVectorIndex, GlobalIdIndex, JoinEngine, MetadataIndex, SparseVectorIndex};

/// Central manager for AXIS with adaptive capabilities
pub struct AxisIndexManager {
    /// Core index components
    global_id_index: Arc<GlobalIdIndex>,
    metadata_index: Arc<MetadataIndex>,
    dense_vector_index: Arc<DenseVectorIndex>,
    sparse_vector_index: Arc<SparseVectorIndex>,
    join_engine: Arc<JoinEngine>,

    /// Adaptive intelligence components
    adaptive_engine: Arc<AdaptiveIndexEngine>,
    migration_engine: Arc<IndexMigrationEngine>,
    performance_monitor: Arc<PerformanceMonitor>,

    /// Collection-specific configurations
    collection_strategies: Arc<RwLock<HashMap<CollectionId, IndexStrategy>>>,

    /// Active migrations
    active_migrations: Arc<RwLock<HashMap<CollectionId, MigrationStatus>>>,

    /// Configuration and metrics
    config: AxisConfig,
    metrics: Arc<RwLock<AxisMetrics>>,
}

/// Status of ongoing migrations
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    pub migration_id: uuid::Uuid,
    pub from_strategy: IndexStrategy,
    pub to_strategy: IndexStrategy,
    pub start_time: DateTime<Utc>,
    pub progress_percentage: f64,
    pub estimated_completion: Option<DateTime<Utc>>,
}

/// AXIS metrics
#[derive(Debug, Clone, Default)]
pub struct AxisMetrics {
    pub total_migrations: u64,
    pub successful_migrations: u64,
    pub failed_migrations: u64,
    pub average_migration_time_ms: u64,
    pub total_collections_managed: u64,
    pub total_vectors_indexed: u64,
}

impl AxisIndexManager {
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

        Ok(Self {
            global_id_index,
            metadata_index,
            dense_vector_index,
            sparse_vector_index,
            join_engine,
            adaptive_engine,
            migration_engine,
            performance_monitor,
            collection_strategies: Arc::new(RwLock::new(HashMap::new())),
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
            config,
            metrics: Arc::new(RwLock::new(AxisMetrics::default())),
        })
    }

    /// Insert a vector with adaptive indexing
    pub async fn insert(&self, vector: VectorRecord) -> Result<()> {
        let collection_id = &vector.collection_id;

        // Ensure we have a strategy for this collection
        self.ensure_collection_strategy(collection_id).await?;

        // Check if vector is expired (MVCC support)
        if let Some(expires_at) = vector.expires_at {
            if Utc::now() >= expires_at {
                // Skip inserting already expired vectors
                return Ok(());
            }
        }

        // Insert into appropriate indexes based on current strategy
        let strategy = self.get_collection_strategy(collection_id).await?;

        // Always insert into global ID index
        self.global_id_index
            .insert(vector.id.clone(), collection_id, &vector)
            .await?;

        // Insert into other indexes based on strategy
        for index_type in &strategy.secondary_indexes {
            match index_type {
                IndexType::Metadata => {
                    self.metadata_index.insert(&vector).await?;
                }
                IndexType::DenseVector => {
                    self.dense_vector_index.insert(&vector).await?;
                }
                IndexType::SparseVector => {
                    self.sparse_vector_index.insert(&vector).await?;
                }
                _ => {} // Handle other index types
            }
        }

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_vectors_indexed += 1;

        // Check if we should evaluate strategy change
        self.maybe_evaluate_strategy(collection_id).await?;

        Ok(())
    }

    /// Delete a vector (soft delete with expires_at)
    pub async fn delete(&self, collection_id: &CollectionId, vector_id: VectorId) -> Result<()> {
        // For MVCC, we don't actually delete - we set expires_at to now
        // This is handled by the storage layer creating a tombstone

        // Remove from indexes
        let strategy = self.get_collection_strategy(collection_id).await?;

        self.global_id_index.remove(&vector_id).await?;

        for index_type in &strategy.secondary_indexes {
            match index_type {
                IndexType::Metadata => {
                    self.metadata_index.remove(&vector_id).await?;
                }
                IndexType::DenseVector => {
                    self.dense_vector_index.remove(&vector_id).await?;
                }
                IndexType::SparseVector => {
                    self.sparse_vector_index.remove(&vector_id).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Query vectors using adaptive indexes
    pub async fn query(&self, query: HybridQuery) -> Result<QueryResult> {
        // Execute query using current strategy
        let collection_id = &query.collection_id;
        let strategy = self.get_collection_strategy(collection_id).await?;

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
            strategy_used: strategy,
            execution_time_ms: 0, // TODO: Track actual time
        })
    }

    /// Analyze collection and trigger migration if beneficial
    pub async fn analyze_and_optimize(&self, collection_id: &CollectionId) -> Result<()> {
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
                println!("AXIS: Initiating migration for collection {} from {:?} to {:?} (estimated improvement: {:.2}%)",
                    collection_id, from.primary_index_type, to.primary_index_type, estimated_improvement * 100.0);

                // Start migration
                self.start_migration(collection_id, from, to).await?;
            }
            MigrationDecision::Stay { reason } => {
                println!(
                    "AXIS: Collection {} staying with current strategy: {}",
                    collection_id, reason
                );
            }
        }

        Ok(())
    }

    /// Start migration to new indexing strategy
    async fn start_migration(
        &self,
        collection_id: &CollectionId,
        from: IndexStrategy,
        to: IndexStrategy,
    ) -> Result<()> {
        let migration_id = uuid::Uuid::new_v4();

        // Record migration start
        let mut migrations = self.active_migrations.write().await;
        migrations.insert(
            collection_id.clone(),
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
        let collection_id = collection_id.clone();
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
                    // Update strategy
                    let mut strategies = collection_strategies.write().await;
                    strategies.insert(collection_id.clone(), migration_result.new_strategy);

                    // Update metrics
                    let mut metrics = metrics.write().await;
                    metrics.total_migrations += 1;
                    metrics.successful_migrations += 1;
                    metrics.average_migration_time_ms = (metrics.average_migration_time_ms
                        * (metrics.total_migrations - 1)
                        + migration_result.duration_ms)
                        / metrics.total_migrations;

                    println!(
                        "AXIS: Migration completed for collection {} in {}ms",
                        collection_id, migration_result.duration_ms
                    );
                }
                Err(e) => {
                    let mut metrics = metrics.write().await;
                    metrics.total_migrations += 1;
                    metrics.failed_migrations += 1;

                    eprintln!(
                        "AXIS: Migration failed for collection {}: {}",
                        collection_id, e
                    );
                }
            }
        });

        Ok(())
    }

    /// Ensure collection has an indexing strategy
    async fn ensure_collection_strategy(&self, collection_id: &CollectionId) -> Result<()> {
        let strategies = self.collection_strategies.read().await;
        if strategies.contains_key(collection_id) {
            return Ok(());
        }
        drop(strategies);

        // Analyze collection and select initial strategy
        let characteristics = self
            .adaptive_engine
            .analyze_collection(collection_id)
            .await?;
        let strategy = self
            .adaptive_engine
            .recommend_strategy(&characteristics)
            .await?;

        let mut strategies = self.collection_strategies.write().await;
        strategies.insert(collection_id.clone(), strategy);

        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.total_collections_managed += 1;

        Ok(())
    }

    /// Get current strategy for collection
    async fn get_collection_strategy(&self, collection_id: &CollectionId) -> Result<IndexStrategy> {
        let strategies = self.collection_strategies.read().await;
        strategies
            .get(collection_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No strategy found for collection {}", collection_id))
    }

    /// Maybe evaluate if strategy should change
    async fn maybe_evaluate_strategy(&self, _collection_id: &CollectionId) -> Result<()> {
        // TODO: Implement periodic evaluation logic
        // For now, we'll rely on explicit analyze_and_optimize calls
        Ok(())
    }

    /// Get migration status for a collection
    pub async fn get_migration_status(
        &self,
        collection_id: &CollectionId,
    ) -> Option<MigrationStatus> {
        let migrations = self.active_migrations.read().await;
        migrations.get(collection_id).cloned()
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> AxisMetrics {
        self.metrics.read().await.clone()
    }
}

/// Hybrid query combining multiple search criteria
#[derive(Debug, Clone)]
pub struct HybridQuery {
    pub collection_id: CollectionId,
    pub vector_query: Option<VectorQuery>,
    pub metadata_filters: Vec<MetadataFilter>,
    pub id_filters: Vec<VectorId>,
    pub k: usize,
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
    pub strategy_used: IndexStrategy,
    pub execution_time_ms: u64,
}

/// Scored result with MVCC support
#[derive(Debug, Clone)]
pub struct ScoredResult {
    pub vector_id: VectorId,
    pub score: f32,
    pub expires_at: Option<DateTime<Utc>>,
}
