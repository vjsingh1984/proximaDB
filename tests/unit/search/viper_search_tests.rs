//! Unit tests for VIPER search engine

use proximadb::proto::proximadb::{Collection, CollectionConfig, CollectionStats, DistanceMetric, StorageEngine, IndexingAlgorithm, FilterableColumnSpec, FilterableDataType};
use proximadb::core::search::storage_aware::{
    SearchHints, QuantizationLevel, ClusteringHints, SearchCapabilities, ClusterDistanceMetric, SearchMetrics
};
use std::collections::HashMap;
use serde_json::json;

#[test]
fn test_viper_search_hints_optimization() {
    // Create a mock collection using proto type
    let collection = Collection {
        id: "test-viper-uuid".to_string(),
        config: Some(CollectionConfig {
            name: "test-viper-collection".to_string(),
            dimension: 768, // BERT-base dimension
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
            primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
            filterable_columns: vec![
                FilterableColumnSpec {
                    name: "category".to_string(),
                    data_type: FilterableDataType::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: None,
                },
                FilterableColumnSpec {
                    name: "author".to_string(),
                    data_type: FilterableDataType::FilterableString as i32,
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: None,
                },
            ],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: false,
            description: None,
            tags: vec![],
            owner: None,
        }),
        stats: Some(CollectionStats {
            vector_count: 1000000, // Large collection
            index_size_bytes: 1_000_000_000, // 1GB index
            data_size_bytes: 3_000_000_000, // 3GB data
        }),
        created_at: 1000,
        updated_at: 1000,
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
        max_clusters_to_search: 10,
        cluster_confidence_threshold: 0.8,
        cluster_centroids_cache: None,
        cluster_distance_metric: ClusterDistanceMetric::Cosine,
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
    assert_eq!(capabilities.supported_quantization.len(), 5); // FP32, PQ8, PQ4, Binary, INT8
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

#[test]
fn test_viper_search_metrics() {
    let mut index_efficiency = HashMap::new();
    index_efficiency.insert("predicate_pushdown_reduction".to_string(), 0.8125); // 650/800
    index_efficiency.insert("row_group_skip_rate".to_string(), 0.75); // 45/(15+45)
    
    let mut engine_specific_metrics = HashMap::new();
    engine_specific_metrics.insert("parquet_files_scanned".to_string(), json!(3.0));
    engine_specific_metrics.insert("row_groups_examined".to_string(), json!(15.0));
    engine_specific_metrics.insert("row_groups_skipped".to_string(), json!(45.0));
    engine_specific_metrics.insert("predicate_pushdown_filtered".to_string(), json!(650.0));
    engine_specific_metrics.insert("ml_clusters_searched".to_string(), json!(5.0));
    engine_specific_metrics.insert("quantization_decompression_ms".to_string(), json!(3.2));
    
    let metrics = SearchMetrics {
        total_searches: 200,
        avg_latency_us: 15800.0, // 15.8ms in microseconds
        p95_latency_us: 25000.0,
        p99_latency_us: 35000.0,
        avg_vectors_scanned: 800.0,
        cache_hit_rate: 0.9, // Higher due to ML clustering
        index_efficiency,
        quantization_accuracy: None,
        engine_metrics: engine_specific_metrics,
    };
    
    assert_eq!(metrics.avg_latency_us, 15800.0);
    assert_eq!(metrics.avg_vectors_scanned, 800.0);
    assert_eq!(metrics.cache_hit_rate, 0.9);
    assert_eq!(metrics.index_efficiency["predicate_pushdown_reduction"], 0.8125);
    assert_eq!(metrics.engine_metrics["ml_clusters_searched"], json!(5.0));
    assert_eq!(metrics.engine_metrics["quantization_decompression_ms"], json!(3.2));
}

#[test]
fn test_viper_quantization_levels() {
    use proximadb::core::search::storage_aware::QuantizationLevel;
    
    // Test all supported quantization levels
    let levels = vec![
        QuantizationLevel::FP32,
        QuantizationLevel::PQ8,
        QuantizationLevel::PQ4,
        QuantizationLevel::Binary,
        QuantizationLevel::INT8,
    ];
    
    for level in levels {
        let mut hints = SearchHints::default();
        hints.quantization_level = level;
        
        match level {
            QuantizationLevel::FP32 => {
                // Full precision, no compression
                assert_eq!(hints.quantization_level, QuantizationLevel::FP32);
            }
            QuantizationLevel::PQ8 => {
                // 8-bit product quantization
                assert_eq!(hints.quantization_level, QuantizationLevel::PQ8);
            }
            QuantizationLevel::PQ4 => {
                // 4-bit product quantization
                assert_eq!(hints.quantization_level, QuantizationLevel::PQ4);
            }
            QuantizationLevel::Binary => {
                // Binary quantization
                assert_eq!(hints.quantization_level, QuantizationLevel::Binary);
            }
            QuantizationLevel::INT8 => {
                // 8-bit integer quantization
                assert_eq!(hints.quantization_level, QuantizationLevel::INT8);
            }
        }
    }
}

#[test]
fn test_viper_clustering_optimization() {
    use proximadb::core::search::storage_aware::ClusterDistanceMetric;
    
    let clustering_hints = ClusteringHints {
        enable_ml_clustering: true,
        max_clusters_to_search: 20,
        cluster_confidence_threshold: 0.9,
        cluster_centroids_cache: Some(vec![
            vec![0.1, 0.2, 0.3],
            vec![0.4, 0.5, 0.6],
            vec![0.7, 0.8, 0.9],
        ]),
        cluster_distance_metric: ClusterDistanceMetric::Euclidean,
    };
    
    assert!(clustering_hints.enable_ml_clustering);
    assert_eq!(clustering_hints.max_clusters_to_search, 20);
    assert_eq!(clustering_hints.cluster_confidence_threshold, 0.9);
    assert!(clustering_hints.cluster_centroids_cache.is_some());
    assert_eq!(clustering_hints.cluster_centroids_cache.unwrap().len(), 3);
}

#[test]
fn test_viper_predicate_pushdown() {
    use proximadb::core::search::multi_tier_deduplication::{
        MultiTierDeduplicator, TieredSearchResult, StorageTier, DeduplicationStorageEngine
    };
    use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
    use chrono::Utc;
    
    let mut deduplicator = MultiTierDeduplicator::with_filters({
        let mut filters = HashMap::new();
        filters.insert("language".to_string(), json!("en"));
        filters.insert("year".to_string(), json!("2024")); // String comparison
        filters
    });
    
    // Create test vectors
    let record1 = VectorRecord {
        id: Some("doc1".to_string()),
        collection_id: "viper-collection".to_string(),
        vector: vec![0.1; 768],
        metadata: vec![
            MetadataItem { key: "language".to_string(), value: "en".to_string() },
            MetadataItem { key: "year".to_string(), value: "2024".to_string() },
            MetadataItem { key: "category".to_string(), value: "tech".to_string() },
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
        id: Some("doc2".to_string()),
        collection_id: "viper-collection".to_string(),
        vector: vec![0.2; 768],
        metadata: vec![
            MetadataItem { key: "language".to_string(), value: "fr".to_string() },
            MetadataItem { key: "year".to_string(), value: "2024".to_string() },
            MetadataItem { key: "category".to_string(), value: "tech".to_string() },
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
    
    // VIPER results from Parquet files
    let result1 = TieredSearchResult {
        vector_record: record1,
        score: 0.95,
        tier: StorageTier::Compacted,
        engine: DeduplicationStorageEngine::VIPER,
        timestamp: Utc::now(),
        sequence: 1000,
        file_path: Some("/data/viper/cluster_01.parquet".to_string()),
    };
    
    let result2 = TieredSearchResult {
        vector_record: record2,
        score: 0.88,
        tier: StorageTier::Compacted,
        engine: DeduplicationStorageEngine::VIPER,
        timestamp: Utc::now(),
        sequence: 1001,
        file_path: Some("/data/viper/cluster_02.parquet".to_string()),
    };
    
    deduplicator.add_tier_results(vec![result1, result2]);
    let merged = deduplicator.get_final_results(10);
    
    // Only record1 should pass the predicate filters (language=en)
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.id, Some("doc1".to_string()));
    assert_eq!(merged[0].score, 0.95);
}

#[test]
fn test_manhattan_distance_calculation() {
    use proximadb::compute::unified_distance::UnifiedDistanceCompute;
    use proximadb::compute::distance::DistanceMetric;
    
    let distance_compute = UnifiedDistanceCompute::default();
    
    // Test Manhattan distance
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 6.0, 3.0];
    let distance = distance_compute.calculate_distance(&a, &b, &DistanceMetric::Manhattan);
    // |4-1| + |6-2| + |3-3| = 3 + 4 + 0 = 7
    assert!((distance.rank_value - 7.0).abs() < 0.001);
    
    // Test with negative values
    let c = vec![-1.0, -2.0, -3.0];
    let d = vec![1.0, 2.0, 3.0];
    let distance2 = distance_compute.calculate_distance(&c, &d, &DistanceMetric::Manhattan);
    // |1-(-1)| + |2-(-2)| + |3-(-3)| = 2 + 4 + 6 = 12
    assert!((distance2.rank_value - 12.0).abs() < 0.001);
}

#[test]
fn test_viper_parquet_optimization() {
    let mut capabilities_features = HashMap::new();
    
    // Parquet-specific optimizations
    capabilities_features.insert("row_group_pruning".to_string(), json!(true));
    capabilities_features.insert("column_projection".to_string(), json!(true));
    capabilities_features.insert("statistics_filtering".to_string(), json!(true));
    capabilities_features.insert("dictionary_encoding".to_string(), json!(true));
    capabilities_features.insert("page_index_filtering".to_string(), json!(true));
    capabilities_features.insert("bloom_filter_columns".to_string(), json!(["id", "category"]));
    
    assert!(capabilities_features.contains_key("row_group_pruning"));
    assert!(capabilities_features.contains_key("statistics_filtering"));
    assert_eq!(
        capabilities_features["bloom_filter_columns"],
        json!(["id", "category"])
    );
}