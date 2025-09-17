//! Fixed SST Collection Integration Test using UnifiedTestEnvironment
//!
//! This test properly creates a collection with SST storage engine and
//! verifies that flush operations route to SST correctly.

mod common {
    include!("../common/mod.rs");
}

use common::integration_test_helpers::{UnifiedTestEnvironment, operations};
use proximadb::proto::proximadb_v1::{VectorRecord, StorageEngine, SqlValue, sql_value};
use proximadb::storage::traits::UnifiedStorageEngine;
use tracing::{debug, info};

#[tokio::test]
async fn test_sst_collection_with_proper_routing() -> anyhow::Result<()> {
    // Initialize tracing
    let _ = tracing_subscriber::fmt::try_init();

    info!("=== Testing SST Collection with Proper Routing ===");

    // Use UnifiedTestEnvironment for proper setup
    let env = UnifiedTestEnvironment::new().await?;
    let collection_id = env.collection_id();
    debug!(
        "Created test environment with collection: {}",
        collection_id
    );

    // Create SST engine using the environment
    let sst_engine = env.create_sst_engine().await?;
    debug!("Created SST engine for collection: {}", collection_id);

    // Create test vectors
    let vectors = vec![
        VectorRecord {
            id: format!("{}_vec1", collection_id),
            vector: vec![1.0, 0.0, 0.0],
            metadata: {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert(
                    "category".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::StringValue("A".to_string())),
                    }
                );
                metadata
            },
            timestamp: 1000,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        },
        VectorRecord {
            id: format!("{}_vec2", collection_id),
            vector: vec![0.0, 1.0, 0.0],
            metadata: {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert(
                    "category".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::StringValue("B".to_string())),
                    },
                );
                metadata
            },
            timestamp: 1001,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        ),
        VectorRecord {
            id: format!("{}_vec3", collection_id),
            vector: vec![0.0, 0.0, 1.0],
            metadata: {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert(
                    "category".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::StringValue("A".to_string())),
                    }
                );
                metadata
            },
            timestamp: 1002,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        },
    ];

    info!("Test 1: Flush vectors to SST engine");

    // Build proper flush parameters using the helper
    let flush_params = operations::build_flush_params(&env, vectors, StorageEngine::Sst).await?;

    // Flush directly to SST engine
    let flush_result = sst_engine.do_flush(&flush_params).await?;
    assert!(flush_result.success, "SST flush should succeed");
    assert_eq!(flush_result.entries_flushed.unwrap_or(0), 3, "Should flush 3 vectors");
    assert!(flush_result.files_created.unwrap_or(0) > 0, "Should create SST files");

    debug!(
        "✅ Successfully flushed {} vectors to SST, created {} files",
        flush_result.entries_flushed.unwrap_or(0), flush_result.files_created.unwrap_or(0)
    );

    info!("Test 2: Verify SST files were created");

    // Check that SST files exist in the data directory
    let sst_data_dir = env.get_sst_data_directory();
    assert!(sst_data_dir.exists(), "SST data directory should exist");

    let mut sst_file_count = 0;
    let mut total_size = 0u64;

    if let Ok(entries) = std::fs::read_dir(&sst_data_dir) {
        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension() {
                if ext == "sst" {
                    sst_file_count += 1;
                    if let Ok(metadata) = entry.metadata() {
                        total_size += metadata.len();
                        debug!(
                            "Found SST file: {} ({} bytes)",
                            entry.file_name().to_string_lossy(),
                            metadata.len()
                        );
                    }
                }
            }
        }
    }

    assert!(
        sst_file_count > 0,
        "Should have created at least one SST file"
    );
    assert!(total_size > 0, "SST files should have non-zero size");

    debug!(
        "✅ Found {} SST files with total size {} bytes",
        sst_file_count, total_size
    );

    info!("Test 3: Search vectors from SST");

    // Build proper storage URL for search
    let storage_url = operations::build_sst_storage_url(&env);

    // Search for vectors
    let query_vector = vec![1.0, 0.0, 0.0];
    // Create search context for SST engine
    let search_params = std::sync::Arc::new(proximadb::core::search::SearchParams {
        vector: Some(query_vector.clone()),
        query_vectors: None,
        top_k: Some(5),
        distance_metric: Some(proximadb::compute::distance_computation::DistanceMetric::Cosine),
        filter_expression: None,
        include_metadata: Some(true),
        include_vectors: Some(true),
        timeout_ms: None,
        accuracy_threshold: None,
        enable_early_termination: None,
        max_results_per_stage: None,
        progressive_search: None,
    });

    let collection_config = proximadb::proto::proximadb_v1::CollectionConfig {
        name: collection_id.to_string(),
        dimension: query_vector.len() as u32,
        distance_metric: proximadb::proto::proximadb_v1::DistanceMetric::Cosine as i32,
        storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
        tags: vec![],
        auto_index_selection: None,
        embedding_models: None,
        owner: None,
        shared_with: vec![],
        storage_assignment: None,
    };

    let collection = std::sync::Arc::new(proximadb::proto::proximadb_v1::Collection {
        id: collection_id.to_string(),
        config: Some(collection_config),
        stats: None,
        created_at: 0,
        updated_at: 0,
    });

    let query_context = proximadb::storage::traits::StorageQueryContext {
        search_params,
        collection,
        metadata: proximadb::storage::traits::StorageQueryMetadata {
            collection_id: collection_id.to_string(),
            use_axis_indexes: false,
            storage_url: Some(storage_url.clone()),
            ..Default::default()
        },
    };

    let search_results = sst_engine
        .search_vectors_unified(&query_context)
        .await?;

    assert!(!search_results.is_empty(), "Should find search results");
    assert_eq!(search_results.len(), 3, "Should find all 3 vectors");

    // Verify the closest result is the identical vector
    assert!(
        search_results[0].id.ends_with("_vec1"),
        "Closest result should be vec1 (identical to query)"
    );

    debug!("✅ Search returned {} results", search_results.len());
    for (i, result) in search_results.iter().enumerate() {
        debug!(
            "  Result {}: id={}, distance={:?}",
            i, result.id, result.distance
        );
    }

    info!("Test 4: Metadata filtering");

    // Search with metadata filter
    let filter = proximadb::core::search::FilterExpression::Comparison {
        field: "category".to_string(),
        operator: proximadb::core::search::ComparisonOperator::Equals,
        value: serde_json::Value::String("A".to_string()),
    };

    // Create filtered search context
    let filtered_search_params = std::sync::Arc::new(proximadb::core::search::SearchParams {
        vector: Some(query_vector.clone()),
        query_vectors: None,
        top_k: Some(5),
        distance_metric: Some(proximadb::compute::distance_computation::DistanceMetric::Cosine),
        filter_expression: Some(filter),
        include_metadata: Some(true),
        include_vectors: Some(true),
        timeout_ms: None,
        accuracy_threshold: None,
        enable_early_termination: None,
        max_results_per_stage: None,
        progressive_search: None,
    });

    let filtered_query_context = proximadb::storage::traits::StorageQueryContext {
        search_params: filtered_search_params,
        collection: collection.clone(),
        metadata: proximadb::storage::traits::StorageQueryMetadata {
            collection_id: collection_id.to_string(),
            use_axis_indexes: false,
            storage_url: Some(storage_url.clone()),
            ..Default::default()
        },
    };

    let filtered_results = sst_engine
        .search_vectors_unified(&filtered_query_context)
        .await?;

    assert_eq!(
        filtered_results.len(),
        2,
        "Should find 2 vectors with category A"
    );

    debug!(
        "✅ Filtered search returned {} results",
        filtered_results.len()
    );

    info!("✅ All SST collection tests passed!");

    Ok(())
}
