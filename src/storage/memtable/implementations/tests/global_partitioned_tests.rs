//! Tests for GlobalPartitionedMemtable using modern WALVectorBatch architecture
//!
//! These tests verify the batch-oriented operations without legacy WalEntry

use super::super::global_partitioned::GlobalPartitionedMemtable;
use crate::compute::distance_computation::DistanceMetric as CoreDistanceMetric;
use crate::core::VectorRecord;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::write_ahead_log::BatchId;
use std::sync::Arc;

#[tokio::test]
async fn test_global_partitioned_batch_operations() {
    let memtable = GlobalPartitionedMemtable::new();

    // Create test vector records
    let now = chrono::Utc::now().timestamp_millis();
    let vector_record1 = VectorRecord {
        id: Some("test_vector_1".to_string()),
        vector: vec![0.1, 0.2, 0.3],
        metadata: vec![],
        timestamp: now as u32,
        updated_at: Some(now as u32),
        expires_at: None,
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    let vector_record2 = VectorRecord {
        id: Some("test_vector_2".to_string()),
        vector: vec![0.4, 0.5, 0.6],
        metadata: vec![],
        timestamp: now as u32,
        updated_at: Some(now as u32),
        expires_at: None,
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    // Create a batch with multiple vectors
    let batch = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![vector_record1.clone(), vector_record2.clone()]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 1024, // Approximate
        is_flushed: false,
            metadata_bloom_filter: None,
    };

    // Test batch insertion with realistic base62 collection ID
    let collection_id = "1uctd3b"; // 7-char base62 ID (realistic)
    let sequences = memtable.add_wal_batch(collection_id, batch).await.unwrap();
    assert_eq!(sequences.len(), 2);
    assert_eq!(sequences[0], 1);
    assert_eq!(sequences[1], 2);

    // Test collection statistics
    let (vector_count, size) = memtable.get_collection_stats(collection_id).await;
    assert_eq!(vector_count, 2);
    assert!(size > 0);

    // Test vector search
    let query_vector = vec![0.1, 0.2, 0.3];
    let results = memtable
        .search_vectors(&query_vector, 5, collection_id, CoreDistanceMetric::Cosine)
        .await
        .unwrap();
    
    assert!(!results.is_empty());
    assert_eq!(results[0].1.id, Some("test_vector_1".to_string())); // Should match the first vector
}

#[tokio::test]
async fn test_global_partitioned_multi_collection() {
    let memtable = GlobalPartitionedMemtable::new();

    // Create batches for different collections
    let now = chrono::Utc::now().timestamp_millis();
    
    // Collection A batch
    let batch_a = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![VectorRecord {
            id: Some("vec_a1".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![],
            timestamp: now as u32,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            // rank removed -  None,
            similarity: None,
            similarity: None,
        
        }]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 512,
        is_flushed: false,
        metadata_bloom_filter: None,
    };

    // Collection B batch
    let batch_b = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![VectorRecord {
            id: Some("vec_b1".to_string()),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![],
            timestamp: now as u32,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            // rank removed -  None,
            similarity: None,
            similarity: None,
        
        }]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 512,
        is_flushed: false,
        metadata_bloom_filter: None,
    };

    // Insert batches into different collections (realistic base62 IDs)
    let collection_a = "1uctd3a"; // 7-char base62 ID
    let collection_b = "1uctd3b"; // 7-char base62 ID
    let _seq_a = memtable.add_wal_batch(collection_a, batch_a).await.unwrap();
    let _seq_b = memtable.add_wal_batch(collection_b, batch_b).await.unwrap();

    // Verify isolation between collections
    let (count_a, _) = memtable.get_collection_stats(collection_a).await;
    let (count_b, _) = memtable.get_collection_stats(collection_b).await;
    assert_eq!(count_a, 1);
    assert_eq!(count_b, 1);

    // Search should only find vectors in the specified collection
    let query = vec![1.0, 1.0, 1.0];
    let results_a = memtable
        .search_vectors(&query, 10, collection_a, CoreDistanceMetric::Euclidean)
        .await
        .unwrap();
    let results_b = memtable
        .search_vectors(&query, 10, collection_b, CoreDistanceMetric::Euclidean)
        .await
        .unwrap();

    assert_eq!(results_a.len(), 1);
    assert_eq!(results_b.len(), 1);
    assert_eq!(results_a[0].1.id, Some("vec_a1".to_string()));
    assert_eq!(results_b[0].1.id, Some("vec_b1".to_string()));
}

