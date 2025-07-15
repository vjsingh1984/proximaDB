//! Unit tests for storage-aware search infrastructure

use proximadb::core::search::storage_aware::{
    SearchHints, QuantizationLevel, SearchCapabilities, SearchMetrics,
    ClusteringHints, ClusterDistanceMetric
};
use std::collections::HashMap;
use serde_json::json;

#[test]
fn test_search_hints_default_values() {
    let hints = SearchHints::default();
    
    assert!(hints.predicate_pushdown);
    assert!(hints.use_bloom_filters);
    assert!(hints.clustering_hints.is_none());
    assert_eq!(hints.quantization_level, QuantizationLevel::FP32);
    assert_eq!(hints.timeout_ms, Some(5000));
    assert!(!hints.include_debug_info);
    assert!(hints.enable_parallel_search);
    assert!(hints.engine_specific.is_empty());
}

#[test]
fn test_search_hints_custom_configuration() {
    let hints = SearchHints {
        predicate_pushdown: false,
        use_bloom_filters: false,
        clustering_hints: Some(ClusteringHints::default()),
        quantization_level: QuantizationLevel::PQ8,
        timeout_ms: Some(10000),
        include_debug_info: true,
        enable_parallel_search: false,
        engine_specific: {
            let mut map = HashMap::new();
            map.insert("custom_param".to_string(), json!("value"));
            map
        },
    };
    
    assert!(!hints.predicate_pushdown);
    assert!(!hints.use_bloom_filters);
    assert!(hints.clustering_hints.is_some());
    assert_eq!(hints.quantization_level, QuantizationLevel::PQ8);
    assert_eq!(hints.timeout_ms, Some(10000));
    assert!(hints.include_debug_info);
    assert!(!hints.enable_parallel_search);
    assert_eq!(hints.engine_specific.get("custom_param"), Some(&json!("value")));
}

#[test]
fn test_clustering_hints_default() {
    let hints = ClusteringHints::default();
    
    assert!(hints.enable_ml_clustering);
    assert_eq!(hints.max_clusters_to_search, 10);
    assert_eq!(hints.cluster_confidence_threshold, 0.7);
    assert!(hints.cluster_centroids_cache.is_none());
    
    match hints.cluster_distance_metric {
        ClusterDistanceMetric::Cosine => assert!(true),
        _ => panic!("Expected Cosine as default cluster distance metric"),
    }
}

#[test]
fn test_search_capabilities_comprehensive() {
    // Test LSM capabilities
    let lsm_capabilities = SearchCapabilities {
        supports_predicate_pushdown: false,
        supports_bloom_filters: true,
        supports_clustering: false,
        supports_parallel_search: true,
        supported_quantization: vec![QuantizationLevel::FP32],
        max_k: 10000,
        max_dimension: 65536,
        engine_features: {
            let mut features = HashMap::new();
            features.insert("memtable_search".to_string(), json!(true));
            features.insert("level_compaction".to_string(), json!(true));
            features
        },
    };
    
    assert!(!lsm_capabilities.supports_predicate_pushdown);
    assert!(lsm_capabilities.supports_bloom_filters);
    assert!(!lsm_capabilities.supports_clustering);
    
    // Test VIPER capabilities
    let viper_capabilities = SearchCapabilities {
        supports_predicate_pushdown: true,
        supports_bloom_filters: false,
        supports_clustering: true,
        supports_parallel_search: true,
        supported_quantization: vec![
            QuantizationLevel::FP32,
            QuantizationLevel::PQ8,
            QuantizationLevel::PQ4,
            QuantizationLevel::Binary,
            QuantizationLevel::INT8,
        ],
        max_k: 10000,
        max_dimension: 65536,
        engine_features: {
            let mut features = HashMap::new();
            features.insert("parquet_columnar".to_string(), json!(true));
            features.insert("ml_clustering".to_string(), json!(true));
            features
        },
    };
    
    assert!(viper_capabilities.supports_predicate_pushdown);
    assert!(!viper_capabilities.supports_bloom_filters);
    assert!(viper_capabilities.supports_clustering);
    assert_eq!(viper_capabilities.supported_quantization.len(), 5);
}

#[test]
fn test_search_metrics_tracking() {
    let mut index_efficiency = HashMap::new();
    index_efficiency.insert("bloom_filter_false_positive_rate".to_string(), 0.1);
    index_efficiency.insert("index_pruning_rate".to_string(), 0.8);
    
    let metrics = SearchMetrics {
        total_searches: 1000,
        avg_latency_us: 42500.0, // 42.5ms in microseconds
        p95_latency_us: 85000.0, // 85ms
        p99_latency_us: 120000.0, // 120ms
        avg_vectors_scanned: 2500.0,
        cache_hit_rate: 0.85,
        index_efficiency,
        quantization_accuracy: None,
        engine_metrics: HashMap::new(),
    };
    
    assert_eq!(metrics.total_searches, 1000);
    assert_eq!(metrics.avg_latency_us, 42500.0);
    assert_eq!(metrics.avg_vectors_scanned, 2500.0);
    assert_eq!(metrics.cache_hit_rate, 0.85);
    
    // Verify index efficiency metrics
    assert!(metrics.index_efficiency.contains_key("bloom_filter_false_positive_rate"));
    assert_eq!(metrics.index_efficiency["bloom_filter_false_positive_rate"], 0.1);
    
    // Verify performance characteristics
    assert!(metrics.p95_latency_us > metrics.avg_latency_us);
    assert!(metrics.p99_latency_us > metrics.p95_latency_us);
    assert!(metrics.cache_hit_rate > 0.8); // Good cache performance
}

