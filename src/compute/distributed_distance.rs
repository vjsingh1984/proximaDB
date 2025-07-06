//! Distributed Distance Computation for Multi-Node ProximaDB
//!
//! This module extends the unified distance system for distributed operations:
//! - Node-aware computation scheduling
//! - Hardware heterogeneity handling
//! - Network-optimized batch processing
//! - Fault tolerance and retries
//! - Result aggregation across nodes

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::distance::DistanceMetric;
use super::unified_distance::{
    DistanceResultOrdering, DistributedDistanceCompute, UnifiedDistanceCompute,
};

/// Node information for distributed computing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeNode {
    /// Unique node identifier
    pub node_id: String,
    /// Node address for network communication
    pub address: String,
    /// Hardware capabilities of the node
    pub capabilities: NodeCapabilities,
    /// Current load factor (0.0 = idle, 1.0 = fully loaded)
    pub load_factor: f32,
    /// Node health status
    pub status: NodeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapabilities {
    /// CPU architecture (x86_64, aarch64, etc.)
    pub cpu_arch: String,
    /// SIMD capabilities (AVX2, NEON, etc.)
    pub simd_level: String,
    /// Number of CPU cores
    pub cpu_cores: usize,
    /// Available memory in GB
    pub memory_gb: f32,
    /// GPU availability
    pub gpu_available: bool,
    /// Network bandwidth in Gbps
    pub network_bandwidth_gbps: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeStatus {
    Active,
    Degraded,
    Offline,
    Maintenance,
}

/// Distributed distance computation manager
pub struct DistributedDistanceManager {
    /// Node registry
    nodes: Arc<RwLock<Vec<ComputeNode>>>,
    /// Local unified distance compute
    local_compute: UnifiedDistanceCompute,
    /// Network client for remote calls
    network_client: Arc<dyn DistanceComputeClient>,
    /// Configuration
    config: DistributedDistanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedDistanceConfig {
    /// Maximum vectors per batch for network transfer
    pub max_batch_size: usize,
    /// Timeout for remote computations (ms)
    pub compute_timeout_ms: u64,
    /// Enable compression for network transfer
    pub enable_compression: bool,
    /// Retry configuration
    pub retry_config: RetryConfig,
    /// Load balancing strategy
    pub load_balancing: LoadBalancingStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Initial retry delay (ms)
    pub initial_delay_ms: u64,
    /// Exponential backoff multiplier
    pub backoff_multiplier: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoadBalancingStrategy {
    /// Round-robin distribution
    RoundRobin,
    /// Least loaded node first
    LeastLoaded,
    /// Hardware capability aware
    CapabilityAware,
    /// Network latency aware
    LatencyAware,
}

/// Network client trait for distributed distance computation
#[async_trait]
pub trait DistanceComputeClient: Send + Sync {
    /// Compute distances on a remote node
    async fn compute_distances_remote(
        &self,
        node_address: &str,
        request: DistanceComputeRequest,
    ) -> Result<DistanceComputeResponse>;

    /// Check node health
    async fn check_node_health(&self, node_address: &str) -> Result<NodeStatus>;

    /// Get node capabilities
    async fn get_node_capabilities(&self, node_address: &str) -> Result<NodeCapabilities>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistanceComputeRequest {
    /// Query vector
    pub query_vector: Vec<f32>,
    /// Target vectors (compressed if enabled)
    pub vectors: Vec<Vec<f32>>,
    /// Distance metric to use
    pub distance_metric: DistanceMetric,
    /// Request ID for tracking
    pub request_id: String,
    /// Compression used
    pub compressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistanceComputeResponse {
    /// Computed distances
    pub distances: Vec<f32>,
    /// Node that computed the distances
    pub compute_node_id: String,
    /// Computation time in ms
    pub compute_time_ms: u64,
    /// Hardware used (CPU/GPU/SIMD level)
    pub hardware_used: String,
}

impl DistributedDistanceManager {
    /// Create new distributed distance manager
    pub fn new(
        network_client: Arc<dyn DistanceComputeClient>,
        config: DistributedDistanceConfig,
    ) -> Self {
        Self {
            nodes: Arc::new(RwLock::new(Vec::new())),
            local_compute: UnifiedDistanceCompute::default(),
            network_client,
            config,
        }
    }

    /// Register a compute node
    pub async fn register_node(&self, node: ComputeNode) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        // Check if node already exists
        if let Some(existing) = nodes.iter_mut().find(|n| n.node_id == node.node_id) {
            *existing = node;
            info!("🔄 Updated existing compute node: {}", existing.node_id);
        } else {
            info!("➕ Registered new compute node: {}", node.node_id);
            nodes.push(node);
        }

        Ok(())
    }

    /// Remove a compute node
    pub async fn unregister_node(&self, node_id: &str) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        nodes.retain(|n| n.node_id != node_id);
        info!("➖ Unregistered compute node: {}", node_id);
        Ok(())
    }

    /// Update node status
    pub async fn update_node_status(&self, node_id: &str, status: NodeStatus) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.status = status.clone();
            debug!("📊 Updated node {} status to {:?}", node_id, status);
        }
        Ok(())
    }

    /// Select nodes for computation based on load balancing strategy
    async fn select_compute_nodes(&self, num_nodes: usize) -> Result<Vec<ComputeNode>> {
        let nodes = self.nodes.read().await;
        let active_nodes: Vec<_> = nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Active)
            .cloned()
            .collect();

        if active_nodes.is_empty() {
            return Err(anyhow::anyhow!("No active compute nodes available"));
        }

        let selected = match self.config.load_balancing {
            LoadBalancingStrategy::RoundRobin => {
                // Simple round-robin selection
                active_nodes.into_iter().take(num_nodes).collect()
            }
            LoadBalancingStrategy::LeastLoaded => {
                // Sort by load factor and select least loaded
                let mut sorted = active_nodes;
                sorted.sort_by(|a, b| a.load_factor.partial_cmp(&b.load_factor).unwrap());
                sorted.into_iter().take(num_nodes).collect()
            }
            LoadBalancingStrategy::CapabilityAware => {
                // Prefer nodes with better hardware capabilities
                let mut sorted = active_nodes;
                sorted.sort_by_key(|n| {
                    let score = n.capabilities.cpu_cores * 1000
                        + (n.capabilities.memory_gb * 10.0) as usize
                        + if n.capabilities.gpu_available {
                            10000
                        } else {
                            0
                        };
                    std::cmp::Reverse(score)
                });
                sorted.into_iter().take(num_nodes).collect()
            }
            LoadBalancingStrategy::LatencyAware => {
                // For now, use round-robin (latency tracking would require historical data)
                active_nodes.into_iter().take(num_nodes).collect()
            }
        };

        Ok(selected)
    }

    /// Partition vectors across nodes for distributed computation
    fn partition_vectors<'a>(
        &self,
        vectors: &'a [&'a [f32]],
        num_nodes: usize,
    ) -> Vec<Vec<&'a [f32]>> {
        let chunk_size = (vectors.len() + num_nodes - 1) / num_nodes;
        vectors
            .chunks(chunk_size)
            .map(|chunk| chunk.to_vec())
            .collect()
    }

    /// Compress vectors for network transfer if enabled
    fn compress_vectors(&self, vectors: &[&[f32]]) -> Result<(Vec<Vec<f32>>, bool)> {
        if self.config.enable_compression {
            // TODO: Implement actual compression (e.g., quantization, zstd)
            // For now, just convert to owned vectors
            Ok((vectors.iter().map(|v| v.to_vec()).collect(), false))
        } else {
            Ok((vectors.iter().map(|v| v.to_vec()).collect(), false))
        }
    }

    /// Execute computation with retry logic
    async fn compute_with_retry(
        &self,
        node: &ComputeNode,
        request: DistanceComputeRequest,
    ) -> Result<DistanceComputeResponse> {
        let mut attempts = 0;
        let mut delay = self.config.retry_config.initial_delay_ms;

        loop {
            match self
                .network_client
                .compute_distances_remote(&node.address, request.clone())
                .await
            {
                Ok(response) => return Ok(response),
                Err(e) if attempts < self.config.retry_config.max_retries => {
                    attempts += 1;
                    warn!(
                        "⚠️ Computation failed on node {} (attempt {}/{}): {}",
                        node.node_id, attempts, self.config.retry_config.max_retries, e
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    delay = (delay as f32 * self.config.retry_config.backoff_multiplier) as u64;
                }
                Err(e) => {
                    error!(
                        "❌ Computation failed on node {} after {} attempts",
                        node.node_id, attempts
                    );
                    return Err(e);
                }
            }
        }
    }

    /// Get the number of registered nodes (for testing)
    pub async fn nodes_count(&self) -> usize {
        self.nodes.read().await.len()
    }

    /// Get all registered nodes (for testing)
    pub async fn get_nodes(&self) -> Vec<ComputeNode> {
        self.nodes.read().await.clone()
    }

    /// Select nodes for computation (exposed for testing)
    pub async fn select_compute_nodes_public(&self, num_nodes: usize) -> Result<Vec<ComputeNode>> {
        self.select_compute_nodes(num_nodes).await
    }

    /// Partition vectors for testing
    pub fn partition_vectors_public<'a>(
        &self,
        vectors: &'a [&'a [f32]],
        num_nodes: usize,
    ) -> Vec<Vec<&'a [f32]>> {
        self.partition_vectors(vectors, num_nodes)
    }

    /// Compress vectors for testing
    pub fn compress_vectors_public(&self, vectors: &[&[f32]]) -> Result<(Vec<Vec<f32>>, bool)> {
        self.compress_vectors(vectors)
    }
}

