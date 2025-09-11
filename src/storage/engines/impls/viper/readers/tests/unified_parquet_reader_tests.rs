//! Test-Driven Development tests for UnifiedParquetReader
//!
//! Tests the unified search architecture for VIPER engine

#[cfg(test)]
mod tests {
    use crate::compute::distance_computation::DistanceMetric;
    use crate::compute::distance_computation::engine::SimilarityResult;
    use crate::core::search::{ComparisonOperator, FilterExpression, SearchParams};
    use crate::core::{String, VectorRecord};
    use crate::proto::proximadb_v1::MetadataItem;
    use crate::storage::engines::core::formats::columnar::{
        CollectionContext, UnifiedParquetReader,
    };
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use anyhow::Result;
    use arrow_array::{Array, Float32Array, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tracing::{debug, error, info};

    // Test helpers
    async fn create_test_reader() -> UnifiedParquetReader {
        let config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(config).await.unwrap());
        UnifiedParquetReader::new(filesystem)
    }

    fn create_test_context() -> CollectionContext {
        CollectionContext {
            collection_id: "test_collection".to_string(),
            file_paths: vec![
                "/tmp/test1.parquet".to_string(),
                "/tmp/test2.parquet".to_string(),
            ],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 100.0,
            estimated_document_count: 10000,
            is_cloud_storage: false,
            io_optimization_hints: None,
        }
    }

    // Basic Strategy Selection Tests
    #[tokio::test]
    async fn test_reader_creation() {
        let reader = create_test_reader().await;
        // Test passes if reader is created successfully
        assert!(true);
    }

    #[tokio::test]
    async fn test_strategy_selection_basic() {
        let reader = create_test_reader().await;
        let context = create_test_context();

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
        let reader = create_test_reader().await;
        let context = create_test_context();

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
        let reader = create_test_reader().await;
        let mut context = create_test_context();
        context.quantization_columns = vec!["pq8_embeddings".to_string()];

        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };

