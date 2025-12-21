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

//! # PULSAR Graph Engine - Distributed Sharded Storage
//!
//! PULSAR (Partitioned Universal Logic for Scalable Analytics & Retrieval) is ProximaDB's
//! distributed graph engine designed for horizontal scaling of large graphs (1B+ nodes).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │            PULSAR Engine                 │
//! ├─────────────────────────────────────────┤
//! │              Coordinator                 │
//! │  ┌─────────────────────────────────────┐ │
//! │  │     Query Router & Distributor       │ │
//! │  └─────────────────────────────────────┘ │
//! ├─────────────────────────────────────────┤
//! │               Sharding                   │
//! │  ┌─────────┬─────────┬─────────────┐    │
//! │  │ Shard 0 │ Shard 1 │   Shard N   │    │
//! │  │(ORION)  │(ORION)  │  (ORION)    │    │
//! │  └─────────┴─────────┴─────────────┘    │
//! ├─────────────────────────────────────────┤
//! │             Replication                  │
//! │  ┌─────────────────────────────────────┐ │
//! │  │    Master-Slave Replication         │ │
//! │  │    Configurable Factor (1-3)        │ │
//! │  └─────────────────────────────────────┘ │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Consistent Hashing**: Nodes distributed using SHA-256 hash ring
//! - **Configurable Replication**: 1-3x replication factor for fault tolerance
//! - **Cross-Shard Queries**: Distributed BFS/DFS traversal
//! - **2PC Transactions**: Basic distributed transaction support
//! - **Hot Shard Detection**: Automatic load balancing

pub mod consensus;
pub mod coordinator;
pub mod monitoring;
pub mod optimizer;
pub mod regions;
pub mod replication;
pub mod sharding;
pub mod transactions;

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::engines::{GraphEngine, orion::OrionGraphEngine};
use crate::graph::{Edge, EdgeId, GraphMemoryPool, Node, NodeId};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// PULSAR distributed graph engine configuration
#[derive(Debug, Clone)]
pub struct PulsarConfig {
    /// Number of shards for data distribution
    pub shard_count: usize,
    /// Replication factor (1-3)
    pub replication_factor: u8,
    /// Consistency level for reads/writes
    pub consistency_level: ConsistencyLevel,
    /// Enable cross-shard query optimization
    pub cross_shard_optimization: bool,
    /// Maximum concurrent cross-shard queries
    pub max_concurrent_queries: usize,
}

/// Consistency levels for distributed operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsistencyLevel {
    /// Read/write from any replica
    Any,
    /// Read/write from majority of replicas
    Quorum,
    /// Read/write from all replicas
    All,
}

/// PULSAR distributed graph engine
#[derive(Debug)]
pub struct PulsarGraphEngine {
    /// Engine configuration
    config: PulsarConfig,

    /// Shared memory pool across all shards
    memory_pool: Arc<GraphMemoryPool>,

    /// Shard engines (each shard is an ORION engine)
    shards: Arc<DashMap<u32, Arc<OrionGraphEngine>>>,

    /// Consistent hash ring for node distribution
    hash_ring: Arc<RwLock<sharding::ConsistentHashRing>>,

    /// Replication manager for fault tolerance
    replication_manager: Arc<replication::ReplicationManager>,

    /// Query coordinator for cross-shard operations
    coordinator: Arc<coordinator::QueryCoordinator>,

    /// Engine statistics
    stats: Arc<RwLock<PulsarStats>>,
}

/// PULSAR engine statistics
#[derive(Debug, Default)]
pub struct PulsarStats {
    pub total_nodes: u64,
    pub total_edges: u64,
    pub shards_active: u32,
    pub cross_shard_queries: u64,
    pub replication_lag_ms: u64,
    pub hot_shards: Vec<u32>,
    pub load_balance_operations: u64,
}

impl Default for PulsarConfig {
    fn default() -> Self {
        Self {
            shard_count: 16,
            replication_factor: 1,
            consistency_level: ConsistencyLevel::Quorum,
            cross_shard_optimization: true,
            max_concurrent_queries: 100,
        }
    }
}

