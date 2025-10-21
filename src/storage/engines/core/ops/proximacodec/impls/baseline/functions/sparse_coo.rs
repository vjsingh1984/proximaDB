// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Sparse COO (Coordinate) encoding - Raw implementation (no headers)
//!
//! Stores sparse data as (index, value) pairs.
//! More efficient than bitmap for very sparse data (<5% non-zero).
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

use super::helpers;
use super::helpers::ToWireFormat;

// ===== Core wire format encoding functions =====

/// Core encoding logic for i32 wire type (used by f32 and i32)
fn encode_sparse_coo_i32_wire(wire_values: &[i32]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    // Find non-zero indices and values
    let mut coords = Vec::new();

    for (idx, &val) in wire_values.iter().enumerate() {
        if val != 0 {
            coords.push((idx as u32, val));
        }
    }

    let mut result = Vec::new();

    // Store count of non-zero values
    let num_nonzero = coords.len() as u32;
    result.extend_from_slice(&num_nonzero.to_le_bytes());

    // Store coordinate pairs
    for (idx, val) in coords {
        result.extend_from_slice(&idx.to_le_bytes());
        result.extend_from_slice(&val.to_le_bytes());
    }

    Ok(result)
}

/// Core encoding logic for i64 wire type
fn encode_sparse_coo_i64_wire(wire_values: &[i64]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    // Find non-zero indices and values
    let mut coords = Vec::new();

    for (idx, &val) in wire_values.iter().enumerate() {
        if val != 0 {
            coords.push((idx as u32, val));
        }
    }

    let mut result = Vec::new();

    // Store count of non-zero values
    let num_nonzero = coords.len() as u32;
    result.extend_from_slice(&num_nonzero.to_le_bytes());

    // Store coordinate pairs
    for (idx, val) in coords {
        result.extend_from_slice(&idx.to_le_bytes());
        result.extend_from_slice(&val.to_le_bytes());
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 sparse values using COO format (raw, no headers)
///
/// # Algorithm
/// 1. Find non-zero values
/// 2. Store as pairs: (index:u32, value:f32)
///
/// # Format (raw data only, NO headers)
/// ```
/// [num_nonzero:4 bytes]([index:4 bytes][value:4 bytes])*
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode (may be sparse)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_sparse_coo_i32_wire)
}

/// Encode i64 sparse values using COO format (raw, no headers)
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_sparse_coo_i64_wire)
}

/// Encode i32 sparse values using COO format (raw, no headers)
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_sparse_coo_i32_wire)
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for i32 wire type
fn decode_sparse_coo_i32_wire(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("SparseCOO decode: insufficient data"));
    }

    // Read number of non-zero values
    let num_nonzero = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    // Each coordinate pair is 8 bytes (4 for index + 4 for value)
    if data.len() < 4 + num_nonzero * 8 {
        return Err(anyhow::anyhow!(
            "SparseCOO decode: insufficient coordinate data"
        ));
    }

    // Initialize result with zeros
    let mut result = vec![0i32; count];

    // Read coordinate pairs
    for i in 0..num_nonzero {
        let offset = 4 + i * 8;

        let idx = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;

        let val = i32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);

        if idx < count {
            result[idx] = val;
        }
    }

    Ok(result)
}

/// Core decoding logic for i64 wire type
fn decode_sparse_coo_i64_wire(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("SparseCOO decode: insufficient data"));
    }

    // Read number of non-zero values
    let num_nonzero = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    // Each coordinate pair is 12 bytes (4 for index + 8 for value)
    if data.len() < 4 + num_nonzero * 12 {
        return Err(anyhow::anyhow!(
            "SparseCOO decode: insufficient coordinate data"
        ));
    }

    // Initialize result with zeros
    let mut result = vec![0i64; count];

    // Read coordinate pairs
    for i in 0..num_nonzero {
        let offset = 4 + i * 12;

        let idx = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;

        let val = i64::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
        ]);

        if idx < count {
            result[idx] = val;
        }
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Decode f32 values from COO encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    helpers::decode_generic::<f32>(data, count, decode_sparse_coo_i32_wire)
}

/// Decode i64 values from COO encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    helpers::decode_generic::<i64>(data, count, decode_sparse_coo_i64_wire)
}

/// Decode i32 values from COO encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    helpers::decode_generic::<i32>(data, count, decode_sparse_coo_i32_wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sparse_coo_f32_roundtrip() {
        // Very sparse vector
        let mut values = vec![0.0f32; 1000];
        values[10] = 1.5;
        values[100] = 2.5;
        values[500] = 3.5;
        values[999] = 4.5;

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_sparse_coo_i64_roundtrip() {
        // Extremely sparse vector
        let mut values = vec![0i64; 10000];
        values[1000] = 42;
        values[5000] = 100;
        values[9999] = 200;

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_sparse_coo_compression() {
        // 99.9% sparse vector
        let mut values = vec![0i32; 100000];
        for i in 0..10 {
            values[i * 10000] = (i as i32 + 1) * 10;
        }

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Verify excellent compression
        // Original: 100000 × 4 bytes = 400000 bytes
        // Encoded: 4 (count) + 10×8 (coords) = 84 bytes
        let original_bytes = values.len() * 4;
        assert!(
            encoded.len() < 100,
            "Should achieve extreme compression: {} bytes vs {} bytes original",
            encoded.len(),
            original_bytes
        );
    }

    #[test]
    fn test_sparse_coo_all_zeros() {
        let values = vec![0i64; 1000];

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Should be minimal (just count of 0)
        assert_eq!(encoded.len(), 4);
    }

    #[test]
    fn test_sparse_coo_better_than_bitmap() {
        // For very sparse data (<1%), COO should be smaller than bitmap
        let mut values = vec![0i32; 100000];
        values[50000] = 42; // Only 1 non-zero

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // COO: 4 (count) + 8 (one coordinate) = 12 bytes
        // Bitmap would need: 4 (count) + 12500 (bitmap) + 4 (value) = ~12508 bytes
        assert!(
            encoded.len() <= 12,
            "COO should be tiny: {} bytes",
            encoded.len()
        );
    }

    #[test]
    fn test_sparse_coo_empty() {
        let values: Vec<f32> = vec![];

        let encoded = encode_f32(&values).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_f32(&encoded, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_sparse_coo_single_value() {
        let mut values = vec![0.0f32; 10000];
        values[5000] = 42.5;

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Minimal size: 4 + 8 = 12 bytes
        assert_eq!(encoded.len(), 12);
    }

    #[test]
    fn test_sparse_coo_sequential_indices() {
        // Even with sequential non-zeros, COO is simple
        let mut values = vec![0i32; 100];
        values[10] = 1;
        values[11] = 2;
        values[12] = 3;

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }
}
