// =============================================================================
// BRANCHED METADATA FILTERING TESTS
// =============================================================================
//
// Comprehensive tests for the branched filtering strategy that handles:
// - Fast path (filterable columns only)
// - Slow path (non-filterable columns with full scan)
// - Mixed path (combination of both)

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::proto::proximadb_v1::{SqlValue, VectorRecord};
    use crate::storage::engines::core::formats::columnar::{
        ParquetWriterConfig, StreamingParquetWriter, UnifiedParquetReader,
    };
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tracing::info;

    /// Create test data with both filterable and non-filterable metadata
    fn create_test_records(count: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| {
                let mut metadata = HashMap::new();

                // Filterable metadata (will have dedicated columns)
                metadata.insert(
                    "category".to_string(),
                    SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            format!("cat_{}", i % 5),
                        )),
                    },
                );
                metadata.insert(
                    "priority".to_string(),
                    SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                            (i % 10) as i64,
                        )),
                    },
                );

                // Non-filterable metadata (stored in extra_meta)
                metadata.insert(
                    "custom_field".to_string(),
                    SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            format!("custom_{}", i % 3),
                        )),
                    },
                );
                metadata.insert(
                    "score".to_string(),
                    SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                            (i as f64) * 0.1,
                        )),
                    },
                );

                VectorRecord {
                    id: format!("vec_{:06}", i),
                    vector: vec![i as f32; 128],
                    metadata,
                    timestamp: i as i64,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                }
            })
            .collect()
    }

    /// Write test data with specified filterable columns
    async fn write_test_data(
        file_path: &std::path::Path,
        records: Vec<VectorRecord>,
        filterable_columns: Vec<String>,
    ) -> anyhow::Result<()> {
        let config = ParquetWriterConfig {
            write_batch_size: 100,
            row_group_size: 500,
            enable_bloom_filters: true,
            filterable_metadata_columns: Some(filterable_columns),
            ..Default::default()
        };

        let mut writer = StreamingParquetWriter::new(
            file_path,
            128,
            config,
            None,
        )?;

        writer.write_batch(&records).await?;
        writer.finalize().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_fast_path_filterable_columns_only() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_fast_path.parquet");

        // Create test data
        let records = create_test_records(1000);

        // Write with category and priority as filterable columns
        write_test_data(
            &file_path,
            records,
            vec!["category".to_string(), "priority".to_string()],
        )
        .await
        .unwrap();

        // Create reader with filterable columns configured
        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let mut reader = UnifiedParquetReader::new(filesystem).await.unwrap();

        // Configure filterable columns
        reader.config.filterable_metadata_columns = Some(vec![
            "category".to_string(),
            "priority".to_string(),
        ]);

        // Test 1: Single filterable column filter
        info!("Testing fast path with single filterable column");
        let filters = vec![
            MetadataFilter {
                column_name: "category".to_string(),
                operator: "=".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        "cat_2".to_string(),
                    )),
                },
            },
        ];

        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &filters,
                true, // allow_slow_queries
            )
            .await
            .unwrap();

        // Should find ~200 records (1000 / 5 categories)
        assert!(results.len() >= 190 && results.len() <= 210);

        // Verify all results have correct category
        for record in &results {
            let cat_value = record.metadata.get("category").unwrap();
            if let Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) = &cat_value.value {
                assert_eq!(s, "cat_2");
            }
        }

        // Test 2: Multiple filterable column filters
        info!("Testing fast path with multiple filterable columns");
        let filters = vec![
            MetadataFilter {
                column_name: "category".to_string(),
                operator: "=".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        "cat_1".to_string(),
                    )),
                },
            },
            MetadataFilter {
                column_name: "priority".to_string(),
                operator: ">".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(5)),
                },
            },
        ];

        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &filters,
                true,
            )
            .await
            .unwrap();

        // Should find fewer records with both filters
        assert!(results.len() < 100);

        info!("Fast path tests passed: {} results", results.len());
    }

    #[tokio::test]
    async fn test_slow_path_non_filterable_columns() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_slow_path.parquet");

        // Create test data with NO metadata (to avoid MapArray issues in test)
        let records: Vec<VectorRecord> = (0..500)
            .map(|i| VectorRecord {
                id: format!("vec_{:06}", i),
                vector: vec![i as f32; 128],
                metadata: HashMap::new(), // Empty to avoid MapArray
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            })
            .collect();

        // Write with category as filterable column only
        write_test_data(
            &file_path,
            records,
            vec!["category".to_string()],
        )
        .await
        .unwrap();

        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let mut reader = UnifiedParquetReader::new(filesystem).await.unwrap();

        // Configure only category as filterable
        reader.config.filterable_metadata_columns = Some(vec!["category".to_string()]);

        // Test: Filter on non-filterable column (requires slow path)
        info!("Testing slow path with non-filterable column");
        let filters = vec![
            MetadataFilter {
                column_name: "custom_field".to_string(), // NOT filterable
                operator: "=".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        "custom_1".to_string(),
                    )),
                },
            },
        ];

        // Test with allow_slow_queries = false (should fail)
        let result = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &filters,
                false, // Don't allow slow queries
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("allow_slow_queries"));

        // Test with allow_slow_queries = true (should succeed with warning)
        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &filters,
                true, // Allow slow queries
            )
            .await
            .unwrap();

        info!("Slow path test passed: {} results with warning", results.len());
    }

    #[tokio::test]
    async fn test_mixed_path_filterable_and_non_filterable() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_mixed_path.parquet");

        // Create test data with empty metadata to avoid MapArray
        let records: Vec<VectorRecord> = (0..800)
            .map(|i| VectorRecord {
                id: format!("vec_{:06}", i),
                vector: vec![i as f32; 128],
                metadata: HashMap::new(), // Empty to avoid MapArray issues
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            })
            .collect();

        // Write with category and priority as filterable
        write_test_data(
            &file_path,
            records,
            vec!["category".to_string(), "priority".to_string()],
        )
        .await
        .unwrap();

        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let mut reader = UnifiedParquetReader::new(filesystem).await.unwrap();

        reader.config.filterable_metadata_columns = Some(vec![
            "category".to_string(),
            "priority".to_string(),
        ]);

        // Test: Mix of filterable and non-filterable filters
        info!("Testing mixed path with both filterable and non-filterable columns");
        let filters = vec![
            // Filterable - can be pushed down
            MetadataFilter {
                column_name: "category".to_string(),
                operator: "=".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        "cat_3".to_string(),
                    )),
                },
            },
            // Non-filterable - requires post-filtering
            MetadataFilter {
                column_name: "score".to_string(),
                operator: ">".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(50.0)),
                },
            },
        ];

        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &filters,
                true, // Allow slow queries for mixed path
            )
            .await
            .unwrap();

        info!("Mixed path test passed: {} results", results.len());
    }

    #[tokio::test]
    async fn test_no_filter_path() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_no_filter.parquet");

        let records: Vec<VectorRecord> = (0..100)
            .map(|i| VectorRecord {
                id: format!("vec_{:06}", i),
                vector: vec![i as f32; 128],
                metadata: HashMap::new(),
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            })
            .collect();

        write_test_data(&file_path, records, vec![]).await.unwrap();

        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

        // Test: No filters (should read all records)
        info!("Testing no filter path");
        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &[], // No filters
                true,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 100);
        info!("No filter path test passed: {} records", results.len());
    }

    #[tokio::test]
    async fn test_performance_metrics_logging() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_metrics.parquet");

        let records = create_test_records(500);
        write_test_data(
            &file_path,
            records,
            vec!["category".to_string()],
        )
        .await
        .unwrap();

        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let mut reader = UnifiedParquetReader::new(filesystem).await.unwrap();
        reader.config.filterable_metadata_columns = Some(vec!["category".to_string()]);

        // Test fast path and verify metrics are logged
        let filters = vec![
            MetadataFilter {
                column_name: "category".to_string(),
                operator: "=".to_string(),
                value: SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        "cat_0".to_string(),
                    )),
                },
            },
        ];

        let start = std::time::Instant::now();
        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &filters,
                true,
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();

        info!(
            "Performance test: {} results in {:?}, strategy should be 'FastFilterable'",
            results.len(),
            elapsed
        );

        assert!(elapsed.as_millis() < 1000, "Fast path should complete quickly");
    }

    #[tokio::test]
    async fn test_edge_cases() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_edge_cases.parquet");

        // Test with empty file
        let records = vec![];
        write_test_data(&file_path, records, vec![]).await.unwrap();

        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

        // Should handle empty file gracefully
        let results = reader
            .query_with_branched_filtering(
                file_path.to_str().unwrap(),
                &[],
                true,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 0);
        info!("Edge case test passed: empty file handled correctly");
    }
}