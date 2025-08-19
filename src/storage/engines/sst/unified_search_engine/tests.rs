//! Unit tests for LSM Unified Search Engine
//! Tests core search functionality with comprehensive coverage

use std::sync::Arc;
use std::collections::HashMap;
use tokio;
use anyhow::Result;
use chrono::Utc;

use super::*;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::{UnifiedQuantizationEngine, InMemoryCodebookStore};
use crate::core::search::{SearchParams, UnifiedSearchContext};

use crate::storage::engines::sst::readers::unified_sstable_reader::{
    UnifiedSstableReader, ReaderConfig, CollectionContext,
};
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};

/// Create test UnifiedSstableReader with local filesystem
async fn create_test_sstable_reader() -> Arc<UnifiedSstableReader> {
    let fs_factory = Arc::new(FilesystemFactory::new(HashMap::new()));
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
fn create_test_search_context() -> UnifiedSearchContext {
    UnifiedSearchContext {
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
                // data_type removed -  crate::proto::proximadb::ColumnDataType::String,
                is_indexed: true,
                estimated_cardinality: 100,
            },
            crate::core::search::FilterableColumn {
                name: "score".to_string(),
                // data_type removed -  crate::proto::proximadb::ColumnDataType::Float,
                is_indexed: false,
                estimated_cardinality: 1000,
            },
        ],
        collection_config: Some(crate::core::search::CollectionConfig {
            default_distance_metric: crate::compute::distance_computation::DistanceMetric::Cosine,
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
        distance_metric: Some(crate::compute::distance_computation::DistanceMetric::Cosine),
        filters: None,
        filter_expression: None,
        accuracy_threshold: None,
        include_expired: Some(false),
        timeout_ms: None,
        enable_two_stage: Some(false),
        enable_clustering_hint: Some(false),
        enable_metadata_filtering_hint: Some(true),
        quantization_hint: None,
        custom_hints: None,
    }
}

/// Create mock search results
fn create_mock_search_results(count: usize) -> Vec<crate::core::search::SearchResult> {
    (0..count).map(|i| {
        crate::core::search::SearchResult {
            id: format!("result_{}", i),
            vector_id: Some(format!("vector_{}", i)),
            similarity: 1.0 - (i as f32 * 0.1),
            similarity: Some(i as f32 * 0.1),
            // rank removed -  Some(i as u16 + 1),
            vector: Some((0..128).map(|j| (i * 128 + j) as f32 / 1000.0).collect()),
            metadata: {
                let mut map = HashMap::new();
                map.insert("category".to_string(), serde_json::Value::String("test".to_string()));
                map.insert("index".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                map
            },
            debug_info: None,
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            timestamp: Some(Utc::now()),
        }
    }).collect()
}

/// Test SstSearchConfig functionality
#[cfg(test)]
mod config_tests {
    use super::*;
    
    #[test]
    fn test_lsm_search_config_default() {
        let config = SstSearchConfig::default();
        
        assert!(config.enable_bloom_filters);
        assert!(config.enable_block_cache);
        assert!(config.enable_mvcc_resolution);
        assert_eq!(config.max_sstables, 100);
        assert!(config.enable_compaction_hints);
    }
    
    #[test]
    fn test_lsm_search_config_custom() {
        let config = SstSearchConfig {
            enable_bloom_filters: false,
            enable_block_cache: false,
            enable_mvcc_resolution: false,
            max_sstables: 50,
            enable_compaction_hints: false,
        };
        
        assert!(!config.enable_bloom_filters);
        assert!(!config.enable_block_cache);
        assert!(!config.enable_mvcc_resolution);
        assert_eq!(config.max_sstables, 50);
        assert!(!config.enable_compaction_hints);
    }
}

/// Test SstUnifiedSearchEngine construction
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
    
    #[tokio::test]
    async fn test_with_custom_config() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        let custom_config = SstSearchConfig {
            enable_bloom_filters: false,
            enable_block_cache: true,
            enable_mvcc_resolution: false,
            max_sstables: 25,
            enable_compaction_hints: true,
        };
        
        let engine = SstUnifiedSearchEngine::with_config(
            sstable_reader,
            distance_compute,
            quantization_engine,
            custom_config.clone(),
        );
        
        assert_eq!(engine.engine_id(), "SstUnifiedSearchEngine");
        assert!(!engine.config.enable_bloom_filters);
        assert!(engine.config.enable_block_cache);
        assert!(!engine.config.enable_mvcc_resolution);
        assert_eq!(engine.config.max_sstables, 25);
        assert!(engine.config.enable_compaction_hints);
    }
}

/// Test UnifiedSearchEngine trait implementation
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
        assert!(can_handle); // LSM can handle all collections
    }
    
    #[tokio::test]
    async fn test_optimization_hints() {
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
        
        let mut context = create_test_search_context();
        context.storage_info.file_count = 15; // More than 10 to trigger hint
        
        let hints = engine.optimization_hints(&context).await;
        assert!(!hints.is_empty());
        
        // Should suggest metadata filtering for collections with many files
        let has_metadata_hint = hints.iter().any(|hint| {
            matches!(hint, crate::core::search::OptimizationHint::UseMetadataFiltering { .. })
        });
        assert!(has_metadata_hint);
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
        assert!(cost_with_filters < cost); // Filters should reduce cost
    }
}

