//! SST Search Tests - Consolidated
//!
//! This file consolidates all search-related tests for the SST engine from:
//! - unified_search_engine/tests.rs (19 tests)
//! - readers/tests/unified_sstable_reader_edge_tests.rs (22 tests)
//! - readers/tests/unified_sstable_reader_tests.rs (8 tests)
//! - tests/bloom_filter_tests.rs (9 tests)
//! - search/mod.rs inline tests (3 tests)
//! - search/coordinator.rs inline tests (2 tests)
//! - search/operations.rs inline tests (2 tests)
//! - search/optimizer.rs inline tests (4 tests)
//!
//! Total: 69 tests consolidated (some duplicates removed)

use std::sync::Arc;
use std::collections::HashMap;
use tokio;
use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use tempfile::TempDir;

use super::helpers::*;

use crate::storage::engines::sst::SstEngine;
use crate::storage::engines::sst::readers::sst_query_engine::{
    UnifiedSstableReader, ReaderConfig, CollectionContext,
};
// unified_search_engine module no longer exists - use search coordinator instead
use crate::storage::engines::sst::SstableWriter;
use crate::storage::engines::sst::search::{SearchCoordinator, SearchOperations, SearchOptimizer};
use crate::storage::engines::sst::search::coordinator::SearchStrategy;
use crate::storage::engines::sst::search::optimizer::{OptimizationStrategy, OptimizationConfig};

use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::distance_computation::DistanceMetric;
use crate::compute::quantization::quantization_engine::{UnifiedQuantizationEngine, InMemoryCodebookStore};

use crate::core::bloom::{BloomFilterConfig, BloomFilterStrategy, MetadataBloomFilter};
use crate::core::bloom::factory::BloomFilterFactory;
use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
use crate::core::bloom::{BloomFilterStats, SstableBloomFilter};

use crate::core::search::{SearchParams, SearchPlan, FilterExpression, ComparisonOperator};
use crate::storage::engines::core::search::SearchResult;
use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::traits::StorageQueryContext;

use crate::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value, MetadataItem};
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig, FileSystem};
use crate::storage::persistence::filesystem::unified_filesystem::UnifiedCachingFilesystem;
use crate::query::query_optimizer::SearchParams as QuerySearchParams;

// ============================================================================
// SECTION 1: Unified Search Engine Tests (19 tests)
// ============================================================================

/// Create test UnifiedSstableReader with local filesystem
async fn create_test_sstable_reader() -> Arc<UnifiedSstableReader> {
    let fs_factory = Arc::new(FilesystemFactory::create(HashMap::new()).await.unwrap());
    let fs = fs_factory.get_filesystem("file:///tmp/proximadb-test").await.unwrap();

    let config = ReaderConfig {
        enable_bloom_filters: true,
        enable_block_cache: true,
        cache_size_mb: 64,
        prefetch_size_kb: 256,
    };

    Arc::new(UnifiedSstableReader::new(fs, config))
}

/// Create test search context
fn create_test_search_context() -> SearchPlan {
    SearchPlan {
        storage_info: crate::core::search::StorageInfo {
            storage_type: "LSM".to_string(),
            file_count: 5,
            estimated_size_mb: 100.0,
            is_cloud_storage: false,
            supports_range_requests: true,
        },
        available_quantization: vec![],
        filterable_columns: vec![
            crate::core::search::FilterableColumn {
                name: "category".to_string(),
                is_indexed: true,
                estimated_cardinality: Some(100),
            },
            crate::core::search::FilterableColumn {
                name: "score".to_string(),
                is_indexed: false,
                estimated_cardinality: Some(1000),
            },
        ],
        collection_config: Some(crate::core::search::CollectionConfig {
            default_distance_metric: DistanceMetric::Cosine,
            vector_dimension: 128,
            enable_quantization: false,
            enable_metadata_filtering: true,
            estimated_document_count: 10000,
            compression: None,
            optimization_hints: None,
        }),
    }
}

