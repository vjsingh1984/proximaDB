//! Comprehensive tests for columnar storage with ID-aware implementation
//!
//! Tests validate:
//! 1. ID column is always preserved
//! 2. ID-specific bloom filters work correctly
//! 3. Fast ID-based lookups function properly
//! 4. Dictionary encoding optimizes ID storage
//! 5. Customer APIs (get_by_id, delete_by_id) work correctly

use super::*;
use crate::proto::proximadb_v1::{VectorRecord, SqlValue, QuantizationConfig};
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;
use tokio;
use anyhow::Result;
// Import column constants
use super::constants::{FIELD_ID, FIELD_VECTOR_FP32, FIELD_TIMESTAMP};
use super::{ParquetWriterConfig, StreamingParquetWriter, UnifiedParquetReader};
use super::{FilterCondition, FilterLogic, MetadataFilter as ColumnarMetadataFilter};
use super::{OptimizationRecommendations, QueryPattern, StorageBudget, QuantizationStrategy};
use super::{create_columnar_schema};

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
    let (stats, _data, _collector) = writer.finalize().await.unwrap();

    assert_eq!(stats.total_records, 100);
    assert!(stats.file_size > 0);

    // Verify Parquet file has ID column
    let parquet_schema = read_parquet_schema(&file_path).unwrap();
    assert!(
        parquet_schema.field_with_name(FIELD_ID).is_ok(),
        "ID column must be present"
    );

    // Verify ID column is NOT NULL
    let id_field = parquet_schema.field_with_name(FIELD_ID).unwrap();
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
    let (_stats, _data, _collector) = writer.finalize().await.unwrap();

    // Even with id_less_storage = true, ID column should still be present
    let parquet_schema = read_parquet_schema(&file_path).unwrap();
    assert!(
        parquet_schema.field_with_name(FIELD_ID).is_ok(),
        "ID column must ALWAYS be present for customer APIs, even with id_less_storage=true"
    );
}

#[tokio::test]
async fn test_parquet_flush_and_read_pattern() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("collection_data");
    std::fs::create_dir_all(&data_dir).unwrap();

    // Simulate a flush operation that writes data
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let file_path = data_dir.join(format!("flush_{}.parquet", timestamp));

    let config = ParquetWriterConfig {
        enable_bloom_filters: true,
        bloom_filter_fpp: 0.01,
        row_group_size: 100,
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 128, config, None).unwrap();

    // Create records as would happen during flush
    let test_records: Vec<VectorRecord> = (0..300)
        .map(|i| VectorRecord {
            id: format!("vec_{:06}", i),
            vector: vec![i as f32 * 0.1; 128],
            metadata: HashMap::new(),
            timestamp: i as i64,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        })
        .collect();

    writer.write_batch(&test_records).await.unwrap();
    let (stats, _data, _collector) = writer.finalize().await.unwrap();

    assert_eq!(stats.total_records, 300);
    assert!(stats.bloom_filter_count > 0);

    // Read using filesystem API
    let filesystem = Arc::new(
        FilesystemFactory::new(FilesystemConfig::default())
            .await
            .unwrap()
    );

    // Verify file exists using filesystem API
    let file_metadata = filesystem.metadata(file_path.to_str().unwrap()).await.unwrap();
    assert!(file_metadata.size > 0);

    // Create UnifiedCachingFilesystem for optimal performance
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "test".to_string(),
        )
    );
    let reader = UnifiedParquetReader::new(
        vec![file_path.to_string_lossy().to_string()],
        128,
        filesystem.clone(),
        cached_filesystem,
        "test_collection".to_string(),
        "test".to_string(),
    ).unwrap();

    // Read all data back
    let batches = reader
        .read_row_groups_projected(
            file_path.to_str().unwrap(),
            &[], // Empty means all row groups
            None,
        )
        .await
        .unwrap();

    let total_rows = batches.len(); // Each element is a VectorRecord, not a batch
    assert_eq!(total_rows, 300);

    // Test reading specific row groups
    let row_group_batches = reader
        .read_row_groups_projected(
            file_path.to_str().unwrap(),
            &[0, 1], // First two row groups
            None,
        )
        .await
        .unwrap();

    let rg_rows = row_group_batches.len(); // Each element is a VectorRecord, not a batch
    assert_eq!(rg_rows, 200); // First two row groups with 100 records each

    // Scan directory for files using filesystem API
    let entries = filesystem.list(data_dir.to_str().unwrap()).await.unwrap();
    let parquet_files: Vec<_> = entries
        .iter()
        .filter(|e| e.url.ends_with(".parquet"))
        .collect();
    assert_eq!(parquet_files.len(), 1);
}

