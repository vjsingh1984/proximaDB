use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::{interval, sleep};
use tracing::{error, info, warn};

use crate::distributed::consistent_hash::NodeId;

/// Node information stored in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: NodeId,
    pub address: SocketAddr,
    pub rest_port: u16,
    pub grpc_port: u16,
    pub region: String,
    pub zone: String,
    pub capabilities: NodeCapabilities,
    pub metadata: HashMap<String, String>,
}

/// Capabilities of a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapabilities {
    pub storage_engines: Vec<String>,
    pub max_memory_gb: u32,
    pub max_disk_gb: u32,
    pub cpu_cores: u32,
    pub supports_replication: bool,
    pub supports_search: bool,
    pub supports_indexing: bool,
}

/// Health status of a node
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeHealth {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub node_id: NodeId,
    pub health: NodeHealth,
    pub last_check: Instant,
    pub response_time_ms: u64,
    pub error_message: Option<String>,
    pub metrics: HealthMetrics,
}

/// Health metrics for a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthMetrics {
    pub cpu_usage_percent: f32,
    pub memory_usage_percent: f32,
    pub disk_usage_percent: f32,
    pub active_connections: u32,
    pub queries_per_second: f32,
    pub error_rate_percent: f32,
}

/// Node registry with health monitoring
/// 
/// This registry maintains information about all nodes in the cluster
/// and continuously monitors their health status.
pub struct NodeRegistry {
    /// Map of node ID to node information
    nodes: Arc<RwLock<HashMap<NodeId, NodeInfo>>>,
    /// Map of node ID to health status
    health_status: Arc<RwLock<HashMap<NodeId, HealthCheckResult>>>,
    /// Health check configuration
    health_config: HealthCheckConfig,
    /// HTTP client for health checks
    http_client: reqwest::Client,
}

/// Configuration for health checks
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    pub check_interval: Duration,
    pub timeout: Duration,
    pub unhealthy_threshold: u32,
    pub healthy_threshold: u32,
    pub degraded_response_time_ms: u64,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        HealthCheckConfig {
            check_interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            unhealthy_threshold: 3,
            healthy_threshold: 2,
            degraded_response_time_ms: 1000,
        }
    }
}

impl NodeRegistry {
    /// Create a new node registry
    pub fn new(config: HealthCheckConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to create HTTP client");
        
        NodeRegistry {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            health_status: Arc::new(RwLock::new(HashMap::new())),
            health_config: config,
            http_client,
        }
    }
    
    /// Register a new node in the cluster
    pub async fn register_node(&self, node_info: NodeInfo) -> Result<()> {
        let node_id = node_info.node_id.clone();
        
        // Add to nodes registry
        {
            let mut nodes = self.nodes.write().await;
            nodes.insert(node_id.clone(), node_info);
        }
        
        // Initialize health status
        {
            let mut health = self.health_status.write().await;
            health.insert(node_id.clone(), HealthCheckResult {
                node_id: node_id.clone(),
                health: NodeHealth::Unknown,
                last_check: Instant::now(),
                response_time_ms: 0,
                error_message: None,
                metrics: HealthMetrics {
                    cpu_usage_percent: 0.0,
                    memory_usage_percent: 0.0,
                    disk_usage_percent: 0.0,
                    active_connections: 0,
                    queries_per_second: 0.0,
                    error_rate_percent: 0.0,
                },
            });
        }
        
        info!("Registered node: {}", node_id.as_str());
        Ok(())
    }
    
    /// Unregister a node from the cluster
    pub async fn unregister_node(&self, node_id: &NodeId) -> Result<()> {
        {
            let mut nodes = self.nodes.write().await;
            nodes.remove(node_id);
        }
        
        {
            let mut health = self.health_status.write().await;
            health.remove(node_id);
        }
        
        info!("Unregistered node: {}", node_id.as_str());
        Ok(())
    }
    