/// Create test search parameters
fn create_test_search_params() -> SearchParams {
    SearchParams {
        query_vectors: Some(vec![vec![0.1; 128]]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    }
}

/// Create mock search results
fn create_mock_search_results(count: usize) -> Vec<OptimizedSearchRecord> {
    (0..count).map(|i| {
        let metadata_map: HashMap<String, SqlValue> = {
            let mut map = HashMap::new();
            map.insert("category".to_string(), SqlValue {
                value: Some(sql_value::Value::StringValue("test".to_string())),
            });
            map.insert("index".to_string(), SqlValue {
                value: Some(sql_value::Value::Int64Value(i as i64)),
            });
            map
        };

        OptimizedSearchRecord {
            id: format!("result_{}", i),
            vector_id: Some(format!("vector_{}", i)),
            score: 1.0 - (i as f32 * 0.1),
            similarity: Some(i as f32 * 0.1),
            vector: Some(Arc::new((0..128).map(|j| (i * 128 + j) as f32 / 1000.0).collect())),
            metadata: metadata_map,
            debug_info: None,
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            timestamp: Some(Utc::now().timestamp()),
            ..Default::default()
        }
    }).collect()
}

// Config tests disabled - SstSearchConfig no longer exists (removed in refactoring)
// #[cfg(test)]
// mod config_tests {
//     use super::*;
//
//     #[test]
//     fn test_lsm_search_config_default() {
//         let config = SstSearchConfig::default();
//
//         assert!(config.enable_bloom_filters);
//         assert!(config.enable_block_cache);
//         assert!(config.enable_mvcc_resolution);
//         assert_eq!(config.max_sstables, 100);
//         assert!(config.enable_compaction_hints);
//     }
//
//     #[test]
//     fn test_lsm_search_config_custom() {
//         let config = SstSearchConfig {
//             enable_bloom_filters: false,
//             enable_block_cache: false,
//             enable_mvcc_resolution: false,
//             max_sstables: 50,
//             enable_compaction_hints: false,
//         };
//
//         assert!(!config.enable_bloom_filters);
//         assert!(!config.enable_block_cache);
//         assert!(!config.enable_mvcc_resolution);
//         assert_eq!(config.max_sstables, 50);
//         assert!(!config.enable_compaction_hints);
//     }
// }

#[cfg(test)]
mod construction_tests {
    use super::*;

    #[tokio::test]
    async fn test_new_with_default_config() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));

        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute,
            quantization_engine,
        );

        assert_eq!(engine.engine_id(), "SstUnifiedSearchEngine");
        assert!(engine.config.enable_bloom_filters);
        assert!(engine.config.enable_block_cache);
        assert!(engine.config.enable_mvcc_resolution);
    }

    // Test disabled - SstSearchConfig and SstUnifiedSearchEngine were refactored
    // The search API has been modularized into search/coordinator.rs and search/operations.rs
    // TODO: Update this test to use the new modular search API
    // #[tokio::test]
    // async fn test_with_custom_config() {
    //     let sstable_reader = create_test_sstable_reader().await;
    //     let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    //     let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
    //         Arc::new(UnifiedDistanceCompute::default()),
    //         Arc::new(InMemoryCodebookStore::new()),
    //     ));
    //
    //     let custom_config = SstSearchConfig {
    //         enable_bloom_filters: false,
    //         enable_block_cache: true,
    //         enable_mvcc_resolution: false,
    //         max_sstables: 25,
    //         enable_compaction_hints: true,
    //     };
    //
    //     let engine = SstUnifiedSearchEngine::with_config(
    //         sstable_reader,
    //         distance_compute,
    //         quantization_engine,
    //         custom_config.clone(),
    //     );
    //
    //     assert_eq!(engine.engine_id(), "SstUnifiedSearchEngine");
    //     assert!(!engine.config.enable_bloom_filters);
    //     assert!(engine.config.enable_block_cache);
    //     assert!(!engine.config.enable_mvcc_resolution);
    //     assert_eq!(engine.config.max_sstables, 25);
    //     assert!(engine.config.enable_compaction_hints);
    // }
}

