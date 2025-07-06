//! Unit tests for WAL unified distance algorithm support
//!
//! This module tests the integration of the unified distance system with WAL operations,
//! ensuring consistent and hardware-accelerated distance calculations across all WAL strategies.

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
    use crate::core::VectorRecord;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::persistence::wal::{
        WalConfig, WalEntry, WalFactory, WalManager, WalOperation,
    };
    use anyhow::Result;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test WAL manager with unified distance support
    async fn create_test_wal_with_unified_distance() -> Result<(WalManager, TempDir)> {
        let temp_dir = TempDir::new()?;

        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        let strategy = WalFactory::create_from_config(&config, filesystem).await?;

        let manager = WalManager::new(strategy, config).await?;

        Ok((manager, temp_dir))
    }

    /// Create test vector records for distance algorithm testing
    fn create_distance_test_vectors() -> Vec<VectorRecord> {
        let now = Utc::now().timestamp_millis();
        vec![
            // Unit vectors for clear distance relationships
            VectorRecord {
                id: "unit_x".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![1.0, 0.0, 0.0], // Unit vector along X
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
                vector: vec![0.0, 1.0, 0.0], // Unit vector along Y
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
                id: "unit_z".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.0, 0.0, 1.0], // Unit vector along Z
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
                vector: vec![0.577, 0.577, 0.577], // Normalized diagonal vector
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
                id: "opposite_x".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![-1.0, 0.0, 0.0], // Opposite to unit_x
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
    async fn test_wal_unified_distance_compute_access() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        // Verify WAL manager implements DistanceComputeProvider
        let distance_compute = manager.distance_compute();

        // Test that we can use the unified distance compute
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0];

        let cosine_distance =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        assert!(
            (cosine_distance - 1.0).abs() < 1e-6,
            "Orthogonal vectors should have cosine distance ≈ 1.0"
        );
    }

    #[tokio::test]
    async fn test_wal_search_with_unified_cosine() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());
        let vectors = create_distance_test_vectors();

        // Insert test vectors
        for vector in &vectors {
            let result = manager
                .insert(
                    collection_id.clone(),
                    crate::core::VectorId::from(vector.id.clone()),
                    vector.clone(),
                )
                .await;
            assert!(result.is_ok(), "Failed to insert vector {}", vector.id);
        }

        // Search with cosine distance
        let query_vector = vec![1.0, 0.0, 0.0]; // Same as unit_x
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                3,
                Some(DistanceMetric::Cosine),
            )
            .await;

        assert!(results.is_ok(), "Search should succeed");
        let results = results.unwrap();
        assert_eq!(results.len(), 3, "Should return top 3 results");

        // Verify results are ordered by unified distance semantics (lower = more similar)
        // Best match should be unit_x (identical vector)
        assert_eq!(results[0].0, "unit_x", "Best match should be unit_x");
        assert!(
            (results[0].1 - 0.0).abs() < 1e-6,
            "Identical vectors should have distance ≈ 0.0"
        );

        // Worst match in top-3 should be orthogonal vectors
        let orthogonal_ids = ["unit_y", "unit_z"];
        assert!(
            orthogonal_ids.contains(&results[2].0.as_str()),
            "Worst match in top-3 should be orthogonal vector"
        );
        assert!(
            (results[2].1 - 1.0).abs() < 1e-6,
            "Orthogonal vectors should have distance ≈ 1.0"
        );
    }

    #[tokio::test]
    async fn test_wal_search_with_unified_euclidean() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());
        let vectors = create_distance_test_vectors();

        // Insert test vectors
        for vector in &vectors {
            let result = manager
                .insert(
                    collection_id.clone(),
                    crate::core::VectorId::from(vector.id.clone()),
                    vector.clone(),
                )
                .await;
            assert!(result.is_ok(), "Failed to insert vector {}", vector.id);
        }

        // Search with Euclidean distance
        let query_vector = vec![1.0, 0.0, 0.0]; // Same as unit_x
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                3,
                Some(DistanceMetric::Euclidean),
            )
            .await;

        assert!(results.is_ok(), "Search should succeed");
        let results = results.unwrap();
        assert_eq!(results.len(), 3, "Should return top 3 results");

        // Verify results are ordered by Euclidean distance (lower is better)
        // Best match should be unit_x (identical vector)
        assert_eq!(results[0].0, "unit_x", "Best match should be unit_x");
        assert!(
            (results[0].1 - 0.0).abs() < 1e-6,
            "Identical vectors should have distance ≈ 0.0"
        );

        // Check that orthogonal vectors have distance √2 ≈ 1.414
        let orthogonal_results: Vec<_> = results
            .iter()
            .filter(|(id, _, _)| id == "unit_y" || id == "unit_z")
            .collect();

        for (id, distance, _) in orthogonal_results {
            assert!(
                (distance - 1.414214).abs() < 1e-5,
                "Orthogonal unit vector {} should have Euclidean distance ≈ √2",
                id
            );
        }
    }

    #[tokio::test]
    async fn test_wal_search_with_unified_manhattan() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());

        // Create vectors with clear Manhattan distance relationships
        let now = Utc::now().timestamp_millis();
        let manhattan_vectors = vec![
            VectorRecord {
                id: "origin".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.0, 0.0, 0.0],
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
                id: "point_1".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![1.0, 1.0, 1.0], // Manhattan distance 3 from origin
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
                id: "point_2".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![2.0, 0.0, 0.0], // Manhattan distance 2 from origin
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
        ];

        // Insert vectors
        for vector in &manhattan_vectors {
            manager
                .insert(
                    collection_id.clone(),
                    crate::core::VectorId::from(vector.id.clone()),
                    vector.clone(),
                )
                .await
                .unwrap();
        }

        // Search from origin with Manhattan distance
        let query_vector = vec![0.0, 0.0, 0.0];
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                3,
                Some(DistanceMetric::Manhattan),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);

        // Verify ordering by Manhattan distance
        assert_eq!(results[0].0, "origin");
        assert!((results[0].1 - 0.0).abs() < 1e-6);

        assert_eq!(results[1].0, "point_2");
        assert!((results[1].1 - 2.0).abs() < 1e-6);

        assert_eq!(results[2].0, "point_1");
        assert!((results[2].1 - 3.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_wal_search_with_unified_dot_product() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());
        let vectors = create_distance_test_vectors();

        // Insert test vectors
        for vector in &vectors {
            manager
                .insert(
                    collection_id.clone(),
                    crate::core::VectorId::from(vector.id.clone()),
                    vector.clone(),
                )
                .await
                .unwrap();
        }

        // Search with dot product (similarity metric - higher is better)
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                5,
                Some(DistanceMetric::DotProduct),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 5);

        // Best match should be unit_x (unified distance = 0.0, from inverted dot product = 1.0)
        assert_eq!(results[0].0, "unit_x");
        assert!((results[0].1 - 0.0).abs() < 1e-6);

        // Orthogonal vectors should have unified distance = 1.0 (from inverted dot product = 0.0)
        let orthogonal_results: Vec<_> = results
            .iter()
            .filter(|(id, _, _)| id == "unit_y" || id == "unit_z")
            .collect();

        for (id, unified_distance, _) in orthogonal_results {
            assert!(
                (unified_distance - 1.0).abs() < 1e-6,
                "Orthogonal vector {} should have unified distance ≈ 1.0",
                id
            );
        }

        // Opposite vector should have unified distance = 2.0 (from inverted dot product = -1.0)
        let opposite_result = results
            .iter()
            .find(|(id, _, _)| id == "opposite_x")
            .expect("Should find opposite_x in results");
        assert!((opposite_result.1 - 2.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_wal_search_with_binary_vectors_hamming() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("binary_collection".to_string());

        // Create binary-like vectors for Hamming distance
        let now = Utc::now().timestamp_millis();
        let binary_vectors = vec![
            VectorRecord {
                id: "binary_1010".to_string(),
                collection_id: "binary_collection".to_string(),
                vector: vec![1.0, 0.0, 1.0, 0.0],
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
                id: "binary_1110".to_string(),
                collection_id: "binary_collection".to_string(),
                vector: vec![1.0, 1.0, 1.0, 0.0], // 1 bit different from 1010
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
                id: "binary_0101".to_string(),
                collection_id: "binary_collection".to_string(),
                vector: vec![0.0, 1.0, 0.0, 1.0], // All bits different from 1010
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
        ];

        // Insert vectors
        for vector in &binary_vectors {
            manager
                .insert(
                    collection_id.clone(),
                    crate::core::VectorId::from(vector.id.clone()),
                    vector.clone(),
                )
                .await
                .unwrap();
        }

        // Search with Hamming distance
        let query_vector = vec![1.0, 0.0, 1.0, 0.0]; // Same as binary_1010
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                3,
                Some(DistanceMetric::Hamming),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);

        // Best match should be identical vector (Hamming distance = 0)
        assert_eq!(results[0].0, "binary_1010");
        assert!((results[0].1 - 0.0).abs() < 1e-6);

        // Second best should be 1 bit different (Hamming distance = 1)
        assert_eq!(results[1].0, "binary_1110");
        assert!((results[1].1 - 1.0).abs() < 1e-6);

        // Worst should be all bits different (Hamming distance = 4)
        assert_eq!(results[2].0, "binary_0101");
        assert!((results[2].1 - 4.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_wal_search_with_set_vectors_jaccard() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("set_collection".to_string());

        // Create set-like vectors for Jaccard distance
        let now = Utc::now().timestamp_millis();
        let set_vectors = vec![
            VectorRecord {
                id: "set_abc".to_string(),
                collection_id: "set_collection".to_string(),
                vector: vec![1.0, 1.0, 1.0, 0.0], // Set {A, B, C}
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
                id: "set_ab".to_string(),
                collection_id: "set_collection".to_string(),
                vector: vec![1.0, 1.0, 0.0, 0.0], // Set {A, B}
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
                id: "set_cd".to_string(),
                collection_id: "set_collection".to_string(),
                vector: vec![0.0, 0.0, 1.0, 1.0], // Set {C, D}
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
        ];

        // Insert vectors
        for vector in &set_vectors {
            manager
                .insert(
                    collection_id.clone(),
                    crate::core::VectorId::from(vector.id.clone()),
                    vector.clone(),
                )
                .await
                .unwrap();
        }

        // Search with Jaccard distance
        let query_vector = vec![1.0, 1.0, 1.0, 0.0]; // Same as set_abc
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                3,
                Some(DistanceMetric::Jaccard),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);

        // Best match should be identical set (Jaccard distance = 0)
        assert_eq!(results[0].0, "set_abc");
        assert!((results[0].1 - 0.0).abs() < 1e-6);

        // Second should be subset (Jaccard distance = 1/3)
        assert_eq!(results[1].0, "set_ab");
        assert!((results[1].1 - 0.333333).abs() < 1e-4);

        // Worst should be least similar set (Jaccard distance = 0.75)
        // Query {A,B,C} vs set_cd {C,D}: intersection = {C} = 1, union = {A,B,C,D} = 4
        // Jaccard distance = 1 - 1/4 = 0.75
        assert_eq!(results[2].0, "set_cd");
        assert!((results[2].1 - 0.75).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_wal_distance_metric_hierarchy() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());
        let vector = create_distance_test_vectors()[0].clone();

        // Insert a test vector
        manager
            .insert(
                collection_id.clone(),
                crate::core::VectorId::from(vector.id.clone()),
                vector,
            )
            .await
            .unwrap();

        let query_vector = vec![1.0, 0.0, 0.0];

        // Test 1: Request override should take precedence
        let results_euclidean = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                1,
                Some(DistanceMetric::Euclidean),
            )
            .await
            .unwrap();

        assert_eq!(results_euclidean.len(), 1);
        assert!(
            (results_euclidean[0].1 - 0.0).abs() < 1e-6,
            "Should use Euclidean distance"
        );

        // Test 2: No request override - should use system default (Cosine)
        let results_default = manager
            .search_vectors_similarity(&collection_id, &query_vector, 1, None)
            .await
            .unwrap();

        assert_eq!(results_default.len(), 1);
        // With cosine distance, identical vectors should have distance 0.0
        assert!(
            (results_default[0].1 - 0.0).abs() < 1e-6,
            "Should use default Cosine distance"
        );
    }

    #[tokio::test]
    async fn test_wal_search_dimension_mismatch_handling() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());

        // Insert 3D vector
        let vector_3d = VectorRecord {
            id: "vector_3d".to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
            timestamp: Utc::now().timestamp_millis(),
            created_at: Utc::now().timestamp_millis(),
            updated_at: Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        manager
            .insert(
                collection_id.clone(),
                crate::core::VectorId::from(vector_3d.id.clone()),
                vector_3d,
            )
            .await
            .unwrap();

        // Search with 2D query vector (dimension mismatch)
        let query_vector_2d = vec![1.0, 2.0];
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector_2d,
                1,
                Some(DistanceMetric::Cosine),
            )
            .await
            .unwrap();

        // Should handle gracefully - either return empty or with fallback distance
        if !results.is_empty() {
            // If result returned, distance should be the fallback value for cosine (2.0)
            assert_eq!(
                results[0].1, 2.0,
                "Cosine distance mismatch should return 2.0 (maximum distance)"
            );
        }
    }

    #[tokio::test]
    async fn test_wal_batch_distance_calculations() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());
        let vectors = create_distance_test_vectors();

        // Insert batch of vectors
        let vector_ids: Vec<_> = vectors
            .iter()
            .map(|v| (crate::core::VectorId::from(v.id.clone()), v.clone()))
            .collect();

        manager
            .insert_batch(collection_id.clone(), vector_ids)
            .await
            .unwrap();

        // Test batch distance calculation efficiency
        let query_vector = vec![0.577, 0.577, 0.577]; // Diagonal vector

        // Search with different k values to test batch processing
        for k in [1, 3, 5] {
            let start = std::time::Instant::now();
            let results = manager
                .search_vectors_similarity(
                    &collection_id,
                    &query_vector,
                    k,
                    Some(DistanceMetric::Cosine),
                )
                .await
                .unwrap();
            let elapsed = start.elapsed();

            assert_eq!(
                results.len(),
                k.min(5),
                "Should return min(k, available) results"
            );
            println!("Search for k={} took {:?}", k, elapsed);

            // Best match should be the diagonal vector itself
            if k >= 1 {
                assert_eq!(
                    results[0].0, "diagonal",
                    "Best match should be diagonal vector"
                );
                assert!(
                    (results[0].1 - 0.0).abs() < 1e-5,
                    "Identical vectors should have distance ≈ 0.0"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_wal_custom_distance_metric_fallback() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        let collection_id = crate::core::CollectionId::from("test_collection".to_string());
        let vector = create_distance_test_vectors()[0].clone();

        manager
            .insert(
                collection_id.clone(),
                crate::core::VectorId::from(vector.id.clone()),
                vector,
            )
            .await
            .unwrap();

        // Test with custom metric (should fall back to cosine)
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = manager
            .search_vectors_similarity(
                &collection_id,
                &query_vector,
                1,
                Some(DistanceMetric::Custom("my_custom_metric".to_string())),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        // Should fall back to cosine distance
        assert!(
            (results[0].1 - 0.0).abs() < 1e-6,
            "Custom metric should fall back to cosine distance"
        );
    }

    #[tokio::test]
    async fn test_wal_hardware_acceleration_detection() {
        let (manager, _temp_dir) = create_test_wal_with_unified_distance()
            .await
            .expect("Failed to create WAL manager");

        // Access unified distance compute
        let distance_compute = manager.distance_compute();

        // Verify platform capability detection
        let platform_capability = crate::compute::distance::detect_platform_capability();
        println!("Detected platform capability: {:?}", platform_capability);

        // Test that distance calculation uses appropriate implementation
        let vec_a = vec![1.0; 128]; // Large vector to benefit from SIMD
        let vec_b = vec![0.5; 128];

        let start = std::time::Instant::now();
        let distance =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        let elapsed = start.elapsed();

        println!("Euclidean distance calculation took {:?}", elapsed);
        assert!(distance > 0.0, "Distance should be calculated");

        // Expected distance: sqrt(128 * (1.0 - 0.5)^2) = sqrt(128 * 0.25) = sqrt(32) ≈ 5.657
        assert!(
            (distance - 5.657).abs() < 0.01,
            "Distance calculation should be accurate"
        );
    }
}
