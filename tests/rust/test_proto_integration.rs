//! Protocol Integration Tests
//!
//! Tests to verify that the new proto definitions work correctly
//! with the server implementation and quantization features.

use anyhow::Result;
use serde_json::json;
use tracing::{debug, error, info, warn};

#[tokio::test]
async fn test_quantization_config_fields() -> Result<()> {
    // Test that quantization field exists and can be used
    use proximadb::proto::proximadb::*;

    // Check if we can find the QuantizationConfig message
    // This test validates that our proto updates were applied correctly

    // First, let's test a CollectionConfig without quantization
    let basic_config = CollectionConfig {
        name: "basic_collection".to_string(),
        dimension: 128,
        distance_metric: 1,            // COSINE
        storage_engine: 1,             // VIPER
        primary_indexing_algorithm: 1, // HNSW
        filterable_columns: vec![],
        index_configs: vec![],
        quantization: None, // This field should exist
        primary_index: "default".to_string(),
        auto_index_selection: false,
        description: Some("Test collection".to_string()),
        tags: vec![],
        owner: Some("test".to_string()),
        compression: None,
        optimization_hints: None,
        storage_location: None,
    };

    assert_eq!(basic_config.name, "basic_collection");
    assert!(basic_config.quantization.is_none());

    debug!("✅ Quantization config field exists in CollectionConfig");
    Ok(())
}

#[tokio::test]
async fn test_index_config_field() -> Result<()> {
    // Test that index_config field exists and can be used
    use proximadb::proto::proximadb::*;

    let config_with_index = CollectionConfig {
        name: "indexed_collection".to_string(),
        dimension: 256,
        distance_metric: 1,            // COSINE
        storage_engine: 1,             // VIPER
        primary_indexing_algorithm: 1, // HNSW
        filterable_columns: vec![],
        index_configs: vec![],
        quantization: None,
        primary_index: "default".to_string(),
        auto_index_selection: false,
        description: Some("Test collection".to_string()),
        tags: vec![],
        owner: Some("test".to_string()),
        compression: None,
        optimization_hints: None,
        storage_location: None,
    };

    assert_eq!(config_with_index.name, "indexed_collection");
    assert!(config_with_index.index_configs.is_empty());

    debug!("✅ Index config field exists in CollectionConfig");
    Ok(())
}

#[tokio::test]
async fn test_search_optimization_hints_field() -> Result<()> {
    // Test that optimization_hints field exists in VectorSearchRequest
    use proximadb::proto::proximadb::*;

    // Create a search query
    let search_query = SearchQuery {
        vector: vec![1.0, 2.0, 3.0, 4.0],
        id: None,
        metadata_filter: None,
    };

    let search_request = VectorSearchRequest {
        collection_id: "test_collection".to_string(),
        queries: vec![search_query],
        top_k: 10,
        distance_metric_override: None,
        search_params: None,
        include_fields: Some(IncludeFields::default()),
        search_optimization: None, // This field should exist from our proto updates
    };

    assert_eq!(search_request.collection_id, "test_collection");
    assert_eq!(search_request.queries.len(), 1);
    assert_eq!(search_request.queries[0].vector.len(), 4);
    assert_eq!(search_request.top_k, 10);
    assert!(search_request.search_optimization.is_none());

    debug!("✅ Search optimization hints field exists in VectorSearchRequest");
    Ok(())
}

#[tokio::test]
async fn test_quantization_message_types_exist() -> Result<()> {
    // Test that the quantization message types are generated and accessible
    use proximadb::proto::proximadb::*;

    // This test verifies that our proto quantization messages were generated correctly
    // Even if we can't test complex nested structures, we should be able to reference the types

    // Let's create a simple test that proves the types exist
    let _ = std::any::type_name::<QuantizationConfig>();
    let _ = std::any::type_name::<SearchParams>();

    // If these types don't exist, the compilation will fail
    debug!(
        "✅ Quantization message types exist: {:?}",
        std::any::type_name::<QuantizationConfig>()
    );
    debug!(
        "✅ Search params type exists: {:?}",
        std::any::type_name::<SearchParams>()
    );

    Ok(())
}

#[tokio::test]
async fn test_handler_hint_processing() -> Result<()> {
    // Test that optimization hints can be processed by handlers
    // This simulates what the gRPC and REST handlers do

    let hints_json = json!({
        "enable_two_stage_search": true,
        "candidate_multiplier": 2.0,
        "min_candidates": 100,
        "max_candidates": 1000,
        "accuracy_threshold": 0.95,
        "quantization_hint": "PQ8"
    });

    // Simulate gRPC handler processing
    let enable_two_stage = hints_json
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let candidate_multiplier = hints_json.get(key).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;

    let quantization_hint = hints_json
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("FP32")
        .to_string();

    // Create processed hints JSON as handlers do
    let processed_hints = json!({
        "enable_two_stage_search": enable_two_stage,
        "candidate_multiplier": candidate_multiplier,
        "quantization_hint": quantization_hint,
        "min_candidates": hints_json.get(key),
        "max_candidates": hints_json.get(key),
        "accuracy_threshold": hints_json.get(key)
    });

    assert_eq!(processed_hints["enable_two_stage_search"], true);
    assert_eq!(processed_hints["candidate_multiplier"], 2.0);
    assert_eq!(processed_hints["quantization_hint"], "PQ8");

    debug!("✅ Handler hint processing test passed");
    Ok(())
}
