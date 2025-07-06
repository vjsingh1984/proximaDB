//! Unit tests for GlobalPartitionedMemtable distance algorithm support

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::core::VectorRecord;
    use crate::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
    use crate::storage::persistence::wal::{WalEntry, WalOperation};
    use chrono::Utc;
    use std::collections::HashMap;

    /// Create test WAL entries with specified vectors and collection
    fn create_test_wal_entries(collection_id: &str) -> Vec<WalEntry> {
        let now = Utc::now();

        vec![
            WalEntry {
                entry_id: "entry_1".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 1,
                global_sequence: 1,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "vector_1".to_string(),
                    record: VectorRecord {
                        id: "vector_1".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![1.0, 0.0, 0.0], // Unit vector along X-axis
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "entry_2".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 2,
                global_sequence: 2,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "vector_2".to_string(),
                    record: VectorRecord {
                        id: "vector_2".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![0.0, 1.0, 0.0], // Unit vector along Y-axis
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "entry_3".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 3,
                global_sequence: 3,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "vector_3".to_string(),
                    record: VectorRecord {
                        id: "vector_3".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![0.707107, 0.707107, 0.0], // 45-degree normalized vector
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "entry_4".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 4,
                global_sequence: 4,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "vector_4".to_string(),
                    record: VectorRecord {
                        id: "vector_4".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![-1.0, 0.0, 0.0], // Opposite vector along X-axis
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
        ]
    }

    #[tokio::test]
    async fn test_memtable_cosine_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";
        let entries = create_test_wal_entries(collection_id);

        // Insert test entries
        for entry in entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 0.0]; // Same as vector_1
        let k = 4;

        // Test cosine distance search
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Cosine)
            .await;
        assert!(results.is_ok(), "Cosine distance search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Should return all 4 vectors");

        // Verify ordering: identical (0.0) < 45-degree (~0.293) < orthogonal (1.0) < opposite (2.0)
        assert!(
            results[0].0 < results[1].0,
            "Results should be ordered by increasing cosine distance"
        );
        assert!(
            results[1].0 < results[2].0,
            "Results should be ordered by increasing cosine distance"
        );
        assert!(
            results[2].0 < results[3].0,
            "Results should be ordered by increasing cosine distance"
        );

        // Verify the best match is the identical vector
        let best_result = &results[0];
        assert!(
            (best_result.0 - 0.0).abs() < 1e-6,
            "Best result should have cosine distance ≈ 0.0"
        );

        // Extract vector ID from the best result
        if let WalOperation::Insert { vector_id, .. } = &best_result.1.operation {
            assert_eq!(vector_id, "vector_1", "Best result should be vector_1");
        } else {
            panic!("Expected Insert operation");
        }
    }

    #[tokio::test]
    async fn test_memtable_euclidean_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";
        let entries = create_test_wal_entries(collection_id);

        // Insert test entries
        for entry in entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 0.0]; // Same as vector_1
        let k = 4;

        // Test Euclidean distance search
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Euclidean)
            .await;
        assert!(results.is_ok(), "Euclidean distance search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Should return all 4 vectors");

        // Verify ordering: identical (0.0) < 45-degree < orthogonal < opposite (2.0)
        assert!(
            results[0].0 < results[1].0,
            "Results should be ordered by increasing Euclidean distance"
        );
        assert!(
            results[1].0 < results[2].0,
            "Results should be ordered by increasing Euclidean distance"
        );
        assert!(
            results[2].0 < results[3].0,
            "Results should be ordered by increasing Euclidean distance"
        );

        // Verify the best match is the identical vector with distance ≈ 0
        let best_result = &results[0];
        assert!(
            (best_result.0 - 0.0).abs() < 1e-6,
            "Best result should have Euclidean distance ≈ 0.0"
        );

        // Extract vector ID from the best result
        if let WalOperation::Insert { vector_id, .. } = &best_result.1.operation {
            assert_eq!(vector_id, "vector_1", "Best result should be vector_1");
        } else {
            panic!("Expected Insert operation");
        }

        // Verify the worst match is the opposite vector with distance ≈ 2.0
        let worst_result = &results[3];
        assert!(
            (worst_result.0 - 2.0).abs() < 1e-6,
            "Worst result should have Euclidean distance ≈ 2.0"
        );
    }

    #[tokio::test]
    async fn test_memtable_manhattan_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";
        let entries = create_test_wal_entries(collection_id);

        // Insert test entries
        for entry in entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 0.0]; // Same as vector_1
        let k = 4;

        // Test Manhattan distance search
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Manhattan)
            .await;
        assert!(results.is_ok(), "Manhattan distance search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Should return all 4 vectors");

        // Verify ordering: identical (0.0) < others
        assert!(
            results[0].0 < results[1].0,
            "Results should be ordered by increasing Manhattan distance"
        );

        // Verify the best match is the identical vector with distance = 0
        let best_result = &results[0];
        assert!(
            (best_result.0 - 0.0).abs() < 1e-6,
            "Best result should have Manhattan distance = 0.0"
        );

        // Extract vector ID from the best result
        if let WalOperation::Insert { vector_id, .. } = &best_result.1.operation {
            assert_eq!(vector_id, "vector_1", "Best result should be vector_1");
        } else {
            panic!("Expected Insert operation");
        }
    }

    #[tokio::test]
    async fn test_memtable_dot_product_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";
        let entries = create_test_wal_entries(collection_id);

        // Insert test entries
        for entry in entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 0.0]; // Same as vector_1
        let k = 4;

        // Test dot product search
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::DotProduct)
            .await;
        assert!(results.is_ok(), "Dot product search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Should return all 4 vectors");

        // Verify ordering: identical (0.0) < 45-degree < orthogonal (1.0) < opposite (2.0)
        assert!(
            results[0].0 < results[1].0,
            "Results should be ordered by increasing dot product distance"
        );
        assert!(
            results[1].0 < results[2].0,
            "Results should be ordered by increasing dot product distance"
        );
        assert!(
            results[2].0 < results[3].0,
            "Results should be ordered by increasing dot product distance"
        );

        // Verify the best match is the identical vector with dot product distance = 0.0
        let best_result = &results[0];
        assert!(
            (best_result.0 - 0.0).abs() < 1e-6,
            "Best result should have dot product distance = 0.0"
        );

        // Verify the orthogonal vector has dot product distance = 1.0
        let orthogonal_result = &results[2];
        assert!(
            (orthogonal_result.0 - 1.0).abs() < 1e-6,
            "Orthogonal vector should have dot product distance = 1.0"
        );

        // Verify the opposite vector has dot product distance = 2.0
        let opposite_result = &results[3];
        assert!(
            (opposite_result.0 - 2.0).abs() < 1e-6,
            "Opposite vector should have dot product distance = 2.0"
        );
    }

    #[tokio::test]
    async fn test_memtable_hamming_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";

        // Create binary-like vectors for Hamming distance testing
        let now = Utc::now();
        let binary_entries = vec![
            WalEntry {
                entry_id: "binary_1".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 1,
                global_sequence: 1,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "binary_1".to_string(),
                    record: VectorRecord {
                        id: "binary_1".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![1.0, 0.0, 1.0, 0.0], // Binary pattern 1010
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "binary_2".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 2,
                global_sequence: 2,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "binary_2".to_string(),
                    record: VectorRecord {
                        id: "binary_2".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![1.0, 1.0, 1.0, 0.0], // Binary pattern 1110 (1 bit different)
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "binary_3".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 3,
                global_sequence: 3,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "binary_3".to_string(),
                    record: VectorRecord {
                        id: "binary_3".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![0.0, 1.0, 0.0, 1.0], // Binary pattern 0101 (all bits different)
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
        ];

        // Insert test entries
        for entry in binary_entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 1.0, 0.0]; // Same as binary_1 (pattern 1010)
        let k = 3;

        // Test Hamming distance search
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Hamming)
            .await;
        assert!(results.is_ok(), "Hamming distance search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 3, "Should return all 3 vectors");

        // Verify ordering: identical (0) < 1 bit different (1) < all bits different (4)
        assert!(
            results[0].0 < results[1].0,
            "Results should be ordered by increasing Hamming distance"
        );
        assert!(
            results[1].0 < results[2].0,
            "Results should be ordered by increasing Hamming distance"
        );

        // Verify the exact Hamming distances
        assert!(
            (results[0].0 - 0.0).abs() < 1e-6,
            "Identical vectors should have Hamming distance = 0"
        );
        assert!(
            (results[1].0 - 1.0).abs() < 1e-6,
            "1 bit different should have Hamming distance = 1"
        );
        assert!(
            (results[2].0 - 4.0).abs() < 1e-6,
            "All bits different should have Hamming distance = 4"
        );
    }

    #[tokio::test]
    async fn test_memtable_jaccard_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";

        // Create set-like vectors for Jaccard distance testing
        let now = Utc::now();
        let set_entries = vec![
            WalEntry {
                entry_id: "set_1".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 1,
                global_sequence: 1,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "set_1".to_string(),
                    record: VectorRecord {
                        id: "set_1".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![1.0, 0.0, 1.0, 1.0], // Set {1, 3, 4}
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "set_2".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 2,
                global_sequence: 2,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "set_2".to_string(),
                    record: VectorRecord {
                        id: "set_2".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![1.0, 1.0, 0.0, 1.0], // Set {1, 2, 4} - 2/4 intersection with query
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
            WalEntry {
                entry_id: "set_3".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 3,
                global_sequence: 3,
                timestamp: now,
                expires_at: None,
                version: 1,
                operation: WalOperation::Insert {
                    vector_id: "set_3".to_string(),
                    record: VectorRecord {
                        id: "set_3".to_string(),
                        collection_id: collection_id.to_string(),
                        vector: vec![0.0, 1.0, 0.0, 0.0], // Set {2} - 0/4 intersection with query
                        metadata: HashMap::new(),
                        timestamp: now.timestamp_millis(),
                        created_at: now.timestamp_millis(),
                        updated_at: now.timestamp_millis(),
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    },
                    expires_at: None,
                },
            },
        ];

        // Insert test entries
        for entry in set_entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 1.0, 1.0]; // Same as set_1
        let k = 3;

        // Test Jaccard distance search
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Jaccard)
            .await;
        assert!(results.is_ok(), "Jaccard distance search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 3, "Should return all 3 vectors");

        // Verify ordering: identical (0.0) < partial overlap < no overlap
        assert!(
            results[0].0 < results[1].0,
            "Results should be ordered by increasing Jaccard distance"
        );
        assert!(
            results[1].0 < results[2].0,
            "Results should be ordered by increasing Jaccard distance"
        );

        // Verify the best match is the identical set with distance = 0.0
        assert!(
            (results[0].0 - 0.0).abs() < 1e-6,
            "Identical sets should have Jaccard distance = 0.0"
        );
    }

    #[tokio::test]
    async fn test_memtable_custom_metric_fallback() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";
        let entries = create_test_wal_entries(collection_id);

        // Insert test entries
        for entry in entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 0.0];
        let k = 4;

        // Test custom metric (should fall back to cosine distance)
        let custom_results = memtable
            .search_vectors(
                &query_vector,
                k,
                collection_id,
                DistanceMetric::Custom("my_custom_metric".to_string()),
            )
            .await;
        assert!(
            custom_results.is_ok(),
            "Custom metric search should succeed"
        );

        let cosine_results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Cosine)
            .await;
        assert!(cosine_results.is_ok(), "Cosine search should succeed");

        let custom_results = custom_results.unwrap();
        let cosine_results = cosine_results.unwrap();

        assert_eq!(
            custom_results.len(),
            cosine_results.len(),
            "Custom and cosine should return same number of results"
        );

        // Verify that custom metric produces the same results as cosine distance
        for (i, (custom_result, cosine_result)) in
            custom_results.iter().zip(cosine_results.iter()).enumerate()
        {
            assert!(
                (custom_result.0 - cosine_result.0).abs() < 1e-6,
                "Custom metric result {} should match cosine result",
                i
            );
        }
    }

    #[tokio::test]
    async fn test_memtable_multiple_collections() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_a = "collection_a";
        let collection_b = "collection_b";

        // Create entries for collection A
        let entries_a = create_test_wal_entries(collection_a);
        // Create entries for collection B with different vectors
        let mut entries_b = create_test_wal_entries(collection_b);

        // Modify collection B vectors to be different (rotated by 45 degrees)
        for entry in &mut entries_b {
            if let WalOperation::Insert { record, .. } = &mut entry.operation {
                // Rotate vectors by 45 degrees to create meaningful distance differences for cosine
                let original = record.vector.clone();
                if original.len() >= 2 {
                    // Simple 45-degree rotation in 2D plane for first two dimensions
                    let cos45 = 0.707107; // cos(45°)
                    let sin45 = 0.707107; // sin(45°)
                    record.vector[0] = cos45 * original[0] - sin45 * original[1];
                    record.vector[1] = sin45 * original[0] + cos45 * original[1];
                    // Keep other dimensions unchanged
                }
                record.collection_id = collection_b.to_string();
            }
            entry.collection_id = collection_b.to_string();
        }

        // Insert entries for both collections
        for entry in entries_a.into_iter().chain(entries_b.into_iter()) {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        let query_vector = vec![1.0, 0.0, 0.0];
        let k = 10; // Request more than available in each collection

        // Test search in collection A
        let results_a = memtable
            .search_vectors(&query_vector, k, collection_a, DistanceMetric::Cosine)
            .await;
        assert!(results_a.is_ok(), "Search in collection A should succeed");
        let results_a = results_a.unwrap();
        assert_eq!(results_a.len(), 4, "Collection A should have 4 results");

        // Test search in collection B
        let results_b = memtable
            .search_vectors(&query_vector, k, collection_b, DistanceMetric::Cosine)
            .await;
        assert!(results_b.is_ok(), "Search in collection B should succeed");
        let results_b = results_b.unwrap();
        assert_eq!(results_b.len(), 4, "Collection B should have 4 results");

        // Verify that results are from different collections
        for result in &results_a {
            if let WalOperation::Insert { record, .. } = &result.1.operation {
                assert_eq!(
                    record.collection_id, collection_a,
                    "Result should be from collection A"
                );
            }
        }

        for result in &results_b {
            if let WalOperation::Insert { record, .. } = &result.1.operation {
                assert_eq!(
                    record.collection_id, collection_b,
                    "Result should be from collection B"
                );
            }
        }

        // Verify different distance scores due to rotated vectors in collection B
        assert!(
            results_a[0].0 < results_b[0].0,
            "Collection A should have lower cosine distance for query [1,0,0] due to 45-degree rotation in collection B"
        );
    }

    #[tokio::test]
    async fn test_memtable_empty_collection_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let query_vector = vec![1.0, 0.0, 0.0];
        let k = 5;

        // Test search in empty collection
        let results = memtable
            .search_vectors(&query_vector, k, "empty_collection", DistanceMetric::Cosine)
            .await;
        assert!(results.is_ok(), "Search in empty collection should succeed");

        let results = results.unwrap();
        assert!(
            results.is_empty(),
            "Empty collection should return no results"
        );
    }

    #[tokio::test]
    async fn test_memtable_dimension_mismatch_handling() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";
        let entries = create_test_wal_entries(collection_id);

        // Insert test entries (3D vectors)
        for entry in entries {
            let result = memtable.append(entry).await;
            assert!(result.is_ok(), "Failed to append entry to memtable");
        }

        // Query with different dimension (2D vector)
        let query_vector = vec![1.0, 0.0]; // 2D instead of 3D
        let k = 4;

        // Test search with dimension mismatch - should handle gracefully
        let results = memtable
            .search_vectors(&query_vector, k, collection_id, DistanceMetric::Cosine)
            .await;
        assert!(
            results.is_ok(),
            "Search with dimension mismatch should succeed"
        );

        let results = results.unwrap();
        // Results should be empty or contain entries with appropriate distance values
        // The exact behavior depends on the implementation's handling of dimension mismatches
        assert!(
            results.len() <= 4,
            "Should not return more results than available"
        );
    }
}
