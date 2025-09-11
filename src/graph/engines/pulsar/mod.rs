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

pub mod coordinator;
pub mod monitoring;
pub mod optimizer;
pub mod replication;
pub mod sharding;

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::engines::{GraphEngine, orion::OrionGraphEngine};
use crate::graph::{Edge, EdgeId, GraphMemoryPool, Node, NodeId};
use dashmap::DashMap;
use std::collections::HashMap;
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
        let memory_pool = Arc::new(GraphMemoryPool::new());

        // Initialize shards
        let shards = Arc::new(DashMap::new());
        for shard_id in 0..config.shard_count {
            let shard_engine =
                Arc::new(OrionGraphEngine::with_memory_pool(Arc::clone(&memory_pool)));
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

    /// Get primary shard engine for a node
    async fn get_primary_shard(&self, node_id: &NodeId) -> Result<Arc<OrionGraphEngine>> {
        let shard_id = self.get_shard_for_node(node_id).await?;

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
                let mut last_result = None;

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
                let mut last_result = None;

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

impl GraphEngine for PulsarGraphEngine {
    fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        let node_id = node.id.clone();

        // Use async runtime to get primary shard
        let rt = tokio::runtime::Handle::current();
        let primary_shard = rt.block_on(self.get_primary_shard(&node_id))?;

        // Insert into primary shard
        let result = primary_shard.insert_node(node.clone())?;

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

        // Update stats
        tokio::spawn({
            let stats = Arc::clone(&self.stats);
            async move {
                let mut stats = stats.write().await;
                stats.total_nodes += 1;
            }
        });

        Ok(result)
    }

    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.execute_with_consistency(id, |shard| shard.get_node(id)))
    }

    fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        let node_id = node.id.clone();
        let rt = tokio::runtime::Handle::current();

        rt.block_on(
            self.execute_with_consistency(&node_id, |shard| shard.update_node(node.clone())),
        )
    }

    fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        let rt = tokio::runtime::Handle::current();
        let result =
            rt.block_on(self.execute_with_consistency(id, |shard| shard.delete_node(id)))?;

        // Update stats
        if result.is_some() {
            tokio::spawn({
                let stats = Arc::clone(&self.stats);
                async move {
                    let mut stats = stats.write().await;
                    stats.total_nodes = stats.total_nodes.saturating_sub(1);
                }
            });
        }

        Ok(result)
    }

    fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        // For edges, we need to consider both source and target nodes
        // For simplicity, use source node's shard as primary
        let rt = tokio::runtime::Handle::current();
        let primary_shard = rt.block_on(self.get_primary_shard(&edge.from_node_id))?;

        let result = primary_shard.insert_edge(edge.clone())?;

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

        // Update stats
        tokio::spawn({
            let stats = Arc::clone(&self.stats);
            async move {
                let mut stats = stats.write().await;
                stats.total_edges += 1;
            }
        });

        Ok(result)
    }

    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        // For edge lookup, we need to search across all shards since we don't know
        // which shard contains the edge. For better performance, we could maintain
        // an edge-to-shard mapping.
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                if let Ok(Some(edge)) = shard.get_edge(id) {
                    return Ok(Some(edge));
                }
            }
            Ok(None)
        })
    }

    fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        let rt = tokio::runtime::Handle::current();
        let primary_shard = rt.block_on(self.get_primary_shard(&edge.from_node_id))?;

        primary_shard.update_edge(edge)
    }

    fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        // Similar to get_edge, we need to search across shards
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                if let Ok(Some(edge)) = shard.delete_edge(id) {
                    // Update stats
                    let mut stats = self.stats.write().await;
                    stats.total_edges = stats.total_edges.saturating_sub(1);

                    return Ok(Some(edge));
                }
            }
            Ok(None)
        })
    }

    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        let rt = tokio::runtime::Handle::current();
        let primary_shard = rt.block_on(self.get_primary_shard(node_id))?;

        primary_shard.get_outgoing_edges(node_id, edge_type)
    }

    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        // Incoming edges might be in different shards, so we need cross-shard query
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            let mut all_edges = Vec::new();

            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                if let Ok(edges) = shard.get_incoming_edges(node_id, edge_type) {
                    all_edges.extend(edges);
                }
            }

            Ok(all_edges)
        })
    }

    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(
            self.coordinator
                .get_cross_shard_neighbors(node_id, edge_type),
        )
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            let mut all_nodes = Vec::new();

            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                if let Ok(nodes) = shard.get_nodes_by_label(label) {
                    all_nodes.extend(nodes);
                }
            }

            Ok(all_nodes)
        })
    }

    fn node_count(&self) -> Result<usize> {
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            let mut total = 0;

            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                total += shard.node_count()?;
            }

            Ok(total)
        })
    }

    fn edge_count(&self) -> Result<usize> {
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            let mut total = 0;

            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                total += shard.edge_count()?;
            }

            Ok(total)
        })
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        let rt = tokio::runtime::Handle::current();

        rt.block_on(async {
            let mut all_nodes = Vec::new();

            for shard_entry in self.shards.iter() {
                let shard = shard_entry.value();
                let mut shard_nodes = shard.get_all_nodes()?;
                all_nodes.append(&mut shard_nodes);
            }

            Ok(all_nodes)
        })
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
    use crate::proto::proximadb_v1::property_value::Value;

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
        let node1_shard = engine.get_shard_for_node("node1").await.unwrap();
        let node2_shard = engine.get_shard_for_node("node2").await.unwrap();

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
            created_at: None,
            updated_at: None,
        };

        // Insert node
        let inserted = engine.insert_node(node).unwrap();
        assert_eq!(inserted.id, "test_node");

        // Give some time for async operations
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Get node
        let retrieved = engine.get_node("test_node").unwrap().unwrap();
        assert_eq!(retrieved.id, "test_node");

        // Verify stats updated
        let stats = engine.get_stats().await;
        assert_eq!(stats.total_nodes, 1);
    }
}
