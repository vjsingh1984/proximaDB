// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Variable-Byte (VByte) Encoding - Raw implementation (no headers)
//!
//! Encodes integers using variable number of bytes.
//! Uses 7 bits per byte for data, 1 bit for continuation.
//! Best for small integers where most values fit in 1-2 bytes.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

use super::helpers;

// ===== Core wire format encoding functions =====

/// Core encoding logic for i32 wire type (used by f32 and i32)
fn encode_vbyte_i32_wire(wire_values: &[i32]) -> Result<Vec<u8>> {
    let mut result = Vec::new();

    for &value in wire_values {
        encode_varint_u32(&mut result, value as u32);
    }

    Ok(result)
}

/// Core encoding logic for i64 wire type
fn encode_vbyte_i64_wire(wire_values: &[i64]) -> Result<Vec<u8>> {
    let mut result = Vec::new();

    for &value in wire_values {
        encode_varint_u64(&mut result, value as u64);
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 values using VByte (raw, no headers)
///
/// # Algorithm
/// Uses unsigned LEB128 encoding:
/// - Each byte uses 7 bits for data, MSB for continuation
/// - If MSB=1, more bytes follow
/// - If MSB=0, this is the last byte
///
/// # Parameters
/// - `values`: f32 slice to encode (interprets bits as u32)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_vbyte_i32_wire)
}

/// Encode i64 values using VByte (raw, no headers)
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_vbyte_i64_wire)
}

/// Encode i32 values using VByte (raw, no headers)
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_vbyte_i32_wire)
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for i32 wire type
fn decode_vbyte_i32_wire(data: &[u8], count: usize) -> Result<Vec<i32>> {
    let mut result = Vec::with_capacity(count);
    let mut offset = 0;

    for _ in 0..count {
        let (value, bytes_read) = decode_varint_u32(&data[offset..])?;
        result.push(value as i32);
        offset += bytes_read;
    }

    Ok(result)
}

/// Core decoding logic for i64 wire type
fn decode_vbyte_i64_wire(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut result = Vec::with_capacity(count);
    let mut offset = 0;

    for _ in 0..count {
        let (value, bytes_read) = decode_varint_u64(&data[offset..])?;
        result.push(value as i64);
        offset += bytes_read;
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Decode f32 values from VByte encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    helpers::decode_generic::<f32>(data, count, decode_vbyte_i32_wire)
}

/// Decode i64 values from VByte encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    helpers::decode_generic::<i64>(data, count, decode_vbyte_i64_wire)
}

/// Decode i32 values from VByte encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    helpers::decode_generic::<i32>(data, count, decode_vbyte_i32_wire)
}

// ===== VarInt encoding/decoding helpers =====

fn encode_varint_u32(buf: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        buf.push(((value & 0x7F) | 0x80) as u8);
        value >>= 7;
    }
    buf.push(value as u8);
}

fn encode_varint_u64(buf: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buf.push(((value & 0x7F) | 0x80) as u8);
        value >>= 7;
    }
    buf.push(value as u8);
}

fn decode_varint_u32(data: &[u8]) -> Result<(u32, usize)> {
    let mut value = 0u32;
    let mut shift = 0;
    let mut bytes_read = 0;

    for &byte in data {
        bytes_read += 1;

        value |= ((byte & 0x7F) as u32) << shift;

        if byte & 0x80 == 0 {
            return Ok((value, bytes_read));
        }

        shift += 7;

        if shift >= 32 {
            return Err(anyhow::anyhow!("VByte decode: overflow in u32"));
        }
    }

    Err(anyhow::anyhow!("VByte decode: incomplete varint"))
}

