// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Integration tests for AXIS (Adaptive eXtensible Indexing System)

use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use proximadb::core::{VectorId, VectorRecord};
use proximadb::index::axis::{
    AxisConfig, AxisIndexManager, CollectionCharacteristics, IndexStrategy, IndexType,
    MigrationDecision,
};

/// Helper function to create test vector records
fn create_test_vectors(count: usize, dimension: usize, sparsity: f32) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for i in 0..count {
        let mut vector_data = vec![0.0; dimension];

        // Create sparse or dense vectors based on sparsity parameter
        let non_zero_count = ((dimension as f32) * (1.0 - sparsity)) as usize;
        for j in 0..non_zero_count {
            let idx = (j * dimension / non_zero_count) % dimension;
            vector_data[idx] = (i as f32 + j as f32) / 100.0;
        }

        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            serde_json::json!(format!("cat_{}", i % 5)),
        );
        metadata.insert("priority".to_string(), serde_json::json!(i % 10));

        vectors.push(VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "test_collection".to_string(),
            vector: vector_data,
            metadata,
            timestamp: Utc::now(),
            expires_at: None, // No expiration for test vectors
        });
    }

    vectors
}

/// Helper function to create expired test vectors (for MVCC testing)
fn create_expired_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for i in 0..count {
        let vector_data = vec![1.0; dimension];
        let mut metadata = HashMap::new();
        metadata.insert("expired".to_string(), serde_json::json!(true));

        vectors.push(VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "test_collection".to_string(),
            vector: vector_data,
            metadata,
            timestamp: Utc::now(),
            expires_at: Some(Utc::now() - Duration::hours(1)), // Expired 1 hour ago
        });
    }

    vectors
}

#[tokio::test]
async fn test_axis_manager_creation() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await;

    assert!(
        axis_manager.is_ok(),
        "AXIS manager should be created successfully"
    );
}

#[tokio::test]
async fn test_vector_insertion_with_mvcc() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Test regular vector insertion
    let test_vectors = create_test_vectors(100, 128, 0.3);

    for vector in test_vectors {
        let result = axis_manager.insert(vector).await;
        assert!(result.is_ok(), "Vector insertion should succeed");
    }

    // Test expired vector insertion (should be skipped)
    let expired_vectors = create_expired_vectors(10, 128);

    for vector in expired_vectors {
        let result = axis_manager.insert(vector).await;
        assert!(
            result.is_ok(),
            "Expired vector insertion should succeed but be skipped"
        );
    }
}

#[tokio::test]
async fn test_adaptive_strategy_selection() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Insert different types of vector collections to trigger strategy adaptation

    // Small dense collection (should use LightweightHNSW)
    let small_dense_vectors = create_test_vectors(1000, 64, 0.1);
    for vector in small_dense_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Large sparse collection (should use different strategy)
    let mut large_sparse_vectors = create_test_vectors(15000, 512, 0.8);
    // Change collection ID to trigger different strategy
    for vector in &mut large_sparse_vectors {
        vector.collection_id = "large_sparse_collection".to_string();
    }

    for vector in large_sparse_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // The system should automatically select appropriate strategies
    // This test verifies the system doesn't crash and processes different data patterns
}

#[tokio::test]
async fn test_collection_analysis_and_optimization() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "analysis_test_collection";

    // Insert vectors for analysis
    let test_vectors = create_test_vectors(5000, 256, 0.5);
    for mut vector in test_vectors {
        vector.collection_id = collection_id.to_string();
        let _ = axis_manager.insert(vector).await;
    }

    // Trigger analysis and optimization
    let result = axis_manager
        .analyze_and_optimize(&collection_id.to_string())
        .await;
    assert!(result.is_ok(), "Collection analysis should succeed");
}

#[tokio::test]
async fn test_mvcc_filtering_during_query() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "mvcc_test_collection";

    // Insert active vectors
    let mut active_vectors = create_test_vectors(100, 128, 0.3);
    for vector in &mut active_vectors {
        vector.collection_id = collection_id.to_string();
    }

    for vector in active_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Insert expired vectors
    let mut expired_vectors = create_expired_vectors(50, 128);
    for vector in &mut expired_vectors {
        vector.collection_id = collection_id.to_string();
    }

    for vector in expired_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Create a hybrid query
    use proximadb::index::axis::manager::{
        FilterOperator, HybridQuery, MetadataFilter, VectorQuery,
    };

    let query = HybridQuery {
        collection_id: collection_id.to_string(),
        vector_query: Some(VectorQuery::Dense {
            vector: vec![0.5; 128],
            similarity_threshold: 0.7,
        }),
        metadata_filters: vec![],
        id_filters: vec![],
        k: 10,
        include_expired: false, // Should filter out expired vectors
    };

    let result = axis_manager.query(query).await;
    assert!(result.is_ok(), "Query with MVCC filtering should succeed");

    // Results should not include expired vectors
    let query_result = result.unwrap();
    for result in &query_result.results {
        assert!(
            result.expires_at.is_none() || result.expires_at.unwrap() > Utc::now(),
            "Query results should not include expired vectors"
        );
    }
}

#[tokio::test]
async fn test_migration_decision_making() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "migration_test_collection";

    // Insert initial vectors
    let mut test_vectors = create_test_vectors(8000, 384, 0.2);
    for vector in &mut test_vectors {
        vector.collection_id = collection_id.to_string();
    }

    for vector in test_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // The system should automatically evaluate migration possibilities
    // This test ensures the migration logic doesn't cause crashes
    let result = axis_manager
        .analyze_and_optimize(&collection_id.to_string())
        .await;
    assert!(
        result.is_ok(),
        "Migration analysis should complete without errors"
    );
}