/// Test search functionality
#[cfg(test)]
mod search_tests {
    use super::*;
    
    // Note: These tests require actual SSTable data to work properly
    // They're kept here as examples of how to test with real data
    
    /*
    #[tokio::test]
    async fn test_search_unified_success() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        // Note: Real SSTable reader tests would require actual data
        // For now, we're testing the engine construction and interface
        
        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine.clone(),
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let result = engine.search_unified(&context, &params, &distance_compute, Some(&quantization_engine)).await;
        
        assert!(result.is_ok());
        let search_result_set = result.unwrap();
        
        assert_eq!(search_result_set.results.len(), 5);
        assert_eq!(search_result_set.total_count, 5);
        assert!(search_result_set.processing_time_us > 0);
        assert_eq!(search_result_set.algorithm, "LSM-BloomOptimized");
    }
    */
    
    #[tokio::test]
    async fn test_search_unified_with_disabled_bloom_filters() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        let mock_results = create_mock_search_results(3);
        sstable_reader.set_search_results(mock_results);
        
        let config = SstSearchConfig {
            enable_bloom_filters: false,
            ..Default::default()
        };
        
        let engine = SstUnifiedSearchEngine::with_config(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine.clone(),
            config,
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let result = engine.search_unified(&context, &params, &distance_compute, Some(&quantization_engine)).await;
        
        assert!(result.is_ok());
        let search_result_set = result.unwrap();
        
        assert_eq!(search_result_set.algorithm, "LSM-Standard");
    }
    
    #[tokio::test]
    async fn test_search_unified_with_mvcc_resolution() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        // Create results with duplicate IDs but different versions
        let mut mock_results = Vec::new();
        for i in 0..3 {
            for version in 1..=2 {
                let mut result = crate::core::search::SearchResult {
                    id: format!("duplicate_{}", i),
                    vector_id: Some(format!("vector_{}", i)),
                    similarity: 1.0 - (i as f32 * 0.1),
                    similarity: Some(i as f32 * 0.1),
                    // rank removed -  Some(i as u16 + 1),
                    vector: Some((0..128).map(|j| (i * 128 + j) as f32 / 1000.0).collect()),
                    metadata: {
                        let mut map = HashMap::new();
                        map.insert("_version".to_string(), serde_json::Value::Number(serde_json::Number::from(version)));
                        map.insert("category".to_string(), serde_json::Value::String("test".to_string()));
                        map
                    },
                    debug_info: None,
                    semantic_similarity: None,
                    quantization_info: None,
                    engine_stats: None,
                    index_path: None,
                            timestamp: Some(Utc::now()),
                };
                mock_results.push(result);
            }
        }
        
        sstable_reader.set_search_results(mock_results);
        
        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine.clone(),
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let result = engine.search_unified(&context, &params, &distance_compute, Some(&quantization_engine)).await;
        
        assert!(result.is_ok());
        let search_result_set = result.unwrap();
        
        // Should have deduplicated results (only latest versions)
        assert_eq!(search_result_set.results.len(), 3);
        
        // Each result should be the version 2 (latest)
        for result in &search_result_set.results {
            let version = result.metadata.get(key)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            assert_eq!(version, 2);
        }
    }
    
    #[tokio::test]
    async fn test_search_unified_failure() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        // Make SSTable reader fail
        sstable_reader.set_should_fail(true);
        
        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine.clone(),
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let result = engine.search_unified(&context, &params, &distance_compute, Some(&quantization_engine)).await;
        
        assert!(result.is_err());
    }
}

/// Test private helper methods
#[cfg(test)]
mod helper_methods_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_discover_sstable_files() {
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
        
        let files = engine.discover_sstable_files(&context).await;
        assert!(files.is_ok());
        
