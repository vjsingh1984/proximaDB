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
/// ```
/// [base:4 bytes][first_delta:4 bytes][bits:1 byte][num_patches:4 bytes]
/// [bitpacked_double_deltas...][patch_count:4][patches:(pos:4, value:4)*]
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode
/// - `base`: Base value for first frame of reference (i64 for compatibility)
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
///
/// # Example
/// ```
/// // Linear sequence: [0.1, 0.2, 0.3, 0.4, 0.5]
/// // First deltas: [0, Δ1, Δ2, Δ3, Δ4]
/// // Second deltas: [0, Δ1, (Δ2-Δ1), (Δ3-Δ2), (Δ4-Δ3)]
/// // If linear: second deltas are constant → excellent compression!
/// ```
pub fn encode_f32(values: &[f32], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    if values.len() == 1 {
        // Single value: just store it directly
        let mut result = Vec::new();
        let base_i32 = base as i32;
        result.extend_from_slice(&base_i32.to_le_bytes());

        // Use i64 arithmetic to avoid overflow
        let v_bits = values[0].to_bits() as i32 as i64;
        let base_i64 = base_i32 as i64;
        let delta = v_bits - base_i64;
        result.extend_from_slice(&delta.to_le_bytes());  // Store as i64 (8 bytes)
        result.push(0); // bits
        result.extend_from_slice(&0u32.to_le_bytes()); // num_patches
        return Ok(result);
    }

    let mut result = Vec::new();
    let base_i32 = base as i32;
    result.extend_from_slice(&base_i32.to_le_bytes());

    // Step 1: Convert f32 to i32 bit patterns (pure scalar)
    let bits: Vec<i32> = values
        .iter()
        .map(|&v| v.to_bits() as i32)
        .collect();

    // Step 2: Compute first deltas from base in i64 (NO OVERFLOW!)
    let first_deltas: Vec<i64> = bits
        .iter()
        .map(|&b| (b as i64) - (base_i32 as i64))
        .collect();

    // Store first delta as i64 (8 bytes)
    result.extend_from_slice(&first_deltas[0].to_le_bytes());

    // Step 3: Compute second deltas (delta of deltas) in i64
    let mut double_deltas: Vec<i64> = Vec::with_capacity(first_deltas.len() - 1);
    for i in 1..first_deltas.len() {
        let dd = first_deltas[i] - first_deltas[i - 1];  // i64 arithmetic - no overflow!
        double_deltas.push(dd);
    }

    // Step 4: Find optimal bit width for 90% of double deltas (outliers will be patched)
    let mut sorted_double_deltas: Vec<u64> = double_deltas
        .iter()
        .map(|&d| d.abs() as u64)  // Use abs() instead of unsigned_abs() for i64
        .collect();
    sorted_double_deltas.sort_unstable();

    let percentile_90_idx = if sorted_double_deltas.len() > 1 {
        (sorted_double_deltas.len() * 90) / 100
    } else {
        0
    };

    let threshold = sorted_double_deltas.get(percentile_90_idx).copied().unwrap_or(0);
    let bits = if threshold == 0 {
        1
    } else {
        // Can go up to 64 bits for worst case
        ((64 - threshold.leading_zeros() as u8) + 1).min(64)
    };

    result.push(bits);

    // Step 5: Separate regular double deltas and patches
    let mut regular_values: Vec<i64> = Vec::with_capacity(double_deltas.len());
    let mut patches: Vec<(u32, i64)> = Vec::new();
    let max_regular = if bits < 64 {
        (1u64 << (bits - 1)) - 1  // Account for sign bit
    } else {
        i64::MAX as u64
    };

    for (idx, &dd) in double_deltas.iter().enumerate() {
        let abs_dd = dd.abs() as u64;
        if abs_dd <= max_regular {
            regular_values.push(dd);
        } else {
            // Store sentinel and mark for patching
            regular_values.push(0);
            patches.push((idx as u32, dd));
        }
    }

    // Store number of patches
    let num_patches = patches.len() as u32;
    result.extend_from_slice(&num_patches.to_le_bytes());

    // Step 6: Bitpack regular double deltas (now i64)
    let packed = bitpack::bitpack_i64(&regular_values, bits)?;
    result.extend(packed);

    // Step 7: Store patches (position:4 bytes + value:8 bytes)
    for (pos, value) in patches {
        result.extend_from_slice(&pos.to_le_bytes());      // 4 bytes
        result.extend_from_slice(&value.to_le_bytes());    // 8 bytes
    }

    Ok(result)
}