#[tokio::test]
async fn test_mvcc_and_logical_deletes() {
    let memtable = GlobalPartitionedMemtable::new();
    let now = chrono::Utc::now().timestamp() as u32; // Current time in seconds

    // Version 1: Insert initial vector
    let vector_v1 = VectorRecord {
        id: Some("test_vector".to_string()),
        vector: vec![1.0, 0.0, 0.0],
        metadata: vec![],
        timestamp: now,
        updated_at: Some(now),
        expires_at: None,
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    // Version 2: Update vector with new data
    let vector_v2 = VectorRecord {
        id: Some("test_vector".to_string()),
        vector: vec![0.0, 1.0, 0.0],
        metadata: vec![],
        timestamp: now,
        updated_at: Some(now),
        expires_at: None,
        version: Some(2), // Higher version
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    // Version 3: Logical delete (expires_at in past)
    let vector_v3_delete = VectorRecord {
        id: Some("test_vector".to_string()),
        vector: vec![0.0, 0.0, 1.0], // Doesn't matter for deletes
        metadata: vec![],
        timestamp: now,
        updated_at: Some(now),
        expires_at: Some(now - 1), // Expired 1 second ago
        version: Some(3), // Highest version (delete)
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    // Insert all versions in separate batches
    let batch1 = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![vector_v1]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 512,
        is_flushed: false,
            metadata_bloom_filter: None,
    };

    let batch2 = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![vector_v2]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 512,
        is_flushed: false,
            metadata_bloom_filter: None,
    };

    let batch3 = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![vector_v3_delete]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 512,
        is_flushed: false,
            metadata_bloom_filter: None,
    };

    // Add batches
    let collection_id = "1uctd3d"; // 7-char base62 ID (realistic)
    let _seq1 = memtable.add_wal_batch(collection_id, batch1).await.unwrap();
    let _seq2 = memtable.add_wal_batch(collection_id, batch2).await.unwrap();
    let _seq3 = memtable.add_wal_batch(collection_id, batch3).await.unwrap();

    // Test get_vector_by_id - should return None due to logical delete
    let result = memtable.get_vector_by_id(collection_id, "test_vector").await.unwrap();
    assert!(result.is_none(), "Vector should be logically deleted");

    // Test search - should not find the vector
    let search_results = memtable
        .search_vectors(&[1.0, 1.0, 1.0], 10, collection_id, CoreDistanceMetric::Euclidean)
        .await
        .unwrap();
    
    // Should not find any results with that ID
    assert!(!search_results.iter().any(|(_, record)| record.id == Some("test_vector".to_string())));

    // Test get_all_vectors - should not include the deleted vector
    let all_vectors = memtable.get_collection_vectors(collection_id).await.unwrap();
    assert!(!all_vectors.iter().any(|record| record.id == Some("test_vector".to_string())));
}

#[tokio::test]
async fn test_global_partitioned_deletion_via_expiry() {
    let memtable = GlobalPartitionedMemtable::new();
    let now = chrono::Utc::now().timestamp() as u32; // Current time in seconds

    // Create a vector that's already expired (for deletion)
    let expired_vector = VectorRecord {
        id: Some("expired_vec".to_string()),
        vector: vec![1.0, 2.0, 3.0],
        metadata: vec![],
        timestamp: now,
        updated_at: Some(now),
        expires_at: Some(now - 1), // Expired 1 second ago
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    // Create a valid vector
    let valid_vector = VectorRecord {
        id: Some("valid_vec".to_string()),
        vector: vec![4.0, 5.0, 6.0],
        metadata: vec![],
        timestamp: now,
        updated_at: Some(now),
        expires_at: Some(now + 3600), // Expires in 1 hour (in seconds)
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        };

    let batch = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![expired_vector, valid_vector]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 1024,
        is_flushed: false,
            metadata_bloom_filter: None,
    };

    let collection_id = "1uctd3e"; // 7-char base62 ID (realistic)
    let _sequences = memtable.add_wal_batch(collection_id, batch).await.unwrap();

    // Get all vectors (should filter out expired ones)
    let all_vectors = memtable.get_collection_vectors(collection_id).await.unwrap();
    assert_eq!(all_vectors.len(), 1);
    assert_eq!(all_vectors[0].id, Some("valid_vec".to_string()));

    // Search should also filter out expired vectors
    let search_results = memtable
        .search_vectors(&[1.0, 1.0, 1.0], 10, collection_id, CoreDistanceMetric::Euclidean)
        .await
        .unwrap();
    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0].1.id, Some("valid_vec".to_string()));
}

#[tokio::test]
async fn test_global_partitioned_clear_operations() {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Add some test data
    let batch = WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vec![
            create_test_vector("vec1", "test_collection", vec![1.0, 0.0]),
            create_test_vector("vec2", "test_collection", vec![0.0, 1.0]),
            create_test_vector("vec3", "test_collection", vec![1.0, 1.0]),
        ]),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: 1536,
        is_flushed: false,
            metadata_bloom_filter: None,
    };

    let sequences = memtable.add_wal_batch("test_collection", batch).await.unwrap();
    assert_eq!(sequences.len(), 3);

    // Mark batches as flushed (simulate successful storage engine flush)
    // In real usage, this would be done after storage engine confirms flush
    memtable.mark_all_batches_flushed("test_collection").await.unwrap();

    // Test clear flushed batches
    let cleared = memtable.clear_flushed_batches("test_collection").await.unwrap();
    assert_eq!(cleared, 3); // Should clear all 3 vectors from the flushed batch

    // Verify collection is now empty
    let (count, _) = memtable.get_collection_stats("test_collection").await;
    assert_eq!(count, 0);
}

// Helper function to create test vectors
fn create_test_vector(id: &str, collection_id: &str, vector: Vec<f32>) -> VectorRecord {
    let now = chrono::Utc::now().timestamp_millis();
    VectorRecord {
        id: Some(id.to_string()),
        vector,
        metadata: vec![],
        timestamp: now as u32,
        updated_at: Some(now as u32),
        expires_at: None,
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    
        }
}