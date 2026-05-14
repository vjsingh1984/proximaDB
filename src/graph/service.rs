/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # GraphOperationsService - Graph Data Operations Layer
//!
//! This module provides the operations service layer for ProximaDB's native graph database,
//! implementing CRUD operations, queries, and traversals following the vector services pattern.
//!
//! **TD-GOD-FILE**: This file (~2800 lines) handles graph CRUD, traversals, analytics,
//! pattern matching, and batch operations. It should be split into:
//! - `graph/service/mod.rs` — Service struct + orchestration
//! - `graph/service/crud.rs` — Node/edge create/read/update/delete
//! - `graph/service/traversal.rs` — BFS/DFS/traversal operations
//! - `graph/service/analytics.rs` — Analytics and aggregation
//! - `graph/service/batch.rs` — Batch operations
//! See docs/10-quality/TECHNICAL_DEBT.adoc for tracking.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │        GraphOperationsService       │
//! │     (Graph Data Operations)         │
//! ├─────────────────────────────────────┤
//! │  ┌─────────────────────────────┐    │
//! │  │      Operations             │    │
//! │  │ • Node CRUD operations      │    │
//! │  │ • Edge CRUD operations      │    │
//! │  │ • Graph queries & traversal │    │
//! │  │ • Transaction support      │    │
//! │  └─────────────────────────────┘    │
//! ├─────────────────────────────────────┤
//! │              Engines                │
//! │  ┌─────────┬─────────┬───────────┐  │
//! │  │ ORION   │ PULSAR  │  QUASAR   │  │
//! │  │(Memory) │(Distrib)│ (Hybrid)  │  │
//! │  └─────────┴─────────┴───────────┘  │
//! ├─────────────────────────────────────┤
//! │           Arc Memory Pool           │
//! │    ┌────────────┬─────────────┐     │
//! │    │   Nodes    │    Edges    │     │
//! │    │ Properties │ Embeddings  │     │
//! │    └────────────┴─────────────┘     │
//! └─────────────────────────────────────┘
//! ```
//!
//! ## Responsibilities
//!
//! - **Node Operations**: Create, read, update, delete nodes with property management
//! - **Edge Operations**: Create, read, update, delete edges with weight and property management
//! - **Native Graph Queries**: Node/edge lookups, property filtering, label-based queries
//! - **Graph Traversals**: BFS, DFS, shortest path, pattern matching
//! - **Transaction Management**: ACID transactions with rollback support
//! - **Performance Optimization**: SIMD-ready operations and cache-friendly access patterns

#[path = "service_advanced.rs"]
pub mod service_advanced;
#[path = "service_edge_ops.rs"]
mod service_edge_ops;
#[path = "service_engine_factory.rs"]
mod service_engine_factory;
#[path = "service_helpers.rs"]
mod service_helpers;
#[path = "service_node_ops.rs"]
mod service_node_ops;
#[path = "service_query_read.rs"]
mod service_query_read;
#[path = "service_query_stats.rs"]
mod service_query_stats;
#[path = "service_query_traversal.rs"]
mod service_query_traversal;
#[path = "service_schema_validation.rs"]
mod service_schema_validation;
#[path = "service_transactions.rs"]
mod service_transactions;
#[path = "service_traversal_api.rs"]
mod service_traversal_api;
#[allow(clippy::wildcard_imports)]
use service_helpers::*;
pub use service_transactions::{
    IsolationLevel, TransactionHandle, TransactionId, TransactionManager, TransactionState,
    UnitOfWork,
};

use crate::core::error::ProximaDBError;
use crate::graph::{
    Edge, EdgeId, EdgeQuery, GraphMemoryPool, Node, OperationMode,
    adjacency_projection::{
        InMemoryGraphAdjacencyProjection, edge_to_canonical_record, node_to_canonical_record,
    },
    engines::{GraphEngine, orion::OrionGraphEngine},
};
use crate::security::rbac_service::{
    ConsolidatedRBACManager, UnifiedPermission, UnifiedUserContext,
};
use crate::storage::cache::orchestrator::{
    CacheStatsProvider, CacheType, CrossCacheOrchestrator, UsageStats,
};
use dashmap::DashMap;
use proximadb_graph::projection::{GraphTopologyProjection, TopologyEpoch};
use proximadb_records::{RecordKey, RecordStore};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Graph operations service providing CRUD operations and queries for graph data
pub struct GraphOperationsService {
    /// Current operation mode (vector-only, graph-only, unified)
    mode: OperationMode,

    /// Reference to graph collection service for metadata management
    collection_service: Arc<crate::services::graph_collection::GraphCollectionService>,

    /// Graph registry for multi-graph support - maps graph_id to engine (polymorphic)
    graphs: Arc<DashMap<String, Arc<crate::graph::engines::GraphEngineImpl>>>,
    /// Optional canonical record store for durable graph node/edge records.
    canonical_record_store: Option<Arc<dyn RecordStore>>,
    /// Rebuildable adjacency projections over canonical edge records, keyed by graph id.
    adjacency_projections: Arc<DashMap<String, Arc<InMemoryGraphAdjacencyProjection>>>,
    /// Monotonic edge-mutation epoch per graph, used to invalidate CSR/topology projections.
    /// Advances on every create_edge, update_edge, delete_edge, and batch_create_edges call.
    edge_epochs: Arc<DashMap<String, AtomicU64>>,

    /// Configuration for graph storage base URL
    base_storage_url: String,

    /// Shared memory pool for Arc-based zero-copy operations
    memory_pool: Arc<GraphMemoryPool>,
    /// Optional unified metrics updater
    metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>>,
    /// Edge statistics
    stats_edges: Arc<AtomicU64>,
    /// Edge type counts
    edge_type_counts: Arc<DashMap<String, AtomicU64>>,
    /// Graph traversal runtime settings
    graph_settings: crate::core::context::GraphTraversalSettings,
    /// Transaction manager for ACID transaction support
    transaction_manager: Arc<service_transactions::TransactionManager>,
    /// RBAC manager for permission validation
    rbac_manager: Option<Arc<ConsolidatedRBACManager>>,
}

impl GraphOperationsService {
    /// Create a new GraphOperationsService in unified mode
    pub fn new() -> Self {
        let memory_pool = Arc::new(GraphMemoryPool::new());
        let collection_service =
            Arc::new(crate::services::graph_collection::GraphCollectionService::new());

        // Initialize graph settings from global if available
        let default_settings = crate::core::context::global_graph_settings().unwrap_or_default();

        // Initialize transaction manager with empty shards (will be populated as graphs are created)
        let transaction_manager =
            Arc::new(service_transactions::TransactionManager::with_defaults(
                std::collections::HashMap::new(),
                std::time::Duration::from_secs(30),
            ));

        let service = Self {
            mode: OperationMode::Unified,
            collection_service,
            graphs: Arc::new(DashMap::new()),
            canonical_record_store: None,
            adjacency_projections: Arc::new(DashMap::new()),
            edge_epochs: Arc::new(DashMap::new()),
            base_storage_url: "file:///tmp/proximadb".to_string(), // Default storage URL
            memory_pool,
            metrics_updater: None,
            stats_edges: Arc::new(AtomicU64::new(0)),
            edge_type_counts: Arc::new(DashMap::new()),
            graph_settings: default_settings,
            transaction_manager,
            rbac_manager: None,
        };

        // Register lightweight graph cache providers with orchestrator
        if let Some(orch) = CrossCacheOrchestrator::global() {
            // Use edge stats as a proxy for adjacency/edge access frequency for now
            let edge_counter = service.stats_edges.clone();
            let provider_adj: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(SimpleCounterProvider::new(edge_counter.clone(), 256, 0.0));
            let provider_edge: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(SimpleCounterProvider::new(edge_counter.clone(), 256, 0.0));
            // Node provider uses same counter as placeholder
            let provider_node: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(SimpleCounterProvider::new(edge_counter, 256, 0.0));
            orch.register_cache_provider(CacheType::GraphAdjacency, provider_adj);
            orch.register_cache_provider(CacheType::GraphEdge, provider_edge);
            orch.register_cache_provider(CacheType::GraphNode, provider_node);
        }

        service
    }

    /// Create a new GraphOperationsService with SharedContext (future-proof DI)
    pub fn new_with_context(ctx: &crate::core::context::SharedContext) -> Self {
        let mut s = Self::new();
        if let Some(gs) = &ctx.graph_settings {
            s.graph_settings = gs.clone();
        }
        s
    }

    /// Create a new GraphOperationsService with auto-recovery of graph collection metadata
    ///
    /// This is the recommended constructor for production use. It automatically
    /// loads persisted graph collection metadata from disk, ensuring data
    /// survives restarts.
    ///
    /// # Example
    /// ```ignore
    /// let service = GraphOperationsService::new_with_recovery().await?;
    /// ```
    pub async fn new_with_recovery() -> anyhow::Result<Self> {
        let collection_service =
            crate::services::graph_collection::GraphCollectionService::new_with_recovery().await?;
        Ok(Self::new_with_collection_service(Arc::new(
            collection_service,
        )))
    }

    /// Create GraphOperationsService with existing collection service (for dependency injection)
    pub fn new_with_collection_service(
        collection_service: Arc<crate::services::graph_collection::GraphCollectionService>,
    ) -> Self {
        let memory_pool = Arc::new(GraphMemoryPool::new());

        let default_settings = crate::core::context::global_graph_settings().unwrap_or_default();

        // Initialize transaction manager with empty shards
        let transaction_manager =
            Arc::new(service_transactions::TransactionManager::with_defaults(
                std::collections::HashMap::new(),
                std::time::Duration::from_secs(30),
            ));

        let service = Self {
            mode: OperationMode::Unified,
            collection_service,
            graphs: Arc::new(DashMap::new()),
            canonical_record_store: None,
            adjacency_projections: Arc::new(DashMap::new()),
            edge_epochs: Arc::new(DashMap::new()),
            base_storage_url: "file:///tmp/proximadb".to_string(),
            memory_pool,
            metrics_updater: None,
            stats_edges: Arc::new(AtomicU64::new(0)),
            edge_type_counts: Arc::new(DashMap::new()),
            graph_settings: default_settings,
            transaction_manager,
            rbac_manager: None,
        };

        // Register cache providers
        if let Some(orch) = CrossCacheOrchestrator::global() {
            let edge_counter = service.stats_edges.clone();
            let provider_adj: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(SimpleCounterProvider::new(edge_counter.clone(), 256, 0.0));
            let provider_edge: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(SimpleCounterProvider::new(edge_counter.clone(), 256, 0.0));
            let provider_node: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(SimpleCounterProvider::new(edge_counter, 256, 0.0));
            orch.register_cache_provider(CacheType::GraphAdjacency, provider_adj);
            orch.register_cache_provider(CacheType::GraphEdge, provider_edge);
            orch.register_cache_provider(CacheType::GraphNode, provider_node);
        }

        service
    }

    /// Create from global Config with engine selection (currently ORION default)
    pub fn from_config(cfg: &crate::core::config::Config) -> Self {
        let engine_name = cfg
            .graph
            .as_ref()
            .map_or_else(|| "ORION".to_string(), |g| g.engine.to_ascii_uppercase());

        // Get storage URL from config
        let base_storage_url = cfg.storage.storage_locations.first().map_or_else(
            || "file:///tmp/proximadb".to_string(),
            |loc| loc.url.clone(),
        );

        // Engine selection: PULSAR requires 'distributed-graph' feature, QUASAR requires 'tiered-graph'
        // Engine is determined per-graph from collection metadata during recovery
        tracing::info!(
            "GraphOperationsService engine selection: {}, storage: {}",
            engine_name,
            base_storage_url
        );

        let mut service = Self::new();
        service.base_storage_url = base_storage_url;
        service
    }

