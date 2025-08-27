#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::collections::HashMap;
    use anyhow::Result;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::{SearchParams, SearchResult, SearchResultSet, SemanticDistance};
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::compute::quantization::unified::{UnifiedQuantizationEngine, UnifiedQuantizationLevel};
    use crate::services::collection::manager::CollectionService;

    // Mock search engine for testing
    struct MockSearchEngine {
        id: String,
        can_handle_result: bool,
        cost_estimate: f64,
        results: Vec<SearchResult>,
    }

    #[async_trait]
    impl UnifiedSearchEngine for MockSearchEngine {
        fn engine_id(&self) -> &str {
            &self.id
        }

        async fn search_unified(
            &self,
            _context: &SearchPlan,
            _params: &SearchParams,
            _distance_compute: &UnifiedDistanceCompute,
            _quantization_engine: Option<&UnifiedQuantizationEngine>,
        ) -> Result<SearchResultSet> {
            Ok(SearchResultSet {
                results: self.results.clone(),
                total_count: self.results.len() as u64,
                query_id: None,
                processing_time_us: 1000,
                algorithm: self.id.clone(),
                metadata: HashMap::new(),
            })
        }

        async fn can_handle(&self, _context: &SearchPlan, _params: &SearchParams) -> bool {
            self.can_handle_result
        }

        async fn optimization_hints(&self, _context: &SearchPlan) -> Vec<OptimizationHint> {
            vec![
                OptimizationHint::UseQuantization {
                    method: UnifiedQuantizationLevel::pq8(32),
                    expected_speedup: 3.0,
                },
                OptimizationHint::UseMetadataFiltering {
                    selectivity_estimate: 0.1,
                },
            ]
        }

        async fn estimate_cost(&self, _context: &SearchPlan, _params: &SearchParams) -> f64 {
            self.cost_estimate
        }
    }

    fn create_test_search_context() -> SearchPlan {
        SearchPlan {
            collection_id: "test_collection".to_string(),
            collection_config: Some(CollectionConfig {
                default_distance_metric: DistanceMetric::Cosine,
                vector_dimension: 128,
                enable_quantization: true,
                enable_metadata_filtering: true,
                estimated_document_count: 10000,
                compression: None,
                optimization_hints: None,
            }),
            filterable_columns: vec![
                FilterableColumn {
                    name: "category".to_string(),
                    // data_type removed -  ColumnData::String,
                    is_indexed: true,
                    estimated_cardinality: Some(10),
                },
                FilterableColumn {
                    name: "price".to_string(),
                    // data_type removed -  ColumnData::Float,
                    is_indexed: false,
                    estimated_cardinality: None,
                },
            ],
            available_quantization: vec![
                UnifiedQuantizationLevel::pq8(32),
                UnifiedQuantizationLevel::int8(),
            ],
            storage_info: StorageInfo {
                is_cloud_storage: true,
                storage_type: "S3".to_string(),
                estimated_size_mb: 500.0,
                file_count: 5,
                supports_range_requests: true,
            },
        }
    }

    fn create_test_search_params() -> SearchParams {
        SearchParams {
            vector: vec![0.1; 128],
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            metadata_filters: None,
            custom_hints: None,
        }
    }

    fn create_test_search_result(id: &str, similarity: f32) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            vector: Some(vec![0.1; 128]),
            metadata: HashMap::new(),
            score,
            // rank removed -  None,
            semantic_similarity: Some(SemanticDistance {
                raw_value: score,
                metric: DistanceMetric::Cosine,
                normalized_score: 1.0 - score, // Higher normalized = more similar
                rank_value: score, // Lower rank_value = better
            }),
        }
    }

    #[test]
    fn test_unified_search_context_creation() {
        let context = create_test_search_context();
        
        assert_eq!(context.collection_id, "test_collection");
        assert!(context.collection_config.is_some());
        
        let config = context.collection_config.unwrap();
        assert_eq!(config.default_distance_metric, DistanceMetric::Cosine);
        assert_eq!(config.vector_dimension, 128);
        assert!(config.enable_quantization);
        assert!(config.enable_metadata_filtering);
        assert_eq!(config.estimated_document_count, 10000);
        
        assert_eq!(context.filterable_columns.len(), 2);
        assert_eq!(context.filterable_columns[0].name, "category");
        assert!(matches!(context.filterable_columns[0].data_type, ColumnData::String));
        assert!(context.filterable_columns[0].is_indexed);
        
        assert_eq!(context.available_quantization.len(), 2);
        
        assert!(context.storage_info.is_cloud_storage);
        assert_eq!(context.storage_info.storage_type, "S3");
        assert_eq!(context.storage_info.estimated_size_mb, 500.0);
    }

    #[test]
    fn test_collection_config_creation() {
        let config = CollectionConfig {
            default_distance_metric: DistanceMetric::Euclidean,
            vector_dimension: 256,
            enable_quantization: false,
            enable_metadata_filtering: true,
            estimated_document_count: 5000,
                compression: None,
                optimization_hints: None,
            };
        
        assert_eq!(config.default_distance_metric, DistanceMetric::Euclidean);
        assert_eq!(config.vector_dimension, 256);
        assert!(!config.enable_quantization);
        assert!(config.enable_metadata_filtering);
        assert_eq!(config.estimated_document_count, 5000);
    }

    #[test]
    fn test_filterable_column_types() {
        let columns = vec![
            FilterableColumn {
                name: "string_col".to_string(),
                // data_type removed -  ColumnData::String,
                is_indexed: true,
                estimated_cardinality: Some(100),
            },
            FilterableColumn {
                name: "int_col".to_string(),
                // data_type removed -  ColumnData::Integer,
                is_indexed: false,
                estimated_cardinality: None,
            },
            FilterableColumn {
                name: "bool_col".to_string(),
                // data_type removed -  ColumnData::Boolean,
                is_indexed: true,
                estimated_cardinality: Some(2),
            },
            FilterableColumn {
                name: "datetime_col".to_string(),
                // data_type removed -  ColumnData::DateTime,
                is_indexed: true,
                estimated_cardinality: Some(1000),
            },
            FilterableColumn {
                name: "json_col".to_string(),
                // data_type removed -  ColumnData::Json,
                is_indexed: false,
                estimated_cardinality: None,
            },
        ];
        
        assert_eq!(columns.len(), 5);
        assert!(matches!(columns[0].data_type, ColumnData::String));
        assert!(matches!(columns[1].data_type, ColumnData::Integer));
        assert!(matches!(columns[2].data_type, ColumnData::Boolean));
        assert!(matches!(columns[3].data_type, ColumnData::DateTime));
        assert!(matches!(columns[4].data_type, ColumnData::Json));
        
        assert!(columns[0].is_indexed);
        assert!(!columns[1].is_indexed);
        assert_eq!(columns[2].estimated_cardinality, Some(2));
    }

    #[test]
    fn test_storage_info_creation() {
        let storage_info = StorageInfo {
            is_cloud_storage: false,
            storage_type: "Local".to_string(),
            estimated_size_mb: 1024.5,
            file_count: 20,
            supports_range_requests: false,
        };
        
        assert!(!storage_info.is_cloud_storage);
        assert_eq!(storage_info.storage_type, "Local");
        assert_eq!(storage_info.estimated_size_mb, 1024.5);
        assert_eq!(storage_info.file_count, 20);
        assert!(!storage_info.supports_range_requests);
    }

    #[test]
    fn test_optimization_hints() {
        let hints = vec![
            OptimizationHint::UseQuantization {
                method: UnifiedQuantizationLevel::pq4(16),
                expected_speedup: 2.5,
            },
            OptimizationHint::UseMetadataFiltering {
                selectivity_estimate: 0.05,
            },
            OptimizationHint::UseColumnProjection {
                columns: vec!["vector".to_string(), "metadata_info".to_string()],
            },
            OptimizationHint::UseRangeRequests {
                chunk_size_mb: 64.0,
            },
            OptimizationHint::UseCaching {
                cache_key: "query_hash_123".to_string(),
            },
        ];
        
        assert_eq!(hints.len(), 5);
        
        match &hints[0] {
            OptimizationHint::UseQuantization { expected_speedup, .. } => {
                assert_eq!(*expected_speedup, 2.5);
            }
            _ => panic!("Expected UseQuantization hint"),
        }
        
        match &hints[1] {
            OptimizationHint::UseMetadataFiltering { selectivity_estimate } => {
                assert_eq!(*selectivity_estimate, 0.05);
            }
            _ => panic!("Expected UseMetadataFiltering hint"),
        }
        
        match &hints[2] {
            OptimizationHint::UseColumnProjection { columns } => {
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0], "vector");
            }
            _ => panic!("Expected UseColumnProjection hint"),
        }
    }

    #[test]
    fn test_unified_search_orchestrator_creation() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        assert_eq!(orchestrator.engines.len(), 0);
    }

    #[test]
    fn test_engine_registration() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let mut orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        let mock_engine = Arc::new(MockSearchEngine {
            id: "test_engine".to_string(),
            can_handle_result: true,
            cost_estimate: 1.0,
            results: vec![],
        });
        
        orchestrator.register_engine(mock_engine);
        assert_eq!(orchestrator.engines.len(), 1);
        assert_eq!(orchestrator.engines[0].engine_id(), "test_engine");
    }

    #[tokio::test]
    async fn test_mock_search_engine_interface() {
        let mock_engine = MockSearchEngine {
            id: "mock_test".to_string(),
            can_handle_result: true,
            cost_estimate: 2.5,
            results: vec![
                create_test_search_result("result1", 0.1),
                create_test_search_result("result2", 0.2),
            ],
        };
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        let distance_compute = UnifiedDistanceCompute::default();
        let quantization_engine = UnifiedQuantizationEngine::new();
        
        // Test engine_id
        assert_eq!(mock_engine.engine_id(), "mock_test");
        
        // Test can_handle
        assert!(mock_engine.can_handle(&context, &params).await);
        
        // Test estimate_cost
        let cost = mock_engine.estimate_cost(&context, &params).await;
        assert_eq!(cost, 2.5);
        
        // Test optimization_hints
        let hints = mock_engine.optimization_hints(&context).await;
        assert_eq!(hints.len(), 2);
        
        // Test search_unified
        let results = mock_engine.search_unified(
            &context,
            &params,
            &distance_compute,
            Some(&quantization_engine),
        ).await.unwrap();
        
        assert_eq!(results.results.len(), 2);
        assert_eq!(results.total_count, 2);
        assert_eq!(results.processing_time_us, 1000);
        assert_eq!(results.algorithm, "mock_test");
    }

    #[tokio::test]
    async fn test_engine_selection_by_capability() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let mut orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        // Register engines with different capabilities
        let engine1 = Arc::new(MockSearchEngine {
            id: "capable_engine".to_string(),
            can_handle_result: true,
            cost_estimate: 1.0,
            results: vec![],
        });
        
        let engine2 = Arc::new(MockSearchEngine {
            id: "incapable_engine".to_string(),
            can_handle_result: false,
            cost_estimate: 0.5,
            results: vec![],
        });
        
        orchestrator.register_engine(engine1);
        orchestrator.register_engine(engine2);
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let selected = orchestrator.select_engines(&context, &params).await.unwrap();
        
        // Should only select the capable engine
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].engine_id(), "capable_engine");
    }

    #[tokio::test]
    async fn test_engine_selection_by_cost() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let mut orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        // Register engines with different costs (both capable)
        let expensive_engine = Arc::new(MockSearchEngine {
            id: "expensive".to_string(),
            can_handle_result: true,
            cost_estimate: 10.0,
            results: vec![],
        });
        
        let cheap_engine = Arc::new(MockSearchEngine {
            id: "cheap".to_string(),
            can_handle_result: true,
            cost_estimate: 1.0,
            results: vec![],
        });
        
        orchestrator.register_engine(expensive_engine);
        orchestrator.register_engine(cheap_engine);
        
        let context = create_test_search_context();
        let params = create_test_search_params();
        
        let selected = orchestrator.select_engines(&context, &params).await.unwrap();
        
        // Should be sorted by cost (cheaper first)
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].engine_id(), "cheap");
        assert_eq!(selected[1].engine_id(), "expensive");
    }

    #[tokio::test]
    async fn test_unified_ranking() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        let mut results = vec![
            create_test_search_result("result3", 0.8), // Worst score
            create_test_search_result("result1", 0.2), // Best score
            create_test_search_result("result2", 0.5), // Middle score
        ];
        
        let params = SearchParams {
            vector: vec![0.1; 128],
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            metadata_filters: None,
            custom_hints: None,
        };
        
        orchestrator.apply_unified_ranking(&mut results, &params).await.unwrap();
        
        // Should be sorted by rank_value (lower = better)
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].id, "result1");
        assert_eq!(results[1].id, "result2");
        assert_eq!(results[2].id, "result3");
        
        // Ranks should be assigned
        assert_eq!(results[0].rank, Some(1));
        assert_eq!(results[1].rank, Some(2));
        assert_eq!(results[2].rank, Some(3));
    }

    #[tokio::test]
    async fn test_unified_ranking_with_limit() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        let mut results = vec![
            create_test_search_result("result3", 0.8),
            create_test_search_result("result1", 0.2),
            create_test_search_result("result2", 0.5),
            create_test_search_result("result4", 0.9),
        ];
        
        let params = SearchParams {
            vector: vec![0.1; 128],
            top_k: Some(2), // Limit to 2 results
            distance_metric: Some(DistanceMetric::Cosine),
            metadata_filters: None,
            custom_hints: None,
        };
        
        orchestrator.apply_unified_ranking(&mut results, &params).await.unwrap();
        
        // Should be limited to top 2 results
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "result1"); // Best score
        assert_eq!(results[1].id, "result2"); // Second best
        
        assert_eq!(results[0].rank, Some(1));
        assert_eq!(results[1].rank, Some(2));
    }

    #[tokio::test]
    async fn test_analyze_storage_info() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new());
        let collection_service = Arc::new(CollectionService::new_for_testing());
        
        let orchestrator = IntegratedSearchOptimizer::new(
            distance_compute,
            quantization_engine,
            collection_service,
        );
        
        let storage_info = orchestrator.analyze_storage_info("test_collection").await.unwrap();
        
        // Should return default values for now
        assert!(storage_info.is_cloud_storage);
        assert_eq!(storage_info.storage_type, "S3");
        assert_eq!(storage_info.estimated_size_mb, 1000.0);
        assert_eq!(storage_info.file_count, 10);
        assert!(storage_info.supports_range_requests);
    }

    #[test]
    fn test_search_params_creation() {
        let params = create_test_search_params();
        
        assert_eq!(params.vector.len(), 128);
        assert_eq!(params.top_k, Some(10));
        assert_eq!(params.distance_metric, Some(DistanceMetric::Cosine));
        assert!(params.metadata_filters.is_empty());
        assert!(params.custom_hints.is_empty());
    }

    #[test]
    fn test_search_result_with_semantic_distance() {
        let result = create_test_search_result("test_result", 0.3);
        
        assert_eq!(result.id, "test_result");
        assert_eq!(result.score, 0.3);
        assert!(result.semantic_distance.is_some());
        
        let semantic = result.semantic_distance.unwrap();
        assert_eq!(semantic.raw_value, 0.3);
        assert_eq!(semantic.metric, DistanceMetric::Cosine);
        assert_eq!(semantic.normalized_score, 0.7); // 1.0 - 0.3
        assert_eq!(semantic.rank_value, 0.3);
    }

    #[test]
    fn test_column_data_type_variants() {
        let types = vec![
            ColumnData::String,
            ColumnData::Integer,
            ColumnData::Float,
            ColumnData::Boolean,
            ColumnData::DateTime,
            ColumnData::Json,
        ];
        
        assert_eq!(types.len(), 6);
        
        // Test that all variants can be created and are distinct
        for (i, type_a) in types.iter().enumerate() {
            for (j, type_b) in types.iter().enumerate() {
                if i != j {
                    // Different indices should have different discriminants
                    // (This tests that each variant is distinct)
                    assert_ne!(
                        std::mem::discriminant(type_a),
                        std::mem::discriminant(type_b)
                    );
                }
            }
        }
    }

    #[test]
    fn test_optimization_hint_variants() {
        let quantization_hint = OptimizationHint::UseQuantization {
            method: UnifiedQuantizationLevel::int8(),
            expected_speedup: 1.5,
        };
        
        let filtering_hint = OptimizationHint::UseMetadataFiltering {
            selectivity_estimate: 0.2,
        };
        
        let projection_hint = OptimizationHint::UseColumnProjection {
            columns: vec!["col1".to_string()],
        };
        
        let range_hint = OptimizationHint::UseRangeRequests {
            chunk_size_mb: 32.0,
        };
        
        let cache_hint = OptimizationHint::UseCaching {
            cache_key: "key123".to_string(),
        };
        
        // Test that different variants have different discriminants
        assert_ne!(
            std::mem::discriminant(&quantization_hint),
            std::mem::discriminant(&filtering_hint)
        );
        assert_ne!(
            std::mem::discriminant(&filtering_hint),
            std::mem::discriminant(&projection_hint)
        );
        assert_ne!(
            std::mem::discriminant(&projection_hint),
            std::mem::discriminant(&range_hint)
        );
        assert_ne!(
            std::mem::discriminant(&range_hint),
            std::mem::discriminant(&cache_hint)
        );
    }
}