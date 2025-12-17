// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Double Delta Encoding - Raw implementation (no headers)
//!
//! Second-order delta encoding: compresses differences of differences.
//! Excellent for data with constant or near-constant rate of change.
//! Best for time series with linear trends or smooth curves.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

// ===== Bitpacking delegation to shared helpers =====
//
// All bitpacking operations now use the shared helpers in bitpack.rs
// to avoid code duplication and ensure consistent sign extension behavior.

use super::bitpack;
use super::helpers;

// ===== Core wire format encoding functions =====

/// Core encoding logic for i32 base + i64 deltas (used by f32)
fn encode_double_delta_i32_base_i64_deltas(wire_values: &[i32]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    if wire_values.len() == 1 {
        let mut result = Vec::new();
        result.extend_from_slice(&wire_values[0].to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    let base = wire_values[0];
    result.extend_from_slice(&base.to_le_bytes());

    if wire_values.len() == 2 {
        let first_delta = (wire_values[1] as i64) - (base as i64);
        result.extend_from_slice(&first_delta.to_le_bytes());
        result.push(0);
        return Ok(result);
    }

    // Compute first-order deltas in i64 (NO OVERFLOW!)
    let mut deltas: Vec<i64> = Vec::with_capacity(wire_values.len() - 1);
    for i in 1..wire_values.len() {
        let curr = wire_values[i] as i64;
        let prev = wire_values[i - 1] as i64;
        let delta = curr - prev;
        deltas.push(delta);
    }

    let first_delta = deltas[0];
    result.extend_from_slice(&first_delta.to_le_bytes());

    // Compute second-order deltas (double deltas) in i64
    let mut double_deltas: Vec<i64> = Vec::with_capacity(deltas.len() - 1);
    for i in 1..deltas.len() {
        let dd = deltas[i] - deltas[i - 1];
        double_deltas.push(dd);
    }

    if double_deltas.is_empty() {
        result.push(0);
        return Ok(result);
    }

    let max_abs = double_deltas
        .iter()
        .map(|&dd| dd.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_abs == 0 {
        1
    } else {
        ((64 - max_abs.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    let packed = bitpack::bitpack_i64(&double_deltas, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Core encoding logic for i32 wire format with i32 deltas (used by i32)
fn encode_double_delta_i32_wire(wire_values: &[i32]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    if wire_values.len() == 1 {
        let mut result = Vec::new();
        result.extend_from_slice(&wire_values[0].to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    let base = wire_values[0];
    result.extend_from_slice(&base.to_le_bytes());

    if wire_values.len() == 2 {
        let first_delta = wire_values[1].wrapping_sub(base);
        result.extend_from_slice(&first_delta.to_le_bytes());
        result.push(0);
        return Ok(result);
    }

    // Compute first-order deltas (i32 with wrapping)
    let mut deltas = Vec::with_capacity(wire_values.len() - 1);
    for i in 1..wire_values.len() {
        let delta = wire_values[i].wrapping_sub(wire_values[i - 1]);
        deltas.push(delta);
    }

    let first_delta = deltas[0];
    result.extend_from_slice(&first_delta.to_le_bytes());

    // Compute second-order deltas
    let mut double_deltas = Vec::with_capacity(deltas.len() - 1);
    for i in 1..deltas.len() {
        let dd = deltas[i].wrapping_sub(deltas[i - 1]);
        double_deltas.push(dd);
    }

    if double_deltas.is_empty() {
        result.push(0);
        return Ok(result);
    }

    let max_abs = double_deltas
        .iter()
        .map(|&dd| dd.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_abs == 0 {
        1
    } else {
        ((32 - max_abs.leading_zeros() as u8) + 1).min(32)
    };

    result.push(bits);

    let packed = bitpack::bitpack_i32(&double_deltas, bits)?;
    result.extend(packed);

    Ok(result)
}

/// Core encoding logic for i64 wire format
fn encode_double_delta_i64_wire(wire_values: &[i64]) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    if wire_values.len() == 1 {
        let mut result = Vec::new();
        result.extend_from_slice(&wire_values[0].to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    let base = wire_values[0];
    result.extend_from_slice(&base.to_le_bytes());

    if wire_values.len() == 2 {
        let first_delta = wire_values[1].wrapping_sub(base);
        result.extend_from_slice(&first_delta.to_le_bytes());
        result.push(0);
        return Ok(result);
    }

    // Compute first-order deltas
    let mut deltas = Vec::with_capacity(wire_values.len() - 1);
    for i in 1..wire_values.len() {
        let delta = wire_values[i].wrapping_sub(wire_values[i - 1]);
        deltas.push(delta);
    }

    let first_delta = deltas[0];
    result.extend_from_slice(&first_delta.to_le_bytes());

    // Compute second-order deltas
    let mut double_deltas = Vec::with_capacity(deltas.len() - 1);
    for i in 1..deltas.len() {
        let dd = deltas[i].wrapping_sub(deltas[i - 1]);
        double_deltas.push(dd);
    }

    if double_deltas.is_empty() {
        result.push(0);
        return Ok(result);
    }

    let max_abs = double_deltas
        .iter()
        .map(|&dd| dd.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_abs == 0 {
        1
    } else {
        ((64 - max_abs.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    let packed = bitpack::bitpack_i64(&double_deltas, bits)?;
    result.extend(packed);

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 values using double delta (raw, no headers)
///
/// # Algorithm
/// 1. Compute first-order deltas: delta[i] = value[i] - value[i-1]
/// 2. Compute second-order deltas: double_delta[i] = delta[i] - delta[i-1]
/// 3. Bit-pack the double deltas with minimal bits
///
/// # Format (raw data only, NO headers)
/// ```text
/// [base:4 bytes][first_delta:8 bytes i64][bits:1 byte][bitpacked_double_deltas...]
/// ```text
///
/// # Parameters
/// - `values`: f32 slice to encode
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_double_delta_i32_base_i64_deltas)
}

/// Encode i64 values using double delta (raw, no headers)
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_double_delta_i64_wire)
}

/// Encode i32 values using double delta (raw, no headers)
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_double_delta_i32_wire)
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for i32 base + i64 deltas (used by f32)
fn decode_double_delta_i32_base_i64_deltas(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("DoubleDelta decode: insufficient data"));
    }

    let base = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    if count == 1 {
        return Ok(vec![base]);
    }

    if data.len() < 13 {
        return Err(anyhow::anyhow!(
            "DoubleDelta decode: insufficient data for first delta"
        ));
    }

    let first_delta = i64::from_le_bytes([
        data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11],
    ]);

    if count == 2 {
        let v1_bits = (base as i64 + first_delta) as i32;
        return Ok(vec![base, v1_bits]);
    }

    let bits = data[12];

    if bits == 0 {
        let mut result = Vec::with_capacity(count);
        result.push(base);

        let mut current = base as i64 + first_delta;
        result.push(current as i32);

        for _ in 2..count {
            current += first_delta;
            result.push(current as i32);
        }

        return Ok(result);
    }

    let num_double_deltas = count - 2;
    let double_deltas = bitpack::unbitpack_i64(&data[13..], bits, num_double_deltas)?;

    let mut result = Vec::with_capacity(count);
    result.push(base);

    let mut prev_value = base as i64;
    let mut prev_delta = first_delta;

    let second_value = prev_value + first_delta;
    result.push(second_value as i32);
    prev_value = second_value;

    for &dd in &double_deltas {
        let delta = prev_delta + dd;
        let value = prev_value + delta;
        result.push(value as i32);
        prev_value = value;
        prev_delta = delta;
    }

    Ok(result)
}

/// Core decoding logic for i32 wire format with i32 deltas
fn decode_double_delta_i32_wire(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("DoubleDelta decode: insufficient data"));
    }

    let base = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    if count == 1 {
        return Ok(vec![base]);
    }

    if data.len() < 9 {
        return Err(anyhow::anyhow!(
            "DoubleDelta decode: insufficient data for first delta"
        ));
    }

    let first_delta = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);

    if count == 2 {
        return Ok(vec![base, base.wrapping_add(first_delta)]);
    }

    let bits = data[8];

    if bits == 0 {
        let mut result = Vec::with_capacity(count);
        result.push(base);

        let mut current = base.wrapping_add(first_delta);
        result.push(current);

        for _ in 2..count {
            current = current.wrapping_add(first_delta);
            result.push(current);
        }

        return Ok(result);
    }

    let num_double_deltas = count - 2;
    let double_deltas = bitpack::unbitpack_i32(&data[9..], bits, num_double_deltas)?;

    let mut result = Vec::with_capacity(count);
    result.push(base);

    let mut prev_value = base;
    let mut prev_delta = first_delta;

    let second_value = base.wrapping_add(first_delta);
    result.push(second_value);
    prev_value = second_value;

    for &dd in &double_deltas {
        let delta = prev_delta.wrapping_add(dd);
        let value = prev_value.wrapping_add(delta);
        result.push(value);
        prev_value = value;
        prev_delta = delta;
    }

    Ok(result)
}

/// Core decoding logic for i64 wire format
fn decode_double_delta_i64_wire(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 8 {
        return Err(anyhow::anyhow!("DoubleDelta decode: insufficient data"));
    }

    let base = i64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);

    if count == 1 {
        return Ok(vec![base]);
    }

    if data.len() < 17 {
        return Err(anyhow::anyhow!(
            "DoubleDelta decode: insufficient data for first delta"
        ));
    }

    let first_delta = i64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);

    if count == 2 {
        return Ok(vec![base, base.wrapping_add(first_delta)]);
    }

    let bits = data[16];

    if bits == 0 {
        let mut result = Vec::with_capacity(count);
        result.push(base);

        let mut current = base.wrapping_add(first_delta);
        result.push(current);

        for _ in 2..count {
            current = current.wrapping_add(first_delta);
            result.push(current);
        }

        return Ok(result);
    }

    let num_double_deltas = count - 2;
    let double_deltas = bitpack::unbitpack_i64(&data[17..], bits, num_double_deltas)?;

    let mut result = Vec::with_capacity(count);
    result.push(base);

    let mut prev_value = base;
    let mut prev_delta = first_delta;

    let second_value = base.wrapping_add(first_delta);
    result.push(second_value);
    prev_value = second_value;

    for &dd in &double_deltas {
        let delta = prev_delta.wrapping_add(dd);
        let value = prev_value.wrapping_add(delta);
        result.push(value);
        prev_value = value;
        prev_delta = delta;
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Decode f32 values from double delta encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    helpers::decode_generic::<f32>(data, count, decode_double_delta_i32_base_i64_deltas)
}

/// Decode i64 values from double delta encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    helpers::decode_generic::<i64>(data, count, decode_double_delta_i64_wire)
}

/// Decode i32 values from double delta encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    helpers::decode_generic::<i32>(data, count, decode_double_delta_i32_wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_delta_linear() {
        // Linear sequence: constant delta, zero double delta (32+ values)
        let values: Vec<i32> = (0..32).map(|i| 100 + i * 10).collect();

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Should be small: base + first_delta + 1 bit for zero double deltas
        assert!(encoded.len() < 30, "Should compress linear data well");
    }

    #[test]
    fn test_double_delta_quadratic() {
        // Quadratic sequence: linear delta, constant double delta (32+ values)
        // values: 0, 1, 4, 9, 16, 25... (squares)
        // deltas: 1, 3, 5, 7, 9... (odd numbers)
        // double deltas: 2, 2, 2, 2... (constant)
        let values: Vec<i32> = (0..32).map(|i| i * i).collect();

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_double_delta_timestamps() {
        // Simulated timestamps with constant rate
        let mut values = Vec::new();
        let mut ts = 1000000i64;
        for _ in 0..100 {
            values.push(ts);
            ts += 1000; // Constant increment
        }

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Should compress very well (constant delta)
        let original_size = values.len() * 8;
        assert!(
            encoded.len() < original_size / 10,
            "Should compress constant-rate timestamps well: {} vs {}",
            encoded.len(),
            original_size
        );
    }

    #[test]
    fn test_double_delta_f32_roundtrip() {
        // Use values with consistent bit-pattern deltas (32+ values)
        let values: Vec<f32> = (0..32).map(|i| 100.0 + i as f32 * 0.5).collect();

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        // DoubleDelta on f32 may have minor precision loss due to bit-level delta encoding
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            let diff = (orig - dec).abs();
            assert!(diff < 0.01, "Expected {}, got {}", orig, dec);
        }
    }

    #[test]
    fn test_double_delta_i64_roundtrip() {
        let values: Vec<i64> = (0..32).map(|i| 1000 + i * 10).collect();

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_double_delta_empty() {
        let values: Vec<i32> = vec![];
        let encoded = encode_i32(&values).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_i32(&encoded, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_double_delta_single_value() {
        let values = vec![42i32];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_double_delta_two_values() {
        let values = vec![10i32, 20];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_double_delta_random() {
        // Random data - worst case for double delta (32+ values)
        let values: Vec<i32> = vec![
            100, 50, 200, 75, 150, 25, 180, 90, 120, 160, 80, 140, 60, 190, 110, 130, 170, 40, 210,
            95, 125, 155, 85, 145, 65, 185, 105, 135, 165, 45, 195, 115,
        ];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_double_delta_sensor_data() {
        // Simulated sensor data with slight variations
        let mut values = Vec::new();
        let mut temp = 20.0f32;
        for i in 0..100 {
            temp += 0.1 + (i as f32 * 0.01); // Gradual increase with acceleration
            values.push(temp);
        }

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            let diff = (orig - dec).abs();
            assert!(diff < 0.1, "Expected {}, got {}", orig, dec);
        }
    }

    #[test]
    fn test_double_delta_constant() {
        // All same value - ultimate compression
        let values = vec![42i32; 100];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);

        // Constant data: base (4) + first_delta=0 (4) + bits (1) = 9 bytes minimum
        // Actual may be slightly more due to implementation details
        assert!(encoded.len() < 30, "Encoded {} bytes", encoded.len());
    }

    // ===== OVERFLOW EDGE CASE TESTS =====
    // These tests verify that the i64 delta fix prevents overflow

    #[test]
    fn test_overflow_i32_extremes() {
        // Test with i32::MAX and i32::MIN - would overflow with i32 deltas
        // Delta between i32::MAX and i32::MIN is 2^32, which exceeds i32 range
        let values = vec![i32::MIN, i32::MAX, i32::MIN, i32::MAX];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to roundtrip i32 extremes");
    }

    #[test]
    fn test_overflow_f32_extreme_bit_patterns() {
        // Test f32 values with extreme bit patterns that would cause i32 delta overflow
        let values = vec![
            f32::from_bits(i32::MAX as u32), // Positive extreme
            f32::from_bits(i32::MIN as u32), // Negative extreme
            f32::from_bits(0),               // Zero
            f32::from_bits(i32::MAX as u32), // Back to positive
        ];

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(
                orig.to_bits(),
                dec.to_bits(),
                "Failed to roundtrip extreme f32 bit pattern: orig={:08x}, dec={:08x}",
                orig.to_bits(),
                dec.to_bits()
            );
        }
    }

    #[test]
    fn test_overflow_maximum_delta_sequence() {
        // Create a sequence where consecutive deltas are at i32 boundary
        // This would definitely overflow with i32 delta computation
        let values = vec![
            0i32,
            i32::MAX / 2,
            i32::MAX,
            i32::MAX / 2,
            0,
            i32::MIN / 2,
            i32::MIN,
        ];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to handle large delta transitions");
    }

    #[test]
    fn test_overflow_f32_infinity_nan() {
        // Test special f32 values with extreme bit patterns
        let values = vec![
            0.0f32,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            f32::MAX,
            f32::MIN,
        ];

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            // Compare bit patterns since NAN != NAN
            assert_eq!(
                orig.to_bits(),
                dec.to_bits(),
                "Failed to roundtrip special f32 value: orig={:08x}, dec={:08x}",
                orig.to_bits(),
                dec.to_bits()
            );
        }
    }

    #[test]
    fn test_overflow_i64_full_range() {
        // Test i64 values across full range
        let values = vec![i64::MIN, i64::MIN / 2, 0i64, i64::MAX / 2, i64::MAX, 0];

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to handle i64 extremes");
    }

    #[test]
    fn test_overflow_alternating_extremes() {
        // Worst case: alternating between extremes
        // Each delta is maximum possible value
        let values = vec![
            i32::MIN,
            i32::MAX,
            i32::MIN,
            i32::MAX,
            i32::MIN,
            i32::MAX,
            i32::MIN,
            i32::MAX,
        ];

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded, "Failed to handle alternating extremes");
    }
}
