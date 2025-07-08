//! Unit tests for GlobalPartitionedMemtable distance algorithm support

#[cfg(test)]
mod tests {
    use crate::compute::distance::DistanceMetric;
    use crate::core::VectorRecord;
    use crate::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
    use crate::storage::persistence::wal::{WalEntry, WalOperation};
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;

    /// Create a test vector record
    fn create_test_vector_record(
        id: &str,
        collection_id: &str,
        vector: Vec<f32>,
        now: DateTime<Utc>,
    ) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
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
        }
    }

    /// Create test WAL entries with specified vectors and collection using modern AvroPayload format
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
                batch_id: None,
                operation: WalOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_1", collection_id, vec![1.0, 0.0, 0.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
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
                batch_id: None,
                operation: WalOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_2", collection_id, vec![0.0, 1.0, 0.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
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
                batch_id: None,
                operation: WalOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_3", collection_id, vec![0.0, 0.0, 1.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
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
                batch_id: None,
                operation: WalOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_4", collection_id, vec![-1.0, 0.0, 0.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
                },
            },
        ]
    }

    /// Test cosine distance search functionality
    #[tokio::test]
    async fn test_memtable_cosine_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection_cosine";
        let entry_count = 4;

        // Create test WAL entries with orthogonal and opposite vectors
        let entries = create_test_wal_entries(collection_id);

        // Insert all entries into the memtable
        for entry in entries {
            memtable.append(entry).await.expect("Failed to append entry");
        }

        println!("Memtable entries: {}", memtable.len().await);

        // Search for vectors similar to [1.0, 0.0, 0.0] using cosine distance
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = memtable
            .search_vectors(&query_vector, entry_count, collection_id, DistanceMetric::Cosine)
            .await;

        assert!(results.is_ok(), "Cosine distance search should succeed");

        let results = results.unwrap();
        println!("Search returned {} results (expected {})", results.len(), entry_count);
        
        // Debug: Print all results with their distances and vectors
        for (i, (distance, entry)) in results.iter().enumerate() {
            if let WalOperation::AvroPayload { avro_data, .. } = &entry.operation {
                if let Ok(vector_record) = VectorRecord::from_avro_bytes(avro_data) {
                    println!("Result {}: distance={:.6}, vector={:?}, id={}", 
                             i, distance, vector_record.vector, vector_record.id);
                }
            }
        }
        
        assert_eq!(results.len(), 4, "Should return all 4 vectors");

        // Verify ordering: identical (0.0) < orthogonal (1.0) < opposite (2.0)
        // Note: orthogonal vectors may have same distance, so we use <= for ties
        assert!(
            results[0].0 < results[1].0,
            "Best result should have lower distance than others: {} < {}",
            results[0].0, results[1].0
        );
        assert!(
            results[1].0 <= results[2].0,
            "Results should be ordered by non-decreasing cosine distance: {} <= {}",
            results[1].0, results[2].0
        );
        assert!(
            results[2].0 <= results[3].0,
            "Results should be ordered by non-decreasing cosine distance: {} <= {}",
            results[2].0, results[3].0
        );
        
        // Verify expected distance values
        assert!(
            (results[0].0 - 0.0).abs() < 1e-6,
            "Identical vector should have distance ≈ 0.0, got {}",
            results[0].0
        );
        assert!(
            (results[1].0 - 1.0).abs() < 1e-6,
            "Orthogonal vector should have distance ≈ 1.0, got {}",
            results[1].0
        );
        assert!(
            (results[3].0 - 2.0).abs() < 1e-6,
            "Opposite vector should have distance ≈ 2.0, got {}",
            results[3].0
        );

        // Verify the best match is the identical vector
        let best_result = &results[0];
        assert!(
            (best_result.0 - 0.0).abs() < 1e-6,
            "Best result should have cosine distance ≈ 0.0"
        );

        // Extract vector ID from the best result using modern AvroPayload
        if let WalOperation::AvroPayload { avro_data, .. } = &best_result.1.operation {
            let vector_record = VectorRecord::from_avro_bytes(avro_data)
                .expect("Failed to deserialize vector record");
            assert_eq!(vector_record.id, "vector_1", "Best result should be vector_1");
        } else {
            panic!("Expected AvroPayload operation in search results");
        }
    }

    /// Test euclidean distance search functionality
    #[tokio::test]
    async fn test_memtable_euclidean_distance_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "test_collection_euclidean";
        let entry_count = 4;

        // Create test WAL entries
        let entries = create_test_wal_entries(collection_id);

        // Insert all entries into the memtable
        for entry in entries {
            memtable.append(entry).await.expect("Failed to append entry");
        }

        // Search for vectors similar to [1.0, 0.0, 0.0] using euclidean distance
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = memtable
            .search_vectors(&query_vector, entry_count, collection_id, DistanceMetric::Euclidean)
            .await;

        assert!(results.is_ok(), "Euclidean distance search should succeed");

        let results = results.unwrap();
        assert_eq!(results.len(), 4, "Should return all 4 vectors");

        // Verify the best match is the identical vector
        let best_result = &results[0];
        assert!(
            (best_result.0 - 0.0).abs() < 1e-6,
            "Best result should have euclidean distance ≈ 0.0"
        );

        // Extract vector ID from the best result using modern AvroPayload
        if let WalOperation::AvroPayload { avro_data, .. } = &best_result.1.operation {
            let vector_record = VectorRecord::from_avro_bytes(avro_data)
                .expect("Failed to deserialize vector record");
            assert_eq!(vector_record.id, "vector_1", "Best result should be vector_1");
        } else {
            panic!("Expected AvroPayload operation in search results");
        }
    }

    /// Test empty collection search
    #[tokio::test]
    async fn test_memtable_empty_collection_search() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_id = "empty_collection";

        // Search in empty collection
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = memtable
            .search_vectors(&query_vector, 10, collection_id, DistanceMetric::Cosine)
            .await;

        assert!(results.is_ok(), "Empty collection search should succeed");
        let results = results.unwrap();
        assert_eq!(results.len(), 0, "Empty collection should return no results");
    }

    /// Test cross-collection isolation
    #[tokio::test]
    async fn test_memtable_cross_collection_isolation() {
        let memtable = GlobalPartitionedMemtable::new();
        let collection_a = "collection_a";
        let collection_b = "collection_b";

        // Insert entries into collection A
        let entries_a = create_test_wal_entries(collection_a);
        for entry in entries_a {
            memtable.append(entry).await.expect("Failed to append entry");
        }

        // Search in collection B (should be empty)
        let query_vector = vec![1.0, 0.0, 0.0];
        let results_b = memtable
            .search_vectors(&query_vector, 10, collection_b, DistanceMetric::Cosine)
            .await;

        assert!(results_b.is_ok(), "Cross-collection search should succeed");
        let results_b = results_b.unwrap();
        assert_eq!(results_b.len(), 0, "Collection B should have no results");

        // Search in collection A (should have results)
        let results_a = memtable
            .search_vectors(&query_vector, 10, collection_a, DistanceMetric::Cosine)
            .await;

        assert!(results_a.is_ok(), "Collection A search should succeed");
        let results_a = results_a.unwrap();
        assert_eq!(results_a.len(), 4, "Collection A should have 4 results");
    }
}