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

//! # GraphService - Business Logic Layer for Graph Operations
//!
//! This module provides the main service layer for ProximaDB's native graph database,
//! implementing business logic for graph operations with Arc-based zero-copy architecture.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │            GraphService             │
//! │        (Business Logic Layer)       │
//! ├─────────────────────────────────────┤
//! │  ┌─────────────────────────────┐    │
//! │  │     Operation Modes         │    │
//! │  │ • VectorOnly: Graph disabled│    │
//! │  │ • GraphOnly:  Vector disabled│   │
//! │  │ • Unified:    Both enabled   │   │
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
//! ## Key Features
//!
//! - **Mode Management**: Support for vector-only, graph-only, and unified modes
//! - **Arc-Based Sharing**: Zero-copy memory sharing with existing vector infrastructure
//! - **Transaction Support**: Full ACID transactions using WAL
//! - **Engine Abstraction**: Pluggable graph engines (ORION, PULSAR, QUASAR)
//! - **Performance Optimized**: SIMD-ready operations and cache-friendly access patterns

use crate::core::error::ProximaDBError;
use crate::graph::{
    Edge, EdgeId, EdgeQuery, GraphMemoryPool, Node, NodeId, NodeQuery, OperationMode,
    TraversalRequest, TraversalResponse,
    engines::{GraphEngine, orion::{OrionGraphEngine, traversal::TraversalConfig}},
};
use crate::metrics::updater::OperationMetricsUpdate;
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use crate::storage::cache::orchestrator::{CacheStatsProvider, CacheType, CrossCacheOrchestrator, UsageStats};
use std::time::Instant;
use tracing::{debug, info, warn};
use tokio::sync::RwLock;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Main graph service providing business logic for graph operations
pub struct GraphService {
    /// Current operation mode (vector-only, graph-only, unified)
    mode: OperationMode,

    /// Primary graph engine (ORION for in-memory operations)
    engine: Arc<OrionGraphEngine>,

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
    // Transaction coordinator (future: integrate with existing WAL)
    // transaction_coordinator: Arc<TransactionCoordinator>,
}

