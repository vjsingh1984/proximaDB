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

//! Node Registry
//!
//! Manages node discovery, registration, and health tracking for the cluster.
//! Provides a view of all nodes in the cluster and their current status.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// Config consolidated into proximadb-config (TD-107, seam S4); re-exported
// so existing `crate::cluster::...` import paths keep resolving.
pub use proximadb_config::cluster_config::NodeRegistryConfig;

/// Role of a node in the cluster
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum NodeRole {
    /// Leader node (handles writes, coordinates cluster)
    Leader,
    /// Follower node (handles reads, receives replication)
    Follower,
    /// Candidate node (participating in leader election)
    Candidate,
    /// Observer node (read-only, no voting rights)
    Observer,
}

/// Current status of a node
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NodeStatus {
    /// Node is starting up
    Starting,
    /// Node is running and operational
    Running,
    /// Node is stopping
    Stopping,
    /// Node has stopped
    Stopped,
    /// Node is unreachable
    Unreachable,
}

/// Health status of a node
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NodeHealth {
    /// Health is unknown (no checks performed yet)
    Unknown,
    /// Node is healthy
    Healthy,
    /// Node is degraded but operational
    Degraded,
    /// Node is unhealthy
    Unhealthy,
}

/// Information about a node in the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier
    pub node_id: String,
    /// Node's advertised address (host:port)
    pub address: String,
    /// Node's role in the cluster
    pub role: NodeRole,
    /// Current status
    pub status: NodeStatus,
    /// Health status
    pub health: NodeHealth,
    /// Last heartbeat timestamp (Unix timestamp in milliseconds)
    pub last_heartbeat: i64,
    /// Node version
    pub version: String,
    /// Node capabilities
    pub capabilities: Vec<String>,
    /// Node metadata
    pub metadata: HashMap<String, String>,
    /// Number of shards hosted by this node
    pub shard_count: u32,
    /// Current load (0.0 - 1.0)
    pub load: f32,
}

impl Default for NodeInfo {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            address: String::new(),
            role: NodeRole::Follower,
            status: NodeStatus::Starting,
            health: NodeHealth::Unknown,
            last_heartbeat: 0,
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: vec!["vector_search".to_string(), "graph_operations".to_string()],
            metadata: HashMap::new(),
            shard_count: 0,
            load: 0.0,
        }
    }
}

/// Internal node state for tracking health checks
struct NodeState {
    info: NodeInfo,
    consecutive_failures: u32,
    consecutive_successes: u32,
    last_check: Option<Instant>,
}

/// Node registry for managing cluster nodes
pub struct NodeRegistry {
    config: NodeRegistryConfig,
    nodes: Arc<RwLock<HashMap<String, NodeState>>>,
}

impl NodeRegistry {
    /// Create a new node registry
    pub fn new(config: NodeRegistryConfig) -> Result<Self> {
        Ok(Self {
            config,
            nodes: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Register a new node in the cluster
    pub async fn register_node(&self, info: NodeInfo) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        tracing::info!(
            node_id = %info.node_id,
            address = %info.address,
            "Registering node in cluster"
        );

        let state = NodeState {
            info,
            consecutive_failures: 0,
            consecutive_successes: 0,
            last_check: None,
        };

        nodes.insert(state.info.node_id.clone(), state);
        Ok(())
    }

    /// Deregister a node from the cluster
    pub async fn deregister_node(&self, node_id: &str) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        if nodes.remove(node_id).is_some() {
            tracing::info!(node_id = %node_id, "Node deregistered from cluster");
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not found: {}", node_id))
        }
    }

    /// Update node status
    pub async fn update_status(&self, node_id: &str, status: NodeStatus) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        if let Some(state) = nodes.get_mut(node_id) {
            state.info.status = status;
            state.info.last_heartbeat = chrono::Utc::now().timestamp_millis();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not found: {}", node_id))
        }
    }

