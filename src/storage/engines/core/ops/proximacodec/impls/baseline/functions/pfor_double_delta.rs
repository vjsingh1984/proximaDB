// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Patched Frame of Reference Double Delta (PForDoubleDelta) - Baseline (pure scalar) implementation
//!
//! **ARCHITECTURE NOTE**: This is the BASELINE implementation.
//! - NO SIMD intrinsics allowed
//! - NO GPU code
//! - Pure portable Rust only
//!
//! For SIMD acceleration, see: `src/storage/engines/core/ops/proximacodec/simd.rs`
//! For GPU acceleration, see: `src/storage/engines/core/ops/proximacodec/impls/gpu/`
//!
//! Double Delta Encoding: Computes deltas of deltas for superior compression on smooth/linear data.
//! Best for: Linearly increasing values, temporal sequences, transposed sorted dimensions.
//!
//! Algorithm:
//! 1. f32 → i32 bit pattern (lossless)
//! 2. Compute first delta from base
//! 3. Compute second delta (delta of deltas)
//! 4. Compress double deltas with PFor (90% threshold for outliers)
//!
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

// ===== Bitpacking delegation to shared helpers =====
//
// All bitpacking operations now use the shared helpers in bitpack.rs
// to avoid code duplication and ensure consistent sign extension behavior.

use super::bitpack;
use super::helpers;

// ===== Core wire format encoding functions =====

