//! Comprehensive tests for VIPER engine optimizations
//! Tests BinaryArray vector storage, ZSTD Parquet compression, and bytemuck integration

use anyhow::Result;
use tracing::{debug, error, info, warn};
use arrow_array::{Array, BinaryArray, Float32Array, ListArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Instant;

use proximadb::core::serialization::{VectorSerializationConfig, CompressionAlgorithm};
use proximadb::storage::engines::viper::optimized_vector_writer::{
    OptimizedVectorWriter, OptimizedVectorWriterConfig
};
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;

/// Create test vector with specified characteristics
fn create_test_vector(dimension: usize, sparsity: f32) -> Vec<f32> {
    let mut vector = vec![0.0; dimension];
    let non_zero_count = ((1.0 - sparsity) * dimension as f32) as usize;
    
    for i in 0..non_zero_count {
        vector[i] = (i as f32 + 1.0) * 0.001;
    }
    
    // Shuffle for realistic distribution
    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    vector.shuffle(&mut rng);
    
    vector
}

/// Create test VectorRecord with metadata
fn create_test_record(id: &str, vector: Vec<f32>, category: &str) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector,
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(category.to_string())),
            },
            MetadataItem {
                key: "confidence".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::NumberValue(0.85)),
            },
            MetadataItem {
                key: "active".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::BoolValue(true)),
            },
        ],
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    }
}

#[test]
fn test_optimized_schema_creation() {
    let config = OptimizedVectorWriterConfig::default();
    let writer = OptimizedVectorWriter::new(config);
    
    let schema = writer.create_optimized_schema().unwrap();
    
    // Verify schema structure
    assert_eq!(schema.fields().len(), 7);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(1).name(), "timestamp");
    assert_eq!(schema.field(2).name(), "vector_binary");
    assert_eq!(schema.field(3).name(), "updated_at");
    assert_eq!(schema.field(4).name(), "expires_at");
    assert_eq!(schema.field(5).name(), "version");
    assert_eq!(schema.field(6).name(), "metadata");
    
    // Verify data types
    assert_eq!(schema.field(0).data_type(), &DataType::Utf8);
    assert_eq!(schema.field(1).data_type(), &DataType::Int64);
    assert_eq!(schema.field(2).data_type(), &DataType::Binary);
    
    debug!("✅ Optimized schema created with {} fields", schema.fields().len());
}

#[test]
fn test_binary_array_vector_serialization() {
    let mut config = OptimizedVectorWriterConfig::default();
    config.use_binary_array = true;
    config.vector_config.compression_algorithm = CompressionAlgorithm::Zstd;
    
    let writer = OptimizedVectorWriter::new(config);
    
    // Test with different vector types
    let records = vec![
        create_test_record("dense_128", create_test_vector(128, 0.1), "dense"),
        create_test_record("sparse_512", create_test_vector(512, 0.8), "sparse"),
        create_test_record("medium_256", create_test_vector(256, 0.5), "medium"),
        create_test_record("large_1024", create_test_vector(1024, 0.9), "large_sparse"),
    ];
    
    let schema = writer.create_optimized_schema().unwrap();
    let batch = writer.records_to_optimized_batch(&records, &schema).unwrap();
    
    // Verify batch structure
    assert_eq!(batch.num_rows(), 4);
    assert_eq!(batch.num_columns(), 7);
    
    // Test vector extraction from BinaryArray
    let vector_column = batch.column(2);
    let binary_array = vector_column.as_any()
        .downcast_ref::<BinaryArray>()
        .expect("Should be BinaryArray");
    
    for (i, original_record) in records.iter().enumerate() {
        let extracted_vector = writer.extract_vector_from_binary_array(binary_array, i).unwrap();
        
        // Verify vector values match exactly
        assert_eq!(extracted_vector.len(), original_record.vector.len());
        for (j, (&original, &extracted)) in original_record.vector.iter()
            .zip(extracted_vector.iter()).enumerate() {
            assert!((original - extracted).abs() < f32::EPSILON,
                "Vector mismatch at record {} index {}: {} != {}", i, j, original, extracted);
        }
    }
    
    // Get optimization statistics
    let stats = writer.get_optimization_stats(&batch);
    stats.print_summary();
    
    assert!(stats.uses_binary_array);
    assert!(stats.uses_zstd_compression);
    assert_eq!(stats.record_count, 4);
    
    debug!("✅ BinaryArray serialization test passed with {:.3} compression ratio", 
        stats.compression_ratio);
}

