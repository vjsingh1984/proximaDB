/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Comprehensive tests for SST compression using unified compression module

use crate::core::compression::{
    CompressionAlgorithm as UnifiedCompressionAlgorithm, CompressionContext,
};
use crate::proto::proximadb_v1::{CompressionAlgorithm, CompressionConfig, MetadataItem};
use crate::storage::engines::impls::sst::{DataBlock, DataBlockCompressionConfig, SstRecord};

fn create_test_record(id: &str, vector_dim: usize) -> SstRecord {
    SstRecord {
        id: id.to_string(),
        vector: vec![1.0; vector_dim],
        metadata: vec![MetadataItem {
            key: "test_key".to_string(),
            value: Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                "test_value".to_string(),
            )),
        }],
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
fn test_unified_compression_roundtrip() {
    let records = vec![
        create_test_record("test1", 128),
        create_test_record("test2", 128),
    ];

    // Test all supported compression algorithms using centralized markers
    use crate::core::compression::markers::*;

    let algorithms_and_markers = vec![
        (UnifiedCompressionAlgorithm::None, MARKER_UNCOMPRESSED),
        (UnifiedCompressionAlgorithm::Zstd, MARKER_ZSTD),
        (UnifiedCompressionAlgorithm::Lz4, MARKER_LZ4),
        (UnifiedCompressionAlgorithm::Snappy, MARKER_SNAPPY),
        (UnifiedCompressionAlgorithm::Gzip, MARKER_GZIP),
        (UnifiedCompressionAlgorithm::Brotli, MARKER_BROTLI),
        (UnifiedCompressionAlgorithm::Bzip2, MARKER_BZIP2),
        (UnifiedCompressionAlgorithm::Deflate, MARKER_DEFLATE),
        (UnifiedCompressionAlgorithm::Xz, MARKER_XZ),
        (UnifiedCompressionAlgorithm::Zlib, MARKER_ZLIB),
        (UnifiedCompressionAlgorithm::Lz4hc, MARKER_LZ4HC),
        (UnifiedCompressionAlgorithm::Lzma, MARKER_LZMA),
        (UnifiedCompressionAlgorithm::Lzo, MARKER_LZO),
    ];

    for (algorithm, expected_marker) in algorithms_and_markers {
        let block = DataBlock::new(1, records.clone());
        let config = DataBlockCompressionConfig {
            compression: algorithm != UnifiedCompressionAlgorithm::None,
            compression_threshold: 100,
            compression_level: 3,
            compression_algorithm: algorithm.clone(),
            // vector_config removed -  Default::default(),
            collection_compression: None,
        };

        let serialized = block.serialize_with_config(&config).unwrap();

        // Check compression marker
        assert_eq!(
            serialized[0], expected_marker,
            "Algorithm {:?} should have marker 0x{:02x} but got 0x{:02x}",
            algorithm, expected_marker, serialized[0]
        );

        // Deserialize and verify data integrity
        let deserialized = DataBlock::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.block_id, 1);
        assert_eq!(deserialized.records.len(), 2);
        assert_eq!(deserialized.records[0].id, "test1");
        assert_eq!(deserialized.records[1].id, "test2");
        assert_eq!(deserialized.compression_algorithm, algorithm);

        // Verify vector data integrity
        assert_eq!(deserialized.records[0].vector, records[0].vector);
        assert_eq!(deserialized.records[1].vector, records[1].vector);
    }
}

