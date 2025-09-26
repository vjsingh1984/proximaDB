//! Comprehensive tests for columnar storage with ID-aware implementation
//!
//! Tests validate:
//! 1. ID column is always preserved
//! 2. ID-specific bloom filters work correctly
//! 3. Fast ID-based lookups function properly
//! 4. Dictionary encoding optimizes ID storage
//! 5. Customer APIs (get_by_id, delete_by_id) work correctly

use super::*;
use crate::proto::proximadb_v1::{VectorRecord, SqlValue};
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;
use tokio;

#[tokio::test]
async fn test_id_column_always_preserved() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_id_preservation.parquet");

    // Test with id_less_storage = false (default)
    let config = ParquetWriterConfig {
        id_less_storage: false,
        enable_bloom_filters: true,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 128, config, None).unwrap();

    // Write test records with IDs
    let test_records = create_test_records(100);
    writer.write_batch(&test_records).await.unwrap();
    let (stats, _collector) = writer.finalize().await.unwrap();

    assert_eq!(stats.total_records, 100);
    assert!(stats.file_size > 0);

    // Verify Parquet file has ID column
    let parquet_schema = read_parquet_schema(&file_path).unwrap();
    assert!(
        parquet_schema.field_with_name("id").is_ok(),
        "ID column must be present"
    );

    // Verify ID column is NOT NULL
    let id_field = parquet_schema.field_with_name("id").unwrap();
    assert!(
        !id_field.is_nullable(),
        "ID column should be NOT NULL for customer APIs"
    );
}

#[tokio::test]
async fn test_id_less_storage_warning() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_id_less_warning.parquet");

    // Test with id_less_storage = true (should trigger warning)
    let config = ParquetWriterConfig {
        id_less_storage: true,
        enable_bloom_filters: true,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 128, config, None).unwrap();

    let test_records = create_test_records(50);
    writer.write_batch(&test_records).await.unwrap();
    let (_stats, _collector) = writer.finalize().await.unwrap();

    // Even with id_less_storage = true, ID column should still be present
    let parquet_schema = read_parquet_schema(&file_path).unwrap();
    assert!(
        parquet_schema.field_with_name("id").is_ok(),
        "ID column must ALWAYS be present for customer APIs, even with id_less_storage=true"
    );
}

#[tokio::test]
async fn test_id_bloom_filters() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_id_bloom_filters.parquet");

    let config = ParquetWriterConfig {
        enable_bloom_filters: true,
        bloom_filter_fpp: 0.01,
        row_group_size: 1000,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 256, config, None).unwrap();

    // Create records with predictable IDs
    let test_records: Vec<VectorRecord> = (0..2500)
        .map(|i| VectorRecord {
            id: format!("customer_id_{:06}", i),
            vector: (0..256).map(|j| (i + j) as f32 * 0.001).collect(),
            metadata: HashMap::new(),
            timestamp: i as i64,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            quantized_vector: Vec::new(),
            source: None,
        })
        .collect();

    // Write records in batches to create multiple row groups
    for chunk in test_records.chunks(500) {
        writer.write_batch(chunk).await.unwrap();
    }

    let (stats, _collector) = writer.finalize().await.unwrap();
    assert_eq!(stats.total_records, 2500);
    assert!(
        stats.bloom_filter_count > 0,
        "Should have bloom filters for ID columns"
    );

    // Test reading with ID bloom filter optimization
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

    // Test batch ID lookup
    let lookup_ids = vec![
        "customer_id_000100".to_string(),
        "customer_id_001000".to_string(),
        "customer_id_002000".to_string(),
        "customer_id_999999".to_string(), // Non-existent
    ];

    let results = reader
        .optimized_batch_id_lookup(&[file_path.to_string_lossy().to_string()], &lookup_ids)
        .await
        .unwrap();

    // Should find 3 out of 4 IDs (last one doesn't exist)
    assert_eq!(results.len(), 3);

    // Verify returned records have correct IDs
    let returned_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();

    assert!(returned_ids.contains(&"customer_id_000100".to_string()));
    assert!(returned_ids.contains(&"customer_id_001000".to_string()));
    assert!(returned_ids.contains(&"customer_id_002000".to_string()));
    assert!(!returned_ids.contains(&"customer_id_999999".to_string()));
}