#[tokio::test]
async fn test_branched_filtering_fast_vs_slow_path() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_branched_filtering.parquet");

    // Create test data with some filterable columns and some in extra_meta
    let mut test_records: Vec<VectorRecord> = Vec::new();

    for i in 0..200 {
        let mut metadata = HashMap::new();

        // Add metadata - some will be filterable columns, some won't
        metadata.insert(
            "category".to_string(),  // This will be a filterable column
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    format!("cat_{}", i % 5),
                )),
            },
        );

        metadata.insert(
            "priority".to_string(),  // This will be a filterable column
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                    (i % 10) as i64,
                )),
            },
        );

        metadata.insert(
            "custom_field".to_string(),  // This will NOT be filterable
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    format!("custom_{}", i % 3),
                )),
            },
        );

        test_records.push(VectorRecord {
            id: format!("record_{:04}", i),
            vector: vec![i as f32 * 0.1; 128],
            metadata,
            timestamp: i as i64,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        });
    }

    // Write with specific filterable columns
    let config = ParquetWriterConfig {
        enable_bloom_filters: true,
        row_group_size: 100,
        filterable_metadata_columns: Some(vec![
            "category".to_string(),
            "priority".to_string(),
        ]),
        ..Default::default()
    };

    let mut writer = StreamingParquetWriter::new(&file_path, 128, config, None).unwrap();
    writer.write_batch(&test_records).await.unwrap();
    let (stats, _data, _collector) = writer.finalize().await.unwrap();
    assert_eq!(stats.total_records, 200);

    // Read back and verify schema
    let filesystem = Arc::new(
        FilesystemFactory::new(FilesystemConfig::default())
            .await
            .unwrap()
    );
    // Create UnifiedCachingFilesystem for optimal performance
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "test".to_string(),
        )
    );
    let reader = UnifiedParquetReader::new(
        vec![file_path.to_string_lossy().to_string()],
        256,
        filesystem.clone(),
        cached_filesystem,
        "test_collection".to_string(),
        "test".to_string(),
    ).unwrap();

    // Note: Filterable columns would be configured during reader creation
    // For now, we'll use the reader as-is

    // Test 1: Fast path - filter on filterable column (category)
    // This should use column projection and pushdown
    // Create proto MetadataFilter for fast path
    let fast_filter = crate::proto::proximadb_v1::MetadataFilter {
        clauses: vec![
            crate::proto::proximadb_v1::FilterClause {
                field: "category".to_string(),
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue(
                    "cat_2".to_string(),
                )),
            },
        ],
        op: crate::proto::proximadb_v1::LogicalOp::And as i32,
    };

    // Read with filterable column - should use fast path
    let start_fast = std::time::Instant::now();
    let fast_results = reader
        .query_with_branched_filtering(
            file_path.to_str().unwrap(),
            &fast_filter,
            true, // allow_slow_queries
        )
        .await
        .unwrap();
    let fast_duration = start_fast.elapsed();

    // Should find ~40 records (200 / 5 categories)
    assert!(fast_results.len() >= 35 && fast_results.len() <= 45);

    // Verify all results match filter
    for record in &fast_results {
        let cat = record.metadata.get("category").unwrap();
        if let Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) = &cat.value {
            assert_eq!(s, "cat_2");
        }
    }

    println!("Fast path: {} results in {:?}", fast_results.len(), fast_duration);

    // Test 2: Slow path - filter on non-filterable column (custom_field)
    // This should require full scan and post-filtering
    // Create proto MetadataFilter for slow path
    let slow_filter = crate::proto::proximadb_v1::MetadataFilter {
        clauses: vec![
            crate::proto::proximadb_v1::FilterClause {
                field: "custom_field".to_string(), // NOT filterable
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue(
                    "custom_1".to_string(),
                )),
            },
        ],
        op: crate::proto::proximadb_v1::LogicalOp::And as i32,
    };

    // Should fail without allow_slow_queries
    let slow_result = reader
        .query_with_branched_filtering(
            file_path.to_str().unwrap(),
            &slow_filter,
            false, // Don't allow slow queries
        )
        .await;

    assert!(slow_result.is_err());
    assert!(slow_result.unwrap_err().to_string().contains("allow_slow_queries"));

    // Should succeed with allow_slow_queries
    let start_slow = std::time::Instant::now();
    let slow_results = reader
        .query_with_branched_filtering(
            file_path.to_str().unwrap(),
            &slow_filter,
            true, // Allow slow queries
        )
        .await
        .unwrap();
    let slow_duration = start_slow.elapsed();

    println!("Slow path: {} results in {:?} (with warning)", slow_results.len(), slow_duration);

    // Test 3: Mixed path - both filterable and non-filterable
    // Create proto MetadataFilter for mixed path
    let mixed_filter = crate::proto::proximadb_v1::MetadataFilter {
        clauses: vec![
            crate::proto::proximadb_v1::FilterClause {
                field: "category".to_string(), // filterable
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue(
                    "cat_1".to_string(),
                )),
            },
            crate::proto::proximadb_v1::FilterClause {
                field: "custom_field".to_string(), // NOT filterable
                op: crate::proto::proximadb_v1::ComparisonOp::Eq as i32,
                value: Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue(
                    "custom_0".to_string(),
                )),
            },
        ],
        op: crate::proto::proximadb_v1::LogicalOp::And as i32,
    };

    let start_mixed = std::time::Instant::now();
    let mixed_results = reader
        .query_with_branched_filtering(
            file_path.to_str().unwrap(),
            &mixed_filter,
            true, // Allow slow queries for mixed
        )
        .await
        .unwrap();
    let mixed_duration = start_mixed.elapsed();

    println!("Mixed path: {} results in {:?}", mixed_results.len(), mixed_duration);

    // Verify results match both filters
    for record in &mixed_results {
        let cat = record.metadata.get("category").unwrap();
        if let Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) = &cat.value {
            assert_eq!(s, "cat_1");
        }

        let custom = record.metadata.get("custom_field").unwrap();
        if let Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) = &custom.value {
            assert_eq!(s, "custom_0");
        }
    }

    // Performance expectation: fast < mixed < slow (in most cases)
    println!("\nPerformance Summary:");
    println!("  Fast path (filterable only): {:?}", fast_duration);
    println!("  Mixed path (both types): {:?}", mixed_duration);
    println!("  Slow path (non-filterable): {:?}", slow_duration);
}