    /// Get information about a specific node
    pub async fn get_node(&self, node_id: &NodeId) -> Option<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes.get(node_id).cloned()
    }
    
    /// Get all registered nodes
    pub async fn get_all_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.read().await;
        nodes.values().cloned().collect()
    }
    
    /// Get all healthy nodes
    pub async fn get_healthy_nodes(&self) -> Vec<NodeInfo> {
        let nodes = self.nodes.read().await;
        let health = self.health_status.read().await;
        
        nodes
            .values()
            .filter(|node| {
                health
                    .get(&node.node_id)
                    .map(|h| h.health == NodeHealth::Healthy)
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }
    
    /// Get health status for a specific node
    pub async fn get_node_health(&self, node_id: &NodeId) -> Option<HealthCheckResult> {
        let health = self.health_status.read().await;
        health.get(node_id).cloned()
    }
    
    /// Get health status for all nodes
    pub async fn get_all_health_status(&self) -> HashMap<NodeId, HealthCheckResult> {
        let health = self.health_status.read().await;
        health.clone()
    }
    
    /// Start health monitoring background task
    pub async fn start_health_monitoring(&self) {
        let nodes = self.nodes.clone();
        let health_status = self.health_status.clone();
        let config = self.health_config.clone();
        let http_client = self.http_client.clone();
        
        tokio::spawn(async move {
            let mut interval = interval(config.check_interval);
            
            loop {
                interval.tick().await;
                
                let node_list = {
                    let nodes = nodes.read().await;
                    nodes.values().cloned().collect::<Vec<_>>()
                };
                
                // Check health of all nodes in parallel
                let health_checks = node_list.into_iter().map(|node| {
                    Self::check_node_health(node, &http_client, &config)
                });
                
                let results = futures::future::join_all(health_checks).await;
                
                // Update health status
                {
                    let mut health = health_status.write().await;
                    for result in results {
                        health.insert(result.node_id.clone(), result);
                    }
                }
            }
        });
    }
    
    /// Check the health of a single node
    async fn check_node_health(
        node: NodeInfo,
        http_client: &reqwest::Client,
        config: &HealthCheckConfig,
    ) -> HealthCheckResult {
        let start = Instant::now();
        let health_url = format!("http://{}:{}/health", node.address.ip(), node.rest_port);
        
        match http_client.get(&health_url).send().await {
            Ok(response) => {
                let response_time = start.elapsed().as_millis() as u64;
                
                if response.status().is_success() {
                    match response.json::<HealthMetrics>().await {
                        Ok(metrics) => {
                            let health = if response_time > config.degraded_response_time_ms {
                                NodeHealth::Degraded
                            } else if metrics.error_rate_percent > 10.0 {
                                NodeHealth::Degraded
                            } else {
                                NodeHealth::Healthy
                            };
                            
                            HealthCheckResult {
                                node_id: node.node_id,
                                health,
                                last_check: Instant::now(),
                                response_time_ms: response_time,
                                error_message: None,
                                metrics,
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse health metrics from node {}: {}", node.node_id.as_str(), e);
                            HealthCheckResult {
                                node_id: node.node_id,
                                health: NodeHealth::Degraded,
                                last_check: Instant::now(),
                                response_time_ms: response_time,
                                error_message: Some(format!("Failed to parse health metrics: {}", e)),
                                metrics: HealthMetrics {
                                    cpu_usage_percent: 0.0,
                                    memory_usage_percent: 0.0,
                                    disk_usage_percent: 0.0,
                                    active_connections: 0,
                                    queries_per_second: 0.0,
                                    error_rate_percent: 0.0,
                                },
                            }
                        }
                    }
                } else {
                    HealthCheckResult {
                        node_id: node.node_id,
                        health: NodeHealth::Unhealthy,
                        last_check: Instant::now(),
                        response_time_ms: response_time,
                        error_message: Some(format!("HTTP error: {}", response.status())),
                        metrics: HealthMetrics {
                            cpu_usage_percent: 0.0,
                            memory_usage_percent: 0.0,
                            disk_usage_percent: 0.0,
                            active_connections: 0,
                            queries_per_second: 0.0,
                            error_rate_percent: 100.0,
                        },
                    }
                }
            }
            Err(e) => {
                error!("Health check failed for node {}: {}", node.node_id.as_str(), e);
                HealthCheckResult {
                    node_id: node.node_id,
                    health: NodeHealth::Unhealthy,
                    last_check: Instant::now(),
                    response_time_ms: config.timeout.as_millis() as u64,
                    error_message: Some(format!("Connection failed: {}", e)),
                    metrics: HealthMetrics {
                        cpu_usage_percent: 0.0,
                        memory_usage_percent: 0.0,
                        disk_usage_percent: 0.0,
                        active_connections: 0,
                        queries_per_second: 0.0,
                        error_rate_percent: 100.0,
                    },
                }
            }
        }
    }
    
    /// Get registry statistics
    pub async fn get_statistics(&self) -> RegistryStatistics {
        let nodes = self.nodes.read().await;
        let health = self.health_status.read().await;
        
        let mut healthy_count = 0;
        let mut degraded_count = 0;
        let mut unhealthy_count = 0;
        let mut unknown_count = 0;
        
        for (node_id, _) in nodes.iter() {
            match health.get(node_id).map(|h| &h.health) {
                Some(NodeHealth::Healthy) => healthy_count += 1,
                Some(NodeHealth::Degraded) => degraded_count += 1,
                Some(NodeHealth::Unhealthy) => unhealthy_count += 1,
                Some(NodeHealth::Unknown) | None => unknown_count += 1,
            }
        }
        
        RegistryStatistics {
            total_nodes: nodes.len(),
            healthy_nodes: healthy_count,
            degraded_nodes: degraded_count,
            unhealthy_nodes: unhealthy_count,
            unknown_nodes: unknown_count,
        }
    }
}

/// Statistics about the node registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryStatistics {
    pub total_nodes: usize,
    pub healthy_nodes: usize,
    pub degraded_nodes: usize,
    pub unhealthy_nodes: usize,
    pub unknown_nodes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    
    #[tokio::test]
    async fn test_node_registry_basic() {
        let registry = NodeRegistry::new(HealthCheckConfig::default());
        
        let node_info = NodeInfo {
            node_id: NodeId::new("test_node".to_string()),
            address: SocketAddr::from_str("127.0.0.1:8080").unwrap(),
            rest_port: 5678,
            grpc_port: 5679,
            region: "us-west-2".to_string(),
            zone: "us-west-2a".to_string(),
            capabilities: NodeCapabilities {
                storage_engines: vec!["VIPER".to_string(), "LSM".to_string()],
                max_memory_gb: 16,
                max_disk_gb: 1000,
                cpu_cores: 8,
                supports_replication: true,
                supports_search: true,
                supports_indexing: true,
            },
            metadata: HashMap::new(),
        };
        
        // Register node
        registry.register_node(node_info.clone()).await.unwrap();
        
        // Check node is registered
        let retrieved = registry.get_node(&node_info.node_id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().node_id, node_info.node_id);
        
        // Check statistics
        let stats = registry.get_statistics().await;
        assert_eq!(stats.total_nodes, 1);
        assert_eq!(stats.unknown_nodes, 1); // Health not checked yet
        
        // Unregister node
        registry.unregister_node(&node_info.node_id).await.unwrap();
        
        // Check node is removed
        let retrieved = registry.get_node(&node_info.node_id).await;
        assert!(retrieved.is_none());
    }
}