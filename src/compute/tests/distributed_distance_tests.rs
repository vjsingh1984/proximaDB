//! Unit tests for distributed distance computation system

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::compute::distributed_distance::{
        ComputeNode, DistanceComputeClient, DistanceComputeRequest, DistanceComputeResponse,
        DistributedDistanceConfig, DistributedDistanceManager, HardwareAwareExecutor,
        LoadBalancingStrategy, NodeCapabilities, NodeStatus,
    };
    use crate::compute::unified_distance::DistributedDistanceCompute;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;

    // Mock client for testing
    struct MockDistanceComputeClient {
        simulate_failure: bool,
        compute_delay_ms: u64,
    }

    impl MockDistanceComputeClient {
        fn new() -> Self {
            Self {
                simulate_failure: false,
                compute_delay_ms: 10,
            }
        }

        fn with_failure() -> Self {
            Self {
                simulate_failure: true,
                compute_delay_ms: 10,
            }
        }
    }

    #[async_trait]
    impl DistanceComputeClient for MockDistanceComputeClient {
        async fn compute_distances_remote(
            &self,
            node_address: &str,
            request: DistanceComputeRequest,
        ) -> Result<DistanceComputeResponse> {
            if self.simulate_failure {
                return Err(anyhow::anyhow!("Simulated network failure"));
            }

            // Simulate network delay
            tokio::time::sleep(tokio::time::Duration::from_millis(self.compute_delay_ms)).await;

            // Mock computation - calculate simple dot products
            let distances: Vec<f32> = request
                .vectors
                .iter()
                .map(|v| {
                    request
                        .query_vector
                        .iter()
                        .zip(v.iter())
                        .map(|(a, b)| a * b)
                        .sum()
                })
                .collect();

            Ok(DistanceComputeResponse {
                distances,
                compute_node_id: format!("mock-{}", node_address),
                compute_time_ms: self.compute_delay_ms,
                hardware_used: "Mock CPU".to_string(),
            })
        }

        async fn check_node_health(&self, _node_address: &str) -> Result<NodeStatus> {
            if self.simulate_failure {
                Ok(NodeStatus::Offline)
            } else {
                Ok(NodeStatus::Active)
            }
        }

        async fn get_node_capabilities(&self, _node_address: &str) -> Result<NodeCapabilities> {
            Ok(NodeCapabilities {
                cpu_arch: "mock".to_string(),
                simd_level: "none".to_string(),
                cpu_cores: 4,
                memory_gb: 8.0,
                gpu_available: false,
                network_bandwidth_gbps: 1.0,
            })
        }
    }

    fn create_test_node(node_id: &str, address: &str, cores: usize) -> ComputeNode {
        ComputeNode {
            node_id: node_id.to_string(),
            address: address.to_string(),
            capabilities: NodeCapabilities {
                cpu_arch: "x86_64".to_string(),
                simd_level: "AVX2".to_string(),
                cpu_cores: cores,
                memory_gb: 16.0,
                gpu_available: false,
                network_bandwidth_gbps: 10.0,
            },
            load_factor: 0.3,
            status: NodeStatus::Active,
        }
    }

    #[tokio::test]
    async fn test_distributed_manager_creation() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        // Manager should be created successfully
        assert_eq!(manager.nodes_count().await, 0);
    }

    #[tokio::test]
    async fn test_node_registration() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        let node = create_test_node("node-1", "192.168.1.10:8080", 16);

        manager.register_node(node.clone()).await.unwrap();

        let nodes = manager.get_nodes().await;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node-1");
        assert_eq!(nodes[0].capabilities.cpu_cores, 16);
    }

    #[tokio::test]
    async fn test_node_update_and_unregister() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        let node = create_test_node("node-1", "192.168.1.10:8080", 16);
        manager.register_node(node).await.unwrap();

        // Update node status
        manager
            .update_node_status("node-1", NodeStatus::Degraded)
            .await
            .unwrap();

        let nodes = manager.get_nodes().await;
        assert_eq!(nodes[0].status, NodeStatus::Degraded);

        // Unregister node
        manager.unregister_node("node-1").await.unwrap();

        assert_eq!(manager.nodes_count().await, 0);
        let nodes_after_unregister = manager.get_nodes().await;
        assert_eq!(nodes_after_unregister.len(), 0);
    }

    #[tokio::test]
    async fn test_load_balancing_strategies() {
        let client = Arc::new(MockDistanceComputeClient::new());

        // Test different load balancing strategies
        let strategies = vec![
            LoadBalancingStrategy::RoundRobin,
            LoadBalancingStrategy::LeastLoaded,
            LoadBalancingStrategy::CapabilityAware,
            LoadBalancingStrategy::LatencyAware,
        ];

        for strategy in strategies {
            let config = DistributedDistanceConfig {
                load_balancing: strategy.clone(),
                ..Default::default()
            };
            let manager = DistributedDistanceManager::new(client.clone(), config);

            // Register nodes with different capabilities
            let nodes = vec![
                ComputeNode {
                    node_id: "low-power".to_string(),
                    address: "192.168.1.10:8080".to_string(),
                    capabilities: NodeCapabilities {
                        cpu_arch: "x86_64".to_string(),
                        simd_level: "SSE2".to_string(),
                        cpu_cores: 4,
                        memory_gb: 8.0,
                        gpu_available: false,
                        network_bandwidth_gbps: 1.0,
                    },
                    load_factor: 0.8,
                    status: NodeStatus::Active,
                },
                ComputeNode {
                    node_id: "high-power".to_string(),
                    address: "192.168.1.11:8080".to_string(),
                    capabilities: NodeCapabilities {
                        cpu_arch: "x86_64".to_string(),
                        simd_level: "AVX2".to_string(),
                        cpu_cores: 32,
                        memory_gb: 64.0,
                        gpu_available: true,
                        network_bandwidth_gbps: 25.0,
                    },
                    load_factor: 0.2,
                    status: NodeStatus::Active,
                },
            ];

            for node in nodes {
                manager.register_node(node).await.unwrap();
            }

            // Select nodes using the strategy
            let selected = manager.select_compute_nodes_public(2).await.unwrap();
            assert_eq!(selected.len(), 2);

            match strategy {
                LoadBalancingStrategy::CapabilityAware => {
                    // Should prefer high-power node first
                    assert_eq!(selected[0].node_id, "high-power");
                }
                LoadBalancingStrategy::LeastLoaded => {
                    // Should prefer node with lower load factor
                    assert_eq!(selected[0].node_id, "high-power");
                }
                _ => {
                    // For other strategies, just verify we get nodes
                    assert!(!selected.is_empty());
                }
            }
        }
    }

    #[tokio::test]
    async fn test_distributed_distance_computation() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        // Register test nodes
        manager
            .register_node(create_test_node("node-1", "192.168.1.10:8080", 16))
            .await
            .unwrap();
        manager
            .register_node(create_test_node("node-2", "192.168.1.11:8080", 32))
            .await
            .unwrap();

        // Prepare test data
        let query_vector = vec![1.0, 0.0, 1.0];
        
        // Create vectors with proper lifetimes
        let node1_vec1 = vec![1.0, 0.0, 1.0]; // Should have high dot product
        let node1_vec2 = vec![0.0, 1.0, 0.0]; // Should have low dot product
        let node2_vec1 = vec![1.0, 1.0, 1.0];
        let node2_vec2 = vec![-1.0, 0.0, -1.0];
        
        let node1_vecs = vec![node1_vec1.as_slice(), node1_vec2.as_slice()];
        let node2_vecs = vec![node2_vec1.as_slice(), node2_vec2.as_slice()];
        
        let node_vectors = vec![
            ("node-1", node1_vecs.as_slice()),
            ("node-2", node2_vecs.as_slice()),
        ];

        // Execute distributed computation
        let results = manager
            .calculate_distance_distributed(
                &query_vector,
                &node_vectors,
                &DistanceMetric::DotProduct,
            )
            .await;

        assert!(results.is_ok());
        let results = results.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "node-1");
        assert_eq!(results[1].0, "node-2");

        // Each node should return distances for its vectors
        assert_eq!(results[0].1.len(), 2);
        assert_eq!(results[1].1.len(), 2);
    }

    #[tokio::test]
    async fn test_distributed_result_aggregation() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        // Mock node results with DISTANCE values (unified semantics: lower = more similar)
        // Original similarity values: 1.0, 0.9, 0.8, 0.7 
        // Converted to distance values: 0.0, 0.1, 0.2, 0.3 (inverted using 1.0 - similarity)
        let node_results = vec![
            (
                "node-1".to_string(),
                vec![(0.0, "vector_1".to_string()), (0.2, "vector_2".to_string())],
            ),
            (
                "node-2".to_string(),
                vec![(0.1, "vector_3".to_string()), (0.3, "vector_4".to_string())],
            ),
        ];

        // Aggregate results
        let aggregated = manager
            .aggregate_distributed_results(
                &node_results,
                &DistanceMetric::DotProduct, // Unified semantics: lower distance = more similar
                3,
            )
            .await
            .unwrap();

        assert_eq!(aggregated.len(), 3);

        // Should be ordered by increasing distance (unified semantics: lower = more similar)
        assert_eq!(aggregated[0].0, 0.0);  // Most similar (was 1.0 similarity)
        assert_eq!(aggregated[0].1, "vector_1");
        assert_eq!(aggregated[1].0, 0.1);  // Second most similar (was 0.9 similarity)
        assert_eq!(aggregated[1].1, "vector_3");
        assert_eq!(aggregated[2].0, 0.2);  // Third most similar (was 0.8 similarity)
        assert_eq!(aggregated[2].1, "vector_2");
    }

    #[tokio::test]
    async fn test_failure_handling_and_retry() {
        let client = Arc::new(MockDistanceComputeClient::with_failure());
        let config = DistributedDistanceConfig {
            retry_config: crate::compute::distributed_distance::RetryConfig {
                max_retries: 2,
                initial_delay_ms: 10,
                backoff_multiplier: 2.0,
            },
            ..Default::default()
        };
        let manager = DistributedDistanceManager::new(client, config);

        manager
            .register_node(create_test_node("failing-node", "192.168.1.10:8080", 16))
            .await
            .unwrap();

        // This should fail after retries
        let query_vector = vec![1.0, 0.0];
        let failing_vec = vec![1.0, 1.0];
        let failing_vecs = vec![failing_vec.as_slice()];
        let node_vectors = vec![("failing-node", failing_vecs.as_slice())];

        let result = manager
            .calculate_distance_distributed(&query_vector, &node_vectors, &DistanceMetric::Cosine)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_hardware_aware_executor() {
        let executor = HardwareAwareExecutor::new();

        let capabilities = NodeCapabilities {
            cpu_arch: std::env::consts::ARCH.to_string(),
            simd_level: "AVX2".to_string(),
            cpu_cores: 8,
            memory_gb: 16.0,
            gpu_available: false,
            network_bandwidth_gbps: 10.0,
        };

        let query = vec![1.0, 0.0, 1.0];
        let vec1 = vec![1.0, 0.0, 1.0];
        let vec2 = vec![0.0, 1.0, 0.0];
        let vec3 = vec![1.0, 1.0, 1.0];
        let vectors = vec![vec1.as_slice(), vec2.as_slice(), vec3.as_slice()];

        // Test different distance metrics
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
        ];

        for metric in metrics {
            let result = executor
                .execute(&query, &vectors, &metric, &capabilities)
                .await;
            assert!(result.is_ok());

            let distances = result.unwrap();
            assert_eq!(distances.len(), 3);

            // All distances should be finite
            for distance in &distances {
                assert!(distance.is_finite());
            }
        }
    }

    #[tokio::test]
    async fn test_compression_support() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig {
            enable_compression: true,
            ..Default::default()
        };
        let manager = DistributedDistanceManager::new(client, config);

        // Test vector compression
        let vec1 = vec![1.0, 2.0, 3.0];
        let vec2 = vec![4.0, 5.0, 6.0];
        let vectors = vec![vec1.as_slice(), vec2.as_slice()];

        let (compressed, was_compressed) = manager.compress_vectors_public(&vectors).unwrap();

        // Currently returns same data (compression not implemented yet)
        assert_eq!(compressed.len(), 2);
        // Compression flag should reflect actual implementation
        assert!(!was_compressed || compressed[0] == vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn test_vector_partitioning() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        // Create test vectors
        let vectors: Vec<Vec<f32>> = (0..10).map(|i| vec![i as f32; 3]).collect();
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        // Test partitioning across different number of nodes
        for num_nodes in 1..=5 {
            let partitions = manager.partition_vectors_public(&vector_refs, num_nodes);

            assert_eq!(partitions.len(), num_nodes);

            // Verify all vectors are included
            let total_vectors: usize = partitions.iter().map(|p| p.len()).sum();
            assert_eq!(total_vectors, 10);

            // Verify partitions are roughly equal
            let expected_per_partition = (10 + num_nodes - 1) / num_nodes;
            for partition in &partitions {
                assert!(partition.len() <= expected_per_partition);
                assert!(partition.len() > 0 || partitions.len() > 10); // Empty partitions only if more nodes than vectors
            }
        }
    }

    #[tokio::test]
    async fn test_performance_monitoring() {
        let client = Arc::new(MockDistanceComputeClient::new());
        let config = DistributedDistanceConfig::default();
        let manager = DistributedDistanceManager::new(client, config);

        manager
            .register_node(create_test_node("perf-node", "192.168.1.10:8080", 16))
            .await
            .unwrap();

        let query_vector = vec![1.0; 128]; // Large vector
        let vectors: Vec<Vec<f32>> = (0..100).map(|i| vec![i as f32 / 100.0; 128]).collect();
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let node_vectors = vec![("perf-node", vector_refs.as_slice())];

        // Time the computation
        let start = std::time::Instant::now();
        let result = manager
            .calculate_distance_distributed(
                &query_vector,
                &node_vectors,
                &DistanceMetric::DotProduct,
            )
            .await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        println!(
            "Distributed computation of 100 128-dimensional vectors took {:?}",
            elapsed
        );

        let results = result.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.len(), 100);
    }
}
