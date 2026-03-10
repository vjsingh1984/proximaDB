// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Simple8b Encoding - Raw implementation (no headers)
//!
//! Packs multiple small integers into 64-bit words using selectors.
//! Each word has a 4-bit selector indicating packing mode.
//! Best for small integers where most values fit in few bits.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

use super::helpers;

/// Simple8b selector modes
/// Each mode packs different number of values with different bit widths
const SELECTORS: [(u8, u8); 16] = [
    (240, 0), // 0: 240 × 0-bit (all zeros)
    (120, 0), // 1: 120 × 0-bit (unused, reserved)
    (60, 1),  // 2: 60 × 1-bit
    (30, 2),  // 3: 30 × 2-bit
    (20, 3),  // 4: 20 × 3-bit
    (15, 4),  // 5: 15 × 4-bit
    (12, 5),  // 6: 12 × 5-bit
    (10, 6),  // 7: 10 × 6-bit
    (8, 7),   // 8: 8 × 7-bit
    (7, 8),   // 9: 7 × 8-bit
    (6, 10),  // 10: 6 × 10-bit
    (5, 12),  // 11: 5 × 12-bit
    (4, 15),  // 12: 4 × 15-bit
    (3, 20),  // 13: 3 × 20-bit
    (2, 30),  // 14: 2 × 30-bit
    (1, 60),  // 15: 1 × 60-bit
];

// ===== Core wire format encoding functions =====

/// Core encoding logic for i32 wire type (used by f32 and i32)
fn encode_simple8b_i32_wire(wire_values: &[i32]) -> Result<Vec<u8>> {
    let ints: Vec<u64> = wire_values.iter().map(|&v| v as u32 as u64).collect();
    encode_u64_internal(&ints)
}

/// Core encoding logic for i64 wire type
fn encode_simple8b_i64_wire(wire_values: &[i64]) -> Result<Vec<u8>> {
    let ints: Vec<u64> = wire_values.iter().map(|&v| v as u64).collect();
    encode_u64_internal(&ints)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 values using Simple8b (raw, no headers)
///
/// # Algorithm
/// 1. Convert f32 to u32 bits
/// 2. Pack integers into 64-bit words with 4-bit selector
/// 3. Choose optimal selector for each batch
///
/// # Format (raw data only, NO headers)
/// ```text
/// [word_count:4 bytes][64-bit words with selector:4 bits + data:60 bits]
/// ```text
///
/// # Parameters
/// - `values`: f32 slice to encode
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_simple8b_i32_wire)
}

/// Encode i64 values using Simple8b (raw, no headers)
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_simple8b_i64_wire)
}

/// Encode i32 values using Simple8b (raw, no headers)
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_simple8b_i32_wire)
}

/// Internal encoding logic for u64 values
fn encode_u64_internal(values: &[u64]) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let mut words = Vec::new();
    let mut pos = 0;

    while pos < values.len() {
        // Find best selector for remaining values
        let remaining = &values[pos..];
        let (_selector, count, word) = pack_word(remaining)?;

        words.push(word);
        pos += count;
    }

    let mut result = Vec::new();

    // Store word count
    let word_count = words.len() as u32;
    result.extend_from_slice(&word_count.to_le_bytes());

    // Store words
    for word in words {
        result.extend_from_slice(&word.to_le_bytes());
    }

    Ok(result)
}

/// Pack as many values as possible into a single 64-bit word
/// Returns: (selector, values_packed, word)
fn pack_word(values: &[u64]) -> Result<(u8, usize, u64)> {
    // Try selectors from smallest bit-width to largest
    for selector in 2..16 {
        let (count, bits) = SELECTORS[selector];
        let count = count as usize;

        if count == 0 || bits == 0 {
            continue;
        }

        let to_pack = values.len().min(count);

        // Check if all values fit in bit width
        let max_val = if bits < 64 {
            (1u64 << bits) - 1
        } else {
            u64::MAX
        };
        if values[..to_pack].iter().all(|&v| v <= max_val) {
            // Pack values into word
            let mut word = (selector as u64) << 60;

            for (i, &val) in values[..to_pack].iter().enumerate() {
                let shift = i * (bits as usize);
                word |= val << shift;
            }

            return Ok((selector as u8, to_pack, word));
        }
    }

    // Fallback: use selector 15 (1 × 60-bit)
    let val = values[0];
    if val >= (1u64 << 60) {
        return Err(anyhow::anyhow!("Simple8b: value too large: {}", val));
    }

    let word = (15u64 << 60) | val;
    Ok((15, 1, word))
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for i32 wire type
fn decode_simple8b_i32_wire(data: &[u8], count: usize) -> Result<Vec<i32>> {
    let ints = decode_u64_internal(data, count)?;
    Ok(ints.iter().map(|&v| v as u32 as i32).collect())
}

/// Core decoding logic for i64 wire type
fn decode_simple8b_i64_wire(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let ints = decode_u64_internal(data, count)?;
    Ok(ints.iter().map(|&v| v as i64).collect())
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Decode f32 values from Simple8b encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    helpers::decode_generic::<f32>(data, count, decode_simple8b_i32_wire)
}

/// Decode i64 values from Simple8b encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    helpers::decode_generic::<i64>(data, count, decode_simple8b_i64_wire)
}

/// Decode i32 values from Simple8b encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    helpers::decode_generic::<i32>(data, count, decode_simple8b_i32_wire)
}