/// Decode f32 values from PForDoubleDelta encoded data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 17 {  // 4 (base) + 8 (first_delta i64) + 1 (bits) + 4 (num_patches)
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data"));
    }

    // Read base (4 bytes)
    let base_i32 = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    if count == 1 {
        // Single value case - read i64 delta
        let delta = i64::from_le_bytes([
            data[4], data[5], data[6], data[7],
            data[8], data[9], data[10], data[11],
        ]);
        let value_bits = ((base_i32 as i64) + delta) as i32 as u32;
        return Ok(vec![f32::from_bits(value_bits)]);
    }

    // Read first delta (8 bytes - now i64!)
    let first_delta = i64::from_le_bytes([
        data[4], data[5], data[6], data[7],
        data[8], data[9], data[10], data[11],
    ]);

    // Read bit width (1 byte)
    let bits = data[12];

    // Read number of patches (4 bytes)
    let num_patches = u32::from_le_bytes([data[13], data[14], data[15], data[16]]) as usize;

    // Calculate size of bitpacked data (count-1 double deltas)
    let double_delta_count = count - 1;
    let bitpacked_bytes = ((double_delta_count * bits as usize) + 7) / 8;

    // Patches are now 12 bytes each: 4 (pos) + 8 (value i64)
    if data.len() < 17 + bitpacked_bytes + num_patches * 12 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data for patches"));
    }

    // Unpack double deltas (now i64!)
    let bitpacked_data = &data[17..17 + bitpacked_bytes];
    let mut double_deltas = bitpack::unbitpack_i64_unsigned(bitpacked_data, bits, double_delta_count)?;

    // Apply patches to double deltas (patches are now i64)
    let patch_start = 17 + bitpacked_bytes;
    for i in 0..num_patches {
        let offset = patch_start + i * 12;  // 12 bytes per patch
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

    // Reconstruct first deltas from double deltas (i64 arithmetic - NO OVERFLOW!)
    let mut first_deltas: Vec<i64> = Vec::with_capacity(count);
    first_deltas.push(first_delta);

    for dd in double_deltas {
        let prev_delta = *first_deltas.last().unwrap();
        first_deltas.push(prev_delta + dd);  // i64 arithmetic - no overflow!
    }

    // Reconstruct f32 values from first deltas (i64 arithmetic - NO OVERFLOW!)
    let result: Vec<f32> = first_deltas
        .iter()
        .map(|&delta| {
            let value_bits = ((base_i32 as i64) + delta) as i32 as u32;
            f32::from_bits(value_bits)
        })
        .collect();

    Ok(result)
}

/// Encode i64 values using PForDoubleDelta (raw, no headers)
pub fn encode_i64(values: &[i64], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    if values.len() == 1 {
        let mut result = Vec::new();
        result.extend_from_slice(&base.to_le_bytes());
        let delta = values[0].wrapping_sub(base);
        result.extend_from_slice(&delta.to_le_bytes());
        result.push(0);
        result.extend_from_slice(&0u32.to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    result.extend_from_slice(&base.to_le_bytes());

    // First deltas
    let first_deltas: Vec<i64> = values
        .iter()
        .map(|&v| v.wrapping_sub(base))
        .collect();

    result.extend_from_slice(&first_deltas[0].to_le_bytes());

    // Second deltas (double deltas)
    let mut double_deltas = Vec::with_capacity(first_deltas.len() - 1);
    for i in 1..first_deltas.len() {
        double_deltas.push(first_deltas[i].wrapping_sub(first_deltas[i - 1]));
    }

    // Find optimal bit width for 90% of double deltas
    let mut sorted_double_deltas: Vec<u64> = double_deltas
        .iter()
        .map(|&d| d.unsigned_abs())
        .collect();
    sorted_double_deltas.sort_unstable();

    let percentile_90_idx = if sorted_double_deltas.len() > 1 {
        (sorted_double_deltas.len() * 90) / 100
    } else {
        0
    };

    let threshold = sorted_double_deltas.get(percentile_90_idx).copied().unwrap_or(0);
    let bits = if threshold == 0 {
        1
    } else {
        64 - threshold.leading_zeros() as u8
    };

    result.push(bits);

    // Separate regular values and patches
    let mut regular_values = Vec::with_capacity(double_deltas.len());
    let mut patches = Vec::new();
    let max_regular = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
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

/// Decode i64 values from PForDoubleDelta encoded data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 17 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data"));
    }

    let base = i64::from_le_bytes([
        data[0], data[1], data[2], data[3],
        data[4], data[5], data[6], data[7],
    ]);

    if count == 1 {
        let delta = i64::from_le_bytes([
            data[8], data[9], data[10], data[11],
            data[12], data[13], data[14], data[15],
        ]);
        return Ok(vec![base.wrapping_add(delta)]);
    }

    let first_delta = i64::from_le_bytes([
        data[8], data[9], data[10], data[11],
        data[12], data[13], data[14], data[15],
    ]);

    let bits = data[16];
    let num_patches = u32::from_le_bytes([data[17], data[18], data[19], data[20]]) as usize;

    let double_delta_count = count - 1;
    let bitpacked_bytes = ((double_delta_count * bits as usize) + 7) / 8;

    if data.len() < 21 + bitpacked_bytes + num_patches * 12 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data for patches"));
    }

    let bitpacked_data = &data[21..21 + bitpacked_bytes];
    let mut double_deltas = bitpack::unbitpack_i64_unsigned(bitpacked_data, bits, double_delta_count)?;

    // Apply patches
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

    // Reconstruct first deltas
    let mut first_deltas = Vec::with_capacity(count);
    first_deltas.push(first_delta);
    for dd in double_deltas {
        let prev = *first_deltas.last().unwrap();
        first_deltas.push(prev.wrapping_add(dd));
    }

    // Reconstruct original values
    let result = first_deltas
        .iter()
        .map(|&delta| base.wrapping_add(delta))
        .collect();

    Ok(result)
}