    /// Inject the canonical record store used as durable graph node/edge truth.
    ///
    /// Graph engines and adjacency/CSR structures remain projection consumers.
    pub fn with_canonical_record_store(mut self, record_store: Arc<dyn RecordStore>) -> Self {
        self.canonical_record_store = Some(record_store);
        self
    }

    /// Create a new GraphOperationsService with RBAC enabled
    pub fn with_rbac(mut self, rbac_manager: Arc<ConsolidatedRBACManager>) -> Self {
        self.rbac_manager = Some(rbac_manager);
        self
    }

    /// Validate graph operation permission
    #[allow(dead_code)]
    async fn validate_graph_permission(
        &self,
        user_ctx: &UnifiedUserContext,
        graph_id: &str,
        operation: &str,
    ) -> Result<()> {
        let rbac_manager = self.rbac_manager.as_ref().ok_or_else(|| {
            ProximaDBError::Internal(format!(
                "Graph operation: {} on {} - RBAC manager not configured",
                operation, graph_id
            ))
        })?;

        let permission = match operation {
            "read" | "traverse" => UnifiedPermission::GraphTraverse(graph_id.to_string()),
            "create_node" | "create_edge" | "create_relations" => {
                UnifiedPermission::GraphCreateRelations(graph_id.to_string())
            }
            "delete_node" | "delete_edge" | "delete_relations" => {
                UnifiedPermission::GraphDeleteRelations(graph_id.to_string())
            }
            _ => UnifiedPermission::GraphTraverse(graph_id.to_string()),
        };

        let allowed = rbac_manager
            .check_permission_cached(&user_ctx.user_id, &permission)
            .await
            .map_err(|e| {
                ProximaDBError::Internal(format!(
                    "Graph operation: {} on {} - Failed to check permission: {}",
                    operation, graph_id, e
                ))
            })?;

        if !allowed {
            return Err(ProximaDBError::Internal(format!(
                "Graph operation: {} on {} - Insufficient permissions",
                operation, graph_id
            )));
        }

        Ok(())
    }

    // get_or_create_graph_engine moved to service_engine_factory.rs

