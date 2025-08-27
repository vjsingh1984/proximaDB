/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Comprehensive tests for SST compression with self-describing block markers

use crate::storage::engines::impls::sst::{
    DataBlock, DataBlockCompressionConfig, SstRecord,
};
use crate::core::compression::{CompressionAlgorithm as UnifiedCompressionAlgorithm};
use crate::proto::proximadb::{CompressionConfig, CompressionAlgorithm, MetadataItem};

fn create_test_record(id: &str, vector_dim: usize) -> SstRecord {
    SstRecord {
        id: id.to_string(),
        vector: vec![1.0; vector_dim],
        metadata: vec![
            MetadataItem {
                key: "test_key".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(
                    "test_value".to_string()
                )),
            }
        ],
        timestamp: 1000,
        updated_at: Some(1000),
        expires_at: None,
        version: Some(1),
        is_tombstone: false,
        sequence_number: 1,
        level: 0,
    }
}

#[test]
fn test_uncompressed_block() {
    let records = vec![
        create_test_record("test1", 128),
        create_test_record("test2", 128),
    ];
    
    let block = DataBlock::new(1, records.clone());
    let config = DataBlockCompressionConfig {
        compression: false,
        compression_threshold: 0,
        compression_level: 0,
        compression_algorithm: UnifiedCompressionAlgorithm::None,
        // vector_config removed -  Default::default(),
        collection_compression: None,
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    use crate::core::compression::markers::MARKER_UNCOMPRESSED;
    
    // Check for uncompressed marker
    assert_eq!(serialized[0], MARKER_UNCOMPRESSED);
    
    // Deserialize and verify
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.block_id, 1);
    assert_eq!(deserialized.records.len(), 2);
    assert_eq!(deserialized.records[0].id, "test1");
}

#[test]
fn test_zstd_compression() {
    let records = vec![
        create_test_record("test1", 256),
        create_test_record("test2", 256),
        create_test_record("test3", 256),
    ];
    
    let block = DataBlock::new(1, records.clone());
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 100,
        compression_level: 3,
        compression_algorithm: UnifiedCompressionAlgorithm::Zstd,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionZstd as i32,
            level: Some(3),
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    // Check for ZSTD marker
    assert_eq!(serialized[0], MARKER_ZSTD);
    
    // Deserialize and verify
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.block_id, 1);
    assert_eq!(deserialized.records.len(), 3);
    assert_eq!(deserialized.records[0].id, "test1");
}

#[test]
fn test_lz4_compression() {
    let records = vec![
        create_test_record("test1", 512),
        create_test_record("test2", 512),
    ];
    
    let block = DataBlock::new(2, records.clone());
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 100,
        compression_level: 0, // LZ4 doesn't use levels in lz4_flex
        compression_algorithm: UnifiedCompressionAlgorithm::Lz4,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionLz4 as i32,
            level: None,
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    use crate::core::compression::markers::MARKER_LZ4;
    
    // Check for LZ4 marker
    assert_eq!(serialized[0], MARKER_LZ4);
    
    // Deserialize and verify
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.block_id, 2);
    assert_eq!(deserialized.records.len(), 2);
}

#[test]
fn test_snappy_compression() {
    let records = vec![create_test_record("test1", 384)];
    
    let block = DataBlock::new(3, records.clone());
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 100,
        compression_level: 0, // Snappy doesn't use levels
        compression_algorithm: UnifiedCompressionAlgorithm::Snappy,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionSnappy as i32,
            level: None,
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    use crate::core::compression::markers::MARKER_SNAPPY;
    
    // Check for Snappy marker
    assert_eq!(serialized[0], MARKER_SNAPPY);
    
    // Deserialize and verify
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.block_id, 3);
    assert_eq!(deserialized.records.len(), 1);
}

#[test]
fn test_gzip_compression() {
    let records = vec![
        create_test_record("gzip_test1", 128),
        create_test_record("gzip_test2", 128),
    ];
    
    let block = DataBlock::new(4, records.clone());
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 100,
        compression_level: 6,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionGzip as i32,
            level: Some(6),
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    // Check for GZIP marker
    assert_eq!(serialized[0], MARKER_GZIP);
    
    // Deserialize and verify
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.block_id, 4);
    assert_eq!(deserialized.records.len(), 2);
}

#[test]
fn test_brotli_compression() {
    let records = vec![create_test_record("brotli_test", 256)];
    
    let block = DataBlock::new(5, records.clone());
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 100,
        compression_level: 4,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionBrotli as i32,
            level: Some(4),
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    // Check for Brotli marker
    assert_eq!(serialized[0], MARKER_BROTLI);
    
    // Deserialize and verify
    let deserialized = DataBlock::deserialize(&serialized).unwrap();
    assert_eq!(deserialized.block_id, 5);
    assert_eq!(deserialized.records.len(), 1);
}

