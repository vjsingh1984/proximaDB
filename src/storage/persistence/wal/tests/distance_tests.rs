//! Unit tests for WAL distance algorithm support

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
    use crate::core::VectorRecord;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::persistence::wal::{WalConfig, WalFactory, WalManager};
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a test WAL manager with temporary directory
    async fn create_test_wal_manager() -> (WalManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];

        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );
        let strategy = WalFactory::create_from_config(&config, filesystem)
            .await
            .expect("Failed to create WAL strategy");

        let manager = WalManager::new(strategy, config)
            .await
            .expect("Failed to create WAL manager");

        (manager, temp_dir)
    }

    /// Create test vector records with specified vectors
    fn create_test_vector_records() -> Vec<VectorRecord> {
        let now = Utc::now().timestamp_millis();
        vec![
            VectorRecord {
                id: "vector_1".to_string(),
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
                id: "vector_2".to_string(),
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
                id: "vector_3".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![0.5, 0.5, 0.0], // 45-degree vector in XY plane
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
                id: "vector_4".to_string(),
                collection_id: "test_collection".to_string(),
                vector: vec![1.0, 0.0, 0.0], // Identical to vector_1 (for duplicate testing)
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

    #[test]
    fn test_distance_metric_cosine_distance() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0];
        let vec_c = vec![1.0, 0.0, 0.0];

        // Test orthogonal vectors (cosine distance = 1.0)
        let distance_ab = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        assert!(
            (distance_ab - 1.0).abs() < 1e-6,
            "Orthogonal vectors should have cosine distance ≈ 1.0"
        );

        // Test identical vectors (cosine distance = 0.0)
        let distance_ac = distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Cosine);
        assert!(
            (distance_ac - 0.0).abs() < 1e-6,
            "Identical vectors should have cosine distance ≈ 0.0"
        );

        // Test 45-degree vectors (cosine distance = 1 - 0.707107 ≈ 0.293)
        let vec_d = vec![0.707107, 0.707107, 0.0]; // Normalized 45-degree vector
        let distance_ad = distance_compute.calculate_distance(&vec_a, &vec_d, &DistanceMetric::Cosine);
        assert!(
            (distance_ad - 0.292893).abs() < 1e-5,  // 1.0 - 0.707107
            "45-degree vectors should have cosine distance ≈ 0.293"
        );
        
        // Verify unified semantics: smaller distance = more similar
        assert!(distance_ac < distance_ad);  // Identical < 45-degree
        assert!(distance_ad < distance_ab);  // 45-degree < orthogonal
    }

    #[test]
    fn test_distance_metric_euclidean_distance() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![0.0, 0.0, 0.0];
        let vec_b = vec![3.0, 4.0, 0.0];
        let vec_c = vec![1.0, 1.0, 1.0];

        // Test 3-4-5 triangle (distance = 5)
        let distance_ab = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        assert!(
            (distance_ab - 5.0).abs() < 1e-6,
            "3-4-5 triangle should have distance = 5"
        );

        // Test identical vectors (distance = 0)
        let distance_aa = distance_compute.calculate_distance(&vec_a, &vec_a, &DistanceMetric::Euclidean);
        assert!(
            (distance_aa - 0.0).abs() < 1e-6,
            "Identical vectors should have distance = 0"
        );

        // Test unit cube diagonal
        let distance_ac = distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Euclidean);
        let expected = (3.0_f32).sqrt(); // sqrt(1^2 + 1^2 + 1^2)
        assert!(
            (distance_ac - expected).abs() < 1e-6,
            "Unit cube diagonal should have distance = sqrt(3)"
        );
    }

    #[test]
    fn test_distance_metric_manhattan_distance() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![0.0, 0.0, 0.0];
        let vec_b = vec![3.0, 4.0, 5.0];
        let vec_c = vec![1.0, -1.0, 2.0];

        // Test Manhattan distance calculation
        let distance_ab = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Manhattan);
        assert!(
            (distance_ab - 12.0).abs() < 1e-6,
            "Manhattan distance should be |3| + |4| + |5| = 12"
        );

        // Test with negative values
        let distance_ac = distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Manhattan);
        assert!(
            (distance_ac - 4.0).abs() < 1e-6,
            "Manhattan distance should be |1| + |-1| + |2| = 4"
        );

        // Test identical vectors
        let distance_aa = distance_compute.calculate_distance(&vec_a, &vec_a, &DistanceMetric::Manhattan);
        assert!(
            (distance_aa - 0.0).abs() < 1e-6,
            "Identical vectors should have Manhattan distance = 0"
        );
    }

    #[test]
    fn test_distance_metric_dot_product_unified() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![1.0, 2.0, 3.0];
        let vec_b = vec![4.0, 5.0, 6.0];
        let vec_c = vec![1.0, 0.0, 0.0];
        let vec_d = vec![0.0, 1.0, 0.0];

        // Test dot product calculation - NOTE: With unified distance, dot product similarity is inverted to distance
        // Raw dot product = 32, inverted distance = 1/32 ≈ 0.03125
        let dot_product_distance_ab = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
        let expected_distance = 1.0 / 32.0; // Inversion of high similarity to low distance
        assert!(
            (dot_product_distance_ab - expected_distance).abs() < 1e-6,
            "Dot product distance should be 1/32 ≈ 0.03125"
        );

        // Test orthogonal vectors (dot product = 0, distance = 1.0)
        let dot_product_distance_cd = distance_compute.calculate_distance(&vec_c, &vec_d, &DistanceMetric::DotProduct);
        assert!(
            (dot_product_distance_cd - 1.0).abs() < 1e-6,
            "Orthogonal vectors should have dot product distance = 1.0 (inverted from similarity 0)"
        );
        
        // Verify unified semantics: smaller distance = more similar
        assert!(dot_product_distance_ab < dot_product_distance_cd); // High similarity < orthogonal
    }

    #[test]
    fn test_distance_metric_hamming_distance() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![1.0, 0.0, 1.0, 0.0];
        let vec_b = vec![1.0, 1.0, 0.0, 0.0];
        let vec_c = vec![1.0, 0.0, 1.0, 0.0];

        // Test Hamming distance (2 positions differ)
        let hamming_ab = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Hamming);
        assert!(
            (hamming_ab - 2.0).abs() < 1e-6,
            "Hamming distance should be 2"
        );

        // Test identical vectors (Hamming distance = 0)
        let hamming_ac = distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Hamming);
        assert!(
            (hamming_ac - 0.0).abs() < 1e-6,
            "Identical vectors should have Hamming distance = 0"
        );
    }

    #[test]
    fn test_distance_metric_jaccard_distance() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![1.0, 0.0, 1.0, 1.0];
        let vec_b = vec![1.0, 1.0, 0.0, 1.0];
        let vec_c = vec![0.0, 0.0, 0.0, 0.0];

        // Test Jaccard distance calculation
        // Intersection: min(1,1) + min(0,1) + min(1,0) + min(1,1) = 1 + 0 + 0 + 1 = 2
        // Union: max(1,1) + max(0,1) + max(1,0) + max(1,1) = 1 + 1 + 1 + 1 = 4
        // Jaccard similarity = 2/4 = 0.5, Jaccard distance = 1 - 0.5 = 0.5
        let jaccard_ab = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Jaccard);
        assert!(
            (jaccard_ab - 0.5).abs() < 1e-6,
            "Jaccard distance should be 0.5"
        );

        // Test with zero vector (special case)
        let jaccard_ac = distance_compute.calculate_distance(&vec_a, &vec_c, &DistanceMetric::Jaccard);
        assert!(
            (jaccard_ac - 1.0).abs() < 1e-6,
            "Jaccard distance with zero vector should be 1.0"
        );
    }

    #[test]
    fn test_distance_metric_custom_fallback() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0];

        // Test custom metric falls back to cosine distance (unified semantics)
        let custom_distance = distance_compute.calculate_distance(
            &vec_a,
            &vec_b,
            &DistanceMetric::Custom("my_custom_metric".to_string()),
        );
        let cosine_distance = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);

        assert!(
            (custom_distance - cosine_distance).abs() < 1e-6,
            "Custom metric should fall back to cosine distance with unified semantics"
        );
    }

    #[test]
    fn test_distance_metric_dimension_mismatch() {
        let distance_compute = UnifiedDistanceCompute::default();
        let vec_a = vec![1.0, 2.0, 3.0];  // 3 dimensions
        let vec_b = vec![4.0, 5.0];       // 2 dimensions

        // Test unified dimension mismatch handling - all values use "lower = more similar" semantics
        
        // Cosine Distance: Should return maximum distance (2.0) for dimension mismatch
        let cosine_result = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        assert_eq!(
            cosine_result, 2.0,
            "Cosine with dimension mismatch should return 2.0 (maximum distance)"
        );

        // Euclidean Distance: Should return infinity for dimension mismatch
        let euclidean_result =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        assert!(
            euclidean_result.is_infinite(),
            "Euclidean with dimension mismatch should return infinity"
        );

        // Manhattan Distance: Should return infinity for dimension mismatch
        let manhattan_result =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Manhattan);
        assert!(
            manhattan_result.is_infinite(),
            "Manhattan with dimension mismatch should return infinity"
        );

        // Dot Product: Should return maximum distance (2.0) for dimension mismatch (unified behavior)
        let dot_product_result =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
        assert_eq!(
            dot_product_result, 2.0,
            "Dot product with dimension mismatch should return 2.0 (maximum distance)"
        );

        // Hamming Distance: Should return maximum discrete distance (1.0)
        let hamming_result = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Hamming);
        assert_eq!(
            hamming_result, 1.0,
            "Hamming with dimension mismatch should return 1.0"
        );

        // Jaccard Distance: Should return maximum discrete distance (1.0)
        let jaccard_result = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Jaccard);
        assert_eq!(
            jaccard_result, 1.0,
            "Jaccard with dimension mismatch should return 1.0"
        );
    }

    #[tokio::test]
    async fn test_distance_metric_hierarchy_request_override() {
        let distance_compute = UnifiedDistanceCompute::default();
        // Test that request-specified distance metric takes precedence
        let resolved_metric = distance_compute.resolve_distance_metric(
            Some(DistanceMetric::Euclidean), 
            None, 
            "test_collection"
        ).await;

        assert!(
            matches!(resolved_metric, DistanceMetric::Euclidean),
            "Request override should take precedence"
        );
    }

    #[tokio::test]
    async fn test_distance_metric_hierarchy_system_default() {
        let distance_compute = UnifiedDistanceCompute::default();
        // Test that system default (Cosine) is used when no override is provided
        let resolved_metric = distance_compute.resolve_distance_metric(None, None, "test_collection").await;

        assert!(
            matches!(resolved_metric, DistanceMetric::Cosine),
            "System default should be Cosine"
        );
    }

    #[tokio::test]
    async fn test_wal_search_with_different_distance_metrics() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let collection_id = "test_collection";
        let records = create_test_vector_records();

        // Insert test vectors
        for (i, record) in records.iter().enumerate() {
            let vector_id = format!("vector_{}", i + 1);
            let result = manager
                .insert(
                    crate::core::CollectionId::from(collection_id.to_string()),
                    crate::core::VectorId::from(vector_id),
                    record.clone(),
                )
                .await;
            assert!(result.is_ok(), "Failed to insert vector {}", i + 1);
        }

        let query_vector = vec![1.0, 0.0, 0.0]; // Same as vector_1
        let k = 3;

        // Test cosine similarity search
        let cosine_results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                k,
                Some(DistanceMetric::Cosine),
            )
            .await;

        assert!(
            cosine_results.is_ok(),
            "Cosine similarity search should succeed"
        );
        let cosine_results = cosine_results.unwrap();
        assert!(
            !cosine_results.is_empty(),
            "Cosine search should return results"
        );

        // With unified semantics, cosine distance: identical vectors should have the lowest distance (0.0)
        let best_cosine_result = &cosine_results[0];
        assert!(
            (best_cosine_result.1 - 0.0).abs() < 1e-6,
            "Best cosine result should have distance ≈ 0.0 (unified semantics)"
        );

        // Test Euclidean distance search
        let euclidean_results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                k,
                Some(DistanceMetric::Euclidean),
            )
            .await;

        assert!(
            euclidean_results.is_ok(),
            "Euclidean distance search should succeed"
        );
        let euclidean_results = euclidean_results.unwrap();
        assert!(
            !euclidean_results.is_empty(),
            "Euclidean search should return results"
        );

        // For Euclidean distance, identical vectors should have the lowest distance (0.0)
        let best_euclidean_result = &euclidean_results[0];
        assert!(
            (best_euclidean_result.1 - 0.0).abs() < 1e-6,
            "Best Euclidean result should have distance ≈ 0.0"
        );

        // Test Manhattan distance search
        let manhattan_results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                k,
                Some(DistanceMetric::Manhattan),
            )
            .await;

        assert!(
            manhattan_results.is_ok(),
            "Manhattan distance search should succeed"
        );
        let manhattan_results = manhattan_results.unwrap();
        assert!(
            !manhattan_results.is_empty(),
            "Manhattan search should return results"
        );

        // Test dot product search
        let dot_product_results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                k,
                Some(DistanceMetric::DotProduct),
            )
            .await;

        assert!(
            dot_product_results.is_ok(),
            "Dot product search should succeed"
        );
        let dot_product_results = dot_product_results.unwrap();
        assert!(
            !dot_product_results.is_empty(),
            "Dot product search should return results"
        );

        // With unified semantics, dot product distance: identical vectors should have the lowest distance (0.0)
        let best_dot_product_result = &dot_product_results[0];
        assert!(
            (best_dot_product_result.1 - 0.0).abs() < 1e-6,
            "Best dot product result should have distance ≈ 0.0 (unified semantics)"
        );
    }

    #[tokio::test]
    async fn test_wal_search_result_ordering() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let collection_id = "test_collection";

        // Create vectors with known relationships
        let now = Utc::now().timestamp_millis();
        let records = vec![
            VectorRecord {
                id: "identical".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![1.0, 0.0, 0.0], // Identical to query
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
                id: "orthogonal".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![0.0, 1.0, 0.0], // Orthogonal to query
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
                id: "opposite".to_string(),
                collection_id: collection_id.to_string(),
                vector: vec![-1.0, 0.0, 0.0], // Opposite to query
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

        // Insert test vectors
        for record in records {
            let result = manager
                .insert(
                    crate::core::CollectionId::from(collection_id.to_string()),
                    crate::core::VectorId::from(record.id.clone()),
                    record,
                )
                .await;
            assert!(result.is_ok(), "Failed to insert vector");
        }

        let query_vector = vec![1.0, 0.0, 0.0];

        // Test cosine similarity ordering (higher is better)
        let cosine_results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                3,
                Some(DistanceMetric::Cosine),
            )
            .await
            .unwrap();

        assert_eq!(cosine_results.len(), 3, "Should return 3 results");

        // Verify unified distance ordering: identical (0.0) < orthogonal (1.0) < opposite (2.0)
        // With unified semantics, ALL metrics use ascending order (lower = more similar)
        assert!(
            cosine_results[0].1 < cosine_results[1].1,
            "Unified semantics: cosine distance should be ordered by increasing distance"
        );
        assert!(
            cosine_results[1].1 < cosine_results[2].1,
            "Unified semantics: cosine distance should be ordered by increasing distance"
        );

        // Test Euclidean distance ordering (lower is better)
        let euclidean_results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                3,
                Some(DistanceMetric::Euclidean),
            )
            .await
            .unwrap();

        assert_eq!(euclidean_results.len(), 3, "Should return 3 results");

        // Verify Euclidean distance ordering: identical (0.0) < orthogonal (1.0) < opposite (2.0)
        assert!(
            euclidean_results[0].1 < euclidean_results[1].1,
            "Euclidean results should be ordered by increasing distance"
        );
        assert!(
            euclidean_results[1].1 < euclidean_results[2].1,
            "Euclidean results should be ordered by increasing distance"
        );

        // Verify the best result is the identical vector for both metrics
        assert_eq!(
            cosine_results[0].0, "identical",
            "Best cosine result should be identical vector"
        );
        assert_eq!(
            euclidean_results[0].0, "identical",
            "Best Euclidean result should be identical vector"
        );
    }

    #[tokio::test]
    async fn test_wal_search_empty_collection() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let collection_id = "empty_collection";
        let query_vector = vec![1.0, 0.0, 0.0];

        // Test search in empty collection
        let results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                5,
                Some(DistanceMetric::Cosine),
            )
            .await;

        assert!(results.is_ok(), "Search in empty collection should succeed");
        let results = results.unwrap();
        assert!(
            results.is_empty(),
            "Search in empty collection should return no results"
        );
    }

    #[tokio::test]
    async fn test_wal_search_large_k_value() {
        let (manager, _temp_dir) = create_test_wal_manager().await;
        let collection_id = "test_collection";
        let records = create_test_vector_records();

        // Insert test vectors
        for (i, record) in records.iter().enumerate() {
            let vector_id = format!("vector_{}", i + 1);
            let result = manager
                .insert(
                    crate::core::CollectionId::from(collection_id.to_string()),
                    crate::core::VectorId::from(vector_id),
                    record.clone(),
                )
                .await;
            assert!(result.is_ok(), "Failed to insert vector {}", i + 1);
        }

        let query_vector = vec![1.0, 0.0, 0.0];
        let large_k = 100; // Much larger than the number of vectors (4)

        // Test search with k larger than available vectors
        let results = manager
            .search_vectors_similarity(
                &crate::core::CollectionId::from(collection_id.to_string()),
                &query_vector,
                large_k,
                Some(DistanceMetric::Cosine),
            )
            .await;

        assert!(results.is_ok(), "Search with large k should succeed");
        let results = results.unwrap();
        assert_eq!(
            results.len(),
            4,
            "Should return only available vectors (4), not requested k (100)"
        );
    }
}