#[tokio::test]
async fn test_performance_metrics_collection() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Insert vectors and verify metrics are collected
    let test_vectors = create_test_vectors(1000, 128, 0.4);

    for vector in test_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Get metrics
    let metrics = axis_manager.get_metrics().await;

    // Verify metrics are being tracked
    assert!(
        metrics.total_vectors_indexed > 0,
        "Should track vector insertions"
    );
    assert!(
        metrics.total_collections_managed > 0,
        "Should track collections"
    );
}

#[tokio::test]
async fn test_vector_deletion_with_soft_deletes() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "delete_test_collection";

    // Insert test vectors
    let mut test_vectors = create_test_vectors(100, 128, 0.3);
    for vector in &mut test_vectors {
        vector.collection_id = collection_id.to_string();
    }

    for vector in test_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Delete some vectors (soft delete)
    for i in 0..50 {
        let result = axis_manager
            .delete(&collection_id.to_string(), Uuid::new_v4())
            .await;
        assert!(result.is_ok(), "Vector deletion should succeed");
    }

    // Verify deletion doesn't crash the system
    // In a real implementation, we would verify the vectors are marked as deleted
    // but still present in storage with expires_at set to past timestamp
}

#[tokio::test]
async fn test_mixed_vector_types() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "mixed_test_collection";

    // Insert dense vectors
    let mut dense_vectors = create_test_vectors(1000, 256, 0.1); // 10% sparsity
    for vector in &mut dense_vectors {
        vector.collection_id = collection_id.to_string();
    }

    // Insert sparse vectors
    let mut sparse_vectors = create_test_vectors(1000, 256, 0.9); // 90% sparsity
    for (i, vector) in sparse_vectors.iter_mut().enumerate() {
        vector.collection_id = collection_id.to_string();
        vector.id = Uuid::new_v4(); // Different ID range
    }

    // Insert all vectors
    for vector in dense_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    for vector in sparse_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // System should handle mixed sparsity patterns
    let result = axis_manager
        .analyze_and_optimize(&collection_id.to_string())
        .await;
    assert!(result.is_ok(), "Mixed vector analysis should succeed");
}

#[tokio::test]
async fn test_concurrent_operations() {
    let config = AxisConfig::default();
    let axis_manager = Arc::new(AxisIndexManager::new(config).await.unwrap());

    let collection_id = "concurrent_test_collection";

    // Create multiple tasks for concurrent insertions
    let mut handles = vec![];

    for task_id in 0..5 {
        let manager = axis_manager.clone();
        let coll_id = format!("{}_{}", collection_id, task_id);

        let handle = tokio::spawn(async move {
            let mut vectors = create_test_vectors(200, 128, 0.3);
            for vector in &mut vectors {
                vector.collection_id = coll_id.clone();
            }

            for vector in vectors {
                let _ = manager.insert(vector).await;
            }
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        let result = handle.await;
        assert!(
            result.is_ok(),
            "Concurrent insertion task should complete successfully"
        );
    }

    // Verify system stability after concurrent operations
    let metrics = axis_manager.get_metrics().await;
    assert!(
        metrics.total_vectors_indexed > 0,
        "Should have processed vectors from concurrent tasks"
    );
}

#[tokio::test]
async fn test_ttl_vector_lifecycle() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "ttl_test_collection";

    // Create vectors with different TTL scenarios
    let mut vectors = Vec::new();

    // Vector that never expires
    vectors.push(VectorRecord {
        id: Uuid::new_v4(),
        collection_id: collection_id.to_string(),
        vector: vec![1.0; 128],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
        expires_at: None,
    });

    // Vector that expires in the future
    vectors.push(VectorRecord {
        id: Uuid::new_v4(),
        collection_id: collection_id.to_string(),
        vector: vec![2.0; 128],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
        expires_at: Some(Utc::now() + Duration::hours(1)),
    });

    // Vector that is already expired
    vectors.push(VectorRecord {
        id: Uuid::new_v4(),
        collection_id: collection_id.to_string(),
        vector: vec![3.0; 128],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
        expires_at: Some(Utc::now() - Duration::hours(1)),
    });

    // Insert all vectors
    for vector in vectors {
        let result = axis_manager.insert(vector).await;
        assert!(result.is_ok(), "TTL vector insertion should succeed");
    }

    // The expired vector should be automatically skipped during insertion
    // and the system should handle TTL logic correctly
}

#[tokio::test]
async fn test_system_recovery_and_consistency() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Insert vectors across multiple collections
    for coll_id in 0..3 {
        let collection_id = format!("recovery_test_collection_{}", coll_id);

        let mut vectors = create_test_vectors(500, 128, 0.4);
        for vector in &mut vectors {
            vector.collection_id = collection_id.clone();
        }

        for vector in vectors {
            let _ = axis_manager.insert(vector).await;
        }
    }

    // Trigger analysis for all collections to ensure system consistency
    for coll_id in 0..3 {
        let collection_id = format!("recovery_test_collection_{}", coll_id);
        let result = axis_manager.analyze_and_optimize(&collection_id).await;
        assert!(
            result.is_ok(),
            "Recovery analysis should succeed for all collections"
        );
    }

    // Verify system metrics remain consistent
    let metrics = axis_manager.get_metrics().await;
    assert_eq!(
        metrics.total_collections_managed, 3,
        "Should track all collections"
    );
}