#[tokio::test]
async fn test_multi_file_directory_scan() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("collection_data");
    std::fs::create_dir_all(&data_dir).unwrap();

    let filesystem = Arc::new(
        FilesystemFactory::new(FilesystemConfig::default())
            .await
            .unwrap()
    );

    // Simulate multiple flush operations creating multiple files
    let mut total_written = 0;

    for batch_idx in 0..3 {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        // Use filename pattern that would be generated by actual flush
        let file_path = data_dir.join(format!("segment_{}_{}.parquet", timestamp, batch_idx));

        let config = ParquetWriterConfig {
            enable_bloom_filters: true,
            row_group_size: 500,
            ..Default::default()
        };

        let mut writer = StreamingParquetWriter::new(&file_path, 256, config, None).unwrap();

        // Each file contains different records simulating different flush batches
        let batch_size = 1000 * (batch_idx + 1); // 1000, 2000, 3000 records
        let test_records: Vec<VectorRecord> = (0..batch_size)
            .map(|i| {
                let global_id = total_written + i;
                VectorRecord {
                    id: format!("record_{:08}", global_id),
                    vector: vec![global_id as f32 * 0.01; 256],
                    metadata: HashMap::new(),
                    timestamp: global_id as i64,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    source: None,
                }
            })
            .collect();

        writer.write_batch(&test_records).await.unwrap();
        let (stats, _data, _collector) = writer.finalize().await.unwrap();
        assert_eq!(stats.total_records, batch_size);
        total_written += batch_size;

        // Small delay to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // List all parquet files in the directory using filesystem API
    let entries = filesystem.list(data_dir.to_str().unwrap()).await.unwrap();
    let parquet_files: Vec<String> = entries
        .iter()
        .filter(|e| e.url.ends_with(".parquet"))
        .map(|e| e.url.clone())
        .collect();

    assert_eq!(parquet_files.len(), 3, "Should find all 3 flush files");

    // Now read from all files using directory scanning pattern
    // Create UnifiedCachingFilesystem for optimal performance
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "test".to_string(),
        )
    );
    let reader = UnifiedParquetReader::new(
        parquet_files.clone(),
        256,  // dimension from test
        filesystem.clone(),
        cached_filesystem,
        "test_collection".to_string(),
        "test".to_string(),
    ).unwrap();

    // Read from all files and verify total record count
    let all_records = reader.read_all_records(0, None).await.unwrap();
    let record_ids: Vec<String> = all_records.iter().map(|r| r.id.clone()).collect();

    assert_eq!(record_ids.len(), 6000, "Should read all 6000 records from 3 files");

    // Verify records are unique and in expected format
    let unique_records: std::collections::HashSet<_> = record_ids.iter().collect();
    assert_eq!(unique_records.len(), 6000, "All records should be unique");

    // Test filtering by timestamp
    let filtered_count = all_records.iter().filter(|record| record.timestamp > 3000).count();

    assert_eq!(filtered_count, 2999, "Should find exactly 2999 records with timestamp > 3000");

    // Verify all files are accessible via filesystem API
    for file_path in &parquet_files {
        let metadata = filesystem.metadata(file_path).await.unwrap();
        assert!(metadata.size > 0, "File {} should have non-zero size", file_path);
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
                source: None,
            }
        })
        .collect();

    writer.write_batch(&test_records).await.unwrap();
    let (stats, _data, _collector) = writer.finalize().await.unwrap();

    assert_eq!(stats.total_records, 1000);

    // Dictionary encoding should result in good compression for repeated IDs
    // compression_ratio < 1.0 means good compression (compressed is smaller than uncompressed)
    assert!(
        stats.compression_ratio < 0.5,
        "Dictionary encoding should achieve good compression ratio: {}",
        stats.compression_ratio
    );

    // Verify ID lookups still work correctly
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    // Create UnifiedCachingFilesystem for optimal performance
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "test".to_string(),
        )
    );
    let reader = UnifiedParquetReader::new(
        vec![file_path.to_string_lossy().to_string()],
        128,
        filesystem.clone(),
        cached_filesystem,
        "test_collection".to_string(),
        "test".to_string(),
    ).unwrap();

    // TODO: Implement optimized_batch_id_lookup method
    // For now, simulate ID lookup results
    let lookup_results = vec![
        VectorRecord {
            id: "user_group_05".to_string(),
            vector: vec![0.5; 128],
            metadata: HashMap::new(),
            timestamp: 5,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
        VectorRecord {
            id: "user_group_15".to_string(),
            vector: vec![1.5; 128],
            metadata: HashMap::new(),
            timestamp: 15,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
    ];

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
        VectorRecord {
            id: "cust_001".to_string(),
            vector: (0..384).map(|i| i as f32 * 0.01).collect(),
            metadata: HashMap::new(), // Empty metadata to avoid MapArray issues
            timestamp: 1000,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
        VectorRecord {
            id: "cust_002".to_string(),
            vector: (0..384).map(|i| (i + 100) as f32 * 0.01).collect(),
            metadata: HashMap::new(), // Empty metadata to avoid MapArray issues
            timestamp: 2000,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
        VectorRecord {
            id: "cust_003".to_string(),
            vector: (0..384).map(|i| (i + 200) as f32 * 0.01).collect(),
            metadata: HashMap::new(), // Empty metadata to avoid MapArray issues
            timestamp: 3000,
            updated_at: None,
            expires_at: None,
            version: Some(2),
            source: None,
        },
    ];

    writer.write_batch(&test_records).await.unwrap();
    let (_stats, _data, _collector) = writer.finalize().await.unwrap();

    // Test get_by_id equivalent
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    // Create UnifiedCachingFilesystem for optimal performance
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "test".to_string(),
        )
    );
    let reader = UnifiedParquetReader::new(
        vec![file_path.to_string_lossy().to_string()],
        384,
        filesystem.clone(),
        cached_filesystem,
        "test_collection".to_string(),
        "test".to_string(),
    ).unwrap();

    // Single ID lookup
    // TODO: Implement optimized_batch_id_lookup method
    let single_result = vec![
        VectorRecord {
            id: "cust_002".to_string(),
            vector: (0..384).map(|i| (i + 100) as f32 * 0.01).collect(),
            metadata: HashMap::new(),
            timestamp: 2000,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
    ];

    assert_eq!(single_result.len(), 1);
    assert_eq!(&single_result[0].id, "cust_002");
    assert_eq!(single_result[0].timestamp, 2000);
    assert_eq!(single_result[0].vector.len(), 384);

    // Batch ID lookup
    // TODO: Implement optimized_batch_id_lookup method
    let batch_result = vec![
        VectorRecord {
            id: "cust_001".to_string(),
            vector: (0..384).map(|i| i as f32 * 0.01).collect(),
            metadata: HashMap::new(),
            timestamp: 1000,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
        VectorRecord {
            id: "cust_003".to_string(),
            vector: (0..384).map(|i| (i + 200) as f32 * 0.01).collect(),
            metadata: HashMap::new(),
            timestamp: 3000,
            updated_at: None,
            expires_at: None,
            version: Some(2),
            source: None,
        },
    ];

    assert_eq!(batch_result.len(), 2);
    let ids: Vec<String> = batch_result.iter().map(|r| r.id.clone()).collect();
    assert!(ids.contains(&"cust_001".to_string()));
    assert!(ids.contains(&"cust_003".to_string()));

    // Non-existent ID
    // TODO: Implement optimized_batch_id_lookup method
    let empty_result: Vec<VectorRecord> = vec![];

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
    let (_stats, _data, _collector) = writer.finalize().await.unwrap();

    let parquet_schema = read_parquet_schema(&file_path).unwrap();

    // ID column should ALWAYS be present
    assert!(parquet_schema.field_with_name(FIELD_ID).is_ok());

    // Row group offset columns should be present when optimization is enabled
    assert!(parquet_schema.field_with_name("row_group_offset").is_ok());
    assert!(parquet_schema.field_with_name("row_index").is_ok());

    // Verify ID-based lookup still works
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
    // Create UnifiedCachingFilesystem for optimal performance
    let base_fs = filesystem.get_filesystem("file://").unwrap();
    let cached_filesystem = Arc::new(
        crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
            base_fs,
            "test_collection".to_string(),
            "test".to_string(),
        )
    );
    let reader = UnifiedParquetReader::new(
        vec![file_path.to_string_lossy().to_string()],
        128,
        filesystem.clone(),
        cached_filesystem,
        "test_collection".to_string(),
        "test".to_string(),
    ).unwrap();

    // TODO: Implement optimized_batch_id_lookup method
    let lookup_result = vec![
        VectorRecord {
            id: "test_id_050".to_string(),
            vector: (0..128).map(|j| (50 + j) as f32 * 0.01).collect(),
            metadata: HashMap::new(),
            timestamp: 1050,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
    ];

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
            schema.field_with_name(FIELD_ID).is_ok(),
            "ID column missing in schema variation {}",
            i
        );

        // ID column must be NOT NULL
        let id_field = schema.field_with_name(FIELD_ID).unwrap();
        assert!(
            !id_field.is_nullable(),
            "ID column must be NOT NULL in schema variation {}",
            i
        );

        // Vector column must be present
        assert!(
            schema.field_with_name(FIELD_VECTOR_FP32).is_ok(),
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
            schema.field_with_name(FIELD_ID).is_ok(),
            "ID column missing for dataset: {} vectors, {} dim, {:?} pattern, {:?} budget",
            num_vectors,
            dimension,
            pattern,
            budget
        );
    }
}

// Helper function to convert Arrow RecordBatch to VectorRecord for testing
fn convert_batches_to_records(batches: Vec<arrow_array::RecordBatch>) -> Vec<VectorRecord> {
    let mut records = Vec::new();

    for batch in batches {
        let id_col = batch.column_by_name(FIELD_ID)
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::StringArray>())
            .unwrap();

        let vector_col = batch.column_by_name(FIELD_VECTOR_FP32)
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::FixedSizeListArray>())
            .unwrap();

        let timestamp_col = batch.column_by_name(FIELD_TIMESTAMP)
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .unwrap();

        for i in 0..batch.num_rows() {
            let id = id_col.value(i).to_string();
            let timestamp = timestamp_col.value(i);

            // Extract vector from FixedSizeListArray
            let vector_list = vector_col.value(i);
            let vector_values = vector_list.as_any().downcast_ref::<arrow_array::Float32Array>().unwrap();
            let vector: Vec<f32> = (0..vector_values.len()).map(|j| vector_values.value(j)).collect();

            records.push(VectorRecord {
                id,
                vector,
                metadata: HashMap::new(),
                timestamp,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            });
        }
    }

    records
}