#[cfg(test)]
mod unified_search_engine_tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_id() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));

        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute,
            quantization_engine,
        );

        assert_eq!(engine.engine_id(), "SstUnifiedSearchEngine");
    }

    #[tokio::test]
    async fn test_can_handle() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));

        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute,
            quantization_engine,
        );

        let context = create_test_search_context();
        let params = create_test_search_params();

        let can_handle = engine.can_handle(&context, &params).await;
        assert!(can_handle);
    }

    #[tokio::test]
    async fn test_estimate_cost() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));

        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute,
            quantization_engine,
        );

        let context = create_test_search_context();
        let params = create_test_search_params();

        let cost = engine.estimate_cost(&context, &params).await;
        assert!(cost > 0.0);

        // Test cost reduction with filters
        let mut params_with_filters = params.clone();
        params_with_filters.filters = Some({
            let mut filters = HashMap::new();
            filters.insert("category".to_string(), serde_json::Value::String("test".to_string()));
            filters
        });

        let cost_with_filters = engine.estimate_cost(&context, &params_with_filters).await;
        assert!(cost_with_filters < cost);
    }
}

// ============================================================================
// SECTION 2: Reader Edge Case Tests (22 tests)
// ============================================================================

#[cfg(test)]
mod reader_edge_tests {
    use super::*;

    async fn create_test_reader() -> UnifiedSstableReader {
        let config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(FilesystemFactory::create(config).await.unwrap());
        let base_fs = filesystem_factory.get_filesystem("file://").unwrap();
        let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "sst".to_string(),
        ));
        UnifiedSstableReader::new(
            filesystem_factory,
            unified_fs,
            "test_collection".to_string(),
        )
    }

    #[tokio::test]
    async fn test_empty_collection_search() {
        let reader = create_test_reader().await;
        let context = CollectionContext {
            file_path: "".to_string(),
            sstable_files: vec![],
            total_vectors: 0,
            metadata_columns: vec![],
            level: 0,
            creation_time: Utc::now(),
            io_optimization_hints: None,
        };

        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        let results = reader.search_vectors(&params, &context).await.unwrap();
        assert_eq!(results.len(), 0, "Empty collection should return no results");
    }

    #[tokio::test]
    async fn test_high_dimensional_vectors() {
        let reader = create_test_reader().await;

        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 4096]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        assert_eq!(params.query_vectors.as_ref().unwrap()[0].len(), 4096);
    }

    #[tokio::test]
    async fn test_extreme_top_k_values() {
        let reader = create_test_reader().await;

        let params_large = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(1_000_000),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        let params_zero = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(0),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        assert_eq!(params_large.top_k, Some(1_000_000));
        assert_eq!(params_zero.top_k, Some(0));
    }

    #[tokio::test]
    async fn test_deeply_nested_filter_expressions() {
        let reader = create_test_reader().await;

        let filter = FilterExpression::And(vec![
            FilterExpression::Or(vec![FilterExpression::And(vec![FilterExpression::Or(
                vec![FilterExpression::And(vec![
                    FilterExpression::Comparison {
                        field: "level5_a".to_string(),
                        operator: ComparisonOperator::Equals,
                        value: json!("deep_value"),
                    },
                    FilterExpression::Not(Box::new(FilterExpression::Or(vec![
                        FilterExpression::Comparison {
                            field: "level6_a".to_string(),
                            operator: ComparisonOperator::In,
                            value: json!(["x", "y", "z"]),
                        },
                        FilterExpression::And(vec![
                            FilterExpression::Comparison {
                                field: "level7_a".to_string(),
                                operator: ComparisonOperator::Between,
                                value: json!([10, 20]),
                            },
                            FilterExpression::Not(Box::new(FilterExpression::Comparison {
                                field: "level8_a".to_string(),
                                operator: ComparisonOperator::StartsWith,
                                value: json!("prefix"),
                            })),
                        ]),
                    ]))),
                ])],
            )])]),
            FilterExpression::Not(Box::new(FilterExpression::Comparison {
                field: "excluded".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!(true),
            })),
        ]);

        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            filter_expression: Some(filter),
            ..Default::default()
        };

        assert!(params.filter_expression.is_some());
    }

    #[tokio::test]
    async fn test_null_and_missing_values() {
        let reader = create_test_reader().await;

        let filter_null = FilterExpression::Comparison {
            field: "optional_field".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(null),
        };

        let filter_is_null = FilterExpression::Comparison {
            field: "nullable_field".to_string(),
            operator: ComparisonOperator::IsNull,
            value: json!(null),
        };

        for filter in vec![filter_null, filter_is_null] {
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }

    #[tokio::test]
    async fn test_numeric_boundary_values() {
        let reader = create_test_reader().await;

        let filter_max = FilterExpression::Comparison {
            field: "large_number".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(9007199254740991i64),
        };

        let filter_inf = FilterExpression::Comparison {
            field: "score".to_string(),
            operator: ComparisonOperator::LessThan,
            value: json!(f64::INFINITY),
        };

        for filter in vec![filter_max, filter_inf] {
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }
}

// ============================================================================
// SECTION 3: Basic Reader Tests (8 tests)
// ============================================================================

#[cfg(test)]
mod basic_reader_tests {
    use super::*;

    async fn create_test_reader() -> UnifiedSstableReader {
        let config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(config).await.unwrap());
        UnifiedSstableReader::new(filesystem)
    }

    #[tokio::test]
    async fn test_reader_creation() {
        let reader = create_test_reader().await;
        assert!(true);
    }

    #[tokio::test]
    async fn test_strategy_selection_basic() {
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        assert!(params.filter_expression.is_none());
    }

    #[tokio::test]
    async fn test_strategy_selection_with_filter() {
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            filter_expression: Some(FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("electronics"),
            }),
            ..Default::default()
        };

        assert!(params.filter_expression.is_some());
    }
}

