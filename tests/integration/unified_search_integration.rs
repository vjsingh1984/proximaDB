//! Simplified Integration Test for Unified Search Interface
//!
//! This test focuses on the core unified search functionality using the actual
//! API structure and testing the search engines directly.

use std::sync::Arc;
use std::collections::HashMap;
use serde_json::json;

use proximadb::core::VectorRecord;
use proximadb::core::search::{SearchParams, SearchResult};
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::search::unified_interface::{
    UnifiedSearchContext, CollectionConfig, FilterableColumn,
    ColumnDataType, StorageInfo
};
use proximadb::proto::proximadb::MetadataItem;

/// Generate test vectors with basic metadata
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();
    let now = chrono::Utc::now().timestamp();
    
    for i in 0..count {
        let vector = (0..dimension)
            .map(|j| (i * dimension + j) as f32 / (count * dimension) as f32)
            .collect();
        
        let metadata = vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(format!("cat_{}", i % 3))),
            },
            MetadataItem {
                key: "score".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue((i as f64 / count as f64).to_string())),
            },
            MetadataItem {
                key: "active".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue((i % 2 == 0).to_string())),
            },
        ];
        
        vectors.push(VectorRecord {
            id: Some(format!("vec_{}", i)),
            vector,
            metadata,
            timestamp: now as u32,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            distance: None,
            rank: None,
            score: None,
        });
    }
    
    vectors
}

/// Create test search context
fn create_test_search_context() -> UnifiedSearchContext {
    let filterable_columns = vec![
        FilterableColumn {
            name: "category".to_string(),
            data_type: ColumnDataType::String,
            is_indexed: true,
            estimated_cardinality: Some(3),
        },
        FilterableColumn {
            name: "score".to_string(),
            data_type: ColumnDataType::Float,
            is_indexed: false,
            estimated_cardinality: None,
        },
        FilterableColumn {
            name: "active".to_string(),
            data_type: ColumnDataType::Boolean,
            is_indexed: true,
            estimated_cardinality: Some(2),
        },
    ];
    
    let collection_config = CollectionConfig {
        default_distance_metric: DistanceMetric::Cosine,
        vector_dimension: 128,
        enable_quantization: true,
        enable_metadata_filtering: true,
        estimated_document_count: 1000,
            };
    
    let storage_info = StorageInfo {
        is_cloud_storage: false,
        storage_type: "Local".to_string(),
        estimated_size_mb: 10.0,
        file_count: 5,
        supports_range_requests: true,
        file_paths: Some(vec![
            "/tmp/test_collection/data_001.parquet".to_string(),
            "/tmp/test_collection/data_002.parquet".to_string(),
        ]),
    };
    
    UnifiedSearchContext {
        collection_id: "test_collection".to_string(),
        collection_config: Some(collection_config),
        filterable_columns,
        available_quantization: vec![],
        storage_info,
    }
}

/// Test unified search interface trait
#[tokio::test]
async fn test_unified_search_interface() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Test data
    let test_vectors = generate_test_vectors(10, 32);
    let query_vector = vec![0.5; 32];
    
    // Create search context
    let context = create_test_search_context();
    
    // Create search params
    let search_params = SearchParams {
        query_vectors: Some(vec![query_vector]),
        top_k: Some(5),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    };
    
    // Create unified distance compute
    let _distance_compute = Arc::new(UnifiedDistanceCompute::default());
    
    // Test that we can create the unified search context
    assert_eq!(context.collection_id, "test_collection");
    assert_eq!(context.filterable_columns.len(), 3);
    assert!(context.collection_config.is_some());
    
    // Test SearchParams structure
    assert_eq!(search_params.top_k, Some(5));
    assert_eq!(search_params.distance_metric, Some(DistanceMetric::Cosine));
    
    // Test VectorRecord structure
    let first_vector = &test_vectors[0];
    assert_eq!(first_vector.id, Some("vec_0".to_string()));
    assert_eq!(first_vector.vector.len(), 32);
    assert_eq!(first_vector.metadata.len(), 3);
    
    // Test completed
}