impl PulsarGraphEngine {
    /// Create a new PULSAR distributed graph engine
    pub fn new(config: PulsarConfig) -> Result<Self> {
        // Shared memory pool for PULSAR-level operations (e.g., cross-shard queries)
        let memory_pool = Arc::new(GraphMemoryPool::new());

        // Initialize shards - each shard has its own isolated memory pool
        // This ensures proper data partitioning and shard isolation
        let shards = Arc::new(DashMap::new());
        for shard_id in 0..config.shard_count {
            // Each shard gets its own memory pool for proper isolation
            let shard_memory_pool = Arc::new(GraphMemoryPool::new());
            let shard_engine =
                Arc::new(OrionGraphEngine::with_memory_pool(shard_memory_pool));
            shards.insert(shard_id as u32, shard_engine);
        }

        // Initialize hash ring
        let hash_ring = Arc::new(RwLock::new(sharding::ConsistentHashRing::new(
            config.shard_count as u32,
        )));

        // Initialize replication manager
        let replication_manager = Arc::new(replication::ReplicationManager::new(
            config.replication_factor,
            &shards,
            Arc::clone(&hash_ring),
        ));

        // Initialize query coordinator
        let coordinator = Arc::new(coordinator::QueryCoordinator::new(
            Arc::clone(&shards),
            Arc::clone(&hash_ring),
            config.max_concurrent_queries,
        ));

        let stats = Arc::new(RwLock::new(PulsarStats {
            shards_active: config.shard_count as u32,
            ..Default::default()
        }));

        Ok(Self {
            config,
            memory_pool,
            shards,
            hash_ring,
            replication_manager,
            coordinator,
            stats,
        })
    }

    /// Get shard ID for a given node
    async fn get_shard_for_node(&self, node_id: &NodeId) -> Result<u32> {
        let hash_ring = self.hash_ring.read().await;
        Ok(hash_ring.get_shard(node_id))
    }

