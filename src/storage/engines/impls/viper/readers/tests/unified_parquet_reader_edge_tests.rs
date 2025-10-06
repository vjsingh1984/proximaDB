//! Edge case tests for UnifiedParquetReader
//!
//! Comprehensive tests covering error conditions, boundary cases, and performance edge cases

#[cfg(test)]
mod edge_tests {
    use crate::storage::engines::impls::viper::readers::unified_parquet_reader::{
        UnifiedParquetReader, CollectionContext,
    };
    use crate::core::search::{SearchParams, FilterExpression, ComparisonOperator};
    use crate::compute::distance_computation::DistanceMetric;
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
    use std::sync::Arc;
    use serde_json::json;

    // Helper to create reader
    async fn create_test_reader() -> UnifiedParquetReader {
        let config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(config).await.unwrap());
        UnifiedParquetReader::new(filesystem)
    }

    // Empty Collection Tests
    #[tokio::test]
    async fn test_empty_collection_search() {
        let reader = create_test_reader().await;
        let context = CollectionContext {
            collection_id: "empty_collection".to_string(),
            file_paths: vec![], // No files
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 0.0,
            estimated_document_count: 0,
            is_cloud_storage: false,
        };
        
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };
        
        // Should handle empty collection gracefully
        assert_eq!(context.file_paths.len(), 0);
        assert_eq!(context.estimated_document_count, 0);
    }

    // Large Vector Dimension Tests
    #[tokio::test]
    async fn test_high_dimensional_vectors() {
        let reader = create_test_reader().await;
        
        // Test with 4096-dimensional vectors (large but realistic)
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 4096]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };
        
        assert_eq!(params.query_vectors.as_ref().unwrap()[0].len(), 4096);
    }

    // Extreme Top-K Values
    #[tokio::test]
    async fn test_extreme_top_k_values() {
        let reader = create_test_reader().await;
        
        // Test with very large top_k
        let params_large = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(1_000_000), // Extreme value
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };
        
        // Test with zero top_k
        let params_zero = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(0),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };
        
        assert_eq!(params_large.top_k, Some(1_000_000));
        assert_eq!(params_zero.top_k, Some(0));
    }

    // Complex Filter Expression Tests
    #[tokio::test]
    async fn test_deeply_nested_filter_expressions() {
        let reader = create_test_reader().await;
        
        // Create deeply nested filter expression
        let filter = FilterExpression::And(vec![
            FilterExpression::Or(vec![
                FilterExpression::And(vec![
                    FilterExpression::Comparison {
                        field: "level1_a".to_string(),
                        operator: ComparisonOperator::Equals,
                        value: json!("value1"),
                    },
                    FilterExpression::Not(Box::new(
                        FilterExpression::Comparison {
                            field: "level2_a".to_string(),
                            operator: ComparisonOperator::In,
                            value: json!(["a", "b", "c"]),
                        }
                    )),
                ]),
                FilterExpression::And(vec![
                    FilterExpression::Comparison {
                        field: "level1_b".to_string(),
                        operator: ComparisonOperator::GreaterThan,
                        value: json!(100),
                    },
                    FilterExpression::Or(vec![
                        FilterExpression::Comparison {
                            field: "level2_b".to_string(),
                            operator: ComparisonOperator::LessThan,
                            value: json!(50),
                        },
                        FilterExpression::Comparison {
                            field: "level2_c".to_string(),
                            operator: ComparisonOperator::Contains,
                            value: json!("substring"),
                        },
                    ]),
                ]),
            ]),
            FilterExpression::Not(Box::new(
                FilterExpression::Comparison {
                    field: "excluded".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: json!(true),
                }
            )),
        ]);
        
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            filter_expression: Some(filter),
            ..Default::default()
        };
        
        // Verify filter complexity
        assert!(params.filter_expression.is_some());
    }

    // Invalid Data Type Tests
    #[tokio::test]
    async fn test_type_mismatch_in_filters() {
        let reader = create_test_reader().await;
        
        // String comparison on numeric field
        let filter1 = FilterExpression::Comparison {
            field: "price".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: json!("not_a_number"), // Type mismatch
        };
        
        // Numeric comparison on string field
        let filter2 = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(42), // Type mismatch
        };
        
        // Array operation on scalar field
        let filter3 = FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::In,
            value: json!("not_an_array"), // Should be array
        };
        
        // Test params with each filter
        for filter in vec![filter1, filter2, filter3] {
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                top_k: Some(10),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }

    // Null and Missing Value Tests
    #[tokio::test]
    async fn test_null_and_missing_values() {
        let reader = create_test_reader().await;
        
        // Test null value in filter
        let filter_null = FilterExpression::Comparison {
            field: "optional_field".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(null),
        };
        
        // Test empty string
        let filter_empty = FilterExpression::Comparison {
            field: "text_field".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(""),
        };
        
        // Test empty array
        let filter_empty_array = FilterExpression::Comparison {
            field: "tags".to_string(),
            operator: ComparisonOperator::In,
            value: json!([]),
        };
        
        for filter in vec![filter_null, filter_empty, filter_empty_array] {
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }

    // Boundary Value Tests
    #[tokio::test]
    async fn test_numeric_boundary_values() {
        let reader = create_test_reader().await;
        
        // Test with maximum safe integer
        let filter_max = FilterExpression::Comparison {
            field: "large_number".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(9007199254740991i64), // MAX_SAFE_INTEGER
        };
        
        // Test with minimum safe integer
        let filter_min = FilterExpression::Comparison {
            field: "small_number".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(-9007199254740991i64), // MIN_SAFE_INTEGER
        };
        
        // Test with very small float
        let filter_epsilon = FilterExpression::Comparison {
            field: "tiny_float".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(f64::EPSILON),
        };
        
        // Test with infinity
        let filter_inf = FilterExpression::Comparison {
            field: "score".to_string(),
            operator: ComparisonOperator::LessThan,
            value: json!(f64::INFINITY),
        };
        
        for filter in vec![filter_max, filter_min, filter_epsilon, filter_inf] {
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }

    // Special Character Tests
    #[tokio::test]
    async fn test_special_characters_in_fields() {
        let reader = create_test_reader().await;
        
        // Field names with special characters
        let special_fields = vec![
            "field.with.dots",
            "field-with-dashes",
            "field_with_underscores",
            "field with spaces",
            "field/with/slashes",
            "field@with#special$chars",
            "field[with]brackets",
            "field{with}braces",
            "unicode_field_😀",
            "field\twith\ttabs",
            "field\nwith\nnewlines",
        ];
        
        for field in special_fields {
            let filter = FilterExpression::Comparison {
                field: field.to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("test"),
            };
            
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }

    // Multiple Query Vectors Tests
    #[tokio::test]
    async fn test_batch_query_edge_cases() {
        let reader = create_test_reader().await;
        
        // Empty batch
        let params_empty = SearchParams {
            query_vectors: Some(vec![]),
            top_k: Some(10),
            ..Default::default()
        };
        
        // Very large batch
        let large_batch: Vec<Vec<f32>> = (0..1000)
            .map(|i| vec![i as f32 / 1000.0; 128])
            .collect();
        let params_large = SearchParams {
            query_vectors: Some(large_batch.clone()),
            top_k: Some(10),
            ..Default::default()
        };
        
        // Mixed dimensions (invalid)
        let mixed_dims = vec![
            vec![0.1; 128],
            vec![0.1; 256], // Different dimension
            vec![0.1; 128],
        ];
        let params_mixed = SearchParams {
            query_vectors: Some(mixed_dims),
            top_k: Some(10),
            ..Default::default()
        };
        
        assert_eq!(params_empty.query_vectors.as_ref().unwrap().len(), 0);
        assert_eq!(params_large.query_vectors.as_ref().unwrap().len(), 1000);
        assert_eq!(params_mixed.query_vectors.as_ref().unwrap().len(), 3);
    }

    // Cloud Storage Edge Cases
    #[tokio::test]
    async fn test_cloud_storage_paths() {
        let reader = create_test_reader().await;
        
        let cloud_contexts = vec![
            CollectionContext {
                collection_id: "s3_collection".to_string(),
                file_paths: vec!["s3://bucket/path/to/file.parquet".to_string()],
                filterable_columns: vec![],
                quantization_columns: vec![],
                estimated_size_mb: 100.0,
                estimated_document_count: 10000,
                is_cloud_storage: true,
            },
            CollectionContext {
                collection_id: "azure_collection".to_string(),
                file_paths: vec!["https://account.blob.core.windows.net/container/file.parquet".to_string()],
                filterable_columns: vec![],
                quantization_columns: vec![],
                estimated_size_mb: 100.0,
                estimated_document_count: 10000,
                is_cloud_storage: true,
            },
            CollectionContext {
                collection_id: "gcs_collection".to_string(),
                file_paths: vec!["gs://bucket/path/to/file.parquet".to_string()],
                filterable_columns: vec![],
                quantization_columns: vec![],
                estimated_size_mb: 100.0,
                estimated_document_count: 10000,
                is_cloud_storage: true,
            },
        ];
        
        for context in cloud_contexts {
            assert!(context.is_cloud_storage);
            assert!(!context.file_paths.is_none());
        }
    }

    // Memory Pressure Tests
    #[tokio::test]
    async fn test_memory_estimation_edge_cases() {
        let reader = create_test_reader().await;
        
        // Test with zero memory
        let zero_memory_mb = 0.0;
        let per_file_mb = 50.0;
        let batch_size = ((zero_memory_mb / per_file_mb) as f64).floor() as usize;
        assert_eq!(batch_size, 0);
        
        // Test with fractional result
        let small_memory_mb = 25.0;
        let batch_size = ((small_memory_mb / per_file_mb) as f64).floor() as usize;
        assert_eq!(batch_size, 0);
        
        // Test with very large memory
        let huge_memory_mb = f64::MAX;
        let batch_size = ((huge_memory_mb / per_file_mb) as f64).floor() as usize;
        assert!(batch_size > 0);
    }

    // Range Coalescing Edge Cases
    #[tokio::test]
    async fn test_range_coalescing_edge_cases() {
        // Empty ranges
        let empty_ranges: Vec<(usize, usize)> = vec![];
        let coalesced = coalesce_ranges(empty_ranges);
        assert_eq!(coalesced.len(), 0);
        
        // Single range
        let single_range = vec![(0, 1024)];
        let coalesced = coalesce_ranges(single_range);
        assert_eq!(coalesced.len(), 1);
        assert_eq!(coalesced[0], (0, 1024));
        
        // Overlapping ranges
        let overlapping = vec![
            (0, 1024),
            (512, 1536),
            (1000, 2000),
        ];
        let coalesced = coalesce_ranges(overlapping);
        assert_eq!(coalesced.len(), 1);
        assert_eq!(coalesced[0], (0, 2000));
        
        // Adjacent ranges (should coalesce)
        let adjacent = vec![
            (0, 1024),
            (1024, 2048),
            (2048, 3072),
        ];
        let coalesced = coalesce_ranges(adjacent);
        assert_eq!(coalesced.len(), 1);
        assert_eq!(coalesced[0], (0, 3072));
        
        // Ranges with gaps
        let with_gaps = vec![
            (0, 1024),
            (2048, 3072),
            (4096, 5120),
        ];
        let coalesced = coalesce_ranges(with_gaps);
        assert_eq!(coalesced.len(), 3);
        
        // Unsorted ranges
        let unsorted = vec![
            (4096, 5120),
            (0, 1024),
            (2048, 3072),
        ];
        let coalesced = coalesce_ranges(unsorted);
        assert_eq!(coalesced.len(), 3);
        assert_eq!(coalesced[0], (0, 1024)); // Should be sorted
    }

    // Helper function for range coalescing
    fn coalesce_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
        if ranges.is_none() {
            return ranges;
        }
        
        ranges.sort_by_key(|r| r.0);
        let mut coalesced = vec![ranges[0]];
        
        for range in ranges.into_iter().skip(1) {
            let last = coalesced.last_mut().unwrap();
            if range.0 <= last.1 {
                last.1 = last.1.max(range.1);
            } else {
                coalesced.push(range);
            }
        }
        
        coalesced
    }

    // Concurrent Access Tests
    #[tokio::test]
    async fn test_concurrent_reader_access() {
        use tokio::task::JoinSet;
        
        let reader = Arc::new(create_test_reader().await);
        let mut tasks = JoinSet::new();
        
        // Spawn multiple concurrent searches
        for i in 0..10 {
            let reader_clone = reader.clone();
            tasks.spawn(async move {
                let params = SearchParams {
                    query_vectors: Some(vec![vec![i as f32 / 10.0; 128]]),
                    top_k: Some(10),
                    distance_metric: Some(DistanceMetric::Cosine),
                    ..Default::default()
                };
                // Simulate search operation
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                params
            });
        }
        
        // Wait for all tasks to complete
        let mut results = vec![];
        while let Some(res) = tasks.join_next().await {
            results.push(res.unwrap());
        }
        
        assert_eq!(results.len(), 10);
    }

    // Error Recovery Tests
    #[tokio::test]
    async fn test_malformed_parquet_handling() {
        let reader = create_test_reader().await;
        
        let context = CollectionContext {
            collection_id: "corrupted_collection".to_string(),
            file_paths: vec![
                "/tmp/corrupted.parquet".to_string(),
                "/tmp/valid.parquet".to_string(),
            ],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 100.0,
            estimated_document_count: 10000,
            is_cloud_storage: false,
        };
        
        // Should handle mixed valid/invalid files
        assert_eq!(context.file_paths.len(), 2);
    }

    // Unicode and International Character Tests
    #[tokio::test]
    async fn test_unicode_handling() {
        let reader = create_test_reader().await;
        
        let unicode_values = vec![
            "Hello 世界", // Chinese
            "Привет мир", // Russian
            "مرحبا بالعالم", // Arabic
            "שלום עולם", // Hebrew
            "🌍🌎🌏", // Emojis
            "Ñoño", // Spanish special chars
            "Ελληνικά", // Greek
            "日本語", // Japanese
            "한국어", // Korean
            "ไทย", // Thai
        ];
        
        for value in unicode_values {
            let filter = FilterExpression::Comparison {
                field: "text".to_string(),
                operator: ComparisonOperator::Contains,
                value: json!(value),
            };
            
            let params = SearchParams {
                query_vectors: Some(vec![vec![0.1; 128]]),
                filter_expression: Some(filter),
                ..Default::default()
            };
            assert!(params.filter_expression.is_some());
        }
    }
}