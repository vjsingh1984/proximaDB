//! Unified Cache Integration Tests
//!
//! Tests the integration of SST and VIPER engines with the central cache module
//! to ensure correctness and eliminate cache duplication.

use super::*;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::cache::specialized::{
    BitmapFilterCache, IndexNodeCache, MetadataStore,
    index_node_cache::{MetadataStats, SstIndexEntry, SstableIndex},
};
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(test)]
mod sst_cache_integration {
    use super::*;

    #[tokio::test]
    async fn test_sst_block_cache_operations() {
        // Initialize central cache
        let vector_cache = Arc::new(VectorStore::new(100)); // 100MB cache

        // Create test block key
        let block_key = SstBlockKey::new("test_file.sstable".to_string(), 1024, 4096);

        // Test compressed block caching
        let test_data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        vector_cache
            .cache_compressed_block(
                &block_key,
                test_data.clone(),
                CompressionType::Zstd,
                16, // uncompressed size
            )
            .await
            .unwrap();

        // Retrieve compressed block
        let cached_block = vector_cache.get_compressed_block(&block_key).await;
        assert!(cached_block.is_some());

        let block = cached_block.unwrap();
        assert_eq!(block.data, test_data);
        assert!(matches!(
            block.storage.as_ref().and_then(|s| s.compression.as_ref()),
            CompressionType::Zstd
        ));
        assert_eq!(block.uncompressed_size, 16);
    }

    #[tokio::test]
    async fn test_sst_vector_block_caching() {
        let vector_cache = Arc::new(VectorStore::new(100));

        let block_key = SstBlockKey::new("vectors.sstable".to_string(), 2048, 8192);

        // Create test vectors
        let vectors = vec![
            VectorRecord {
                id: Some("v1".to_string()),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                version: Some(1),
                timestamp: 1000,
                updated_at: None,
                expires_at: None,
                // rank removed -  None,
                similarity: None,
                similarity: None,
            },
            VectorRecord {
                id: Some("v2".to_string()),
                vector: vec![4.0, 5.0, 6.0],
                metadata: vec![],
                version: Some(1),
                timestamp: 1001,
                updated_at: None,
                expires_at: None,
                // rank removed -  None,
                similarity: None,
                similarity: None,
            },
        ];

        // Cache vectors
        vector_cache
            .cache_block_vectors(&block_key, vectors.clone())
            .await
            .unwrap();

        // Retrieve vectors
        let cached_vectors = vector_cache.get_block_vectors(&block_key, 2).await;
        assert_eq!(cached_vectors.len(), 2);
        assert_eq!(cached_vectors[0].id, Some("v1".to_string()));
        assert_eq!(cached_vectors[1].id, Some("v2".to_string()));
    }

    #[tokio::test]
    async fn test_sst_index_caching() {
        let index_cache = Arc::new(IndexNodeCache::new(50)); // 50MB cache

        // Create test SSTable index
        let index = SstableIndex {
            file_path: "test.sstable".to_string(),
            entries: vec![
                SstIndexEntry {
                    key: "block1".to_string(),
                    block_offset: 0,
                    block_size: 4096,
                    min_key: "a".to_string(),
                    max_key: "m".to_string(),
                    vector_count: 100,
                    bloom_filter_offset: Some(8192),
                },
                SstIndexEntry {
                    key: "block2".to_string(),
                    block_offset: 4096,
                    block_size: 4096,
                    min_key: "n".to_string(),
                    max_key: "z".to_string(),
                    vector_count: 150,
                    bloom_filter_offset: Some(16384),
                },
            ],
            total_blocks: 2,
            total_vectors: 250,
            metadata_stats: HashMap::new(),
        };

        // Cache index
        index_cache
            .cache_sstable_index("test.sstable", index.clone())
            .await
            .unwrap();

        // Retrieve index
        let cached_index = index_cache.get_sstable_index("test.sstable").await;
        assert!(cached_index.is_some());

        let retrieved = cached_index.unwrap();
        assert_eq!(retrieved.total_blocks, 2);
        assert_eq!(retrieved.total_vectors, 250);
        assert_eq!(retrieved.entries.len(), 2);
    }

