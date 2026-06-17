// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! End-to-end integration tests for ProximaCodec with ProximaDataBlock
//!
//! This test verifies that the entire encoding pipeline works correctly:
//! 1. ProximaCodec encodes vectors with adaptive scheme selection
//! 2. ProximaDataBlock uses ProximaCodec for all encoding strategies
//! 3. Round-trip encoding/decoding preserves data perfectly (lossless)

use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::core::formats::proximablocks::block_structures::ProximaDataBlock;
use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout,
};
use proximadb::storage::engines::core::ops::proximacodec::types::ProximaScheme;
use proximadb::storage::engines::core::ops::proximacodec::{ProximaCodec, analysis};

#[test]
fn test_proximacodec_basic_roundtrip() {
    let codec = ProximaCodec::global();

    // Test f32 encoding
    let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
    let encoded = codec
        .encode(&values, ProximaScheme::Delta { base: 0 })
        .unwrap();
    let decoded = codec.decode(&encoded).unwrap();
    assert_eq!(values, decoded);

    // Test i32 encoding
    let values_i32 = vec![10i32, 20, 30, 40, 50];
    let encoded_i32 = codec
        .encode_i32(&values_i32, ProximaScheme::Delta { base: 0 })
        .unwrap();
    let decoded_i32 = codec.decode_i32(&encoded_i32).unwrap();
    assert_eq!(values_i32, decoded_i32);

    // Test i64 encoding
    let values_i64 = vec![100i64, 200, 300, 400, 500];
    let encoded_i64 = codec
        .encode_i64(&values_i64, ProximaScheme::Delta { base: 0 })
        .unwrap();
    let decoded_i64 = codec.decode_i64(&encoded_i64).unwrap();
    assert_eq!(values_i64, decoded_i64);
}

#[test]
fn test_proximacodec_lossless_enforcement() {
    let codec = ProximaCodec::global();

    // Test that lossy schemes are automatically replaced with lossless Delta
    let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];

    // These schemes would be lossy for f32 - should fallback to Delta
    let lossy_schemes = vec![
        ProximaScheme::Simple8b,
        ProximaScheme::RunLength,
        ProximaScheme::VByte,
    ];

    for scheme in lossy_schemes {
        let encoded = codec.encode(&values, scheme.clone()).unwrap();
        let decoded = codec.decode(&encoded).unwrap();

        // Verify lossless - exact match
        assert_eq!(values, decoded, "Failed with scheme {:?}", scheme);
    }
}

#[test]
fn test_proximacodec_adaptive_scheme_selection() {
    let _codec = ProximaCodec::global();

    // Sequential data - should choose Delta, DoubleDelta, or PForDelta (all valid for sequential)
    let sequential: Vec<f32> = (0..100).map(|i| i as f32).collect();
    let scheme = analysis::analyze_and_choose_scheme_f32(&sequential);

    match scheme {
        ProximaScheme::Delta { .. }
        | ProximaScheme::DoubleDelta { .. }
        | ProximaScheme::PForDelta { .. }
        | ProximaScheme::PForDoubleDelta { .. } => {
            println!("✓ Sequential data correctly identified: {:?}", scheme);
        }
        other => panic!(
            "Expected Delta/DoubleDelta/PForDelta/PForDoubleDelta for sequential, got {:?}",
            other
        ),
    }

    // Sparse data - should choose SparseBitmap or SparseCOO
    let mut sparse = vec![0.0f32; 100];
    sparse[10] = 1.0;
    sparse[50] = 2.0;
    sparse[90] = 3.0;

    let scheme = analysis::analyze_and_choose_scheme_f32(&sparse);
    match scheme {
        ProximaScheme::SparseBitmap | ProximaScheme::SparseCOO => {
            println!("✓ Sparse data correctly identified: {:?}", scheme);
        }
        other => panic!("Expected Sparse scheme for sparse data, got {:?}", other),
    }
}

