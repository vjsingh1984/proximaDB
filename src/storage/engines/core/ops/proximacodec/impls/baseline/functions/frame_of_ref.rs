// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Frame of Reference (FOR) encoding - Baseline (pure scalar) implementation
//!
//! **ARCHITECTURE NOTE**: This is the BASELINE implementation.
//! - NO SIMD intrinsics allowed
//! - NO GPU code
//! - Pure portable Rust only
//!
//! For SIMD acceleration, see: `src/storage/engines/core/ops/proximacodec/simd.rs`
//! For GPU acceleration, see: `src/storage/engines/core/ops/proximacodec/impls/gpu/`
//!
//! Stores a base value and encodes offsets from that base using fixed bit width.
//! Best for clustered data with small variance.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

/// Encode f32 values using Frame of Reference (raw, no headers)
///
/// # Algorithm
/// 1. Convert f32 to i32 (via to_bits)
/// 2. Compute offsets from base
/// 3. Pack offsets using specified bit width
///
/// # Format (raw data only, NO headers)
/// ```
/// [base:4 bytes][bitpacked_offsets...]
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode
/// - `base`: Base value for offset calculation (as i64)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
///
/// # Note
/// The bit width is determined automatically from the maximum offset.
/// The bit width is stored in the wire header (not in raw data).
pub fn encode_f32(values: &[f32], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();

    // Store base value (4 bytes for i32 representation)
    let base_i32 = base as i32;
    result.extend_from_slice(&base_i32.to_le_bytes());

    // Convert f32 to i32 and compute offsets (pure scalar)
    let offsets: Vec<i32> = values
        .iter()
        .map(|&v| {
            let v_bits = v.to_bits() as i32;
            v_bits.wrapping_sub(base_i32)
        })
        .collect();

    // Find optimal bit width for offsets
    let max_offset_abs = offsets
        .iter()
        .map(|&o| o.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_offset_abs == 0 {
        1
    } else {
        // Add 1 bit for sign, but cap at 32
        ((32 - max_offset_abs.leading_zeros() as u8) + 1).min(32)
    };

    // Store bit width
    result.push(bits);

    // Bit-pack the offsets
    let packed = bitpack_i32(&offsets, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Encode i64 values using Frame of Reference (raw, no headers)
pub fn encode_i64(values: &[i64], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();

    // Store base value (8 bytes)
    result.extend_from_slice(&base.to_le_bytes());

    // Compute offsets from base
    let offsets: Vec<i64> = values
        .iter()
        .map(|&v| v.wrapping_sub(base))
        .collect();

    // Find optimal bit width for offsets
    let max_offset_abs = offsets
        .iter()
        .map(|&o| o.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_offset_abs == 0 {
        1
    } else {
        // Add 1 bit for sign, but cap at 64
        ((64 - max_offset_abs.leading_zeros() as u8) + 1).min(64)
    };

    // Store bit width
    result.push(bits);

    // Bit-pack the offsets
    let packed = bitpack_i64(&offsets, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Encode i32 values using Frame of Reference (raw, no headers)
pub fn encode_i32(values: &[i32], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();

    let base_i32 = base as i32;
    result.extend_from_slice(&base_i32.to_le_bytes());

    // Compute offsets from base
    let offsets: Vec<i32> = values
        .iter()
        .map(|&v| v.wrapping_sub(base_i32))
        .collect();

    // Find optimal bit width for offsets
    let max_offset_abs = offsets
        .iter()
        .map(|&o| o.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_offset_abs == 0 {
        1
    } else {
        // Add 1 bit for sign, but cap at 32
        ((32 - max_offset_abs.leading_zeros() as u8) + 1).min(32)
    };

    // Store bit width
    result.push(bits);

    // Bit-pack the offsets
    let packed = bitpack_i32(&offsets, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Decode f32 values from Frame of Reference encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 5 {
        return Err(anyhow::anyhow!("FOR decode: insufficient data"));
    }

    // Read base (4 bytes)
    let base_i32 = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    // Read bits (1 byte)
    let bits = data[4];

    // Unpack offsets
    let offsets = unbitpack_i32(&data[5..], bits, count)?;

    // Reconstruct values
    let values: Vec<f32> = offsets
        .iter()
        .map(|&offset| {
            let value_bits = base_i32.wrapping_add(offset) as u32;
            f32::from_bits(value_bits)
        })
        .collect();

    Ok(values)
}

/// Decode i64 values from Frame of Reference encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 9 {
        return Err(anyhow::anyhow!("FOR decode: insufficient data"));
    }

    // Read base (8 bytes)
    let base = i64::from_le_bytes([
        data[0], data[1], data[2], data[3],
        data[4], data[5], data[6], data[7],
    ]);

    // Read bits (1 byte)
    let bits = data[8];

    // Unpack offsets
    let offsets = unbitpack_i64(&data[9..], bits, count)?;

    // Reconstruct values
    let values: Vec<i64> = offsets
        .iter()
        .map(|&offset| base.wrapping_add(offset))
        .collect();

    Ok(values)
}

/// Decode i32 values from Frame of Reference encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 5 {
        return Err(anyhow::anyhow!("FOR decode: insufficient data"));
    }

    // Read base (4 bytes)
    let base = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    // Read bits (1 byte)
    let bits = data[4];

    // Unpack offsets
    let offsets = unbitpack_i32(&data[5..], bits, count)?;

    // Reconstruct values
    let values: Vec<i32> = offsets
        .iter()
        .map(|&offset| base.wrapping_add(offset))
        .collect();

    Ok(values)
}

// ===== Bit-packing helpers =====

/// Bit-pack i32 values (simple implementation, no SIMD)
fn bitpack_i32(values: &[i32], bits: u8) -> Result<Vec<u8>> {
    if bits > 32 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 32", bits));
    }

    if bits == 0 {
        return Ok(Vec::new());
    }

    let total_bits = values.len() * bits as usize;
    let total_bytes = (total_bits + 7) / 8;
    let mut result = vec![0u8; total_bytes];

    let mut bit_offset = 0;
    for &value in values {
        let value_u32 = value as u32;

        for bit_pos in 0..bits {
            let bit = (value_u32 >> bit_pos) & 1;
            let byte_idx = bit_offset / 8;
            let bit_idx = bit_offset % 8;

            if byte_idx < result.len() {
                result[byte_idx] |= (bit as u8) << bit_idx;
            }

            bit_offset += 1;
        }
    }

    Ok(result)
}

/// Bit-pack i64 values
fn bitpack_i64(values: &[i64], bits: u8) -> Result<Vec<u8>> {
    if bits > 64 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 64", bits));
    }

    if bits == 0 {
        return Ok(Vec::new());
    }

    let total_bits = values.len() * bits as usize;
    let total_bytes = (total_bits + 7) / 8;
    let mut result = vec![0u8; total_bytes];

    let mut bit_offset = 0;
    for &value in values {
        let value_u64 = value as u64;

        for bit_pos in 0..bits {
            let bit = (value_u64 >> bit_pos) & 1;
            let byte_idx = bit_offset / 8;
            let bit_idx = bit_offset % 8;

            if byte_idx < result.len() {
                result[byte_idx] |= (bit as u8) << bit_idx;
            }

            bit_offset += 1;
        }
    }

    Ok(result)
}

/// Unpack i32 values from bit-packed data
fn unbitpack_i32(data: &[u8], bits: u8, count: usize) -> Result<Vec<i32>> {
    if bits > 32 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 32", bits));
    }

    if bits == 0 {
        return Ok(vec![0; count]);
    }

    let mut result = Vec::with_capacity(count);
    let mut bit_offset = 0;

    for _ in 0..count {
        let mut value = 0u32;

        for bit_pos in 0..bits {
            let byte_idx = bit_offset / 8;
            let bit_idx = bit_offset % 8;

            if byte_idx < data.len() {
                let bit = (data[byte_idx] >> bit_idx) & 1;
                value |= (bit as u32) << bit_pos;
            }

            bit_offset += 1;
        }

        // Sign extend: if high bit is set, extend with 1s
        let signed_value = if bits < 32 && (value & (1 << (bits - 1))) != 0 {
            // Sign bit is set - extend with 1s
            let mask = !0u32 << bits;
            (value | mask) as i32
        } else {
            value as i32
        };

        result.push(signed_value);
    }

    Ok(result)
}

/// Unpack i64 values from bit-packed data
fn unbitpack_i64(data: &[u8], bits: u8, count: usize) -> Result<Vec<i64>> {
    if bits > 64 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 64", bits));
    }

    if bits == 0 {
        return Ok(vec![0; count]);
    }

    let mut result = Vec::with_capacity(count);
    let mut bit_offset = 0;

    for _ in 0..count {
        let mut value = 0u64;

        for bit_pos in 0..bits {
            let byte_idx = bit_offset / 8;
            let bit_idx = bit_offset % 8;

            if byte_idx < data.len() {
                let bit = (data[byte_idx] >> bit_idx) & 1;
                value |= (bit as u64) << bit_pos;
            }

            bit_offset += 1;
        }

        // Sign extend: if high bit is set, extend with 1s
        let signed_value = if bits < 64 && (value & (1 << (bits - 1))) != 0 {
            // Sign bit is set - extend with 1s
            let mask = !0u64 << bits;
            (value | mask) as i64
        } else {
            value as i64
        };

        result.push(signed_value);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_for_f32_roundtrip() {
        // Clustered values around 100.0
        let values = vec![100.0f32, 101.0, 102.0, 103.0, 104.0];
        let base = 100.0f32.to_bits() as i64;

        let encoded = encode_f32(&values, base).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_for_i64_roundtrip() {
        // Clustered values around 1000
        let values = vec![1000i64, 1001, 1002, 1003, 1004];
        let base = 1000;

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_for_i32_roundtrip() {
        // Clustered values around 500
        let values = vec![500i32, 501, 502, 503, 504];
        let base = 500;

        let encoded = encode_i32(&values, base).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_for_compression() {
        // Values in range 1000-1015 (need 4 bits for offsets)
        let values: Vec<i64> = (1000..1016).collect();
        let base = 1000;

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Verify compression
        // Original: 16 values × 8 bytes = 128 bytes
        // Encoded: 8 bytes (base) + 1 byte (bits) + 16×4 bits = 8 + 1 + 8 = 17 bytes
        let original_bytes = values.len() * 8;
        assert!(
            encoded.len() < original_bytes,
            "Should compress: {} < {}",
            encoded.len(),
            original_bytes
        );
    }

    #[test]
    fn test_for_constant_data() {
        // All same value - offsets are all 0
        let values = vec![42i32; 100];
        let base = 42;

        let encoded = encode_i32(&values, base).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Verify extreme compression (all offsets = 0, stored as 1 bit each)
        let original_bytes = values.len() * 4;
        assert!(encoded.len() < original_bytes / 10); // Should be much smaller
    }

    #[test]
    fn test_for_empty() {
        let values: Vec<i64> = vec![];
        let base = 0;

        let encoded = encode_i64(&values, base).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_i64(&encoded, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_for_single_value() {
        let values = vec![42i64];
        let base = 42;

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, 1).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_for_negative_offsets() {
        // Base in middle, some values below
        let values = vec![95i32, 96, 100, 104, 105];
        let base = 100;

        let encoded = encode_i32(&values, base).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_for_optimal_base() {
        // Base = minimum value for best compression
        let values = vec![1000i64, 1001, 1002, 1050, 1100];
        let base = 1000; // Minimum

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // With optimal base, max offset is 100, needs ~7 bits
        // Much better than encoding full values
    }
}