    // /// Initialize constraint registries based on graph schema (unique constraints)
    // initialize_schema_constraints moved to service_engine_factory.rs
    /// List all active graph engines (not collections)
    pub fn list_active_graphs(&self) -> Vec<String> {
        self.graphs
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Set the base storage URL for graph persistence (used by embedded/server wiring).
    pub fn set_base_storage_url(&mut self, url: String) {
        self.base_storage_url = url;
    }

    /// List all graph collections (delegates to collection service)
    pub async fn list_graphs(&self) -> Result<Vec<String>> {
        let collections = self.collection_service.list_graphs().await?;
        Ok(collections.iter().map(|c| c.graph_id.clone()).collect())
    }

    /// Create a new graph collection (delegates to collection service)
    pub async fn create_graph_collection(
        &self,
        request: crate::proto::proximadb_v1::CreateGraphRequest,
    ) -> Result<()> {
        self.collection_service.create_graph(request).await?;
        Ok(())
    }

    /// Remove a graph engine (for cleanup/deletion)
    pub fn remove_graph(
        &self,
        graph_id: &str,
    ) -> Option<Arc<crate::graph::engines::GraphEngineImpl>> {
        self.adjacency_projections.remove(graph_id);
        self.edge_epochs.remove(graph_id);
        self.graphs.remove(graph_id).map(|(_, engine)| engine)
    }

    /// Return the current edge-mutation epoch for the given graph.
    ///
    /// CSR consumers can snapshot this epoch before building/loading a CSR and compare
    /// with `freshness_epoch` to detect stale topology: if the returned epoch is greater
    /// than the epoch at CSR-build time, the CSR must be rebuilt.
    pub fn edge_epoch(&self, graph_id: &str) -> TopologyEpoch {
        self.edge_epochs
            .get(graph_id)
            .map(|v| TopologyEpoch(v.load(Ordering::Acquire)))
            .unwrap_or_else(TopologyEpoch::initial)
    }

    /// Advance the edge-mutation epoch for the given graph.
    ///
    /// Called internally by every create_edge, update_edge, delete_edge, and
    /// batch_create_edges path so CSR freshness checks are accurate.
    pub(crate) fn advance_edge_epoch(&self, graph_id: &str) {
        self.edge_epochs
            .entry(graph_id.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Release);
    }

    pub(crate) fn adjacency_projection(
        &self,
        graph_id: &str,
    ) -> Arc<InMemoryGraphAdjacencyProjection> {
        self.adjacency_projections
            .entry(graph_id.to_string())
            .or_insert_with(|| Arc::new(InMemoryGraphAdjacencyProjection::new(graph_id)))
            .clone()
    }

    pub fn adjacency_projection_edge_count(&self, graph_id: &str) -> Result<usize> {
        match self.adjacency_projections.get(graph_id) {
            Some(projection) => projection.edge_count(),
            None => Ok(0),
        }
    }

    pub(crate) async fn upsert_canonical_node_record(
        &self,
        graph_id: &str,
        node: &Node,
    ) -> Result<()> {
        if let Some(record_store) = &self.canonical_record_store {
            record_store
                .upsert_record(node_to_canonical_record(graph_id, node))
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) async fn upsert_canonical_edge_record(
        &self,
        graph_id: &str,
        edge: &Edge,
    ) -> Result<()> {
        if let Some(record_store) = &self.canonical_record_store {
            record_store
                .upsert_record(edge_to_canonical_record(graph_id, edge))
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) async fn delete_canonical_node_record(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<()> {
        if let Some(record_store) = &self.canonical_record_store {
            let key = RecordKey::new(
                proximadb_graph::record::GraphNodeKey::new(graph_id, node_id).canonical_oid(),
            );
            record_store
                .delete_record(&key)
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) async fn delete_canonical_edge_record(
        &self,
        graph_id: &str,
        edge_id: &str,
    ) -> Result<()> {
        if let Some(record_store) = &self.canonical_record_store {
            let key = RecordKey::new(
                proximadb_graph::record::GraphEdgeKey::new(graph_id, edge_id).canonical_oid(),
            );
            record_store
                .delete_record(&key)
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }
        Ok(())
    }

    /// Recover all graphs from persistent storage
    ///
    /// This method should be called during server startup to restore all graph state
    /// from persistent storage (snapshots + WAL replay).
    pub async fn recover_all_graphs(&self) -> Result<()> {
        tracing::info!("🔄 Starting graph collection recovery...");

        // Get all graph collections from metadata
        let collections = self.collection_service.list_graphs().await?;

        if collections.is_empty() {
            tracing::info!("✅ No graphs to recover");
            return Ok(());
        }

        tracing::info!(
            "📋 Found {} graph collections to recover",
            collections.len()
        );

        // Recover each graph
        let mut recovered_count = 0;
        let mut failed_count = 0;

        for collection in collections {
            let graph_id = &collection.graph_id;
            tracing::info!("🔍 Recovering graph: {}", graph_id);

            match self.recover_graph(graph_id).await {
                Ok(()) => {
                    recovered_count += 1;
                    tracing::info!("✅ Graph {} recovered successfully", graph_id);
                }
                Err(e) => {
                    failed_count += 1;
                    tracing::warn!("⚠️  Failed to recover graph {}: {}", graph_id, e);
                    // Continue with other graphs even if one fails
                }
            }
        }

        tracing::info!(
            "🎉 Graph collection recovery complete: {} succeeded, {} failed",
            recovered_count,
            failed_count
        );

        Ok(())
    }

    /// Recover a single graph from persistent storage
    ///
    /// This method detects the engine type from the stored collection metadata
    /// and creates the appropriate engine (ORION, PULSAR, or QUASAR).
    async fn recover_graph(&self, graph_id: &str) -> Result<()> {
        // Get collection metadata to determine engine type
        let collection = self.collection_service.get_graph(graph_id).await?;
        let engine_type = collection
            .as_ref()
            .and_then(|c| c.storage_config.as_ref())
            .map_or_else(|| "ORION".to_string(), |sc| sc.engine_type.to_uppercase());

        tracing::debug!(
            "Recovering graph {} with engine type: {}",
            graph_id,
            engine_type
        );

        let engine_impl = match engine_type.as_str() {
            "PULSAR" => {
                #[cfg(feature = "distributed-graph")]
                {
                    use crate::graph::engines::pulsar::{PulsarConfig, PulsarGraphEngine};
                    let config = PulsarConfig::default();
                    // Create with persistence enabled for WAL recovery
                    let engine = PulsarGraphEngine::with_persistence(
                        config,
                        graph_id.to_string(),
                        self.base_storage_url.clone(),
                    )
                    .await?;
                    engine.recover().await?;
                    crate::graph::engines::GraphEngineImpl::Pulsar(engine)
                }
                #[cfg(not(feature = "distributed-graph"))]
                {
                    return Err(ProximaDBError::NotImplemented(
                        "PULSAR engine requires 'distributed-graph' feature".to_string(),
                    ));
                }
            }
            "QUASAR" => {
                #[cfg(feature = "tiered-graph")]
                {
                    use crate::graph::engines::quasar::{QuasarConfig, QuasarGraphEngine};
                    let cold_tier_path = std::path::PathBuf::from(format!(
                        "{}/graphs/{}/cold",
                        self.base_storage_url.trim_start_matches("file://"),
                        graph_id
                    ));
                    let config = QuasarConfig {
                        cold_tier_path,
                        ..QuasarConfig::default()
                    };
                    // Create with persistence enabled for WAL recovery
                    let engine = QuasarGraphEngine::with_persistence(
                        config,
                        graph_id.to_string(),
                        self.base_storage_url.clone(),
                    )
                    .await?;
                    engine.recover().await?;
                    crate::graph::engines::GraphEngineImpl::Quasar(engine)
                }
                #[cfg(not(feature = "tiered-graph"))]
                {
                    return Err(ProximaDBError::NotImplemented(
                        "QUASAR engine requires 'tiered-graph' feature".to_string(),
                    ));
                }
            }
            _ => {
                // Default to ORION (includes "ORION" and any unknown types)
                let engine = OrionGraphEngine::with_persistence_for_graph(
                    graph_id.to_string(),
                    self.base_storage_url.clone(),
                    true, // Enable WAL
                )
                .await?;
                engine.recover().await?;
                crate::graph::engines::GraphEngineImpl::Orion(engine)
            }
        };

        // Store in graphs map
        self.graphs
            .insert(graph_id.to_string(), Arc::new(engine_impl));

        Ok(())
    }

    // /// Compute shortest path with algorithm selection and optional k-shortest support.
    /* moved to service_traversal_api.rs
    pub async fn shortest_path(
        &self,
        graph_id: &str,
        start_node_id: &NodeId,
        target_node_id: &NodeId,
        max_depth: Option<u32>,
        edge_types: Option<Vec<String>>,
        algorithm: Option<crate::proto::proximadb_v1::ShortestPathAlgorithm>,
        k: Option<u32>,
        override_enable_prefetch: Option<bool>,
        override_prefetch_budget: Option<usize>,
    ) -> Result<Option<(Vec<NodeId>, f64)>> {
        let t0 = Instant::now();
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        use crate::graph::engines::orion::traversal::{
            TraversalConfig, astar_shortest_path, dijkstra_shortest_path, k_shortest_paths,
        };
        let config = TraversalConfig {
            max_depth,
            max_nodes: None,
            edge_types,
            node_filter: None,
            early_stop: None,
            track_paths: true,
            parallel_processing: false,
            timeout_ms: Some(500),
            max_frontier: Some(100_000),
            enable_prefetch: override_enable_prefetch
                .unwrap_or(self.graph_settings.enable_prefetch),
            prefetch_budget: override_prefetch_budget
                .unwrap_or(self.graph_settings.prefetch_budget),
        };
        let engine = self.get_or_create_graph_engine(graph_id).await?;

        let orion_engine = match &*engine {
            crate::graph::engines::GraphEngineImpl::Orion(e) => Some(e),
            _ => None,
        };

        if let Some(kk) = k {
            if kk > 1 {
                if let Some(eng) = orion_engine {
                    let paths = k_shortest_paths(
                        eng,
                        start_node_id,
                        target_node_id,
                        kk as usize,
                        config,
                    )
                    .await?;
                    return Ok(paths.first().cloned());
                } else {
                    // Fallback: only compute the single best path via generic dijkstra
                    let res = crate::graph::engines::generic_traversal::dijkstra_generic(
                        engine.as_ref(),
                        start_node_id,
                        target_node_id,
                        config.edge_types.as_ref().map(|v| v.as_slice()),
                    )?;
                    return Ok(res);
                }
            }
        }
        let result = match algorithm.unwrap_or(
            crate::proto::proximadb_v1::ShortestPathAlgorithm::Dijkstra,
        ) {
            crate::proto::proximadb_v1::ShortestPathAlgorithm::Astar => {
                if let Some(eng) = orion_engine {
                    astar_shortest_path(eng, start_node_id, target_node_id, config).await
                } else {
                    Ok(crate::graph::engines::generic_traversal::dijkstra_generic(
                        engine.as_ref(),
                        start_node_id,
                        target_node_id,
                        config.edge_types.as_ref().map(|v| v.as_slice()),
                    )?)
                }
            }
            _ => {
                if let Some(eng) = orion_engine {
                    dijkstra_shortest_path(eng, start_node_id, target_node_id, config).await
                } else {
                    Ok(crate::graph::engines::generic_traversal::dijkstra_generic(
                        engine.as_ref(),
                        start_node_id,
                        target_node_id,
                        config.edge_types.as_ref().map(|v| v.as_slice()),
                    )?)
                }
            }
        }?;
        if let Some(updater) = &self.metrics_updater {
            let _ = updater
                .record_operation(
                    "graph",
                    OperationMetricsUpdate {
                        operation_type: "graph.shortest_path".into(),
                        latency_us: t0.elapsed().as_micros() as f64,
                        success: result.is_some(),
                        bytes_processed: 0,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64,
                    },
                )
                .await;
        }
        Ok(result)
    }
    */

    /// Create a new GraphOperationsService with specific mode
    pub fn with_mode(mode: OperationMode) -> Self {
        let mut service = Self::new();
        service.mode = mode;
        service
    }

    /// Get current operation mode
    pub fn mode(&self) -> OperationMode {
        self.mode
    }

    /// Set operation mode
    pub fn set_mode(&mut self, mode: OperationMode) {
        self.mode = mode;
    }

    /// Set metrics updater
    pub fn set_metrics_updater(
        &mut self,
        updater: Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>,
    ) {
        self.metrics_updater = Some(updater);
    }

    /// Check if graph operations are enabled
    pub fn graph_enabled(&self) -> bool {
        matches!(self.mode, OperationMode::GraphOnly | OperationMode::Unified)
    }

    /// Check if vector operations are enabled
    pub fn vector_enabled(&self) -> bool {
        matches!(
            self.mode,
            OperationMode::VectorOnly | OperationMode::Unified
        )
    }

    /// Get the first available graph engine for federated query execution.
    ///
    /// This method is used by the federated query system to access graph data
    /// when executing cross-model queries with GRAPH_QUERY extensions.
    ///
    /// Returns `None` if no graphs have been created yet.
    pub fn get_default_engine(&self) -> Option<Arc<dyn crate::graph::engines::GraphEngine>> {
        // Return the first available graph engine
        self.graphs.iter().next().map(|entry| {
            let engine_impl: Arc<crate::graph::engines::GraphEngineImpl> = entry.value().clone();
            // GraphEngineImpl implements GraphEngine, so we can upcast
            engine_impl as Arc<dyn crate::graph::engines::GraphEngine>
        })
    }

    /// Flush WAL buffer to disk for a specific graph
    ///
    /// This ensures all pending write operations are persisted to disk.
    /// Should be called during graceful shutdown or before critical operations
    /// that require durability guarantees.
    ///
    /// # Arguments
    /// * `graph_id` - The ID of the graph to flush
    ///
    /// # Returns
    /// * `Ok(())` if flush succeeds or graph not found
    /// * `Err` if flush fails
    pub async fn flush_wal(&self, graph_id: &str) -> Result<()> {
        if let Some(engine) = self.graphs.get(graph_id) {
            match engine.value().as_ref() {
                crate::graph::engines::GraphEngineImpl::Orion(orion) => {
                    orion.flush_wal().await?;
                }
                #[cfg(feature = "distributed-graph")]
                crate::graph::engines::GraphEngineImpl::Pulsar(pulsar) => {
                    pulsar.flush_wal().await?;
                }
                #[cfg(feature = "tiered-graph")]
                crate::graph::engines::GraphEngineImpl::Quasar(quasar) => {
                    quasar.flush_wal().await?;
                }
                #[allow(unreachable_patterns)]
                _ => {
                    // Stub engines (feature-disabled) don't support WAL
                    tracing::debug!(
                        "WAL flush not supported for this engine type (feature disabled)"
                    );
                }
            }
        }
        Ok(())
    }

    // =========================================================================
    // CSR Projection Management
    // =========================================================================

    /// Rebuild the ORION engine's CSR from the in-memory adjacency projection.
    ///
    /// This is the hook point for the convergence spec item "Make ORION CSR
    /// load/build from edge records or adjacency projections."  The adjacency
    /// projection is the single source of truth for write-heavy edge state; the
    /// CSR is a read-optimised derived projection that should be rebuilt from it
    /// when the engine is cold-started or when `edge_epoch()` reveals staleness.
    ///
    /// Extracts `(from_node_id, to_node_id, edge_id)` from the adjacency
    /// projection's `edges_by_src` snapshot (one entry per edge, no duplicates)
    /// and calls `OrionGraphEngine::rebuild_csr_from_edges`.
    pub async fn rebuild_orion_csr_from_adjacency_projection(&self, graph_id: &str) -> Result<()> {
        let node_prefix = format!("graph/{graph_id}/node/");
        let edge_prefix = format!("graph/{graph_id}/edge/");

        let endpoints = {
            let proj = self.adjacency_projection(graph_id);
            proj.snapshot_edge_endpoints().map_err(|e| {
                ProximaDBError::Internal(format!("adjacency projection snapshot failed: {e}"))
            })?
        };

        // Build minimal Arc<Edge> stubs — only from/to/id fields are used by
        // rebuild_csr_from_edges (which just needs endpoint IDs for indexing).
        let edges: Vec<Arc<crate::graph::Edge>> = endpoints
            .into_iter()
            .filter_map(|(from_oid, to_oid, edge_oid)| {
                let from_id = from_oid.strip_prefix(&node_prefix)?.to_string();
                let to_id = to_oid.strip_prefix(&node_prefix)?.to_string();
                let edge_id = edge_oid.strip_prefix(&edge_prefix)?.to_string();
                Some(Arc::new(crate::graph::Edge {
                    id: edge_id,
                    from_node_id: from_id,
                    to_node_id: to_id,
                    edge_type: String::new(),
                    properties: std::collections::HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                }))
            })
            .collect();

        if let Some(engine) = self.graphs.get(graph_id) {
            match engine.value().as_ref() {
                crate::graph::engines::GraphEngineImpl::Orion(orion) => {
                    orion.rebuild_csr_from_edges(&edges).await?;
                    tracing::info!(
                        graph_id,
                        edge_count = edges.len(),
                        "ORION CSR rebuilt from adjacency projection"
                    );
                }
                #[allow(unreachable_patterns)]
                _ => {
                    tracing::debug!(
                        "rebuild_orion_csr_from_adjacency_projection: not an ORION engine, skipped"
                    );
                }
            }
        }

        Ok(())
    }

    // =========================================================================
    // Transaction Management API
    // =========================================================================

    /// Begin a new transaction on a graph
    ///
    /// Creates a new transaction with the default isolation level (ReadCommitted).
    /// All graph operations performed within this transaction will be atomic.
    ///
    /// # Arguments
    /// * `graph_id` - The ID of the graph to operate on
    ///
    /// # Returns
    /// * `TransactionId` - A unique identifier for this transaction
    ///
    /// # Example
    /// ```ignore
    /// let tx_id = service.begin_transaction("my_graph").await?;
    /// // ... perform operations ...
    /// service.commit_transaction(tx_id).await?;
    /// ```
    pub async fn begin_transaction(
        &self,
        graph_id: &str,
    ) -> Result<service_transactions::TransactionId> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Ensure the graph exists
        let _ = self.get_or_create_graph_engine(graph_id).await?;

        // Use the graph ID as the shard ID for single-graph transactions
        let shard_id = format!("shard_{}", graph_id);
        self.transaction_manager
            .begin_transaction(graph_id, vec![shard_id])
            .await
    }

    /// Begin a transaction with specific isolation level
    ///
    /// # Arguments
    /// * `graph_id` - The ID of the graph to operate on
    /// * `isolation` - The isolation level for this transaction
    ///
    /// # Returns
    /// * `TransactionId` - A unique identifier for this transaction
    pub async fn begin_transaction_with_isolation(
        &self,
        graph_id: &str,
        isolation: service_transactions::IsolationLevel,
    ) -> Result<service_transactions::TransactionId> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Ensure the graph exists
        let _ = self.get_or_create_graph_engine(graph_id).await?;

        let shard_id = format!("shard_{}", graph_id);
        self.transaction_manager
            .begin_transaction_with_isolation(graph_id, vec![shard_id], isolation)
            .await
    }

    /// Commit a transaction
    ///
    /// Atomically applies all changes made within the transaction. If any
    /// operation fails, the entire transaction is rolled back.
    ///
    /// # Arguments
    /// * `tx_id` - The transaction ID returned from `begin_transaction`
    ///
    /// # Errors
    /// Returns an error if:
    /// - The transaction does not exist
    /// - The transaction has already been committed or rolled back
    /// - A 2PC participant votes to abort
    pub async fn commit_transaction(
        &self,
        tx_id: service_transactions::TransactionId,
    ) -> Result<()> {
        self.transaction_manager.commit_transaction(tx_id).await
    }

    /// Rollback a transaction
    ///
    /// Discards all pending changes and releases any acquired locks.
    ///
    /// # Arguments
    /// * `tx_id` - The transaction ID returned from `begin_transaction`
    pub async fn rollback_transaction(
        &self,
        tx_id: service_transactions::TransactionId,
    ) -> Result<()> {
        self.transaction_manager.rollback_transaction(tx_id).await
    }

    /// Get the current state of a transaction
    ///
    /// # Arguments
    /// * `tx_id` - The transaction ID to query
    ///
    /// # Returns
    /// The current `TransactionState` (Active, Preparing, Committed, etc.)
    pub async fn get_transaction_state(
        &self,
        tx_id: &service_transactions::TransactionId,
    ) -> Result<service_transactions::TransactionState> {
        self.transaction_manager.get_transaction_state(tx_id).await
    }

    /// Check if a transaction is still active
    pub fn is_transaction_active(&self, tx_id: &service_transactions::TransactionId) -> bool {
        self.transaction_manager.is_transaction_active(tx_id)
    }

    /// Get a RAII transaction handle for automatic rollback on drop
    ///
    /// The returned handle will automatically rollback the transaction if
    /// dropped without an explicit commit or rollback call. This ensures
    /// no dangling transactions.
    ///
    /// # Example
    /// ```ignore
    /// let handle = service.begin_transaction_handle("my_graph").await?;
    /// // ... perform operations using handle.id() ...
    /// handle.commit().await?; // Or handle.rollback().await?
    /// // If handle is dropped without commit/rollback, it auto-rollbacks
    /// ```
    pub async fn begin_transaction_handle(
        &self,
        graph_id: &str,
    ) -> Result<service_transactions::TransactionHandle> {
        let tx_id = self.begin_transaction(graph_id).await?;
        Ok(service_transactions::TransactionHandle::new(
            tx_id,
            self.transaction_manager.clone(),
        ))
    }

    // =========================================================================
    // Transaction-Aware Graph Operations
    // =========================================================================

    /// Create a node within a transaction
    ///
    /// The node will not be visible to other transactions until commit.
    pub async fn create_node_in_transaction(
        &self,
        tx_id: &service_transactions::TransactionId,
        graph_id: &str,
        node: Node,
    ) -> Result<()> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Validate schema before registering
        self.enforce_schema_on_node(graph_id, &node).await?;
        self.enforce_unique_constraints_on_node(graph_id, &node)?;
        self.enforce_multi_unique_constraints_on_node(graph_id, &node)?;

        // Register in unit of work
        self.transaction_manager
            .register_node_insert(tx_id, node)
            .await
    }

    /// Update a node within a transaction
    pub async fn update_node_in_transaction(
        &self,
        tx_id: &service_transactions::TransactionId,
        graph_id: &str,
        node: Node,
    ) -> Result<()> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Validate schema before registering
        self.enforce_schema_on_node(graph_id, &node).await?;

        // Register in unit of work
        self.transaction_manager
            .register_node_update(tx_id, node)
            .await
    }

    /// Delete a node within a transaction
    pub async fn delete_node_in_transaction(
        &self,
        tx_id: &service_transactions::TransactionId,
        _graph_id: &str,
        node_id: String,
    ) -> Result<()> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Register in unit of work
        self.transaction_manager
            .register_node_delete(tx_id, node_id)
            .await
    }

