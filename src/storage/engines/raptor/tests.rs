#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::storage::traits::UnifiedStorageEngine;
    use crate::core::VectorRecord;
    use anyhow::Result;
    use std::collections::HashMap;

    async fn create_test_engine() -> Result<RaptorEngine> {
        let config = RaptorConfig {
            rowgroup_size: 100,
            compression: config::CompressionCodec::Snappy,
            enable_statistics: true,
            enable_bloom_filters: false, // Simplified for tests
            bloom_fpp: 0.01,
            enable_hnsw: false, // Simplified for tests
            enable_simd: false, // Simplified for tests
            cache_size_mb: 10,
            enable_prefetching: false,
            enable_range_reads: false,
            compaction_threshold_files: 5,
        };
        
        let engine = RaptorEngine::new(
            "test_collection".to_string(),
            "/tmp/raptor_test".to_string(),
            config,
        ).await?;
        
        Ok(engine)
    }

    #[tokio::test]
    async fn test_engine_basic_info() -> Result<()> {
        let engine = create_test_engine().await?;
        
        assert_eq!(engine.engine_name(), "RAPTOR");
        assert_eq!(engine.engine_version(), "1.0.0");
        assert_eq!(engine.strategy(), crate::storage::traits::StorageEngineStrategy::Hybrid);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_insert_and_retrieve() -> Result<()> {
        let engine = create_test_engine().await?;
        
        // Create test vector
        let vector = VectorRecord {
            id: Some("test_vec_1".to_string()),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata: HashMap::from([
                ("category".to_string(), "test".to_string()),
                ("version".to_string(), "1".to_string()),
            ]),
            version: Some(1),
            timestamp: Some(1234567890),
            ..Default::default()
        };
        
        // Insert vector (using internal method)
        engine.insert_batch_internal(vec![vector.clone()]).await?;
        
        // Retrieve vector
        let retrieved = engine.get_vector_by_id("test_collection", "test_vec_1").await?;
        
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, vector.id);
        assert_eq!(retrieved.vector.len(), 4);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_search_vectors() -> Result<()> {
        let engine = create_test_engine().await?;
        
        // Insert test vectors
        let vectors = vec![
            VectorRecord {
                id: Some("vec1".to_string()),
                vector: vec![1.0, 0.0, 0.0, 0.0],
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: Some(1234567890),
                ..Default::default()
            },
            VectorRecord {
                id: Some("vec2".to_string()),
                vector: vec![0.0, 1.0, 0.0, 0.0],
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: Some(1234567891),
                ..Default::default()
            },
            VectorRecord {
                id: Some("vec3".to_string()),
                vector: vec![0.0, 0.0, 1.0, 0.0],
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: Some(1234567892),
                ..Default::default()
            },
        ];
        
        engine.insert_batch_internal(vectors).await?;
        
        // Search for similar vectors
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = engine.search_vectors_unified(
            "test_collection",
            "/tmp/raptor_test",
            &query,
            2,
            &crate::compute::distance_computation::DistanceMetric::Cosine,
            None,
            false,
            false,
        ).await?;
        
        assert!(!results.is_empty());
        assert!(results.len() <= 2);
        
        // First result should be the exact match
        assert_eq!(results[0].id, "vec1");
        assert!(results[0].score < 0.01); // Very close to 0 distance
        
        Ok(())
    }

    #[tokio::test]
    async fn test_flush_operation() -> Result<()> {
        let engine = create_test_engine().await?;
        
        // Insert some vectors
        let vectors = (0..10).map(|i| {
            VectorRecord {
                id: Some(format!("flush_vec_{}", i)),
                vector: vec![i as f32 * 0.1; 4],
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: Some(1234567890 + i),
                ..Default::default()
            }
        }).collect();
        
        engine.insert_batch_internal(vectors).await?;
        
        // Perform flush
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some("test_collection".to_string()),
            force: false,
            synchronous: true,
            ..Default::default()
        };
        
        let result = engine.do_flush(&flush_params).await?;
        
        assert!(result.success);
        assert_eq!(result.collections_affected, vec!["test_collection"]);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_compaction_operation() -> Result<()> {
        let engine = create_test_engine().await?;
        
        // Perform compaction
        let compact_params = crate::storage::traits::CompactionParameters {
            collection_id: Some("test_collection".to_string()),
            force: false,
            synchronous: true,
            ..Default::default()
        };
        
        let result = engine.do_compact(&compact_params).await?;
        
        assert!(result.success);
        assert_eq!(result.collections_affected, vec!["test_collection"]);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_storage_tier_detection() {
        // Test S3 detection
        assert_eq!(
            RaptorEngine::determine_storage_tier("s3://bucket/path"),
            crate::storage::persistence::filesystem::StorageTier::S3Standard
        );
        
        // Test S3 Express detection
        assert_eq!(
            RaptorEngine::determine_storage_tier("s3://express-bucket/path"),
            crate::storage::persistence::filesystem::StorageTier::S3Express
        );
        
        // Test GCS detection
        assert_eq!(
            RaptorEngine::determine_storage_tier("gs://bucket/path"),
            crate::storage::persistence::filesystem::StorageTier::GcsSSD
        );
        
        // Test Azure detection
        assert_eq!(
            RaptorEngine::determine_storage_tier("azure://container/path"),
            crate::storage::persistence::filesystem::StorageTier::AzurePremium
        );
        
        // Test NVMe detection
        assert_eq!(
            RaptorEngine::determine_storage_tier("/mnt/nvme/data"),
            crate::storage::persistence::filesystem::StorageTier::NVMe
        );
        
        // Test SSD detection
        assert_eq!(
            RaptorEngine::determine_storage_tier("/mnt/ssd/data"),
            crate::storage::persistence::filesystem::StorageTier::SSD
        );
        
        // Default to HDD
        assert_eq!(
            RaptorEngine::determine_storage_tier("/var/data"),
            crate::storage::persistence::filesystem::StorageTier::HDD
        );
    }

    #[tokio::test]
    async fn test_rowgroup_management() -> Result<()> {
        let schema = RaptorEngine::create_default_schema();
        let mut manager = RowGroupManager::new(schema);
        
        // Create a test batch
        use arrow_array::{Float32Array, StringArray, RecordBatch};
        use std::sync::Arc;
        
        let id_array = Arc::new(StringArray::from(vec!["id1", "id2", "id3"]));
        let vector_array = Arc::new(Float32Array::from(vec![
            0.1, 0.2, 0.3, 0.4,  // First vector
            0.5, 0.6, 0.7, 0.8,  // Second vector
            0.9, 1.0, 1.1, 1.2,  // Third vector
        ]));
        
        let batch = RecordBatch::try_from_iter(vec![
            ("id", id_array as arrow_array::ArrayRef),
            ("vector", vector_array as arrow_array::ArrayRef),
        ])?;
        
        let config = RaptorConfig::default();
        let rowgroup = manager.add_rowgroup(&batch, &config)?;
        
        assert_eq!(rowgroup.row_count, 3);
        assert_eq!(rowgroup.vector_stats.dimension, 4);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_clustering_integration() -> Result<()> {
        use crate::index::axis::clustering::{ClusteringConfig, ClusteringAlgorithm, KMeansConfig};
        use crate::index::axis::cluster_manager::ClusterManager;
        
        let clustering_config = ClusteringConfig {
            algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
                k: 2,
                max_iterations: 10,
                ..Default::default()
            }),
            min_vectors_for_clustering: 3,
            max_clusters: 10,
            distance_metric: crate::compute::distance_computation::DistanceMetric::Cosine,
            adaptive_cluster_count: false,
            recompute_threshold: 100,
            enable_incremental: false,
        };
        
        let mut cluster_manager = ClusterManager::new(clustering_config).await?;
        
        // Test vectors
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.9, 0.1, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.9, 0.1],
        ];
        
        let assignments = cluster_manager.cluster_vectors(&vectors).await?;
        
        assert_eq!(assignments.len(), 4);
        
        // Vectors 0 and 1 should be in the same cluster
        assert_eq!(assignments[0].cluster_id, assignments[1].cluster_id);
        
        // Vectors 2 and 3 should be in the same cluster
        assert_eq!(assignments[2].cluster_id, assignments[3].cluster_id);
        
        // Different clusters for different groups
        assert_ne!(assignments[0].cluster_id, assignments[2].cluster_id);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_cloud_io_optimization() -> Result<()> {
        let engine = create_test_engine().await?;
        
        // Test that cloud storage detection works
        assert!(engine.is_cloud_storage() == false); // Local path
        
        // Test with cloud path
        let cloud_config = RaptorConfig::default();
        let cloud_engine = RaptorEngine::new(
            "cloud_test".to_string(),
            "s3://test-bucket/raptor".to_string(),
            cloud_config,
        ).await?;
        
        assert!(cloud_engine.is_cloud_storage());
        
        Ok(())
    }
}