    /// Sync version of get_shard_for_node (for use in sync contexts)
    fn get_shard_for_node_sync(&self, node_id: &NodeId) -> Result<u32> {
        let hash_ring = self.hash_ring.try_read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire hash ring lock".to_string())
        })?;
        Ok(hash_ring.get_shard(node_id))
    }

    /// Get primary shard engine for a node
    async fn get_primary_shard(&self, node_id: &NodeId) -> Result<Arc<OrionGraphEngine>> {
        let shard_id = self.get_shard_for_node(node_id).await?;

        self.shards
            .get(&shard_id)
            .map(|entry| Arc::clone(&entry))
            .ok_or_else(|| ProximaDBError::Internal(format!("Shard {} not found", shard_id)))
    }

    /// Sync version of get_primary_shard (for use in sync contexts)
    fn get_primary_shard_sync(&self, node_id: &NodeId) -> Result<Arc<OrionGraphEngine>> {
        let shard_id = self.get_shard_for_node_sync(node_id)?;

        self.shards
            .get(&shard_id)
            .map(|entry| Arc::clone(&entry))
            .ok_or_else(|| ProximaDBError::Internal(format!("Shard {} not found", shard_id)))
    }

    /// Get all replica shards for a node
    async fn get_replica_shards(&self, node_id: &NodeId) -> Result<Vec<Arc<OrionGraphEngine>>> {
        let primary_shard_id = self.get_shard_for_node(node_id).await?;
        let replica_ids = self
            .replication_manager
            .get_replicas(primary_shard_id)
            .await?;

        let mut shards = Vec::new();
        for shard_id in replica_ids {
            if let Some(shard) = self.shards.get(&shard_id) {
                shards.push(Arc::clone(&shard));
            }
        }

        Ok(shards)
    }

    /// Sync version of get_replica_shards (for use in sync contexts)
    fn get_replica_shards_sync(&self, node_id: &NodeId) -> Result<Vec<Arc<OrionGraphEngine>>> {
        let primary_shard_id = self.get_shard_for_node_sync(node_id)?;
        // For sync version, we can't await async operations, so we just get primary shard
        // This is acceptable for tests and simple use cases
        let primary_shard = self.shards.get(&primary_shard_id).ok_or_else(|| {
            ProximaDBError::Internal(format!("Shard {} not found", primary_shard_id))
        })?;

        Ok(vec![Arc::clone(&primary_shard)])
    }

    /// Execute operation on replicas based on consistency level
    async fn execute_with_consistency<F, T>(&self, node_id: &NodeId, operation: F) -> Result<T>
    where
        F: Fn(&OrionGraphEngine) -> Result<T> + Send + Sync + 'static,
        T: Send + Sync + 'static + Clone,
    {
        let replica_shards = self.get_replica_shards(node_id).await?;

        match self.config.consistency_level {
            ConsistencyLevel::Any => {
                // Execute on first available replica
                if let Some(shard) = replica_shards.first() {
                    operation(shard.as_ref())
                } else {
                    Err(ProximaDBError::Internal(
                        "No replicas available".to_string(),
                    ))
                }
            }
            ConsistencyLevel::Quorum => {
                // Execute on majority of replicas
                let required_success = (replica_shards.len() / 2) + 1;
                let mut successes = 0;
                let mut last_result: Option<T> = None;

                for shard in &replica_shards {
                    if let Ok(result) = operation(shard.as_ref()) {
                        successes += 1;
                        last_result = Some(result);

                        if successes >= required_success {
                            return Ok(last_result.unwrap());
                        }
                    }
                }

                Err(ProximaDBError::Internal(format!(
                    "Quorum not reached: {}/{} required",
                    successes, required_success
                )))
            }
            ConsistencyLevel::All => {
                // Execute on all replicas
                let mut last_result: Option<T> = None;

                for shard in &replica_shards {
                    let result = operation(shard.as_ref())?;
                    last_result = Some(result);
                }

                last_result.ok_or_else(|| {
                    ProximaDBError::Internal("No results from any replica".to_string())
                })
            }
        }
    }

    /// Sync version of execute_with_consistency (for use in sync contexts)
    fn execute_with_consistency_sync<F, T>(&self, node_id: &NodeId, operation: F) -> Result<T>
    where
        F: Fn(&OrionGraphEngine) -> Result<T>,
        T: Clone,
    {
        let replica_shards = self.get_replica_shards_sync(node_id)?;

        match self.config.consistency_level {
            ConsistencyLevel::Any => {
                // Execute on first available replica
                if let Some(shard) = replica_shards.first() {
                    operation(shard.as_ref())
                } else {
                    Err(ProximaDBError::Internal(
                        "No replicas available".to_string(),
                    ))
                }
            }
            ConsistencyLevel::Quorum => {
                // Execute on majority of replicas
                let required_success = (replica_shards.len() / 2) + 1;
                let mut successes = 0;
                let mut last_result: Option<T> = None;

                for shard in &replica_shards {
                    if let Ok(result) = operation(shard.as_ref()) {
                        successes += 1;
                        last_result = Some(result);

                        if successes >= required_success {
                            return Ok(last_result.unwrap());
                        }
                    }
                }

                Err(ProximaDBError::Internal(format!(
                    "Quorum not reached: {}/{} required",
                    successes, required_success
                )))
            }
            ConsistencyLevel::All => {
                // Execute on all replicas
                let mut last_result: Option<T> = None;

                for shard in &replica_shards {
                    let result = operation(shard.as_ref())?;
                    last_result = Some(result);
                }

                last_result.ok_or_else(|| {
                    ProximaDBError::Internal("No results from any replica".to_string())
                })
            }
        }
    }

    /// Get engine statistics
    pub async fn get_stats(&self) -> PulsarStats {
        let stats = self.stats.read().await;
        PulsarStats {
            total_nodes: stats.total_nodes,
            total_edges: stats.total_edges,
            shards_active: stats.shards_active,
            cross_shard_queries: stats.cross_shard_queries,
            replication_lag_ms: stats.replication_lag_ms,
            hot_shards: stats.hot_shards.clone(),
            load_balance_operations: stats.load_balance_operations,
        }
    }

    /// Perform cross-shard traversal
    pub async fn cross_shard_traversal(
        &self,
        start_node: &NodeId,
        max_depth: u32,
    ) -> Result<Vec<Arc<Node>>> {
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.cross_shard_queries += 1;
        }

        self.coordinator
            .distributed_bfs(start_node, max_depth)
            .await
    }

    /// Rebalance shards if needed
    pub async fn rebalance_if_needed(&self) -> Result<bool> {
        let stats = self.get_stats().await;

        // Simple hot shard detection (could be more sophisticated)
        if !stats.hot_shards.is_empty() {
            tracing::info!(
                "Hot shards detected: {:?}, considering rebalance",
                stats.hot_shards
            );

            // Update load balance stats
            {
                let mut stats_mut = self.stats.write().await;
                stats_mut.load_balance_operations += 1;
            }

            // For now, just return true to indicate rebalance was considered
            return Ok(true);
        }

        Ok(false)
    }
}