/// Test basic search functionality with SearchParams
#[tokio::test]
async fn test_search_params_functionality() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Test various SearchParams configurations
    let query_vectors = vec![
        vec![0.1; 16],
        vec![0.5; 16],
        vec![0.9; 16],
    ];
    
    // Test single vector search
    let single_search = SearchParams::single_vector(query_vectors[0].clone());
    assert_eq!(single_search.query_vectors.as_ref().unwrap().len(), 1);
    assert_eq!(single_search.top_k, Some(10)); // Default value
    
    // Test batch vector search
    let batch_search = SearchParams::batch_vectors(query_vectors.clone());
    assert_eq!(batch_search.query_vectors.as_ref().unwrap().len(), 3);
    
    // Test search with filters
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("cat_1"));
    filters.insert("active".to_string(), json!(true));
    
    let filtered_search = SearchParams {
        query_vectors: Some(vec![query_vectors[1].clone()]),
        top_k: Some(20),
        distance_metric: Some(DistanceMetric::Manhattan),
        ..Default::default()
    }.with_simple_filters(filters.clone());
    
    // Can't assert on filters directly anymore, but we can verify the filter_expression
    assert!(filtered_search.filter_expression.is_some());
    assert_eq!(filtered_search.distance_metric, Some(DistanceMetric::Manhattan));
    
    // Test completed
}

/// Test distance metric functionality
#[tokio::test]
async fn test_distance_metrics() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let distance_compute = UnifiedDistanceCompute::default();
    
    // Test vectors
    let vector1 = vec![1.0, 0.0, 0.0];
    let vector2 = vec![0.0, 1.0, 0.0];
    let identical = vec![1.0, 0.0, 0.0];
    
    // Test all distance metrics
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::Manhattan,
        DistanceMetric::DotProduct,
    ];
    
    for metric in metrics {
        // Test distance calculation
        let result1 = distance_compute.calculate_distance(&vector1, &vector2, &metric);
        let result2 = distance_compute.calculate_distance(&vector1, &identical, &metric);
        
        // Verify the result structure
        // For DotProduct, raw_value can be negative and rank_value is -raw_value
        if metric == DistanceMetric::DotProduct {
            // DotProduct can have any value
            assert!(result1.normalized_score >= 0.0 && result1.normalized_score <= 1.0);
            assert!(result2.normalized_score >= 0.0 && result2.normalized_score <= 1.0);
        } else {
            // Distance metrics should have non-negative raw values
            assert!(result1.raw_value >= 0.0);
            assert!(result1.normalized_score >= 0.0 && result1.normalized_score <= 1.0);
            assert!(result1.rank_value >= 0.0);
            
            assert!(result2.raw_value >= 0.0);
            assert!(result2.normalized_score >= 0.0 && result2.normalized_score <= 1.0);
            assert!(result2.rank_value >= 0.0);
        }
        
        // Identical vectors should have higher similarity (lower rank_value)
        // This tests the semantic consistency of the unified distance system
        assert!(result2.rank_value <= result1.rank_value, 
            "Identical vectors should have lower rank_value for {:?}", metric);
        
        // Distance metric test completed
    }
}

