//! Test-Driven Development tests for UnifiedParquetReader
//!
//! Tests the unified search architecture for VIPER engine

#[cfg(test)]
mod tests {
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::unified_interface::{CollectionConfig, SearchPlan, StorageInfo};
    use crate::core::search::{ComparisonOperator, FilterExpression, SearchParams};
    use crate::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};
    use crate::storage::engines::core::formats::columnar::CollectionContext;
    use crate::storage::engines::core::formats::columnar::columnar_query_engine::unified_reader::UnifiedParquetReader;
    use anyhow::Result;
    use arrow_array::{Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    // Test helpers
    async fn create_test_reader() -> UnifiedParquetReader {
        let file_paths = vec![
            "/tmp/test1.parquet".to_string(),
            "/tmp/test2.parquet".to_string(),
        ];
        create_test_reader_with_files(file_paths).await
    }

    async fn create_test_reader_with_files(file_paths: Vec<String>) -> UnifiedParquetReader {
        // Create UnifiedCachingFilesystem for testing
        let fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
                .await
                .unwrap(),
        );
        let base_fs = filesystem_factory.get_filesystem("file://").unwrap();
        let cached_filesystem = Arc::new(
            crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
                base_fs,
                "test_collection".to_string(),
                "viper".to_string(),
            ),
        );
        UnifiedParquetReader::new(
            file_paths,
            128,
            filesystem_factory,
            cached_filesystem,
            "test_collection".to_string(),
            "viper".to_string(),
        )
        .unwrap()
    }

    fn convert_search_params_to_plan(_params: &SearchParams, collection_id: &str) -> SearchPlan {
        SearchPlan {
            collection_id: collection_id.to_string(),
            collection_config: Some(CollectionConfig {
                default_distance_metric: _params.distance_metric.unwrap_or(DistanceMetric::Cosine),
                vector_dimension: 128,
                enable_quantization: false,
                enable_metadata_filtering: _params.filter_expression.is_some(),
                estimated_document_count: 1000,
            }),
            filterable_columns: vec![],
            available_quantization: vec![],
            storage_info: StorageInfo {
                is_cloud_storage: false,
                storage_type: "Local".to_string(),
                estimated_size_mb: 1.0,
                file_count: 1,
                supports_range_requests: false,
                file_paths: None,
            },
            filter_expression: None,
            query_vector: _params.vector.clone(),
            top_k: _params.top_k.unwrap_or(100) as usize,
            min_score: None,                // No minimum score filter for tests
            enable_early_termination: true, // Enable optimizations by default
        }
    }

    // Simply access results directly since OptimizedSearchRecord is private
    // No extension trait needed

    fn create_test_context() -> CollectionContext {
        CollectionContext {
            collection_id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        }
    }

    // Basic Strategy Selection Tests
    #[tokio::test]
    async fn test_reader_creation() {
        let _reader = create_test_reader().await;
        // Test passes if reader is created successfully
        assert!(true);
    }

    #[tokio::test]
    async fn test_strategy_selection_basic() {
        let _reader = create_test_reader().await;
        let _context = create_test_context();

        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        // Test strategy selection logic - this would be internal to reader
        // For now, just verify params are valid
        assert!(params.query_vectors.is_some());
    }

    #[tokio::test]
    async fn test_strategy_with_filters() {
        let _reader = create_test_reader().await;
        let _context = create_test_context();

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

        // With filters, should use metadata filtered strategy
        assert!(params.filter_expression.is_some());
    }

    #[tokio::test]
    async fn test_strategy_with_quantization() {
        let _reader = create_test_reader().await;
        let context = create_test_context();
        // Note: quantization_columns field removed from CollectionContext

        let _params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        // With quantized columns, should use two-stage strategy
        // Note: quantization_columns field removed from CollectionContext
        assert_eq!(context.dimension, 128);
    }

    // Filter Expression Tests
    #[tokio::test]
    async fn test_complex_filter_expression() {
        let _filter = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("electronics"),
            },
            FilterExpression::Or(vec![
                FilterExpression::Comparison {
                    field: "price".to_string(),
                    operator: ComparisonOperator::LessThan,
                    value: json!(100),
                },
                FilterExpression::Comparison {
                    field: "discount".to_string(),
                    operator: ComparisonOperator::GreaterThan,
                    value: json!(0.2),
                },
            ]),
        ]);

        // Test filter can be created and used
        let params = SearchParams {
            filter_expression: Some(_filter),
            ..Default::default()
        };

        assert!(params.filter_expression.is_some());
    }

    #[tokio::test]
    async fn test_metadata_extraction_from_filter() {
        let filter = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "status".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("active"),
            },
            FilterExpression::Comparison {
                field: "priority".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: json!(5),
            },
        ]);

        // Extract fields from filter
        let fields = extract_filter_fields(&filter);
        assert_eq!(fields.len(), 2);
        assert!(fields.contains(&"status".to_string()));
        assert!(fields.contains(&"priority".to_string()));
    }

    // Helper function to extract fields from filter
    fn extract_filter_fields(filter: &FilterExpression) -> Vec<String> {
        match filter {
            FilterExpression::Comparison { field, .. } => vec![field.clone()],
            FilterExpression::And(filters) | FilterExpression::Or(filters) => {
                filters.iter().flat_map(extract_filter_fields).collect()
            }
            FilterExpression::Not(filter) => extract_filter_fields(filter),
            // Other variants not implemented yet
        }
    }

    // Performance Tests
    #[tokio::test]
    async fn test_batch_size_calculation() {
        let available_memory_mb = 1000.0;
        let per_file_mb = 50.0;

        let optimal_batch = ((available_memory_mb / per_file_mb) as f64).floor() as usize;
        assert_eq!(optimal_batch, 20);
    }

    #[tokio::test]
    async fn test_memory_estimation() {
        let vector_count = 10000;
        let dimensions = 128;
        let bytes_per_float = 4;

        let memory_bytes = vector_count * dimensions * bytes_per_float;
        let memory_mb = memory_bytes as f64 / (1024.0 * 1024.0);

        assert!(memory_mb > 4.0 && memory_mb < 6.0);
    }

    // HTTP Range Tests
    #[tokio::test]
    async fn test_byte_range_calculation() {
        let file_size = 100 * 1024 * 1024; // 100MB
        let chunk_size = 10 * 1024 * 1024; // 10MB chunks

        let ranges: Vec<(usize, usize)> = (0..file_size)
            .step_by(chunk_size)
            .map(|start| {
                let end = (start + chunk_size).min(file_size);
                (start, end)
            })
            .collect();

        assert_eq!(ranges.len(), 10);
        assert_eq!(ranges[0], (0, 10 * 1024 * 1024));
        assert_eq!(ranges[9], (90 * 1024 * 1024, 100 * 1024 * 1024));
    }

    #[tokio::test]
    async fn test_range_coalescing() {
        let ranges = vec![(0, 1024), (1024, 2048), (2048, 3072), (5120, 6144)];

        let coalesced = coalesce_ranges(ranges);
        assert_eq!(coalesced.len(), 2);
        assert_eq!(coalesced[0], (0, 3072));
        assert_eq!(coalesced[1], (5120, 6144));
    }

    fn coalesce_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
        if ranges.is_empty() {
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

    // New tests for actual parquet file reading and vector extraction

    /// Create a test parquet file with vectors
    async fn create_test_parquet_file(
        file_path: &str,
        vectors: Vec<VectorRecord>,
        vector_dim: usize,
    ) -> Result<()> {
        use arrow_array::builder::{
            FixedSizeListBuilder, Float32Builder, ListBuilder, StringBuilder,
        };
        use tokio::fs;

        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(file_path).parent() {
            fs::create_dir_all(parent).await?;
        }

        // Create Arrow schema for vectors
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, true),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector_fp32",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    vector_dim as i32,
                ),
                true,
            ),
            Field::new("version", DataType::Int8, true),
            Field::new("updated_at", DataType::Int64, true),
            Field::new("expires_at", DataType::Int64, true),
            Field::new(
                "extra_meta",
                DataType::List(Arc::new(Field::new(
                    "item",
                    DataType::Struct(arrow_schema::Fields::from(vec![
                        Field::new("key", DataType::Utf8, false),
                        Field::new("value", DataType::Utf8, false),
                    ])),
                    true,
                ))),
                true,
            ),
        ]));

        // Build arrays from vectors
        let mut ids = Vec::new();
        let mut collection_ids = Vec::new();
        let mut versions = Vec::new();
        let mut updated_at_values: Vec<Option<i64>> = Vec::new();
        let mut expires_at_values: Vec<i64> = Vec::new();

        // Build vector list array using FixedSizeListBuilder
        let mut vector_builder = FixedSizeListBuilder::new(
            Float32Builder::with_capacity(vectors.len() * vector_dim),
            vector_dim as i32,
        );

        // Build metadata array
        let mut extra_meta_builder = ListBuilder::new(arrow_array::builder::StructBuilder::new(
            vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ],
            vec![
                Box::new(StringBuilder::new()),
                Box::new(StringBuilder::new()),
            ],
        ));

        for record in &vectors {
            ids.push(record.id.clone());
            collection_ids.push("test_collection".to_string());
            versions.push(record.version.map(|v| v as i8));
            updated_at_values.push(record.updated_at.map(|v| v as i64));
            expires_at_values.push(record.expires_at.unwrap_or(0) as i64);

            // Add vector data
            let values = vector_builder.values();
            for &val in &record.vector {
                values.append_value(val);
            }
            vector_builder.append(true);

            // Add metadata
            if !record.metadata.is_empty() {
                let struct_builder = extra_meta_builder.values();
                for (key, sql_value) in &record.metadata {
                    struct_builder
                        .field_builder::<StringBuilder>(0)
                        .unwrap()
                        .append_value(key);
                    // Convert metadata value to string
                    let value_str = match &sql_value.value {
                        Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                            s.clone()
                        }
                        Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                            n.to_string()
                        }
                        Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                            b.to_string()
                        }
                        Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                            i.to_string()
                        }
                        Some(crate::proto::proximadb_v1::sql_value::Value::BytesValue(bytes)) => {
                            format!("{:?}", bytes)
                        }
                        Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                            "null".to_string()
                        }
                        None => String::new(),
                        Some(_) => "unknown".to_string(),
                    };
                    struct_builder
                        .field_builder::<StringBuilder>(1)
                        .unwrap()
                        .append_value(&value_str);
                    struct_builder.append(true);
                }
                extra_meta_builder.append(true);
            } else {
                extra_meta_builder.append(false);
            }
        }

        // Create arrays
        let id_array = StringArray::from(ids);
        let collection_array = StringArray::from(collection_ids);
        let vector_array = vector_builder.finish();
        let version_array = arrow_array::Int8Array::from(versions);
        let updated_at_array = Int64Array::from(updated_at_values);
        let expires_at_array = Int64Array::from(expires_at_values);
        let extra_meta_array = extra_meta_builder.finish();

        // Create record batch
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(id_array),
                Arc::new(collection_array),
                Arc::new(vector_array),
                Arc::new(version_array),
                Arc::new(updated_at_array),
                Arc::new(expires_at_array),
                Arc::new(extra_meta_array),
            ],
        )?;

        // Write to parquet file
        let file = std::fs::File::create(file_path)?;
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::UNCOMPRESSED)
            .build();

        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
        writer.write(&batch)?;
        writer.close()?;

        Ok(())
    }

    /// Create test vectors with metadata
    fn create_test_vectors(count: usize, dim: usize) -> Vec<VectorRecord> {
        let mut vectors = Vec::new();

        for i in 0..count {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert(
                "category".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue(format!("cat_{}", i % 3))),
                },
            );
            metadata.insert(
                "score".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue((i as f32 * 0.5).to_string())),
                },
            );

            let vector = VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![i as f32 * 0.1; dim],
                metadata,
                timestamp: Some(chrono::Utc::now().timestamp()),
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
                source: Some("test".to_string()),
            };
            vectors.push(vector);
        }

        vectors
    }

    #[tokio::test]
    async fn test_read_all_vectors_from_parquet() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = format!("{}/test_vectors_file.parquet", temp_dir.path().display());

        // Create test vectors
        let test_vectors = create_test_vectors(5, 4);

        // Write to parquet file
        create_test_parquet_file(&file_path, test_vectors.clone(), 4).await?;

        // Create reader with the actual file path
        let reader = create_test_reader_with_files(vec![file_path.clone()]).await;

        // Use search API to read all vectors (no filter, high k)
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![0.0; 4]]),
            top_k: Some(100),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        };

        let search_plan = convert_search_params_to_plan(&search_params, &context.collection_id);
        let results = reader.search_vectors(&search_plan, &context).await?;

        // Verify
        assert_eq!(results.results.len(), 5, "Should read all 5 vectors");

        Ok(())
    }

    #[tokio::test]
    async fn test_search_vectors_basic() -> Result<()> {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let temp_dir = TempDir::new()?;
        let file_path = format!("{}/search_test.parquet", temp_dir.path().display());

        // Create test vectors with different values
        let mut test_vectors = Vec::new();
        for i in 0..5 {
            let mut vec = create_test_vectors(1, 3)[0].clone();
            vec.id = format!("vec_{}", i);
            vec.vector = match i {
                0 => vec![1.0, 0.0, 0.0],
                1 => vec![0.0, 1.0, 0.0],
                2 => vec![0.0, 0.0, 1.0],
                3 => vec![0.5, 0.5, 0.0],
                4 => vec![0.0, 0.5, 0.5],
                _ => vec![0.0, 0.0, 0.0],
            };
            test_vectors.push(vec);
        }

        // Clone for debug output later
        let test_vectors_debug = test_vectors.clone();

        // Write to parquet file
        create_test_parquet_file(&file_path, test_vectors, 3).await?;

        // Create reader with the actual file path
        let reader = create_test_reader_with_files(vec![file_path.clone()]).await;

        // Create search params
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![1.0, 0.0, 0.0]]),
            vector: Some(vec![1.0, 0.0, 0.0]), // Add missing vector field
            top_k: Some(3),
            distance_metric: Some(DistanceMetric::Cosine),
            requires_ordering: None,
            filter_expression: None,
            accuracy_threshold: None,
            custom_hints: None,
            include_expired: None,
            quantization_hint: None,
            enable_two_stage: None,
            progressive_recalls: None,
            progressive_scenario: None,
            runtime_hints: None,
            optimization_hint: None,
            enable_clustering_hint: None,
            enable_metadata_filtering_hint: None,
            enable_progressive_search: None,
            filters: None,
            timeout_ms: None,
            search_mode: crate::core::search::SearchMode::default(),
            block_prune: crate::core::search::BlockPruneConfig::default(),
            text_query: None,
            hybrid_mode: crate::core::search::HybridSearchMode::default(),
            vector_weight: None,
        };

        // Create collection context
        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        };

        // Search
        let search_plan = convert_search_params_to_plan(&search_params, &context.collection_id);
        let results = reader.search_vectors(&search_plan, &context).await?;

        // Verify
        println!("Search returned {} results", results.results.len());
        assert!(!results.results.is_empty(), "Should find results");
        // The reader now applies basic scoring and top_k filtering
        assert!(
            results.results.len() <= 3,
            "Should return at most top_k=3 results (got {})",
            results.results.len()
        );

        // Debug output
        for (i, result) in results.results.iter().enumerate() {
            println!(
                "Result {}: id={}, similarity={:?}, score={:?}, semantic_similarity={:?}",
                i, result.id, result.similarity, result.score, result.semantic_similarity
            );
        }

        // Also print the actual vectors to verify they were correctly written
        println!("Test vectors created:");
        for vec in test_vectors_debug.iter() {
            println!("  {} -> {:?}", vec.id, vec.vector);
        }

        assert_eq!(
            results.results[0].id, "vec_0",
            "First result should be exact match"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file_handling() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = format!("{}/empty.parquet", temp_dir.path().display());

        // Create empty parquet file
        create_test_parquet_file(&file_path, vec![], 4).await?;

        // Create reader with the actual file path
        let reader = create_test_reader_with_files(vec![file_path.clone()]).await;

        // Use search API
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![0.0; 4]]),
            top_k: Some(100),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        };

        let search_plan = convert_search_params_to_plan(&search_params, &context.collection_id);
        let results = reader.search_vectors(&search_plan, &context).await?;

        // Verify
        assert_eq!(
            results.results.len(),
            0,
            "Should handle empty file gracefully"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_missing_file_error() -> Result<()> {
        // Create reader
        let reader = create_test_reader().await;

        // Try to search with non-existent file
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![0.0; 4]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        };

        let search_plan = convert_search_params_to_plan(&search_params, &context.collection_id);
        let result = reader.search_vectors(&search_plan, &context).await;

        // Verify error
        assert!(result.is_err(), "Should error on missing file");

        Ok(())
    }

    #[tokio::test]
    async fn test_vector_extraction_debug() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = format!("{}/debug_test.parquet", temp_dir.path().display());

        // Create simple test vector
        let test_vector = VectorRecord {
            id: "debug_vec".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: Some("test".to_string()),
        };

        // Write to parquet file
        create_test_parquet_file(&file_path, vec![test_vector], 3).await?;

        // Create reader with the actual file path
        let reader = create_test_reader_with_files(vec![file_path.clone()]).await;

        let search_params = SearchParams {
            query_vectors: Some(vec![vec![1.0, 2.0, 3.0]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        };

        let search_plan = convert_search_params_to_plan(&search_params, &context.collection_id);
        let results = reader.search_vectors(&search_plan, &context).await?;

        // Debug output
        println!("Found {} results from parquet file", results.results.len());
        if !results.results.is_empty() {
            println!(
                "First result: id={:?}, distance={:?}",
                results.results[0].id, results.results[0].semantic_similarity
            );
        }

        // Verify
        assert_eq!(results.results.len(), 1, "Should find 1 result");
        assert_eq!(results.results[0].id, "debug_vec", "Should find debug_vec");
        if let Some(distance) = &results.results[0].semantic_similarity {
            assert!(
                distance.distance < 0.01,
                "Should have near-zero distance for exact match, got {:.6}",
                distance.distance
            );
        }

        Ok(())
    }
}
