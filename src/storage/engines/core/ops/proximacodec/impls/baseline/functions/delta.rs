// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Delta encoding - Baseline (pure scalar) implementation
//!
//! **ARCHITECTURE NOTE**: This is the BASELINE implementation.
//! - NO SIMD intrinsics allowed
//! - NO GPU code
//! - Pure portable Rust only
//!
//! For SIMD acceleration, see: `src/storage/engines/core/ops/proximacodec/simd.rs`
//! For GPU acceleration, see: `src/storage/engines/core/ops/proximacodec/impls/gpu/`
//!
//! Computes differences from a base value and bit-packs the deltas.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

// ===== Bitpacking delegation to shared helpers =====
//
// All bitpacking operations now use the shared helpers in bitpack.rs
// to avoid code duplication and ensure consistent sign extension behavior.

use super::bitpack;

/// Encode f32 values using delta encoding (raw, no headers)
///
/// # Algorithm
/// 1. Convert f32 to i32 (via to_bits)
/// 2. Compute deltas from base
/// 3. Find optimal bit width
/// 4. Bit-pack deltas
///
/// # Format (raw data only, NO headers)
/// ```
/// [base:4 bytes][bits:1 byte][bitpacked_deltas...]
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode
/// - `base`: Base value for delta calculation (as i64)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();

    // Store base value (4 bytes for i32 representation)
    let base_i32 = base as i32;
    result.extend_from_slice(&base_i32.to_le_bytes());

    // Convert f32 to i32 and compute deltas in i64 (NO OVERFLOW!)
    // Critical: i32 deltas can overflow, so we use i64
    let deltas: Vec<i64> = values
        .iter()
        .map(|&v| {
            let v_bits = v.to_bits() as i32 as i64;  // Sign-extend to i64
            v_bits - (base_i32 as i64)  // i64 arithmetic - no overflow!
        })
        .collect();

    // Find optimal bit width for deltas (now i64)
    let max_delta_abs = deltas
        .iter()
        .map(|&d| d.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_delta_abs == 0 {
        1
    } else {
        // Add 1 bit for sign, but cap at 64 (not 32!)
        ((64 - max_delta_abs.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    // Bit-pack the deltas (delegate to shared helper)
    let packed = bitpack::bitpack_i64(&deltas, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Encode i64 values using delta encoding (raw, no headers)
pub fn encode_i64(values: &[i64], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();

    // Store base value (8 bytes)
    result.extend_from_slice(&base.to_le_bytes());

    // Compute deltas
    let deltas: Vec<i64> = values
        .iter()
        .map(|&v| v.wrapping_sub(base))
        .collect();

    // Find optimal bit width
    let max_delta_abs = deltas
        .iter()
        .map(|&d| d.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_delta_abs == 0 {
        1
    } else {
        // Add 1 bit for sign, but cap at 64
        ((64 - max_delta_abs.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    // Bit-pack the deltas (delegate to shared helper)
    let packed = bitpack::bitpack_i64(&deltas, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Encode i32 values using delta encoding (raw, no headers)
pub fn encode_i32(values: &[i32], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();

    let base_i32 = base as i32;
    result.extend_from_slice(&base_i32.to_le_bytes());

    // Compute deltas in i64 (NO OVERFLOW!)
    // Critical: i32 deltas can overflow, so we use i64
    let deltas: Vec<i64> = values
        .iter()
        .map(|&v| (v as i64) - (base_i32 as i64))  // i64 arithmetic - no overflow!
        .collect();

    // Find optimal bit width (now i64)
    let max_delta_abs = deltas
        .iter()
        .map(|&d| d.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_delta_abs == 0 {
        1
    } else {
        // Add 1 bit for sign, but cap at 64 (not 32!)
        ((64 - max_delta_abs.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    // Bit-pack the deltas (delegate to shared helper)
    let packed = bitpack::bitpack_i64(&deltas, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Decode f32 values from delta-encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    if data.len() < 5 {
        return Err(anyhow::anyhow!("Delta decode: insufficient data"));
    }

    // Read base (4 bytes)
    let base_i32 = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    // Read bits (1 byte)
    let bits = data[4];

    // Unpack deltas (delegate to shared helper with sign extension)
    let deltas = bitpack::unbitpack_i64(&data[5..], bits, count)?;

    // Reconstruct values using i64 arithmetic (NO OVERFLOW!)
    let values: Vec<f32> = deltas
        .iter()
        .map(|&delta| {
            let value_i64 = (base_i32 as i64) + delta;  // i64 arithmetic - no overflow!
            let value_bits = value_i64 as i32 as u32;
            f32::from_bits(value_bits)
        })
        .collect();

    Ok(values)
}

/// Decode i64 values from delta-encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 || data.is_empty() {
        return Ok(Vec::new());
    }

    if data.len() < 9 {
        return Err(anyhow::anyhow!("Delta decode: insufficient data"));
    }

    // Read base (8 bytes)
    let base = i64::from_le_bytes([
        data[0], data[1], data[2], data[3],
        data[4], data[5], data[6], data[7],
    ]);

    // Read bits (1 byte)
    let bits = data[8];

    // Unpack deltas (delegate to shared helper with sign extension)
    let deltas = bitpack::unbitpack_i64(&data[9..], bits, count)?;

    // Reconstruct values
    let values: Vec<i64> = deltas
        .iter()
        .map(|&delta| base.wrapping_add(delta))
        .collect();

    Ok(values)
}

/// Decode i32 values from delta-encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if data.len() < 5 {
        return Err(anyhow::anyhow!("Delta decode: insufficient data"));
    }

    // Read base (4 bytes)
    let base = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    // Read bits (1 byte)
    let bits = data[4];

    // Unpack deltas (now i64!) (delegate to shared helper with sign extension)
    let deltas = bitpack::unbitpack_i64(&data[5..], bits, count)?;

    // Reconstruct values using i64 arithmetic (NO OVERFLOW!)
    let values: Vec<i32> = deltas
        .iter()
        .map(|&delta| {
            let value_i64 = (base as i64) + delta;  // i64 arithmetic - no overflow!
            value_i64 as i32
        })
        .collect();

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_f32_roundtrip() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
        let base = 0;

        let encoded = encode_f32(&values, base).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_delta_i64_roundtrip() {
        let values = vec![100i64, 200, 300, 400, 500];
        let base = 0;

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_delta_sequential() {
        let values: Vec<i64> = (0..1000).collect();
        let base = 0;

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Verify compression
        let original_bytes = values.len() * 8;
        assert!(encoded.len() < original_bytes, "Should compress sequential data");
    }

    // ===== OVERFLOW EDGE CASE TESTS =====
    // These tests verify that the i64 delta fix prevents overflow

    #[test]
    fn test_overflow_i32_extremes() {
        // Test with i32::MAX and i32::MIN - would overflow with i32 deltas
        let values = vec![i32::MIN, i32::MAX, i32::MIN, i32::MAX];
        let base = 0;

        let encoded = encode_i32(&values, base).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to roundtrip i32 extremes");
    }

    #[test]
    fn test_overflow_f32_extreme_bit_patterns() {
        // Test f32 values with extreme bit patterns that would cause i32 delta overflow
        let values = vec![
            f32::from_bits(i32::MAX as u32),
            f32::from_bits(i32::MIN as u32),
            f32::from_bits(0),
            f32::from_bits(i32::MAX as u32),
        ];
        let base = 0;

        let encoded = encode_f32(&values, base).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits(),
                "Failed to roundtrip extreme f32 bit pattern: orig={:08x}, dec={:08x}",
                orig.to_bits(), dec.to_bits());
        }
    }

    #[test]
    fn test_overflow_alternating_extremes() {
        // Worst case: alternating between extremes causes maximum deltas
        let values = vec![i32::MIN, i32::MAX, i32::MIN, i32::MAX, i32::MIN, i32::MAX];
        let base = 0;

        let encoded = encode_i32(&values, base).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to handle alternating extremes");
    }

    #[test]
    fn test_overflow_i64_full_range() {
        // Test i64 values across full range
        let values = vec![i64::MIN, i64::MIN / 2, 0i64, i64::MAX / 2, i64::MAX];
        let base = 0;

        let encoded = encode_i64(&values, base).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to handle i64 extremes");
    }

    #[test]
    fn test_overflow_f32_special_values() {
        // Test special f32 values with extreme bit patterns
        let values = vec![
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            f32::MAX,
            f32::MIN,
            0.0f32,
        ];
        let base = 0;

        let encoded = encode_f32(&values, base).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            // Compare bit patterns since NAN != NAN
            assert_eq!(orig.to_bits(), dec.to_bits(),
                "Failed to roundtrip special f32 value: orig={:08x}, dec={:08x}",
                orig.to_bits(), dec.to_bits());
        }
    }
}
