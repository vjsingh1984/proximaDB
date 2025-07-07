use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{timeout, Instant};
use tracing::{error, info, warn};

use crate::distributed::consistent_hash::{NodeId, ConsistentHashRing};
use crate::distributed::node_registry::NodeRegistry;
use crate::core::avro_unified::WalVectorBatch;

/// Replication configuration
#[derive(Debug, Clone)]
pub struct ReplicationConfig {
    pub replication_factor: u32,
    pub consistency_level: ConsistencyLevel,
    pub timeout: Duration,
    pub max_concurrent_replications: usize,
    pub retry_attempts: u32,
    pub retry_delay: Duration,
}

/// Consistency level for replication
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsistencyLevel {
    /// Only require write to succeed on primary node
    One,
    /// Require write to succeed on majority of replicas
    Quorum,
    /// Require write to succeed on all replicas
    All,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        ReplicationConfig {
            replication_factor: 3,
            consistency_level: ConsistencyLevel::Quorum,
            timeout: Duration::from_secs(30),
            max_concurrent_replications: 10,
            retry_attempts: 3,
            retry_delay: Duration::from_millis(100),
        }
    }
}

/// Replication request for a vector batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationRequest {
    pub collection_id: String,
    pub batch: WalVectorBatch,
    pub sequence_number: u64,
    pub source_node: NodeId,
    pub target_nodes: Vec<NodeId>,
    pub consistency_level: ConsistencyLevel,
}

/// Replication response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationResponse {
    pub success: bool,
    pub node_id: NodeId,
    pub sequence_number: u64,
    pub error_message: Option<String>,
    pub response_time_ms: u64,
}

/// Replication result summary
#[derive(Debug, Clone)]
pub struct ReplicationResult {
    pub success: bool,
    pub successful_nodes: Vec<NodeId>,
    pub failed_nodes: Vec<NodeId>,
    pub total_response_time_ms: u64,
    pub consistency_achieved: bool,
}

/// Replication manager handles data replication across nodes
/// 
/// This manager ensures data consistency and availability by:
/// - Replicating data to multiple nodes based on replication factor
/// - Enforcing consistency levels (One, Quorum, All)
/// - Handling node failures and retries
/// - Managing concurrent replication operations
pub struct ReplicationManager {
    config: ReplicationConfig,
    hash_ring: Arc<RwLock<ConsistentHashRing>>,
    node_registry: Arc<NodeRegistry>,
    /// Semaphore to limit concurrent replications
    replication_semaphore: Arc<Semaphore>,
    /// gRPC client for inter-node communication
    grpc_client: Option<Arc<dyn InterNodeClient + Send + Sync>>,
}

/// Trait for inter-node communication
#[async_trait::async_trait]
pub trait InterNodeClient {
    async fn replicate_batch(
        &self,
        target_node: &NodeId,
        request: ReplicationRequest,
    ) -> Result<ReplicationResponse>;
    
    async fn sync_metadata(
        &self,
        target_node: &NodeId,
        collection_id: &str,
        metadata: Vec<u8>,
    ) -> Result<()>;
}

impl ReplicationManager {
    /// Create a new replication manager
    pub fn new(
        config: ReplicationConfig,
        hash_ring: Arc<RwLock<ConsistentHashRing>>,
        node_registry: Arc<NodeRegistry>,
    ) -> Self {
        let replication_semaphore = Arc::new(Semaphore::new(config.max_concurrent_replications));
        
        ReplicationManager {
            config,
            hash_ring,
            node_registry,
            replication_semaphore,
            grpc_client: None,
        }
    }
    
    /// Set the gRPC client for inter-node communication
    pub fn set_grpc_client(&mut self, client: Arc<dyn InterNodeClient + Send + Sync>) {
        self.grpc_client = Some(client);
    }
    
