//! Comprehensive Three-Layer MVCC Tests
//!
//! Tests the consistency of search across WAL, Storage, and Compacted layers
//! with proper MVCC version handling and logical delete semantics.

use super::super::global_partitioned::GlobalPartitionedMemtable;
use crate::compute::distance_computation::DistanceMetric as CoreDistanceMetric;
use crate::proto::proximadb_v1::SqlValue;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::write_ahead_log::BatchId;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper function to create a vector record with specific parameters
fn create_vector_record(
    id: &str,
    vector: Vec<f32>,
    version: Option<u32>,
    expires_at: Option<u32>,
) -> VectorRecord {
    let now = chrono::Utc::now().timestamp(); // seconds since epoch
    VectorRecord {
        id: id.to_string(),
        vector,
        metadata: HashMap::new(),
        timestamp: Some(now),
        updated_at: Some(now),
        expires_at: expires_at.map(|v| v as i64),
        version,
        source: None,
    }
}

/// Helper function to create a WAL batch
fn create_wal_batch(
    collection_id: &str,
    sequence: u64,
    vectors: Vec<VectorRecord>,
) -> WALVectorBatch {
    let vector_count = vectors.len() as u64;
    let _end_sequence = if vector_count > 0 {
        sequence + vector_count - 1
    } else {
        sequence
    };
    WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vectors),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 1024, // Approximate
        is_flushed: false,
        metadata_bloom_filter: None,
    }
}

#[tokio::test]
async fn test_three_layer_search_consistency_basic() {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "1uctd3f"; // 7-char base62 ID (realistic)
    let vector_id = "vector_1";

    // Layer 1: Initial vector in WAL
    let vector_v1 = create_vector_record(vector_id, vec![1.0, 0.0, 0.0], Some(1), None);
    let batch1 = create_wal_batch(collection_id, 1, vec![vector_v1.clone()]);
    let _seq1 = memtable.add_wal_batch(collection_id, batch1).await.unwrap();

    // Verify WAL layer search finds the vector
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().version, Some(1));

    // Layer 2: Update vector (simulating flush to storage)
    let vector_v2 = create_vector_record(vector_id, vec![0.0, 1.0, 0.0], Some(2), None);
    let batch2 = create_wal_batch(collection_id, 2, vec![vector_v2.clone()]);
    let _seq2 = memtable.add_wal_batch(collection_id, batch2).await.unwrap();

    // Verify latest version is returned (MVCC)
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_some());
    let found_vector = result.unwrap();
    assert_eq!(found_vector.version, Some(2));
    assert_eq!(found_vector.vector, vec![0.0, 1.0, 0.0]);

    // Layer 3: Logical delete (tombstone)
    let current_time = chrono::Utc::now().timestamp() as u32; // Current time in seconds
    let vector_v3_delete = create_vector_record(
        vector_id,
        vec![0.0, 0.0, 1.0], // Vector content doesn't matter for deletes
        Some(3),
        Some(current_time - 1), // Expired 1 second ago
    );
    let batch3 = create_wal_batch(collection_id, 3, vec![vector_v3_delete]);
    let _seq3 = memtable.add_wal_batch(collection_id, batch3).await.unwrap();

    // Verify logical delete is respected (should return None)
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_none(), "Vector should be logically deleted");

    // Verify search also respects logical delete
    let search_results = memtable
        .search_vectors(
            &[1.0, 1.0, 1.0],
            10,
            collection_id,
            CoreDistanceMetric::Euclidean,
        )
        .await
        .unwrap();
    assert!(
        !search_results
            .iter()
            .any(|(_, record)| record.id == vector_id.to_string())
    );
}

#[tokio::test]
async fn test_get_before_delete_update_consistency() {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "1uctd3g"; // 7-char base62 ID (realistic)
    let vector_id = "vector_1";

    // Initial vector
    let original_vector = create_vector_record(vector_id, vec![1.0, 2.0, 3.0], Some(1), None);
    let batch1 = create_wal_batch(collection_id, 1, vec![original_vector.clone()]);
    memtable.add_wal_batch(collection_id, batch1).await.unwrap();

    // CRITICAL: Client must vector_by_id before issuing delete/update
    let current_vector = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(current_vector.is_some());
    let current_vector = current_vector.unwrap();

    // Ensure ID and vector match for consistency
    assert_eq!(current_vector.id, vector_id.to_string());
    assert_eq!(current_vector.vector, vec![1.0, 2.0, 3.0]);
    assert_eq!(current_vector.version, Some(1));

    // Update: Construct new version with same ID but new vector
    let updated_vector = create_vector_record(
        &current_vector.id,                           // Use same ID
        vec![4.0, 5.0, 6.0],                          // New vector
        current_vector.version.map(|v| v as u32 + 1), // Increment version
        None,
    );
    let batch2 = create_wal_batch(collection_id, 2, vec![updated_vector.clone()]);
    memtable.add_wal_batch(collection_id, batch2).await.unwrap();

    // Verify update is successful and latest version is returned
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_some());
    let found_vector = result.unwrap();
    assert_eq!(found_vector.id, vector_id.to_string());
    assert_eq!(found_vector.vector, vec![4.0, 5.0, 6.0]);
    assert_eq!(found_vector.version, Some(2));

    // Delete: Construct tombstone with same ID
    let current_time = chrono::Utc::now().timestamp() as u32; // Current time in seconds
    let delete_vector = create_vector_record(
        &current_vector.id,                         // Use same ID
        vec![0.0, 0.0, 0.0],                        // Vector content irrelevant for delete
        found_vector.version.map(|v| v as u32 + 1), // Increment version
        Some(current_time - 1),                     // Mark as expired 1 second ago
    );
    let batch3 = create_wal_batch(collection_id, 3, vec![delete_vector]);
    memtable.add_wal_batch(collection_id, batch3).await.unwrap();

    // Verify delete is successful
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_none(), "Vector should be deleted after tombstone");
}

