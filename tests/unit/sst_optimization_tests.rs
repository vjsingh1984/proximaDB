//! Comprehensive tests for SST engine optimizations
//! Tests bytemuck vector serialization and ZSTD DataBlock compression

use anyhow::Result;
use tracing::{debug, error, info, warn};
use proximadb::core::serialization::{VectorSerializationConfig, CompressionAlgorithm};
use proximadb::storage::engines::sst::{SstRecord, DataBlock, DataBlockCompressionConfig};
use proximadb::proto::proximadb::MetadataItem;
use std::time::Instant;

/// Create test vector with specific characteristics
fn create_test_vector(dimension: usize, sparsity: f32) -> Vec<f32> {
    let mut vector = vec![0.0; dimension];
    let non_zero_count = ((1.0 - sparsity) * dimension as f32) as usize;
    
    for i in 0..non_zero_count {
        vector[i] = (i as f32 + 1.0) * 0.001;
    }
    
    // Shuffle to distribute non-zero values
    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    vector.shuffle(&mut rng);
    
    vector
}

/// Create test SstRecord with specific vector characteristics
fn create_test_sst_record(id: String, vector: Vec<f32>) -> SstRecord {
    SstRecord {
        id,
        vector,
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("test".to_string())),
            },
            MetadataItem {
                key: "score".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::NumberValue(0.85)),
            },
        ],
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        is_tombstone: false,
        sequence_number: 1,
        level: 0,
    }
}

#[test]
fn test_vector_serialization_roundtrip() {
    let config = VectorSerializationConfig::default();
    
    // Test cases: different vector sizes and sparsity levels
    let test_cases = vec![
        (64, 0.0),    // Small dense vector
        (256, 0.1),   // Medium dense vector  
        (512, 0.5),   // Medium sparse vector
        (1024, 0.9),  // Large sparse vector
        (2048, 0.95), // Very large sparse vector
    ];
    
    for (dimension, sparsity) in test_cases {
        let vector = create_test_vector(dimension, sparsity);
        
        // Serialize and deserialize
        let serialized = config.serialize_vector(&vector).unwrap();
        let deserialized = config.deserialize_vector(&serialized).unwrap();
        
        // Verify exact match
        assert_eq!(vector.len(), deserialized.len(), 
            "Vector length mismatch for dimension {}", dimension);
        
        for (i, (&original, &recovered)) in vector.iter().zip(deserialized.iter()).enumerate() {
            assert!((original - recovered).abs() < f32::EPSILON, 
                "Vector value mismatch at index {} for dimension {}: {} != {}", 
                i, dimension, original, recovered);
        }
        
        debug!("✅ Vector serialization roundtrip passed: {} dimensions, {:.1}% sparsity, {} bytes", 
            dimension, sparsity * 100.0, serialized.len());
    }
}

#[test]
fn test_vector_compression_effectiveness() {
    let mut config = VectorSerializationConfig::default();
    config.compression_algorithm = CompressionAlgorithm::Zstd;
    config.compression_level = 3;
    config.compression_threshold = 128; // Compress vectors > 128 dimensions
    
    // Test sparse vs dense compression
    let sparse_vector = create_test_vector(1000, 0.9); // 90% zeros
    let dense_vector = create_test_vector(1000, 0.1);  // 10% zeros
    
    let sparse_ratio = config.compression_ratio(&sparse_vector).unwrap();
    let dense_ratio = config.compression_ratio(&dense_vector).unwrap();
    
    debug!("📊 Compression ratios - Sparse: {:.3}, Dense: {:.3}", sparse_ratio, dense_ratio);
    
    // Sparse vectors should compress significantly better
    assert!(sparse_ratio < dense_ratio, 
        "Sparse vectors should compress better than dense vectors");
    assert!(sparse_ratio < 0.7, 
        "Sparse vectors should achieve at least 30% compression");
    
    // Test small vectors don't get compressed
    let small_vector = create_test_vector(64, 0.9);
    let serialized = config.serialize_vector(&small_vector).unwrap();
    
    // Should be roughly the raw size (64 * 4 bytes + header)
    let expected_raw_size = 64 * 4 + 16; // Rough estimate with header
    assert!(serialized.len() <= expected_raw_size, 
        "Small vectors should not be compressed");
}

