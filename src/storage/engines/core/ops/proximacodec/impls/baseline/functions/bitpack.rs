// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! BitPacked encoding - Raw implementation (no headers)
//!
//! Packs integer values using a fixed bit width.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

/// Encode f32 values using bitpacking (raw, no headers)
///
/// # Algorithm
/// 1. Convert f32 to u32 (via to_bits)
/// 2. Pack using specified bit width
///
/// # Format (raw data only, NO headers)
/// ```
/// [bitpacked_values...]
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode
/// - `bits`: Bit width for packing (1-32)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    if bits > 32 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 32", bits));
    }

    if values.is_empty() {
        return Ok(Vec::new());
    }

    // Convert f32 to u32
    let u32_values: Vec<u32> = values.iter().map(|&v| v.to_bits()).collect();

    // Bit-pack
    bitpack_u32(&u32_values, bits)
}

/// Encode i64 values using bitpacking (raw, no headers)
pub fn encode_i64(values: &[i64], bits: u8) -> Result<Vec<u8>> {
    if bits > 64 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 64", bits));
    }

    if values.is_empty() {
        return Ok(Vec::new());
    }

    // Convert to u64 for bitpacking
    let u64_values: Vec<u64> = values.iter().map(|&v| v as u64).collect();

    bitpack_u64(&u64_values, bits)
}

/// Encode i32 values using bitpacking (raw, no headers)
pub fn encode_i32(values: &[i32], bits: u8) -> Result<Vec<u8>> {
    if bits > 32 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 32", bits));
    }

    if values.is_empty() {
        return Ok(Vec::new());
    }

    // Convert to u32 for bitpacking
    let u32_values: Vec<u32> = values.iter().map(|&v| v as u32).collect();

    bitpack_u32(&u32_values, bits)
}

/// Decode f32 values from bitpacked data
pub fn decode_f32(data: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    if bits > 32 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 32", bits));
    }

    // Unpack to u32
    let u32_values = unbitpack_u32(data, bits, count)?;

    // Convert u32 to f32
    let values: Vec<f32> = u32_values.iter().map(|&v| f32::from_bits(v)).collect();

    Ok(values)
}

/// Decode i64 values from bitpacked data
pub fn decode_i64(data: &[u8], bits: u8, count: usize) -> Result<Vec<i64>> {
    if bits > 64 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 64", bits));
    }

    // Unpack to u64
    let u64_values = unbitpack_u64(data, bits, count)?;

    // Convert u64 to i64
    let values: Vec<i64> = u64_values.iter().map(|&v| v as i64).collect();

    Ok(values)
}

/// Decode i32 values from bitpacked data
pub fn decode_i32(data: &[u8], bits: u8, count: usize) -> Result<Vec<i32>> {
    if bits > 32 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 32", bits));
    }

    // Unpack to u32
    let u32_values = unbitpack_u32(data, bits, count)?;

    // Convert u32 to i32
    let values: Vec<i32> = u32_values.iter().map(|&v| v as i32).collect();

    Ok(values)
}

// ===== Bit-packing helpers =====

/// Bit-pack u32 values (simple implementation, no SIMD)
fn bitpack_u32(values: &[u32], bits: u8) -> Result<Vec<u8>> {
    if bits == 0 {
        return Ok(Vec::new());
    }

    let total_bits = values.len() * bits as usize;
    let total_bytes = (total_bits + 7) / 8;
    let mut result = vec![0u8; total_bytes];

    let mut bit_offset = 0;
    for &value in values {
        // Mask to keep only the specified number of bits
        let masked_value = if bits < 32 {
            value & ((1u32 << bits) - 1)
        } else {
            value
        };

        for bit_pos in 0..bits {
            let bit = (masked_value >> bit_pos) & 1;
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

/// Bit-pack u64 values
fn bitpack_u64(values: &[u64], bits: u8) -> Result<Vec<u8>> {
    if bits == 0 {
        return Ok(Vec::new());
    }

    let total_bits = values.len() * bits as usize;
    let total_bytes = (total_bits + 7) / 8;
    let mut result = vec![0u8; total_bytes];

    let mut bit_offset = 0;
    for &value in values {
        // Mask to keep only the specified number of bits
        let masked_value = if bits < 64 {
            value & ((1u64 << bits) - 1)
        } else {
            value
        };

        for bit_pos in 0..bits {
            let bit = (masked_value >> bit_pos) & 1;
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

/// Unpack u32 values from bit-packed data
fn unbitpack_u32(data: &[u8], bits: u8, count: usize) -> Result<Vec<u32>> {
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

        result.push(value);
    }

    Ok(result)
}

/// Unpack u64 values from bit-packed data
fn unbitpack_u64(data: &[u8], bits: u8, count: usize) -> Result<Vec<u64>> {
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

        result.push(value);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitpack_f32_roundtrip() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
        let bits = 32; // Full precision

        let encoded = encode_f32(&values, bits).unwrap();
        let decoded = decode_f32(&encoded, bits, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_bitpack_i64_roundtrip() {
        let values = vec![10i64, 20, 30, 40, 50];
        let bits = 16; // 16 bits per value

        let encoded = encode_i64(&values, bits).unwrap();
        let decoded = decode_i64(&encoded, bits, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_bitpack_i32_roundtrip() {
        let values = vec![100i32, 200, 300, 400, 500];
        let bits = 16; // 16 bits per value

        let encoded = encode_i32(&values, bits).unwrap();
        let decoded = decode_i32(&encoded, bits, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_bitpack_small_width() {
        // Values that fit in 4 bits (0-15)
        let values = vec![1i32, 5, 10, 15, 3];
        let bits = 4;

        let encoded = encode_i32(&values, bits).unwrap();
        let decoded = decode_i32(&encoded, bits, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Verify compression
        let original_bytes = values.len() * 4; // 4 bytes per i32
        let compressed_bytes = encoded.len();
        assert!(
            compressed_bytes < original_bytes,
            "Expected compression: {} < {}",
            compressed_bytes,
            original_bytes
        );
    }

    #[test]
    fn test_bitpack_empty() {
        let values: Vec<i64> = vec![];
        let bits = 16;

        let encoded = encode_i64(&values, bits).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_i64(&encoded, bits, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_bitpack_single_bit() {
        // Binary values (0 or 1)
        let values = vec![0i32, 1, 0, 1, 1, 0];
        let bits = 1;

        let encoded = encode_i32(&values, bits).unwrap();
        let decoded = decode_i32(&encoded, bits, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Verify extreme compression
        let original_bytes = values.len() * 4;
        let compressed_bytes = encoded.len();
        assert_eq!(compressed_bytes, 1); // 6 bits fit in 1 byte
        assert!(compressed_bytes < original_bytes);
    }
}