#[test]
fn test_unified_compression_efficiency() {
    // Create highly compressible data
    let mut record = create_test_record("compress_test", 1000);
    record.vector = vec![42.0; 1000]; // Highly compressible repeated values

    let block = DataBlock::new(1, vec![record]);

    // Test uncompressed
    let uncompressed_config = DataBlockCompressionConfig {
        compression: false,
        compression_threshold: 0,
        compression_level: 3,
        compression_algorithm: UnifiedCompressionAlgorithm::None,
        // vector_config removed -  Default::default(),
        collection_compression: None,
    };
    let uncompressed = block.serialize_with_config(&uncompressed_config).unwrap();

    // Test with various compression algorithms
    let compression_algorithms = vec![
        UnifiedCompressionAlgorithm::Zstd,
        UnifiedCompressionAlgorithm::Lz4,
        UnifiedCompressionAlgorithm::Brotli,
    ];

    for algorithm in compression_algorithms {
        let config = DataBlockCompressionConfig {
            compression: true,
            compression_threshold: 100,
            compression_level: 6,
            compression_algorithm: algorithm.clone(),
            // vector_config removed -  Default::default(),
            collection_compression: None,
        };

        let compressed = block.serialize_with_config(&config).unwrap();

        // Compressed should be significantly smaller
        assert!(
            compressed.len() < uncompressed.len() / 2,
            "Algorithm {:?}: compressed size {} should be much less than uncompressed {}",
            algorithm,
            compressed.len(),
            uncompressed.len()
        );

        // Verify decompression integrity
        let deserialized = DataBlock::deserialize(&compressed).unwrap();
        assert_eq!(deserialized.records[0].vector.len(), 1000);
        assert_eq!(deserialized.records[0].vector[0], 42.0);
    }
}

#[test]
fn test_unified_compression_threshold() {
    let records = vec![create_test_record("small", 4)]; // Very small record

    let block = DataBlock::new(1, records);
    let config = DataBlockCompressionConfig {
        compression: true,
        compression_threshold: 10000, // High threshold
        compression_level: 3,
        compression_algorithm: UnifiedCompressionAlgorithm::Zstd,
        // vector_config removed -  Default::default(),
        collection_compression: None,
    };

    let serialized = block.serialize_with_config(&config).unwrap();

    // Should not compress due to threshold - should use uncompressed marker
    assert_eq!(serialized[0], MARKER_UNCOMPRESSED);
}

#[test]
fn test_unified_compression_context_integration() {
    // Test that SST context is properly used with unified compression
    use crate::core::compression;

    let test_data = b"Test data for compression context verification".repeat(100);

    // Compress using unified module with SST context
    let compressed = compression::compress(
        &test_data,
        UnifiedCompressionAlgorithm::Zstd,
        3,
        CompressionContext::Block,
    )
    .unwrap();

    // Decompress using unified module
    let decompressed = compression::decompress(
        &compressed,
        UnifiedCompressionAlgorithm::Zstd,
        CompressionContext::Block,
    )
    .unwrap();

    assert_eq!(test_data, decompressed.as_slice());
}

#[test]
fn test_unified_compression_mixed_deserialization() {
    // Test that blocks compressed with different algorithms can be deserialized together
    let algorithms = vec![
        UnifiedCompressionAlgorithm::None,
        UnifiedCompressionAlgorithm::Zstd,
        UnifiedCompressionAlgorithm::Lz4,
        UnifiedCompressionAlgorithm::Snappy,
    ];

    let mut serialized_blocks = Vec::new();

    for (i, algorithm) in algorithms.iter().enumerate() {
        let records = vec![create_test_record(&format!("test_{}", i), 128)];
        let block = DataBlock::new(i as u32, records);

        let config = DataBlockCompressionConfig {
            compression: *algorithm != UnifiedCompressionAlgorithm::None,
            compression_threshold: 100,
            compression_level: 3,
            compression_algorithm: algorithm.clone(),
            // vector_config removed -  Default::default(),
            collection_compression: None,
        };

        let serialized = block.serialize_with_config(&config).unwrap();
        serialized_blocks.push((serialized, algorithm.clone()));
    }

    // Deserialize all blocks and verify
    for (i, (serialized, original_algorithm)) in serialized_blocks.iter().enumerate() {
        let deserialized = DataBlock::deserialize(serialized).unwrap();
        assert_eq!(deserialized.block_id, i as u32);
        assert_eq!(deserialized.records[0].id, format!("test_{}", i));
        assert_eq!(deserialized.compression_algorithm, *original_algorithm);
    }
}
