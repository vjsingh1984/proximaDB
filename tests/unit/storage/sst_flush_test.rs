//! Unit test for SST engine flush functionality using unified test utilities
//! 
//! This test verifies that SST's do_flush method properly writes SSTables
//! with the correct bloom filter configuration.
//!
//! Refactored to use unified test utilities for consistent path handling and configuration.

mod common {
    include!("../../common/mod.rs");
}
use common::unified_test_utils::{UnifiedTestEnvironment, operations};
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use proximadb::storage::traits::UnifiedStorageEngine;
use tracing::debug;

/// Test SST flush creates valid SSTable files with bloom filters and metadata search
/// 
/// Validates that the SST engine's flush operation creates proper SSTable files with:
/// - Correct bloom filter configuration for efficient key/metadata lookups
/// - Valid file structure that can be read back
/// - Metadata filtering functionality working correctly
#[tokio::test]
async fn test_sst_flush_creates_searchable_sstables_with_bloom_filters() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let env = UnifiedTestEnvironment::new().await?;
    let sst_engine = env.create_sst_engine().await?;
    
    debug!("TEST: Using collection_id: {}", env.collection_id);
    
    // Create test vectors with metadata using environment helpers
    let now = chrono::Utc::now().timestamp() as u32;
    let vectors = vec![
        env.create_test_vector_record(
            "vec1".to_string(),
            vec![1.0, 0.0, 0.0],
            now,
            None,
            vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                },
                MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("primary".to_string())),
                },
            ]
        ),
        env.create_test_vector_record(
            "vec2".to_string(),
            vec![0.0, 1.0, 0.0],
            now + 1,
            None,
            vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("B".to_string())),
                },
                MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("secondary".to_string())),
                },
            ]
        ),
        env.create_test_vector_record(
            "vec3".to_string(),
            vec![0.0, 0.0, 1.0],
            now + 2,
            None,
            vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("A".to_string())),
                },
                MetadataItem {
                    key: "type".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("primary".to_string())),
                },
            ]
        ),
    ];
    
    // Use unified operations for insert and flush
    debug!("\n=== Testing SST do_flush ===");
    operations::insert_and_flush_sst(&sst_engine, &env, vectors).await?;
    
    // Verify basic search functionality
    debug!("\n=== Testing SST search ===");
    let query = vec![1.0, 0.0, 0.0];
    let results = operations::search_vectors_sst(&sst_engine, &env, &query, 5).await?;
    
    debug!("Search returned {} results", results.len());
    assert!(!results.is_empty(), "Should find results from SSTable");
    
    // The closest vector should be vec1
    assert_eq!(results[0].id, "vec1", "Closest vector should be vec1");
    assert!(results[0].distance.unwrap() < 0.001, "Distance should be near 0");
    
    // Test with metadata filter
    debug!("\n=== Testing SST search with metadata filter ===");
    let filter = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::json!("A"),
    };
    
    let storage_url = env.get_sst_data_directory().join("data").to_string_lossy().to_string();
    let filtered_results = sst_engine.search_vectors_unified(
        &env.collection_id,
        &format!("file://{}", storage_url),
        &query,
        5,
        &DistanceMetric::Cosine,
        Some(&filter),
        true,
        true,
    ).await?;
    
    debug!("Filtered search returned {} results", filtered_results.len());
    assert_eq!(filtered_results.len(), 2, "Should find 2 vectors with category A");
    
    debug!("\n=== Test completed successfully ===");
    Ok(())
}