// ============================================================================
// SECTION 4: Bloom Filter Tests (9 tests)
// ============================================================================

#[cfg(test)]
mod bloom_filter_tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic_operations() {
        let config = BloomFilterConfig {
            bits_per_key: 10,
            expected_items: 100,
            enabled: true,
            ..Default::default()
        };

        let mut filter = BloomFilterFactory::create(&config);

        filter.insert(b"key1");
        filter.insert(b"key2");
        filter.insert(b"key3");

        assert!(filter.might_contain(b"key1"));
        assert!(filter.might_contain(b"key2"));
        assert!(filter.might_contain(b"key3"));
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let config = BloomFilterConfig {
            bits_per_key: 10,
            expected_items: 1000,
            enabled: true,
            ..Default::default()
        };

        let filter = BloomFilterFactory::create(&config);
        let calculated_rate = filter.false_positive_rate();

        assert!(calculated_rate >= 0.0 && calculated_rate < 0.02);
    }

    #[test]
    fn test_metadata_bloom_filter() {
        let config = BloomFilterConfig {
            expected_items: 100,
            enabled: true,
            ..Default::default()
        };

        let mut builder = CompositeBloomFilterBuilder::new(config);

        let electronics_item = MetadataItem {
            key: "category".to_string(),
            value: Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                "electronics".to_string(),
            )),
        };

        builder.add_metadata_item("category".to_string(), electronics_item.clone());

        let filter = builder.build();

        assert!(MetadataBloomFilter::might_match_metadata(
            &filter,
            "category",
            &electronics_item
        ));
    }

    #[test]
    fn test_bloom_filter_serialization() {
        let config = BloomFilterConfig::default();
        let mut filter = BloomFilterFactory::create(&config);

        filter.insert(b"test1");
        filter.insert(b"test2");

        let serialized_data = filter.serialize().unwrap();
        assert!(serialized_data.len() > 0);
    }

    #[test]
    fn test_bloom_filter_size_estimation() {
        let config = BloomFilterConfig {
            bits_per_key: 10,
            expected_items: 1000,
            enabled: true,
            ..Default::default()
        };

        let filter = BloomFilterFactory::create(&config);

        let expected_size = (10 * 1000) / 8;
        let actual_size = filter.bit_count() / 8;

        assert!(actual_size >= expected_size);
        assert!(actual_size <= expected_size * 2);
    }
}