        let file_list = files.unwrap();
        assert!(!file_list.is_empty());
        assert!(file_list.iter().any(|f| f.contains_hash("level0")));
        assert!(file_list.iter().any(|f| f.contains_hash("level1")));
    }
    
    #[tokio::test]
    async fn test_apply_optimization_hints() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        let config = SstSearchConfig {
            max_sstables: 2,
            ..Default::default()
        };
        
        let engine = SstUnifiedSearchEngine::with_config(
            sstable_reader,
            distance_compute,
            quantization_engine,
            config,
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let original_files = vec![
            "file1.sstable".to_string(),
            "file2.sstable".to_string(),
            "file3.sstable".to_string(),
            "file4.sstable".to_string(),
        ];
        
        let optimized_files = engine.apply_optimization_hints(original_files, &context, &params).await;
        assert!(optimized_files.is_ok());
        
        let file_list = optimized_files.unwrap();
        assert_eq!(file_list.len(), 2); // Should be limited by max_sstables
    }
    
    #[tokio::test]
    async fn test_apply_mvcc_resolution() {
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
        
        // Create test results with versions
        let results = vec![
            crate::core::search::SearchResult {
                id: "doc1".to_string(),
                vector_id: Some("vector1".to_string()),
                similarity: 0.9,
                similarity: Some(0.1),
                // rank removed -  Some(1),
                vector: None,
                metadata: {
                    let mut map = HashMap::new();
                    map.insert("_version".to_string(), serde_json::Value::Number(serde_json::Number::from(1)));
                    map
                },
                debug_info: None,
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: None,
                index_path: None,
                timestamp: Some(Utc::now()),
            },
            crate::core::search::SearchResult {
                id: "doc1".to_string(),
                vector_id: Some("vector1".to_string()),
                similarity: 0.8,
                similarity: Some(0.2),
                // rank removed -  Some(2),
                vector: None,
                metadata: {
                    let mut map = HashMap::new();
                    map.insert("_version".to_string(), serde_json::Value::Number(serde_json::Number::from(2)));
                    map
                },
                debug_info: None,
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: None,
                index_path: None,
                timestamp: Some(Utc::now()),
            },
            crate::core::search::SearchResult {
                id: "doc2".to_string(),
                vector_id: Some("vector2".to_string()),
                similarity: 0.7,
                similarity: Some(0.3),
                // rank removed -  Some(3),
                vector: None,
                metadata: {
                    let mut map = HashMap::new();
                    map.insert("_version".to_string(), serde_json::Value::Number(serde_json::Number::from(1)));
                    map
                },
                debug_info: None,
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: None,
                index_path: None,
                timestamp: Some(Utc::now()),
            },
        ];
        
        let resolved = engine.apply_mvcc_resolution(results);
        assert!(resolved.is_ok());
        
        let resolved_results = resolved.unwrap();
        assert_eq!(resolved_results.len(), 2); // doc1 (version 2) and doc2 (version 1)
        
        // First result should be doc1 with version 2 (higher score)
        assert_eq!(resolved_results[0].id, "doc1");
        let version = resolved_results[0].metadata.get(key)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(version, 2);
        
        // Results should be sorted by score descending
        assert!(resolved_results[0].score >= resolved_results[1].score);
    }
}

/// Test edge cases and error handling
#[cfg(test)]
mod edge_case_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_search_with_empty_results() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        // Set empty results
        sstable_reader.set_search_results(Vec::new());
        
        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine.clone(),
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let result = engine.search_unified(&context, &params, &distance_compute, Some(&quantization_engine)).await;
        
        assert!(result.is_ok());
        let search_result_set = result.unwrap();
        
        assert_eq!(search_result_set.results.len(), 0);
        assert_eq!(search_result_set.total_count, 0);
    }
    
    #[tokio::test]
    async fn test_mvcc_resolution_with_no_versions() {
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
        
        // Create results without version metadata
        let results = vec![
            crate::core::search::SearchResult {
                id: "doc1".to_string(),
                vector_id: Some("vector1".to_string()),
                similarity: 0.9,
                similarity: Some(0.1),
                // rank removed -  Some(1),
                vector: None,
                metadata: vec![], // No version metadata
                debug_info: None,
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: None,
                index_path: None,
                timestamp: Some(Utc::now()),
            }
        ];
        
        let resolved = engine.apply_mvcc_resolution(results.clone());
        assert!(resolved.is_ok());
        
        let resolved_results = resolved.unwrap();
        assert_eq!(resolved_results.len(), 1);
        assert_eq!(resolved_results[0].id, "doc1");
    }
    
    #[tokio::test]
    async fn test_apply_optimization_hints_with_no_limit() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        let config = SstSearchConfig {
            max_sstables: 1000, // Very high limit
            ..Default::default()
        };
        
        let engine = SstUnifiedSearchEngine::with_config(
            sstable_reader,
            distance_compute,
            quantization_engine,
            config,
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let original_files = vec![
            "file1.sstable".to_string(),
            "file2.sstable".to_string(),
        ];
        
        let original_count = original_files.len();
        
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let optimized_files = runtime.block_on(
            engine.apply_optimization_hints(original_files, &context, &params)
        );
        
        assert!(optimized_files.is_ok());
        let file_list = optimized_files.unwrap();
        assert_eq!(file_list.len(), original_count); // Should not be truncated
    }
}

/// Integration tests
#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_full_search_workflow() {
        let sstable_reader = create_test_sstable_reader().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
        
        let mock_results = create_mock_search_results(10);
        sstable_reader.set_search_results(mock_results);
        
        let engine = SstUnifiedSearchEngine::new(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine.clone(),
        );
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        // Test can_handle
        assert!(engine.can_handle(&context, &params).await);
        
        // Test optimization hints
        let hints = engine.optimization_hints(&context).await;
        assert!(!hints.is_empty());
        
        // Test cost estimation
        let cost = engine.estimate_cost(&context, &params).await;
        assert!(cost > 0.0);
        
        // Test actual search
        let result = engine.search_unified(&context, &params, &distance_compute, Some(&quantization_engine)).await;
        assert!(result.is_ok());
        
        let search_result_set = result.unwrap();
        assert_eq!(search_result_set.results.len(), 10);
        assert!(search_result_set.processing_time_us > 0);
    }
}