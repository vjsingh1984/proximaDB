// Decompression Cache Tests

#[cfg(test)]
mod tests {
    use super::super::decompression_cache::*;
    use super::super::{DataBlock, SstRecord, CompressionAlgorithmSst};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = DecompressionCache::new(10); // 10MB cache
        
        let key = BlockCacheKey {
            file_path: "test.sst".to_string(),
            block_id: 1,
            block_offset: 0,
        };
        
        // Test miss
        assert!(cache.get(&key).await.is_none());
        
        // Test put and hit
        let block = DataBlock::new(1, vec![]);
        cache.put(key.clone(), block.clone(), Some(CompressionAlgorithmSst::Zstd))
            .await
            .unwrap();
        
        assert!(cache.get(&key).await.is_some());
        
        // Check stats
        let stats = cache.get_stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let cache = DecompressionCache::new(1); // 1MB cache - very small for testing
        
        // Fill cache with blocks
        for i in 0..100 {
            let key = BlockCacheKey {
                file_path: "test.sst".to_string(),
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
        let cache = DecompressionCache::new(10);
        
        // Add multiple blocks from same file
        for i in 0..5 {
            let key = BlockCacheKey {
                file_path: "test_file.sst".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            
            let block = DataBlock::new(i, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Add blocks from different file
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: "other_file.sst".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            
            let block = DataBlock::new(i, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Invalidate first file
        cache.invalidate_file("test_file.sst").await;
        
        // Check that blocks from first file are gone
        for i in 0..5 {
            let key = BlockCacheKey {
                file_path: "test_file.sst".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            assert!(cache.get(&key).await.is_none());
        }
        
        // Check that blocks from second file are still there
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: "other_file.sst".to_string(),
                block_id: i,
                block_offset: i as u64 * 1000,
            };
            assert!(cache.get(&key).await.is_some());
        }
    }

    #[tokio::test]
    async fn test_cache_invalidation_by_collection() {
        let cache = DecompressionCache::new(10);
        
        // Add blocks for collection1
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection1/file_{}.sst", i),
                block_id: 0,
                block_offset: 0,
            };
            
            let block = DataBlock::new(0, vec![]);
            cache.put(key, block, None).await.unwrap();
        }
        
        // Add blocks for collection2
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection2/file_{}.sst", i),
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
                file_path: format!("collection1/file_{}.sst", i),
                block_id: 0,
                block_offset: 0,
            };
            assert!(cache.get(&key).await.is_none());
        }
        
        // Check that collection2 blocks are still there
        for i in 0..3 {
            let key = BlockCacheKey {
                file_path: format!("collection2/file_{}.sst", i),
                block_id: 0,
                block_offset: 0,
            };
            assert!(cache.get(&key).await.is_some());
        }
    }

    #[tokio::test]
    async fn test_cache_hit_rate() {
        let cache = DecompressionCache::new(10);
        
        let key1 = BlockCacheKey {
            file_path: "test.sst".to_string(),
            block_id: 1,
            block_offset: 0,
        };
        
        let key2 = BlockCacheKey {
            file_path: "test.sst".to_string(),
            block_id: 2,
            block_offset: 1000,
        };
        
        // Add one block
        let block = DataBlock::new(1, vec![]);
        cache.put(key1.clone(), block, None).await.unwrap();
        
        // Perform multiple accesses
        cache.get(&key1).await; // Hit
        cache.get(&key1).await; // Hit
        cache.get(&key2).await; // Miss
        cache.get(&key1).await; // Hit
        cache.get(&key2).await; // Miss
        
        // Check hit rate
        let hit_rate = cache.get_hit_rate().await;
        // 3 hits out of 5 accesses = 60%
        assert!((hit_rate - 0.6).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_cache_prefetching() {
        let cache = DecompressionCache::new(10);
        
        // Simulate prefetching multiple blocks
        let file_path = "prefetch_test.sst";
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
            blocks.push((i, DataBlock::new(i, records), Some(CompressionAlgorithmSst::Lz4)));
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
            assert!(cache.get(&key).await.is_some());
        }
    }

    #[tokio::test]
    async fn test_cache_by_compression_algorithm() {
        let cache = DecompressionCache::new(10);
        
        // Add blocks with different compression algorithms
        let algorithms = vec![
            CompressionAlgorithmSst::Zstd,
            CompressionAlgorithmSst::Lz4,
            CompressionAlgorithmSst::Snappy,
        ];
        
        for (i, algo) in algorithms.iter().enumerate() {
            for j in 0..3 {
                let key = BlockCacheKey {
                    file_path: format!("file_{}.sst", i),
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
            enable_prefetch: true,
            prefetch_threshold: 5,
            ttl_seconds: 300,
            invalidation_check_interval_seconds: 30,
        };
        
        let cache = DecompressionCache::from_config(config);
        
        // Add a block
        let key = BlockCacheKey {
            file_path: "test.sst".to_string(),
            block_id: 1,
            block_offset: 0,
        };
        
        let block = DataBlock::new(1, vec![]);
        cache.put(key.clone(), block, None).await.unwrap();
        
        // Verify it's cached
        assert!(cache.get(&key).await.is_some());
        
        // Clear cache
        cache.clear().await;
        
        // Verify it's gone
        assert!(cache.get(&key).await.is_none());
        assert_eq!(cache.get_current_size().await, 0);
    }
}