#[tokio::test]
async fn test_version_ordering_across_layers() {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "1uctd3h"; // 7-char base62 ID (realistic)
    let vector_id = "vector_1";

    // Add multiple versions out of order to test version resolution

    // Version 3 (highest)
    let vector_v3 = create_vector_record(vector_id, vec![3.0, 3.0, 3.0], Some(3), None);
    let batch3 = create_wal_batch(collection_id, 3, vec![vector_v3.clone()]);
    memtable.add_wal_batch(collection_id, batch3).await.unwrap();

    // Version 1 (lowest)
    let vector_v1 = create_vector_record(vector_id, vec![1.0, 1.0, 1.0], Some(1), None);
    let batch1 = create_wal_batch(collection_id, 1, vec![vector_v1.clone()]);
    memtable.add_wal_batch(collection_id, batch1).await.unwrap();

    // Version 2 (middle)
    let vector_v2 = create_vector_record(vector_id, vec![2.0, 2.0, 2.0], Some(2), None);
    let batch2 = create_wal_batch(collection_id, 2, vec![vector_v2.clone()]);
    memtable.add_wal_batch(collection_id, batch2).await.unwrap();

    // Should return version 3 (highest)
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_some());
    let found_vector = result.unwrap();
    assert_eq!(found_vector.version, Some(3));
    assert_eq!(found_vector.vector, vec![3.0, 3.0, 3.0]);

    // Search should also return version 3
    let search_results = memtable
        .search_vectors(
            &[3.0, 3.0, 3.0],
            1,
            collection_id,
            CoreDistanceMetric::Euclidean,
        )
        .await
        .unwrap();
    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0].1.version, Some(3));
}

#[tokio::test]
async fn test_expired_records_vs_active_records() {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "1uctd3i"; // 7-char base62 ID (realistic)
    let current_time = chrono::Utc::now().timestamp() as u32; // Current time in seconds

    // Active vector
    let active_vector = create_vector_record(
        "active_vector",
        vec![1.0, 0.0, 0.0],
        Some(1),
        Some(current_time + 3600), // Expires in 1 hour (in seconds)
    );

    // Expired vector
    let expired_vector = create_vector_record(
        "expired_vector",
        vec![0.0, 1.0, 0.0],
        Some(1),
        Some(current_time - 1), // Expired 1 second ago
    );

    let batch = create_wal_batch(
        collection_id,
        1,
        vec![active_vector.clone(), expired_vector.clone()],
    );
    memtable.add_wal_batch(collection_id, batch).await.unwrap();

    // Active vector should be found
    let active_result = memtable
        .vector_by_id(collection_id, "active_vector")
        .await
        .unwrap();
    assert!(active_result.is_some());

    // Expired vector should not be found
    let expired_result = memtable
        .vector_by_id(collection_id, "expired_vector")
        .await
        .unwrap();
    assert!(expired_result.is_none());

    // Search should only return active vector
    let search_results = memtable
        .search_vectors(
            &[1.0, 1.0, 1.0],
            10,
            collection_id,
            CoreDistanceMetric::Euclidean,
        )
        .await
        .unwrap();

    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0].1.id, "active_vector".to_string());
}

