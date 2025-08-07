// UnifiedQueryPlanner Tests

#[cfg(test)]
mod tests {
    use super::super::unified_query_planner::*;
    use super::super::{CompressionAlgorithmSst, FileMetadata, BlockMetadata};
    use crate::proto::{SearchQuery, OptimizationHints, QuantizationType};
    use crate::sql::parser::{ParsedQuery, WhereCondition, SelectField};
    use crate::storage::metadata::quantization::QuantizationConfig;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn create_test_planner() -> UnifiedQueryPlanner {
        UnifiedQueryPlanner::new()
    }

    fn create_test_metadata(
        compression: Option<CompressionAlgorithmSst>,
        quantized: bool,
    ) -> FileMetadata {
        FileMetadata {
            id: "test.sst".to_string(),
            path: "/data/test.sst".to_string(),
            size: 1024 * 1024,
            compression_algorithm: compression,
            compression_ratio: compression.map(|_| 0.5),
            is_quantized: quantized,
            quantization_config: if quantized {
                Some(QuantizationConfig {
                    quantization_type: QuantizationType::BinaryQuantization,
                    codebook_size: 256,
                    subvector_dimension: 16,
                    distance_computation: "asymmetric".to_string(),
                })
            } else {
                None
            },
            bloom_filter_size: 1024,
            index_size: 2048,
            num_records: 1000,
            min_timestamp: 0,
            max_timestamp: 1000000,
            block_metadata: vec![
                BlockMetadata {
                    block_id: 0,
                    offset: 0,
                    compressed_size: 4096,
                    uncompressed_size: 8192,
                    num_records: 100,
                },
                BlockMetadata {
                    block_id: 1,
                    offset: 4096,
                    compressed_size: 4096,
                    uncompressed_size: 8192,
                    num_records: 100,
                },
            ],
        }
    }

    #[tokio::test]
    async fn test_planner_creation() {
        let planner = create_test_planner();
        assert!(planner.compression_stats.is_empty());
        assert!(planner.quantization_stats.is_empty());
    }

    #[tokio::test]
    async fn test_analyze_compression_status() {
        let planner = create_test_planner();
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
            create_test_metadata(Some(CompressionAlgorithmSst::Lz4), false),
            create_test_metadata(None, false),
        ];
        
        let status = planner.analyze_compression_status(&files).await;
        