#[async_trait]
impl DistributedDistanceCompute for DistributedDistanceManager {
    async fn calculate_distance_distributed(
        &self,
        query: &[f32],
        node_vectors: &[(&str, &[&[f32]])],
        metric: &DistanceMetric,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        let mut all_results = Vec::new();

        for (node_id, vectors) in node_vectors {
            // Find the node
            let nodes = self.nodes.read().await;
            let node = nodes
                .iter()
                .find(|n| n.node_id == *node_id)
                .ok_or_else(|| anyhow::anyhow!("Node {} not found", node_id))?
                .clone();
            drop(nodes);

            // Prepare request
            let (compressed_vectors, compressed) = self.compress_vectors(vectors)?;
            let request = DistanceComputeRequest {
                query_vector: query.to_vec(),
                vectors: compressed_vectors,
                distance_metric: metric.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                compressed,
            };

            // Execute with retry
            let response = self.compute_with_retry(&node, request).await?;

            info!(
                "✅ Computed {} distances on node {} in {}ms using {}",
                response.distances.len(),
                response.compute_node_id,
                response.compute_time_ms,
                response.hardware_used
            );

            all_results.push((node_id.to_string(), response.distances));
        }

        Ok(all_results)
    }

    async fn aggregate_distributed_results(
        &self,
        node_results: &[(String, Vec<(f32, String)>)],
        metric: &DistanceMetric,
        k: usize,
    ) -> Result<Vec<(f32, String)>> {
        // Merge all results from different nodes
        let mut all_results = Vec::new();
        for (node_id, results) in node_results {
            for (distance, vector_id) in results {
                all_results.push((*distance, vector_id.clone()));
            }
        }

        // Sort and limit using unified distance system
        DistanceResultOrdering::sort_and_limit(&mut all_results, metric, &self.local_compute, k);

        Ok(all_results)
    }
}

