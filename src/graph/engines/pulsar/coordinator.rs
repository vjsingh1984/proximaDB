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

//! # PULSAR Query Coordinator Module
//!
//! Coordinates cross-shard queries and distributed graph traversal operations.
//! Implements distributed BFS/DFS algorithms that work across multiple shards.

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use super::sharding::ConsistentHashRing;
use crate::graph::engines::orion::OrionGraphEngine;
use crate::graph::engines::GraphEngine;
use crate::graph::{Edge, EdgeId, Node, NodeId};
use dashmap::DashMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{Duration, Instant};

/// Query coordinator for distributed graph operations
#[derive(Debug)]
pub struct QueryCoordinator {
    /// Reference to all shard engines
    shards: Arc<DashMap<u32, Arc<OrionGraphEngine>>>,
    /// Consistent hash ring for shard routing
    hash_ring: Arc<RwLock<ConsistentHashRing>>,
    /// Semaphore to limit concurrent cross-shard queries
    query_semaphore: Arc<Semaphore>,
    /// Query statistics
    stats: Arc<RwLock<CoordinatorStats>>,
}

/// Coordinator statistics
#[derive(Debug, Default)]
pub struct CoordinatorStats {
    pub cross_shard_queries: u64,
    pub total_query_time_ms: u64,
    pub average_query_time_ms: f64,
    pub nodes_visited_across_shards: u64,
    pub shard_hits_per_query: HashMap<u64, Vec<u32>>,
    pub query_complexity_distribution: HashMap<String, u64>, // "simple", "medium", "complex"
}

/// Distributed traversal context
#[derive(Debug)]
pub struct TraversalContext {
    /// Nodes visited so far
    visited: HashSet<NodeId>,
    /// Query ID for tracking
    query_id: u64,
    /// Start time for performance tracking
    start_time: Instant,
    /// Maximum depth to traverse
    max_depth: u32,
    /// Current depth
    current_depth: u32,
    /// Shards involved in this query
    shards_involved: HashSet<u32>,
}

/// Distributed BFS result
#[derive(Debug)]
pub struct DistributedTraversalResult {
    /// Nodes found during traversal
    pub nodes: Vec<Arc<Node>>,
    /// Paths taken (optional, for path-finding queries)
    pub paths: Vec<Vec<NodeId>>,
    /// Query statistics
    pub stats: TraversalStats,
}

/// Statistics for a single traversal operation
#[derive(Debug)]
pub struct TraversalStats {
    /// Total time taken
    pub duration_ms: u64,
    /// Number of shards involved
    pub shards_involved: u32,
    /// Total nodes visited
    pub nodes_visited: u32,
    /// Total edges traversed
    pub edges_traversed: u32,
    /// Cross-shard hops
    pub cross_shard_hops: u32,
}

impl QueryCoordinator {
    /// Create a new query coordinator
    pub fn new(
        shards: Arc<DashMap<u32, Arc<OrionGraphEngine>>>,
        hash_ring: Arc<RwLock<ConsistentHashRing>>,
        max_concurrent_queries: usize,
    ) -> Self {
        Self {
            shards,
            hash_ring,
            query_semaphore: Arc::new(Semaphore::new(max_concurrent_queries)),
            stats: Arc::new(RwLock::new(CoordinatorStats::default())),
        }
    }

    /// Perform distributed BFS traversal
    pub async fn distributed_bfs(
        &self,
        start_node: &NodeId,
        max_depth: u32,
    ) -> Result<Vec<Arc<Node>>> {
        let _permit = self
            .query_semaphore
            .acquire()
            .await
            .map_err(|e| ProximaDBError::Internal(e.to_string()))?;

        let query_id = self.generate_query_id().await;
        let mut context = TraversalContext {
            visited: HashSet::new(),
            query_id,
            start_time: Instant::now(),
            max_depth,
            current_depth: 0,
            shards_involved: HashSet::new(),
        };

        let result = self
            .execute_distributed_bfs(start_node, &mut context)
            .await?;

        // Update statistics
        self.update_query_stats(&context).await;

        Ok(result)
    }