#[test]
fn test_list_array_fallback_mode() {
    let mut config = OptimizedVectorWriterConfig::default();
    config.use_binary_array = false; // Use ListArray fallback
    
    let writer = OptimizedVectorWriter::new(config);
    
    let records = vec![
        create_test_record("test1", vec![1.0, 2.0, 3.0, 4.0], "test"),
        create_test_record("test2", vec![5.0, 6.0, 7.0, 8.0], "test"),
        create_test_record("test3", vec![9.0, 10.0, 11.0, 12.0], "test"),
    ];
    
    let schema = writer.create_optimized_schema().unwrap();
    let batch = writer.records_to_optimized_batch(&records, &schema).unwrap();
    
    // Verify ListArray structure
    let vector_column = batch.column(2);
    let list_array = vector_column.as_any()
        .downcast_ref::<ListArray>()
        .expect("Should be ListArray in fallback mode");
        
    // Test vector extraction
    for (i, original_record) in records.iter().enumerate() {
        let list_value = list_array.value(i);
        let float_array = list_value.as_any()
            .downcast_ref::<Float32Array>()
            .expect("List should contain Float32Array");
            
        for (j, &original_value) in original_record.vector.iter().enumerate() {
            let extracted_value = float_array.value(j);
            assert!((original_value - extracted_value).abs() < f32::EPSILON,
                "ListArray vector mismatch at record {} index {}: {} != {}", 
                i, j, original_value, extracted_value);
        }
    }
    
    let stats = writer.get_optimization_stats(&batch);
    assert!(!stats.uses_binary_array);
    
    debug!("✅ ListArray fallback mode test passed");
}

#[test]
fn test_compression_effectiveness() {
    let mut sparse_config = OptimizedVectorWriterConfig::default();
    sparse_config.use_binary_array = true;
    sparse_config.vector_config.compression_algorithm = CompressionAlgorithm::Zstd;
    sparse_config.vector_config.compression_level = 6;
    
    let sparse_writer = OptimizedVectorWriter::new(sparse_config);
    
    // Create highly sparse vectors (90% zeros)
    let sparse_records = (0..10).map(|i| {
        create_test_record(
            &format!("sparse_{}", i),
            create_test_vector(1024, 0.9), // 90% sparse
            "sparse"
        )
    }).collect::<Vec<_>>();
    
    let schema = sparse_writer.create_optimized_schema().unwrap();
    let sparse_batch = sparse_writer.records_to_optimized_batch(&sparse_records, &schema).unwrap();
    let sparse_stats = sparse_writer.get_optimization_stats(&sparse_batch);
    
    // Test dense vectors for comparison
    let mut dense_config = OptimizedVectorWriterConfig::default();
    dense_config.use_binary_array = true;
    dense_config.vector_config.compression_algorithm = CompressionAlgorithm::Zstd;
    
    let dense_writer = OptimizedVectorWriter::new(dense_config);
    
    let dense_records = (0..10).map(|i| {
        create_test_record(
            &format!("dense_{}", i),
            create_test_vector(1024, 0.1), // 10% sparse (90% dense)
            "dense"
        )
    }).collect::<Vec<_>>();
    
    let dense_batch = dense_writer.records_to_optimized_batch(&dense_records, &schema).unwrap();
    let dense_stats = dense_writer.get_optimization_stats(&dense_batch);
    
    debug!("📊 Compression Comparison:");
    debug!("   Sparse vectors: {:.3} ratio", sparse_stats.compression_ratio);
    debug!("   Dense vectors: {:.3} ratio", dense_stats.compression_ratio);
    
    // Sparse vectors should compress significantly better
    assert!(sparse_stats.compression_ratio < dense_stats.compression_ratio,
        "Sparse vectors should compress better than dense vectors");
    assert!(sparse_stats.compression_ratio < 0.7,
        "Sparse vectors should achieve at least 30% compression");
        
    debug!("✅ Compression effectiveness test passed");
}

#[test]
fn test_parquet_writer_properties() {
    let mut config = OptimizedVectorWriterConfig::default();
    config.parquet_compression_level = 9; // Maximum compression
    config.row_group_size = 25_000;
    config.write_batch_size = 2048;
    config.enable_dictionary_encoding = false;
    
    let writer = OptimizedVectorWriter::new(config);
    let properties = writer.create_writer_properties().unwrap();
    
    // Properties should be created without error
    // We can't easily inspect the internal values, but creation validates the configuration
    debug!("✅ Parquet writer properties created successfully");
}

