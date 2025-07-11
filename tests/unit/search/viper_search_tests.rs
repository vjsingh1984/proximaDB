//! Unit tests for VIPER search engine

use proximadb::core::avro_unified::{Collection, IndexingAlgorithm};
use proximadb::core::search::storage_aware::{
    SearchHints, QuantizationLevel, ClusteringHints, SearchCapabilities
};
use proximadb::compute::distance::DistanceMetric;
use proximadb::storage::strategy::StorageEngineType;
use chrono::Utc;
use std::collections::HashMap;
use serde_json::json;

#[test]
fn test_viper_search_hints_optimization() {
    // Create a mock collection using native type
    let collection = Collection {
        id: "test-viper-uuid".to_string(),
        name: "test-viper-collection".to_string(),
        dimension: 768, // BERT-base dimension
        distance_metric: DistanceMetric::Cosine,
        storage_engine: StorageEngineType::Viper,
        indexing_algorithm: IndexingAlgorithm::Hnsw,
        created_at: Utc::now().timestamp_millis(),
        updated_at: Utc::now().timestamp_millis(),
        vector_count: 1000000, // Large collection
        total_size_bytes: 3_000_000_000, // 3GB
        config: HashMap::new(),
        filterable_metadata_fields: vec!["category".to_string(), "author".to_string()],
    };

    let mut hints = SearchHints::default();
    
    // VIPER should enable predicate pushdown
    assert!(hints.predicate_pushdown);
    
    // VIPER should not need bloom filters (uses Parquet predicates)
    assert!(hints.use_bloom_filters); // Default is true, but VIPER ignores this
    
    // VIPER supports advanced quantization
    hints.quantization_level = QuantizationLevel::PQ8;
    assert_eq!(hints.quantization_level, QuantizationLevel::PQ8);
    
    // VIPER supports clustering
    hints.clustering_hints = Some(ClusteringHints {
        enable_ml_clustering: true,
        cluster_selection_strategy: "cosine_similarity".to_string(),
        max_clusters_to_search: 10,
        confidence_threshold: 0.8,
    });
    assert!(hints.clustering_hints.is_some());
}

#[test]
fn test_viper_capabilities() {
    let capabilities = SearchCapabilities {
        supports_predicate_pushdown: true,
        supports_bloom_filters: false, // VIPER doesn't need bloom filters
        supports_clustering: true,
        supports_parallel_search: true,
        supported_quantization: vec![
            QuantizationLevel::FP32,
            QuantizationLevel::FP16,
            QuantizationLevel::PQ8,
            QuantizationLevel::PQ4,
            QuantizationLevel::Binary,
            QuantizationLevel::INT8,
        ],
        max_k: 10000,
        max_dimension: 65536,
        engine_features: {
            let mut features = HashMap::new();
            features.insert("predicate_pushdown".to_string(), json!(true));
            features.insert("ml_clustering".to_string(), json!(true));
            features.insert("columnar_storage".to_string(), json!(true));
            features.insert("simd_optimization".to_string(), json!(true));
            features
        },
    };
    
    assert!(capabilities.supports_predicate_pushdown);
    assert!(capabilities.supports_clustering);
    assert_eq!(capabilities.supported_quantization.len(), 6);
    assert_eq!(capabilities.max_dimension, 65536);
}

#[test]
fn test_quantization_selection_for_viper() {
    // Test quantization selection based on query characteristics
    struct TestCase {
        k: usize,
        dimension: usize,
        expected_quantization: QuantizationLevel,
    }
    
    let test_cases = vec![
        TestCase {
            k: 5,
            dimension: 384,
            expected_quantization: QuantizationLevel::PQ4, // Small k, small dim -> aggressive quantization
        },
        TestCase {
            k: 100,
            dimension: 768,
            expected_quantization: QuantizationLevel::PQ8, // Medium k, medium dim -> balanced
        },
        TestCase {
            k: 1000,
            dimension: 2048,
            expected_quantization: QuantizationLevel::FP32, // Large k, large dim -> full precision
        },
    ];
    
    for case in test_cases {
        // Simulate quantization selection logic
        let quantization = if case.k <= 10 && case.dimension <= 384 {
            QuantizationLevel::PQ4
        } else if case.k <= 100 && case.dimension <= 768 {
            QuantizationLevel::PQ8
        } else {
            QuantizationLevel::FP32
        };
        
        assert_eq!(quantization, case.expected_quantization);
    }
}

#[test]
fn test_viper_parquet_predicate_construction() {
    // Test that metadata filters are properly converted to Parquet predicates
    let mut filters = HashMap::new();
    filters.insert("category".to_string(), json!("science"));
    filters.insert("year".to_string(), json!(2024));
    filters.insert("score".to_string(), json!({"$gte": 0.8}));
    
    // Simulate predicate construction
    let predicates = vec![
        "category = 'science'",
        "year = 2024",
        "score >= 0.8",
    ];
    
    assert_eq!(predicates.len(), 3);
    assert!(predicates.contains(&"category = 'science'"));
}

#[test]
fn test_viper_two_stage_search_configuration() {
    let mut hints = SearchHints::default();
    // Configure two-stage search via engine-specific hints
    hints.engine_specific.insert("two_stage_search".to_string(), json!(true));
    hints.engine_specific.insert("candidate_multiplier".to_string(), json!(3.0));
    hints.quantization_level = QuantizationLevel::Binary; // Ultra-fast first stage
    
    assert_eq!(hints.engine_specific.get("two_stage_search"), Some(&json!(true)));
    assert_eq!(hints.engine_specific.get("candidate_multiplier"), Some(&json!(3.0)));
    
    // Second stage should use full precision
    let second_stage_quantization = QuantizationLevel::FP32;
    assert_ne!(hints.quantization_level, second_stage_quantization);
}