    /// Update node health based on health check result
    pub async fn update_health(&self, node_id: &str, healthy: bool) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        if let Some(state) = nodes.get_mut(node_id) {
            state.last_check = Some(Instant::now());

            if healthy {
                state.consecutive_failures = 0;
                state.consecutive_successes += 1;

                if state.consecutive_successes >= self.config.healthy_threshold {
                    state.info.health = NodeHealth::Healthy;
                }
            } else {
                state.consecutive_successes = 0;
                state.consecutive_failures += 1;

                if state.consecutive_failures >= self.config.unhealthy_threshold {
                    state.info.health = NodeHealth::Unhealthy;
                } else if state.consecutive_failures > 0 {
                    state.info.health = NodeHealth::Degraded;
                }
            }

            state.info.last_heartbeat = chrono::Utc::now().timestamp_millis();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not found: {}", node_id))
        }
    }

    /// Update node role
    pub async fn update_role(&self, node_id: &str, role: NodeRole) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        if let Some(state) = nodes.get_mut(node_id) {
            state.info.role = role;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not found: {}", node_id))
        }
    }

    /// Get information about a specific node
    pub async fn get_node(&self, node_id: &str) -> Option<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes.get(node_id).map(|s| s.info.clone())
    }

    /// List all nodes in the cluster
    pub async fn list_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes.values().map(|s| s.info.clone()).collect()
    }

    /// Get all healthy nodes
    pub async fn get_healthy_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes
            .values()
            .filter(|s| s.info.health == NodeHealth::Healthy)
            .map(|s| s.info.clone())
            .collect()
    }

    /// Get the current leader node
    pub async fn get_leader(&self) -> Option<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes
            .values()
            .find(|s| s.info.role == NodeRole::Leader)
            .map(|s| s.info.clone())
    }

    /// Get nodes that can serve a specific shard
    pub async fn get_nodes_for_shard(&self, _shard_id: &str) -> Vec<NodeInfo> {
        // In a full implementation, this would check shard assignments
        // For now, return all healthy nodes
        self.get_healthy_nodes().await
    }

    /// Get node count by status
    pub async fn count_by_status(&self) -> HashMap<NodeStatus, usize> {
        let nodes = self.nodes.read().await;
        let mut counts = HashMap::new();

        for state in nodes.values() {
            *counts.entry(state.info.status).or_insert(0) += 1;
        }

        counts
    }

    /// Get node count by health
    pub async fn count_by_health(&self) -> HashMap<NodeHealth, usize> {
        let nodes = self.nodes.read().await;
        let mut counts = HashMap::new();

        for state in nodes.values() {
            *counts.entry(state.info.health).or_insert(0) += 1;
        }

        counts
    }

    /// Mark stale nodes as unreachable
    pub async fn check_stale_nodes(&self) -> Vec<String> {
        let mut nodes = self.nodes.write().await;
        let timeout = Duration::from_secs(self.config.dead_node_timeout_secs);
        let mut stale_nodes = Vec::new();

        for (node_id, state) in nodes.iter_mut() {
            if let Some(last_check) = state.last_check
                && last_check.elapsed() > timeout
            {
                state.info.status = NodeStatus::Unreachable;
                state.info.health = NodeHealth::Unhealthy;
                stale_nodes.push(node_id.clone());
            }
        }

        stale_nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_registration() {
        let registry = NodeRegistry::new(NodeRegistryConfig::default())
            .expect("NodeRegistry creation should not fail");

        let info = NodeInfo {
            node_id: "node-1".to_string(),
            address: "127.0.0.1:5679".to_string(),
            ..Default::default()
        };

        registry
            .register_node(info)
            .await
            .expect("Node registration should succeed");

        let node = registry.get_node("node-1").await;
        assert!(node.is_some());
        let node = node.expect("Node should exist after registration");
        assert_eq!(node.address, "127.0.0.1:5679");
    }

    #[tokio::test]
    async fn test_health_tracking() {
        let config = NodeRegistryConfig {
            unhealthy_threshold: 2,
            healthy_threshold: 2,
            ..Default::default()
        };
        let registry = NodeRegistry::new(config).expect("NodeRegistry creation should not fail");

        let info = NodeInfo {
            node_id: "node-1".to_string(),
            ..Default::default()
        };
        registry
            .register_node(info)
            .await
            .expect("Node registration should succeed");

        // Two successful checks should mark healthy
        registry
            .update_health("node-1", true)
            .await
            .expect("Health update should succeed");
        registry
            .update_health("node-1", true)
            .await
            .expect("Health update should succeed");

        let node = registry
            .get_node("node-1")
            .await
            .expect("Node should exist after registration");
        assert_eq!(node.health, NodeHealth::Healthy);

        // Two failed checks should mark unhealthy
        registry
            .update_health("node-1", false)
            .await
            .expect("Health update should succeed");
        registry
            .update_health("node-1", false)
            .await
            .expect("Health update should succeed");

        let node = registry
            .get_node("node-1")
            .await
            .expect("Node should still exist");
        assert_eq!(node.health, NodeHealth::Unhealthy);
    }

    #[tokio::test]
    async fn test_role_update() {
        let registry = NodeRegistry::new(NodeRegistryConfig::default())
            .expect("NodeRegistry creation should not fail");

        let info = NodeInfo {
            node_id: "node-1".to_string(),
            role: NodeRole::Follower,
            ..Default::default()
        };
        registry
            .register_node(info)
            .await
            .expect("Node registration should succeed");

        registry
            .update_role("node-1", NodeRole::Leader)
            .await
            .expect("Role update should succeed");

        let leader = registry.get_leader().await;
        assert!(leader.is_some());
        let leader = leader.expect("Leader should exist after role update");
        assert_eq!(leader.node_id, "node-1");
    }
}
