//! Unit tests for WAL with unified distance system

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
    use crate::core::{String, VectorId, VectorRecord};
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::persistence::wal::{WalConfig, WalBatchFactory, WalManager, WalStrategyType};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio;

    /// Create a test WAL manager with unified distance support
    async fn create_test_wal_manager() -> (Arc<WalManager>, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let mut wal_config = WalConfig::default();
        wal_config.multi_disk.data_directories =
            vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config = FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem"),
        );

        let wal_strategy = WalBatchFactory::create_strategy(WalStrategyType::AvroBatch, &wal_config, filesystem)
            .await
            .expect("Failed to create WAL strategy");

        let wal_manager = Arc::new(
            WalManager::new(wal_strategy, wal_config)
                .await
                .expect("Failed to create WAL manager"),
        );

        (wal_manager, temp_dir)
    }

    /// Create test vector records with known geometric relationships
    fn create_test_vector_records() -> Vec<VectorRecord> {
        let now = chrono::Utc::now().timestamp_millis();

        vec![
            VectorRecord {
                id: "unit_x".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![1.0, 0.0, 0.0], // Unit vector along X-axis
                metadata: HashMap::new(),
                timestamp: now,
                created_at: now,
                updated_at: now,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: "unit_y".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.0, 1.0, 0.0], // Unit vector along Y-axis
                metadata: HashMap::new(),
                timestamp: now,
                created_at: now,
                updated_at: now,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: "diagonal".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.707, 0.707, 0.0], // 45-degree vector
                metadata: HashMap::new(),
                timestamp: now,
                created_at: now,
                updated_at: now,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: "scaled_x".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![2.0, 0.0, 0.0], // Scaled version of unit_x
                metadata: HashMap::new(),
                timestamp: now,
                created_at: now,
                updated_at: now,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ]
    }

    #[tokio::test]
    async fn test_wal_unified_distance_system_integration() {
        let (manager, _temp_dir) = create_test_wal_manager().await;

        // Verify WAL manager has unified distance support
        let distance_compute = manager.distance_compute();
        assert_eq!(distance_compute.system_default(), &DistanceMetric::Cosine);

        println!("✅ WAL manager created with unified distance support");
    }

    #[tokio::test]
    async fn test_unified_distance_calculations() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let distance_compute = manager.distance_compute();

        // Test vectors
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0];
        let vec_c = vec![1.0, 0.0, 0.0]; // Identical to vec_a

        // Test cosine distance
        let cosine_ab =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        assert!(
            (cosine_ab - 1.0).abs() < 1e-6,
            "Orthogonal vectors should have cosine distance ≈ 1.0"
        );

        let cosine_ac =
            distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Cosine);
        assert!(
            (cosine_ac - 0.0).abs() < 1e-6,
            "Identical vectors should have cosine distance ≈ 0"
        );

        // Test Euclidean distance
        let euclidean_ab =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        let expected_euclidean = (2.0_f32).sqrt(); // sqrt((1-0)^2 + (0-1)^2)
        assert!(
            (euclidean_ab - expected_euclidean).abs() < 1e-6,
            "Euclidean distance should be sqrt(2)"
        );

        let euclidean_ac =
            distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Euclidean);
        assert!(
            (euclidean_ac - 0.0).abs() < 1e-6,
            "Identical vectors should have Euclidean distance = 0"
        );

        // Test Manhattan distance
        let manhattan_ab =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Manhattan);
        assert!(
            (manhattan_ab - 2.0).abs() < 1e-6,
            "Manhattan distance should be |1-0| + |0-1| = 2"
        );

        // Test dot product
        let dot_ab =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
        assert!(
            (dot_ab - 1.0).abs() < 1e-6,
            "Dot product distance of orthogonal vectors should be 1.0 (inverted from 0.0)"
        );

        let dot_ac =
            distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::DotProduct);
        assert!(
            (dot_ac - 0.0).abs() < 1e-6,
            "Dot product distance of identical unit vectors should be 0 (inverted from 1.0)"
        );

        println!("✅ All unified distance calculations verified");
    }

    #[tokio::test]
    async fn test_wal_search_with_unified_distances() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let collection_id = String::from("test_collection".to_string());
        let records = create_test_vector_records();

        // Insert test vectors
        for (i, record) in records.iter().enumerate() {
            let vector_id = VectorId::from(record.id.clone());
            let result = manager
                .insert(collection_id.clone(), vector_id, record.clone())
                .await;
            assert!(
                result.is_ok(),
                "Failed to insert vector {}: {:?}",
                i + 1,
                result
            );
        }

        let query_vector = vec![1.0, 0.0, 0.0]; // Same as unit_x
        let k = 3;

        // Test search with different distance metrics
        let metrics_to_test = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::Manhattan,
            DistanceMetric::DotProduct,
        ];

        for metric in metrics_to_test {
            let search_results = manager
                .search_vectors_similarity(&collection_id, &query_vector, k, Some(metric.clone()))
                .await;

            assert!(
                search_results.is_ok(),
                "Search with {:?} should succeed: {:?}",
                metric,
                search_results
            );
            let results = search_results.unwrap();
            assert!(
                !results.is_empty(),
                "Search with {:?} should return results",
                metric
            );

            // Verify that the best match is the expected vector
            let best_result = &results[0];
            
            // For Cosine distance, both unit_x and scaled_x have the same similarity
            // (they're in the same direction), so either is a valid best match
            match metric {
                DistanceMetric::Cosine => {
                    assert!(
                        best_result.0 == "unit_x" || best_result.0 == "scaled_x",
                        "Best match for Cosine should be unit_x or scaled_x (same direction), got: {}",
                        best_result.0
                    );
                }
                _ => {
                    assert_eq!(
                        best_result.0, "unit_x",
                        "Best match for {:?} should be unit_x (identical to query), got: {}",
                        metric,
                        best_result.0
                    );
                }
            }

            println!("✅ Search with {:?} metric successful", metric);
        }
    }

    #[tokio::test]
    async fn test_distance_metric_consistency_across_components() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let distance_compute = manager.distance_compute();

        // Create a simple unified distance compute for comparison
        let standalone_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

        let vec_a = vec![1.0, 2.0, 3.0];
        let vec_b = vec![4.0, 5.0, 6.0];

        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::Manhattan,
            DistanceMetric::DotProduct,
        ];

        for metric in metrics {
            let wal_result = distance_compute.calculate_distance(&vec_a, &vec_b, &metric);
            let standalone_result = standalone_compute.calculate_distance(&vec_a, &vec_b, &metric);

            assert!(
                (wal_result - standalone_result).abs() < 1e-10,
                "Distance calculations should be consistent for {:?}: WAL={}, Standalone={}",
                metric,
                wal_result,
                standalone_result
            );

            println!(
                "✅ Distance metric {:?} consistent across components",
                metric
            );
        }
    }

    #[test]
    fn test_dimension_mismatch_handling() {
        let distance_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

        let vec_3d = vec![1.0, 2.0, 3.0];
        let vec_2d = vec![4.0, 5.0];

        // Test that dimension mismatches are handled gracefully
        let cosine_result =
            distance_compute.calculate_distance(&vec_3d, &vec_2d, &DistanceMetric::Cosine);
        assert_eq!(
            cosine_result, 2.0,
            "Cosine with dimension mismatch should return 2.0"
        );

        let euclidean_result =
            distance_compute.calculate_distance(&vec_3d, &vec_2d, &DistanceMetric::Euclidean);
        assert!(
            euclidean_result.is_infinite(),
            "Euclidean with dimension mismatch should return infinity"
        );

        let manhattan_result =
            distance_compute.calculate_distance(&vec_3d, &vec_2d, &DistanceMetric::Manhattan);
        assert!(
            manhattan_result.is_infinite(),
            "Manhattan with dimension mismatch should return infinity"
        );

        println!("✅ Dimension mismatch handling verified");
    }

    #[test]
    fn test_edge_cases() {
        let distance_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

        // Test with zero vectors
        let zero_vec = vec![0.0, 0.0, 0.0];
        let unit_vec = vec![1.0, 0.0, 0.0];

        let cosine_zero =
            distance_compute.calculate_distance(&zero_vec, &unit_vec, &DistanceMetric::Cosine);
        // Cosine distance with zero vector returns NaN due to division by zero
        assert!(
            cosine_zero.is_nan(),
            "Cosine with zero vector should return NaN"
        );

        // Test with very small vectors (near numerical precision limits)
        let tiny_vec_a = vec![1e-10, 1e-10, 1e-10];
        let tiny_vec_b = vec![2e-10, 2e-10, 2e-10];

        let cosine_tiny =
            distance_compute.calculate_distance(&tiny_vec_a, &tiny_vec_b, &DistanceMetric::Cosine);
        assert!(
            (cosine_tiny - 0.0).abs() < 1e-6,
            "Cosine distance of parallel tiny vectors should be ≈ 0.0"
        );

        // Test custom metric fallback
        let custom_result = distance_compute.calculate_distance(
            &unit_vec,
            &unit_vec,
            &DistanceMetric::Custom,
        );
        let cosine_result =
            distance_compute.calculate_distance(&unit_vec, &unit_vec, &DistanceMetric::Cosine);
        assert_eq!(
            custom_result, cosine_result,
            "Custom metric should fallback to cosine"
        );

        println!("✅ Edge cases handled correctly");
    }
}