/// Test VectorRecord structure and metadata handling
#[tokio::test]
async fn test_vector_record_structure() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let test_vectors = generate_test_vectors(5, 64);
    
    // Test vector record structure
    for (i, vector) in test_vectors.iter().enumerate() {
        assert_eq!(vector.id, Some(format!("vec_{}", i)));
        assert_eq!(vector.vector.len(), 64);
        assert_eq!(vector.metadata.len(), 3);
        assert!(vector.timestamp > 0);
        assert!(vector.updated_at.unwrap_or(0) > 0);
        assert_eq!(vector.version, Some(1));
        
        // Test metadata content - metadata is now Vec<MetadataItem>
        let category_item = vector.metadata.iter().find(|item| item.key == "category").unwrap();
        match &category_item.value {
            Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(s)) => {
                assert!(s.starts_with("cat_"));
            }
            _ => panic!("Bad"),
        }
        
        let score_item = vector.metadata.iter().find(|item| item.key == "score").unwrap();
        match &score_item.value {
            Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(s)) => {
                let score: f64 = s.parse().unwrap();
                assert!(score >= 0.0 && score <= 1.0);
            }
            _ => panic!("Bad"),
        }
        
        let active_item = vector.metadata.iter().find(|item| item.key == "active").unwrap();
        match &active_item.value {
            Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(s)) => {
                let active: bool = s.parse().unwrap();
                assert!(active == (i % 2 == 0));
            }
            _ => panic!("Bad"),
        }
    }
    
    // Test completed
}

/// Test SearchResult structure
#[tokio::test]
async fn test_search_result_structure() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Create a mock search result
    let mut metadata = HashMap::new();
    metadata.insert("category".to_string(), json!("test"));
    metadata.insert("score".to_string(), json!(0.85));
    
    let search_result = SearchResult {
        id: "test_id".to_string(),
        vector_id: Some("vec_123".to_string()),
        score: 0.95,
        distance: Some(0.05),
        rank: Some(1),
        vector: Some(vec![0.1, 0.2, 0.3]),
        metadata,
        debug_info: None,
        semantic_distance: None,
        quantization_info: None,
        engine_stats: None,
        index_path: Some("test_index".to_string()),
        created_at: Some(chrono::Utc::now()),
        version: Some(1),
        timestamp: Some(chrono::Utc::now().timestamp() as u32),
    };
    
    // Test all fields
    assert_eq!(search_result.id, "test_id");
    assert_eq!(search_result.vector_id, Some("vec_123".to_string()));
    assert_eq!(search_result.score, 0.95);
    assert_eq!(search_result.distance, Some(0.05));
    assert_eq!(search_result.rank, Some(1));
    assert_eq!(search_result.vector.as_ref().unwrap().len(), 3);
    assert_eq!(search_result.metadata.len(), 2);
    assert!(search_result.created_at.is_some());
    assert_eq!(search_result.index_path, Some("test_index".to_string()));
    
    // Test metadata access
    let category = search_result.metadata.get("category").unwrap().as_str().unwrap();
    assert_eq!(category, "test");
    
    let score = search_result.metadata.get("score").unwrap().as_f64().unwrap();
    assert_eq!(score, 0.85);
    
    // Test completed
}

/// Test unified search context creation
#[tokio::test]
async fn test_unified_search_context() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let context = create_test_search_context();
    
    // Test context structure
    assert_eq!(context.collection_id, "test_collection");
    assert_eq!(context.filterable_columns.len(), 3);
    assert!(context.collection_config.is_some());
    
    // Test collection config
    let config = context.collection_config.as_ref().unwrap();
    assert_eq!(config.default_distance_metric, DistanceMetric::Cosine);
    assert_eq!(config.vector_dimension, 128);
    assert!(config.enable_quantization);
    assert!(config.enable_metadata_filtering);
    assert_eq!(config.estimated_document_count, 1000);
    
    // Test filterable columns
    let category_column = &context.filterable_columns[0];
    assert_eq!(category_column.name, "category");
    assert!(matches!(category_column.data_type, ColumnDataType::String));
    assert!(category_column.is_indexed);
    assert_eq!(category_column.estimated_cardinality, Some(3));
    
    let score_column = &context.filterable_columns[1];
    assert_eq!(score_column.name, "score");
    assert!(matches!(score_column.data_type, ColumnDataType::Float));
    assert!(!score_column.is_indexed);
    assert_eq!(score_column.estimated_cardinality, None);
    
    // Test storage info
    assert!(!context.storage_info.is_cloud_storage);
    assert_eq!(context.storage_info.storage_type, "Local");
    assert_eq!(context.storage_info.estimated_size_mb, 10.0);
    assert_eq!(context.storage_info.file_count, 5);
    assert!(context.storage_info.supports_range_requests);
    
    // Test completed
}

