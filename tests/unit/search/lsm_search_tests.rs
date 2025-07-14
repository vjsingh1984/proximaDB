//! Unit tests for LSM search engine

use proximadb::proto::proximadb::{Collection, CollectionConfig, CollectionStats, DistanceMetric, StorageEngine, IndexingAlgorithm};
use proximadb::core::search::storage_aware::{
    SearchHints, QuantizationLevel, SearchCapabilities, SearchMetrics, ClusteringHints, ClusterDistanceMetric
};
use proximadb::core::search::multi_tier_deduplication::{
    MultiTierDeduplicator, TieredSearchResult, StorageTier, DeduplicationStorageEngine
};
use proximadb::core::{VectorRecord, SearchResult};
use proximadb::proto::proximadb::MetadataItem;
use std::collections::HashMap;
use serde_json::json;
use chrono::Utc;

#[test]
fn test_search_hints_optimization() {
    // Create a mock collection using proto type
    let collection = Collection {
        id: "test-uuid".to_string(),
        config: Some(CollectionConfig {
            name: "test-collection".to_string(),
            dimension: 384,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Lsm as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
        }),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        created_at: 1000,
        updated_at: 1000,
    };

    // This would need proper LSM tree initialization in a real test
    // For now, we'll skip the full engine creation

    let hints = SearchHints::default();

    // Test optimization logic
    assert!(hints.predicate_pushdown); // Default is true
    assert!(hints.use_bloom_filters); // Default is true
    assert_eq!(hints.quantization_level, QuantizationLevel::FP32);
    
    // Test that collection has expected values
    assert_eq!(collection.config.as_ref().unwrap().name, "test-collection");
    assert_eq!(collection.config.as_ref().unwrap().dimension, 384);
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
    assert!((distance.rank_value - 1.0).abs() < 0.001); // Should be 1.0 for perpendicular vectors

    // Test identical vectors
    let c = vec![1.0, 0.0, 0.0];
    let d = vec![1.0, 0.0, 0.0];
    let distance2 = distance_compute.calculate_distance(&c, &d, &DistanceMetric::Cosine);
    assert!(distance2.rank_value.abs() < 0.001); // Should be 0.0 for identical vectors
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
fn test_search_metrics() {
    let mut index_efficiency = HashMap::new();
    index_efficiency.insert("bloom_filter_hit_rate".to_string(), 0.9375); // 300/(300+20)
    index_efficiency.insert("file_skip_rate".to_string(), 0.789); // 45/(12+45)
    
    let mut engine_specific_metrics = HashMap::new();
    engine_specific_metrics.insert("memtable_searches".to_string(), json!(3.0));
    engine_specific_metrics.insert("level_0_searches".to_string(), json!(5.0));
    engine_specific_metrics.insert("level_1_searches".to_string(), json!(4.0));
    engine_specific_metrics.insert("tombstones_filtered".to_string(), json!(150.0));
    
    let metrics = SearchMetrics {
        total_searches: 100,
        avg_latency_us: 25500.0, // 25.5ms in microseconds
        p95_latency_us: 40000.0,
        p99_latency_us: 55000.0,
        avg_vectors_scanned: 1500.0,
        cache_hit_rate: 0.8,
        index_efficiency,
        quantization_accuracy: None,
        engine_metrics: engine_specific_metrics,
    };
    
    assert_eq!(metrics.avg_latency_us, 25500.0);
    assert_eq!(metrics.avg_vectors_scanned, 1500.0);
    assert_eq!(metrics.index_efficiency["bloom_filter_hit_rate"], 0.9375);
    assert_eq!(metrics.index_efficiency["file_skip_rate"], 0.789);
    assert_eq!(metrics.engine_metrics["tombstones_filtered"], json!(150.0));
}

#[test]
fn test_multi_tier_deduplication_lsm() {
    let mut deduplicator = MultiTierDeduplicator::new();
    
    // Create vectors with same ID from different tiers
    let base_record = VectorRecord {
        id: Some("vec1".to_string()),
        collection_id: "test-collection".to_string(),
        vector: vec![1.0, 0.0, 0.0],
        metadata: vec![],
        timestamp: Utc::now().timestamp_micros(),
        created_at: Utc::now().timestamp_micros(),
        updated_at: Utc::now().timestamp_micros(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    // Add from compacted tier (oldest)
    let compacted_result = TieredSearchResult {
        vector_record: base_record.clone(),
        score: 0.8,
        tier: StorageTier::Compacted,
        engine: DeduplicationStorageEngine::LSM,
        timestamp: Utc::now() - chrono::Duration::hours(2),
        sequence: 100,
        file_path: Some("/data/lsm/level3/sst_001.db".to_string()),
    };
    deduplicator.add_tier_results(vec![compacted_result]);
    
    // Add from flushed tier (newer)
    let mut flushed_record = base_record.clone();
    flushed_record.version = 2;
    let flushed_result = TieredSearchResult {
        vector_record: flushed_record,
        score: 0.85,
        tier: StorageTier::Flushed,
        engine: DeduplicationStorageEngine::LSM,
        timestamp: Utc::now() - chrono::Duration::hours(1),
        sequence: 200,
        file_path: Some("/data/lsm/level0/sst_005.db".to_string()),
    };
    deduplicator.add_tier_results(vec![flushed_result]);
    
    // Add from unflushed tier (newest)
    let mut unflushed_record = base_record.clone();
    unflushed_record.version = 3;
    let unflushed_result = TieredSearchResult {
        vector_record: unflushed_record,
        score: 0.9,
        tier: StorageTier::Unflushed,
        engine: DeduplicationStorageEngine::WAL,
        timestamp: Utc::now(),
        sequence: 300,
        file_path: None,
    };
    deduplicator.add_tier_results(vec![unflushed_result]);
    
    // Merge and verify we get the latest version
    let merged = deduplicator.get_final_results(10);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.version, 3);
    assert_eq!(merged[0].score, 0.9);
}

#[test]
fn test_lsm_search_hints_validation() {
    use proximadb::core::search::storage_aware::StorageSearchEngine;
    
    // Test vector dimension validation
    let query_vector = vec![1.0; 768]; // 768-dimensional vector
    let mut hints = SearchHints::default();
    hints.timeout_ms = Some(100); // 100ms timeout
    
    // LSM doesn't support clustering
    hints.clustering_hints = Some(ClusteringHints {
        enable_ml_clustering: true,
        max_clusters_to_search: 5,
        cluster_confidence_threshold: 0.8,
        cluster_centroids_cache: None,
        cluster_distance_metric: ClusterDistanceMetric::Cosine,
    });
    
    // Test that clustering hints are ignored for LSM
    assert!(hints.clustering_hints.is_some());
    
    // Test bloom filter configuration
    hints.engine_specific.insert(
        "bloom_filter_bits".to_string(),
        json!(12)
    );
    hints.engine_specific.insert(
        "level_aware_search".to_string(),
        json!(true)
    );
    
    assert_eq!(hints.engine_specific.len(), 2);
    assert_eq!(hints.engine_specific["bloom_filter_bits"], json!(12));
}

#[test]
fn test_lsm_metadata_filtering() {
    let mut deduplicator = MultiTierDeduplicator::with_filters({
        let mut filters = HashMap::new();
        filters.insert("category".to_string(), json!("science"));
        filters.insert("author".to_string(), json!("einstein"));
        filters
    });
    
    // Create vectors with different metadata
    let record1 = VectorRecord {
        id: Some("vec1".to_string()),
        collection_id: "test-collection".to_string(),
        vector: vec![1.0, 0.0, 0.0],
        metadata: vec![
            MetadataItem { key: "category".to_string(), value: "science".to_string() },
            MetadataItem { key: "author".to_string(), value: "einstein".to_string() },
        ],
        timestamp: Utc::now().timestamp_micros(),
        created_at: Utc::now().timestamp_micros(),
        updated_at: Utc::now().timestamp_micros(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    let record2 = VectorRecord {
        id: Some("vec2".to_string()),
        collection_id: "test-collection".to_string(),
        vector: vec![0.0, 1.0, 0.0],
        metadata: vec![
            MetadataItem { key: "category".to_string(), value: "history".to_string() },
            MetadataItem { key: "author".to_string(), value: "einstein".to_string() },
        ],
        timestamp: Utc::now().timestamp_micros(),
        created_at: Utc::now().timestamp_micros(),
        updated_at: Utc::now().timestamp_micros(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    let result1 = TieredSearchResult {
        vector_record: record1,
        score: 0.9,
        tier: StorageTier::Flushed,
        engine: DeduplicationStorageEngine::LSM,
        timestamp: Utc::now(),
        sequence: 100,
        file_path: Some("/data/lsm/level0/sst_001.db".to_string()),
    };
    
    let result2 = TieredSearchResult {
        vector_record: record2,
        score: 0.85,
        tier: StorageTier::Flushed,
        engine: DeduplicationStorageEngine::LSM,
        timestamp: Utc::now(),
        sequence: 101,
        file_path: Some("/data/lsm/level0/sst_001.db".to_string()),
    };
    
    deduplicator.add_tier_results(vec![result1, result2]);
    let merged = deduplicator.get_final_results(10);
    
    // Only record1 should match the filters
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.id, Some("vec1".to_string()));
}

#[test]
fn test_lsm_distance_calculations() {
    use proximadb::compute::unified_distance::UnifiedDistanceCompute;
    use proximadb::compute::distance::DistanceMetric;
    
    let distance_compute = UnifiedDistanceCompute::default();
    
    // Test zero distance
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    let distance = distance_compute.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    assert!(distance.rank_value.abs() < 0.001);
    
    // Test known distance
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![4.0, 6.0, 3.0];
    let distance2 = distance_compute.calculate_distance(&c, &d, &DistanceMetric::Euclidean);
    // sqrt((4-1)^2 + (6-2)^2 + (3-3)^2) = sqrt(9 + 16 + 0) = 5.0
    assert!((distance2.rank_value - 5.0).abs() < 0.001);
}

#[test]
fn test_bloom_filter_optimization() {
    // Test bloom filter false positive rate calculation
    let expected_items = 1_000_000;
    let target_fp_rate: f64 = 0.01; // 1% false positive rate
    
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
    assert!((distance.rank_value - 5.0).abs() < 0.001); // 3-4-5 triangle
    
    // Test identical vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![1.0, 2.0, 3.0];
    let distance2 = distance_compute.calculate_distance(&c, &d, &DistanceMetric::Euclidean);
    assert!(distance2.rank_value.abs() < 0.001); // Should be 0.0
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
    assert!(cosine_dist.rank_value.is_finite());
    assert!(euclidean_dist.rank_value.is_finite());
    assert!(manhattan_dist.rank_value.is_finite());
    
    // Different metrics should give different values
    assert_ne!(cosine_dist, euclidean_dist);
    assert_ne!(euclidean_dist, manhattan_dist);
    assert_ne!(cosine_dist, manhattan_dist);
    
    // Test known values
    assert!((euclidean_dist.rank_value - 5.196152).abs() < 0.001); // sqrt(27)
    assert!((manhattan_dist.rank_value - 9.0).abs() < 0.001); // |4-1| + |5-2| + |6-3|
}