#[async_trait::async_trait]
impl GraphEngine for PulsarGraphEngine {
    async fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        let node_id = node.id.clone();

        // Route via consistent hash ring to select primary shard
        let primary_shard = self.get_primary_shard(&node_id).await?;

        // Insert into primary shard
        let result = primary_shard.insert_node(node.clone()).await?;

        // Replicate to other shards asynchronously
        tokio::spawn({
            let replication_manager = Arc::clone(&self.replication_manager);
            let node_for_replication = node;

            async move {
                if let Err(e) = replication_manager
                    .replicate_node_insert(node_for_replication)
                    .await
                {
                    tracing::error!("Failed to replicate node insert: {:?}", e);
                }
            }
        });

        // Update stats synchronously (await for test correctness)
        {
            let mut stats = self.stats.write().await;
            stats.total_nodes += 1;
        }

        Ok(result)
    }

    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        // Prefer primary shard derived from hash ring; fall back to scan
        if let Ok(primary) = self.get_primary_shard_sync(id) {
            if let Ok(Some(node)) = primary.get_node(id) {
                return Ok(Some(node));
            }
        }

        // Fallback: search all shards (in case of replication)
        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            if let Ok(Some(node)) = shard.get_node(id) {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    async fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        // TODO: Add WAL support for update_node
        // Get primary shard and update directly (avoiding sync closure issues)
        let primary_shard = self.get_primary_shard(&node.id).await?;
        primary_shard.update_node(node).await
    }

    async fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        // TODO: Add WAL support for delete_node
        // Get primary shard and delete directly
        let primary_shard = self.get_primary_shard(id).await?;
        let result = GraphEngine::delete_node(&*primary_shard, id).await?;

        // Update stats synchronously (await for test correctness)
        if result.is_some() {
            let mut stats = self.stats.write().await;
            stats.total_nodes = stats.total_nodes.saturating_sub(1);
        }

        Ok(result)
    }

    async fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        // For edges, we need to consider both source and target nodes
        // Validate both nodes exist first (looking in appropriate shards)

        // Validate source node exists (in its shard)
        let source_shard = self.get_primary_shard(&edge.from_node_id).await?;
        if source_shard.get_node(&edge.from_node_id)?.is_none() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Source node {} does not exist",
                edge.from_node_id
            )));
        }

        // Validate target node exists (in its shard - might be different from source)
        let target_shard = self.get_primary_shard(&edge.to_node_id).await?;
        if target_shard.get_node(&edge.to_node_id)?.is_none() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Target node {} does not exist",
                edge.to_node_id
            )));
        }

        // Store edge on source node's shard, using insert_edge_unchecked to skip
        // OrionGraphEngine's validation (we already validated at PULSAR level)
        let result = source_shard.insert_edge_unchecked(edge.clone()).await?;

        // Replicate edge insertion
        tokio::spawn({
            let replication_manager = Arc::clone(&self.replication_manager);
            let edge_for_replication = edge;

            async move {
                if let Err(e) = replication_manager
                    .replicate_edge_insert(edge_for_replication)
                    .await
                {
                    tracing::error!("Failed to replicate edge insert: {:?}", e);
                }
            }
        });

        // Update stats synchronously (await for test correctness)
        {
            let mut stats = self.stats.write().await;
            stats.total_edges += 1;
        }

        Ok(result)
    }

    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        // For edge lookup, we need to search across all shards since we don't know
        // which shard contains the edge. For better performance, we could maintain
        // an edge-to-shard mapping.
        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            if let Ok(Some(edge)) = shard.get_edge(id) {
                return Ok(Some(edge));
            }
        }
        Ok(None)
    }

    async fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        // TODO: Add WAL support for update_edge
        let primary_shard = self.get_primary_shard(&edge.from_node_id).await?;

        primary_shard.update_edge(edge).await
    }

    async fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        // TODO: Add WAL support for delete_edge
        // Similar to get_edge, we need to search across shards
        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            if let Ok(Some(edge)) = GraphEngine::delete_edge(&**shard, id).await {
                // Update stats (using try_write for sync context)
                if let Ok(mut stats) = self.stats.try_write() {
                    stats.total_edges = stats.total_edges.saturating_sub(1);
                }

                return Ok(Some(edge));
            }
        }
        Ok(None)
    }

    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        let primary_shard = self.get_primary_shard_sync(node_id)?;

        primary_shard.get_outgoing_edges(node_id, edge_type)
    }

    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        // Incoming edges might be in different shards, so we need cross-shard query
        let mut all_edges = Vec::new();

        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            if let Ok(edges) = shard.get_incoming_edges(node_id, edge_type) {
                all_edges.extend(edges);
            }
        }

        Ok(all_edges)
    }

    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>> {
        // Get outgoing edges and resolve target nodes
        let edges = self.get_outgoing_edges(node_id, edge_type)?;
        let mut neighbors = Vec::new();

        for edge in edges {
            // Get the target node (might be in a different shard)
            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                if let Ok(Some(node)) = shard.get_node(&edge.to_node_id) {
                    neighbors.push(node);
                    break;
                }
            }
        }

        Ok(neighbors)
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        let mut seen_ids = std::collections::HashSet::new();
        let mut all_nodes = Vec::new();

        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            if let Ok(nodes) = shard.get_nodes_by_label(label) {
                for node in nodes {
                    // Only add if we haven't seen this node ID before (deduplication)
                    if seen_ids.insert(node.id.clone()) {
                        all_nodes.push(node);
                    }
                }
            }
        }

        Ok(all_nodes)
    }

    fn node_count(&self) -> Result<usize> {
        // Count unique nodes across all shards (avoiding replication duplicates)
        let mut seen_ids = std::collections::HashSet::new();
        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            // Get all nodes from this shard and track their IDs
            let nodes = shard.get_all_nodes()?;
            for node in nodes {
                seen_ids.insert(node.id.clone());
            }
        }
        Ok(seen_ids.len())
    }

    fn edge_count(&self) -> Result<usize> {
        // Count unique edges across all shards (avoiding replication duplicates)
        let mut seen_ids = std::collections::HashSet::new();
        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            // Access the memory pool directly to get all edges
            for edge_entry in shard.memory_pool().edges.iter() {
                seen_ids.insert(edge_entry.key().clone());
            }
        }
        Ok(seen_ids.len())
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        // Synchronous aggregation of nodes from all shards to avoid nested runtimes
        let mut all_nodes = Vec::new();

        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            let mut shard_nodes = shard.get_all_nodes()?;
            all_nodes.append(&mut shard_nodes);
        }

        Ok(all_nodes)
    }

    // ===== Bulk Operations - Distribute across shards for performance =====

    async fn bulk_insert_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Arc<Node>>> {
        use std::collections::HashMap;

        // Group nodes by their target shard
        let mut shard_batches: HashMap<u32, Vec<Node>> = HashMap::new();
        for node in nodes {
            let shard_id = self.get_shard_for_node_sync(&node.id).unwrap_or(0);
            shard_batches.entry(shard_id).or_default().push(node);
        }

        // Insert each batch into its target shard using bulk operations
        let mut all_results = Vec::new();
        for (shard_id, batch) in shard_batches {
            if let Some(shard) = self.shards.get(&shard_id) {
                let batch_len = batch.len();
                let results = shard.bulk_insert_nodes(batch).await?;
                all_results.extend(results);

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    stats.total_nodes += batch_len as u64;
                }
            }
        }

        Ok(all_results)
    }

    async fn bulk_insert_edges(&self, edges: Vec<Edge>) -> Result<Vec<Arc<Edge>>> {
        use std::collections::HashMap;

        // Group edges by source node's shard (consistent with insert_edge behavior)
        let mut shard_batches: HashMap<u32, Vec<Edge>> = HashMap::new();
        for edge in edges {
            let shard_id = self.get_shard_for_node_sync(&edge.from_node_id).unwrap_or(0);
            shard_batches.entry(shard_id).or_default().push(edge);
        }

        // Insert each batch into its target shard using bulk operations
        let mut all_results = Vec::new();
        for (shard_id, batch) in shard_batches {
            if let Some(shard) = self.shards.get(&shard_id) {
                let batch_len = batch.len();
                let results = shard.bulk_insert_edges(batch).await?;
                all_results.extend(results);

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    stats.total_edges += batch_len as u64;
                }
            }
        }

        Ok(all_results)
    }

    async fn bulk_delete_nodes(&self, node_ids: Vec<NodeId>) -> Result<Vec<Option<Arc<Node>>>> {
        use std::collections::HashMap;

        // Group node IDs by their shard
        let mut shard_batches: HashMap<u32, Vec<NodeId>> = HashMap::new();
        for node_id in node_ids {
            let shard_id = self.get_shard_for_node_sync(&node_id).unwrap_or(0);
            shard_batches.entry(shard_id).or_default().push(node_id);
        }

        // Delete from each shard
        let mut all_results = Vec::new();
        for (shard_id, batch) in shard_batches {
            if let Some(shard) = self.shards.get(&shard_id) {
                let results = shard.bulk_delete_nodes(batch).await?;
                let deleted_count = results.iter().filter(|r| r.is_some()).count();
                all_results.extend(results);

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    stats.total_nodes = stats.total_nodes.saturating_sub(deleted_count as u64);
                }
            }
        }

        Ok(all_results)
    }

    async fn bulk_delete_edges(&self, edge_ids: Vec<EdgeId>) -> Result<Vec<Option<Arc<Edge>>>> {
        // For edges, we need to search all shards since we don't know which shard contains each edge
        let mut all_results = Vec::new();

        for shard_entry in self.shards.iter() {
            let shard = shard_entry.value();
            // Try to delete all edge IDs from this shard - it will skip ones it doesn't have
            let results = shard.bulk_delete_edges(edge_ids.clone()).await?;
            let deleted_count = results.iter().filter(|r| r.is_some()).count();
            all_results.extend(results.into_iter().filter(|r| r.is_some()));

            // Update stats
            if deleted_count > 0 {
                let mut stats = self.stats.write().await;
                stats.total_edges = stats.total_edges.saturating_sub(deleted_count as u64);
            }
        }

        Ok(all_results)
    }
}