/// Internal decoding logic
fn decode_u64_internal(data: &[u8], count: usize) -> Result<Vec<u64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("Simple8b decode: insufficient data"));
    }

    // Read word count
    let word_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    if data.len() < 4 + word_count * 8 {
        return Err(anyhow::anyhow!("Simple8b decode: insufficient word data"));
    }

    let mut result = Vec::with_capacity(count);
    let mut offset = 4;

    for _ in 0..word_count {
        if result.len() >= count {
            break;
        }

        let word = u64::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);
        offset += 8;

        // Extract selector (top 4 bits)
        let selector = (word >> 60) as usize;

        if selector >= SELECTORS.len() {
            return Err(anyhow::anyhow!(
                "Simple8b decode: invalid selector {}",
                selector
            ));
        }

        let (num_values, bits) = SELECTORS[selector];

        if bits == 0 {
            // All zeros
            for _ in 0..num_values.min((count - result.len()) as u8) {
                result.push(0);
            }
        } else {
            let mask = if bits < 64 {
                (1u64 << bits) - 1
            } else {
                u64::MAX
            };

            for i in 0..num_values {
                if result.len() >= count {
                    break;
                }

                let shift = (i as usize) * (bits as usize);
                let val = (word >> shift) & mask;
                result.push(val);
            }
        }
    }

    // Truncate to exact count
    result.truncate(count);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple8b_small_values() {
        // Values that fit in 3 bits (0-7)
        let values = vec![1i32, 2, 3, 4, 5, 6, 7, 0];

        let encoded = encode_i32(&values).expect("encoding should succeed for valid small values");
        let decoded = decode_i32(&encoded, values.len())
            .expect("decoding should succeed for valid encoded data");

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_simple8b_zeros() {
        // All zeros - should use selector 0
        let values = vec![0i32; 240];

        let encoded = encode_i32(&values).expect("encoding should succeed for zero values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("decoding should succeed for zero values");

        assert_eq!(values, decoded);

        // Should be very compact: 4 bytes (count) + 8+ bytes (words)
        assert!(encoded.len() < 50, "Encoded {} bytes", encoded.len());
    }

    #[test]
    fn test_simple8b_mixed_sizes() {
        // Mix of small and medium values
        let values = vec![1i32, 2, 15, 31, 63, 127, 255, 511];

        let encoded = encode_i32(&values).expect("encoding should succeed for mixed size values");
        let decoded = decode_i32(&encoded, values.len())
            .expect("decoding should succeed for mixed size values");

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_simple8b_i64_roundtrip() {
        let values = vec![0i64, 1, 10, 100, 1000, 10000];

        let encoded = encode_i64(&values).expect("encoding should succeed for i64 values");
        let decoded =
            decode_i64(&encoded, values.len()).expect("decoding should succeed for i64 values");

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_simple8b_f32_roundtrip() {
        let values = vec![0.0f32, 1.0, 2.0, 3.0, 4.0];

        let encoded = encode_f32(&values).expect("encoding should succeed for f32 values");
        let decoded =
            decode_f32(&encoded, values.len()).expect("decoding should succeed for f32 values");

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits());
        }
    }

    #[test]
    fn test_simple8b_empty() {
        let values: Vec<i32> = vec![];
        let encoded = encode_i32(&values).expect("encoding should succeed for empty values");
        assert!(encoded.is_empty());

        let decoded =
            decode_i32(&encoded, 0).expect("decoding should succeed for empty encoded data");
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_simple8b_single_value() {
        let values = vec![42i32];

        let encoded = encode_i32(&values).expect("encoding should succeed for single value");
        let decoded =
            decode_i32(&encoded, values.len()).expect("decoding should succeed for single value");

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_simple8b_sequential() {
        // Sequential values 0-99
        let values: Vec<i32> = (0..100).collect();

        let encoded = encode_i32(&values).expect("encoding should succeed for sequential values");
        let decoded = decode_i32(&encoded, values.len())
            .expect("decoding should succeed for sequential values");

        assert_eq!(values, decoded);

        // Should compress well (100 values in ~7 bits each)
        assert!(encoded.len() < 400); // Original: 400 bytes
    }

    #[test]
    fn test_simple8b_compression() {
        // Many small values - ideal for Simple8b
        let mut values = Vec::new();
        for _ in 0..1000 {
            values.push(1i32);
            values.push(2i32);
            values.push(3i32);
        }

        let encoded = encode_i32(&values).expect("encoding should succeed for compression test");
        let decoded = decode_i32(&encoded, values.len())
            .expect("decoding should succeed for compression test");

        assert_eq!(values, decoded);

        // Original: 12000 bytes (3000 × 4)
        // Compressed: Should be much smaller
        let original_size = values.len() * 4;
        let compression_ratio = original_size as f64 / encoded.len() as f64;
        assert!(compression_ratio > 5.0);
    }

    #[test]
    fn test_simple8b_one_bit_values() {
        // Values 0 and 1 only (1-bit)
        let values = vec![0i32, 1, 0, 1, 1, 0, 1, 1];

        let encoded = encode_i32(&values).expect("encoding should succeed for 1-bit values");
        let decoded =
            decode_i32(&encoded, values.len()).expect("decoding should succeed for 1-bit values");

        assert_eq!(values, decoded);
    }
}