#[test]
fn test_all_compression_algorithms() {
    // Test data
    let records = vec![
        create_test_record("test1", 128),
        create_test_record("test2", 128),
    ];
    
    // Test each algorithm
    let algorithms = vec![
        (CompressionAlgorithm::CompressionZstd, MARKER_ZSTD, 3),
        (CompressionAlgorithm::CompressionLz4, MARKER_LZ4, 0),
        (CompressionAlgorithm::CompressionSnappy, MARKER_SNAPPY, 0),
        (CompressionAlgorithm::CompressionGzip, MARKER_GZIP, 6),
        (CompressionAlgorithm::CompressionBrotli, MARKER_BROTLI, 4),
        (CompressionAlgorithm::CompressionBzip2, MARKER_BZIP2, 5),
        (CompressionAlgorithm::CompressionDeflate, MARKER_DEFLATE, 6),
        (CompressionAlgorithm::CompressionXz, MARKER_XZ, 6),
        (CompressionAlgorithm::CompressionZlib, MARKER_ZLIB, 6),
        (CompressionAlgorithm::CompressionLz4hc, MARKER_LZ4HC, 0),
        (CompressionAlgorithm::CompressionLzma, MARKER_LZMA, 6),
    ];
    
    for (algo, expected_marker, level) in algorithms {
        let block = DataBlock::new(100, records.clone());
        let config = DataBlockCompressionConfig {
            compression: true,
            compression_threshold: 100,
            compression_level: level,
            // vector_config removed -  Default::default(),
            collection_compression: Some(CompressionConfig {
                algorithm: algo as i32,
                level: Some(level),
                dynamic_block_sizing: false,
                block_size_mb: Some(8),
                adaptive: false,
            }),
        };
        
        let serialized = block.serialize_with_config(&config).unwrap();
        
        // Check marker
        assert_eq!(
            serialized[0], expected_marker,
            "Algorithm {:?} should have marker {:02x} but got {:02x}",
            algo, expected_marker, serialized[0]
        );
        
        // Deserialize and verify
        let deserialized = DataBlock::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.block_id, 100);
        assert_eq!(deserialized.records.len(), 2);
        assert_eq!(deserialized.records[0].id, "test1");
        assert_eq!(deserialized.records[1].id, "test2");
    }
}

#[test]
fn test_compression_threshold() {
    let records = vec![create_test_record("small", 4)]; // Very small record
    
    let block = DataBlock::new(6, records.clone());
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 10000, // High threshold
        compression_level: 3,
        compression_algorithm: UnifiedCompressionAlgorithm::Zstd,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionZstd as i32,
            level: Some(3),
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    // Should not compress due to threshold
    assert_eq!(serialized[0], MARKER_UNCOMPRESSED);
}

#[test]
fn test_compression_ratio_check() {
    // Create highly compressible data (repeated values)
    let mut record = create_test_record("compress_test", 1000);
    record.vector = vec![1.0; 1000]; // Highly compressible
    
    let block = DataBlock::new(7, vec![record]);
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 100,
        compression_level: 3,
        compression_algorithm: UnifiedCompressionAlgorithm::Zstd,
        // vector_config removed -  Default::default(),
        collection_compression: Some(CompressionConfig {
            algorithm: CompressionAlgorithm::CompressionZstd as i32,
            level: Some(3),
            dynamic_block_sizing: false,
            block_size_mb: Some(8),
            adaptive: false,
        }),
    };
    
    let serialized = block.serialize_with_config(&config).unwrap();
    
    // Should compress well
    assert_eq!(serialized[0], MARKER_ZSTD);
    
    // Compressed size should be much smaller than uncompressed
    let uncompressed_config = DataBlockCompressionConfig {
        compression: false,
        ..config
    };
    let uncompressed = block.serialize_with_config(&uncompressed_config).unwrap();
    
    assert!(
        serialized.len() < uncompressed.len() / 2,
        "Compressed size {} should be much less than uncompressed {}",
        serialized.len(),
        uncompressed.len()
    );
}

#[test]
fn test_mixed_compression_deserialization() {
    // Create blocks with different compression algorithms
    let blocks_data = vec![
        (CompressionAlgorithm::CompressionNone, MARKER_UNCOMPRESSED),
        (CompressionAlgorithm::CompressionZstd, MARKER_ZSTD),
        (CompressionAlgorithm::CompressionLz4, MARKER_LZ4),
        (CompressionAlgorithm::CompressionSnappy, MARKER_SNAPPY),
    ];
    
    let mut serialized_blocks = Vec::new();
    
    for (i, (algo, _expected_marker)) in blocks_data.iter().enumerate() {
        let records = vec![create_test_record(&format!("test_{}", i), 128)];
        let block = DataBlock::new(i as u32, records);
        
        let config = DataBlockCompressionConfig {
            compression: *algo != CompressionAlgorithm::CompressionNone,
            compression_threshold: 100,
            compression_level: 3,
            // vector_config removed -  Default::default(),
            collection_compression: if *algo != CompressionAlgorithm::CompressionNone {
                Some(CompressionConfig {
                    algorithm: *algo as i32,
                    level: Some(3),
                    dynamic_block_sizing: false,
                    block_size_mb: Some(8),
                    adaptive: false,
                })
            } else {
                None
            },
        };
        
        let serialized = block.serialize_with_config(&config).unwrap();
        serialized_blocks.push(serialized);
    }
    
    // Deserialize all blocks and verify
    for (i, serialized) in serialized_blocks.iter().enumerate() {
        let deserialized = DataBlock::deserialize(serialized).unwrap();
        assert_eq!(deserialized.block_id, i as u32);
        assert_eq!(deserialized.records[0].id, format!("test_{}", i));
    }
}

#[test]
fn test_backward_compatibility() {
    // Test that old bincode format can still be deserialized
    let records = vec![create_test_record("legacy", 64)];
    let block = DataBlock::new(99, records);
    
    // Use bincode directly (old format)
    let legacy_data = bincode::serialize(&block).unwrap();
    
    // Should still deserialize
    let deserialized = DataBlock::deserialize(&legacy_data).unwrap();
    assert_eq!(deserialized.block_id, 99);
    assert_eq!(deserialized.records[0].id, "legacy");
}