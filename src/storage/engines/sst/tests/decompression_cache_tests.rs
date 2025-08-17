// Decompression Cache Tests

#[cfg(test)]
mod tests {
    use super::super::decompression_cache::*;
    use proximadb::core::serialization::CompressionAlgorithm};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    /// Create a test cache config with minimal values
    fn test_cache_config(max_size_mb: usize) -> CacheConfig {
        CacheConfig {
            max_size_mb,
            min_size_mb: 0,      // No minimum for tests
            max_cap_mb: 8192,    // Keep cap at 8GB
            enable_prefetch: false,
            prefetch_threshold: 3,
            ttl_seconds: 0,
            invalidation_check_interval_seconds: 0,
        }
    }

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = DecompressionCache::from_config(test_cache_config(10)); // 10MB cache
        
        let key = BlockCacheKey {
            file_path: "test.sstable".to_string(),
            block_id: 1,
            block_offset: 0,
        };
        
        // Test miss
        assert!(cache.get(&key);
        
        // Test put and hit
        let block = DataBlock::new(1, vec![]);
        cache.put(key.clone(), block.clone(), Some(CompressionAlgorithm::Zstd))
            .await
            .unwrap();
        
        assert!(cache.get(&key);
        
        // Check stats
        let stats = cache.get_stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let cache = DecompressionCache::from_config(test_cache_config(1)); // 1MB cache - very small for testing
        
        // Fill cache with blocks
        for i in 0..100 {
            let key = BlockCacheKey {
                file_path: "test.sstable".to_string(),
                block_id: i,
                block_offset: 0,
            };
            
            // Create a block with some data
            let mut records = vec![];
            for j in 0..100 {
                records.push(SstRecord {
                    id: format!("id_{}", j),
                    vector: vec![0.0; 128], // 128-dim vector
                    metadata: vec![],
                    timestamp: 0,
                    updated_at: None,
                    expires_at: None,
                    version: None,
                    is_tombstone: false,
                    sequence_number: 0,
                    level: 0,
                });
            }
            
            let block = DataBlock::new(i, records);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Check that evictions happened
        let stats = cache.get_stats().await;
        assert!(stats.evictions > 0);
        
        // Cache size should be under limit
        let current_size = cache.get_current_size().await;
        assert!(current_size <= 1024 * 1024);
    }

    #[tokio::test]
    async fn test_cache_invalidation_by_file() {
        let cache = DecompressionCache::from_config(test_cache_config(10));
        
        // Add multiple blocks from same file
        for i in 0..5 {
            let key = BlockCacheKey {
                file_path: "test_file.sstable".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            
            let block = DataBlock::new(i, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Add blocks from different file
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: "other_file.sstable".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            
            let block = DataBlock::new(i, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Invalidate first file
        cache.invalidate_file("test_file.sstable").await;
        
        // Check that blocks from first file are gone
        for i in 0..5 {
            let key = BlockCacheKey {
                file_path: "test_file.sstable".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            assert!(cache.get(&key);
        }
        
        // Check that blocks from second file are still there
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: "other_file.sstable".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            assert!(cache.get(&key);
        }
    }

    #[tokio::test]
    async fn test_cache_invalidation_by_collection() {
        let cache = DecompressionCache::from_config(test_cache_config(10));
        
        // Add blocks for collection1
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection1/file_{}.sstable", i),
                block_id: 0,
                block_offset: 0,
            };
            
            let block = DataBlock::new(0, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Add blocks for collection2
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection2/file_{}.sstable", i),
                block_id: 0,
                block_offset: 0,
            };
            
            let block = DataBlock::new(0, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Invalidate collection1
        cache.invalidate_collection("collection1").await;
        
        // Check that collection1 blocks are gone
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection1/file_{}.sstable", i),
                block_id: 0,
                block_offset: 0,
            };
            assert!(cache.get(&key);
        }
        
        // Check that collection2 blocks are still there
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection2/file_{}.sstable", i),
                block_id: 0,
                block_offset: 0,
            };
            assert!(cache.get(&key);
        }
    }

    #[tokio::test]
    async fn test_cache_hit_rate() {
        let cache = DecompressionCache::from_config(test_cache_config(10));
        
        let key1 = BlockCacheKey {
            file_path: "test.sstable".to_string(),
            block_id: 1,
            block_offset: 0,
        };
        
        let key2 = BlockCacheKey {
            file_path: "test.sstable".to_string(),
            block_id: 2,
            block_offset: 1000,
        };
        
        // Add one block
        let block = DataBlock::new(1, vec![]);
        cache.put(key1.clone(), block, None).await.unwrap();
        
        // Perform multiple accesses
        cache.get(&key).await; // Hit
        cache.get(&key).await; // Hit
        cache.get(&key).await; // Miss
        cache.get(&key).await; // Hit
        cache.get(&key).await; // Miss
        
        // Check hit rate
        let hit_rate = cache.get_hit_rate().await;
        // 3 hits out of 5 accesses = 60%
        assert!((hit_rate - 0.6).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_cache_prefetching() {
        let cache = DecompressionCache::from_config(test_cache_config(10));
        
        // Simulate prefetching multiple blocks
        let file_path = "prefetch_test.sstable";
        let mut blocks = vec![];
        
        for i in 0..10 {
            let mut records = vec![];
            for j in 0..10 {
                records.push(SstRecord {
                    id: format!("id_{}_{}",i, j),
                    vector: vec![i as f32; 64],
                    metadata: vec![],
                    timestamp: 0,
                    updated_at: None,
                    expires_at: None,
                    version: None,
                    is_tombstone: false,
                    sequence_number: 0,
                    level: 0,
                });
            }
            blocks.push((i, DataBlock::new(i, records), Some(CompressionAlgorithm::Lz4)));
        }
        
        // Prefetch all blocks
        cache.prefetch_file_blocks(file_path, blocks).await.unwrap();
        
        // Verify all blocks are cached
        for i in 0..10 {
            let key = BlockCacheKey {
                file_path: file_path.to_string(),
                block_id: i,
                block_offset: 0,
            };
            assert!(cache.get(&key);
        }
    }

    #[tokio::test]
    async fn test_cache_by_compression_algorithm() {
        let cache = DecompressionCache::from_config(test_cache_config(10));
        
        // Add blocks with different compression algorithms
        let algorithms = vec![
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
        ];
        
        for (i, algo) in algorithms.iter().enumerate() {
            for j in 0..3 {
                let key = BlockCacheKey {
                    file_path: format!("file_{}.sstable", i),
                    block_id: j,
                    block_offset: j as u64 * 1000,
                };
                
                let block = DataBlock::new(j, vec![]);
                cache.put(key, block, Some(*algo)).await.unwrap();
            }
        }
        
        // Get blocks by algorithm
        for algo in &algorithms {
            let blocks = cache.get_blocks_by_algorithm(*algo).await;
            assert_eq!(blocks.len(), 3);
        }
    }

    #[tokio::test]
    async fn test_cache_config() {
        let config = CacheConfig {
            max_size_mb: 256,
            min_size_mb: 0,      // No minimum for tests
            max_cap_mb: 8192,    // 8GB cap
            enable_prefetch: true,
            prefetch_threshold: 5,
            ttl_seconds: 300,
            invalidation_check_interval_seconds: 30,
        };
        
        let cache = DecompressionCache::from_config(config);
        
        // Add a block
        let key = BlockCacheKey {
            file_path: "test.sstable".to_string(),
            block_id: 1,
            block_offset: 0,
        };
        
        let block = DataBlock::new(1, vec![]);
        cache.put(key.clone(), block, None).await.unwrap();
        
        // Verify it's cached
        assert!(cache.get(&key);
        
        // Clear cache
        cache.clear().await;
        
        // Verify it's gone
        assert!(cache.get(&key);
        assert_eq!(cache.get_current_size().await, 0);
    }
}