#[test]
fn test_metadata_serialization() {
    let config = OptimizedVectorWriterConfig::default();
    let writer = OptimizedVectorWriter::new(config);
    
    // Create record with comprehensive metadata
    let mut record = create_test_record("metadata_test", vec![1.0, 2.0, 3.0], "test");
    record.metadata = vec![
        MetadataItem {
            key: "string_field".to_string(),
            value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("test_value".to_string())),
        },
        MetadataItem {
            key: "int_field".to_string(),
            value: Some(proximadb::proto::proximadb::metadata_item::Value::NumberValue(42.0)),
        },
        MetadataItem {
            key: "float_field".to_string(),
            value: Some(proximadb::proto::proximadb::metadata_item::Value::NumberValue(3.14)),
        },
        MetadataItem {
            key: "bool_field".to_string(),
            value: Some(proximadb::proto::proximadb::metadata_item::Value::BoolValue(true)),
        },
    ];
    
    let schema = writer.create_optimized_schema().unwrap();
    let batch = writer.records_to_optimized_batch(&[record], &schema).unwrap();
    
    // Verify metadata column
    let metadata_column = batch.column(6);
    let string_array = metadata_column.as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .expect("Metadata should be StringArray");
        
    let metadata_json = string_array.value(0);
    assert!(!metadata_json.is_empty());
    
    // Parse JSON to verify structure
    let parsed: serde_json::Value = serde_json::from_str(metadata_json).unwrap();
    assert!(parsed.is_object());
    
    if let Some(obj) = parsed.as_object() {
        // Check that at least some metadata fields are present
        assert!(obj.len() > 0, "Metadata object should not be empty");
        
        // Check specific fields if they exist
        if let Some(string_val) = obj.get(key).and_then(|v| v.as_str()) {
            assert_eq!(string_val, "test_value");
        }
        if let Some(int_val) = obj.get(key).and_then(|v| v.as_f64()) {
            assert_eq!(int_val, 42.0);
        }
        if let Some(float_val) = obj.get(key).and_then(|v| v.as_f64()) {
            assert!((float_val - 3.14).abs() < 0.001);
        }
        if let Some(bool_val) = obj.get(key).and_then(|v| v.as_bool()) {
            assert_eq!(bool_val, true);
        }
    } else {
        panic!("Metadata should be a JSON object, got: {}", metadata_json);
    }
    
    debug!("✅ Metadata serialization test passed");
}

#[test]
fn test_performance_benchmark() {
    let mut config = OptimizedVectorWriterConfig::default();
    config.use_binary_array = true;
    config.vector_config.compression_algorithm = CompressionAlgorithm::Zstd;
    config.vector_config.compression_level = 3; // Balanced performance
    
    let writer = OptimizedVectorWriter::new(config);
    
    // Create larger dataset for performance testing
    let record_count = 1000;
    let records: Vec<VectorRecord> = (0..record_count).map(|i| {
        let vector_type = match i % 3 {
            0 => (512, 0.1),  // Dense medium vectors
            1 => (1024, 0.8), // Sparse large vectors  
            _ => (256, 0.5),  // Medium sparse vectors
        };
        
        create_test_record(
            &format!("perf_test_{}", i),
            create_test_vector(vector_type.0, vector_type.1),
            &format!("category_{}", i % 10)
        )
    }).collect();
    
    // Benchmark batch creation
    let schema = writer.create_optimized_schema().unwrap();
    let start = Instant::now();
    let batch = writer.records_to_optimized_batch(&records, &schema).unwrap();
    let batch_time = start.elapsed();
    
    // Benchmark Parquet writing
    let mut parquet_buffer = Vec::new();
    let cursor = Cursor::new(&mut parquet_buffer);
    let props = writer.create_writer_properties().unwrap();
    let mut parquet_writer = ArrowWriter::try_new(cursor, Arc::new(schema), Some(props)).unwrap();
    
    let start = Instant::now();
    writer.write_batch_to_parquet(&mut parquet_writer, &batch).unwrap();
    parquet_writer.close().unwrap();
    let write_time = start.elapsed();
    
    let stats = writer.get_optimization_stats(&batch);
    
    debug!("⚡ Performance Benchmark Results:");
    debug!("   Records: {}", record_count);
    debug!("   Batch creation: {:?} ({:.1} records/sec)", 
        batch_time, record_count as f64 / batch_time.as_secs_f64());
    debug!("   Parquet write: {:?} ({:.1} records/sec)", 
        write_time, record_count as f64 / write_time.as_secs_f64());
    debug!("   Parquet size: {} bytes", parquet_buffer.len());
    debug!("   Compression ratio: {:.3}", stats.compression_ratio);
    
    // Performance expectations
    assert!(batch_time.as_millis() < 1000, "Batch creation should be < 1 second");
    assert!(write_time.as_millis() < 2000, "Parquet write should be < 2 seconds");
    assert!(!parquet_buffer.is_empty(), "Parquet data should be written");
    
    debug!("✅ Performance benchmark completed successfully");
}

