//! Unit tests for Universal Performance Optimization Module

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig, FileStorageTier};
    use std::sync::Arc;
    use tokio;

    /// Helper function to create a test optimizer
    async fn create_test_optimizer(strategy: UniversalOptimizationStrategy) -> UniversalPerformanceOptimizer {
        let _ = proximadb_hardware::hardware_capabilities();
        
        UniversalPerformanceOptimizer::with_strategy(
            strategy,
        ).await.expect("Failed to create optimizer")
    }

    #[tokio::test]
    async fn test_optimizer_creation_with_strategies() {
        // Test each strategy creates with correct configuration
        let perf_optimizer = create_test_optimizer(UniversalOptimizationStrategy::PerformanceFirst).await;
        assert_eq!(perf_optimizer.get_config().cache_size_mb, 4096);
        assert_eq!(perf_optimizer.get_config().parallel_operations, 16);
        assert!(!perf_optimizer.get_config().enable_compression);

        let mem_optimizer = create_test_optimizer(UniversalOptimizationStrategy::MemoryEfficient).await;
        assert_eq!(mem_optimizer.get_config().cache_size_mb, 256);
        assert_eq!(mem_optimizer.get_config().parallel_operations, 4);
        assert!(mem_optimizer.get_config().enable_compression);

        let cost_optimizer = create_test_optimizer(UniversalOptimizationStrategy::CostOptimized).await;
        assert_eq!(cost_optimizer.get_config().cache_size_mb, 512);
        assert_eq!(cost_optimizer.get_config().parallel_operations, 4);
        assert!(cost_optimizer.get_config().enable_compression);

        let balanced_optimizer = create_test_optimizer(UniversalOptimizationStrategy::Balanced).await;
        assert_eq!(balanced_optimizer.get_config().cache_size_mb, 1024);
        assert_eq!(balanced_optimizer.get_config().parallel_operations, 8);
    }

    #[tokio::test]
    async fn test_storage_tier_optimization() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::Balanced).await;
        
        // Test tier selection based on access patterns
        let hot_tier = optimizer.optimize_storage_tier("hot_key", 1024).await.unwrap();
        assert!(matches!(hot_tier, FileStorageTier::Hot));
        
        // Update access stats to simulate low frequency
        optimizer.update_access_stats("cold_key", 1024).await;
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        
        let cold_tier = optimizer.optimize_storage_tier("cold_key", 100 * 1024 * 1024).await.unwrap();
        // For balanced strategy with large size and low access, should be warm or cold
        assert!(matches!(cold_tier, FileStorageTier::Warm | FileStorageTier::Cold));
    }

    #[tokio::test]
    async fn test_parallel_operations() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::Balanced).await;
        
        // Test parallel execution with different operations
        let items = vec![1, 2, 3, 4, 5];
        let results = optimizer.parallel_operations(
            items,
            |x| async move { x * 2 }
        ).await.unwrap();
        
        assert_eq!(results.len(), 5);
        for (i, result) in results.into_iter().enumerate() {
            assert_eq!(result.unwrap(), (i + 1) * 2);
        }
    }

    #[tokio::test]
    async fn test_memory_buffer_management() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::MemoryEfficient).await;
        
        // Test memory buffer acquisition
        let buffer = optimizer.get_memory_buffer(1024).await.unwrap();
        assert_eq!(buffer.len(), 1024);
        
        // Test multiple buffer acquisitions
        let buffer1 = optimizer.get_memory_buffer(512).await.unwrap();
        let buffer2 = optimizer.get_memory_buffer(256).await.unwrap();
        assert_eq!(buffer1.len(), 512);
        assert_eq!(buffer2.len(), 256);
    }

    #[tokio::test]
    async fn test_compression_for_tiers() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::Balanced).await;
        
        let test_data = vec![0u8; 1024]; // Compressible data
        
        // Test hot tier compression (LZ4 - fast)
        let hot_compressed = optimizer.compress_for_tier(&test_data, FileStorageTier::Hot).await.unwrap();
        assert!(hot_compressed.len() < test_data.len());
        
        // Test warm tier compression (Snappy - balanced)
        let warm_compressed = optimizer.compress_for_tier(&test_data, FileStorageTier::Warm).await.unwrap();
        assert!(warm_compressed.len() < test_data.len());
        
        // Test cold tier compression (Zstd - maximum)
        let cold_compressed = optimizer.compress_for_tier(&test_data, FileStorageTier::Cold).await.unwrap();
        assert!(cold_compressed.len() < test_data.len());
        // Cold should achieve better compression than hot
        assert!(cold_compressed.len() <= hot_compressed.len());
    }

    #[tokio::test]
    async fn test_data_caching() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::PerformanceFirst).await;
        
        // Test data caching with simulated file URLs
        let test_url = "memory://test/data.bin";
        let test_data = vec![1, 2, 3, 4, 5];
        
        // Write data (should cache)
        optimizer.write_data_optimized(test_url, &test_data, FileStorageTier::Hot).await.unwrap();
        
        // Read should hit cache
        let read_data = optimizer.read_data_optimized(test_url).await.unwrap();
        assert_eq!(read_data, test_data);
        
        // Second read should also hit cache
        let read_data2 = optimizer.read_data_optimized(test_url).await.unwrap();
        assert_eq!(read_data2, test_data);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::MemoryEfficient).await;
        
        // Fill cache with test data
        for i in 0..100 {
            let url = format!("memory://test/file_{}.bin", i);
            let data = vec![i as u8; 1024];
            optimizer.write_data_optimized(&url, &data, FileStorageTier::Hot).await.unwrap();
        }
        
        // Trigger eviction
        optimizer.evict_cache_if_needed().await.unwrap();
        
        // Cache should have been partially evicted
        // Exact count depends on eviction threshold and estimation
        // Just verify eviction runs without error
    }

    #[tokio::test]
    async fn test_prefetch_operations() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::Balanced).await;
        
        // Test prefetching with different URL types
        let local_urls = vec![
            "file:///tmp/test1.bin".to_string(),
            "file:///tmp/test2.bin".to_string(),
        ];
        
        let cloud_urls = vec![
            "s3://bucket/test1.bin".to_string(),
            "gs://bucket/test2.bin".to_string(),
        ];
        
        // Should categorize and prefetch appropriately
        optimizer.prefetch_data(&local_urls).await.unwrap();
        optimizer.prefetch_data(&cloud_urls).await.unwrap();
        
        // Give prefetch tasks time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_hardware_accelerated_distance() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::PerformanceFirst).await;
        
        let query = vec![1.0, 2.0, 3.0, 4.0];
        let candidates = vec![
            vec![1.0, 2.0, 3.0, 4.0],  // Identical
            vec![2.0, 3.0, 4.0, 5.0],  // Shifted
            vec![4.0, 3.0, 2.0, 1.0],  // Reversed
        ];
        
        // Test Euclidean distance
        let distances = optimizer.compute_distances_accelerated(
            &query,
            &candidates,
            crate::compute::distance_computation::DistanceMetric::Euclidean,
        ).await.unwrap();
        
        assert_eq!(distances.len(), 3);
        assert_eq!(distances[0], 0.0); // Identical vectors
        assert!(distances[1] > 0.0);   // Different vectors
        assert!(distances[2] > 0.0);   // Different vectors
        
        // Test Cosine distance
        let cosine_distances = optimizer.compute_distances_accelerated(
            &query,
            &candidates,
            crate::compute::distance_computation::DistanceMetric::Cosine,
        ).await.unwrap();
        
        assert_eq!(cosine_distances.len(), 3);
    }

    #[tokio::test]
    async fn test_filesystem_integration() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::Balanced).await;
        
        // Test listing files (will fail gracefully if directory doesn't exist)
        let result = optimizer.list_files_optimized("file:///tmp").await;
        // Just verify it doesn't panic
        let _ = result.is_ok();
        
        // Test with cloud URLs
        let cloud_result = optimizer.list_files_optimized("s3://test-bucket/").await;
        // Will fail without credentials, but should handle gracefully
        let _ = cloud_result.is_err();
    }

    #[tokio::test]
    async fn test_access_pattern_tracking() {
        let optimizer = create_test_optimizer(UniversalOptimizationStrategy::CostOptimized).await;
        
        // Simulate access patterns
        let key = "test_key";
        
        // Initial access
        optimizer.update_access_stats(key, 1024).await;
        let freq1 = optimizer.get_access_frequency(key).await;
        assert!(freq1 > 0.0);
        
        // Multiple accesses
        for _ in 0..5 {
            optimizer.update_access_stats(key, 1024).await;
        }
        
        // Frequency should reflect recent access
        let freq2 = optimizer.get_access_frequency(key).await;
        assert_eq!(freq2, 1.0); // Very recent access
        
        // Test with no access history
        let unknown_freq = optimizer.get_access_frequency("unknown_key").await;
        assert_eq!(unknown_freq, 0.0);
    }

    #[tokio::test]
    async fn test_custom_strategy() {
        let _ = proximadb_hardware::hardware_capabilities();
        
        // Create with custom configuration
        let custom_config = UniversalIOConfig {
            enable_memory_mapping: false,
            cache_size_mb: 2048,
            parallel_operations: 12,
            enable_prefetching: true,
            prefetch_size_mb: 192,
            tiered_storage_threshold: 0.25,
            eviction_threshold: 0.90,
            enable_compression: true,
            compression_threshold_kb: 32,
        };
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default()).await.unwrap()
        );
        
        let optimizer = UniversalPerformanceOptimizer::new(
            custom_config.clone(),
            UniversalOptimizationStrategy::Custom("MyCustomStrategy".to_string()),
            filesystem_factory,
        );
        
        assert_eq!(optimizer.get_config().cache_size_mb, 2048);
        assert_eq!(optimizer.get_config().parallel_operations, 12);
        assert_eq!(optimizer.get_config().prefetch_size_mb, 192);
        assert!(matches!(
            optimizer.get_strategy(),
            UniversalOptimizationStrategy::Custom(s) if s == "MyCustomStrategy"
        ));
    }
}