        assert_eq!(status.total_files, 3);
        assert_eq!(status.compressed_files, 2);
        assert_eq!(status.uncompressed_files, 1);
        assert_eq!(status.algorithm_distribution.get(&CompressionAlgorithmSst::Zstd), Some(&1));
        assert_eq!(status.algorithm_distribution.get(&CompressionAlgorithmSst::Lz4), Some(&1));
    }

    #[tokio::test]
    async fn test_analyze_quantization_status() {
        let planner = create_test_planner();
        
        let files = vec![
            create_test_metadata(None, true),
            create_test_metadata(None, true),
            create_test_metadata(None, false),
        ];
        
        let status = planner.analyze_quantization_status(&files).await;
        
        assert_eq!(status.total_files, 3);
        assert_eq!(status.quantized_files, 2);
        assert_eq!(status.full_precision_files, 1);
        assert_eq!(status.type_distribution.get(&QuantizationType::BinaryQuantization), Some(&2));
    }

    #[tokio::test]
    async fn test_plan_proto_query_with_compression_hints() {
        let mut planner = create_test_planner();
        
        let query = SearchQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 10,
            filter: None,
            include_metadata: true,
            include_vectors: false,
            radius: None,
            optimization_hints: Some(OptimizationHints {
                prefer_compressed_search: true,
                decompression_budget_ms: Some(100),
                use_decompression_cache: true,
                compression_aware_routing: true,
                enable_two_stage: false,
                coarse_quantization_k: None,
                rerank_multiplier: None,
                allow_approximate: false,
                target_recall: None,
                cache_key: None,
                prefetch_count: None,
                parallel_decompression: false,
                adaptive_k: false,
                use_hnsw_ef_runtime: None,
                skip_reranking: false,
                batch_mode: false,
                streaming_mode: false,
                hardware_hints: None,
                predicate_pushdown: false,
            }),
        };
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
            create_test_metadata(None, false),
        ];
        
        let plan = planner.plan_proto_query(&query, &files).await.unwrap();
        
        assert!(plan.use_compression_aware_routing);
        assert!(plan.use_decompression_cache);
        assert!(plan.prefer_compressed);
        assert_eq!(plan.decompression_budget_ms, Some(100));
    }

    #[tokio::test]
    async fn test_plan_proto_query_with_quantization() {
        let mut planner = create_test_planner();
        
        let query = SearchQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 10,
            filter: None,
            include_metadata: false,
            include_vectors: false,
            radius: None,
            optimization_hints: Some(OptimizationHints {
                prefer_compressed_search: false,
                decompression_budget_ms: None,
                use_decompression_cache: false,
                compression_aware_routing: false,
                enable_two_stage: true,
                coarse_quantization_k: Some(100),
                rerank_multiplier: Some(2.0),
                allow_approximate: true,
                target_recall: Some(0.95),
                cache_key: None,
                prefetch_count: None,
                parallel_decompression: false,
                adaptive_k: false,
                use_hnsw_ef_runtime: None,
                skip_reranking: false,
                batch_mode: false,
                streaming_mode: false,
                hardware_hints: None,
                predicate_pushdown: false,
            }),
        };
        
        let files = vec![
            create_test_metadata(None, true),
            create_test_metadata(None, false),
        ];
        
        let plan = planner.plan_proto_query(&query, &files).await.unwrap();
        
        assert!(plan.enable_two_stage);
        assert_eq!(plan.coarse_k, 100);
        assert_eq!(plan.rerank_multiplier, 2.0);
        assert!(plan.allow_approximate);
    }

    #[tokio::test]
    async fn test_plan_sql_query() {
        let mut planner = create_test_planner();
        
        let query = ParsedQuery {
            select_fields: vec![
                SelectField::Aliased("id".to_string(), None),
                SelectField::Aliased("metadata".to_string(), None),
            ],
            from_table: "test_collection".to_string(),
            where_conditions: vec![
                WhereCondition::Comparison {
                    field: "status".to_string(),
                    operator: "=".to_string(),
                    value: "'active'".to_string(),
                },
            ],
            order_by: vec![],
            limit: Some(10),
            vector_similarity: Some(crate::sql::parser::VectorSimilarity {
                vector_field: "vector".to_string(),
                query_vector: vec![0.1; 128],
                distance_metric: "cosine".to_string(),
            }),
        };
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
            create_test_metadata(None, true),
        ];
        
        let plan = planner.plan_sql_query(&query, &files).await.unwrap();
        
        assert!(!plan.filtered_files.is_empty());
        assert!(plan.compression_status.compressed_files > 0 || plan.quantization_status.quantized_files > 0);
    }

    #[tokio::test]
    async fn test_estimate_decompression_cost() {
        let planner = create_test_planner();
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
            create_test_metadata(Some(CompressionAlgorithmSst::Lz4), false),
            create_test_metadata(Some(CompressionAlgorithmSst::Snappy), false),
        ];
        
        let cost = planner.estimate_decompression_cost(&files).await;
        
        // ZSTD: 30ms, LZ4: 10ms, Snappy: 5ms
        assert_eq!(cost, 45);
    }

    #[tokio::test]
    async fn test_determine_search_strategy_mixed() {
        let planner = create_test_planner();
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
            create_test_metadata(None, true),
            create_test_metadata(None, false),
        ];
        
        let strategy = planner.determine_search_strategy(&files).await;
        
        assert_eq!(strategy, SearchStrategy::Hybrid);
    }

    #[tokio::test]
    async fn test_determine_search_strategy_direct() {
        let planner = create_test_planner();
        
        let files = vec![
            create_test_metadata(None, false),
            create_test_metadata(None, false),
        ];
        
        let strategy = planner.determine_search_strategy(&files).await;
        
        assert_eq!(strategy, SearchStrategy::Direct);
    }

    #[tokio::test]
    async fn test_determine_search_strategy_two_stage() {
        let planner = create_test_planner();
        
        let files = vec![
            create_test_metadata(None, true),
            create_test_metadata(None, true),
        ];
        
        let strategy = planner.determine_search_strategy(&files).await;
        
        assert_eq!(strategy, SearchStrategy::TwoStage);
    }

    #[tokio::test]
    async fn test_concurrent_planning() {
        let planner = Arc::new(create_test_planner());
        let files = Arc::new(vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
            create_test_metadata(None, true),
        ]);
        
        let mut handles = vec![];
        
        for i in 0..10 {
            let planner_clone = Arc::clone(&planner);
            let files_clone = Arc::clone(&files);
            
            let handle = tokio::spawn(async move {
                let query = SearchQuery {
                    collection_id: format!("collection_{}", i),
                    vector: vec![0.1; 128],
                    k: 10,
                    filter: None,
                    include_metadata: false,
                    include_vectors: false,
                    radius: None,
                    optimization_hints: Some(OptimizationHints {
                        prefer_compressed_search: i % 2 == 0,
                        decompression_budget_ms: Some(100),
                        use_decompression_cache: true,
                        compression_aware_routing: true,
                        enable_two_stage: i % 3 == 0,
                        coarse_quantization_k: Some(100),
                        rerank_multiplier: Some(2.0),
                        allow_approximate: false,
                        target_recall: None,
                        cache_key: None,
                        prefetch_count: None,
                        parallel_decompression: false,
                        adaptive_k: false,
                        use_hnsw_ef_runtime: None,
                        skip_reranking: false,
                        batch_mode: false,
                        streaming_mode: false,
                        hardware_hints: None,
                        predicate_pushdown: false,
                    }),
                };
                
                planner_clone.plan_proto_query(&query, &files_clone).await
            });
            
            handles.push(handle);
        }
        
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_update_statistics() {
        let mut planner = create_test_planner();
        
        // Update compression stats
        planner.update_compression_stats(CompressionAlgorithmSst::Zstd, 100.0, 0.5).await;
        planner.update_compression_stats(CompressionAlgorithmSst::Zstd, 120.0, 0.6).await;
        
        let stats = planner.compression_stats.get(&CompressionAlgorithmSst::Zstd).unwrap();
        assert_eq!(stats.total_operations, 2);
        assert_eq!(stats.avg_decompression_time_ms, 110.0);
        assert_eq!(stats.avg_compression_ratio, 0.55);
        
        // Update quantization stats
        planner.update_quantization_stats(QuantizationType::BinaryQuantization, 0.95, 10.0).await;
        planner.update_quantization_stats(QuantizationType::BinaryQuantization, 0.93, 12.0).await;
        
        let stats = planner.quantization_stats.get(&QuantizationType::BinaryQuantization).unwrap();
        assert_eq!(stats.total_operations, 2);
        assert_eq!(stats.avg_recall, 0.94);
        assert_eq!(stats.avg_speedup, 11.0);
    }

    #[tokio::test]
    async fn test_cache_aware_planning() {
        let mut planner = create_test_planner();
        
        // Simulate cached blocks
        let query = SearchQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 10,
            filter: None,
            include_metadata: false,
            include_vectors: false,
            radius: None,
            optimization_hints: Some(OptimizationHints {
                prefer_compressed_search: false,
                decompression_budget_ms: Some(50),
                use_decompression_cache: true,
                compression_aware_routing: true,
                enable_two_stage: false,
                coarse_quantization_k: None,
                rerank_multiplier: None,
                allow_approximate: false,
                target_recall: None,
                cache_key: Some("cached_query".to_string()),
                prefetch_count: Some(5),
                parallel_decompression: true,
                adaptive_k: false,
                use_hnsw_ef_runtime: None,
                skip_reranking: false,
                batch_mode: false,
                streaming_mode: false,
                hardware_hints: None,
                predicate_pushdown: false,
            }),
        };
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), false),
        ];
        
        let plan = planner.plan_proto_query(&query, &files).await.unwrap();
        
        assert!(plan.use_decompression_cache);
        assert_eq!(plan.prefetch_count, 5);
        assert!(plan.parallel_decompression);
    }

    #[tokio::test]
    async fn test_adaptive_planning() {
        let mut planner = create_test_planner();
        
        // Train planner with historical data
        for _ in 0..10 {
            planner.update_compression_stats(CompressionAlgorithmSst::Zstd, 100.0, 0.5).await;
            planner.update_quantization_stats(QuantizationType::BinaryQuantization, 0.95, 10.0).await;
        }
        
        let query = SearchQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 10,
            filter: None,
            include_metadata: false,
            include_vectors: false,
            radius: None,
            optimization_hints: Some(OptimizationHints {
                prefer_compressed_search: false,
                decompression_budget_ms: None,
                use_decompression_cache: false,
                compression_aware_routing: false,
                enable_two_stage: false,
                coarse_quantization_k: None,
                rerank_multiplier: None,
                allow_approximate: false,
                target_recall: None,
                cache_key: None,
                prefetch_count: None,
                parallel_decompression: false,
                adaptive_k: true,  // Enable adaptive planning
                use_hnsw_ef_runtime: None,
                skip_reranking: false,
                batch_mode: false,
                streaming_mode: false,
                hardware_hints: None,
                predicate_pushdown: false,
            }),
        };
        
        let files = vec![
            create_test_metadata(Some(CompressionAlgorithmSst::Zstd), true),
        ];
        
        let plan = planner.plan_proto_query(&query, &files).await.unwrap();
        
        // Planner should adapt based on historical stats
        assert!(plan.adaptive_k);
    }
}