    /// Execute the actual distributed BFS
    async fn execute_distributed_bfs(
        &self,
        start_node: &NodeId,
        context: &mut TraversalContext,
    ) -> Result<Vec<Arc<Node>>> {
        let mut result_nodes = Vec::new();
        let mut queue = VecDeque::new();

        // Add start node to queue
        queue.push_back((start_node.clone(), 0u32)); // (node_id, depth)
        context.visited.insert(start_node.clone());

        while let Some((current_node_id, depth)) = queue.pop_front() {
            if depth > context.max_depth {
                continue;
            }

            // Get the shard for this node
            let shard_id = self.get_shard_for_node(&current_node_id).await?;
            context.shards_involved.insert(shard_id);

            // Get the node from its shard
            if let Some(shard) = self.shards.get(&shard_id) {
                // Get the actual node
                if let Some(node) = shard.get_node(&current_node_id)? {
                    result_nodes.push(node);

                    // Get neighbors if we haven't reached max depth
                    if depth < context.max_depth {
                        let neighbors = self
                            .get_cross_shard_neighbors_internal(&current_node_id, None, context)
                            .await?;

                        // Add unvisited neighbors to queue
                        for neighbor in neighbors {
                            if !context.visited.contains(&neighbor.id) {
                                context.visited.insert(neighbor.id.clone());
                                queue.push_back((neighbor.id.clone(), depth + 1));
                            }
                        }
                    }
                }
            }
        }

        Ok(result_nodes)
    }