    /// Replicate a vector batch to appropriate nodes
    pub async fn replicate_batch(
        &self,
        collection_id: &str,
        batch: WalVectorBatch,
        sequence_number: u64,
        source_node: NodeId,
    ) -> Result<ReplicationResult> {
        let start_time = Instant::now();
        
        // Get target nodes from hash ring
        let target_nodes = {
            let ring = self.hash_ring.read().await;
            ring.get_collection_nodes(collection_id)
        };
        
        // Filter out source node and unhealthy nodes
        let healthy_targets = self.filter_healthy_nodes(&target_nodes, &source_node).await;
        
        if healthy_targets.is_empty() {
            return Err(anyhow::anyhow!("No healthy target nodes available for replication"));
        }
        
        let request = ReplicationRequest {
            collection_id: collection_id.to_string(),
            batch,
            sequence_number,
            source_node,
            target_nodes: healthy_targets.clone(),
            consistency_level: self.config.consistency_level.clone(),
        };
        
        // Perform parallel replication to all target nodes
        let replication_results = self.replicate_to_nodes(request, &healthy_targets).await;
        
        // Analyze results for consistency
        let successful_count = replication_results.iter().filter(|r| r.success).count();
        let required_success_count = self.calculate_required_success_count(&healthy_targets);
        
        let consistency_achieved = successful_count >= required_success_count;
        
        let successful_nodes: Vec<_> = replication_results
            .iter()
            .filter(|r| r.success)
            .map(|r| r.node_id.clone())
            .collect();
        
        let failed_nodes: Vec<_> = replication_results
            .iter()
            .filter(|r| !r.success)
            .map(|r| r.node_id.clone())
            .collect();
        
        let total_response_time = start_time.elapsed().as_millis() as u64;
        
        let result = ReplicationResult {
            success: consistency_achieved,
            successful_nodes,
            failed_nodes,
            total_response_time_ms: total_response_time,
            consistency_achieved,
        };
        
        if consistency_achieved {
            info!(
                "Replication successful for collection {} sequence {}: {}/{} nodes",
                collection_id, sequence_number, successful_count, healthy_targets.len()
            );
        } else {
            error!(
                "Replication failed for collection {} sequence {}: {}/{} nodes (required: {})",
                collection_id, sequence_number, successful_count, healthy_targets.len(), required_success_count
            );
        }
        
        Ok(result)
    }
    
    /// Filter out unhealthy nodes and source node
    async fn filter_healthy_nodes(
        &self,
        target_nodes: &[NodeId],
        source_node: &NodeId,
    ) -> Vec<NodeId> {
        let mut healthy_nodes = Vec::new();
        
        for node in target_nodes {
            if node == source_node {
                continue; // Skip source node
            }
            
            if let Some(health) = self.node_registry.get_node_health(node).await {
                match health.health {
                    crate::distributed::node_registry::NodeHealth::Healthy |
                    crate::distributed::node_registry::NodeHealth::Degraded => {
                        healthy_nodes.push(node.clone());
                    }
                    _ => {
                        warn!("Skipping unhealthy node {} for replication", node.as_str());
                    }
                }
            }
        }
        
        healthy_nodes
    }
    
    /// Calculate required number of successful replications based on consistency level
    fn calculate_required_success_count(&self, target_nodes: &[NodeId]) -> usize {
        match self.config.consistency_level {
            ConsistencyLevel::One => 1,
            ConsistencyLevel::Quorum => (target_nodes.len() / 2) + 1,
            ConsistencyLevel::All => target_nodes.len(),
        }
    }
    
    /// Replicate to multiple nodes in parallel
    async fn replicate_to_nodes(
        &self,
        request: ReplicationRequest,
        target_nodes: &[NodeId],
    ) -> Vec<ReplicationResponse> {
        let grpc_client = match &self.grpc_client {
            Some(client) => client.clone(),
            None => {
                error!("No gRPC client configured for replication");
                return target_nodes
                    .iter()
                    .map(|node| ReplicationResponse {
                        success: false,
                        node_id: node.clone(),
                        sequence_number: request.sequence_number,
                        error_message: Some("No gRPC client configured".to_string()),
                        response_time_ms: 0,
                    })
                    .collect();
            }
        };
        
        // Create replication tasks for each target node
        let replication_tasks = target_nodes.iter().map(|node| {
            let client = grpc_client.clone();
            let request = request.clone();
            let node = node.clone();
            let semaphore = self.replication_semaphore.clone();
            let config = self.config.clone();
            
            async move {
                // Acquire semaphore permit to limit concurrent replications
                let _permit = semaphore.acquire().await.unwrap();
                
                Self::replicate_to_single_node(client, request, node, config).await
            }
        });
        
        // Execute all replications in parallel
        futures::future::join_all(replication_tasks).await
    }
    
