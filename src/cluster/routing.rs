/*
 * Copyright 2025 ProximaDB
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

//! Shard-Aware Routing Service
//!
//! Provides intelligent routing of requests to appropriate nodes based on:
//! - Shard placement and replication
//! - Node health and load balancing
//! - Read/write operation requirements
//! - Locality preferences

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::node_registry::{NodeHealth, NodeInfo, NodeRole};
use super::shard::ShardId;

/// Configuration for the routing service
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    /// Enable read replicas for load distribution
    pub enable_read_replicas: bool,
    /// Maximum number of retries for failed requests
    pub max_retries: u32,
    /// Timeout for routing decisions in milliseconds
    pub routing_timeout_ms: u64,
    /// Enable sticky sessions for consistency
    pub sticky_sessions: bool,
    /// Load balancing strategy
    pub load_balancing: LoadBalancingStrategy,
    /// Enable locality-aware routing
    pub locality_aware: bool,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            enable_read_replicas: true,
            max_retries: 3,
            routing_timeout_ms: 100,
            sticky_sessions: false,
            load_balancing: LoadBalancingStrategy::RoundRobin,
            locality_aware: true,
        }
    }
}

/// Load balancing strategies
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadBalancingStrategy {
    /// Round-robin across available nodes
    RoundRobin,
    /// Route to node with lowest load
    LeastLoaded,
    /// Route to node with lowest latency
    LeastLatency,
    /// Random node selection
    Random,
    /// Weighted round-robin based on node capacity
    WeightedRoundRobin,
}

/// Result of a routing decision
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// Target node for the request
    pub target_node: NodeInfo,
    /// Shard ID if applicable
    pub shard_id: Option<ShardId>,
    /// Whether this is a primary or replica
    pub is_primary: bool,
    /// Retry count if this is a retry
    pub retry_count: u32,
    /// Routing latency in microseconds
    pub routing_latency_us: u64,
}

/// Routing request type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperationType {
    /// Read operation (can use replicas)
    Read,
    /// Write operation (must go to primary)
    Write,
    /// Admin operation (must go to leader)
    Admin,
}

/// Routing statistics
#[derive(Debug, Default)]
struct RoutingStats {
    total_routes: u64,
    primary_routes: u64,
    replica_routes: u64,
    retries: u64,
    failures: u64,
    total_latency_us: u64,
}

/// Internal node state for routing
struct RoutableNode {
    info: NodeInfo,
    round_robin_counter: u64,
    last_latency_ms: f64,
    weight: u32,
}

/// Routing service for shard-aware request routing
pub struct RoutingService {
    config: RoutingConfig,
    /// Routing table: shard_id -> (primary_node, replica_nodes)
    routing_table: Arc<RwLock<HashMap<ShardId, ShardRoute>>>,
    /// All known nodes with routing state
    nodes: Arc<RwLock<HashMap<String, RoutableNode>>>,
    /// Round-robin counter for load balancing
    rr_counter: Arc<RwLock<u64>>,
    /// Routing statistics
    stats: Arc<RwLock<RoutingStats>>,
}

/// Route information for a shard
#[derive(Debug, Clone)]
pub struct ShardRoute {
    /// Primary node for this shard
    pub primary: String,
    /// Replica nodes for this shard
    pub replicas: Vec<String>,
    /// Shard is available for routing
    pub available: bool,
}

impl RoutingService {
    /// Create a new routing service
    pub fn new(config: RoutingConfig) -> Result<Self> {
        Ok(Self {
            config,
            routing_table: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            rr_counter: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(RoutingStats::default())),
        })
    }

    /// Route a request to an appropriate node
    pub async fn route(
        &self,
        collection_id: &str,
        operation: OperationType,
        vector_id: Option<&str>,
    ) -> Result<RouteDecision> {
        let start = Instant::now();

        // Determine shard for this request
        let shard_id = self.compute_shard_id(collection_id, vector_id).await?;

        // Get route for the shard
        let route = {
            let table = self.routing_table.read().await;
            table.get(&shard_id).cloned()
        };

        let target_node = match &route {
            Some(r) if r.available => {
                self.select_node(r, operation).await?
            }
            _ => {
                // No specific route, use any available node
                self.select_any_node(operation).await?
            }
        };

        let is_primary = match &route {
            Some(r) => target_node.node_id == r.primary,
            None => true,
        };

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_routes += 1;
            if is_primary {
                stats.primary_routes += 1;
            } else {
                stats.replica_routes += 1;
            }
            stats.total_latency_us += start.elapsed().as_micros() as u64;
        }

        Ok(RouteDecision {
            target_node,
            shard_id: Some(shard_id),
            is_primary,
            retry_count: 0,
            routing_latency_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Compute shard ID for a request
    async fn compute_shard_id(
        &self,
        collection_id: &str,
        vector_id: Option<&str>,
    ) -> Result<ShardId> {
        // Simple hash-based sharding
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        if let Some(vid) = vector_id {
            vid.hash(&mut hasher);
        }

        Ok(ShardId::new(format!("shard_{:016x}", hasher.finish())))
    }

    /// Select a node based on operation type and load balancing
    async fn select_node(
        &self,
        route: &ShardRoute,
        operation: OperationType,
    ) -> Result<NodeInfo> {
        let nodes = self.nodes.read().await;

        match operation {
            OperationType::Write | OperationType::Admin => {
                // Write operations must go to primary
                nodes
                    .get(&route.primary)
                    .filter(|n| n.info.health == NodeHealth::Healthy)
                    .map(|n| n.info.clone())
                    .ok_or_else(|| anyhow::anyhow!("Primary node unavailable"))
            }
            OperationType::Read => {
                if self.config.enable_read_replicas && !route.replicas.is_empty() {
                    // Try to use a replica
                    self.select_from_nodes(&route.replicas, &nodes).await
                        .or_else(|_| {
                            // Fall back to primary
                            nodes
                                .get(&route.primary)
                                .map(|n| n.info.clone())
                                .ok_or_else(|| anyhow::anyhow!("No nodes available"))
                        })
                } else {
                    // Use primary
                    nodes
                        .get(&route.primary)
                        .map(|n| n.info.clone())
                        .ok_or_else(|| anyhow::anyhow!("Primary node unavailable"))
                }
            }
        }
    }

    /// Select a node from a list using load balancing
    async fn select_from_nodes(
        &self,
        node_ids: &[String],
        nodes: &HashMap<String, RoutableNode>,
    ) -> Result<NodeInfo> {
        let healthy_nodes: Vec<_> = node_ids
            .iter()
            .filter_map(|id| nodes.get(id))
            .filter(|n| n.info.health == NodeHealth::Healthy)
            .collect();

        if healthy_nodes.is_empty() {
            return Err(anyhow::anyhow!("No healthy nodes available"));
        }

        let selected = match self.config.load_balancing {
            LoadBalancingStrategy::RoundRobin => {
                let mut counter = self.rr_counter.write().await;
                let idx = (*counter as usize) % healthy_nodes.len();
                *counter += 1;
                &healthy_nodes[idx]
            }
            LoadBalancingStrategy::LeastLoaded => {
                healthy_nodes
                    .iter()
                    .min_by(|a, b| a.info.load.partial_cmp(&b.info.load).unwrap())
                    .unwrap()
            }
            LoadBalancingStrategy::LeastLatency => {
                healthy_nodes
                    .iter()
                    .min_by(|a, b| a.last_latency_ms.partial_cmp(&b.last_latency_ms).unwrap())
                    .unwrap()
            }
            LoadBalancingStrategy::Random => {
                use rand::Rng;
                let idx = rand::thread_rng().gen_range(0..healthy_nodes.len());
                &healthy_nodes[idx]
            }
            LoadBalancingStrategy::WeightedRoundRobin => {
                // Simplified weighted round-robin
                let total_weight: u32 = healthy_nodes.iter().map(|n| n.weight).sum();
                let mut counter = self.rr_counter.write().await;
                let target = (*counter as u32) % total_weight;
                *counter += 1;

                let mut cumulative = 0u32;
                healthy_nodes
                    .iter()
                    .find(|n| {
                        cumulative += n.weight;
                        cumulative > target
                    })
                    .unwrap()
            }
        };

        Ok(selected.info.clone())
    }

    /// Select any available node for operations without specific routing
    async fn select_any_node(&self, operation: OperationType) -> Result<NodeInfo> {
        let nodes = self.nodes.read().await;

        let healthy_nodes: Vec<_> = nodes
            .values()
            .filter(|n| n.info.health == NodeHealth::Healthy)
            .filter(|n| {
                match operation {
                    OperationType::Admin => n.info.role == NodeRole::Leader,
                    _ => true,
                }
            })
            .collect();

        if healthy_nodes.is_empty() {
            return Err(anyhow::anyhow!("No healthy nodes available"));
        }

        // Use round-robin for selection
        let mut counter = self.rr_counter.write().await;
        let idx = (*counter as usize) % healthy_nodes.len();
        *counter += 1;

        Ok(healthy_nodes[idx].info.clone())
    }

    /// Update routing table for a shard
    pub async fn update_route(&self, shard_id: ShardId, route: ShardRoute) -> Result<()> {
        let mut table = self.routing_table.write().await;
        table.insert(shard_id, route);
        Ok(())
    }

    /// Register a node for routing
    pub async fn register_node(&self, info: NodeInfo, weight: u32) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        nodes.insert(
            info.node_id.clone(),
            RoutableNode {
                info,
                round_robin_counter: 0,
                last_latency_ms: 0.0,
                weight,
            },
        );
        Ok(())
    }

    /// Update node latency for latency-based routing
    pub async fn update_node_latency(&self, node_id: &str, latency_ms: f64) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            // Exponential moving average
            node.last_latency_ms = node.last_latency_ms * 0.7 + latency_ms * 0.3;
        }
        Ok(())
    }

    /// Get routing statistics
    pub async fn get_stats(&self) -> RoutingStatsSummary {
        let stats = self.stats.read().await;
        RoutingStatsSummary {
            total_routes: stats.total_routes,
            primary_routes: stats.primary_routes,
            replica_routes: stats.replica_routes,
            retries: stats.retries,
            failures: stats.failures,
            avg_latency_us: if stats.total_routes > 0 {
                stats.total_latency_us / stats.total_routes
            } else {
                0
            },
        }
    }
}

/// Summary of routing statistics
#[derive(Debug, Clone)]
pub struct RoutingStatsSummary {
    pub total_routes: u64,
    pub primary_routes: u64,
    pub replica_routes: u64,
    pub retries: u64,
    pub failures: u64,
    pub avg_latency_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_routing_service_creation() {
        let config = RoutingConfig::default();
        let service = RoutingService::new(config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_shard_id_computation() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let shard1 = service.compute_shard_id("collection1", Some("vec1")).await.unwrap();
        let shard2 = service.compute_shard_id("collection1", Some("vec1")).await.unwrap();
        let shard3 = service.compute_shard_id("collection1", Some("vec2")).await.unwrap();

        // Same input should produce same shard
        assert_eq!(shard1.id(), shard2.id());
        // Different input should produce different shard
        assert_ne!(shard1.id(), shard3.id());
    }

    #[tokio::test]
    async fn test_node_registration() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let info = NodeInfo {
            node_id: "node-1".to_string(),
            address: "127.0.0.1:5679".to_string(),
            health: NodeHealth::Healthy,
            ..Default::default()
        };

        service.register_node(info, 100).await.unwrap();

        let result = service.select_any_node(OperationType::Read).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().node_id, "node-1");
    }
}
