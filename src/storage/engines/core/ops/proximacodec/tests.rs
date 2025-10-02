// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for ProximaCodec
//!
//! Tests the complete encode-decode pipeline through WireFormatManager

use super::impls::baseline::{BaselineDecoder, BaselineEncoder};
use super::traits::{RawDecoder, RawEncoder};
use super::types::{ProximaScheme, TypeId};
use super::wire_format::WireFormatManager;

/// Test complete roundtrip: encode → wire format → decode
#[test]
fn test_complete_roundtrip_f32_delta() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;
    let wire_manager = WireFormatManager::new();

    let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
    let scheme = ProximaScheme::Delta { base: 0 };

    // Encode raw data
    let raw_encoded = encoder.encode_f32(&values, &scheme).unwrap();

    // Create wire format header
    let header = wire_manager.write_header(&scheme, values.len(), TypeId::F32);

    // Combine header + raw data
    let mut with_header = header;
    with_header.extend_from_slice(&raw_encoded);

    // Read wire format header
    let parsed_header = wire_manager.read_header(&with_header).unwrap();

    assert_eq!(parsed_header.type_id, TypeId::F32);
    assert_eq!(parsed_header.count, values.len());
    match parsed_header.scheme {
        ProximaScheme::Delta { .. } => {}, // Success
        _ => panic!("Expected Delta scheme"),
    }

    // Extract raw data (skip header bytes)
    let raw_data = &with_header[parsed_header.data_offset..];

    // Decode raw data
    let decoded = decoder.decode_f32(raw_data, &parsed_header.scheme, parsed_header.count).unwrap();

    assert_eq!(decoded, values);
}

#[test]
fn test_complete_roundtrip_i64_delta() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;
    let wire_manager = WireFormatManager::new();

    let values = vec![100i64, 200, 300, 400, 500];
    let scheme = ProximaScheme::Delta { base: 0 };

    // Encode raw data
    let raw_encoded = encoder.encode_i64(&values, &scheme).unwrap();

    // Create wire format header
    let header = wire_manager.write_header(&scheme, values.len(), TypeId::I64);

    // Combine header + raw data
    let mut with_header = header;
    with_header.extend_from_slice(&raw_encoded);

    // Read wire format header
    let parsed_header = wire_manager.read_header(&with_header).unwrap();

    assert_eq!(parsed_header.type_id, TypeId::I64);
    assert_eq!(parsed_header.count, values.len());
    match parsed_header.scheme {
        ProximaScheme::Delta { .. } => {}, // Success
        _ => panic!("Expected Delta scheme"),
    }

    // Extract raw data (skip header bytes)
    let raw_data = &with_header[parsed_header.data_offset..];

    // Decode raw data
    let decoded = decoder.decode_i64(raw_data, &parsed_header.scheme, parsed_header.count).unwrap();

    assert_eq!(decoded, values);
}

#[test]
fn test_complete_roundtrip_i32_delta() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;
    let wire_manager = WireFormatManager::new();

    let values = vec![10i32, 20, 30, 40, 50];
    let scheme = ProximaScheme::Delta { base: 0 };

    // Encode raw data
    let raw_encoded = encoder.encode_i32(&values, &scheme).unwrap();

    // Create wire format header
    let header = wire_manager.write_header(&scheme, values.len(), TypeId::I32);

    // Combine header + raw data
    let mut with_header = header;
    with_header.extend_from_slice(&raw_encoded);

    // Read wire format header
    let parsed_header = wire_manager.read_header(&with_header).unwrap();

    assert_eq!(parsed_header.type_id, TypeId::I32);
    assert_eq!(parsed_header.count, values.len());
    match parsed_header.scheme {
        ProximaScheme::Delta { .. } => {}, // Success
        _ => panic!("Expected Delta scheme"),
    }

    // Extract raw data (skip header bytes)
    let raw_data = &with_header[parsed_header.data_offset..];

    // Decode raw data
    let decoded = decoder.decode_i32(raw_data, &parsed_header.scheme, parsed_header.count).unwrap();

    assert_eq!(decoded, values);
}

/// Test sequential data compression
#[test]
fn test_sequential_data_compression() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values: Vec<i64> = (0..1000).collect();
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_i64(&values, &scheme).unwrap();
    let decoded = decoder.decode_i64(&encoded, &scheme, values.len()).unwrap();

    assert_eq!(decoded, values);

    // Verify compression
    let original_bytes = values.len() * 8;
    assert!(
        encoded.len() < original_bytes,
        "Should compress sequential data: {} bytes vs {} bytes",
        encoded.len(),
        original_bytes
    );
}

/// Test large floating-point vectors
#[test]
fn test_large_f32_vector() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    // Simulate embedding vectors
    let values: Vec<f32> = (0..1536).map(|i| (i as f32) * 0.001).collect();
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_f32(&values, &scheme).unwrap();
    let decoded = decoder.decode_f32(&encoded, &scheme, values.len()).unwrap();

    assert_eq!(decoded.len(), values.len());
    for (original, decoded_val) in values.iter().zip(decoded.iter()) {
        assert_eq!(original, decoded_val);
    }
}

/// Test edge case: empty vector
#[test]
fn test_empty_vector() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values: Vec<i64> = vec![];
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_i64(&values, &scheme).unwrap();
    assert!(encoded.is_empty());

    let decoded = decoder.decode_i64(&encoded, &scheme, 0).unwrap();
    assert!(decoded.is_empty());
}

/// Test edge case: single value
#[test]
fn test_single_value() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values = vec![42i64];
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_i64(&values, &scheme).unwrap();
    let decoded = decoder.decode_i64(&encoded, &scheme, 1).unwrap();

    assert_eq!(decoded, values);
}

/// Test negative deltas
#[test]
fn test_negative_deltas() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values = vec![1000i64, 900, 800, 700, 600];
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_i64(&values, &scheme).unwrap();
    let decoded = decoder.decode_i64(&encoded, &scheme, values.len()).unwrap();

    assert_eq!(decoded, values);
}

/// Test mixed positive and negative deltas
#[test]
fn test_mixed_deltas() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values = vec![100i64, 200, 150, 300, 250];
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_i64(&values, &scheme).unwrap();
    let decoded = decoder.decode_i64(&encoded, &scheme, values.len()).unwrap();

    assert_eq!(decoded, values);
}

/// Test with different base values
#[test]
fn test_different_base_values() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values = vec![1000i64, 1100, 1200, 1300, 1400];

    for base in [0, 500, 1000, 1500] {
        let scheme = ProximaScheme::Delta { base };

        let encoded = encoder.encode_i64(&values, &scheme).unwrap();
        let decoded = decoder.decode_i64(&encoded, &scheme, values.len()).unwrap();

        assert_eq!(decoded, values, "Failed with base={}", base);
    }
}

/// Test f32 special values
#[test]
fn test_f32_special_values() {
    let encoder = BaselineEncoder;
    let decoder = BaselineDecoder;

    let values = vec![0.0f32, -0.0, 1.0, -1.0, 100.5];
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_f32(&values, &scheme).unwrap();
    let decoded = decoder.decode_f32(&encoded, &scheme, values.len()).unwrap();

    assert_eq!(decoded.len(), values.len());
    for (original, decoded_val) in values.iter().zip(decoded.iter()) {
        // Use exact bit comparison for f32
        assert_eq!(original.to_bits(), decoded_val.to_bits());
    }
}
