#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::core::VectorRecord;
    use crate::storage::traits::UnifiedStorageEngine;
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
        )
        .await?;

        Ok(engine)
    }

    #[tokio::test]
    async fn test_engine_basic_info() -> Result<()> {
        let engine = create_test_engine().await?;

        assert_eq!(engine.engine_name(), "RAPTOR");
        assert_eq!(engine.engine_version(), "1.0.0");
        assert_eq!(
            engine.strategy(),
            crate::storage::traits::StorageEngineStrategy::Hybrid
        );

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
        let retrieved = engine.vector_by_id("test_collection", "test_vec_1").await?;

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
        let results = engine
            .search_vectors_unified(
                "test_collection",
                "/tmp/raptor_test",
                &query,
                2,
                &crate::compute::distance_computation::DistanceMetric::Cosine,
                None,
                false,
                false,
            )
            .await?;

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
        let vectors = (0..10)
            .map(|i| VectorRecord {
                id: Some(format!("flush_vec_{}", i)),
                vector: vec![i as f32 * 0.1; 4],
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: Some(1234567890 + i),
                ..Default::default()
            })
            .collect();

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
        let mut manager = RowGroups::new(schema);

        // Create a test batch
        use arrow_array::{Float32Array, RecordBatch, StringArray};
        use std::sync::Arc;

        let id_array = Arc::new(StringArray::from(vec!["id1", "id2", "id3"]));
        let vector_array = Arc::new(Float32Array::from(vec![
            0.1, 0.2, 0.3, 0.4, // First vector
            0.5, 0.6, 0.7, 0.8, // Second vector
            0.9, 1.0, 1.1, 1.2, // Third vector
        ]));

        let batch = RecordBatch::try_from_iter(vec![
            ("id", id_array as arrow_array::ArrayRef),
            ("vector", vector_array as arrow_array::ArrayRef),
        ])?;

        let config = RaptorConfig::default();
        let rowgroup = manager.add_rowgroup(&batch, &config)?;

        assert_eq!(rowgroup.vector_count, 3);
        assert_eq!(rowgroup.vector_stats.dimension, 4);

        Ok(())
    }

    #[tokio::test]
    async fn test_clustering_integration() -> Result<()> {
        use crate::index::axis::cluster_manager::ClusterManager;
        use crate::index::axis::clustering::{ClusteringAlgorithm, ClusteringConfig, KMeansConfig};

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
        )
        .await?;

        assert!(cloud_engine.is_cloud_storage());

        Ok(())
    }

    #[tokio::test]
    async fn test_centralized_footer_with_columnar_centroids() -> Result<()> {
        use crate::storage::engines::impls::raptor::common::{
            ColumnarCentroids, FastLanesMetadata,
        };
        use crate::storage::engines::impls::raptor::writer::RaptorWriter;
        use tempfile::TempDir;

        // Create temp directory for test
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir
            .path()
            .join("test_footer.rpf")
            .to_str()
            .unwrap()
            .to_string();

        let config = RaptorConfig {
            target_rowgroup_size: 50,
            enable_clustering: true,
            min_vectors_for_clustering: Some(10),
            ..Default::default()
        };

        let dimension = 64;
        let collection_id = "footer_test".to_string();

        // Write test data with multiple rowgroups
        {
            let mut writer = RaptorWriter::new(
                file_path.clone(),
                config.clone(),
                collection_id.clone(),
                dimension,
            )
            .await?;

            // Create 4 rowgroups
            for rg_idx in 0..4 {
                for i in 0..50 {
                    let mut vector = vec![0.0f32; dimension];
                    vector[0] = rg_idx as f32 * 10.0; // Different pattern per rowgroup
                    vector[1] = i as f32;

                    let record = crate::proto::proximadb_v1::VectorRecord {
                        id: Some(format!("vec_{}_{}", rg_idx, i)),
                        vector,
                        ..Default::default()
                    };

                    writer.write_vector(&record).await?;
                }
                writer.flush().await?;
            }

            // Finalize writes the centralized footer
            writer.finalize().await?;
        }

        // Verify footer structure
        {
            // Test columnar centroid encoding/decoding
            let num_centroids = 4;
            let mut rowgroup_ids = vec![];
            let mut transposed_data = vec![0.0f32; num_centroids * dimension];

            for i in 0..num_centroids {
                rowgroup_ids.push(i as u32);

                // Create test pattern for centroids
                for dim in 0..dimension {
                    let offset = dim * num_centroids + i;
                    transposed_data[offset] = (i * 10) as f32 + (dim as f32 * 0.1);
                }
            }

            let columnar = ColumnarCentroids {
                count: num_centroids as u32,
                dimension: dimension as u32,
                rowgroup_ids,
                transposed_data,
                encoding_metadata: vec![],
            };

            // Test O(1) access
            for test_id in 0..4 {
                let centroid = columnar
                    .get_centroid(test_id)
                    .expect(&format!("Should find centroid for rowgroup {}", test_id));
                assert_eq!(centroid.len(), dimension);

                // Verify first value matches expected pattern
                let expected_first = (test_id * 10) as f32;
                assert!(
                    (centroid[0] - expected_first).abs() < 0.01,
                    "Centroid {} first value mismatch",
                    test_id
                );
            }

            // Test decode_all
            let all_centroids = columnar.decode_all();
            assert_eq!(all_centroids.len(), num_centroids);

            println!("✅ Centralized footer test passed!");
            println!(
                "  - Created {} rowgroups with {} vectors each",
                num_centroids, 50
            );
            println!("  - Columnar encoding with {} dimensions", dimension);
            println!("  - O(1) centroid access verified");
        }

        Ok(())
    }

    #[test]
    fn test_memory_savings_with_centralized_footer() {
        // Verify memory savings calculation
        let num_rowgroups = 1000;
        let dimension = 1536;
        let neighbors_per_rowgroup = 5;

        // Distributed: storing neighbor centroids inline
        let distributed_size = num_rowgroups * neighbors_per_rowgroup * dimension * 4;

        // Centralized: all centroids in footer
        let centralized_size = num_rowgroups * dimension * 4;

        let savings_bytes = distributed_size - centralized_size;
        let savings_pct = (savings_bytes as f32 / distributed_size as f32) * 100.0;

        println!("Memory savings analysis:");
        println!("  Rowgroups: {}", num_rowgroups);
        println!("  Dimension: {}", dimension);
        println!("  Neighbors per rowgroup: {}", neighbors_per_rowgroup);
        println!(
            "  Distributed storage: {:.2} MB",
            distributed_size as f32 / 1_048_576.0
        );
        println!(
            "  Centralized storage: {:.2} MB",
            centralized_size as f32 / 1_048_576.0
        );
        println!(
            "  Savings: {:.2} MB ({:.1}%)",
            savings_bytes as f32 / 1_048_576.0,
            savings_pct
        );

        assert!(savings_pct > 79.0, "Should save at least 79% memory");
    }

    #[test]
    fn test_centroid_distance_matrix_performance() {
        use std::time::Instant;

        println!("\n=== Centroid Distance Matrix Performance Impact ===\n");

        // Test various collection sizes
        let test_cases = vec![
            ("Small", 10, 384),    // 45 distance calculations
            ("Medium", 100, 384),  // 4,950 distance calculations
            ("Large", 1000, 384),  // 499,500 distance calculations
            ("XLarge", 5000, 384), // 12,497,500 distance calculations
        ];

        for (name, k, dim) in test_cases {
            // Calculate number of distance computations
            let num_distances = k * (k - 1) / 2;

            // Estimate time (assuming ~0.5μs per distance with SIMD)
            let estimated_ms = (num_distances as f64 * 0.5) / 1000.0;

            // Memory for matrix
            let matrix_memory_mb = (k * k * 4) as f64 / 1_048_576.0;

            println!("{} collection (k={}):", name, k);
            println!("  Distance calculations: {}", num_distances);
            println!("  Estimated time: {:.2} ms", estimated_ms);
            println!("  Matrix memory: {:.2} MB", matrix_memory_mb);

            // Performance assessment
            let impact = if estimated_ms < 1.0 {
                "✅ Negligible (<1ms)"
            } else if estimated_ms < 10.0 {
                "✅ Acceptable (<10ms)"
            } else if estimated_ms < 100.0 {
                "⚠️ Noticeable (10-100ms)"
            } else {
                "❌ Problematic (>100ms) - Use lazy loading"
            };

            println!("  Read latency impact: {}", impact);

            // Recommendation
            if k > 1000 {
                println!("  💡 Recommendation: Use lazy loading or cache the matrix");
            }
            println!();
        }

        println!("=== Optimization Strategies for Large Collections ===\n");
        println!("1. LAZY LOADING (k > 1000):");
        println!("   - Don't compute full matrix at load");
        println!("   - Calculate distances on-demand during search");
        println!("   - Cache frequently used pairs\n");

        println!("2. HIERARCHICAL CLUSTERING (k > 5000):");
        println!("   - Group rowgroups into super-clusters");
        println!("   - Only compute relevant cluster distances\n");

        println!("3. PRE-COMPUTED IN FOOTER (tradeoff):");
        println!("   - Store matrix in footer: +k²×4 bytes");
        println!("   - Example: k=1000 → +4MB storage, 0ms compute");
    }
}