    /// Get neighbors across shards
    pub async fn get_cross_shard_neighbors(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Node>>> {
        let mut context = TraversalContext {
            visited: HashSet::new(),
            query_id: self.generate_query_id().await,
            start_time: Instant::now(),
            max_depth: 1,
            current_depth: 0,
            shards_involved: HashSet::new(),
        };

        self.get_cross_shard_neighbors_internal(node_id, edge_type, &mut context)
            .await
    }

    /// Internal method for getting cross-shard neighbors
    async fn get_cross_shard_neighbors_internal(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
        context: &mut TraversalContext,
    ) -> Result<Vec<Arc<Node>>> {
        let mut all_neighbors = Vec::new();

        // Get outgoing edges from the node's primary shard
        let primary_shard_id = self.get_shard_for_node(node_id).await?;
        context.shards_involved.insert(primary_shard_id);

        if let Some(primary_shard) = self.shards.get(&primary_shard_id) {
            // Get outgoing edges
            let outgoing_edges = primary_shard.get_outgoing_edges(node_id, edge_type)?;
            if !outgoing_edges.is_empty() {
                for edge in outgoing_edges {
                    // The target node might be in a different shard
                    let target_shard_id = self.get_shard_for_node(&edge.to_node_id).await?;
                    context.shards_involved.insert(target_shard_id);

                    if let Some(target_shard) = self.shards.get(&target_shard_id) {
                        if let Some(target_node) = target_shard.get_node(&edge.to_node_id)? {
                            all_neighbors.push(target_node);
                        }
                    }
                }
            }
        }

        // Also check for incoming edges (could be from other shards)
        // This requires checking all shards, which is expensive but thorough
        for shard_entry in self.shards.iter() {
            let shard_id = *shard_entry.key();
            let shard = shard_entry.value();

            // Skip if this is the primary shard (already checked above)
            if shard_id == primary_shard_id {
                continue;
            }

            // Check for incoming edges to our node
            let incoming_edges = shard.get_incoming_edges(node_id, edge_type)?;
            if !incoming_edges.is_empty() {
                context.shards_involved.insert(shard_id);

                for edge in incoming_edges {
                    // The source node is in this shard
                    if let Some(source_node) = shard.get_node(&edge.from_node_id)? {
                        all_neighbors.push(source_node);
                    }
                }
            }
        }

        Ok(all_neighbors)
    }

    /// Perform distributed DFS traversal
    pub async fn distributed_dfs(
        &self,
        start_node: &NodeId,
        max_depth: u32,
    ) -> Result<DistributedTraversalResult> {
        let _permit = self
            .query_semaphore
            .acquire()
            .await
            .map_err(|e| ProximaDBError::Internal(e.to_string()))?;

        let query_id = self.generate_query_id().await;
        let mut context = TraversalContext {
            visited: HashSet::new(),
            query_id,
            start_time: Instant::now(),
            max_depth,
            current_depth: 0,
            shards_involved: HashSet::new(),
        };

        let mut result_nodes = Vec::new();
        let mut paths = Vec::new();
        let mut current_path = Vec::new();

        self.execute_distributed_dfs(
            start_node,
            &mut context,
            &mut result_nodes,
            &mut current_path,
            &mut paths,
        )
        .await?;

        let stats = self
            .create_traversal_stats(&context, result_nodes.len())
            .await;
        self.update_query_stats(&context).await;

        Ok(DistributedTraversalResult {
            nodes: result_nodes,
            paths,
            stats,
        })
    }

    /// Execute recursive DFS
    async fn execute_distributed_dfs(
        &self,
        node_id: &NodeId,
        context: &mut TraversalContext,
        result_nodes: &mut Vec<Arc<Node>>,
        current_path: &mut Vec<NodeId>,
        all_paths: &mut Vec<Vec<NodeId>>,
    ) -> Result<()> {
        if context.current_depth > context.max_depth {
            return Ok(());
        }

        if context.visited.contains(node_id) {
            return Ok(());
        }

        // Mark as visited
        context.visited.insert(node_id.clone());
        current_path.push(node_id.clone());

        // Get the node
        let shard_id = self.get_shard_for_node(node_id).await?;
        context.shards_involved.insert(shard_id);

        if let Some(shard) = self.shards.get(&shard_id) {
            if let Some(node) = shard.get_node(node_id)? {
                result_nodes.push(node);

                // Get neighbors and recurse
                if context.current_depth < context.max_depth {
                    context.current_depth += 1;

                    let neighbors = self
                        .get_cross_shard_neighbors_internal(node_id, None, context)
                        .await?;

                    for neighbor in neighbors {
                        if !context.visited.contains(&neighbor.id) {
                            Box::pin(self.execute_distributed_dfs(
                                &neighbor.id,
                                context,
                                result_nodes,
                                current_path,
                                all_paths,
                            ))
                            .await?;
                        }
                    }

                    context.current_depth -= 1;
                }
            }
        }

        // Add current path to results if it's complete
        if context.current_depth == context.max_depth || current_path.len() > 1 {
            all_paths.push(current_path.clone());
        }

        current_path.pop();
        Ok(())
    }

    /// Find shortest path between two nodes across shards
    pub async fn find_shortest_path(
        &self,
        start_node: &NodeId,
        end_node: &NodeId,
        max_depth: u32,
    ) -> Result<Option<Vec<NodeId>>> {
        let _permit = self
            .query_semaphore
            .acquire()
            .await
            .map_err(|e| ProximaDBError::Internal(e.to_string()))?;

        let mut context = TraversalContext {
            visited: HashSet::new(),
            query_id: self.generate_query_id().await,
            start_time: Instant::now(),
            max_depth,
            current_depth: 0,
            shards_involved: HashSet::new(),
        };

        // BFS for shortest path
        let mut queue = VecDeque::new();
        let mut parent_map: HashMap<NodeId, NodeId> = HashMap::new();

        queue.push_back((start_node.clone(), 0u32));
        context.visited.insert(start_node.clone());

        while let Some((current_node_id, depth)) = queue.pop_front() {
            if depth > max_depth {
                continue;
            }

            if current_node_id == *end_node {
                // Found the target, reconstruct path
                let mut path = Vec::new();
                let mut current = end_node.clone();

                while let Some(parent) = parent_map.get(&current) {
                    path.push(current.clone());
                    current = parent.clone();
                }
                path.push(start_node.clone());
                path.reverse();

                self.update_query_stats(&context).await;
                return Ok(Some(path));
            }

            // Get neighbors
            let neighbors = self
                .get_cross_shard_neighbors_internal(&current_node_id, None, &mut context)
                .await?;

            for neighbor in neighbors {
                if !context.visited.contains(&neighbor.id) {
                    context.visited.insert(neighbor.id.clone());
                    parent_map.insert(neighbor.id.clone(), current_node_id.clone());
                    queue.push_back((neighbor.id.clone(), depth + 1));
                }
            }
        }

        self.update_query_stats(&context).await;
        Ok(None) // No path found
    }

    /// Get shard ID for a node using the hash ring
    async fn get_shard_for_node(&self, node_id: &NodeId) -> Result<u32> {
        let hash_ring = self.hash_ring.read().await;
        Ok(hash_ring.get_shard(node_id))
    }

    /// Generate unique query ID
    async fn generate_query_id(&self) -> u64 {
        let mut stats = self.stats.write().await;
        stats.cross_shard_queries += 1;
        stats.cross_shard_queries
    }

    /// Update query statistics
    async fn update_query_stats(&self, context: &TraversalContext) {
        let duration = context.start_time.elapsed().as_millis() as u64;
        let mut stats = self.stats.write().await;

        stats.total_query_time_ms += duration;
        stats.average_query_time_ms =
            stats.total_query_time_ms as f64 / stats.cross_shard_queries as f64;
        stats.nodes_visited_across_shards += context.visited.len() as u64;

        // Record shards involved in this query
        stats.shard_hits_per_query.insert(
            context.query_id,
            context.shards_involved.iter().cloned().collect(),
        );

        // Classify query complexity
        let complexity = if context.shards_involved.len() == 1 {
            "simple"
        } else if context.shards_involved.len() <= 3 {
            "medium"
        } else {
            "complex"
        };

        *stats
            .query_complexity_distribution
            .entry(complexity.to_string())
            .or_insert(0) += 1;
    }

    /// Create traversal statistics
    async fn create_traversal_stats(
        &self,
        context: &TraversalContext,
        nodes_found: usize,
    ) -> TraversalStats {
        TraversalStats {
            duration_ms: context.start_time.elapsed().as_millis() as u64,
            shards_involved: context.shards_involved.len() as u32,
            nodes_visited: context.visited.len() as u32,
            edges_traversed: nodes_found as u32, // Approximation
            cross_shard_hops: context.shards_involved.len().saturating_sub(1) as u32,
        }
    }

    /// Get coordinator statistics
    pub async fn get_stats(&self) -> CoordinatorStats {
        let stats = self.stats.read().await;
        CoordinatorStats {
            cross_shard_queries: stats.cross_shard_queries,
            total_query_time_ms: stats.total_query_time_ms,
            average_query_time_ms: stats.average_query_time_ms,
            nodes_visited_across_shards: stats.nodes_visited_across_shards,
            shard_hits_per_query: stats.shard_hits_per_query.clone(),
            query_complexity_distribution: stats.query_complexity_distribution.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphMemoryPool;
    use crate::graph::engines::pulsar::sharding::ConsistentHashRing;

    fn create_test_setup() -> (
        Arc<DashMap<u32, Arc<OrionGraphEngine>>>,
        Arc<RwLock<ConsistentHashRing>>,
    ) {
        let memory_pool = Arc::new(GraphMemoryPool::new());
        let shards = Arc::new(DashMap::new());

        for i in 0..4 {
            let engine = Arc::new(OrionGraphEngine::with_memory_pool(Arc::clone(&memory_pool)));
            shards.insert(i, engine);
        }

        let hash_ring = Arc::new(RwLock::new(ConsistentHashRing::new(4)));
        (shards, hash_ring)
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let (shards, hash_ring) = create_test_setup();
        let coordinator = QueryCoordinator::new(shards, hash_ring, 10);

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.cross_shard_queries, 0);
    }

    #[tokio::test]
    async fn test_get_shard_for_node() {
        let (shards, hash_ring) = create_test_setup();
        let coordinator = QueryCoordinator::new(shards, hash_ring, 10);

        let shard_id = coordinator.get_shard_for_node("test_node").await.unwrap();
        assert!(shard_id < 4);

        // Same node should always map to same shard
        let shard_id2 = coordinator.get_shard_for_node("test_node").await.unwrap();
        assert_eq!(shard_id, shard_id2);
    }

    #[tokio::test]
    async fn test_distributed_bfs() {
        let (shards, hash_ring) = create_test_setup();
        let coordinator = QueryCoordinator::new(shards, hash_ring, 10);

        // For this test, BFS on a non-existent node should return empty results
        let results = coordinator.distributed_bfs("nonexistent", 2).await.unwrap();
        assert!(results.is_empty());

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.cross_shard_queries, 1);
    }

    #[tokio::test]
    async fn test_query_id_generation() {
        let (shards, hash_ring) = create_test_setup();
        let coordinator = QueryCoordinator::new(shards, hash_ring, 10);

        let id1 = coordinator.generate_query_id().await;
        let id2 = coordinator.generate_query_id().await;

        assert_ne!(id1, id2);
        assert!(id2 > id1);
    }

    #[tokio::test]
    async fn test_traversal_context() {
        let context = TraversalContext {
            visited: HashSet::new(),
            query_id: 1,
            start_time: Instant::now(),
            max_depth: 3,
            current_depth: 0,
            shards_involved: HashSet::new(),
        };

        assert_eq!(context.query_id, 1);
        assert_eq!(context.max_depth, 3);
        assert_eq!(context.current_depth, 0);
        assert!(context.visited.is_empty());
        assert!(context.shards_involved.is_empty());
    }
}