fn decode_varint_u64(data: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    let mut bytes_read = 0;

    for &byte in data {
        bytes_read += 1;

        value |= ((byte & 0x7F) as u64) << shift;

        if byte & 0x80 == 0 {
            return Ok((value, bytes_read));
        }

        shift += 7;

        if shift >= 64 {
            return Err(anyhow::anyhow!("VByte decode: overflow in u64"));
        }
    }

    Err(anyhow::anyhow!("VByte decode: incomplete varint"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vbyte_small_values() {
        // Small values fit in 1 byte each
        let values = vec![0i32, 1, 10, 50, 100, 127];

        let encoded = encode_i32(&values).expect("failed to encode small i32 values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("failed to decode small i32 values");

        assert_eq!(values, decoded);

        // Each value should be 1 byte (all < 128)
        assert_eq!(encoded.len(), values.len());
    }

    #[test]
    fn test_vbyte_medium_values() {
        // Values that fit in 2 bytes (128-16383)
        let values = vec![128i32, 200, 1000, 5000, 16383];

        let encoded = encode_i32(&values).expect("failed to encode medium i32 values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("failed to decode medium i32 values");

        assert_eq!(values, decoded);

        // Each value should be 2 bytes
        assert_eq!(encoded.len(), values.len() * 2);
    }

    #[test]
    fn test_vbyte_large_values() {
        // Mix of small and large values
        let values = vec![1i32, 128, 16384, 2097152];

        let encoded = encode_i32(&values).expect("failed to encode large i32 values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("failed to decode large i32 values");

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_vbyte_i64_roundtrip() {
        let values = vec![0i64, 127, 128, 16383, 16384, 1000000];

        let encoded = encode_i64(&values).expect("failed to encode i64 values");
        let decoded = decode_i64(&encoded, values.len()).expect("failed to decode i64 values");

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_vbyte_f32_roundtrip() {
        let values = vec![1.0f32, 2.0, 3.0, 100.0, 1000.0];

        let encoded = encode_f32(&values).expect("failed to encode f32 values");
        let decoded = decode_f32(&encoded, values.len()).expect("failed to decode f32 values");

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits());
        }
    }

    #[test]
    fn test_vbyte_empty() {
        let values: Vec<i32> = vec![];
        let encoded = encode_i32(&values).expect("failed to encode empty i32 array");
        assert!(encoded.is_empty());

        let decoded = decode_i32(&encoded, 0).expect("failed to decode empty i32 array");
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_vbyte_compression() {
        // Mostly small values - best case for VByte
        let mut values = Vec::new();
        for i in 0..100 {
            values.push(i as i32); // 0-99: all fit in 1-2 bytes
        }

        let encoded = encode_i32(&values).expect("failed to encode compression test values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("failed to decode compression test values");

        assert_eq!(values, decoded);

        // Original: 400 bytes (100 × i32)
        // VByte: ~150 bytes (most values in 1-2 bytes)
        assert!(
            encoded.len() < 200,
            "Should compress well: {} bytes",
            encoded.len()
        );
    }

    #[test]
    fn test_vbyte_zeros() {
        // Zeros encode to single byte
        let values = vec![0i32; 100];

        let encoded = encode_i32(&values).expect("failed to encode zero values");
        let decoded = decode_i32(&encoded, values.len()).expect("failed to decode zero values");

        assert_eq!(values, decoded);
        assert_eq!(encoded.len(), 100); // 1 byte per zero
    }

    #[test]
    fn test_vbyte_max_values() {
        // Test maximum values
        let values = vec![i32::MAX, i32::MAX - 1, i32::MAX - 1000];

        let encoded = encode_i32(&values).expect("failed to encode max i32 values");
        let decoded = decode_i32(&encoded, values.len()).expect("failed to decode max i32 values");

        assert_eq!(values, decoded);

        // i32::MAX needs 5 bytes in VByte
        assert!(encoded.len() >= 15); // 5 bytes × 3 values
    }

    #[test]
    fn test_vbyte_sequence() {
        // Sequential IDs
        let values: Vec<i32> = (1..=50).collect();

        let encoded = encode_i32(&values).expect("failed to encode sequential i32 values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("failed to decode sequential i32 values");

        assert_eq!(values, decoded);
    }
}