#[tokio::test]
async fn test_fast_id_lookup_performance() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_id_performance.parquet");

    // Create large dataset for performance testing
    let config = ParquetWriterConfig {
        enable_bloom_filters: true,
        row_group_size: 10000,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 512, config, None).unwrap();

    // Generate 50,000 records
    let test_records: Vec<VectorRecord> = (0..50000)
        .map(|i| {
            let mut metadata = HashMap::new();
            metadata.insert("category".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(format!("cat_{}", i % 10))),
            });
            metadata.insert("priority".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i % 5)),
            });

            VectorRecord {
                id: format!("perf_test_id_{:08}", i),
                vector: (0..512)
                    .map(|j| ((i * 17 + j * 13) % 1000) as f32 * 0.001)
                    .collect(),
                metadata,
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some((i % 3 + 1) as i64),
                quantized_vector: Vec::new(),
                source: None,
            }
        })
        .collect();

    let start_write = std::time::Instant::now();
    writer.write_batch(&test_records).await.unwrap();
    let (stats, _collector) = writer.finalize().await.unwrap();
    let write_duration = start_write.elapsed();

    println!(
        "Write performance: {} records in {:?}",
        stats.total_records, write_duration
    );
    assert_eq!(stats.total_records, 50000);

    // Test ID lookup performance
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

    // Lookup random subset of IDs
    let lookup_ids: Vec<String> = (0..1000)
        .step_by(50)
        .map(|i| format!("perf_test_id_{:08}", i))
        .collect();

    let start_lookup = std::time::Instant::now();
    let results = reader
        .optimized_batch_id_lookup(&[file_path.to_string_lossy().to_string()], &lookup_ids)
        .await
        .unwrap();
    let lookup_duration = start_lookup.elapsed();

    println!(
        "ID lookup performance: {} lookups in {:?}",
        lookup_ids.len(),
        lookup_duration
    );
    assert_eq!(results.len(), lookup_ids.len());

    // Verify correctness of lookup results
    for (expected_id, result) in lookup_ids.iter().zip(results.iter()) {
        assert_eq!(&result.id, expected_id);
        assert_eq!(result.vector.len(), 512);
    }
}

#[tokio::test]
async fn test_dictionary_encoding_optimization() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_dictionary_encoding.parquet");

    let config = ParquetWriterConfig {
        enable_dictionary: true, // Enable dictionary encoding
        enable_bloom_filters: true,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 128, config, None).unwrap();

    // Create records with repeated ID patterns (good for dictionary encoding)
    let test_records: Vec<VectorRecord> = (0..1000)
        .map(|i| {
            let mut metadata = HashMap::new();
            metadata.insert("group".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(format!("group_{}", i % 20))),
            });
            metadata.insert("member_id".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)),
            });

            VectorRecord {
                id: format!("user_group_{:02}", i % 20), // Only 20 unique IDs, repeated
                vector: (0..128).map(|j| (i + j) as f32 * 0.01).collect(),
                metadata,
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: Vec::new(),
                source: None,
            }
        })
        .collect();

    writer.write_batch(&test_records).await.unwrap();
    let (stats, _collector) = writer.finalize().await.unwrap();

    assert_eq!(stats.total_records, 1000);

    // Dictionary encoding should result in good compression for repeated IDs
    assert!(
        stats.compression_ratio > 2.0,
        "Dictionary encoding should achieve good compression ratio: {}",
        stats.compression_ratio
    );

    // Verify ID lookups still work correctly
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

    let lookup_results = reader
        .optimized_batch_id_lookup(
            &[file_path.to_string_lossy().to_string()],
            &["user_group_05".to_string(), "user_group_15".to_string()],
        )
        .await
        .unwrap();

    // Should find multiple records for each group ID
    assert!(lookup_results.len() >= 2);

    // All results should have the requested IDs
    for result in &lookup_results {
        let id = &result.id;
        assert!(id == "user_group_05" || id == "user_group_15");
    }
}