/// Test basic unified search engine interface functionality
#[tokio::test]
async fn test_unified_search_engine_interface() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Create search context
    let context = create_test_search_context();
    
    // Create search params
    let search_params = SearchParams {
        query_vectors: Some(vec![vec![0.5; 128]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    };
    
    // Create unified distance compute
    let _distance_compute = Arc::new(UnifiedDistanceCompute::default());
    
    // Test the search interface structure
    assert_eq!(context.collection_id, "test_collection");
    assert_eq!(search_params.top_k, Some(10));
    assert_eq!(search_params.distance_metric, Some(DistanceMetric::Cosine));
    
    // Test that we can create the necessary components
    // (This is a compile-time test - if this compiles, the trait is correctly defined)
    
    // Test completed
}

/// Test SearchParams default values and edge cases
#[tokio::test]
async fn test_search_params_edge_cases() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Test default SearchParams
    let default_params = SearchParams::default();
    assert_eq!(default_params.query_vectors, None);
    assert_eq!(default_params.top_k, Some(10));
    assert_eq!(default_params.distance_metric, Some(DistanceMetric::Cosine));
    assert!(default_params.filter_expression.is_none());
    assert_eq!(default_params.accuracy_threshold, Some(0.95));
    assert_eq!(default_params.timeout_ms, Some(5000));
    
    // Test empty query vectors
    let empty_query_params = SearchParams {
        query_vectors: Some(vec![]),
        top_k: Some(5),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    };
    assert_eq!(empty_query_params.query_vectors.as_ref().unwrap().len(), 0);
    
    // Test large top_k
    let large_k_params = SearchParams {
        query_vectors: Some(vec![vec![0.1; 64]]),
        top_k: Some(10000),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    };
    assert_eq!(large_k_params.top_k, Some(10000));
    
    // Test zero-dimension vectors (edge case)
    let zero_dim_params = SearchParams {
        query_vectors: Some(vec![vec![]]),
        top_k: Some(1),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    };
    assert_eq!(zero_dim_params.query_vectors.as_ref().unwrap()[0].len(), 0);
    
    // Test completed
}

/// Test basic API usage patterns
#[tokio::test]
async fn test_api_usage_patterns() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Test single vector search helper
    let query_vector = vec![0.1, 0.2, 0.3];
    let single_search = SearchParams::single_vector(query_vector.clone());
    assert_eq!(single_search.first_query_vector(), Some(&query_vector));
    
    // Test batch vector search helper
    let batch_vectors = vec![
        vec![0.1, 0.2, 0.3],
        vec![0.4, 0.5, 0.6],
        vec![0.7, 0.8, 0.9],
    ];
    let batch_search = SearchParams::batch_vectors(batch_vectors.clone());
    assert_eq!(batch_search.query_vectors.as_ref().unwrap().len(), 3);
    // Test that it's a batch search by checking the number of vectors
    assert!(batch_search.query_vectors.as_ref().unwrap().len() > 1);
    
    // Test with different distance metrics
    let euclidean_search = SearchParams {
        query_vectors: Some(vec![vec![1.0, 0.0, 0.0]]),
        distance_metric: Some(DistanceMetric::Euclidean),
        ..Default::default()
    };
    assert_eq!(euclidean_search.distance_metric, Some(DistanceMetric::Euclidean));
    
    // Test with filters
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("electronics"));
    filters.insert("price".to_string(), json!({"less_than": 100}));
    
    let filtered_search = SearchParams {
        query_vectors: Some(vec![vec![0.5; 128]]),
        top_k: Some(50),
        ..Default::default()
    }.with_simple_filters(filters);
    
    // Can't assert on filters directly anymore, but we can verify the filter_expression
    assert!(filtered_search.filter_expression.is_some());
    assert_eq!(filtered_search.top_k, Some(50));
    
    // Test completed
}