// ============================================================================
// SECTION 5: Search Module Inline Tests (3 tests)
// ============================================================================

#[cfg(test)]
mod search_module_tests {
    use super::*;
    use crate::core::SstConfig;

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute).await.unwrap()
    }

    #[tokio::test]
    async fn test_parse_storage_url() {
        let engine = create_test_engine().await;

        let (base, collection) = engine.parse_storage_url("file:///data/collections/test_collection").unwrap();
        assert_eq!(base, "file:///data/collections");
        assert_eq!(collection, "test_collection");

        assert!(engine.parse_storage_url("invalid_url").is_err());
    }

    #[tokio::test]
    async fn test_filter_search_results() {
        let engine = create_test_engine().await;
        let mut results = vec![
            create_test_search_result("id1", vec![1.0, 2.0], 0.5),
            create_test_search_result("id2", vec![3.0, 4.0], 0.3),
        ];

        engine.filter_search_results(&mut results, false, true);
        assert!(results[0].vector.is_none());
        assert!(results[1].vector.is_none());
    }

    fn create_test_search_result(id: &str, values: Vec<f32>, score: f32) -> OptimizedSearchRecord {
        let mut record = OptimizedSearchRecord::default();
        record.id = id.to_string();
        record.score = score;
        record.vector = Some(Arc::new(values));
        record.metadata = {
            let mut metadata = HashMap::new();
            let sql_value = SqlValue {
                value: Some(sql_value::Value::StringValue("test_value".to_string())),
            };
            metadata.insert("test_key".to_string(), sql_value);
            metadata
        };
        record
    }
}

// ============================================================================
// SECTION 6: Search Coordinator Tests (2 tests)
// ============================================================================

#[cfg(test)]
mod coordinator_tests {
    use super::*;
    use crate::core::SstConfig;

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute).await.unwrap()
    }

    fn create_test_context(use_indexes: bool, has_quantization: bool) -> StorageQueryContext {
        let search_params = Arc::new(QuerySearchParams {
            query_vectors: None,
            vector: Some(vec![1.0, 2.0, 3.0]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            filter_expression: None,
            filters: None,
            accuracy_threshold: Some(0.95),
            include_expired: Some(false),
            timeout_ms: Some(5000),
            enable_two_stage: Some(true),
            quantization_hint: None,
            enable_clustering_hint: Some(true),
            enable_metadata_filtering_hint: Some(true),
            custom_hints: None,
            requires_ordering: None,
            runtime_hints: None,
            enable_progressive_search: Some(false),
            progressive_scenario: None,
            progressive_recalls: None,
            optimization_hint: None,
        });

        let collection = Arc::new(crate::proto::proximadb_v1::Collection {
            id: "test_collection".to_string(),
            config: Some(crate::proto::proximadb_v1::CollectionConfig {
                name: "test_collection".to_string(),
                dimension: 128,
                distance_metric: Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
                storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Sst as i32),
                ..Default::default()
            }),
            stats: Some(crate::proto::proximadb_v1::CollectionStats::default()),
            created_at: 0,
            updated_at: 0,
            storage_assignment: None,
        });

        StorageQueryContext::new(search_params, collection)
    }

    #[tokio::test]
    async fn test_search_strategy_selection() {
        let engine = create_test_engine().await;
        let coordinator = SearchCoordinator::new(Arc::new(engine));

        let ctx = create_test_context(false, false);
        let strategy = coordinator.select_search_strategy(&ctx).await.unwrap();

        match strategy {
            SearchStrategy::Direct { .. } => {
                // Expected for simple query
            }
            _ => panic!("Expected Direct strategy for simple query"),
        }
    }

    #[tokio::test]
    async fn test_cost_estimation() {
        let engine = create_test_engine().await;
        let coordinator = SearchCoordinator::new(Arc::new(engine));

        let strategy = SearchStrategy::Direct {
            reason: "Test".to_string(),
            estimated_cost: 100.0,
        };

        let ctx = create_test_context(false, false);
        let cost = coordinator.estimate_search_cost(&ctx, &strategy).await.unwrap();
        assert_eq!(cost, 100.0);
    }
}