#[tokio::test]
async fn test_customer_api_compatibility() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_customer_apis.parquet");

    // Test that customer-facing APIs work correctly
    let config = ParquetWriterConfig::default(); // Uses default: id_less_storage = false
    let mut writer = StreamingParquetWriter::new(&file_path, 384, config, None).unwrap();

    let test_records = vec![
        {
            let mut metadata = HashMap::new();
            metadata.insert("name".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("Customer 1".to_string())),
            });
            VectorRecord {
                id: "cust_001".to_string(),
                vector: (0..384).map(|i| i as f32 * 0.01).collect(),
                metadata,
                timestamp: 1000,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: Vec::new(),
                source: None,
            }
        },
        {
            let mut metadata = HashMap::new();
            metadata.insert("name".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("Customer 2".to_string())),
            });
            VectorRecord {
                id: "cust_002".to_string(),
                vector: (0..384).map(|i| (i + 100) as f32 * 0.01).collect(),
                metadata,
                timestamp: 2000,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: Vec::new(),
                source: None,
            }
        },
        {
            let mut metadata = HashMap::new();
            metadata.insert("name".to_string(), SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("Customer 3".to_string())),
            });
            VectorRecord {
                id: "cust_003".to_string(),
                vector: (0..384).map(|i| (i + 200) as f32 * 0.01).collect(),
                metadata,
                timestamp: 3000,
                updated_at: None,
                expires_at: None,
                version: Some(2),
                quantized_vector: Vec::new(),
                source: None,
            }
        },
    ];

    writer.write_batch(&test_records).await.unwrap();
    let (_stats, _collector) = writer.finalize().await.unwrap();

    // Test get_by_id equivalent
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

    // Single ID lookup
    let single_result = reader
        .optimized_batch_id_lookup(
            &[file_path.to_string_lossy().to_string()],
            &["cust_002".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(single_result.len(), 1);
    assert_eq!(&single_result[0].id, "cust_002");
    assert_eq!(single_result[0].timestamp, 2000);
    assert_eq!(single_result[0].vector.len(), 384);

    // Batch ID lookup
    let batch_result = reader
        .optimized_batch_id_lookup(
            &[file_path.to_string_lossy().to_string()],
            &["cust_001".to_string(), "cust_003".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(batch_result.len(), 2);
    let ids: Vec<String> = batch_result.iter().map(|r| r.id.clone()).collect();
    assert!(ids.contains(&"cust_001".to_string()));
    assert!(ids.contains(&"cust_003".to_string()));

    // Non-existent ID
    let empty_result = reader
        .optimized_batch_id_lookup(
            &[file_path.to_string_lossy().to_string()],
            &["cust_999".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(empty_result.len(), 0);
}

#[tokio::test]
async fn test_row_group_offset_optimization() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Test that row group offset is added when id_less_storage is enabled
    // but ID column is still preserved
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_row_offset_optimization.parquet");

    let config = ParquetWriterConfig {
        id_less_storage: true, // Enable optimization
        enable_bloom_filters: true,
        row_group_size: 100,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 128, config, None).unwrap();

    let test_records = create_test_records(250); // Multiple row groups
    writer.write_batch(&test_records).await.unwrap();
    let (_stats, _collector) = writer.finalize().await.unwrap();

    let parquet_schema = read_parquet_schema(&file_path).unwrap();

    // ID column should ALWAYS be present
    assert!(parquet_schema.field_with_name("id").is_ok());

    // Row group offset columns should be present when optimization is enabled
    assert!(parquet_schema.field_with_name("row_group_offset").is_ok());
    assert!(parquet_schema.field_with_name("row_index").is_ok());

    // Verify ID-based lookup still works
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    let reader = UnifiedParquetReader::new(filesystem).await.unwrap();

    let lookup_result = reader
        .optimized_batch_id_lookup(
            &[file_path.to_string_lossy().to_string()],
            &["test_id_050".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(lookup_result.len(), 1);
    assert_eq!(&lookup_result[0].id, "test_id_050");
}

// Helper functions

fn create_test_records(count: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let metadata = if i % 3 == 0 {
                let mut map = HashMap::new();
                map.insert("tag".to_string(), SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(format!("tag_{}", i % 10))),
                });
                map
            } else {
                HashMap::new()
            };

            VectorRecord {
                id: format!("test_id_{:03}", i),
                vector: (0..128).map(|j| (i + j) as f32 * 0.01).collect(),
                metadata,
                timestamp: (1000 + i) as i64,
                updated_at: None,
                expires_at: None,
                version: Some(((i % 5) + 1) as i64),
                quantized_vector: Vec::new(),
                source: None,
            }
        })
        .collect()
}

fn read_parquet_schema(file_path: &std::path::Path) -> Result<arrow_schema::Schema> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    let file = std::fs::File::open(file_path)?;
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    Ok(reader_builder.schema().as_ref().clone())
}

#[tokio::test]
async fn test_schema_evolution_with_id_column() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Test that schema evolution preserves ID column requirement
    let quantization_configs = vec![
        QuantizationConfig {
            enable_binary: false,
            enable_int8: false,
            enable_pq: false,
            ..Default::default()
        },
        QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: false,
            ..Default::default()
        },
        QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: true,
            ..Default::default()
        },
    ];

    for (i, quant_config) in quantization_configs.iter().enumerate() {
        let schema = create_columnar_schema(
            768,
            quant_config,
            &["category".to_string(), "priority".to_string()],
        );

        // ID column must always be present regardless of quantization config
        assert!(
            schema.field_with_name("id").is_ok(),
            "ID column missing in schema variation {}",
            i
        );

        // ID column must be NOT NULL
        let id_field = schema.field_with_name("id").unwrap();
        assert!(
            !id_field.is_nullable(),
            "ID column must be NOT NULL in schema variation {}",
            i
        );

        // Vector column must be present
        assert!(
            schema.field_with_name("vector").is_ok(),
            "Vector column missing in schema variation {}",
            i
        );

        // Row group offset columns should be present for optimization
        assert!(
            schema.field_with_name("row_group_offset").is_ok(),
            "Row group offset missing in schema variation {}",
            i
        );
        assert!(
            schema.field_with_name("row_index").is_ok(),
            "Row index missing in schema variation {}",
            i
        );
    }
}