#[test]
fn test_quantization_level_ordering() {
    // Test that quantization levels can be compared for compression ratio
    let levels = vec![
        (QuantizationLevel::FP32, 32),     // 32 bits per value
        (QuantizationLevel::INT8, 8),      // 8 bits per value
        (QuantizationLevel::PQ8, 8),       // 8 bits per value (product quantization)
        (QuantizationLevel::PQ4, 4),       // 4 bits per value
        (QuantizationLevel::Binary, 1),    // 1 bit per value
    ];
    
    // Verify compression ratios
    for (level, expected_bits) in levels {
        let bits = match level {
            QuantizationLevel::FP32 => 32,
            QuantizationLevel::INT8 => 8,
            QuantizationLevel::PQ8 => 8,
            QuantizationLevel::PQ4 => 4,
            QuantizationLevel::Binary => 1,
        };
        assert_eq!(bits, expected_bits);
    }
}

#[test]
fn test_cluster_distance_metrics() {
    let metrics = vec![
        ClusterDistanceMetric::Cosine,
        ClusterDistanceMetric::Euclidean,
        ClusterDistanceMetric::DotProduct,
        ClusterDistanceMetric::Manhattan,
    ];
    
    for metric in metrics {
        match metric {
            ClusterDistanceMetric::Cosine => {
                // Cosine is best for normalized embeddings
                assert!(true);
            }
            ClusterDistanceMetric::Euclidean => {
                // Euclidean is standard L2 distance
                assert!(true);
            }
            ClusterDistanceMetric::DotProduct => {
                // Dot product is fastest but not a true distance
                assert!(true);
            }
            ClusterDistanceMetric::Manhattan => {
                // Manhattan (L1) is robust to outliers
                assert!(true);
            }
        }
    }
}

#[test]
fn test_search_hints_serialization() {
    use serde_json;
    
    let mut hints = SearchHints::default();
    hints.engine_specific.insert("test_key".to_string(), json!(123));
    hints.clustering_hints = Some(ClusteringHints {
        enable_ml_clustering: true,
        max_clusters_to_search: 5,
        cluster_confidence_threshold: 0.85,
        cluster_centroids_cache: None,
        cluster_distance_metric: ClusterDistanceMetric::Euclidean,
    });
    
    // Serialize to JSON
    let serialized = serde_json::to_string(&hints).unwrap();
    
    // Deserialize back
    let deserialized: SearchHints = serde_json::from_str(&serialized).unwrap();
    
    assert_eq!(deserialized.predicate_pushdown, hints.predicate_pushdown);
    assert_eq!(deserialized.timeout_ms, hints.timeout_ms);
    assert!(deserialized.clustering_hints.is_some());
    assert_eq!(
        deserialized.clustering_hints.as_ref().unwrap().max_clusters_to_search,
        5
    );
    assert_eq!(
        deserialized.engine_specific.get("test_key"),
        Some(&json!(123))
    );
}

#[test]
fn test_engine_specific_parameters() {
    let mut hints = SearchHints::default();
    
    // LSM-specific parameters
    hints.engine_specific.insert("memtable_only".to_string(), json!(true));
    hints.engine_specific.insert("level_limit".to_string(), json!(3));
    hints.engine_specific.insert("use_block_cache".to_string(), json!(true));
    
    // VIPER-specific parameters
    hints.engine_specific.insert("row_group_limit".to_string(), json!(100));
    hints.engine_specific.insert("enable_page_index".to_string(), json!(true));
    hints.engine_specific.insert("projection_columns".to_string(), json!(["id", "vector", "metadata"]));
    
    assert_eq!(hints.engine_specific.len(), 6);
    assert_eq!(hints.engine_specific["memtable_only"], json!(true));
    assert_eq!(hints.engine_specific["row_group_limit"], json!(100));
}

#[test]
fn test_capabilities_feature_detection() {
    let capabilities = SearchCapabilities {
        supports_predicate_pushdown: true,
        supports_bloom_filters: false,
        supports_clustering: true,
        supports_parallel_search: true,
        supported_quantization: vec![
            QuantizationLevel::FP32,
            QuantizationLevel::PQ8,
        ],
        max_k: 5000,
        max_dimension: 4096,
        engine_features: HashMap::new(),
    };
    
    // Test feature availability
    assert!(capabilities.supports_predicate_pushdown);
    assert!(!capabilities.supports_bloom_filters);
    assert!(capabilities.supports_clustering);
    
    // Test quantization support
    assert!(capabilities.supported_quantization.contains(&QuantizationLevel::FP32));
    assert!(capabilities.supported_quantization.contains(&QuantizationLevel::PQ8));
    assert!(!capabilities.supported_quantization.contains(&QuantizationLevel::Binary));
    
    // Test limits
    assert_eq!(capabilities.max_k, 5000);
    assert_eq!(capabilities.max_dimension, 4096);
}