/// Core encoding logic for i32 base + i64 deltas (used by f32 and i32)
fn encode_pfor_double_delta_i32_base(wire_values: &[i32], base: i32) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    if wire_values.len() == 1 {
        let mut result = Vec::new();
        result.extend_from_slice(&base.to_le_bytes());
        let delta = (wire_values[0] as i64) - (base as i64);
        result.extend_from_slice(&delta.to_le_bytes());
        result.push(0);
        result.extend_from_slice(&0u32.to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    result.extend_from_slice(&base.to_le_bytes());

    // Compute first deltas in i64 (NO OVERFLOW!)
    let first_deltas: Vec<i64> = wire_values
        .iter()
        .map(|&v| (v as i64) - (base as i64))
        .collect();

    result.extend_from_slice(&first_deltas[0].to_le_bytes());

    // Compute second deltas (double deltas) in i64
    let mut double_deltas: Vec<i64> = Vec::with_capacity(first_deltas.len() - 1);
    for i in 1..first_deltas.len() {
        let dd = first_deltas[i] - first_deltas[i - 1];
        double_deltas.push(dd);
    }

    // Find optimal bit width for 90% of double deltas
    let mut sorted_double_deltas: Vec<u64> =
        double_deltas.iter().map(|&d| d.unsigned_abs()).collect();
    sorted_double_deltas.sort_unstable();

    let percentile_90_idx = if sorted_double_deltas.len() > 1 {
        (sorted_double_deltas.len() * 90) / 100
    } else {
        0
    };

    let threshold = sorted_double_deltas
        .get(percentile_90_idx)
        .copied()
        .unwrap_or(0);
    let bits = if threshold == 0 {
        1
    } else {
        ((64 - threshold.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    // Separate regular double deltas and patches
    let mut regular_values: Vec<i64> = Vec::with_capacity(double_deltas.len());
    let mut patches: Vec<(u32, i64)> = Vec::new();
    let max_regular = if bits < 64 {
        (1u64 << (bits - 1)) - 1
    } else {
        i64::MAX as u64
    };

    for (idx, &dd) in double_deltas.iter().enumerate() {
        let abs_dd = dd.unsigned_abs();
        if abs_dd <= max_regular {
            regular_values.push(dd);
        } else {
            regular_values.push(0);
            patches.push((idx as u32, dd));
        }
    }

    let num_patches = patches.len() as u32;
    result.extend_from_slice(&num_patches.to_le_bytes());

    // Bitpack regular double deltas
    let packed = bitpack::bitpack_i64(&regular_values, bits)?;
    result.extend(packed);

    // Store patches
    for (pos, value) in patches {
        result.extend_from_slice(&pos.to_le_bytes());
        result.extend_from_slice(&value.to_le_bytes());
    }

    Ok(result)
}

/// Core encoding logic for i64 wire format
fn encode_pfor_double_delta_i64_wire(wire_values: &[i64], base: i64) -> Result<Vec<u8>> {
    if wire_values.is_empty() {
        return Ok(Vec::new());
    }

    if wire_values.len() == 1 {
        let mut result = Vec::new();
        result.extend_from_slice(&base.to_le_bytes());
        let delta = wire_values[0].wrapping_sub(base);
        result.extend_from_slice(&delta.to_le_bytes());
        result.push(0);
        result.extend_from_slice(&0u32.to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    result.extend_from_slice(&base.to_le_bytes());

    // Compute first deltas
    let first_deltas: Vec<i64> = wire_values.iter().map(|&v| v.wrapping_sub(base)).collect();

    result.extend_from_slice(&first_deltas[0].to_le_bytes());

    // Compute second deltas
    let mut double_deltas: Vec<i64> = Vec::with_capacity(first_deltas.len() - 1);
    for i in 1..first_deltas.len() {
        let dd = first_deltas[i].wrapping_sub(first_deltas[i - 1]);
        double_deltas.push(dd);
    }

    // Find optimal bit width for 90% of double deltas
    let mut sorted_double_deltas: Vec<u64> =
        double_deltas.iter().map(|&d| d.unsigned_abs()).collect();
    sorted_double_deltas.sort_unstable();

    let percentile_90_idx = if sorted_double_deltas.len() > 1 {
        (sorted_double_deltas.len() * 90) / 100
    } else {
        0
    };

    let threshold = sorted_double_deltas
        .get(percentile_90_idx)
        .copied()
        .unwrap_or(0);
    let bits = if threshold == 0 {
        1
    } else {
        ((64 - threshold.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    // Separate regular double deltas and patches
    let mut regular_values: Vec<i64> = Vec::with_capacity(double_deltas.len());
    let mut patches: Vec<(u32, i64)> = Vec::new();
    let max_regular = if bits < 64 {
        (1u64 << (bits - 1)) - 1
    } else {
        i64::MAX as u64
    };

    for (idx, &dd) in double_deltas.iter().enumerate() {
        let abs_dd = dd.unsigned_abs();
        if abs_dd <= max_regular {
            regular_values.push(dd);
        } else {
            regular_values.push(0);
            patches.push((idx as u32, dd));
        }
    }

    let num_patches = patches.len() as u32;
    result.extend_from_slice(&num_patches.to_le_bytes());

    // Bitpack regular double deltas
    let packed = bitpack::bitpack_i64(&regular_values, bits)?;
    result.extend(packed);

    // Store patches
    for (pos, value) in patches {
        result.extend_from_slice(&pos.to_le_bytes());
        result.extend_from_slice(&value.to_le_bytes());
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 values using PForDoubleDelta (raw, no headers)
///
/// # Algorithm
/// 1. Convert f32 to i32 bits (lossless)
/// 2. Compute first deltas from base
/// 3. Compute second deltas (delta of deltas)
/// 4. Identify outliers in double deltas (values requiring more bits than 90th percentile)
/// 5. Store regular double deltas with bitpacking
/// 6. Store outliers as patches with their positions
///
/// # Format (raw data only, NO headers)
/// ```text
/// [base:4 bytes][first_delta:4 bytes][bits:1 byte][num_patches:4 bytes]
/// [bitpacked_double_deltas...][patch_count:4][patches:(pos:4, value:4)*]
/// ```text
///
/// # Parameters
/// - `values`: f32 slice to encode
/// - `base`: Base value for first frame of reference (i64 for compatibility)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
///
/// # Example
/// ```text
/// // Linear sequence: [0.1, 0.2, 0.3, 0.4, 0.5]
/// // First deltas: [0, Δ1, Δ2, Δ3, Δ4]
/// // Second deltas: [0, Δ1, (Δ2-Δ1), (Δ3-Δ2), (Δ4-Δ3)]
/// // If linear: second deltas are constant → excellent compression!
/// ```text
pub fn encode_f32(values: &[f32], base: i64) -> Result<Vec<u8>> {
    helpers::encode_generic(values, |wire_values| {
        encode_pfor_double_delta_i32_base(wire_values, base as i32)
    })
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for i32 base + i64 deltas (used by f32 and i32)
fn decode_pfor_double_delta_i32_base(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 17 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data"));
    }

    let base = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    if count == 1 {
        let delta = i64::from_le_bytes([
            data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11],
        ]);
        return Ok(vec![((base as i64) + delta) as i32]);
    }

    let first_delta = i64::from_le_bytes([
        data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11],
    ]);

    let bits = data[12];
    let num_patches = u32::from_le_bytes([data[13], data[14], data[15], data[16]]) as usize;

    let double_delta_count = count - 1;
    let bitpacked_bytes = ((double_delta_count * bits as usize) + 7) / 8;

    if data.len() < 17 + bitpacked_bytes + num_patches * 12 {
        return Err(anyhow::anyhow!(
            "PForDoubleDelta decode: insufficient data for patches"
        ));
    }

    let bitpacked_data = &data[17..17 + bitpacked_bytes];
    let mut double_deltas = bitpack::unbitpack_i64(&bitpacked_data, bits, double_delta_count)?;

    let patch_start = 17 + bitpacked_bytes;
    for i in 0..num_patches {
        let offset = patch_start + i * 12;
        let pos = u32::from_le_bytes([
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

        if pos < double_deltas.len() {
            double_deltas[pos] = value;
        }
    }

    // Reconstruct values by accumulating first deltas
    let mut result = Vec::with_capacity(count);
    let mut first_delta_accumulator = first_delta;

    // First value = base + first_delta
    result.push((base as i64).wrapping_add(first_delta_accumulator) as i32);

    // Remaining values
    for &dd in &double_deltas {
        first_delta_accumulator = first_delta_accumulator.wrapping_add(dd);
        result.push((base as i64).wrapping_add(first_delta_accumulator) as i32);
    }

    Ok(result)
}

/// Core decoding logic for i64 wire format
fn decode_pfor_double_delta_i64_wire(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 17 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data"));
    }

    let base = i64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);

    if count == 1 {
        let delta = i64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        return Ok(vec![base.wrapping_add(delta)]);
    }

    let first_delta = i64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);

    let bits = data[16];
    let num_patches = u32::from_le_bytes([data[17], data[18], data[19], data[20]]) as usize;

    let double_delta_count = count - 1;
    let bitpacked_bytes = ((double_delta_count * bits as usize) + 7) / 8;

    if data.len() < 21 + bitpacked_bytes + num_patches * 12 {
        return Err(anyhow::anyhow!(
            "PForDoubleDelta decode: insufficient data for patches"
        ));
    }

    let bitpacked_data = &data[21..21 + bitpacked_bytes];
    let mut double_deltas = bitpack::unbitpack_i64(&bitpacked_data, bits, double_delta_count)?;

    let patch_start = 21 + bitpacked_bytes;
    for i in 0..num_patches {
        let offset = patch_start + i * 12;
        let pos = u32::from_le_bytes([
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

        if pos < double_deltas.len() {
            double_deltas[pos] = value;
        }
    }

    // Reconstruct values by accumulating first deltas
    let mut result = Vec::with_capacity(count);
    let mut first_delta_accumulator = first_delta;

    // First value = base + first_delta
    result.push(base.wrapping_add(first_delta_accumulator));

    // Remaining values
    for &dd in &double_deltas {
        first_delta_accumulator = first_delta_accumulator.wrapping_add(dd);
        result.push(base.wrapping_add(first_delta_accumulator));
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Decode f32 values from PForDoubleDelta encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    helpers::decode_generic::<f32>(data, count, decode_pfor_double_delta_i32_base)
}

/// Encode i64 values using PForDoubleDelta (raw, no headers)
pub fn encode_i64(values: &[i64], base: i64) -> Result<Vec<u8>> {
    helpers::encode_generic(values, |wire_values| {
        encode_pfor_double_delta_i64_wire(wire_values, base)
    })
}

/// Decode i64 values from PForDoubleDelta encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    helpers::decode_generic::<i64>(data, count, decode_pfor_double_delta_i64_wire)
}

/// Encode i32 values using PForDoubleDelta (raw, no headers)
pub fn encode_i32(values: &[i32], base: i64) -> Result<Vec<u8>> {
    helpers::encode_generic(values, |wire_values| {
        encode_pfor_double_delta_i32_base(wire_values, base as i32)
    })
}

/// Decode i32 values from PForDoubleDelta encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    helpers::decode_generic::<i32>(data, count, decode_pfor_double_delta_i32_base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pfor_double_delta_linear_sequence() {
        // Perfect case: linear sequence (constant second deltas)
        let values = vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];

        let encoded = encode_f32(&values, 0).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(
                orig.to_bits(),
                dec.to_bits(),
                "Mismatch: {} != {}",
                orig,
                dec
            );
        }

        println!("Linear sequence compression:");
        println!("  Original: {} bytes", values.len() * 4);
        println!("  Encoded:  {} bytes", encoded.len());
        println!(
            "  Ratio:    {:.1}%",
            (encoded.len() as f64 / (values.len() * 4) as f64) * 100.0
        );
    }

    #[test]
    fn test_pfor_double_delta_smooth_embeddings() {
        // Normalized embedding-like values (smooth)
        let values: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();

        let encoded = encode_f32(&values, 0).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits());
        }

        println!("Smooth embeddings compression:");
        println!("  Original: {} bytes", values.len() * 4);
        println!("  Encoded:  {} bytes", encoded.len());
        println!(
            "  Ratio:    {:.1}%",
            (encoded.len() as f64 / (values.len() * 4) as f64) * 100.0
        );
    }

    #[test]
    fn test_pfor_double_delta_with_outliers() {
        // Mostly linear with some outliers
        let mut values: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();
        values[50] = 5.0; // Outlier
        values[75] = -2.0; // Outlier

        let encoded = encode_f32(&values, 0).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits());
        }

        println!("With outliers compression:");
        println!("  Original: {} bytes", values.len() * 4);
        println!("  Encoded:  {} bytes", encoded.len());
        println!(
            "  Ratio:    {:.1}%",
            (encoded.len() as f64 / (values.len() * 4) as f64) * 100.0
        );
    }

    #[test]
    fn test_pfor_double_delta_i64() {
        let values: Vec<i64> = (0..100).collect();

        let encoded = encode_i64(&values, 0).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_pfor_double_delta_single_value() {
        let values = vec![42.0f32];

        let encoded = encode_f32(&values, 0).unwrap();
        let decoded = decode_f32(&encoded, 1).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_pfor_double_delta_empty() {
        let values: Vec<f32> = vec![];

        let encoded = encode_f32(&values, 0).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_f32(&encoded, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_pfor_double_delta_constant_values() {
        // All same value → all deltas = 0 → all double deltas = 0
        let values = vec![0.5f32; 100];

        let encoded = encode_f32(&values, 0).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits());
        }

        // Should be extremely compressed (all double deltas = 0)
        println!("Constant values compression:");
        println!("  Original: {} bytes", values.len() * 4);
        println!("  Encoded:  {} bytes", encoded.len());
        assert!(encoded.len() < 100, "Should compress constant data well");
    }
}
