//! Unit tests for GlobalPartitionedMemtable with unified distance system

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
    use crate::core::VectorRecord;
    use crate::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
    use crate::storage::persistence::wal::{WalEntry, WalOperation};
    use chrono::Utc;
    use std::collections::HashMap;

    /// Helper to create a WAL entry with vector data
    fn create_vector_wal_entry(
        entry_id: &str,
        collection_id: &str,
        vector_id: &str,
        vector: Vec<f32>,
        sequence: u64,
    ) -> WalEntry {
        let now = Utc::now();
        WalEntry {
            entry_id: entry_id.to_string(),
            collection_id: collection_id.to_string(),
            sequence,
            global_sequence: sequence,
            timestamp: now,
            expires_at: None,
            version: 1,
            operation: WalOperation::Insert {
                vector_id: vector_id.to_string(),
                record: VectorRecord {
                    id: vector_id.to_string(),
                    collection_id: collection_id.to_string(),
                    vector,
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
        }
    }

    #[tokio::test]
    async fn test_memtable_unified_distance_provider() {
        let memtable = GlobalPartitionedMemtable::new();

        // Verify memtable implements DistanceComputeProvider
        let distance_compute = memtable.distance_compute();

        // Test direct distance calculation
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![0.0, 1.0, 0.0];

        let cosine_distance =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        assert!(
            (cosine_distance - 1.0).abs() < 1e-6,
            "Orthogonal vectors should have cosine distance ≈ 1.0"
        );

        let euclidean_distance =
            distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
        assert!(
            (euclidean_distance - 1.414214).abs() < 1e-5,
            "Unit orthogonal vectors should have Euclidean distance ≈ √2"
        );
    }

    #[tokio::test]
    async fn test_memtable_unified_search_ordering() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection";

        // Create vectors with specific distance relationships
        let entries = vec![
            create_vector_wal_entry("e1", collection_id, "identical", vec![1.0, 0.0, 0.0], 1),
            create_vector_wal_entry("e2", collection_id, "similar", vec![0.9, 0.1, 0.0], 2),
            create_vector_wal_entry("e3", collection_id, "orthogonal", vec![0.0, 1.0, 0.0], 3),
            create_vector_wal_entry("e4", collection_id, "opposite", vec![-1.0, 0.0, 0.0], 4),
        ];

        // Insert entries
        for entry in entries {
            memtable.append(entry).await.unwrap();
        }

        let query_vector = vec![1.0, 0.0, 0.0];

        // Test Cosine distance (lower is better)
        let cosine_results = memtable
            .search_vectors(&query_vector, 4, collection_id, DistanceMetric::Cosine)
            .await
            .unwrap();

        assert_eq!(cosine_results.len(), 4);

        // Extract vector IDs for verification
        let cosine_ids: Vec<String> = cosine_results
            .iter()
            .map(|(_, entry)| match &entry.operation {
                WalOperation::Insert { vector_id, .. } => vector_id.clone(),
                _ => String::new(),
            })
            .collect();

        // Verify ordering: identical < similar < orthogonal < opposite (distance ordering)
        assert_eq!(
            cosine_ids[0], "identical",
            "Best match should be identical vector"
        );
        assert_eq!(
            cosine_ids[1], "similar",
            "Second best should be similar vector"
        );
        assert_eq!(
            cosine_ids[3], "opposite",
            "Worst match should be opposite vector"
        );

        // Test Euclidean distance (lower is better)
        let euclidean_results = memtable
            .search_vectors(&query_vector, 4, collection_id, DistanceMetric::Euclidean)
            .await
            .unwrap();

        assert_eq!(euclidean_results.len(), 4);

        let euclidean_ids: Vec<String> = euclidean_results
            .iter()
            .map(|(_, entry)| match &entry.operation {
                WalOperation::Insert { vector_id, .. } => vector_id.clone(),
                _ => String::new(),
            })
            .collect();

        // Verify ordering: identical < similar < orthogonal < opposite
        assert_eq!(
            euclidean_ids[0], "identical",
            "Best match should be identical vector"
        );
        assert_eq!(
            euclidean_ids[1], "similar",
            "Second best should be similar vector"
        );
        assert_eq!(
            euclidean_ids[3], "opposite",
            "Worst match should be opposite vector (distance 2.0)"
        );
    }

    #[tokio::test]
    async fn test_memtable_batch_distance_performance() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "perf_test";

        // Create many vectors for performance testing
        let num_vectors = 1000;
        let dimension = 128;

        for i in 0..num_vectors {
            let mut vector = vec![0.0; dimension];
            // Create diverse vectors
            vector[i % dimension] = 1.0;
            vector[(i + 1) % dimension] = 0.5;

            let entry = create_vector_wal_entry(
                &format!("entry_{}", i),
                collection_id,
                &format!("vector_{}", i),
                vector,
                i as u64 + 1,
            );

            memtable.append(entry).await.unwrap();
        }

        // Query vector
        let mut query_vector = vec![0.0; dimension];
        query_vector[0] = 1.0;
        query_vector[1] = 0.5;

        // Test search performance with unified distance
        let start = std::time::Instant::now();
        let results = memtable
            .search_vectors(&query_vector, 100, collection_id, DistanceMetric::Cosine)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 100);
        println!(
            "Unified distance search on {} vectors took {:?}",
            num_vectors, elapsed
        );

        // Verify that results are properly ordered by distance
        for i in 1..results.len() {
            assert!(
                results[i - 1].0 <= results[i].0,
                "Results should be ordered by increasing distance"
            );
        }
    }

    #[tokio::test]
    async fn test_memtable_multi_collection_unified_distance() {
        let memtable = GlobalPartitionedMemtable::new();

        // Create entries for different collections with different distance preferences
        let collections = vec![
            ("euclidean_collection", DistanceMetric::Euclidean),
            ("cosine_collection", DistanceMetric::Cosine),
            ("manhattan_collection", DistanceMetric::Manhattan),
        ];

        // Insert test data for each collection
        for (collection_id, _) in &collections {
            let entries = vec![
                create_vector_wal_entry("e1", collection_id, "v1", vec![1.0, 0.0], 1),
                create_vector_wal_entry("e2", collection_id, "v2", vec![0.0, 1.0], 2),
                create_vector_wal_entry("e3", collection_id, "v3", vec![0.707, 0.707], 3),
            ];

            for entry in entries {
                memtable.append(entry).await.unwrap();
            }
        }

        let query_vector = vec![1.0, 0.0];

        // Test each collection with its preferred distance metric
        for (collection_id, metric) in &collections {
            let results = memtable
                .search_vectors(&query_vector, 3, collection_id, metric.clone())
                .await
                .unwrap();

            assert_eq!(results.len(), 3);

            // Verify first result is always v1 (identical to query)
            match &results[0].1.operation {
                WalOperation::Insert { vector_id, .. } => {
                    assert_eq!(
                        vector_id, "v1",
                        "Best match should be v1 for collection {}",
                        collection_id
                    );
                }
                _ => panic!("Expected Insert operation"),
            }

            // Verify distance values match the metric
            match metric {
                DistanceMetric::Euclidean => {
                    assert!(
                        (results[0].0 - 0.0).abs() < 1e-6,
                        "Euclidean distance for identical vectors should be 0"
                    );
                }
                DistanceMetric::Cosine => {
                    assert!(
                        (results[0].0 - 0.0).abs() < 1e-6,
                        "Cosine distance for identical vectors should be 0"
                    );
                }
                DistanceMetric::Manhattan => {
                    assert!(
                        (results[0].0 - 0.0).abs() < 1e-6,
                        "Manhattan distance for identical vectors should be 0"
                    );
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_memtable_avro_payload_unified_distance() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "avro_collection";

        // Create regular insert entry
        let regular_entry = create_vector_wal_entry(
            "regular_entry",
            collection_id,
            "regular_vector",
            vec![1.0, 0.0, 0.0],
            1,
        );

        // Create Avro payload entry with vector data
        let vector_record = VectorRecord {
            id: "avro_vector".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![0.0, 1.0, 0.0],
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

        let avro_data = vector_record.to_avro_bytes().unwrap();

        let avro_entry = WalEntry {
            entry_id: "avro_entry".to_string(),
            collection_id: collection_id.to_string(),
            sequence: 2,
            global_sequence: 2,
            timestamp: Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::AvroPayload {
                operation_type: "insert".to_string(),
                avro_data,
            },
        };

        // Insert both entries
        memtable.append(regular_entry).await.unwrap();
        memtable.append(avro_entry).await.unwrap();

        // Search should find both vectors
        let query_vector = vec![0.5, 0.5, 0.0];
        let results = memtable
            .search_vectors(&query_vector, 2, collection_id, DistanceMetric::Cosine)
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            2,
            "Should find both regular and Avro payload vectors"
        );

        // Both vectors should have similar cosine distance to the query
        for (score, _) in &results {
            assert!(
                *score > 0.2 && *score < 0.3,
                "Both vectors should have similar cosine distance to 45-degree query"
            );
        }
    }

    #[tokio::test]
    async fn test_memtable_hardware_acceleration_batch() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "hardware_test";

        // Create high-dimensional vectors to benefit from SIMD
        let dimension = 256;
        let num_vectors = 100;

        for i in 0..num_vectors {
            let mut vector = vec![0.0; dimension];
            // Create pattern that benefits from SIMD
            for j in 0..dimension {
                vector[j] = ((i + j) % 10) as f32 / 10.0;
            }

            let entry = create_vector_wal_entry(
                &format!("entry_{}", i),
                collection_id,
                &format!("vector_{}", i),
                vector,
                i as u64 + 1,
            );

            memtable.append(entry).await.unwrap();
        }

        let query_vector: Vec<f32> = (0..dimension).map(|i| (i % 10) as f32 / 10.0).collect();

        // Time the search operation
        let start = std::time::Instant::now();
        let results = memtable
            .search_vectors(&query_vector, 50, collection_id, DistanceMetric::DotProduct)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 50);
        println!(
            "Hardware-accelerated search on {} {}-dimensional vectors took {:?}",
            num_vectors, dimension, elapsed
        );

        // Verify results are ordered correctly for dot product distance (lower is better)
        for i in 1..results.len() {
            assert!(
                results[i - 1].0 <= results[i].0,
                "Results should be ordered by increasing distance (inverted dot product)"
            );
        }
    }

    #[tokio::test]
    async fn test_memtable_distance_metric_consistency() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "consistency_test";

        // Create test vectors
        let entries = vec![
            create_vector_wal_entry("e1", collection_id, "v1", vec![1.0, 0.0], 1),
            create_vector_wal_entry("e2", collection_id, "v2", vec![0.0, 1.0], 2),
        ];

        for entry in entries {
            memtable.append(entry).await.unwrap();
        }

        let query_vector = vec![1.0, 0.0];

        // Test that the same query produces consistent results
        for _ in 0..5 {
            let results = memtable
                .search_vectors(&query_vector, 2, collection_id, DistanceMetric::Cosine)
                .await
                .unwrap();

            assert_eq!(results.len(), 2);

            // First result should always be v1
            match &results[0].1.operation {
                WalOperation::Insert { vector_id, .. } => {
                    assert_eq!(vector_id, "v1", "Best match should consistently be v1");
                }
                _ => panic!("Expected Insert operation"),
            }

            // Score should be consistent
            assert!(
                (results[0].0 - 0.0).abs() < 1e-6,
                "Cosine distance should be consistent (0 for identical vectors)"
            );
        }
    }

    #[tokio::test]
    async fn test_memtable_bincode_payload_unified_distance() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "bincode_collection";

        // Create regular insert entry (should work with both strategies)
        let regular_entry = create_vector_wal_entry(
            "regular_entry",
            collection_id,
            "regular_vector",
            vec![1.0, 0.0, 0.0],
            1,
        );

        // Create a Bincode-style WAL entry - this would be how the bincode strategy stores data
        let vector_record = VectorRecord {
            id: "bincode_vector".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![0.0, 1.0, 0.0],
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

        let bincode_entry = WalEntry {
            entry_id: "bincode_entry".to_string(),
            collection_id: collection_id.to_string(),
            sequence: 2,
            global_sequence: 2,
            timestamp: Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::Insert {
                vector_id: "bincode_vector".to_string(),
                record: vector_record,
                expires_at: None,
            },
        };

        // Insert both entries
        memtable.append(regular_entry).await.unwrap();
        memtable.append(bincode_entry).await.unwrap();

        // Search should find both vectors
        let query_vector = vec![0.5, 0.5, 0.0];
        let results = memtable
            .search_vectors(&query_vector, 2, collection_id, DistanceMetric::Cosine)
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            2,
            "Should find both regular and Bincode payload vectors"
        );

        // Both vectors should have similar cosine distance to the query
        for (score, _) in &results {
            assert!(
                *score > 0.2 && *score < 0.3,
                "Both vectors should have similar cosine distance to 45-degree query, got score: {}",
                score
            );
        }
    }

    #[tokio::test]
    async fn test_memtable_mixed_wal_operations_unified_distance() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "mixed_collection";

        // Create different types of WAL operations that could exist in the same memtable
        
        // 1. Regular Insert operation (Bincode strategy)
        let insert_entry = create_vector_wal_entry(
            "insert_entry",
            collection_id,
            "insert_vector",
            vec![1.0, 0.0, 0.0],
            1,
        );

        // 2. Update operation (Bincode strategy)
        let updated_vector_record = VectorRecord {
            id: "updated_vector".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![0.0, 1.0, 0.0],
            metadata: HashMap::new(),
            timestamp: Utc::now().timestamp_millis(),
            created_at: Utc::now().timestamp_millis(),
            updated_at: Utc::now().timestamp_millis(),
            expires_at: None,
            version: 2, // Updated version
            rank: None,
            score: None,
            distance: None,
        };

        let update_entry = WalEntry {
            entry_id: "update_entry".to_string(),
            collection_id: collection_id.to_string(),
            sequence: 2,
            global_sequence: 2,
            timestamp: Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::Update {
                vector_id: "updated_vector".to_string(),
                record: updated_vector_record,
                expires_at: None,
            },
        };

        // 3. Avro Payload operation (Avro strategy)
        let avro_vector_record = VectorRecord {
            id: "avro_vector".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![0.0, 0.0, 1.0],
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

        let avro_data = avro_vector_record.to_avro_bytes().unwrap();

        let avro_entry = WalEntry {
            entry_id: "avro_entry".to_string(),
            collection_id: collection_id.to_string(),
            sequence: 3,
            global_sequence: 3,
            timestamp: Utc::now(),
            expires_at: None,
            version: 1,
            operation: WalOperation::AvroPayload {
                operation_type: "insert".to_string(),
                avro_data,
            },
        };

        // Insert all entries
        memtable.append(insert_entry).await.unwrap();
        memtable.append(update_entry).await.unwrap();
        memtable.append(avro_entry).await.unwrap();

        // Search should find all three vectors
        let query_vector = vec![0.333, 0.333, 0.333]; // Equidistant to all three unit vectors
        let results = memtable
            .search_vectors(&query_vector, 10, collection_id, DistanceMetric::Cosine)
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            3,
            "Should find all three vectors (Insert, Update, AvroPayload)"
        );

        // All vectors should have reasonable similarity scores
        for (score, entry) in &results {
            assert!(
                *score > 0.4 && *score < 0.8,
                "All vectors should have reasonable cosine distance to equidistant query, got score: {} for entry: {}",
                score, entry.entry_id
            );
        }

        // Verify we have all three operation types represented
        let operation_types: std::collections::HashSet<_> = results
            .iter()
            .map(|(_, entry)| match &entry.operation {
                WalOperation::Insert { .. } => "Insert",
                WalOperation::Update { .. } => "Update", 
                WalOperation::AvroPayload { .. } => "AvroPayload",
                _ => "Other",
            })
            .collect();

        assert_eq!(operation_types.len(), 3, "Should have all three operation types");
        assert!(operation_types.contains("Insert"));
        assert!(operation_types.contains("Update"));
        assert!(operation_types.contains("AvroPayload"));
    }
}