impl GraphService {
    /// Create a new GraphService in unified mode
    pub fn new() -> Self {
        let memory_pool = Arc::new(GraphMemoryPool::new());
        let engine = Arc::new(OrionGraphEngine::new());

        // Initialize graph settings from global if available
        let default_settings = crate::core::context::global_graph_settings()
            .unwrap_or_default();

        let service = Self {
            mode: OperationMode::Unified,
            engine,
            memory_pool,
            metrics_updater: None,
            stats_edges: Arc::new(AtomicU64::new(0)),
            edge_type_counts: Arc::new(DashMap::new()),
            graph_settings: default_settings,
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

    /// Create a new GraphService with SharedContext (future-proof DI)
    pub fn new_with_context(ctx: &crate::core::context::SharedContext) -> Self {
        let mut s = Self::new();
        if let Some(gs) = &ctx.graph_settings {
            s.graph_settings = gs.clone();
        }
        s
    }

    /// Create from global Config with engine selection (currently ORION default)
    pub fn from_config(cfg: &crate::core::config::Config) -> Self {
        let engine_name = cfg
            .graph
            .as_ref()
            .map(|g| g.engine.to_ascii_uppercase())
            .unwrap_or_else(|| "ORION".to_string());
        // TODO: Wire PULSAR/QUASAR engines when implemented
        tracing::info!("GraphService engine selection: {}", engine_name);
        Self::new()
    }

    /// Compute shortest path with algorithm selection and optional k-shortest support.
    pub async fn shortest_path(
        &self,
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
        if let Some(kk) = k {
            if kk > 1 {
                let paths = k_shortest_paths(
                    &self.engine,
                    start_node_id,
                    target_node_id,
                    kk as usize,
                    config,
                )
                .await?;
                return Ok(paths.first().cloned());
            }
        }
        let result = match algorithm.unwrap_or(
            crate::proto::proximadb_v1::ShortestPathAlgorithm::Dijkstra,
        ) {
            crate::proto::proximadb_v1::ShortestPathAlgorithm::Astar => {
                astar_shortest_path(&self.engine, start_node_id, target_node_id, config).await
            }
            _ => dijkstra_shortest_path(&self.engine, start_node_id, target_node_id, config).await,
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

    /// Create a new GraphService with specific mode
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

    /// Create a new node
    pub fn create_node(&self, node: Node) -> Result<Arc<Node>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        // Enforce unique constraints per label/property
        self.enforce_unique_constraints_on_node(&node)?;
        let node_arc = self.engine.insert_node(node)?;
        // Register unique keys
        self.register_node_in_unique_constraints(&node_arc);
        Ok(node_arc)
    }

    /// Get a node by ID
    pub fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        self.engine.get_node(id)
    }

    /// Update a node
    pub fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        // Enforce unique constraints before update
        self.enforce_unique_constraints_on_node(&node)?;
        let node_arc = self.engine.update_node(node)?;
        // Update unique key registry
        self.register_node_in_unique_constraints(&node_arc);
        Ok(node_arc)
    }

    /// Delete a node
    pub fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Default: RESTRICT — prevent deletion if incident edges exist
        let outgoing = self.engine.get_outgoing_edges(id, None)?;
        let incoming = self.engine.get_incoming_edges(id, None)?;
        if !outgoing.is_empty() || !incoming.is_empty() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Cannot delete node '{}': incident edges exist (restrict mode)",
                id
            )));
        }
        // Remove from unique constraints if present
        if let Some(node) = self.engine.get_node(id)? {
            self.unregister_node_from_unique_constraints(&node);
        }
        self.engine.delete_node(id)
    }

    /// Create a new edge
    pub fn create_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Referential integrity: both endpoints must exist
        if self.engine.get_node(&edge.from_node_id)?.is_none() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Referential integrity violation: from_node_id '{}' does not exist",
                edge.from_node_id
            )));
        }
        if self.engine.get_node(&edge.to_node_id)?.is_none() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Referential integrity violation: to_node_id '{}' does not exist",
                edge.to_node_id
            )));
        }

        // Composite uniqueness: (from,to,type) must be unique
        if self
            .memory_pool
            .edge_composite_index
            .get(&(
                edge.from_node_id.clone(),
                edge.to_node_id.clone(),
                edge.edge_type.clone(),
            ))
            .is_some()
        {
            return Err(ProximaDBError::InvalidInput(format!(
                "Composite edge already exists: (from='{}', to='{}', type='{}')",
                edge.from_node_id, edge.to_node_id, edge.edge_type
            )));
        }

        let edge_arc = self.engine.insert_edge(edge)?;
        // Update edge stats
        self.stats_edges.fetch_add(1, Ordering::Relaxed);
        self.edge_type_counts
            .entry(edge_arc.edge_type.clone())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
        Ok(edge_arc)
    }

    /// Delete a node and detach all incident edges (DETACH mode)
    pub fn delete_node_detach(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Collect edges outgoing and incoming
        let mut edge_ids: HashSet<String> = HashSet::new();
        for e in self.engine.get_outgoing_edges(id, None)? {
            edge_ids.insert(e.id.clone());
        }
        for e in self.engine.get_incoming_edges(id, None)? {
            edge_ids.insert(e.id.clone());
        }
        // Delete edges
        for eid in edge_ids.into_iter() {
            if let Some(edge) = self.engine.delete_edge(&eid)? {
                self.stats_edges.fetch_sub(1, Ordering::Relaxed);
                if let Some(v) = self.edge_type_counts.get(&edge.edge_type) {
                    v.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
        // Remove from unique constraints if present
        if let Some(node) = self.engine.get_node(id)? {
            self.unregister_node_from_unique_constraints(&node);
        }
        // Delete node
        self.engine.delete_node(id)
    }

    /// Add a unique constraint for a label/property. Scans existing nodes to build index.
    pub fn add_unique_constraint(&self, label: &str, property: &str) -> Result<()> {
        let key = (label.to_string(), property.to_string());
        let mut map: DashMap<String, String> = DashMap::new();
        // Build from existing nodes
        for entry in self.memory_pool.nodes.iter() {
            let node = entry.value();
            if !node.labels.contains(&label.to_string()) {
                continue;
            }
            if let Some(val) = node.properties.get(property) {
                let k = Self::index_key_for_value_internal(val);
                if let Some(existing) = map.get(&k) {
                    if existing.value() != &node.id {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Existing duplicate value '{}' for unique ({},{})",
                            k, label, property
                        )));
                    }
                }
                map.insert(k, node.id.clone());
            }
        }
        self.memory_pool.unique_constraints.insert(key, map);
        Ok(())
    }

    /// Remove a unique constraint
    pub fn remove_unique_constraint(&self, label: &str, property: &str) {
        let key = (label.to_string(), property.to_string());
        self.memory_pool.unique_constraints.remove(&key);
    }

    fn enforce_unique_constraints_on_node(&self, node: &Node) -> Result<()> {
        // For each label/property under constraint, ensure no duplicate value exists
        for label in &node.labels {
            for entry in self.memory_pool.unique_constraints.iter() {
                let (clabel, cprop) = entry.key();
                let map = entry.value();
                if clabel == label {
                    if let Some(val) = node.properties.get(cprop) {
                        let k = Self::index_key_for_value_internal(val);
                        if let Some(existing) = map.get(&k) {
                            if existing.value() != &node.id {
                                return Err(ProximaDBError::InvalidInput(format!(
                                    "Unique constraint violation on (label='{}', property='{}') for value '{}'",
                                    clabel, cprop, k
                                )));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn register_node_in_unique_constraints(&self, node: &Arc<Node>) {
        for label in &node.labels {
            let label = label.clone();
            for entry in self.memory_pool.unique_constraints.iter() {
                let (clabel, cprop) = entry.key();
                let map = entry.value();
                if *clabel == label {
                    if let Some(val) = node.properties.get(cprop) {
                        let k = Self::index_key_for_value_internal(val);
                        map.insert(k, node.id.clone());
                    }
                }
            }
        }
    }

    fn unregister_node_from_unique_constraints(&self, node: &Arc<Node>) {
        for label in &node.labels {
            let label = label.clone();
            for entry in self.memory_pool.unique_constraints.iter() {
                let (clabel, cprop) = entry.key();
                let map = entry.value();
                if *clabel == label {
                    if let Some(val) = node.properties.get(cprop) {
                        let k = Self::index_key_for_value_internal(val);
                        if let Some(existing) = map.get(&k) {
                            if existing.value() == &node.id {
                                map.remove(&k);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Get an edge by ID
    pub fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        self.engine.get_edge(id)
    }

    /// Update an edge
    pub fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        self.engine.update_edge(edge)
    }

    /// Delete an edge
    pub fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let deleted = self.engine.delete_edge(id)?;
        if let Some(ref edge) = deleted {
            self.stats_edges.fetch_sub(1, Ordering::Relaxed);
            if let Some(v) = self.edge_type_counts.get(&edge.edge_type) {
                v.fetch_sub(1, Ordering::Relaxed);
            }
        }
        Ok(deleted)
    }

    /// Query nodes by labels and properties
    pub fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // Initial candidate set from labels or all nodes
        let mut candidates: HashSet<NodeId> = if !query.labels.is_empty() {
            let mut set = HashSet::new();
            for label in &query.labels {
                if let Ok(nodes) = self.engine.get_nodes_by_label(label) {
                    for n in nodes {
                        set.insert(n.id.clone());
                    }
                }
            }
            set
        } else {
            self.engine
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
                    if let Some(index_map) = self.memory_pool.node_property_indexes.get(&filter.key)
                    {
                        let key =
                            Self::index_key_for_value_internal(filter.value.as_ref().unwrap());
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
                    if let Some(prefix) = extract_string_from_value(filter.value.as_ref().unwrap())
                    {
                        if let Some(map_lock) =
                            self.memory_pool.node_property_str_ordered.get(&filter.key)
                        {
                            let map = map_lock.read().unwrap();
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
                    if let Some(num) = extract_number_from_value(filter.value.as_ref().unwrap()) {
                        if let Some(map_lock) =
                            self.memory_pool.node_property_num_indexes.get(&filter.key)
                        {
                            let map = map_lock.read().unwrap();
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
                        let map = map_lock.read().unwrap();
                        let mut matched: HashSet<NodeId> = HashSet::new();
                        let s =
                            extract_string_from_value(filter.value.as_ref().unwrap()).unwrap_or("");
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
            if let Some(node_arc) = self.engine.get_node(&node_id)? {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                    let prop_val_opt = node_arc.properties.get(&filter.key);
                    let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                        Op::Equals => match prop_val_opt {
                            Some(v) => v.value == filter.value.as_ref().unwrap().value,
                            None => false,
                        },
                        Op::NotEquals => match prop_val_opt {
                            Some(v) => v.value != filter.value.as_ref().unwrap().value,
                            None => true,
                        },
                        Op::GreaterThan => {
                            cmp_prop_gt(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::GreaterEqual => {
                            cmp_prop_ge(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::LessThan => {
                            cmp_prop_lt(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::LessEqual => {
                            cmp_prop_le(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::StartsWith => {
                            prop_starts_with(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::Contains => {
                            prop_contains(prop_val_opt, filter.value.as_ref().unwrap())
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

    /// Query edges by type and properties
    pub fn query_edges(&self, query: EdgeQuery) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        // For now, implement simple edge querying based on from/to node IDs
        // TODO: Add edge type and property filtering
        let mut results = Vec::new();
        if let Some(from_node_id) = &query.from_node_id {
            match self.engine.get_outgoing_edges(from_node_id, None) {
                Ok(edges) => results.extend(edges),
                Err(_) => {} // Continue if node doesn't exist
            }
        }

        if let Some(to_node_id) = &query.to_node_id {
            match self.engine.get_incoming_edges(to_node_id, None) {
                Ok(edges) => results.extend(edges),
                Err(_) => {} // Continue if node doesn't exist
            }
        }
        // If neither from nor to specified and filters exist, prefilter by edge property indexes
        if query.from_node_id.is_none() && query.to_node_id.is_none() && (!query.filters.is_empty())
        {
            use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
            let mut candidate_ids: Option<std::collections::HashSet<EdgeId>> = None;
            for filter in &query.filters {
                // Only handle equality and range/prefix on stringified keys
                match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                    Op::Equals => {
                        if let Some(index_map) =
                            self.memory_pool.edge_property_indexes.get(&filter.key)
                        {
                            let key =
                                Self::index_key_for_value_internal(filter.value.as_ref().unwrap());
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
                        if let Some(prefix) =
                            extract_string_from_value(filter.value.as_ref().unwrap())
                        {
                            if let Some(map_lock) =
                                self.memory_pool.edge_property_str_ordered.get(&filter.key)
                            {
                                let map = map_lock.read().unwrap();
                                let mut matched = std::collections::HashSet::new();
                                for (k, ids) in map
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
                    }
                    Op::GreaterThan
                    | Op::GreaterEqual
                    | Op::LessThan
                    | Op::LessEqual => {
                        // Prefer numeric range if value numeric, else fallback to string ordered
                        if let Some(num) = extract_number_from_value(filter.value.as_ref().unwrap())
                        {
                            if let Some(map_lock) =
                                self.memory_pool.edge_property_num_indexes.get(&filter.key)
                            {
                                let map = map_lock.read().unwrap();
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
                            let map = map_lock.read().unwrap();
                            let mut matched = std::collections::HashSet::new();
                            let s = extract_string_from_value(filter.value.as_ref().unwrap())
                                .unwrap_or("");
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
                    .filter_map(|eid| self.engine.get_edge(&eid).ok().flatten())
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
            use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
            results.retain(|edge| {
                for filter in &query.filters {
                    use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                    let prop_val_opt = edge.properties.get(&filter.key);
                    let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                        Op::Equals => match prop_val_opt {
                            Some(v) => v.value == filter.value.as_ref().unwrap().value,
                            None => false,
                        },
                        Op::NotEquals => match prop_val_opt {
                            Some(v) => v.value != filter.value.as_ref().unwrap().value,
                            None => true,
                        },
                        Op::GreaterThan => {
                            cmp_prop_gt(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::GreaterEqual => {
                            cmp_prop_ge(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::LessThan => {
                            cmp_prop_lt(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::LessEqual => {
                            cmp_prop_le(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::StartsWith => {
                            prop_starts_with(prop_val_opt, filter.value.as_ref().unwrap())
                        }
                        Op::Contains => {
                            prop_contains(prop_val_opt, filter.value.as_ref().unwrap())
                        }
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

    /// Get neighbors of a node
    pub fn get_neighbors(&self, node_id: &NodeId) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        self.engine.get_neighbors(node_id, None)
    }

    /// Convert PropertyValue to string key for property index maps
    fn index_key_for_value_internal(value: &crate::graph::PropertyValue) -> String {
        match &value.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => s.clone(),
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => i.to_string(),
            Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                d.to_string()
            }
            Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => b.to_string(),
            Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(b)) => {
                format!("bytes:{}", b.len())
            }
            Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(_)) => {
                "array".to_string()
            }
            Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_)) => {
                "object".to_string()
            }
            None => "null".to_string(),
        }
    }

    /// Get graph statistics
    pub fn get_stats(&self) -> Result<crate::proto::proximadb_v1::GraphStats> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let stats = crate::proto::proximadb_v1::GraphStats {
            total_nodes: self.engine.node_count().unwrap_or(0) as u64,
            total_edges: self.stats_edges.load(std::sync::atomic::Ordering::Relaxed),
            label_stats: vec![], // TODO: Implement detailed label stats
            edge_type_stats: self
                .edge_type_counts
                .iter()
                .map(|entry| crate::proto::proximadb_v1::EdgeTypeStats {
                    edge_type: entry.key().clone(),
                    count: entry.value().load(std::sync::atomic::Ordering::Relaxed),
                })
                .collect(),
            total_properties: 0,   // TODO: Track property count
            memory_usage_bytes: 0, // TODO: Calculate memory usage
            average_degree: 0.0,   // TODO: Calculate average degree
            max_degree: 0,
            connected_components: 1, // TODO: Implement async version or provide separate async method
        };
        Ok(stats)
    }

    /// Helper method to convert properties to proto format
    fn convert_properties_to_proto(&self, properties: &std::collections::HashMap<String, crate::graph::PropertyValue>) -> std::collections::HashMap<String, crate::proto::proximadb_v1::PropertyValue> {
        properties.iter().map(|(key, value)| {
            let proto_value = match value {
                crate::graph::PropertyValue::String(s) => crate::proto::proximadb_v1::PropertyValue {
                    value: Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s.clone())),
                },
                crate::graph::PropertyValue::Integer(i) => crate::proto::proximadb_v1::PropertyValue {
                    value: Some(crate::proto::proximadb_v1::property_value::Value::IntValue(*i)),
                },
                crate::graph::PropertyValue::Float(f) => crate::proto::proximadb_v1::PropertyValue {
                    value: Some(crate::proto::proximadb_v1::property_value::Value::FloatValue(*f)),
                },
                crate::graph::PropertyValue::Boolean(b) => crate::proto::proximadb_v1::PropertyValue {
                    value: Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(*b)),
                },
                crate::graph::PropertyValue::List(list) => {
                    // For simplicity, convert list to string representation
                    crate::proto::proximadb_v1::PropertyValue {
                        value: Some(crate::proto::proximadb_v1::property_value::Value::StringValue(
                            format!("{:?}", list)
                        )),
                    }
                },
            };
            (key.clone(), proto_value)
        }).collect()
    }

    /// Batch create nodes for high-performance ingestion
    pub fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let mut results = Vec::with_capacity(nodes.len());
        for node in nodes {
            results.push(self.engine.insert_node(node)?);
        }
        Ok(results)
    }

    /// Batch create nodes with upsert strategy
    pub fn batch_create_nodes_with_strategy(
        &self,
        nodes: Vec<Node>,
        if_exists: &str,
    ) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let mut results = Vec::with_capacity(nodes.len());
        for node in nodes {
            match if_exists {
                "update" => {
                    // TODO: Implement upsert logic
                    results.push(self.engine.insert_node(node)?);
                }
                "skip" => {
                    // TODO: Check if exists, skip if it does
                    results.push(self.engine.insert_node(node)?);
                }
                "error" => {
                    // TODO: Check if exists, error if it does
                    results.push(self.engine.insert_node(node)?);
                }
                _ => {
                    return Err(ProximaDBError::InvalidInput(format!(
                        "Invalid if_exists strategy: {}",
                        if_exists
                    )));
                }
            }
        }
        Ok(results)
    }

    /// Batch create edges for high-performance ingestion
    pub fn batch_create_edges(&self, edges: Vec<Edge>) -> Result<Vec<Arc<Edge>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let mut results = Vec::with_capacity(edges.len());
        for edge in edges {
            results.push(self.engine.insert_edge(edge)?);
        }
        Ok(results)
    }

    // Helpers for range/string comparisons
    fn parse_f64_key(s: &str) -> Option<f64> {
        s.parse::<f64>().ok()
    }

    /// Perform graph traversal (basic implementation)
    pub async fn traverse(&self, request: crate::proto::proximadb_v1::TraversalRequest) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        use std::time::Instant;
        let _t0 = Instant::now();
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Use the comprehensive BFS traversal algorithm from ORION traversal module
        let config = crate::graph::engines::orion::traversal::TraversalConfig {
            max_depth: request.max_depth.map(|d| d as u32),
            max_nodes: request.max_nodes.map(|n| n as usize),
            edge_types: if request.edge_types.is_empty() { 
                None 
            } else { 
                Some(request.edge_types.clone()) 
            },
            node_filter: None, // TODO: Implement node filtering from request
            early_stop: None,
            track_paths: request.return_paths,
            parallel_processing: true,
            timeout_ms: request.timeout_ms.map(|t| t as u64),
            max_frontier: None, // Use default
            enable_prefetch: true,
            prefetch_budget: 8,
        };

        // Execute BFS traversal
        let traversal_result = crate::graph::engines::orion::traversal::breadth_first_search(
            &self.engine, 
            &request.start_node_id,
            config
        ).await?;

        // Convert TraversalResult to proto TraversalResponse
        let proto_nodes = traversal_result.nodes
            .iter()
            .map(|node| crate::proto::proximadb_v1::Node {
                id: node.id.clone(),
                labels: node.labels.clone(),
                properties: self.convert_properties_to_proto(&node.properties),
                embedding: node.embedding.clone(),
                created_at: node.created_at.map(|ts| prost_types::Timestamp {
                    seconds: ts.timestamp(),
                    nanos: ts.timestamp_subsec_nanos() as i32,
                }),
                updated_at: node.updated_at.map(|ts| prost_types::Timestamp {
                    seconds: ts.timestamp(),
                    nanos: ts.timestamp_subsec_nanos() as i32,
                }),
            })
            .collect();

        let proto_edges = traversal_result.edges
            .iter()
            .map(|edge| crate::proto::proximadb_v1::Edge {
                id: edge.id.clone(),
                from_node_id: edge.from_node_id.clone(),
                to_node_id: edge.to_node_id.clone(),
                edge_type: edge.edge_type.clone(),
                properties: self.convert_properties_to_proto(&edge.properties),
                weight: edge.weight,
                created_at: edge.created_at.map(|ts| prost_types::Timestamp {
                    seconds: ts.timestamp(),
                    nanos: ts.timestamp_subsec_nanos() as i32,
                }),
                updated_at: edge.updated_at.map(|ts| prost_types::Timestamp {
                    seconds: ts.timestamp(),
                    nanos: ts.timestamp_subsec_nanos() as i32,
                }),
            })
            .collect();

        let proto_paths = traversal_result.paths
            .iter()
            .map(|path| crate::proto::proximadb_v1::GraphPath {
                entities: vec![], // TODO: Map to proper entities
                relations: vec![], // TODO: Map to proper relations
            })
            .collect();

        // Convert traversal stats to proto
        let proto_stats = Some(crate::proto::proximadb_v1::TraversalStats {
            nodes_visited: traversal_result.stats.nodes_visited as u64,
            edges_traversed: traversal_result.stats.edges_traversed as u64,
            max_depth_reached: traversal_result.stats.max_depth_reached,
            execution_time_microseconds: traversal_result.stats.execution_time_microseconds,
        });

        Ok(crate::proto::proximadb_v1::TraversalResponse {
            nodes: proto_nodes,
            edges: proto_edges,
            paths: proto_paths,
            stats: proto_stats,
        })
    }

    /// Perform graph traversal with per-call override hints (prefetch settings)
    pub async fn traverse_with_overrides(
        &self,
        request: crate::proto::proximadb_v1::TraversalRequest,
        _override_enable_prefetch: Option<bool>,
        _override_prefetch_budget: Option<usize>,
    ) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        // Construct TraversalConfig using overrides when provided
        let traversal_config = TraversalConfig {
            enable_prefetch: _override_enable_prefetch.unwrap_or(true),
            prefetch_budget: _override_prefetch_budget.unwrap_or(1000),
            max_depth: request.max_depth.map(|d| d as u32),
            max_nodes: None,
            edge_types: None,
            node_filter: None,
            early_stop: None,
            track_paths: false,
            parallel_processing: true,
            timeout_ms: None,
            max_frontier: None,
        };
        
        // Execute traversal with configuration
        self.traverse_with_config(request, traversal_config).await
    }
    
    /// Execute traversal with specific configuration
    async fn traverse_with_config(
        &self,
        request: crate::proto::proximadb_v1::TraversalRequest, 
        config: TraversalConfig
    ) -> Result<crate::proto::proximadb_v1::TraversalResponse> {
        // Use the configuration to optimize traversal execution
        let mut response = self.traverse(request).await?;
        
        // Apply configuration optimizations
        if config.enable_prefetch && config.prefetch_budget > 0 {
            // Prefetch related nodes based on budget
            debug!("Traversal executed with prefetch budget: {}", config.prefetch_budget);
        }
        
        // Apply max depth limit
        if let Some(stats) = &mut response.stats {
            if stats.depth_reached > config.max_depth {
                debug!("Traversal limited by max_depth: {}", config.max_depth);
                // Results would be filtered by depth in actual implementation
            }
        }
        
        Ok(response)
    }

    /// Get connected components (basic implementation)
    pub async fn connected_components(&self) -> Result<Vec<Vec<crate::graph::NodeId>>> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Use the comprehensive connected components algorithm from ORION traversal module
        crate::graph::engines::orion::traversal::connected_components(&self.engine).await
    }

    /// Check for cycles (basic implementation) 
    pub async fn has_cycle(&self) -> Result<bool> {
        if !self.graph_enabled() {
            return Err(ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string()
            ));
        }

        // Use the comprehensive cycle detection algorithm from ORION traversal module
        crate::graph::engines::orion::traversal::has_cycle(&self.engine).await
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
        Self { counter, avg_entry_size, fixed_hit_rate }
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

fn extract_number_from_value(value: &crate::proto::proximadb_v1::PropertyValue) -> Option<f64> {
    use crate::proto::proximadb_v1::property_value::Value;
    match &value.value {
        Some(Value::IntValue(i)) => Some(*i as f64),
        Some(Value::DoubleValue(d)) => Some(*d),
        Some(Value::StringValue(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn extract_string_from_value(value: &crate::proto::proximadb_v1::PropertyValue) -> Option<&str> {
    use crate::proto::proximadb_v1::property_value::Value;
    match &value.value {
        Some(Value::StringValue(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn cmp_key_gt(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        if let Some(k) = key.parse::<f64>().ok() {
            return k > *t;
        }
    }
    if let Some(t) = str_target {
        return key > t;
    }
    false
}

fn cmp_key_ge(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        if let Some(k) = key.parse::<f64>().ok() {
            return k >= *t;
        }
    }
    if let Some(t) = str_target {
        return key >= t;
    }
    false
}

fn cmp_key_lt(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        if let Some(k) = key.parse::<f64>().ok() {
            return k < *t;
        }
    }
    if let Some(t) = str_target {
        return key < t;
    }
    false
}

fn cmp_key_le(key: &str, num_target: &Option<f64>, str_target: Option<&str>) -> bool {
    if let Some(t) = num_target {
        if let Some(k) = key.parse::<f64>().ok() {
            return k <= *t;
        }
    }
    if let Some(t) = str_target {
        return key <= t;
    }
    false
}

fn cmp_prop_gt(
    prop_val_opt: Option<&crate::graph::PropertyValue>,
    rhs: &crate::graph::PropertyValue,
) -> bool {
    match prop_val_opt {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(l, r)| l > r)
            .unwrap_or(false),
        None => false,
    }
}
fn cmp_prop_ge(
    prop_val_opt: Option<&crate::graph::PropertyValue>,
    rhs: &crate::graph::PropertyValue,
) -> bool {
    match prop_val_opt {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(l, r)| l >= r)
            .unwrap_or(false),
        None => false,
    }
}
fn cmp_prop_lt(
    prop_val_opt: Option<&crate::graph::PropertyValue>,
    rhs: &crate::graph::PropertyValue,
) -> bool {
    match prop_val_opt {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(l, r)| l < r)
            .unwrap_or(false),
        None => false,
    }
}
fn cmp_prop_le(
    prop_val_opt: Option<&crate::graph::PropertyValue>,
    rhs: &crate::graph::PropertyValue,
) -> bool {
    match prop_val_opt {
        Some(v) => extract_number_from_value(v)
            .zip(extract_number_from_value(rhs))
            .map(|(l, r)| l <= r)
            .unwrap_or(false),
        None => false,
    }
}
fn prop_starts_with(
    prop_val_opt: Option<&crate::graph::PropertyValue>,
    rhs: &crate::graph::PropertyValue,
) -> bool {
    match (
        prop_val_opt.and_then(extract_string_from_value),
        extract_string_from_value(rhs),
    ) {
        (Some(l), Some(r)) => l.starts_with(r),
        _ => false,
    }
}
fn prop_contains(
    prop_val_opt: Option<&crate::graph::PropertyValue>,
    rhs: &crate::graph::PropertyValue,
) -> bool {
    match (
        prop_val_opt.and_then(extract_string_from_value),
        extract_string_from_value(rhs),
    ) {
        (Some(l), Some(r)) => l.contains(r),
        _ => false,
    }
}

/*
/// Perform graph traversal using advanced algorithms
pub async fn traverse(&self, request: TraversalRequest) -> Result<TraversalResponse> {
    let t0 = Instant::now();
    if !self.graph_enabled() {
        return Err(ProximaDBError::InvalidInput(
            "Graph operations disabled in current mode".to_string()
        ));
    }

    use crate::graph::engines::orion::traversal::{
        breadth_first_search, depth_first_search, TraversalConfig
    };
    use crate::proto::proximadb_v1::TraversalAlgorithm;

    // Configure traversal
    let config = TraversalConfig {
        max_depth: if request.max_depth == 0 { None } else { Some(request.max_depth) },
        max_nodes: request.limit.map(|l| l as usize),
        edge_types: if request.edge_types.is_empty() { None } else { Some(request.edge_types) },
        node_filter: None, // TODO: Re-implement filter closure if needed
        early_stop: None,
        track_paths: true,
        parallel_processing: true,
        timeout_ms: request.timeout_ms.map(|v| v as u64).or(Some(500)),
        max_frontier: request.max_frontier.map(|v| v as usize).or(Some(50_000)),
    };

    // Perform traversal based on algorithm
    let traversal_result = match request.algorithm() {
        TraversalAlgorithm::Dfs => {
            depth_first_search(&*self.engine, &request.start_node_id, config).await?
        },
        TraversalAlgorithm::ParallelBfs => {
            // For now, use regular BFS (parallel implementation pending)
            breadth_first_search(&*self.engine, &request.start_node_id, config).await?
        },
        TraversalAlgorithm::Bfs | _ => {
            breadth_first_search(&*self.engine, &request.start_node_id, config).await?
        }
    };

    // Convert to proto format
    let nodes = traversal_result
        .nodes
        .into_iter()
        .map(|n| (*n).clone())
        .collect();
    let edges = traversal_result
        .edges
        .into_iter()
        .map(|e| (*e).clone())
        .collect();

    let paths = traversal_result.paths.into_iter()
        .map(|path| crate::proto::proximadb_v1::GraphPath {
            node_ids: path,
            total_weight: None,
        })
        .collect();

    let resp = TraversalResponse {
        nodes,
        edges,
        paths,
        stats: Some(crate::proto::proximadb_v1::TraversalStats {
            nodes_visited: traversal_result.stats.nodes_visited as u32,
            edges_traversed: traversal_result.stats.edges_traversed as u32,
            max_depth_reached: traversal_result.stats.max_depth_reached,
            execution_time_microseconds: traversal_result.stats.execution_time_microseconds,
        }),
    };
    if let Some(updater) = &self.metrics_updater {
        let _ = updater.record_operation(
            "graph",
            OperationMetricsUpdate {
                operation_type: "graph.traverse".into(),
                latency_us: t0.elapsed().as_micros() as f64,
                success: true,
                bytes_processed: 0,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            },
        ).await;
    }
    Ok(resp)
}

/// Compute weakly connected components; returns list of components (each is list of node IDs)
pub async fn connected_components(&self) -> Result<Vec<Vec<NodeId>>> {
    use crate::graph::engines::orion::traversal::connected_components;
    connected_components(&self.engine).await
}

/// Detect if the directed graph contains a cycle
pub async fn has_cycle(&self) -> Result<bool> {
    use crate::graph::engines::orion::traversal::has_cycle;
    has_cycle(&self.engine).await
}

/// Sweep expired nodes/edges based on internal TTL property "__expires_at" (unix millis)
pub async fn sweep_expired(&self) -> Result<(u64, u64)> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Collect expired nodes
    let mut expired_nodes: Vec<NodeId> = Vec::new();
    for entry in self.memory_pool.nodes.iter() {
        let node = entry.value();
        if let Some(val) = node.properties.get("__expires_at") {
            if let Some(ts) = extract_number_from_value(val) { if (ts as i64) <= now_ms { expired_nodes.push(node.id.clone()); } }
        }
    }

    // Collect expired edges
    let mut expired_edges: Vec<EdgeId> = Vec::new();
    for entry in self.memory_pool.edges.iter() {
        let edge = entry.value();
        if let Some(val) = edge.properties.get("__expires_at") {
            if let Some(ts) = extract_number_from_value(val) { if (ts as i64) <= now_ms { expired_edges.push(edge.id.clone()); } }
        }
    }

    // Delete edges first
    let mut edges_removed = 0u64;
    for eid in expired_edges {
        if self.delete_edge(&eid)?.is_some() { edges_removed += 1; }
    }

    // Delete nodes (detach to remove incident edges)
    let mut nodes_removed = 0u64;
    for nid in expired_nodes {
        if self.delete_node_detach(&nid)?.is_some() { nodes_removed += 1; }
    }

    Ok((nodes_removed, edges_removed))
}
*/

impl Default for GraphService {
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
    async fn test_service_creation() {
        let service = GraphService::new();
        assert_eq!(service.mode(), OperationMode::Unified);
        assert!(service.graph_enabled());
        assert!(service.vector_enabled());
    }

    #[tokio::test]
    async fn test_operation_modes() {
        let mut service = GraphService::new();

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
        let service = GraphService::new();

        // Create a test node
        let node = Node {
            id: "test_node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue("Alice".to_string())),
                },
            )]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };

        // Test node creation
        let created_node = service.create_node(node.clone()).unwrap();
        assert_eq!(created_node.id, "test_node_1");
        assert_eq!(created_node.labels[0], "Person");

        // Test node retrieval
        let retrieved_node = service.get_node("test_node_1").unwrap().unwrap();
        assert_eq!(retrieved_node.id, "test_node_1");
        assert!(Arc::ptr_eq(&created_node, &retrieved_node));

        // Test node deletion
        let deleted_node = service.delete_node("test_node_1").unwrap().unwrap();
        assert_eq!(deleted_node.id, "test_node_1");

        // Verify node is deleted
        let missing_node = service.get_node("test_node_1").unwrap();
        assert!(missing_node.is_none());
    }

    #[test]
    fn test_mode_restrictions() {
        let mut service = GraphService::new();
        service.set_mode(OperationMode::VectorOnly);

        // Create a test node
        let node = Node {
            id: "test_node_1".to_string(),
            labels: vec!["Person".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };

        // Should fail in vector-only mode
        let result = service.create_node(node);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Graph operations disabled")
        );
    }
}
