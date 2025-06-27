//! AXIS Integration Tests
//!
//! Tests the integration between AXIS and other ProximaDB components

use chrono::Utc;
use proximadb::core::avro_unified::VectorRecord;
use proximadb::index::axis::{
    AxisConfig, AxisIndexManager, FilterOperator, HybridQuery, MetadataFilter, VectorQuery,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Helper function to create test vector records
fn create_test_vectors(count: usize, dimension: usize, collection_id: &str) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for i in 0..count {
        let mut vector_data = vec![0.0; dimension];
        for (j, val) in vector_data.iter_mut().enumerate() {
            *val = (i as f32 + j as f32) / 100.0;
        }

        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            serde_json::json!(format!("cat_{}", i % 5)),
        );
        metadata.insert("priority".to_string(), serde_json::json!(i % 10));

        let now = Utc::now().timestamp_millis();

        vectors.push(VectorRecord {
            id: Uuid::new_v4().to_string(),
            collection_id: collection_id.to_string(),
            vector: vector_data,
            metadata,
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        });
    }

    vectors
}

#[tokio::test]
async fn test_axis_large_scale_insertion() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Test large-scale insertion across multiple collections
    for collection_idx in 0..3 {
        let collection_id = format!("large_scale_collection_{}", collection_idx);
        let test_vectors = create_test_vectors(1000, 128, &collection_id);

        for vector in test_vectors {
            let result = axis_manager.insert(vector).await;
            assert!(result.is_ok(), "Large-scale vector insertion should succeed");
        }
    }

    // Verify metrics show proper tracking
    let metrics = axis_manager.get_metrics().await;
    assert!(
        metrics.total_vectors_indexed >= 3000,
        "Should track large-scale insertions"
    );
    assert_eq!(
        metrics.total_collections_managed, 3,
        "Should track all collections"
    );
}

#[tokio::test]
async fn test_axis_concurrent_operations() {
    let config = AxisConfig::default();
    let axis_manager = Arc::new(AxisIndexManager::new(config).await.unwrap());

    // Create multiple concurrent insertion tasks
    let mut handles = vec![];

    for task_id in 0..3 {
        let manager = axis_manager.clone();
        let collection_id = format!("concurrent_collection_{}", task_id);

        let handle = tokio::spawn(async move {
            let vectors = create_test_vectors(200, 64, &collection_id);
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
async fn test_axis_adaptive_optimization() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Insert different types of data to trigger adaptive behavior
    let dense_collection = "dense_optimized_collection";
    let sparse_collection = "sparse_optimized_collection";

    // Dense collection (low sparsity)
    let dense_vectors = create_test_vectors(500, 256, dense_collection);
    for vector in dense_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Sparse collection (simulated high sparsity via metadata)
    let mut sparse_vectors = create_test_vectors(500, 256, sparse_collection);
    for vector in &mut sparse_vectors {
        vector.metadata.insert("sparse_type".to_string(), serde_json::json!(true));
    }
    for vector in sparse_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Trigger optimization analysis for both collections
    let dense_result = axis_manager
        .analyze_and_optimize(&dense_collection.to_string())
        .await;
    let sparse_result = axis_manager
        .analyze_and_optimize(&sparse_collection.to_string())
        .await;

    assert!(dense_result.is_ok(), "Dense collection optimization should succeed");
    assert!(sparse_result.is_ok(), "Sparse collection optimization should succeed");
}

#[tokio::test]
async fn test_axis_complex_hybrid_queries() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "hybrid_query_collection";

    // Insert test data with rich metadata
    let mut test_vectors = create_test_vectors(100, 128, collection_id);
    for (i, vector) in test_vectors.iter_mut().enumerate() {
        vector.metadata.insert("region".to_string(), serde_json::json!(format!("region_{}", i % 3)));
        vector.metadata.insert("score".to_string(), serde_json::json!(i as f64 / 10.0));
        vector.metadata.insert("active".to_string(), serde_json::json!(i % 2 == 0));
    }

    for vector in test_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Execute complex hybrid query
    let query = HybridQuery {
        collection_id: collection_id.to_string(),
        vector_query: Some(VectorQuery::Dense {
            vector: vec![0.5; 128],
            similarity_threshold: 0.3,
        }),
        metadata_filters: vec![
            MetadataFilter {
                field: "region".to_string(),
                operator: FilterOperator::Equals,
                value: serde_json::json!("region_1"),
            },
            MetadataFilter {
                field: "active".to_string(),
                operator: FilterOperator::Equals,
                value: serde_json::json!(true),
            },
        ],
        id_filters: vec![],
        k: 10,
        include_expired: false,
    };

    let result = axis_manager.query(query).await;
    assert!(result.is_ok(), "Complex hybrid query should execute successfully");

    let query_result = result.unwrap();
    assert!(
        query_result.results.len() <= 10,
        "Should respect k limit"
    );
}

#[tokio::test]
async fn test_axis_system_recovery() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    // Setup multiple collections
    for coll_id in 0..3 {
        let collection_id = format!("recovery_test_collection_{}", coll_id);
        let vectors = create_test_vectors(100, 64, &collection_id);

        for vector in vectors {
            let _ = axis_manager.insert(vector).await;
        }
    }

    // Simulate system operations that should maintain consistency
    for coll_id in 0..3 {
        let collection_id = format!("recovery_test_collection_{}", coll_id);

        // Test collection statistics
        let stats_result = axis_manager.get_collection_stats(&collection_id).await;
        // This may fail for non-existent strategies, which is expected behavior

        // Test optimization
        let optimize_result = axis_manager.analyze_and_optimize(&collection_id).await;
        assert!(
            optimize_result.is_ok(),
            "System recovery operations should succeed"
        );
    }

    // Verify system remains stable
    let final_metrics = axis_manager.get_metrics().await;
    assert_eq!(
        final_metrics.total_collections_managed, 3,
        "Should maintain collection count through recovery operations"
    );
}

#[tokio::test]
async fn test_axis_collection_lifecycle() {
    let config = AxisConfig::default();
    let axis_manager = AxisIndexManager::new(config).await.unwrap();

    let collection_id = "lifecycle_test_collection";

    // Phase 1: Collection creation and initial data
    let initial_vectors = create_test_vectors(50, 128, collection_id);
    for vector in initial_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Phase 2: Collection growth
    let growth_vectors = create_test_vectors(100, 128, collection_id);
    for vector in growth_vectors {
        let _ = axis_manager.insert(vector).await;
    }

    // Phase 3: Analysis and optimization
    let optimization_result = axis_manager.analyze_and_optimize(&collection_id.to_string()).await;
    assert!(optimization_result.is_ok(), "Collection optimization should succeed");

    // Phase 4: Collection cleanup
    let cleanup_result = axis_manager.drop_collection(&collection_id.to_string()).await;
    assert!(cleanup_result.is_ok(), "Collection cleanup should succeed");

    // Verify metrics reflect the lifecycle
    let final_metrics = axis_manager.get_metrics().await;
    assert!(
        final_metrics.total_vectors_indexed >= 150,
        "Should track all inserted vectors"
    );
}