    /// Replicate to a single node with retries
    async fn replicate_to_single_node(
        client: Arc<dyn InterNodeClient + Send + Sync>,
        request: ReplicationRequest,
        target_node: NodeId,
        config: ReplicationConfig,
    ) -> ReplicationResponse {
        let start_time = Instant::now();
        
        for attempt in 0..config.retry_attempts {
            match timeout(
                config.timeout,
                client.replicate_batch(&target_node, request.clone()),
            ).await {
                Ok(Ok(response)) => {
                    return response;
                }
                Ok(Err(e)) => {
                    warn!(
                        "Replication attempt {} failed for node {}: {}",
                        attempt + 1,
                        target_node.as_str(),
                        e
                    );
                    
                    if attempt < config.retry_attempts - 1 {
                        tokio::time::sleep(config.retry_delay).await;
                    }
                }
                Err(_) => {
                    warn!(
                        "Replication attempt {} timed out for node {}",
                        attempt + 1,
                        target_node.as_str()
                    );
                    
                    if attempt < config.retry_attempts - 1 {
                        tokio::time::sleep(config.retry_delay).await;
                    }
                }
            }
        }
        
        let response_time = start_time.elapsed().as_millis() as u64;
        
        ReplicationResponse {
            success: false,
            node_id: target_node,
            sequence_number: request.sequence_number,
            error_message: Some(format!(
                "All {} replication attempts failed",
                config.retry_attempts
            )),
            response_time_ms: response_time,
        }
    }
    
    /// Get replication statistics
    pub async fn get_statistics(&self) -> ReplicationStatistics {
        // This would typically track metrics over time
        // For now, returning basic configuration info
        ReplicationStatistics {
            replication_factor: self.config.replication_factor,
            consistency_level: self.config.consistency_level.clone(),
            active_replications: self.replication_semaphore.available_permits(),
            max_concurrent_replications: self.config.max_concurrent_replications,
        }
    }
}

/// Statistics about replication operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationStatistics {
    pub replication_factor: u32,
    pub consistency_level: ConsistencyLevel,
    pub active_replications: usize,
    pub max_concurrent_replications: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributed::node_registry::{NodeInfo, NodeCapabilities, HealthCheckConfig};
    use std::net::SocketAddr;
    use std::str::FromStr;
    
    struct MockInterNodeClient;
    
    #[async_trait::async_trait]
    impl InterNodeClient for MockInterNodeClient {
        async fn replicate_batch(
            &self,
            target_node: &NodeId,
            request: ReplicationRequest,
        ) -> Result<ReplicationResponse> {
            // Simulate successful replication
            Ok(ReplicationResponse {
                success: true,
                node_id: target_node.clone(),
                sequence_number: request.sequence_number,
                error_message: None,
                response_time_ms: 50,
            })
        }
        
        async fn sync_metadata(
            &self,
            _target_node: &NodeId,
            _collection_id: &str,
            _metadata: Vec<u8>,
        ) -> Result<()> {
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_replication_manager_basic() {
        let config = ReplicationConfig::default();
        let hash_ring = Arc::new(RwLock::new(ConsistentHashRing::new(100, 3)));
        let node_registry = Arc::new(NodeRegistry::new(HealthCheckConfig::default()));
        
        // Add nodes to hash ring and registry
        {
            let mut ring = hash_ring.write().await;
            ring.add_node(NodeId::new("node1".to_string())).unwrap();
            ring.add_node(NodeId::new("node2".to_string())).unwrap();
            ring.add_node(NodeId::new("node3".to_string())).unwrap();
        }
        
        // Register nodes
        for i in 1..=3 {
            let node_info = NodeInfo {
                node_id: NodeId::new(format!("node{}", i)),
                address: SocketAddr::from_str(&format!("127.0.0.1:808{}", i)).unwrap(),
                rest_port: 5678,
                grpc_port: 5679,
                region: "us-west-2".to_string(),
                zone: "us-west-2a".to_string(),
                capabilities: NodeCapabilities {
                    storage_engines: vec!["VIPER".to_string()],
                    max_memory_gb: 16,
                    max_disk_gb: 1000,
                    cpu_cores: 8,
                    supports_replication: true,
                    supports_search: true,
                    supports_indexing: true,
                },
                metadata: HashMap::new(),
            };
            node_registry.register_node(node_info).await.unwrap();
        }
        
        let mut replication_manager = ReplicationManager::new(config, hash_ring, node_registry);
        replication_manager.set_grpc_client(Arc::new(MockInterNodeClient));
        
        let stats = replication_manager.get_statistics().await;
        assert_eq!(stats.replication_factor, 3);
        assert_eq!(stats.consistency_level, ConsistencyLevel::Quorum);
    }
}