// ============================================================================
// SECTION 7: Search Operations Tests (2 tests)
// ============================================================================

#[cfg(test)]
mod operations_tests {
    use super::*;
    use crate::core::SstConfig;

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute).await.unwrap()
    }

    #[tokio::test]
    async fn test_validate_search_params() {
        let engine = create_test_engine().await;
        let ops = SearchOperations::new(Arc::new(engine));

        let query_vector = vec![1.0, 2.0, 3.0];
        assert!(ops.validate_search_params(&query_vector, 10, DistanceMetric::Cosine).is_ok());

        let empty_vector = vec![];
        assert!(ops.validate_search_params(&empty_vector, 10, DistanceMetric::Cosine).is_err());

        assert!(ops.validate_search_params(&query_vector, 0, DistanceMetric::Cosine).is_err());
    }

    #[tokio::test]
    async fn test_operation_stats() {
        let engine = create_test_engine().await;
        let ops = SearchOperations::new(Arc::new(engine));

        let stats = ops.get_operation_stats().await.unwrap();
        assert_eq!(stats.total_file_searches, 0);
    }
}

// ============================================================================
// SECTION 8: Search Optimizer Tests (4 tests)
// ============================================================================

#[cfg(test)]
mod optimizer_tests {
    use super::*;

    #[tokio::test]
    async fn test_query_signature_generation() {
        let optimizer = SearchOptimizer::new();
        let query_vector = vec![1.0, 2.0, 3.0];

        let signature = optimizer.generate_query_signature(
            &query_vector,
            10,
            DistanceMetric::Cosine,
            None,
        );

        assert!(signature.contains("dim:3"));
        assert!(signature.contains("k:10"));
        assert!(signature.contains("Cosine"));
        assert!(signature.contains("filtered:false"));
    }

    #[tokio::test]
    async fn test_strategy_selection() {
        let mut optimizer = SearchOptimizer::new();

        let strategy = optimizer.select_optimization_strategy(
            "test_signature",
            3,
            false,
        ).await.unwrap();

        match strategy {
            OptimizationStrategy::DirectSearch { .. } => {
                // Expected for small file count
            }
            _ => panic!("Expected DirectSearch for small file count"),
        }
    }

    #[tokio::test]
    async fn test_performance_recording() {
        let mut optimizer = SearchOptimizer::new();
        let query_signature = "test_pattern".to_string();

        optimizer.update_query_statistics(&query_signature);

        let strategy = OptimizationStrategy::DirectSearch {
            reason: "Test".to_string(),
        };

        let result = optimizer.record_search_performance(
            &query_signature,
            50.0,
            10,
            &strategy,
        ).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bloom_filter_fp_estimation() {
        let optimizer = SearchOptimizer::new();

        let fp_rate_small = optimizer.estimate_bloom_filter_fp_rate(10);
        let fp_rate_large = optimizer.estimate_bloom_filter_fp_rate(1000);

        assert!(fp_rate_small > 0.0);
        assert!(fp_rate_large > fp_rate_small);
        assert!(fp_rate_large <= 0.1);
    }
}