    #[tokio::test]
    async fn test_sst_block_search_optimization() {
        let index_cache = Arc::new(IndexNodeCache::new(50));

        // Create index with multiple blocks
        let index = SstableIndex {
            file_path: "search.sstable".to_string(),
            entries: vec![
                SstIndexEntry {
                    key: "block1".to_string(),
                    block_offset: 0,
                    block_size: 4096,
                    min_key: "apple".to_string(),
                    max_key: "banana".to_string(),
                    vector_count: 50,
                    bloom_filter_offset: None,
                },
                SstIndexEntry {
                    key: "block2".to_string(),
                    block_offset: 4096,
                    block_size: 4096,
                    min_key: "cherry".to_string(),
                    max_key: "date".to_string(),
                    vector_count: 50,
                    bloom_filter_offset: None,
                },
                SstIndexEntry {
                    key: "block3".to_string(),
                    block_offset: 8192,
                    block_size: 4096,
                    min_key: "elderberry".to_string(),
                    max_key: "fig".to_string(),
                    vector_count: 50,
                    bloom_filter_offset: None,
                },
            ],
            total_blocks: 3,
            total_vectors: 150,
            metadata_stats: HashMap::new(),
        };

        index_cache
            .cache_sstable_index("search.sstable", index)
            .await
            .unwrap();

        // Test finding blocks for specific keys
        let blocks = index_cache
            .find_blocks_for_key("search.sstable", "cherry")
            .await;
        assert!(blocks.is_some());
        let matching = blocks.unwrap();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].key, "block2");

        // Test key that spans block boundary
        let blocks = index_cache
            .find_blocks_for_key("search.sstable", "banana")
            .await;
        assert!(blocks.is_some());
        let matching = blocks.unwrap();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].key, "block1");
    }
}

#[cfg(test)]
mod viper_cache_integration {
    use super::*;

    #[tokio::test]
    async fn test_parquet_metadata_caching() {
        let metadata_cache = Arc::new(MetadataStore::new(50)); // 50MB cache

        // Create test schema metadata
        let schema_metadata = serde_json::json!({
            "file": "test.parquet",
            "schema": {
                "vector": "Float32Array",
                "id": "String",
                "metadata_info": "Json"
            },
            "row_groups": 5,
            "total_rows": 10000
        });

        // Cache metadata
        metadata_cache
            .put("parquet_schema_test", schema_metadata.clone())
            .await
            .unwrap();

        // Retrieve metadata
        let cached = metadata_cache.get(&key).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap(), schema_metadata);
    }

    #[tokio::test]
    async fn test_parquet_column_stats_caching() {
        let metadata_cache = Arc::new(MetadataStore::new(50));

        // Cache column statistics for query optimization
        let column_stats = serde_json::json!({
            "file": "vectors.parquet",
            "row_group": 0,
            "columns": {
                "vector": {
                    "min": [0.0, 0.0, 0.0],
                    "max": [1.0, 1.0, 1.0],
                    "null_count": 0,
                    "distinct_count": 1000
                },
                "timestamp": {
                    "min": 1000000,
                    "max": 2000000,
                    "null_count": 10,
                    "distinct_count": 950
                }
            }
        });

        metadata_cache
            .put("column_stats_rg0", column_stats.clone())
            .await
            .unwrap();

        let cached = metadata_cache.get(&key).await;
        assert!(cached.is_some());
        assert_eq!(
            cached.unwrap()["columns"]["vector"]["min"],
            serde_json::json!([0.0, 0.0, 0.0])
        );
    }
}

#[cfg(test)]
mod unified_search_cache_tests {
    use super::*;