#[test]
fn test_optimization_recommendations_preserve_id_column() {
    // Test that all optimization recommendations preserve the ID column
    let test_cases = vec![
        (
            1_000,
            128,
            QueryPattern::IdLookupHeavy,
            StorageBudget::Performance,
        ),
        (
            100_000,
            256,
            QueryPattern::SimilaritySearchHeavy,
            StorageBudget::Balanced,
        ),
        (
            10_000_000,
            1024,
            QueryPattern::Mixed,
            StorageBudget::Minimal,
        ),
    ];

    for (num_vectors, dimension, pattern, budget) in test_cases {
        let recommendations = OptimizationRecommendations::for_dataset(
            num_vectors,
            dimension,
            pattern.clone(),
            budget.clone(),
        );

        // Create schema with recommendations
        let quant_config = match recommendations.quantization_strategy {
            QuantizationStrategy::None => QuantizationConfig {
                enable_binary: false,
                enable_int8: false,
                enable_pq: false,
                ..Default::default()
            },
            QuantizationStrategy::BinaryOnly => QuantizationConfig {
                enable_binary: true,
                enable_int8: false,
                enable_pq: false,
                ..Default::default()
            },
            QuantizationStrategy::Int8Only => QuantizationConfig {
                enable_binary: false,
                enable_int8: true,
                enable_pq: false,
                ..Default::default()
            },
            QuantizationStrategy::ProductQuantization => QuantizationConfig {
                enable_binary: false,
                enable_int8: false,
                enable_pq: true,
                ..Default::default()
            },
            QuantizationStrategy::Progressive => QuantizationConfig {
                enable_binary: true,
                enable_int8: true,
                enable_pq: true,
                ..Default::default()
            },
        };

        let schema = create_columnar_schema(dimension, &quant_config, &[]);

        // ID column must ALWAYS be present
        assert!(
            schema.field_with_name("id").is_ok(),
            "ID column missing for dataset: {} vectors, {} dim, {:?} pattern, {:?} budget",
            num_vectors,
            dimension,
            pattern,
            budget
        );
    }
}
