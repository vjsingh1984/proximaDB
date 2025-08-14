//! Core SST functionality tests using unified test utilities
//!
//! Refactored to use unified test utilities for consistent path handling and configuration.

use common::unified_test_utils::{UnifiedTestEnvironment, operations};
use proximadb::core::VectorRecord;
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::distance_computation::DistanceMetric;
use std::sync::Arc;
use tracing::{debug, info};

/// Test SST engine insert, flush, and search with metadata filtering
/// 
/// Validates core SST functionality: vector insertion, memtable flush to SST files,
/// similarity search, and metadata-based filtering using unified test utilities.
#[tokio::test]
async fn test_sst_insert_flush_search_with_metadata() -> anyhow::Result<()> {
    let env = UnifiedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create test vectors with metadata
    let vectors = vec![
        env.create_test_vector_record(
            "vec1".to_string(),
            vec![1.0, 0.0, 0.0],
            1000,
            None,
            vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                }
            ]
        ),
        env.create_test_vector_record(
            "vec2".to_string(),
            vec![0.0, 1.0, 0.0],
            1001,
            None,
            vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("B".to_string())),
                }
            ]
        ),
        env.create_test_vector_record(
            "vec3".to_string(),
            vec![0.0, 0.0, 1.0],
            1002,
            None,
            vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                }
            ]
        ),
    ];
    
    // Insert and flush using unified operations
    operations::insert_and_flush_sst(&engine, &env, vectors).await?;
    
    // Search without filters - query closest to vec1
    let results = operations::search_vectors_sst(&engine, &env, &vec![1.0, 0.0, 0.0], 3).await?;
    
    debug!("Search results count: {}", results.len());
    assert!(!results.is_empty(), "Should find results");
    assert_eq!(results[0].id, "vec1", "First result should be vec1");
    
    // Search with metadata filter
    let filter = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("A".to_string()),
    };
    
    let storage_url = env.get_sst_data_directory().join("data").to_string_lossy().to_string();
    let filtered_results = engine.search_vectors_unified(
        &env.collection_id,
        &format!("file://{}", storage_url),
        &vec![0.0, 1.0, 0.0],  // Query closest to vec2 (category B)
        3,
        &DistanceMetric::Cosine,
        Some(&filter),
        true,
        true,
    ).await?;
    
    // Should only return vec1 and vec3 (category A)
    assert_eq!(filtered_results.len(), 2, "Should find 2 category A vectors");
    for result in &filtered_results {
        assert!(result.id == "vec1" || result.id == "vec3", 
                "Results should only be category A vectors");
    }
    
    Ok(())
}

/// Test SST multi-batch compaction and search consistency
/// 
/// Validates that multiple flush operations create separate SST files that can be 
/// compacted together while maintaining search consistency across all vectors.
#[tokio::test]
async fn test_sst_multi_batch_compaction_consistency() -> anyhow::Result<()> {
    let env = UnifiedTestEnvironment::new().await?;
    let mut engine = env.create_sst_engine().await?;
    
    // Create multiple flushes to trigger compaction
    for batch in 0..3 {
        let vectors: Vec<_> = (0..5).map(|i| {
            env.create_test_vector_record(
                format!("batch{}_vec{}", batch, i),
                vec![batch as f32, i as f32, 0.0],
                1000 + batch * 5 + i,
                None,
                vec![]
            )
        }).collect();
        
        operations::insert_and_flush_sst(&engine, &env, vectors).await?;
    }
    
    // Verify all vectors are searchable
    let all_results = operations::search_vectors_sst(&engine, &env, &vec![1.0, 1.0, 0.0], 15).await?;
    assert_eq!(all_results.len(), 15, "Should find all 15 vectors");
    
    // Perform compaction
    operations::compact_sst_storage(&mut engine, &env).await?;
    
    // Verify vectors are still searchable after compaction
    let post_compact_results = operations::search_vectors_sst(&engine, &env, &vec![1.0, 1.0, 0.0], 15).await?;
    assert_eq!(post_compact_results.len(), 15, "Should find all 15 vectors after compaction");
    
    Ok(())
}


/// Test SST persistence and recovery across engine restarts
/// 
/// Validates that vectors flushed to SST files persist across engine restarts
/// and can be searched consistently after recovery.
#[tokio::test]
async fn test_sst_persistence_across_restarts() -> anyhow::Result<()> {
    let env = UnifiedTestEnvironment::new().await?;
    
    // Phase 1: Write data
    {
        let engine = env.create_sst_engine().await?;
        
        let vectors = vec![
            env.create_test_vector_record(
                "persist1".to_string(),
                vec![1.0, 2.0, 3.0],
                1000,
                None,
                vec![]
            ),
            env.create_test_vector_record(
                "persist2".to_string(),
                vec![4.0, 5.0, 6.0],
                1001,
                None,
                vec![]
            ),
        ];
        
        operations::insert_and_flush_sst(&engine, &env, vectors).await?;
    }
    
    // Phase 2: Create new engine and verify data persisted
    {
        let engine = env.create_sst_engine().await?;
        
        // Search for persisted vectors
        let results = operations::search_vectors_sst(&engine, &env, &vec![1.0, 2.0, 3.0], 2).await?;
        
        debug!("Search returned {} results", results.len());
        for (i, result) in results.iter().enumerate() {
            debug!("  Result {}: id={}, distance={:?}", i, result.id, result.distance);
        }
        
        assert_eq!(results.len(), 2, "Should find both persisted vectors");
        
        // Verify exact vector retrieval
        let vec_ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(vec_ids.contains(&"persist1"), "Should find persist1");
        assert!(vec_ids.contains(&"persist2"), "Should find persist2");
    }
    
    Ok(())
}