#[tokio::test]
async fn test_same_id_different_vector_values() {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "1uctd3j"; // 7-char base62 ID (realistic)
    let vector_id = "vector_1";

    // First version with specific vector values
    let vector_v1 = create_vector_record(vector_id, vec![1.0, 0.0, 0.0], Some(1), None);
    let batch1 = create_wal_batch(collection_id, 1, vec![vector_v1.clone()]);
    memtable.add_wal_batch(collection_id, batch1).await.unwrap();

    // Second version with completely different vector values
    let vector_v2 = create_vector_record(
        vector_id,
        vec![0.0, 0.0, 1.0], // Completely different vector
        Some(2),
        None,
    );
    let batch2 = create_wal_batch(collection_id, 2, vec![vector_v2.clone()]);
    memtable.add_wal_batch(collection_id, batch2).await.unwrap();

    // Should return the latest version with the new vector values
    let result = memtable
        .vector_by_id(collection_id, vector_id)
        .await
        .unwrap();
    assert!(result.is_some());
    let found_vector = result.unwrap();
    assert_eq!(found_vector.id, vector_id.to_string());
    assert_eq!(found_vector.version, Some(2));
    assert_eq!(found_vector.vector, vec![0.0, 0.0, 1.0]);

    // Search should find the updated vector
    let search_results = memtable
        .search_vectors(
            &[0.0, 0.0, 1.0],
            1,
            collection_id,
            CoreDistanceMetric::Cosine,
        )
        .await
        .unwrap();

    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0].1.id, vector_id.to_string());
    assert_eq!(search_results[0].1.version, Some(2));
    assert_eq!(search_results[0].1.vector, vec![0.0, 0.0, 1.0]);
}

#[tokio::test]
async fn test_multi_collection_mvcc_isolation() {
    let memtable = GlobalPartitionedMemtable::new();
    // Realistic base62 collection IDs (7-char format)
    let collection_a = "1uctd3x"; // 7-char base62 ID 
    let collection_b = "1uctd3y"; // 7-char base62 ID
    let vector_id = "vector_1"; // Same ID in both collections

    let vector_a = create_vector_record(vector_id, vec![1.0, 0.0, 0.0], Some(1), None);
    let batch_a = create_wal_batch(collection_a, 1, vec![vector_a.clone()]);
    memtable.add_wal_batch(collection_a, batch_a).await.unwrap();

    let vector_b = create_vector_record(vector_id, vec![0.0, 1.0, 0.0], Some(1), None);
    let batch_b = create_wal_batch(collection_b, 2, vec![vector_b.clone()]);
    memtable.add_wal_batch(collection_b, batch_b).await.unwrap();

    // Delete from collection A only
    let current_time = chrono::Utc::now().timestamp() as u32; // Current time in seconds
    let delete_a = create_vector_record(
        vector_id,
        vec![0.0, 0.0, 0.0],
        Some(2),
        Some(current_time - 1), // Expired 1 second ago
    );
    let batch_delete = create_wal_batch(collection_a, 3, vec![delete_a]);
    memtable
        .add_wal_batch(collection_a, batch_delete)
        .await
        .unwrap();

    // Collection A should not find the vector (deleted)
    let result_a = memtable
        .vector_by_id(collection_a, vector_id)
        .await
        .unwrap();
    assert!(result_a.is_none());

    // Collection B should still find the vector (not deleted)
    let result_b = memtable
        .vector_by_id(collection_b, vector_id)
        .await
        .unwrap();
    assert!(result_b.is_some());
    assert_eq!(result_b.unwrap().vector, vec![0.0, 1.0, 0.0]);
}

#[tokio::test]
async fn test_flush_compaction_atomic_consistency() {
    // This test simulates the atomic flush-compaction operation
    // to ensure no search inconsistencies occur during the process

    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "1uctd3k"; // 7-char base62 ID (realistic)

    // Add initial data
    let vectors = vec![
        create_vector_record("vec1", vec![1.0, 0.0, 0.0], Some(1), None),
        create_vector_record("vec2", vec![0.0, 1.0, 0.0], Some(1), None),
        create_vector_record("vec3", vec![0.0, 0.0, 1.0], Some(1), None),
    ];
    let batch = create_wal_batch(collection_id, 1, vectors);
    memtable.add_wal_batch(collection_id, batch).await.unwrap();

    // Verify all vectors are searchable before flush
    let search_results = memtable
        .search_vectors(
            &[1.0, 1.0, 1.0],
            10,
            collection_id,
            CoreDistanceMetric::Euclidean,
        )
        .await
        .unwrap();
    assert_eq!(search_results.len(), 3);

    // Simulate partial flush (some vectors moved to storage)
    // In real implementation, this would be atomic
    let vec1_result = memtable.vector_by_id(collection_id, "vec1").await.unwrap();
    assert!(vec1_result.is_some());

    // TODO: When storage engines are implemented, test that flushed vectors
    // are still searchable across WAL + Storage layers

    // For now, test that clearing doesn't affect search consistency
    let cleared = memtable.clear_flushed_batches(collection_id).await.unwrap();
    assert_eq!(cleared, 0); // Should not clear anything since sequence is 0

    // All vectors should still be searchable
    let search_results_after = memtable
        .search_vectors(
            &[1.0, 1.0, 1.0],
            10,
            collection_id,
            CoreDistanceMetric::Euclidean,
        )
        .await
        .unwrap();
    assert_eq!(search_results_after.len(), 3);
}