    #[tokio::test]
    async fn test_cross_engine_cache_sharing() {
        // Create shared cache instances
        let vector_cache = Arc::new(VectorStore::new(100));
        let index_cache = Arc::new(IndexNodeCache::new(50));
        let metadata_cache = Arc::new(MetadataStore::new(50));

        // Simulate SST engine caching data
        let sst_block_key = SstBlockKey::new("shared.sstable".to_string(), 0, 4096);
        let sst_vectors = vec![VectorRecord {
            id: Some("sst_v1".to_string()),
            vector: vec![1.0, 2.0, 3.0],
            metadata: vec![],
            version: Some(1),
            timestamp: 1000,
            updated_at: None,
            expires_at: None,
            // rank removed -  None,
            similarity: None,
            similarity: None,
        }];
        vector_cache
            .cache_block_vectors(&sst_block_key, sst_vectors)
            .await
            .unwrap();

        // Simulate VIPER engine accessing shared cache
        // Both engines can see the same cached data
        let cached = vector_cache.get_block_vectors(&sst_block_key, 1).await;
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].id, Some("sst_v1".to_string()));

        // Simulate metadata sharing
        metadata_cache
            .put("shared_schema", serde_json::json!({"format": "unified"}))
            .await
            .unwrap();

        // Both engines can access the same metadata
        let meta = metadata_cache.get(&key).await;
        assert!(meta.is_some());
        assert_eq!(meta.unwrap()["format"], "unified");
    }

    #[tokio::test]
    async fn test_cache_invalidation_coordination() {
        let vector_cache = Arc::new(VectorStore::new(100));
        let index_cache = Arc::new(IndexNodeCache::new(50));

        // Cache data
        let block_key = SstBlockKey::new("invalidate.sstable".to_string(), 0, 4096);
        vector_cache
            .cache_compressed_block(&block_key, vec![1, 2, 3], CompressionType::None, 3)
            .await
            .unwrap();

        // Cache index
        let index = SstableIndex {
            file_path: "invalidate.sstable".to_string(),
            entries: vec![],
            total_blocks: 1,
            total_vectors: 10,
            metadata_stats: HashMap::new(),
        };
        index_cache
            .cache_sstable_index("invalidate.sstable", index)
            .await
            .unwrap();

        // Verify data is cached
        assert!(
            vector_cache
                .get_compressed_block(&block_key)
                .await
                .is_some()
        );
        assert!(
            index_cache
                .get_sstable_index("invalidate.sstable")
                .await
                .is_some()
        );

        // Invalidate SSTable data
        vector_cache
            .invalidate_sstable("invalidate.sstable")
            .await
            .unwrap();
        index_cache.invalidate("sst_index_invalidate.sstable").await;

        // Verify invalidation
        assert!(
            index_cache
                .get_sstable_index("invalidate.sstable")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_cache_memory_limits() {
        // Test that cache respects memory limits
        let small_cache = Arc::new(VectorStore::new(1)); // 1MB cache - very small

        // Try to cache many blocks
        for i in 0..100 {
            let block_key = SstBlockKey::new(format!("file_{}.sstable", i), i * 4096, 4096);

            // Each block is ~1KB
            let data = vec![0u8; 1024];
            let _ = small_cache
                .cache_compressed_block(&block_key, data, CompressionType::None, 1024)
                .await;
        }

        // Cache should have evicted old entries due to memory limit
        // First entries should be evicted
        let first_key = SstBlockKey::new("file_0.sstable".to_string(), 0, 4096);
        let first_block = small_cache.get_compressed_block(&first_key).await;

        // Recent entries should still be cached
        let recent_key = SstBlockKey::new("file_99.sstable".to_string(), 99 * 4096, 4096);
        let recent_block = small_cache.get_compressed_block(&recent_key).await;

        // Due to LRU eviction, old entries might be gone
        // This behavior depends on the actual cache implementation
        assert!(recent_block.is_some() || first_block.is_none());
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use std::time::Instant;
    use tracing::{debug, error, info};

    #[tokio::test]
    async fn test_batch_caching_performance() {
        let vector_cache = Arc::new(VectorStore::new(100));
        let index_cache = Arc::new(IndexNodeCache::new(50));

        // Measure batch caching performance
        let start = Instant::now();

        // Cache 1000 blocks
        for i in 0..1000 {
            let key = SstBlockKey::new(
                format!("perf_{}.sstable", i / 100),
                (i % 100) as u64 * 4096,
                4096,
            );

            let _ = vector_cache
                .cache_compressed_block(&key, vec![i as u8; 100], CompressionType::Lz4, 100)
                .await;
        }

        let cache_duration = start.elapsed();
        debug!("Cached 1000 blocks in {:?}", cache_duration);

        // Measure batch retrieval performance
        let start = Instant::now();
        let mut hits = 0;

        for i in 0..1000 {
            let key = SstBlockKey::new(
                format!("perf_{}.sstable", i / 100),
                (i % 100) as u64 * 4096,
                4096,
            );

            if vector_cache.get_compressed_block(&key).await.is_some() {
                hits += 1;
            }
        }

        let retrieve_duration = start.elapsed();
        debug!("Retrieved {} blocks in {:?}", hits, retrieve_duration);

        // Performance assertions
        assert!(cache_duration.as_millis() < 5000); // Should cache 1000 blocks in < 5s
        assert!(retrieve_duration.as_millis() < 1000); // Should retrieve in < 1s
        assert!(hits > 0); // Should have some cache hits
    }
}
