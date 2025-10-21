// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Run-Length Encoding (RLE) - Raw implementation (no headers)
//!
//! Compresses sequences of repeated values.
//! Best for data with long runs of identical values.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

use super::helpers;
use super::helpers::ToWireFormat;

// ===== Core wire format encoding functions =====

/// Core encoding logic for i32 wire type (used by f32 and i32)
fn encode_rle_i32_wire(wire_values: &[i32]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    let mut runs = Vec::new();
    let mut current_value = wire_values[0];
    let mut run_length = 1u32;

    for &val in &wire_values[1..] {
        if val == current_value {
            run_length += 1;
        } else {
            runs.push((run_length, current_value));
            current_value = val;
            run_length = 1;
        }
    }

    // Push final run
    runs.push((run_length, current_value));

    let mut result = Vec::new();

    // Store number of runs
    let num_runs = runs.len() as u32;
    result.extend_from_slice(&num_runs.to_le_bytes());

    // Store run-length pairs
    for (length, value) in runs {
        result.extend_from_slice(&length.to_le_bytes());
        result.extend_from_slice(&value.to_le_bytes());
    }

    Ok(result)
}

/// Core encoding logic for i64 wire type
fn encode_rle_i64_wire(wire_values: &[i64]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    let mut runs = Vec::new();
    let mut current_value = wire_values[0];
    let mut run_length = 1u32;

    for &val in &wire_values[1..] {
        if val == current_value {
            run_length += 1;
        } else {
            runs.push((run_length, current_value));
            current_value = val;
            run_length = 1;
        }
    }

    // Push final run
    runs.push((run_length, current_value));

    let mut result = Vec::new();

    // Store number of runs
    let num_runs = runs.len() as u32;
    result.extend_from_slice(&num_runs.to_le_bytes());

    // Store run-length pairs
    for (length, value) in runs {
        result.extend_from_slice(&length.to_le_bytes());
        result.extend_from_slice(&value.to_le_bytes());
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 values using run-length encoding (raw, no headers)
///
/// # Algorithm
/// 1. Find runs of identical values
/// 2. Store as (run_length:u32, value:f32) pairs
///
/// # Format (raw data only, NO headers)
/// ```
/// [num_runs:4 bytes]([run_length:4 bytes][value:4 bytes])*
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_rle_i32_wire)
}

/// Encode i64 values using run-length encoding (raw, no headers)
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_rle_i64_wire)
}

/// Encode i32 values using run-length encoding (raw, no headers)
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_rle_i32_wire)
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for i32 wire type
fn decode_rle_i32_wire(data: &[u8]) -> Result<Vec<i32>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("RLE decode: insufficient data"));
    }

    // Read number of runs
    let num_runs = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    // Each run is 8 bytes (4 for length + 4 for value)
    if data.len() < 4 + num_runs * 8 {
        return Err(anyhow::anyhow!("RLE decode: insufficient run data"));
    }

    let mut result = Vec::new();

    // Read and expand runs
    for i in 0..num_runs {
        let offset = 4 + i * 8;

        let run_length = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;

        let value = i32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);

        // Expand the run
        for _ in 0..run_length {
            result.push(value);
        }
    }

    Ok(result)
}

/// Core decoding logic for i64 wire type
fn decode_rle_i64_wire(data: &[u8]) -> Result<Vec<i64>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("RLE decode: insufficient data"));
    }

    // Read number of runs
    let num_runs = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    // Each run is 12 bytes (4 for length + 8 for value)
    if data.len() < 4 + num_runs * 12 {
        return Err(anyhow::anyhow!("RLE decode: insufficient run data"));
    }

    let mut result = Vec::new();

    // Read and expand runs
    for i in 0..num_runs {
        let offset = 4 + i * 12;

        let run_length = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;

        let value = i64::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
        ]);

        // Expand the run
        for _ in 0..run_length {
            result.push(value);
        }
    }

    Ok(result)
}

// ===== Public API (thin wrappers using wire format converters) =====

/// Decode f32 values from run-length encoded data
pub fn decode_f32(data: &[u8]) -> Result<Vec<f32>> {
    let wire_values = decode_rle_i32_wire(data)?;
    Ok(wire_values.iter().map(|&w| f32::from_wire(w)).collect())
}

/// Decode i64 values from run-length encoded data
pub fn decode_i64(data: &[u8]) -> Result<Vec<i64>> {
    decode_rle_i64_wire(data)
}

/// Decode i32 values from run-length encoded data
pub fn decode_i32(data: &[u8]) -> Result<Vec<i32>> {
    decode_rle_i32_wire(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rle_f32_roundtrip() {
        // Repeated values
        let mut values = Vec::new();
        values.extend(vec![1.0f32; 100]);
        values.extend(vec![2.0f32; 50]);
        values.extend(vec![3.0f32; 75]);

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_rle_i64_roundtrip() {
        // Long runs
        let mut values = Vec::new();
        values.extend(vec![42i64; 1000]);
        values.extend(vec![100i64; 500]);
        values.extend(vec![200i64; 750]);

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_rle_compression() {
        // Constant value - extreme compression
        let values = vec![42i32; 100000];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded).unwrap();

        assert_eq!(values, decoded);

        // Verify extreme compression
        // Original: 100000 × 4 bytes = 400000 bytes
        // Encoded: 4 (num_runs=1) + 8 (1 run) = 12 bytes
        assert_eq!(
            encoded.len(),
            12,
            "Should be minimal: {} bytes",
            encoded.len()
        );
    }

    #[test]
    fn test_rle_multiple_runs() {
        // Multiple different runs
        let mut values = Vec::new();
        for i in 0..10 {
            values.extend(vec![i as i32; 100]);
        }

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded).unwrap();

        assert_eq!(values, decoded);

        // 10 runs: 4 (count) + 10×8 = 84 bytes vs 1000×4 = 4000 bytes
        let original_bytes = values.len() * 4;
        assert!(
            encoded.len() < original_bytes / 20,
            "Should compress well: {} < {}",
            encoded.len(),
            original_bytes / 20
        );
    }

    #[test]
    fn test_rle_worst_case() {
        // No repeated values - worst case for RLE
        let values: Vec<i32> = (0..100).collect();

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded).unwrap();

        assert_eq!(values, decoded);

        // Will be larger due to RLE overhead (100 runs of length 1)
        // 4 (count) + 100×8 = 804 bytes vs 100×4 = 400 bytes original
        let original_bytes = values.len() * 4;
        assert!(encoded.len() > original_bytes);
    }

    #[test]
    fn test_rle_empty() {
        let values: Vec<f32> = vec![];

        let encoded = encode_f32(&values).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_f32(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_rle_single_value() {
        let values = vec![42.0f32];

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_rle_alternating() {
        // Alternating pattern - bad for RLE
        let mut values = Vec::new();
        for i in 0..100 {
            values.push(if i % 2 == 0 { 1i64 } else { 2i64 });
        }

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded).unwrap();

        assert_eq!(values, decoded);

        // 100 runs of length 1 each - inefficient
    }

    #[test]
    fn test_rle_long_run() {
        // Single very long run
        let values = vec![42i32; 1000000];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded).unwrap();

        assert_eq!(values, decoded);

        // Extreme compression: 12 bytes vs 4MB
        assert_eq!(encoded.len(), 12);
    }
}