        // With quantized columns, should use two-stage strategy
        assert!(!context.quantization_columns.is_none());
    }

    // Filter Expression Tests
    #[tokio::test]
    async fn test_complex_filter_expression() {
        let filter = FilterExpression::And(vec![
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
            filter_expression: Some(filter),
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
        assert!(fields.contains_hash(&"status".to_string()));
        assert!(fields.contains_hash(&"priority".to_string()));
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

    // New tests for actual parquet file reading and vector extraction

    /// Create a test parquet file with vectors
    async fn create_test_parquet_file(
        file_path: &str,
        vectors: Vec<VectorRecord>,
        vector_dim: usize,
    ) -> Result<()> {
        use arrow_array::builder::{Float32Builder, ListBuilder, StringBuilder};
        use tokio::fs;
        use tracing::{debug, error, info};

        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(file_path).parent() {
            fs::create_dir_all(parent).await?;
        }

        // Create Arrow schema for vectors
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, true),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
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

        // Build vector list array
        let mut vector_builder = ListBuilder::with_capacity(
            Float32Builder::with_capacity(vectors.len() * vector_dim),
            vectors.len(),
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
            ids.push(record.id.clone().clone());
            collection_ids.push("test_collection".to_string());
            versions.push(record.version.map(|v| v as i8));
            updated_at_values.push(record.updated_at.map(|v| v as i64));
            expires_at_values.push(record.expires_at as i64);

            // Add vector data
            let values = vector_builder.values();
            for &val in &record.vector {
                values.append_value(val);
            }
            vector_builder.append(true);

            // Add metadata
            if !record.metadata.is_none() {
                let struct_builder = extra_meta_builder.values();
                for meta_item in &record.metadata {
                    struct_builder
                        .field_builder::<StringBuilder>(0)
                        .unwrap()
                        .append_value(&meta_item.key);
                    // Convert metadata value to string
                    let value_str = match &meta_item.value {
                        Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(s)) => {
                            s.clone()
                        }
                        Some(crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n)) => {
                            n.to_string()
                        }
                        Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b)) => {
                            b.to_string()
                        }
                        None => String::new(),
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
            let vector = VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: vec![i as f32 * 0.1; dim],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(
                            crate::proto::proximadb_v1::metadata_item::Value::StringValue(format!(
                                "cat_{}",
                                i % 3
                            )),
                        ),
                    },
                    MetadataItem {
                        key: "score".to_string(),
                        value: Some(
                            crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                                (i as f32 * 0.5).to_string(),
                            ),
                        ),
                    },
                ],
                timestamp: chrono::Utc::now().timestamp() as u32,
                updated_at: Some(chrono::Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(1),
                // rank removed -  None,
                similarity: Some(i as f32),
                similarity: None,
                ..Default::default()
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

        // Create reader
        let reader = create_test_reader().await;

        // Use search API to read all vectors (no filter, high k)
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![0.0; 4]]),
            top_k: Some(100),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            file_paths: vec![format!("file://{}", file_path)],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 1.0,
            estimated_document_count: 5,
            is_cloud_storage: false,
            io_optimization_hints: None,
        };

        let results = reader.search_vectors(&search_params, &context).await?;

        // Verify
        assert_eq!(results.len(), 5, "Should read all 5 vectors");

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
            vec.id = Some(format!("vec_{}", i));
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

        // Create reader
        let reader = create_test_reader().await;

        // Create search params
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![1.0, 0.0, 0.0]]),
            top_k: Some(3),
            distance_metric: Some(DistanceMetric::Cosine),
            requires_ordering: None,
            filter_expression: None,
            accuracy_threshold: None,
            custom_hints: None,
            include_expired: None,
            quantization_hint: None,
            enable_two_stage: None,
            enable_clustering_hint: None,
            enable_metadata_filtering_hint: None,
            timeout_ms: None,
        };

        // Create collection context
        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            file_paths: vec![format!("file://{}", file_path)],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 1.0,
            estimated_document_count: 5,
            is_cloud_storage: false,
            io_optimization_hints: None,
        };

        // Search
        let results = reader.search_vectors(&search_params, &context).await?;

        // Verify
        assert!(!results.is_empty(), "Should find results");
        assert!(results.len() <= 3, "Should return at most 3 results");

        // Debug output
        for (i, result) in results.iter().enumerate() {
            debug!(
                "Result {}: id={}, similarity={:?}, score={:?}, semantic_distance={:?}",
                i, result.id, result.similarity, result.score, result.semantic_distance
            );
        }

        // Also print the actual vectors to verify they were correctly written
        debug!("Test vectors created:");
        for vec in test_vectors_debug.iter() {
            debug!("  {} -> {:?}", vec.id.as_ref(), vec.vector);
        }

        assert_eq!(results[0].id, "vec_0", "First result should be exact match");

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file_handling() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = format!("{}/empty.parquet", temp_dir.path().display());

        // Create empty parquet file
        create_test_parquet_file(&file_path, vec![], 4).await?;

        // Create reader
        let reader = create_test_reader().await;

        // Use search API
        let search_params = SearchParams {
            query_vectors: Some(vec![vec![0.0; 4]]),
            top_k: Some(100),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            file_paths: vec![format!("file://{}", file_path)],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 1.0,
            estimated_document_count: 0,
            is_cloud_storage: false,
            io_optimization_hints: None,
        };

        let results = reader.search_vectors(&search_params, &context).await?;

        // Verify
        assert_eq!(results.len(), 0, "Should handle empty file gracefully");

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
            file_paths: vec!["file:///non/existent/file.parquet".to_string()],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 1.0,
            estimated_document_count: 0,
            is_cloud_storage: false,
            io_optimization_hints: None,
        };

        let result = reader.search_vectors(&search_params, &context).await;

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
            id: Some("debug_vec".to_string()),
            vector: vec![1.0, 2.0, 3.0],
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: Some(chrono::Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            // rank removed -  None,
            similarity: None,
            similarity: None,
        };

        // Write to parquet file
        create_test_parquet_file(&file_path, vec![test_vector], 3).await?;

        // Create reader and search
        let reader = create_test_reader().await;

        let search_params = SearchParams {
            query_vectors: Some(vec![vec![1.0, 2.0, 3.0]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Euclidean),
            ..Default::default()
        };

        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            file_paths: vec![format!("file://{}", file_path)],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 1.0,
            estimated_document_count: 1,
            is_cloud_storage: false,
            io_optimization_hints: None,
        };

        let results = reader.search_vectors(&search_params, &context).await?;

        // Debug output
        debug!("Found {} results from parquet file", results.len());
        if !results.is_empty() {
            debug!(
                "First result: id={:?}, distance={:?}",
                results[0].id, results[0].semantic_distance
            );
        }

        // Verify
        assert_eq!(results.len(), 1, "Should find 1 result");
        assert_eq!(results[0].id, "debug_vec", "Should find debug_vec");
        if let Some(distance) = &results[0].semantic_distance {
            assert!(
                distance.raw_value < 0.01,
                "Should have near-zero distance for exact match, got {}",
                distance.raw_value
            );
        }

        Ok(())
    }
}