/// Hardware-aware distance computation executor
pub struct HardwareAwareExecutor {
    /// Unified distance compute for local execution
    unified_compute: UnifiedDistanceCompute,
}

impl HardwareAwareExecutor {
    pub fn new() -> Self {
        Self {
            unified_compute: UnifiedDistanceCompute::default(),
        }
    }

    /// Execute distance computation with optimal hardware selection
    pub async fn execute(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
        metric: &DistanceMetric,
        capabilities: &NodeCapabilities,
    ) -> Result<Vec<f32>> {
        // Use unified compute which already handles hardware acceleration
        let distances = self
            .unified_compute
            .calculate_distance_batch(query, vectors, metric);

        debug!(
            "🎯 Computed {} distances using {:?} on {} with SIMD level {}",
            distances.len(),
            metric,
            capabilities.cpu_arch,
            capabilities.simd_level
        );

        Ok(distances)
    }
}

impl Default for DistributedDistanceConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 10000,
            compute_timeout_ms: 30000,
            enable_compression: true,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                backoff_multiplier: 2.0,
            },
            load_balancing: LoadBalancingStrategy::CapabilityAware,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_capabilities() {
        let capabilities = NodeCapabilities {
            cpu_arch: "x86_64".to_string(),
            simd_level: "AVX2".to_string(),
            cpu_cores: 16,
            memory_gb: 32.0,
            gpu_available: true,
            network_bandwidth_gbps: 10.0,
        };

        assert_eq!(capabilities.cpu_arch, "x86_64");
        assert!(capabilities.gpu_available);
    }

    #[test]
    fn test_partition_vectors() {
        let manager = DistributedDistanceManager::new(
            Arc::new(MockDistanceComputeClient),
            DistributedDistanceConfig::default(),
        );

        let vectors: Vec<Vec<f32>> = (0..10).map(|i| vec![i as f32; 4]).collect();
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let partitions = manager.partition_vectors(&vector_refs, 3);

        assert_eq!(partitions.len(), 3);
        assert_eq!(partitions[0].len(), 4); // First partition has 4 vectors
        assert_eq!(partitions[1].len(), 4); // Second partition has 4 vectors
        assert_eq!(partitions[2].len(), 2); // Third partition has 2 vectors
    }

    #[tokio::test]
    async fn test_node_registration() {
        let manager = DistributedDistanceManager::new(
            Arc::new(MockDistanceComputeClient),
            DistributedDistanceConfig::default(),
        );

        let node = ComputeNode {
            node_id: "node-1".to_string(),
            address: "192.168.1.10:8080".to_string(),
            capabilities: NodeCapabilities {
                cpu_arch: "x86_64".to_string(),
                simd_level: "AVX2".to_string(),
                cpu_cores: 16,
                memory_gb: 32.0,
                gpu_available: false,
                network_bandwidth_gbps: 1.0,
            },
            load_factor: 0.5,
            status: NodeStatus::Active,
        };

        manager.register_node(node.clone()).await.unwrap();

        let nodes = manager.nodes.read().await;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node-1");
    }

    // Mock implementation for testing
    struct MockDistanceComputeClient;

    #[async_trait]
    impl DistanceComputeClient for MockDistanceComputeClient {
        async fn compute_distances_remote(
            &self,
            _node_address: &str,
            request: DistanceComputeRequest,
        ) -> Result<DistanceComputeResponse> {
            // Mock computation - return zeros
            let distances = vec![0.0; request.vectors.len()];

            Ok(DistanceComputeResponse {
                distances,
                compute_node_id: "mock-node".to_string(),
                compute_time_ms: 10,
                hardware_used: "Mock CPU".to_string(),
            })
        }

        async fn check_node_health(&self, _node_address: &str) -> Result<NodeStatus> {
            Ok(NodeStatus::Active)
        }

        async fn get_node_capabilities(&self, _node_address: &str) -> Result<NodeCapabilities> {
            Ok(NodeCapabilities {
                cpu_arch: "mock".to_string(),
                simd_level: "none".to_string(),
                cpu_cores: 1,
                memory_gb: 1.0,
                gpu_available: false,
                network_bandwidth_gbps: 1.0,
            })
        }
    }
}