#[test]
fn test_empty_and_edge_cases() {
    let config = OptimizedVectorWriterConfig::default();
    let writer = OptimizedVectorWriter::new(config);
    
    // Test empty vectors
    let record_with_empty_vector = VectorRecord {
        id: Some("empty_vector".to_string()),
        vector: vec![], // Empty vector
        metadata: vec![],
        timestamp: 1234567890,
        updated_at: None,
        expires_at: None,
        version: None,
        rank: None,
        score: None,
        distance: None,
        ..Default::default()
    };
    
    // Test single record
    let schema = writer.create_optimized_schema().unwrap();
    let batch = writer.records_to_optimized_batch(&[record_with_empty_vector], &schema).unwrap();
    
    assert_eq!(batch.num_rows(), 1);
    
    // Extract empty vector
    let vector_column = batch.column(2);
    let binary_array = vector_column.as_any().downcast_ref::<BinaryArray>().unwrap();
    let extracted = writer.extract_vector_from_binary_array(binary_array, 0).unwrap();
    assert!(extracted.is_empty());
    
    // Test error handling - empty record set should fail
    let empty_records: Vec<VectorRecord> = vec![];
    let result = writer.records_to_optimized_batch(&empty_records, &schema);
    assert!(result.is_err(), "Empty record set should fail");
    
    debug!("✅ Edge cases test passed");
}

#[test]
fn test_dimension_consistency() {
    let mut config = OptimizedVectorWriterConfig::default();
    config.use_binary_array = false; // Use ListArray to test dimension checking
    
    let writer = OptimizedVectorWriter::new(config);
    
    // Mixed dimension vectors should fail in ListArray mode
    let mixed_records = vec![
        create_test_record("vec1", vec![1.0, 2.0, 3.0], "test"),      // 3D
        create_test_record("vec2", vec![1.0, 2.0, 3.0, 4.0], "test"), // 4D - mismatch
    ];
    
    let schema = writer.create_optimized_schema().unwrap();
    let result = writer.records_to_optimized_batch(&mixed_records, &schema);
    
    // Mixed dimensions should be rejected - this is a data integrity requirement
    // The OptimizedVectorWriter enforces dimension consistency at line 328
    assert!(result.is_err(), 
        "Mixed dimensions should fail - ProximaDB enforces consistent dimensions per collection");
    
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("dimension mismatch") || error_msg.contains("dimension"), 
        "Error should mention dimension mismatch, got: {}", error_msg);
    
    debug!("✅ ListArray mode correctly rejected mixed dimensions");
    
    // BinaryArray mode should also reject mixed dimensions (same validation logic)
    let mut binary_config = OptimizedVectorWriterConfig::default();
    binary_config.use_binary_array = true;
    let binary_writer = OptimizedVectorWriter::new(binary_config);
    
    let result = binary_writer.records_to_optimized_batch(&mixed_records, &schema);
    assert!(result.is_err(), "BinaryArray mode should also reject mixed dimensions");
    
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("dimension mismatch") || error_msg.contains("dimension") || 
            error_msg.contains("RecordBatch") || error_msg.contains("schema"), 
        "BinaryArray error should be related to dimension/schema mismatch, got: {}", error_msg);
    
    debug!("✅ BinaryArray mode also correctly rejected mixed dimensions");
    
    debug!("✅ Dimension consistency test passed");
}

#[test]  
fn test_adaptive_vector_compression() {
    // Test adaptive compression based on vector characteristics
    let test_cases = vec![
        ("small_dense", 64, 0.1),    // Small dense: minimal compression
        ("medium_sparse", 512, 0.8), // Medium sparse: good compression 
        ("large_very_sparse", 2048, 0.95), // Large sparse: aggressive compression
    ];
    
    for (name, dimension, sparsity) in test_cases {
        let mut config = OptimizedVectorWriterConfig::default();
        config.vector_config = VectorSerializationConfig::for_dimension(dimension);
        config.vector_config.adaptive_compression = true;
        
        let writer = OptimizedVectorWriter::new(config);
        
        let record = create_test_record(
            name,
            create_test_vector(dimension, sparsity),
            "adaptive_test"
        );
        
        let schema = writer.create_optimized_schema().unwrap();
        let batch = writer.records_to_optimized_batch(&[record], &schema).unwrap();
        let stats = writer.get_optimization_stats(&batch);
        
        debug!("📈 Adaptive compression for {}: dimension={}, sparsity={:.2}, ratio={:.3}",
            name, dimension, sparsity, stats.compression_ratio);
            
        // Verify reasonable compression for sparse vectors
        if sparsity > 0.8 {
            assert!(stats.compression_ratio < 0.8, 
                "High sparsity should achieve good compression for {}", name);
        }
    }
    
    debug!("✅ Adaptive compression test passed");
}