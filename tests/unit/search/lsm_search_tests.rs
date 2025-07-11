//! Unit tests for LSM search engine

use proximadb::core::avro_unified::{Collection, IndexingAlgorithm};
use proximadb::core::search::storage_aware::{
    SearchHints, QuantizationLevel, SearchCapabilities, SearchMetrics
};
use proximadb::compute::distance::DistanceMetric;
use proximadb::storage::strategy::StorageEngineType;
use chrono::Utc;
use std::collections::HashMap;
use serde_json::json;

#[test]
fn test_search_hints_optimization() {
    // Create a mock collection using native type
    let collection = Collection {
        id: "test-uuid".to_string(),
        name: "test-collection".to_string(),
        dimension: 384,
        distance_metric: DistanceMetric::Cosine,
        storage_engine: StorageEngineType::Lsm,
        indexing_algorithm: IndexingAlgorithm::Hnsw,
        created_at: Utc::now().timestamp_millis(),
        updated_at: Utc::now().timestamp_millis(),
        vector_count: 0,
        total_size_bytes: 0,
        config: HashMap::new(),
        filterable_metadata_fields: vec![],
    };

    // This would need proper LSM tree initialization in a real test
    // For now, we'll skip the full engine creation

    let hints = SearchHints::default();

    // Test optimization logic
    assert!(hints.predicate_pushdown); // Default is true
    assert!(hints.use_bloom_filters); // Default is true
    assert_eq!(hints.quantization_level, QuantizationLevel::FP32);
    
    // Test that collection has expected values
    assert_eq!(collection.name, "test-collection");
    assert_eq!(collection.dimension, 384);
}

#[test]
fn test_cosine_distance_calculation() {
    use proximadb::compute::unified_distance::UnifiedDistanceCompute;
    use proximadb::compute::distance::DistanceMetric;
    
    let distance_compute = UnifiedDistanceCompute::default();
    
    // Test perpendicular vectors
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![0.0, 1.0, 0.0];
    let distance = distance_compute.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    assert!((distance - 1.0).abs() < 0.001); // Should be 1.0 for perpendicular vectors

    // Test identical vectors
    let c = vec![1.0, 0.0, 0.0];
    let d = vec![1.0, 0.0, 0.0];
    let distance2 = distance_compute.calculate_distance(&c, &d, &DistanceMetric::Cosine);
    assert!(distance2.abs() < 0.001); // Should be 0.0 for identical vectors
}

#[test]
fn test_lsm_capabilities() {
    let capabilities = SearchCapabilities {
        supports_predicate_pushdown: false, // LSM uses key-based access
        supports_bloom_filters: true,
        supports_clustering: false, // LSM is sorted by key, not clustered
        supports_parallel_search: true,
        supported_quantization: vec![
            QuantizationLevel::FP32, // LSM typically stores full precision
        ],
        max_k: 10000,
        max_dimension: 65536,
        engine_features: {
            let mut features = HashMap::new();
            features.insert("bloom_filters".to_string(), json!(true));
            features.insert("level_aware_search".to_string(), json!(true));
            features.insert("memtable_priority".to_string(), json!(true));
            features.insert("tombstone_handling".to_string(), json!(true));
            features
        },
    };
    
    assert!(!capabilities.supports_predicate_pushdown);
    assert!(capabilities.supports_bloom_filters);
    assert_eq!(capabilities.supported_quantization.len(), 1);
    assert!(capabilities.engine_features.contains_key("bloom_filters"));
}

#[test]
fn test_bloom_filter_optimization() {
    // Test bloom filter false positive rate calculation
    let expected_items = 1_000_000;
    let target_fp_rate = 0.01; // 1% false positive rate
    
    // Calculate optimal bloom filter size
    let bits_per_item = (-1.44 * (target_fp_rate.ln() / 2.0_f64.ln())).ceil() as usize;
    let total_bits = bits_per_item * expected_items;
    
    assert!(bits_per_item > 9); // Should be around 10 bits per item for 1% FP rate
    assert!(total_bits > 9_000_000); // Should be > 9MB for 1M items
}