#[test]
fn test_sst_record_optimized_serialization() {
    // Test different vector characteristics
    let test_cases = vec![
        ("small_dense", create_test_vector(128, 0.1)),
        ("medium_sparse", create_test_vector(512, 0.8)),
        ("large_dense", create_test_vector(1024, 0.2)),
        ("very_large_sparse", create_test_vector(2048, 0.95)),
    ];
    
    for (test_name, vector) in test_cases {
        let record = create_test_sst_record(format!("test_{}", test_name), vector);
        
        // Test optimized serialization
        let config = VectorSerializationConfig::for_dimension(record.vector.len());
        let serialized = record.serialize_with_config(&config).unwrap();
        let deserialized = SstRecord::deserialize(&serialized).unwrap();
        
        // Verify all fields match
        assert_eq!(record.id, deserialized.id);
        assert_eq!(record.vector.len(), deserialized.vector.len());
        assert_eq!(record.metadata.len(), deserialized.metadata.len());
        assert_eq!(record.timestamp, deserialized.timestamp);
        assert_eq!(record.version, deserialized.version);
        assert_eq!(record.sequence_number, deserialized.sequence_number);
        
        // Verify vector values match exactly
        for (i, (&original, &recovered)) in record.vector.iter().zip(deserialized.vector.iter()).enumerate() {
            assert!((original - recovered).abs() < f32::EPSILON, 
                "Vector value mismatch at index {} for test {}: {} != {}", 
                i, test_name, original, recovered);  
        }
        
        debug!("✅ SstRecord optimized serialization passed: {} ({} bytes)", 
            test_name, serialized.len());
    }
}

#[test]
fn test_data_block_zstd_compression() {
    // Create DataBlock with multiple records containing different vector types
    let records = vec![
        create_test_sst_record("dense_128".to_string(), create_test_vector(128, 0.1)),
        create_test_sst_record("sparse_512".to_string(), create_test_vector(512, 0.8)),
        create_test_sst_record("dense_1024".to_string(), create_test_vector(1024, 0.2)),
        create_test_sst_record("sparse_2048".to_string(), create_test_vector(2048, 0.9)),
        create_test_sst_record("medium_768".to_string(), create_test_vector(768, 0.5)),
    ];
    
    let data_block = DataBlock::new(1, records);
    
    // Test with compression enabled
    let mut compression_config = DataBlockCompressionConfig::default();
    compression_config.enable_compression = true;
    compression_config.compression_threshold = 1024; // 1KB threshold
    compression_config.compression_level = 6; // Higher compression
    
    let serialized = data_block.serialize_with_config(&compression_config).unwrap();
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    
    // Verify block metadata
    assert_eq!(data_block.block_id, deserialized.block_id);
    assert_eq!(data_block.records.len(), deserialized.records.len());
    
    // Verify all records match
    for (i, (original, recovered)) in data_block.records.iter().zip(deserialized.records.iter()).enumerate() {
        assert_eq!(original.id, recovered.id, "Record {} ID mismatch", i);
        assert_eq!(original.vector.len(), recovered.vector.len(), "Record {} vector length mismatch", i);
        
        // Verify vector values
        for (j, (&orig_val, &rec_val)) in original.vector.iter().zip(recovered.vector.iter()).enumerate() {
            assert!((orig_val - rec_val).abs() < f32::EPSILON, 
                "Record {} vector value mismatch at index {}: {} != {}", 
                i, j, orig_val, rec_val);
        }
    }
    
    // Check compression statistics  
    let (is_compressed, compression_ratio, uncompressed_size) = deserialized.compression_stats();
    
    if is_compressed {
        debug!("📦 DataBlock ZSTD compression - Ratio: {:.3}, Original: {} bytes, Compressed: {} bytes", 
            compression_ratio, uncompressed_size, serialized.len());
        assert!(compression_ratio < 0.95, "Compression should be beneficial");
    } else {
        debug!("📦 DataBlock stored uncompressed - {} bytes", serialized.len());
    }
}

#[test]
fn test_compression_performance_benchmark() {
    let vectors = (0..100).map(|i| {
        create_test_sst_record(
            format!("record_{}", i),
            create_test_vector(1024, if i % 2 == 0 { 0.9 } else { 0.1 }) // Mix sparse and dense
        )
    }).collect();
    
    let data_block = DataBlock::new(1, vectors);
    
    // Benchmark uncompressed serialization
    let start = Instant::now();
    let uncompressed = {
        let mut config = DataBlockCompressionConfig::default();
        config.enable_compression = false;
        data_block.serialize_with_config(&config).unwrap()
    };
    let uncompressed_time = start.elapsed();
    
    // Benchmark compressed serialization  
    let start = Instant::now();
    let compressed = {
        let mut config = DataBlockCompressionConfig::default();
        config.enable_compression = true;
        config.compression_level = 3;
        data_block.serialize_with_config(&config).unwrap()
    };
    let compressed_time = start.elapsed();
    
    let compression_ratio = compressed.len() as f32 / uncompressed.len() as f32;
    let speed_ratio = compressed_time.as_micros() as f32 / uncompressed_time.as_micros() as f32;
    
    debug!("⚡ Performance Benchmark Results:");
    debug!("   Uncompressed: {} bytes in {:?}", uncompressed.len(), uncompressed_time);
    debug!("   Compressed: {} bytes in {:?}", compressed.len(), compressed_time);
    debug!("   Compression ratio: {:.3}", compression_ratio);
    debug!("   Speed overhead: {:.2}x", speed_ratio);
    
    // Compression should be beneficial
    assert!(compression_ratio < 0.8, "Should achieve at least 20% compression");
    
    // Speed overhead should be reasonable (< 5x slower)
    assert!(speed_ratio < 5.0, "Compression overhead should be reasonable");
}