#[test]
fn test_proximadatablock_with_proximacodec() {
    // Create test vectors
    let mut records = Vec::new();
    for i in 0..10 {
        let mut record = VectorRecord::default();
        record.id = format!("vec_{}", i);
        record.vector = vec![i as f32, (i + 1) as f32, (i + 2) as f32, (i + 3) as f32];
        record.timestamp = Some(i as i64);
        records.push(record);
    }

    // Test all encoding strategies
    let strategies = vec![
        VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
        VectorEncodingLayout::FullVector,
        VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector,
        VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector,
    ];

    for strategy in strategies {
        println!("\n=== Testing strategy: {:?} ===", strategy);

        // Create compression config for this strategy
        let config = BlockCompressionConfig {
            algorithm: CompressionAlgorithm::Zstd,
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 256,
            dictionary_compression: false,
            vector_layout: strategy.clone(),
            metadata_algorithm: None,
        };

        let proxima_records: Vec<_> = records
            .iter()
            .cloned()
            .map(proximadb::proto::defaults::vector_record_to_proxima_record)
            .collect();
        let block = ProximaDataBlock::new(proxima_records, config);
        println!("Block created with {} records", block.records.len());

        // Serialize (uses ProximaCodec internally)
        let serialized = block.serialize().unwrap();
        println!("Original size: {} bytes", records.len() * 4 * 4);
        println!("Serialized size: {} bytes", serialized.len());

        // Deserialize
        let deserialized = ProximaDataBlock::deserialize(&serialized, None).unwrap();
        println!(
            "Deserialized block has {} records",
            deserialized.records.len()
        );

        // Verify round-trip
        assert_eq!(block.records.len(), deserialized.records.len());
        for (i, (orig, deser)) in block
            .records
            .iter()
            .zip(deserialized.records.iter())
            .enumerate()
        {
            // Convert back to VectorRecord shape for field-level comparison.
            let orig = proximadb_records::conversions::proxima_record_to_vector(orig);
            let deser = proximadb_records::conversions::proxima_record_to_vector(deser);
            if orig.id != deser.id {
                println!(
                    "ID mismatch at index {}: orig='{}', deser='{}'",
                    i, orig.id, deser.id
                );
            }
            assert_eq!(orig.id, deser.id);

            if orig.vector != deser.vector {
                println!(
                    "Vector mismatch at index {}: orig={:?}, deser={:?}",
                    i, orig.vector, deser.vector
                );
            }
            assert_eq!(orig.vector, deser.vector);

            if orig.timestamp != deser.timestamp {
                println!(
                    "Timestamp mismatch at index {}: orig={:?}, deser={:?}",
                    i, orig.timestamp, deser.timestamp
                );
            }
            assert_eq!(orig.timestamp, deser.timestamp);
        }

        println!("✓ Strategy {:?} passed round-trip test", strategy);
    }
}

#[test]
fn test_large_vector_encoding() {
    let codec = ProximaCodec::global();

    // Simulate embedding vectors (e.g., OpenAI ada-002: 1536 dimensions)
    let values: Vec<f32> = (0..1536).map(|i| (i as f32) * 0.001).collect();

    // Analyze and choose optimal scheme
    let detected_scheme = analysis::analyze_and_choose_scheme_f32(&values);
    println!(
        "Detected scheme for 1536-dim embedding: {:?}",
        detected_scheme
    );

    // Encode
    let encoded = codec.encode(&values, detected_scheme).unwrap();

    // Decode
    let decoded = codec.decode(&encoded).unwrap();

    // Verify lossless
    assert_eq!(values.len(), decoded.len());
    for (orig, dec) in values.iter().zip(decoded.iter()) {
        assert_eq!(orig, dec);
    }

    println!("Original size: {} bytes", values.len() * 4);
    println!("Encoded size: {} bytes", encoded.len());
    println!(
        "Compression ratio: {:.2}x",
        (values.len() * 4) as f32 / encoded.len() as f32
    );
}

#[test]
fn test_type_safety() {
    let codec = ProximaCodec::global();

    // Encode as f32
    let values_f32 = vec![1.0f32, 2.0, 3.0];
    let encoded = codec
        .encode(&values_f32, ProximaScheme::Delta { base: 0 })
        .unwrap();

    // Try to decode as i64 (should fail)
    let result_i64 = codec.decode_i64(&encoded);
    assert!(result_i64.is_err(), "Should fail to decode f32 as i64");

    // Try to decode as i32 (should fail)
    let result_i32 = codec.decode_i32(&encoded);
    assert!(result_i32.is_err(), "Should fail to decode f32 as i32");

    // Decode as f32 (should succeed)
    let decoded = codec.decode(&encoded).unwrap();
    assert_eq!(values_f32, decoded);
}

#[test]
fn test_compression_effectiveness() {
    let codec = ProximaCodec::global();

    // Sequential data - should compress very well with Delta
    let sequential: Vec<i64> = (0..10000).collect();
    let encoded = codec
        .encode_i64(&sequential, ProximaScheme::Delta { base: 0 })
        .unwrap();

    let original_size = sequential.len() * 8;
    let compressed_size = encoded.len();
    let ratio = original_size as f32 / compressed_size as f32;

    println!("Sequential data compression:");
    println!("  Original: {} bytes", original_size);
    println!("  Compressed: {} bytes", compressed_size);
    println!("  Ratio: {:.2}x", ratio);

    // Should achieve at least 2x compression for sequential data
    assert!(
        ratio > 2.0,
        "Expected at least 2x compression for sequential data"
    );
}