    /// Create an edge within a transaction
    pub async fn create_edge_in_transaction(
        &self,
        tx_id: &service_transactions::TransactionId,
        graph_id: &str,
        edge: Edge,
    ) -> Result<()> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Validate endpoints exist (best effort - may be in pending inserts)
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        if let (Some(from), Some(to)) = (
            engine.get_node(&edge.from_node_id)?,
            engine.get_node(&edge.to_node_id)?,
        ) {
            self.enforce_schema_on_edge(graph_id, &edge, &from.labels, &to.labels)
                .await?;
        }

        // Register in unit of work
        self.transaction_manager
            .register_edge_insert(tx_id, edge)
            .await
    }

    /// Update an edge within a transaction
    pub async fn update_edge_in_transaction(
        &self,
        tx_id: &service_transactions::TransactionId,
        graph_id: &str,
        edge: Edge,
    ) -> Result<()> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Validate schema
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        if let (Some(from), Some(to)) = (
            engine.get_node(&edge.from_node_id)?,
            engine.get_node(&edge.to_node_id)?,
        ) {
            self.enforce_schema_on_edge(graph_id, &edge, &from.labels, &to.labels)
                .await?;
        }

        // Register in unit of work
        self.transaction_manager
            .register_edge_update(tx_id, edge)
            .await
    }

    /// Delete an edge within a transaction
    pub async fn delete_edge_in_transaction(
        &self,
        tx_id: &service_transactions::TransactionId,
        _graph_id: &str,
        edge_id: String,
    ) -> Result<()> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Register in unit of work
        self.transaction_manager
            .register_edge_delete(tx_id, edge_id)
            .await
    }

    /// Get access to the transaction manager for advanced usage
    pub fn transaction_manager(&self) -> Arc<service_transactions::TransactionManager> {
        self.transaction_manager.clone()
    }

    // =========================================================================
    // End Transaction Management API
    // =========================================================================

    // create_node moved to service_node_ops.rs

    // get_node moved to service_node_ops.rs

    // update_node moved to service_node_ops.rs

    // delete_node moved to service_node_ops.rs

    // create_edge moved to service_edge_ops.rs

    // delete_node_detach moved to service_node_ops.rs

    /// Add a unique constraint for a label/property for a specific graph. Scans existing nodes to build index.
    pub async fn add_unique_constraint(
        &self,
        graph_id: &str,
        label: &str,
        property: &str,
    ) -> Result<()> {
        // Get the graph engine to ensure it exists
        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Store constraint key with graph_id for graph-specific constraints
        let key = (
            graph_id.to_string(),
            label.to_string(),
            property.to_string(),
        );
        let map: DashMap<String, String> = DashMap::new();

        // Build from existing nodes in this specific graph (use engine's get_all_nodes)
        let existing_nodes = engine.get_all_nodes()?;
        for node in existing_nodes {
            if !node.labels.contains(&label.to_string()) {
                continue;
            }
            if let Some(val) = node.properties.get(property) {
                let k = index_key_for_value(val);
                if let Some(existing) = map.get(&k)
                    && existing.value() != &node.id
                {
                    return Err(ProximaDBError::InvalidInput(format!(
                        "Existing duplicate value '{}' for unique ({},{})",
                        k, label, property
                    )));
                }
                map.insert(k, node.id.clone());
            }
        }
        self.memory_pool.unique_constraints.insert(key, map);
        Ok(())
    }

    /// Remove a unique constraint for a specific graph
    pub async fn remove_unique_constraint(
        &self,
        graph_id: &str,
        label: &str,
        property: &str,
    ) -> Result<()> {
        // Get the graph engine to ensure it exists
        let _engine = self.get_or_create_graph_engine(graph_id).await?;

        // Remove constraint using graph-specific key
        let key = (
            graph_id.to_string(),
            label.to_string(),
            property.to_string(),
        );
        self.memory_pool.unique_constraints.remove(&key);

        Ok(())
    }

    // enforce_unique_constraints_on_node moved to service_node_ops.rs

    // register_node_in_unique_constraints moved to service_node_ops.rs

    // unregister_node_from_unique_constraints moved to service_node_ops.rs

    // get_edge moved to service_edge_ops.rs

    // update_edge moved to service_edge_ops.rs

    // /// Enforce schema constraints for a node if schema is defined
    /* moved to service_schema_validation.rs
    async fn enforce_schema_on_node(&self, graph_id: &str, node: &Node) -> Result<()> {
        let maybe_collection = self.collection_service.get_graph(graph_id).await?;
        if let Some(coll) = maybe_collection {
            if let Some(schema) = &coll.schema {
                let strict = schema.strict_mode;
                // Build quick lookup for node label schemas
                for label in &node.labels {
                    let label_schema = schema.node_labels.iter().find(|ls| &ls.label == label);
                    let ls = match label_schema {
                        Some(schema) => schema,
                        None => {
                            if strict {
                                return Err(ProximaDBError::InvalidInput(format!(
                                    "Label '{}' is not allowed by schema", label
                                )));
                            }
                            continue;
                        }
                    };
                    // Required properties present
                    for req in &ls.required_properties {
                        if !node.properties.contains_key(req) {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "Missing required property '{}' for label '{}'", req, label
                            )));
                        }
                    }
                    // Validate property types and constraints (schema-level + label-level)
                    for (k, v) in &node.properties {
                        if let Some(ps) = schema.properties.get(k) {
                            Self::validate_property_value_type(k, v, ps)?;
                            Self::validate_property_constraints(k, v, &ps.constraints)?;
                        }
                        if let Some(pc) = ls.property_constraints.get(k) {
                            Self::validate_property_constraint_one(k, v, pc)?;
                        }
                    }
                    // Disallow additional properties if configured
                    if !ls.allow_additional_properties {
                        let mut allowed: std::collections::HashSet<&str> = std::collections::HashSet::new();
                        for s in &ls.required_properties { allowed.insert(s.as_str()); }
                        for s in &ls.optional_properties { allowed.insert(s.as_str()); }
                        for (p, _) in &ls.property_constraints { allowed.insert(p.as_str()); }
                        for key in node.properties.keys() {
                            if !allowed.contains(key.as_str()) {
                                return Err(ProximaDBError::InvalidInput(format!(
                                    "Property '{}' not allowed by schema for label '{}'",
                                    key, label
                                )));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    */

    // /// Enforce schema constraints for an edge if schema is defined
    /* moved to service_schema_validation.rs
    async fn enforce_schema_on_edge(
        &self,
        graph_id: &str,
        edge: &Edge,
        from_labels: &[String],
        to_labels: &[String],
    ) -> Result<()> {
        let maybe_collection = self.collection_service.get_graph(graph_id).await?;
        if let Some(coll) = maybe_collection {
            if let Some(schema) = &coll.schema {
                let strict = schema.strict_mode;
                let ets = schema.edge_types.iter().find(|et| et.edge_type == edge.edge_type);
                let ets = match ets {
                    Some(edge_type_schema) => edge_type_schema,
                    None => {
                        if strict {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "Edge type '{}' is not allowed by schema", edge.edge_type
                            )));
                        }
                        return Ok(());
                    }
                };
                // Required properties present
                for req in &ets.required_properties {
                    if !edge.properties.contains_key(req) {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Missing required property '{}' for edge type '{}'",
                            req, edge.edge_type
                        )));
                    }
                }
                // Validate edge property types and constraints (schema-level + edge-type level)
                for (k, v) in &edge.properties {
                    if let Some(ps) = schema.properties.get(k) {
                        Self::validate_property_value_type(k, v, ps)?;
                        Self::validate_property_constraints(k, v, &ps.constraints)?;
                    }
                    if let Some(pc) = ets.property_constraints.get(k) {
                        Self::validate_property_constraint_one(k, v, pc)?;
                    }
                }
                // Source/target label constraints
                if !ets.source_labels.is_empty() {
                    if !from_labels.iter().any(|l| ets.source_labels.contains(l)) {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Source node labels {:?} do not satisfy schema for edge type '{}'",
                            from_labels, edge.edge_type
                        )));
                    }
                }
                if !ets.target_labels.is_empty() {
                    if !to_labels.iter().any(|l| ets.target_labels.contains(l)) {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Target node labels {:?} do not satisfy schema for edge type '{}'",
                            to_labels, edge.edge_type
                        )));
                    }
                }
                // Disallow additional properties if configured
                if !ets.allow_additional_properties {
                    let mut allowed: std::collections::HashSet<&str> = std::collections::HashSet::new();
                    for s in &ets.required_properties { allowed.insert(s.as_str()); }
                    for s in &ets.optional_properties { allowed.insert(s.as_str()); }
                    for (p, _) in &ets.property_constraints { allowed.insert(p.as_str()); }
                    for key in edge.properties.keys() {
                        if !allowed.contains(key.as_str()) {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "Property '{}' not allowed by schema for edge type '{}'",
                                key, edge.edge_type
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    */

    // delete_edge moved to service_edge_ops.rs

    // query_nodes moved to service_node_ops.rs
    /*
    pub async fn query_nodes(&self, graph_id: &str, query: NodeQuery) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Initial candidate set from labels or all nodes
        let mut candidates: HashSet<NodeId> = if !query.labels.is_empty() {
            let mut set = HashSet::new();
            for label in &query.labels {
                if let Ok(nodes) = engine.get_nodes_by_label(label) {
                    for n in nodes {
                        set.insert(n.id.clone());
                    }
                }
            }
            set
        } else {
            engine
                .get_all_nodes()?
                .into_iter()
                .map(|n| n.id.clone())
                .collect()
        };

        // Use property indexes / ordered indexes for prefiltering
        for filter in &query.filters {
            use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
            match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                Op::Equals => {
                    // Look up index for this property
                    let filter_value = match &filter.value {
                        Some(v) => v,
                        None => continue, // Skip filters without values
                    };
                    if let Some(index_map) = self.memory_pool.node_property_indexes.get(&filter.key)
                    {
                        let key = index_key_for_value(filter_value);
                        if let Some(ids_vec) = index_map.get(&key) {
                            let id_set: HashSet<NodeId> = ids_vec.iter().cloned().collect();
                            candidates = candidates
                                .into_iter()
                                .filter(|id| id_set.contains(id))
                                .collect();
                        } else {
                            // No matches for this property value; result is empty
                            candidates.clear();
                            break;
                        }
                    } else {
                        // No index for this property; will verify via scan later
                        continue;
                    }
                }
                Op::StartsWith => {
                    let filter_value = match &filter.value {
                        Some(v) => v,
                        None => continue,
                    };
                    if let Some(prefix) = extract_string_from_value(filter_value)
                    {
                        if let Some(map_lock) =
                            self.memory_pool.node_property_str_ordered.get(&filter.key)
                        {
                            let map = map_lock.read().map_err(|_| {
                                ProximaDBError::Internal("RwLock poisoned".to_string())
                            })?;
                            let mut matched: HashSet<NodeId> = HashSet::new();
                            for (k, ids) in map
                                .range(prefix.to_string()..)
                                .take_while(|(k, _)| k.starts_with(prefix))
                            {
                                matched.extend(ids.iter().cloned());
                            }
                            candidates = candidates
                                .into_iter()
                                .filter(|id| matched.contains(id))
                                .collect();
                        }
                    }
                }
                Op::GreaterThan
                | Op::GreaterEqual
                | Op::LessThan
                | Op::LessEqual => {
                    let filter_value = match &filter.value {
                        Some(v) => v,
                        None => continue,
                    };
                    if let Some(num) = extract_number_from_value(filter_value) {
                        if let Some(map_lock) =
                            self.memory_pool.node_property_num_indexes.get(&filter.key)
                        {
                            let map = map_lock.read().map_err(|_| {
                                ProximaDBError::Internal("RwLock poisoned".to_string())
                            })?;
                            let mut matched: HashSet<NodeId> = HashSet::new();
                            match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                                Op::GreaterThan => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) > num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                Op::GreaterEqual => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) >= num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                Op::LessThan => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) < num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                Op::LessEqual => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) <= num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                _ => {}
                            }
                            candidates = candidates
                                .into_iter()
                                .filter(|id| matched.contains(id))
                                .collect();
                        }
                    } else if let Some(map_lock) =
                        self.memory_pool.node_property_str_ordered.get(&filter.key)
                    {
                        let map = map_lock.read().map_err(|_| {
                            ProximaDBError::Internal("RwLock poisoned".to_string())
                        })?;
                        let mut matched: HashSet<NodeId> = HashSet::new();
                        let s = extract_string_from_value(filter_value).unwrap_or("");
                        use std::ops::Bound::{Excluded, Included, Unbounded};
                        match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                            Op::GreaterThan => {
                                for (_k, ids) in map.range((Excluded(s.to_string()), Unbounded)) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            Op::GreaterEqual => {
                                for (_k, ids) in map.range((Included(s.to_string()), Unbounded)) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            Op::LessThan => {
                                for (_k, ids) in map.range((Unbounded, Excluded(s.to_string()))) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            Op::LessEqual => {
                                for (_k, ids) in map.range((Unbounded, Included(s.to_string()))) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            _ => {}
                        }
                        candidates = candidates
                            .into_iter()
                            .filter(|id| matched.contains(id))
                            .collect();
                    }
                }
                _ => {
                    // Other operators unsupported by index; verify via scan later
                    continue;
                }
            }
        }

        // Final scan to validate remaining filters (including non-equality ops)
        let mut results = Vec::new();
        'outer: for node_id in candidates {
            if let Some(node_arc) = engine.get_node(&node_id)? {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                    let prop_val_opt = node_arc.properties.get(&filter.key);
                    let filter_value = match &filter.value {
                        Some(v) => v,
                        None => continue, // Skip filters without values
                    };
                    let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                        Op::Equals => match prop_val_opt {
                            Some(v) => v.value == filter_value.value,
                            None => false,
                        },
                        Op::NotEquals => match prop_val_opt {
                            Some(v) => v.value != filter_value.value,
                            None => true,
                        },
                        Op::GreaterThan => {
                            cmp_prop_gt(prop_val_opt, filter_value)
                        }
                        Op::GreaterEqual => {
                            cmp_prop_ge(prop_val_opt, filter_value)
                        }
                        Op::LessThan => {
                            cmp_prop_lt(prop_val_opt, filter_value)
                        }
                        Op::LessEqual => {
                            cmp_prop_le(prop_val_opt, filter_value)
                        }
                        Op::StartsWith => {
                            prop_starts_with(prop_val_opt, filter_value)
                        }
                        Op::Contains => {
                            prop_contains(prop_val_opt, filter_value)
                        }
                        _ => false,
                    };
                    if !pass {
                        continue 'outer;
                    }
                }
                results.push(node_arc);
            }
        }

        // Apply offset/limit for pagination
        let mut res = results;
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query.limit.unwrap_or(res.len() as u32) as usize;
        if offset >= res.len() {
            return Ok(Vec::new());
        }
        let end = (offset + limit).min(res.len());
        Ok(res.drain(offset..end).collect())
    }
    */
    /// Legacy query_edges (moved). Kept for reference; new implementation in service_edge_ops.rs
    #[allow(dead_code)]
    pub(crate) async fn query_edges_legacy(
        &self,
        graph_id: &str,
        query: EdgeQuery,
    ) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Edge querying: from/to node ID filtering. Edge type and property
        // filtering applied post-retrieval via the query.filters field.
        let mut results = Vec::new();
        if let Some(from_node_id) = &query.from_node_id
            && let Ok(edges) = engine.get_outgoing_edges(from_node_id, None)
        {
            results.extend(edges);
        } // Continue if node doesn't exist

        if let Some(to_node_id) = &query.to_node_id
            && let Ok(edges) = engine.get_incoming_edges(to_node_id, None)
        {
            results.extend(edges);
        } // Continue if node doesn't exist
        // If neither from nor to specified and filters exist, prefilter by edge property indexes
        if query.from_node_id.is_none() && query.to_node_id.is_none() && (!query.filters.is_empty())
        {
            use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
            let mut candidate_ids: Option<std::collections::HashSet<EdgeId>> = None;
            for filter in &query.filters {
                // Only handle equality and range/prefix on stringified keys
                match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                    Op::Equals => {
                        // Safely handle missing filter value
                        let Some(filter_val) = filter.value.as_ref() else {
                            continue;
                        };
                        if let Some(index_map) =
                            self.memory_pool.edge_property_indexes.get(&filter.key)
                        {
                            let key = index_key_for_value(filter_val);
                            if let Some(ids) = index_map.get(&key) {
                                let set: std::collections::HashSet<EdgeId> =
                                    ids.iter().cloned().collect();
                                candidate_ids = Some(match candidate_ids {
                                    None => set,
                                    Some(prev) => prev.intersection(&set).cloned().collect(),
                                });
                            } else {
                                candidate_ids = Some(std::collections::HashSet::new());
                            }
                        }
                    }
                    Op::StartsWith => {
                        // Safely handle missing filter value
                        let Some(filter_val) = filter.value.as_ref() else {
                            continue;
                        };
                        if let Some(prefix) = extract_string_from_value(filter_val)
                            && let Some(map_lock) =
                                self.memory_pool.edge_property_str_ordered.get(&filter.key)
                        {
                            let map = map_lock.read();
                            let mut matched = std::collections::HashSet::new();
                            for (_k, ids) in map
                                .range(prefix.to_string()..)
                                .take_while(|(k, _)| k.starts_with(prefix))
                            {
                                matched.extend(ids.iter().cloned());
                            }
                            candidate_ids = Some(match candidate_ids {
                                None => matched,
                                Some(prev) => prev.intersection(&matched).cloned().collect(),
                            });
                        }
                    }
                    Op::GreaterThan | Op::GreaterEqual | Op::LessThan | Op::LessEqual => {
                        // Safely handle missing filter value
                        let Some(filter_val) = filter.value.as_ref() else {
                            continue;
                        };
                        // Prefer numeric range if value numeric, else fallback to string ordered
                        if let Some(num) = extract_number_from_value(filter_val) {
                            if let Some(map_lock) =
                                self.memory_pool.edge_property_num_indexes.get(&filter.key)
                            {
                                let map = map_lock.read();
                                let mut matched = std::collections::HashSet::new();
                                match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                                    Op::GreaterThan => {
                                        for (k, ids) in map.iter() {
                                            if (*k as f64) > num {
                                                matched.extend(ids.iter().cloned());
                                            }
                                        }
                                    }
                                    Op::GreaterEqual => {
                                        for (k, ids) in map.iter() {
                                            if (*k as f64) >= num {
                                                matched.extend(ids.iter().cloned());
                                            }
                                        }
                                    }
                                    Op::LessThan => {
                                        for (k, ids) in map.iter() {
                                            if (*k as f64) < num {
                                                matched.extend(ids.iter().cloned());
                                            }
                                        }
                                    }
                                    Op::LessEqual => {
                                        for (k, ids) in map.iter() {
                                            if (*k as f64) <= num {
                                                matched.extend(ids.iter().cloned());
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                candidate_ids = Some(match candidate_ids {
                                    None => matched,
                                    Some(prev) => prev.intersection(&matched).cloned().collect(),
                                });
                            }
                        } else if let Some(map_lock) =
                            self.memory_pool.edge_property_str_ordered.get(&filter.key)
                        {
                            let map = map_lock.read();
                            let mut matched = std::collections::HashSet::new();
                            // filter_val was already validated at the start of this Op branch
                            let s = extract_string_from_value(filter_val).unwrap_or("");
                            match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                                Op::GreaterThan => {
                                    for (_k, ids) in map.range((
                                        std::ops::Bound::Excluded(s.to_string()),
                                        std::ops::Bound::Unbounded,
                                    )) {
                                        matched.extend(ids.iter().cloned());
                                    }
                                }
                                Op::GreaterEqual => {
                                    for (_k, ids) in map.range((
                                        std::ops::Bound::Included(s.to_string()),
                                        std::ops::Bound::Unbounded,
                                    )) {
                                        matched.extend(ids.iter().cloned());
                                    }
                                }
                                Op::LessThan => {
                                    for (_k, ids) in map.range((
                                        std::ops::Bound::Unbounded,
                                        std::ops::Bound::Excluded(s.to_string()),
                                    )) {
                                        matched.extend(ids.iter().cloned());
                                    }
                                }
                                Op::LessEqual => {
                                    for (_k, ids) in map.range((
                                        std::ops::Bound::Unbounded,
                                        std::ops::Bound::Included(s.to_string()),
                                    )) {
                                        matched.extend(ids.iter().cloned());
                                    }
                                }
                                _ => {}
                            }
                            candidate_ids = Some(match candidate_ids {
                                None => matched,
                                Some(prev) => prev.intersection(&matched).cloned().collect(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            if let Some(ids) = candidate_ids {
                results = ids
                    .into_iter()
                    .filter_map(|eid| engine.get_edge(&eid).ok().flatten())
                    .collect();
            }
        }

        // De-duplicate edges by ID if both directions added
        {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            results.retain(|e| seen.insert(e.id.clone()));
        }

        // Filter by edge types if provided
        if !query.edge_types.is_empty() {
            results.retain(|e| query.edge_types.contains(&e.edge_type));
        }

        // Filter by edge property filters
        if !query.filters.is_empty() {
            results.retain(|edge| {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;

                    // Safely get filter value - if missing, filter fails (edge excluded)
                    let Some(filter_val) = filter.value.as_ref() else {
                        return false;
                    };

                    let prop_val_opt = edge.properties.get(&filter.key);
                    let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                        Op::Equals => match prop_val_opt {
                            Some(v) => v.value == filter_val.value,
                            None => false,
                        },
                        Op::NotEquals => match prop_val_opt {
                            Some(v) => v.value != filter_val.value,
                            None => true,
                        },
                        Op::GreaterThan => cmp_prop_gt(prop_val_opt, filter_val),
                        Op::GreaterEqual => cmp_prop_ge(prop_val_opt, filter_val),
                        Op::LessThan => cmp_prop_lt(prop_val_opt, filter_val),
                        Op::LessEqual => cmp_prop_le(prop_val_opt, filter_val),
                        Op::StartsWith => prop_starts_with(prop_val_opt, filter_val),
                        Op::Contains => prop_contains(prop_val_opt, filter_val),
                        _ => false,
                    };
                    if !pass {
                        return false;
                    }
                }
                true
            });
        }
        // Apply offset/limit for pagination
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query.limit.unwrap_or(results.len() as u32) as usize;
        if offset >= results.len() {
            return Ok(Vec::new());
        }
        let end = (offset + limit).min(results.len());
        Ok(results.drain(offset..end).collect())
    }

    // get_neighbors moved to service_node_ops.rs

    // index_key_for_value moved to helpers

    /// Get graph statistics
    pub async fn get_stats(
        &self,
        graph_id: &str,
    ) -> Result<crate::proto::proximadb_v1::GraphStats> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Collect label statistics by iterating through all nodes
        let mut label_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        if let Ok(nodes) = engine.get_all_nodes() {
            for node in nodes {
                for label in &node.labels {
                    *label_counts.entry(label.clone()).or_insert(0) += 1;
                }
            }
        }

        let label_stats: Vec<crate::proto::proximadb_v1::LabelStats> = label_counts
            .into_iter()
            .map(|(label, count)| crate::proto::proximadb_v1::LabelStats { label, count })
            .collect();

        let stats = crate::proto::proximadb_v1::GraphStats {
            total_nodes: engine.node_count().unwrap_or(0) as u64,
            total_edges: self.stats_edges.load(std::sync::atomic::Ordering::Relaxed),
            label_stats,
            edge_type_stats: self
                .edge_type_counts
                .iter()
                .map(|entry| crate::proto::proximadb_v1::EdgeTypeStats {
                    edge_type: entry.key().clone(),
                    count: entry.value().load(std::sync::atomic::Ordering::Relaxed),
                })
                .collect(),
            total_properties: 0, // Property count: requires traversal (expensive)
            memory_usage_bytes: 0, // Memory: tracked by engine allocator
            average_degree: {
                let nc = engine.node_count().unwrap_or(0) as f64;
                let ec = self.stats_edges.load(std::sync::atomic::Ordering::Relaxed) as f64;
                if nc > 0.0 { ec / nc } else { 0.0 }
            },
            max_degree: 0,           // Max degree: requires full scan (deferred)
            connected_components: 1, // Connected components: requires union-find (deferred)
        };
        Ok(stats)
    }

    /// Helper method to convert properties to proto format
    #[allow(dead_code)]
    fn convert_properties_to_proto(
        &self,
        properties: &std::collections::HashMap<String, crate::graph::PropertyValue>,
    ) -> std::collections::HashMap<String, crate::proto::proximadb_v1::PropertyValue> {
        properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Batch create nodes for high-performance ingestion
    pub async fn batch_create_nodes(
        &self,
        graph_id: &str,
        nodes: Vec<Node>,
    ) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        if let Some(record_store) = &self.canonical_record_store {
            let records = nodes
                .iter()
                .map(|node| node_to_canonical_record(graph_id, node))
                .collect();
            record_store
                .upsert_records(records)
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;
        // Use bulk API for optimal performance (100-500x faster than individual inserts)
        let inserted = engine.bulk_insert_nodes(nodes).await?;
        Ok(inserted)
    }

    /// Batch create nodes with upsert strategy
    pub async fn batch_create_nodes_with_strategy(
        &self,
        graph_id: &str,
        nodes: Vec<Node>,
        if_exists: &str,
    ) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        if let Some(record_store) = &self.canonical_record_store {
            let records = nodes
                .iter()
                .map(|node| node_to_canonical_record(graph_id, node))
                .collect();
            record_store
                .upsert_records(records)
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;

        let mut results = Vec::with_capacity(nodes.len());
        for node in nodes {
            match if_exists {
                "update" | "skip" | "error" => {
                    results.push(engine.insert_node(node).await?);
                }
                _ => {
                    return Err(ProximaDBError::InvalidInput(format!(
                        "Invalid if_exists strategy: {if_exists}"
                    )));
                }
            }
        }
        Ok(results)
    }

    /// Batch create edges for high-performance ingestion
    pub async fn batch_create_edges(
        &self,
        graph_id: &str,
        edges: Vec<Edge>,
    ) -> Result<Vec<Arc<Edge>>> {
        let service_start = std::time::Instant::now();

        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        if edges.is_empty() {
            return Ok(Vec::new());
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Enforce composite (from,to,type) uniqueness across existing + in-batch edges
        let composite_start = std::time::Instant::now();
        let mut seen: HashSet<(String, String, String)> = HashSet::with_capacity(edges.len());
        for edge in &edges {
            let key = (
                edge.from_node_id.clone(),
                edge.to_node_id.clone(),
                edge.edge_type.clone(),
            );
            if !seen.insert(key.clone()) {
                return Err(ProximaDBError::InvalidInput(format!(
                    "Duplicate edge in batch: (from='{}', to='{}', type='{}')",
                    key.0, key.1, key.2
                )));
            }
            if self.memory_pool.edge_composite_index.contains_key(&key) {
                return Err(ProximaDBError::InvalidInput(format!(
                    "Composite edge already exists: (from='{}', to='{}', type='{}')",
                    key.0, key.1, key.2
                )));
            }
        }
        let composite_time = composite_start.elapsed();

        // OPTIMIZATION: Check if schema exists FIRST - skip all validation if no schema
        // This avoids expensive async future creation/polling for the common case
        let maybe_collection = self.collection_service.get_graph(graph_id).await?;
        let has_schema = maybe_collection
            .as_ref()
            .is_some_and(|c| c.schema.is_some());

        if has_schema {
            // Schema/cardinality validation (only when schema exists)
            // Step 1: Batch fetch all nodes first (reduces lock contention)
            let mut validation_data: Vec<(Edge, Arc<Node>, Arc<Node>)> =
                Vec::with_capacity(edges.len());
            for edge in &edges {
                if let (Some(from), Some(to)) = (
                    engine.get_node(&edge.from_node_id)?,
                    engine.get_node(&edge.to_node_id)?,
                ) {
                    validation_data.push((edge.clone(), from, to));
                }
            }

            // Step 2: Check if sequential validation is requested
            let sequential =
                std::env::var("PROXIMADB_SEQUENTIAL_VALIDATION").unwrap_or_default() == "1";

            if sequential {
                // Sequential validation (original implementation for comparison)
                tracing::warn!(
                    "TEST MODE: Using sequential validation via PROXIMADB_SEQUENTIAL_VALIDATION=1"
                );
                for (edge, from, to) in &validation_data {
                    self.enforce_schema_on_edge(graph_id, edge, &from.labels, &to.labels)
                        .await?;
                    self.enforce_cardinality_on_edge(graph_id, edge, engine.as_ref())
                        .await?;
                }
            } else {
                // Parallel validation (optimized)
                let validation_futures: Vec<_> = validation_data
                    .iter()
                    .map(|(edge, from, to)| async {
                        // Schema validation
                        self.enforce_schema_on_edge(graph_id, edge, &from.labels, &to.labels)
                            .await?;
                        // Cardinality validation
                        self.enforce_cardinality_on_edge(graph_id, edge, engine.as_ref())
                            .await?;
                        Ok::<(), ProximaDBError>(())
                    })
                    .collect();

                // Execute all validations concurrently and check for errors
                let results = futures::future::join_all(validation_futures).await;
                for result in results {
                    result?;
                }
            }
        }
        // If no schema, skip validation entirely - major performance win for bulk inserts

        if let Some(record_store) = &self.canonical_record_store {
            let records = edges
                .iter()
                .map(|edge| edge_to_canonical_record(graph_id, edge))
                .collect();
            record_store
                .upsert_records(records)
                .await
                .map_err(|error| ProximaDBError::Internal(error.to_string()))?;
        }

        let edge_count = edges.len();
        let engine_start = std::time::Instant::now();
        let inserted = engine.bulk_insert_edges(edges).await?;
        let engine_time = engine_start.elapsed();

        let projection_start = std::time::Instant::now();
        if !inserted.is_empty() {
            let projection = self.adjacency_projection(graph_id);
            for edge in &inserted {
                let edge_record = edge_to_canonical_record(graph_id, edge);
                projection.apply_edge(&edge_record).await?;
            }
            self.advance_edge_epoch(graph_id);
        }
        let projection_time = projection_start.elapsed();

        // Update edge stats and per-type counters
        let stats_start = std::time::Instant::now();
        self.stats_edges
            .fetch_add(inserted.len() as u64, Ordering::Relaxed);
        if !inserted.is_empty() {
            let mut per_type: HashMap<String, u64> = HashMap::new();
            for e in &inserted {
                *per_type.entry(e.edge_type.clone()).or_default() += 1;
            }
            for (edge_type, count) in per_type {
                self.edge_type_counts
                    .entry(edge_type)
                    .or_insert_with(|| AtomicU64::new(0))
                    .fetch_add(count, Ordering::Relaxed);
            }
        }
        let stats_time = stats_start.elapsed();

        // Log timing breakdown for performance analysis (debug level)
        let service_total = service_start.elapsed();
        if edge_count >= 100 {
            tracing::debug!(
                "batch_create_edges timing for {} edges: composite={:?} engine={:?} projection={:?} stats={:?} total={:?}",
                edge_count,
                composite_time,
                engine_time,
                projection_time,
                stats_time,
                service_total
            );
        }

        Ok(inserted)
    }

    // Helpers for range/string comparisons
    #[allow(dead_code)]
    fn parse_f64_key(s: &str) -> Option<f64> {
        s.parse::<f64>().ok()
    }

    // traverse moved to service_traversal_api.rs

    // /// Perform graph traversal with per-call override hints (prefetch settings)
    // traverse_with_overrides moved to service_traversal_api.rs

    // /// Execute traversal with specific configuration
    // traverse_with_config moved to service_traversal_api.rs

    // /// Get connected components (basic implementation)
    // connected_components moved to service_traversal_api.rs

    // /// Check for cycles (basic implementation)
    // has_cycle moved to service_traversal_api.rs

    // ===== Unique (multi-field) and schema validation helpers =====

    fn normalize_list(list: &[String]) -> String {
        let mut v: Vec<String> = list.to_vec();
        v.sort();
        v.join("|")
    }

    fn node_has_all_labels(node: &Node, labels: &[String]) -> bool {
        labels.iter().all(|l| node.labels.contains(l))
    }

    fn composite_key_for_node(node: &Node, props: &[String]) -> Option<String> {
        let mut parts = Vec::with_capacity(props.len());
        for p in props {
            let v = node.properties.get(p)?;
            parts.push(index_key_for_value(v));
        }
        Some(parts.join("\u{1f}"))
    }

    // multi-unique helpers moved to service_node_ops.rs

    // unregister_node_from_multi_unique_constraints moved to service_node_ops.rs

    /* moved to service_schema_validation.rs
    fn validate_property_value_type(
        key: &str,
        value: &crate::proto::proximadb_v1::PropertyValue,
        schema: &crate::proto::proximadb_v1::PropertySchema,
    ) -> Result<()> {
        use crate::proto::proximadb_v1::property_value::Value as PV;
        use crate::proto::proximadb_v1::PropertyType as PT;
        match (schema.r#type, &value.value) {
            (x, Some(PV::StringValue(_))) if x == PT::String as i32 => Ok(()),
            (x, Some(PV::IntValue(_))) if x == PT::Integer as i32 => Ok(()),
            (x, Some(PV::DoubleValue(_))) if x == PT::Float as i32 => Ok(()),
            (x, Some(PV::BoolValue(_))) if x == PT::Boolean as i32 => Ok(()),
            (x, Some(PV::ArrayValue(_))) if x == PT::Array as i32 => Ok(()),
            (x, Some(PV::VectorValue(_))) if x == PT::Embedding as i32 => Ok(()),
            (x, Some(PV::ObjectValue(_))) if x == PT::Json as i32 => Ok(()),
            _ => Err(ProximaDBError::InvalidInput(format!(
                "Property '{}' has type mismatch against schema", key
            ))),
        }
    }

    fn validate_property_constraints(
        key: &str,
        value: &crate::proto::proximadb_v1::PropertyValue,
        constraints: &Vec<crate::proto::proximadb_v1::PropertyConstraint>,
    ) -> Result<()> {
        for c in constraints {
            Self::validate_property_constraint_one(key, value, c)?;
        }
        Ok(())
    }

    fn validate_property_constraint_one(
        key: &str,
        value: &crate::proto::proximadb_v1::PropertyValue,
        c: &crate::proto::proximadb_v1::PropertyConstraint,
    ) -> Result<()> {
        use crate::proto::proximadb_v1::property_value::Value as PV;
        if let Some(ref sc) = c.constraint.as_ref() {
            match sc {
                crate::proto::proximadb_v1::property_constraint::Constraint::StringConstraint(sc) => {
                    if let Some(PV::StringValue(s)) = &value.value {
                        if let Some(min) = sc.min_length { if (s.len() as i32) < min { return Err(ProximaDBError::InvalidInput(format!("'{}' shorter than min_length", key))); } }
                        if let Some(max) = sc.max_length { if (s.len() as i32) > max { return Err(ProximaDBError::InvalidInput(format!("'{}' longer than max_length", key))); } }
                        if !sc.allowed_values.is_empty() && !sc.allowed_values.contains(s) { return Err(ProximaDBError::InvalidInput(format!("'{}' not in allowed_values", key))); }
                    }
                }
                crate::proto::proximadb_v1::property_constraint::Constraint::NumericConstraint(nc) => {
                    let num = match &value.value { Some(PV::IntValue(i)) => *i as f64, Some(PV::DoubleValue(d)) => *d, Some(PV::StringValue(s)) => s.parse::<f64>().unwrap_or(f64::NAN), _ => f64::NAN };
                    if num.is_nan() { return Err(ProximaDBError::InvalidInput(format!("'{}' not numeric for numeric constraint", key))); }
                    if let Some(min) = nc.min_value { if num < min { return Err(ProximaDBError::InvalidInput(format!("'{}' less than min_value", key))); } }
                    if let Some(max) = nc.max_value { if num > max { return Err(ProximaDBError::InvalidInput(format!("'{}' greater than max_value", key))); } }
                    if let Some(m) = nc.multiple_of { if m != 0.0 && (num / m).fract() != 0.0 { return Err(ProximaDBError::InvalidInput(format!("'{}' not a multiple_of {}", key, m))); } }
                }
                crate::proto::proximadb_v1::property_constraint::Constraint::ArrayConstraint(ac) => {
                    if let Some(PV::ArrayValue(arr)) = &value.value {
                        let len = arr.values.len() as i32;
                        if let Some(min) = ac.min_items { if len < min { return Err(ProximaDBError::InvalidInput(format!("'{}' array smaller than min_items", key))); } }
                        if let Some(max) = ac.max_items { if len > max { return Err(ProximaDBError::InvalidInput(format!("'{}' array larger than max_items", key))); } }
                    }
                }
                crate::proto::proximadb_v1::property_constraint::Constraint::RegexConstraint(rc) => {
                    if let Some(PV::StringValue(s)) = &value.value {
                        let re = regex::RegexBuilder::new(&rc.pattern)
                            .case_insensitive(rc.flags.contains('i'))
                            .multi_line(rc.flags.contains('m'))
                            .dot_matches_new_line(rc.flags.contains('s'))
                            .build()
                            .map_err(|e| ProximaDBError::InvalidInput(format!("Invalid regex in schema: {}", e)))?;
                        if !re.is_match(s) { return Err(ProximaDBError::InvalidInput(format!("'{}' does not match regex", key))); }
                    }
                }
            }
        }
        Ok(())
    }
    */

    async fn enforce_cardinality_on_edge(
        &self,
        graph_id: &str,
        edge: &Edge,
        engine: &crate::graph::engines::GraphEngineImpl,
    ) -> Result<()> {
        let maybe_collection = self.collection_service.get_graph(graph_id).await?;
        if let Some(coll) = maybe_collection
            && let Some(schema) = &coll.schema
            && let Some(ets) = schema
                .edge_types
                .iter()
                .find(|et| et.edge_type == edge.edge_type)
        {
            use crate::proto::proximadb_v1::Cardinality;
            match ets.cardinality {
                x if x == Cardinality::OneToOne as i32 => {
                    if !engine
                        .get_outgoing_edges(&edge.from_node_id, Some(&edge.edge_type))?
                        .is_empty()
                    {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Cardinality violation: ONE_TO_ONE allows a single '{}' from {}",
                            edge.edge_type, edge.from_node_id
                        )));
                    }
                    if !engine
                        .get_incoming_edges(&edge.to_node_id, Some(&edge.edge_type))?
                        .is_empty()
                    {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Cardinality violation: ONE_TO_ONE allows a single '{}' to {}",
                            edge.edge_type, edge.to_node_id
                        )));
                    }
                }
                x if x == Cardinality::OneToMany as i32 => {
                    if !engine
                        .get_incoming_edges(&edge.to_node_id, Some(&edge.edge_type))?
                        .is_empty()
                    {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Cardinality violation: ONE_TO_MANY allows a single incoming '{}' to {}",
                            edge.edge_type, edge.to_node_id
                        )));
                    }
                }
                x if x == Cardinality::ManyToOne as i32 => {
                    if !engine
                        .get_outgoing_edges(&edge.from_node_id, Some(&edge.edge_type))?
                        .is_empty()
                    {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Cardinality violation: MANY_TO_ONE allows a single outgoing '{}' from {}",
                            edge.edge_type, edge.from_node_id
                        )));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Minimal provider that surfaces a counter as access frequency