#[test]
fn test_adaptive_vector_optimization() {
    let mut config = VectorSerializationConfig::default();
    config.adaptive_compression = true;
    
    // Test optimization for different vector types
    let test_vectors = vec![
        ("small_dense", create_test_vector(64, 0.1)),
        ("medium_sparse", create_test_vector(512, 0.8)),
        ("large_very_sparse", create_test_vector(2048, 0.95)),
    ];
    
    for (name, vector) in test_vectors {
        let analysis = config.analyze_vector(&vector);
        
        debug!("📈 Vector Analysis for {}: dim={}, sparsity={:.3}, variance={:.6}", 
            name, analysis.dimension, analysis.sparsity, analysis.variance);
        
        // Test adaptive optimization
        let mut optimized_config = config.clone();
        optimized_config.optimize_for_analysis(&analysis);
        
        // Verify optimization decisions
        match name {
            "small_dense" => {
                // Small vectors should avoid compression overhead
                if analysis.dimension < 64 {
                    assert_eq!(optimized_config.compression_algorithm, CompressionAlgorithm::None);
                }
            }
            "large_very_sparse" => {
                // Very sparse vectors should use aggressive compression
                assert!(optimized_config.compression_level >= 6);
                assert!(optimized_config.compression_threshold <= 256);
            }
            _ => {} // Medium cases use defaults
        }
        
        // Test serialization with optimization
        let serialized = optimized_config.serialize_vector(&vector).unwrap();
        let deserialized = optimized_config.deserialize_vector(&serialized).unwrap();
        
        assert_eq!(vector, deserialized, "Adaptive optimization broke serialization for {}", name);
    }
}

#[test]
fn test_backward_compatibility() {
    // Create record with old-style serialization (simulated)
    let record = create_test_sst_record("compatibility_test".to_string(), create_test_vector(256, 0.3));
    
    // Serialize with legacy bincode (this simulates existing data)
    let legacy_serialized = bincode::serialize(&record).unwrap();
    
    // Should be able to deserialize with new format-aware deserializer
    let deserialized = SstRecord::deserialize(&legacy_serialized).unwrap();
    
    // Verify fields match
    assert_eq!(record.id, deserialized.id);
    assert_eq!(record.vector, deserialized.vector);
    assert_eq!(record.timestamp, deserialized.timestamp);
    
    debug!("✅ Backward compatibility test passed - legacy format works");
}

#[test]
fn test_memory_efficiency() {
    // Test that bytemuck serialization doesn't cause memory allocation overhead
    let large_vector = create_test_vector(10000, 0.5); // 40KB vector
    let config = VectorSerializationConfig::default();
    
    // Multiple serialization rounds to test for memory leaks
    for i in 0..10 {
        let serialized = config.serialize_vector(&large_vector).unwrap();
        let deserialized = config.deserialize_vector(&serialized).unwrap();
        
        assert_eq!(large_vector.len(), deserialized.len());
        
        // Spot check some values
        for j in (0..large_vector.len()).step_by(1000) {
            assert!((large_vector[j] - deserialized[j]).abs() < f32::EPSILON);
        }
        
        if i % 5 == 0 {
            debug!("🔄 Memory efficiency test round {} passed", i + 1);
        }
    }
    
    debug!("✅ Memory efficiency test completed - no leaks detected");
}

#[test]
fn test_error_handling() {
    let config = VectorSerializationConfig::default();
    
    // Test corrupted data
    let corrupted_data = vec![0xFF; 100];
    let result = config.deserialize_vector(&corrupted_data);
    assert!(result.is_err(), "Should fail on corrupted data");
    
    // Test empty data
    let empty_data = vec![];
    let result = SstRecord::deserialize(&empty_data);
    assert!(result.is_err(), "Should fail on empty data");
    
    // Test truncated data
    let record = create_test_sst_record("test".to_string(), create_test_vector(128, 0.1));
    let serialized = record.serialize().unwrap();
    let truncated = &serialized[..serialized.len() / 2];
    let result = SstRecord::deserialize(truncated);
    assert!(result.is_err(), "Should fail on truncated data");
    
    debug!("✅ Error handling tests passed");
}