/// Encode i32 values using PForDoubleDelta (raw, no headers)
pub fn encode_i32(values: &[i32], base: i64) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    if values.len() == 1 {
        let mut result = Vec::new();
        let base_i32 = base as i32;
        result.extend_from_slice(&base_i32.to_le_bytes());
        let delta = values[0].wrapping_sub(base_i32);
        result.extend_from_slice(&delta.to_le_bytes());
        result.push(0);
        result.extend_from_slice(&0u32.to_le_bytes());
        return Ok(result);
    }

    let mut result = Vec::new();
    let base_i32 = base as i32;
    result.extend_from_slice(&base_i32.to_le_bytes());

    // First deltas
    let first_deltas: Vec<i32> = values
        .iter()
        .map(|&v| v.wrapping_sub(base_i32))
        .collect();

    result.extend_from_slice(&first_deltas[0].to_le_bytes());

    // Double deltas
    let mut double_deltas = Vec::with_capacity(first_deltas.len() - 1);
    for i in 1..first_deltas.len() {
        double_deltas.push(first_deltas[i].wrapping_sub(first_deltas[i - 1]));
    }

    // Find optimal bit width for 90% of double deltas
    let mut sorted_double_deltas: Vec<u32> = double_deltas
        .iter()
        .map(|&d| d.unsigned_abs())
        .collect();
    sorted_double_deltas.sort_unstable();

    let percentile_90_idx = if sorted_double_deltas.len() > 1 {
        (sorted_double_deltas.len() * 90) / 100
    } else {
        0
    };

    let threshold = sorted_double_deltas.get(percentile_90_idx).copied().unwrap_or(0);
    let bits = if threshold == 0 {
        1
    } else {
        32 - threshold.leading_zeros() as u8
    };

    result.push(bits);

    // Separate regular values and patches
    let mut regular_values = Vec::with_capacity(double_deltas.len());
    let mut patches = Vec::new();
    let max_regular = (1u64 << bits) - 1;

    for (idx, &dd) in double_deltas.iter().enumerate() {
        let abs_dd = dd.unsigned_abs() as u64;
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
    let packed = bitpack::bitpack_i32(&regular_values, bits)?;
    result.extend(packed);

    // Store patches
    for (pos, value) in patches {
        result.extend_from_slice(&pos.to_le_bytes());
        result.extend_from_slice(&value.to_le_bytes());
    }

    Ok(result)
}

/// Decode i32 values from PForDoubleDelta encoded data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 13 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data"));
    }

    let base = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);

    if count == 1 {
        let delta = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        return Ok(vec![base.wrapping_add(delta)]);
    }

    let first_delta = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let bits = data[8];
    let num_patches = u32::from_le_bytes([data[9], data[10], data[11], data[12]]) as usize;

    let double_delta_count = count - 1;
    let bitpacked_bytes = ((double_delta_count * bits as usize) + 7) / 8;

    if data.len() < 13 + bitpacked_bytes + num_patches * 8 {
        return Err(anyhow::anyhow!("PForDoubleDelta decode: insufficient data for patches"));
    }

    let bitpacked_data = &data[13..13 + bitpacked_bytes];
    let mut double_deltas = bitpack::unbitpack_i32_unsigned(bitpacked_data, bits, double_delta_count)?;

    // Apply patches
    let patch_start = 13 + bitpacked_bytes;
    for i in 0..num_patches {
        let offset = patch_start + i * 8;
        let pos = u32::from_le_bytes([
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

        if pos < double_deltas.len() {
            double_deltas[pos] = value;
        }
    }

    // Reconstruct first deltas
    let mut first_deltas = Vec::with_capacity(count);
    first_deltas.push(first_delta);
    for dd in double_deltas {
        let prev = *first_deltas.last().unwrap();
        first_deltas.push(prev.wrapping_add(dd));
    }

    // Reconstruct original values
    let result = first_deltas
        .iter()
        .map(|&delta| base.wrapping_add(delta))
        .collect();

    Ok(result)
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
            assert_eq!(orig.to_bits(), dec.to_bits(), "Mismatch: {} != {}", orig, dec);
        }

        println!("Linear sequence compression:");
        println!("  Original: {} bytes", values.len() * 4);
        println!("  Encoded:  {} bytes", encoded.len());
        println!("  Ratio:    {:.1}%", (encoded.len() as f64 / (values.len() * 4) as f64) * 100.0);
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
        println!("  Ratio:    {:.1}%", (encoded.len() as f64 / (values.len() * 4) as f64) * 100.0);
    }

    #[test]
    fn test_pfor_double_delta_with_outliers() {
        // Mostly linear with some outliers
        let mut values: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();
        values[50] = 5.0;  // Outlier
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
        println!("  Ratio:    {:.1}%", (encoded.len() as f64 / (values.len() * 4) as f64) * 100.0);
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