impl Default for PulsarGraphEngine {
    fn default() -> Self {
        Self::new(PulsarConfig::default()).expect("Failed to create default PULSAR engine")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PropertyValue;
    // PropertyValue is now a struct, not enum - use direct field access;

    #[tokio::test]
    async fn test_pulsar_engine_creation() {
        let config = PulsarConfig::default();
        let engine = PulsarGraphEngine::new(config).unwrap();

        assert_eq!(engine.node_count().unwrap(), 0);
        assert_eq!(engine.edge_count().unwrap(), 0);

        let stats = engine.get_stats().await;
        assert_eq!(stats.shards_active, 16); // Default shard count
    }

    #[tokio::test]
    async fn test_node_distribution() {
        let config = PulsarConfig {
            shard_count: 4,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).unwrap();

        // Test nodes go to different shards
        let node1_shard = engine
            .get_shard_for_node(&"node1".to_string())
            .await
            .unwrap();
        let node2_shard = engine
            .get_shard_for_node(&"node2".to_string())
            .await
            .unwrap();

        // Shards should be within expected range
        assert!(node1_shard < 4);
        assert!(node2_shard < 4);
    }

    #[tokio::test]
    async fn test_basic_operations() {
        let engine = PulsarGraphEngine::new(PulsarConfig::default()).unwrap();

        // Create test node
        let node = Node {
            id: "test_node".to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        // Insert node
        let inserted = engine.insert_node(node).await.unwrap();
        assert_eq!(inserted.id, "test_node");

        // Get node
        let retrieved = engine.get_node(&"test_node".to_string()).unwrap().unwrap();
        assert_eq!(retrieved.id, "test_node");

        // Verify stats updated (stats now updated synchronously, no sleep needed)
        let stats = engine.get_stats().await;
        assert_eq!(stats.total_nodes, 1);
    }

    #[tokio::test]
    async fn test_hash_ring_routing_for_nodes() {
        let config = PulsarConfig {
            shard_count: 4,
            replication_factor: 1,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).unwrap();

        let node_id = "route_me".to_string();
        let node = Node {
            id: node_id.clone(),
            labels: vec!["TestLabel".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        // Determine expected shard from hash ring
        let expected_shard = engine.get_shard_for_node(&node_id).await.unwrap();

        // Insert and ensure it lands on the expected shard only (replication_factor=1)
        engine.insert_node(node).await.unwrap();

        for shard_entry in engine.shards.iter() {
            let shard_id = *shard_entry.key();
            let shard = shard_entry.value();
            let present = shard.get_node(&node_id).unwrap().is_some();
            assert_eq!(
                present,
                shard_id == expected_shard,
                "Node should be only on primary shard"
            );
        }
    }
}