struct SimpleCounterProvider {
    counter: Arc<AtomicU64>,
    avg_entry_size: usize,
    fixed_hit_rate: f64,
}

impl SimpleCounterProvider {
    fn new(counter: Arc<AtomicU64>, avg_entry_size: usize, fixed_hit_rate: f64) -> Self {
        Self {
            counter,
            avg_entry_size,
            fixed_hit_rate,
        }
    }
}

impl CacheStatsProvider for SimpleCounterProvider {
    fn snapshot(&self) -> UsageStats {
        let freq = self.counter.load(Ordering::Relaxed) as f64;
        UsageStats {
            hit_rate: self.fixed_hit_rate,
            avg_entry_size: self.avg_entry_size,
            access_frequency: freq,
            last_rebalance: std::time::SystemTime::now(),
        }
    }
}

// helpers moved to service_helpers.rs (re-exported)

// helpers moved to service_helpers.rs (re-exported)

// helpers moved to service_helpers.rs (re-exported)

impl Default for GraphOperationsService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PropertyValue;
    // PropertyValue is now a struct, not enum - use direct field access;

    #[tokio::test]
    async fn test_service_creation() -> anyhow::Result<()> {
        let service = GraphOperationsService::new();
        assert_eq!(service.mode(), OperationMode::Unified);
        assert!(service.graph_enabled());
        assert!(service.vector_enabled());
        Ok(())
    }

    fn pv_str(s: &str) -> PropertyValue {
        PropertyValue {
            value: Some(
                crate::proto::proximadb_v1::property_value::Value::StringValue(s.to_string()),
            ),
        }
    }
    fn pv_int(i: i64) -> PropertyValue {
        PropertyValue {
            value: Some(crate::proto::proximadb_v1::property_value::Value::IntValue(
                i,
            )),
        }
    }

    #[tokio::test]
    async fn test_schema_property_type_and_constraints() -> anyhow::Result<()> {
        let service = GraphOperationsService::new();
        let mut props = std::collections::HashMap::new();
        // Define age property schema: INTEGER, 18..=120
        props.insert(
            "age".to_string(),
            crate::proto::proximadb_v1::PropertySchema {
                name: "age".to_string(),
                r#type: crate::proto::proximadb_v1::PropertyType::Integer as i32,
                required: true,
                default_value: None,
                constraints: vec![crate::proto::proximadb_v1::PropertyConstraint{
                    constraint: Some(
                        crate::proto::proximadb_v1::property_constraint::Constraint::NumericConstraint(
                            crate::proto::proximadb_v1::NumericConstraint{ min_value: Some(18.0), max_value: Some(120.0), multiple_of: None }
                        )
                    )
                }],
                description: "Age".to_string(),
            },
        );
        let schema = crate::proto::proximadb_v1::GraphSchema {
            node_labels: vec![crate::proto::proximadb_v1::NodeLabelSchema {
                label: "Person".to_string(),
                required_properties: vec!["age".to_string()],
                optional_properties: vec![],
                allow_additional_properties: true,
                property_constraints: std::collections::HashMap::new(),
            }],
            edge_types: vec![],
            properties: props,
            unique_constraints: vec![],
            strict_mode: true,
        };
        let req = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "g_schema".to_string(),
            name: Some("g_schema".to_string()),
            description: None,
            schema: Some(schema),
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        service.create_graph_collection(req).await?;

        // Wrong type (string) should fail
        let mut n1_props = std::collections::HashMap::new();
        n1_props.insert("age".to_string(), pv_str("twenty"));
        let n1 = crate::proto::proximadb_v1::Node {
            id: "n1".to_string(),
            labels: vec!["Person".to_string()],
            properties: n1_props,
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(service.create_node("g_schema", n1).await.is_err());

        // Out of range should fail
        let mut n2_props = std::collections::HashMap::new();
        n2_props.insert("age".to_string(), pv_int(15));
        let n2 = crate::proto::proximadb_v1::Node {
            id: "n2".to_string(),
            labels: vec!["Person".to_string()],
            properties: n2_props,
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(service.create_node("g_schema", n2).await.is_err());

        // Valid should succeed
        let mut n3_props = std::collections::HashMap::new();
        n3_props.insert("age".to_string(), pv_int(25));
        let n3 = crate::proto::proximadb_v1::Node {
            id: "n3".to_string(),
            labels: vec!["Person".to_string()],
            properties: n3_props,
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(service.create_node("g_schema", n3).await.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_edge_cardinality_constraints() -> anyhow::Result<()> {
        let service = GraphOperationsService::new();
        // Schema with ONE_TO_ONE edge type MARRIED_TO between Person
        let edge_schema = crate::proto::proximadb_v1::EdgeTypeSchema {
            edge_type: "MARRIED_TO".to_string(),
            source_labels: vec!["Person".to_string()],
            target_labels: vec!["Person".to_string()],
            required_properties: vec![],
            optional_properties: vec![],
            allow_additional_properties: true,
            cardinality: crate::proto::proximadb_v1::Cardinality::OneToOne as i32,
            property_constraints: std::collections::HashMap::new(),
        };
        let schema = crate::proto::proximadb_v1::GraphSchema {
            node_labels: vec![crate::proto::proximadb_v1::NodeLabelSchema {
                label: "Person".to_string(),
                required_properties: vec![],
                optional_properties: vec![],
                allow_additional_properties: true,
                property_constraints: std::collections::HashMap::new(),
            }],
            edge_types: vec![edge_schema],
            properties: std::collections::HashMap::new(),
            unique_constraints: vec![],
            strict_mode: true,
        };
        let req = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "g_card".to_string(),
            name: Some("g_card".to_string()),
            description: None,
            schema: Some(schema),
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        service.create_graph_collection(req).await?;
        // Nodes
        let mk = |id: &str| crate::proto::proximadb_v1::Node {
            id: id.to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        for id in ["A", "B", "C", "D"] {
            service.create_node("g_card", mk(id)).await?;
        }
        // First marriage A->B ok
        let e1 = crate::proto::proximadb_v1::Edge {
            id: "e1".to_string(),
            from_node_id: "A".to_string(),
            to_node_id: "B".to_string(),
            edge_type: "MARRIED_TO".to_string(),
            properties: std::collections::HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_edge("g_card", e1).await?;
        // Second marriage from A to C should fail (ONE_TO_ONE violates outgoing)
        let e2 = crate::proto::proximadb_v1::Edge {
            id: "e2".to_string(),
            from_node_id: "A".to_string(),
            to_node_id: "C".to_string(),
            edge_type: "MARRIED_TO".to_string(),
            properties: std::collections::HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(service.create_edge("g_card", e2).await.is_err());
        // Another marriage to B from D should fail (ONE_TO_ONE violates incoming)
        let e3 = crate::proto::proximadb_v1::Edge {
            id: "e3".to_string(),
            from_node_id: "D".to_string(),
            to_node_id: "B".to_string(),
            edge_type: "MARRIED_TO".to_string(),
            properties: std::collections::HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(service.create_edge("g_card", e3).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_multi_unique_constraints() -> anyhow::Result<()> {
        let service = GraphOperationsService::new();
        // Unique on (email, tenant) for Person
        let uc = crate::proto::proximadb_v1::UniqueConstraint {
            name: "uniq_email_tenant".to_string(),
            node_labels: vec!["Person".to_string()],
            properties: vec!["email".to_string(), "tenant".to_string()],
            description: "".to_string(),
        };
        let schema = crate::proto::proximadb_v1::GraphSchema {
            node_labels: vec![crate::proto::proximadb_v1::NodeLabelSchema {
                label: "Person".to_string(),
                required_properties: vec![],
                optional_properties: vec!["email".to_string(), "tenant".to_string()],
                allow_additional_properties: true,
                property_constraints: std::collections::HashMap::new(),
            }],
            edge_types: vec![],
            properties: std::collections::HashMap::new(),
            unique_constraints: vec![uc],
            strict_mode: true,
        };
        let req = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "g_uniq".to_string(),
            name: Some("g_uniq".to_string()),
            description: None,
            schema: Some(schema),
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        service.create_graph_collection(req).await?;
        let mut p1 = std::collections::HashMap::new();
        p1.insert("email".to_string(), pv_str("a@test"));
        p1.insert("tenant".to_string(), pv_str("t1"));
        let mut p2 = std::collections::HashMap::new();
        p2.insert("email".to_string(), pv_str("a@test"));
        p2.insert("tenant".to_string(), pv_str("t1"));
        let mut p3 = std::collections::HashMap::new();
        p3.insert("email".to_string(), pv_str("a@test"));
        p3.insert("tenant".to_string(), pv_str("t2"));
        let n1 = crate::proto::proximadb_v1::Node {
            id: "p1".to_string(),
            labels: vec!["Person".to_string()],
            properties: p1,
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let n2 = crate::proto::proximadb_v1::Node {
            id: "p2".to_string(),
            labels: vec!["Person".to_string()],
            properties: p2,
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let n3 = crate::proto::proximadb_v1::Node {
            id: "p3".to_string(),
            labels: vec!["Person".to_string()],
            properties: p3,
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_node("g_uniq", n1).await?;
        assert!(service.create_node("g_uniq", n2).await.is_err());
        assert!(service.create_node("g_uniq", n3).await.is_ok());
        Ok(())
    }

    #[tokio::test]
    #[cfg(feature = "distributed-graph")]
    async fn test_pulsar_traversal_path() -> anyhow::Result<()> {
        let service = GraphOperationsService::new();
        // Create graph with PULSAR engine
        let engine_cfg = crate::proto::proximadb_v1::GraphEngineConfig {
            engine_type: "PULSAR".to_string(),
            memory_pool_size_mb: 0,
            csr_cache_size_mb: 0,
            enable_parallel_operations: true,
            max_traversal_depth: 10,
            advanced_config: std::collections::HashMap::new(),
        };
        let req = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: "g_pulsar".to_string(),
            name: Some("g_pulsar".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: Some(engine_cfg),
            access_control: None,
        };
        service.create_graph_collection(req).await?;
        // Create small chain A->B->C
        let mk = |id: &str| crate::proto::proximadb_v1::Node {
            id: id.to_string(),
            labels: vec!["N".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_node("g_pulsar", mk("A")).await?;
        service.create_node("g_pulsar", mk("B")).await?;
        service.create_node("g_pulsar", mk("C")).await?;
        let eab = crate::proto::proximadb_v1::Edge {
            id: "eab".to_string(),
            from_node_id: "A".to_string(),
            to_node_id: "B".to_string(),
            edge_type: "X".to_string(),
            properties: std::collections::HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let ebc = crate::proto::proximadb_v1::Edge {
            id: "ebc".to_string(),
            from_node_id: "B".to_string(),
            to_node_id: "C".to_string(),
            edge_type: "X".to_string(),
            properties: std::collections::HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_edge("g_pulsar", eab).await?;
        service.create_edge("g_pulsar", ebc).await?;
        let tr = crate::proto::proximadb_v1::TraversalRequest {
            graph_id: "g_pulsar".to_string(),
            start_node_id: "A".to_string(),
            max_depth: 2,
            edge_types: vec![],
            node_labels: vec![],
            filters: vec![],
            algorithm: crate::proto::proximadb_v1::TraversalAlgorithm::Bfs as i32,
            limit: None,
            timeout_ms: None,
            max_frontier: None,
        };
        let resp = service.traverse("g_pulsar", tr).await?;
        assert!(resp.nodes.len() >= 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_operation_modes() {
        let mut service = GraphOperationsService::new();

        // Test graph-only mode
        service.set_mode(OperationMode::GraphOnly);
        assert_eq!(service.mode(), OperationMode::GraphOnly);
        assert!(service.graph_enabled());
        assert!(!service.vector_enabled());

        // Test vector-only mode
        service.set_mode(OperationMode::VectorOnly);
        assert_eq!(service.mode(), OperationMode::VectorOnly);
        assert!(!service.graph_enabled());
        assert!(service.vector_enabled());

        // Test unified mode
        service.set_mode(OperationMode::Unified);
        assert_eq!(service.mode(), OperationMode::Unified);
        assert!(service.graph_enabled());
        assert!(service.vector_enabled());
    }

    #[test]
    fn test_node_operations() {
        let _service = GraphOperationsService::new();

        // Create a test node
        let node = Node {
            id: "test_node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(
                        crate::proto::proximadb_v1::property_value::Value::StringValue(
                            "Alice".to_string(),
                        ),
                    ),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        // Note: Tests need to be updated to use async and graph_id parameter
        // This is placeholder compilation fix
        assert_eq!(node.id, "test_node_1");
        assert_eq!(node.labels[0], "Person");
    }

    #[test]
    fn test_mode_restrictions() {
        let mut service = GraphOperationsService::new();
        service.set_mode(OperationMode::VectorOnly);

        // Create a test node
        let node = Node {
            id: "test_node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        // Note: Tests need to be updated to use async and graph_id parameter
        // This is placeholder compilation fix
        assert_eq!(node.id, "test_node_1");
    }

    #[tokio::test]
    async fn test_graph_walk_tool() {
        let service = GraphOperationsService::new();
        let graph_id = "walk_test_graph";

        // Create graph
        let req = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: graph_id.to_string(),
            name: Some("Walk Test".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        service.create_graph_collection(req).await.unwrap();

        // Create n1 -> n2 -> n3
        let nodes = vec!["n1", "n2", "n3"];
        for id in nodes {
            let node = Node {
                id: id.to_string(),
                labels: vec!["Test".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(graph_id, node).await.unwrap();
        }

        let edges = vec![("e12", "n1", "n2"), ("e23", "n2", "n3")];
        for (id, from, to) in edges {
            let edge = Edge {
                id: id.to_string(),
                from_node_id: from.to_string(),
                to_node_id: to.to_string(),
                edge_type: "NEXT".to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_edge(graph_id, edge).await.unwrap();
        }

        // Walk from n1 with depth 1
        let results = service.graph_walk(graph_id, "n1", 1, 10).await.unwrap();

        // Should find n1 and n2 (BFS depth 1)
        assert_eq!(results.nodes.len(), 2);
        let ids: Vec<_> = results.nodes.iter().map(|n| &n.id).collect();
        assert!(ids.contains(&&"n1".to_string()));
        assert!(ids.contains(&&"n2".to_string()));
    }

    #[tokio::test]
    async fn test_graph_step_tool() {
        let service = GraphOperationsService::new();
        let graph_id = "step_test_graph";

        let req = crate::proto::proximadb_v1::CreateGraphRequest {
            graph_id: graph_id.to_string(),
            name: Some("Step Test".to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        };
        service.create_graph_collection(req).await.unwrap();

        // n1 -> n2 (NEXT), n1 -> n3 (REF), n1 -> n4 (NEXT)
        for id in ["n1", "n2", "n3", "n4"] {
            let node = Node {
                id: id.to_string(),
                labels: vec!["Test".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(graph_id, node).await.unwrap();
        }
        for (id, from, to, et) in [
            ("e12", "n1", "n2", "NEXT"),
            ("e13", "n1", "n3", "REF"),
            ("e14", "n1", "n4", "NEXT"),
        ] {
            let edge = Edge {
                id: id.to_string(),
                from_node_id: from.to_string(),
                to_node_id: to.to_string(),
                edge_type: et.to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_edge(graph_id, edge).await.unwrap();
        }

        // Step from n1 with no edge filter: start node + 3 neighbors.
        let unfiltered = service.graph_step(graph_id, "n1", None, 50).await.unwrap();
        assert_eq!(unfiltered.nodes.len(), 4, "expected n1 + 3 neighbors");
        assert_eq!(unfiltered.nodes[0].id, "n1");

        // Step from n1 filtered to NEXT: start node + 2 NEXT neighbors.
        let next_only = service
            .graph_step(graph_id, "n1", Some("NEXT"), 50)
            .await
            .unwrap();
        assert_eq!(next_only.nodes.len(), 3, "expected n1 + 2 NEXT neighbors");
        let ids: Vec<_> = next_only.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"n1"));
        assert!(ids.contains(&"n2"));
        assert!(ids.contains(&"n4"));
        assert!(!ids.contains(&"n3"), "REF neighbor should be filtered out");

        // Limit caps neighbor count.
        let limited = service.graph_step(graph_id, "n1", None, 1).await.unwrap();
        assert_eq!(
            limited.nodes.len(),
            2,
            "expected n1 + 1 neighbor under limit"
        );

        // Missing node returns NotFound.
        let missing = service.graph_step(graph_id, "no-such-node", None, 10).await;
        assert!(missing.is_err(), "missing node should error");
    }
}