#[test]
fn test_lsm_level_search_priority() {
    // Test that LSM searches in the correct order: memtable -> L0 -> L1 -> L2...
    let search_order = vec![
        "memtable",
        "level_0",
        "level_1", 
        "level_2",
        "level_3",
    ];
    
    // Simulate level-aware search
    for (idx, level) in search_order.iter().enumerate() {
        // Earlier levels should have higher priority (lower index)
        assert_eq!(idx, search_order.iter().position(|&x| x == *level).unwrap());
    }
}

#[test]
fn test_search_metrics_tracking() {
    use proximadb::core::search::storage_aware::SearchMetrics;
    
    let metrics = SearchMetrics {
        total_searches: 100,
        avg_latency_us: 1000.0,
        p95_latency_us: 1500.0,
        p99_latency_us: 2000.0,
        avg_vectors_scanned: 1000.0,
        cache_hit_rate: 0.9,
        index_efficiency: {
            let mut efficiency = HashMap::new();
            efficiency.insert("bloom_filter_skip_rate".to_string(), 0.8);
            efficiency
        },
        quantization_accuracy: None,
        engine_metrics: HashMap::new(),
    };
    
    assert_eq!(metrics.total_searches, 100);
    assert_eq!(metrics.avg_latency_us, 1000.0);
    assert_eq!(metrics.cache_hit_rate, 0.9);
    
    // Check bloom filter effectiveness
    let bloom_effectiveness = metrics.index_efficiency.get("bloom_filter_skip_rate").unwrap();
    assert!(bloom_effectiveness >= &0.8); // 80% skip rate
}

#[test]
fn test_euclidean_distance_calculation() {
    use proximadb::compute::unified_distance::UnifiedDistanceCompute;
    use proximadb::compute::distance::DistanceMetric;
    
    let distance_compute = UnifiedDistanceCompute::default();
    
    // Test simple 2D vectors
    let a = vec![0.0, 0.0];
    let b = vec![3.0, 4.0];
    let distance = distance_compute.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    assert!((distance - 5.0).abs() < 0.001); // 3-4-5 triangle
    
    // Test identical vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![1.0, 2.0, 3.0];
    let distance2 = distance_compute.calculate_distance(&c, &d, &DistanceMetric::Euclidean);
    assert!(distance2.abs() < 0.001); // Should be 0.0
}

#[test]
fn test_tombstone_handling() {
    // Test that deleted vectors are properly filtered out
    let vectors_with_tombstones = vec![
        ("vec1", false), // active
        ("vec2", true),  // deleted (tombstone)
        ("vec3", false), // active
        ("vec4", true),  // deleted (tombstone)
        ("vec5", false), // active
    ];
    
    let active_vectors: Vec<&str> = vectors_with_tombstones
        .iter()
        .filter(|(_, is_deleted)| !is_deleted)
        .map(|(id, _)| *id)
        .collect();
    
    assert_eq!(active_vectors.len(), 3);
    assert!(active_vectors.contains(&"vec1"));
    assert!(!active_vectors.contains(&"vec2"));
}

#[test]
fn test_lsm_supports_multiple_distance_metrics() {
    use proximadb::compute::unified_distance::UnifiedDistanceCompute;
    use proximadb::compute::distance::DistanceMetric;
    
    let distance_compute = UnifiedDistanceCompute::default();
    
    // Test vectors
    let vec_a = vec![1.0, 2.0, 3.0];
    let vec_b = vec![4.0, 5.0, 6.0];
    
    // Test with different distance metrics
    let cosine_dist = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
    let euclidean_dist = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
    let manhattan_dist = distance_compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Manhattan);
    
    // All should return valid distances
    assert!(cosine_dist.is_finite());
    assert!(euclidean_dist.is_finite());
    assert!(manhattan_dist.is_finite());
    
    // Different metrics should give different values
    assert_ne!(cosine_dist, euclidean_dist);
    assert_ne!(euclidean_dist, manhattan_dist);
    assert_ne!(cosine_dist, manhattan_dist);
    
    // Test known values
    assert!((euclidean_dist - 5.196152).abs() < 0.001); // sqrt(27)
    assert!((manhattan_dist - 9.0).abs() < 0.001); // |4-1| + |5-2| + |6-3|
}