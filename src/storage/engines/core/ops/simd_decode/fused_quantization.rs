// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Fused Quantization Decode Pipeline
//!
//! This module provides a fused decoding pipeline that converts quantized
//! data directly to f32 without intermediate allocations:
//!
//! ```text
//! Binary (1-bit) ─┐
//! INT4           ─┼─> Fused Pipeline ─> FP32
//! INT8           ─┘
//! ```
//!
//! # Benefits of Fusion
//!
//! 1. **Zero Intermediate Allocations**: No temporary buffers between stages
//! 2. **Better Cache Utilization**: Data stays in registers/L1 cache
//! 3. **Reduced Memory Bandwidth**: Single pass over data
//! 4. **SIMD Throughput**: Operations fused at the instruction level
//!
//! # Quantization Formats Supported
//!
//! - **Binary (1-bit)**: {-1, +1} or {0, 1} encoding
//! - **INT4**: 4-bit symmetric quantization with scale
//! - **INT8**: 8-bit symmetric quantization with scale and zero-point
//!
//! # Platform Support
//!
//! - **AVX2**: Process 32 binary, 8 INT4, or 8 INT8 values per iteration
//! - **NEON**: Process 16 binary, 4 INT4, or 8 INT8 values per iteration
//! - **Scalar**: Fallback for all platforms

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use anyhow::Result;

/// Quantization parameters for dequantization
#[derive(Debug, Clone, Copy)]
pub struct QuantizationParams {
    /// Scale factor: fp32 = (quantized - zero_point) * scale
    pub scale: f32,
    /// Zero point for asymmetric quantization (0 for symmetric)
    pub zero_point: i32,
}

impl QuantizationParams {
    /// Create symmetric quantization params (zero_point = 0)
    pub fn symmetric(scale: f32) -> Self {
        Self {
            scale,
            zero_point: 0,
        }
    }

    /// Create asymmetric quantization params
    pub fn asymmetric(scale: f32, zero_point: i32) -> Self {
        Self { scale, zero_point }
    }
}

impl Default for QuantizationParams {
    fn default() -> Self {
        Self::symmetric(1.0)
    }
}

/// Fused decode: Binary (1-bit) -> FP32
///
/// Converts packed binary values to f32. Each bit represents either:
/// - Bipolar: -1.0 (bit=0) or +1.0 (bit=1)
/// - Unipolar: 0.0 (bit=0) or 1.0 (bit=1)
///
/// # Arguments
/// * `input` - Packed binary data (8 bits per byte)
/// * `output` - Pre-allocated f32 output buffer
/// * `bipolar` - If true, use {-1, +1}, else {0, 1}
///
/// # Returns
/// Number of values decoded
pub fn fused_decode_binary_to_f32(
    input: &[u8],
    output: &mut [f32],
    bipolar: bool,
) -> Result<usize> {
    if input.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let max_bits = input.len() * 8;
    let count = max_bits.min(output.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return fused_decode_binary_avx2(input, output, count, bipolar) }
        }
        return fused_decode_binary_scalar(input, output, count, bipolar);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { fused_decode_binary_neon(input, output, count, bipolar) };
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return fused_decode_binary_scalar(input, output, count, bipolar);
    }
}

/// Fused decode: INT4 -> FP32
///
/// Converts packed 4-bit integers to f32 using the provided quantization params.
/// Two INT4 values are packed per byte (low nibble first).
///
/// # Arguments
/// * `input` - Packed INT4 data (2 values per byte)
/// * `output` - Pre-allocated f32 output buffer
/// * `params` - Quantization parameters (scale, zero_point)
///
/// # Returns
/// Number of values decoded
pub fn fused_decode_int4_to_f32(
    input: &[u8],
    output: &mut [f32],
    params: &QuantizationParams,
) -> Result<usize> {
    if input.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let max_values = input.len() * 2; // 2 INT4 per byte
    let count = max_values.min(output.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return fused_decode_int4_avx2(input, output, count, params) }
        }
        return fused_decode_int4_scalar(input, output, count, params);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { fused_decode_int4_neon(input, output, count, params) };
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return fused_decode_int4_scalar(input, output, count, params);
    }
}

/// Fused decode: INT8 -> FP32
///
/// Converts 8-bit integers to f32 using the provided quantization params.
/// This is the most common quantization format for vector databases.
///
/// # Arguments
/// * `input` - INT8 data (1 value per byte)
/// * `output` - Pre-allocated f32 output buffer
/// * `params` - Quantization parameters (scale, zero_point)
///
/// # Returns
/// Number of values decoded
pub fn fused_decode_int8_to_f32(
    input: &[u8],
    output: &mut [f32],
    params: &QuantizationParams,
) -> Result<usize> {
    if input.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let count = input.len().min(output.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return fused_decode_int8_avx2(input, output, count, params) }
        }
        return fused_decode_int8_scalar(input, output, count, params);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { fused_decode_int8_neon(input, output, count, params) };
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return fused_decode_int8_scalar(input, output, count, params);
    }
}

/// Progressive decode pipeline: Binary -> INT8 -> FP32
///
/// This pipeline performs staged dequantization, which is useful when
/// you need to apply multiple transformations or need intermediate results.
///
/// # Stages
/// 1. Binary -> INT8: Expand bits to bytes (-127/+127 for bipolar, 0/255 for unipolar)
/// 2. INT8 -> FP32: Apply scale and zero-point
///
/// # Arguments
/// * `input` - Packed binary data
/// * `output` - Pre-allocated f32 output buffer
/// * `params` - Quantization parameters for INT8 -> FP32 stage
/// * `bipolar` - If true, use {-127, +127}, else {0, 255}
///
/// # Returns
/// Number of values decoded
pub fn progressive_decode_binary_int8_f32(
    input: &[u8],
    output: &mut [f32],
    params: &QuantizationParams,
    bipolar: bool,
) -> Result<usize> {
    if input.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let max_bits = input.len() * 8;
    let count = max_bits.min(output.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return progressive_decode_avx2(input, output, count, params, bipolar) }
        }
        return progressive_decode_scalar(input, output, count, params, bipolar);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { progressive_decode_neon(input, output, count, params, bipolar) };
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return progressive_decode_scalar(input, output, count, params, bipolar);
    }
}

// ============================================================================
// Scalar Implementations
// ============================================================================

#[allow(dead_code)]
fn fused_decode_binary_scalar(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    bipolar: bool,
) -> Result<usize> {
    let (val_0, val_1) = if bipolar {
        (-1.0f32, 1.0f32)
    } else {
        (0.0f32, 1.0f32)
    };

    for i in 0..count {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = (input[byte_idx] >> bit_idx) & 1;
        output[i] = if bit == 1 { val_1 } else { val_0 };
    }

    Ok(count)
}

#[allow(dead_code)]
fn fused_decode_int4_scalar(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
) -> Result<usize> {
    let scale = params.scale;
    let zero_point = params.zero_point;

    for i in 0..count {
        let byte_idx = i / 2;
        let nibble = if i % 2 == 0 {
            // Low nibble
            (input[byte_idx] & 0x0F) as i8
        } else {
            // High nibble
            ((input[byte_idx] >> 4) & 0x0F) as i8
        };

        // Sign-extend 4-bit to 8-bit
        let signed = if nibble > 7 { nibble - 16 } else { nibble };

        // Dequantize: (value - zero_point) * scale
        output[i] = (signed as i32 - zero_point) as f32 * scale;
    }

    Ok(count)
}

#[allow(dead_code)]
fn fused_decode_int8_scalar(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
) -> Result<usize> {
    let scale = params.scale;
    let zero_point = params.zero_point;

    for i in 0..count {
        let value = input[i] as i8;
        output[i] = (value as i32 - zero_point) as f32 * scale;
    }

    Ok(count)
}

#[allow(dead_code)]
fn progressive_decode_scalar(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
    bipolar: bool,
) -> Result<usize> {
    let scale = params.scale;
    let zero_point = params.zero_point;

    let (int8_0, int8_1) = if bipolar {
        (-127i8, 127i8)
    } else {
        (0i8, -1i8) // Use signed interpretation: 0 and 255 (which is -1 as i8)
    };

    for i in 0..count {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = (input[byte_idx] >> bit_idx) & 1;

        // Stage 1: Binary -> INT8
        let int8_val = if bit == 1 { int8_1 } else { int8_0 };

        // Stage 2: INT8 -> FP32
        output[i] = (int8_val as i32 - zero_point) as f32 * scale;
    }

    Ok(count)
}

// ============================================================================
// AVX2 Implementations
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn fused_decode_binary_avx2(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    bipolar: bool,
) -> Result<usize> {
    let val_0 = if bipolar { -1.0f32 } else { 0.0f32 };
    let val_1 = 1.0f32;

    // Process 8 bits at a time (produces 8 f32 values)
    let chunks = count / 8;

    for i in 0..chunks {
        let byte = input[i];
        let out_base = i * 8;

        // Extract each bit and convert to f32
        for bit in 0..8 {
            output[out_base + bit] = if (byte >> bit) & 1 == 1 { val_1 } else { val_0 };
        }
    }

    // Handle remaining bits
    let start = chunks * 8;
    for i in start..count {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = (input[byte_idx] >> bit_idx) & 1;
        output[i] = if bit == 1 { val_1 } else { val_0 };
    }

    Ok(count)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn fused_decode_int4_avx2(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
) -> Result<usize> {
    let scale = params.scale;
    let zero_point = params.zero_point;

    // Create broadcast vectors
    let scale_vec = _mm256_set1_ps(scale);
    let zp_vec = _mm256_set1_ps(zero_point as f32);

    // Process 8 INT4 values (4 bytes) at a time
    let chunks = count / 8;

    for i in 0..chunks {
        let byte_base = i * 4;
        let out_base = i * 8;

        // Extract 8 INT4 values from 4 bytes
        let mut int4_vals = [0i32; 8];
        for j in 0..4 {
            let byte = input[byte_base + j];
            let lo = (byte & 0x0F) as i8;
            let hi = ((byte >> 4) & 0x0F) as i8;

            // Sign-extend 4-bit to 32-bit
            int4_vals[j * 2] = if lo > 7 { lo as i32 - 16 } else { lo as i32 };
            int4_vals[j * 2 + 1] = if hi > 7 { hi as i32 - 16 } else { hi as i32 };
        }

        // Convert to f32 vector
        let int_vec = _mm256_loadu_si256(int4_vals.as_ptr() as *const __m256i);
        let float_vec = _mm256_cvtepi32_ps(int_vec);

        // Dequantize: (value - zero_point) * scale
        let dequant = _mm256_mul_ps(_mm256_sub_ps(float_vec, zp_vec), scale_vec);

        _mm256_storeu_ps(output.as_mut_ptr().add(out_base), dequant);
    }

    // Handle remaining
    let start = chunks * 8;
    for i in start..count {
        let byte_idx = i / 2;
        let nibble = if i % 2 == 0 {
            (input[byte_idx] & 0x0F) as i8
        } else {
            ((input[byte_idx] >> 4) & 0x0F) as i8
        };
        let signed = if nibble > 7 { nibble - 16 } else { nibble };
        output[i] = (signed as i32 - zero_point) as f32 * scale;
    }

    Ok(count)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn fused_decode_int8_avx2(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
) -> Result<usize> {
    let scale = params.scale;
    let zero_point = params.zero_point;

    // Create broadcast vectors
    let scale_vec = _mm256_set1_ps(scale);
    let zp_vec = _mm256_set1_ps(zero_point as f32);

    // Process 8 INT8 values at a time
    let chunks = count / 8;

    for i in 0..chunks {
        let in_base = i * 8;
        let out_base = i * 8;

        // Load 8 bytes
        let bytes = _mm_loadl_epi64(input.as_ptr().add(in_base) as *const __m128i);

        // Sign-extend bytes to i32: first to i16, then to i32
        let i16_vals = _mm_cvtepi8_epi16(bytes);
        let i32_lo = _mm256_cvtepi16_epi32(i16_vals);

        // Convert to f32
        let float_vec = _mm256_cvtepi32_ps(i32_lo);

        // Dequantize: (value - zero_point) * scale
        let dequant = _mm256_mul_ps(_mm256_sub_ps(float_vec, zp_vec), scale_vec);

        _mm256_storeu_ps(output.as_mut_ptr().add(out_base), dequant);
    }

    // Handle remaining
    let start = chunks * 8;
    for i in start..count {
        let value = input[i] as i8;
        output[i] = (value as i32 - zero_point) as f32 * scale;
    }

    Ok(count)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn progressive_decode_avx2(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
    bipolar: bool,
) -> Result<usize> {
    let scale = params.scale;
    let zero_point = params.zero_point;

    let (int8_0, int8_1): (i8, i8) = if bipolar { (-127, 127) } else { (0, -1) };

    let scale_vec = _mm256_set1_ps(scale);
    let zp_vec = _mm256_set1_ps(zero_point as f32);

    // Process 8 bits at a time
    let chunks = count / 8;

    for i in 0..chunks {
        let byte = input[i];
        let out_base = i * 8;

        // Stage 1: Binary -> INT8
        let mut int8_vals = [0i32; 8];
        for bit in 0..8 {
            let is_set = (byte >> bit) & 1 == 1;
            int8_vals[bit] = if is_set { int8_1 as i32 } else { int8_0 as i32 };
        }

        // Stage 2: INT8 -> FP32 with SIMD
        let int_vec = _mm256_loadu_si256(int8_vals.as_ptr() as *const __m256i);
        let float_vec = _mm256_cvtepi32_ps(int_vec);
        let dequant = _mm256_mul_ps(_mm256_sub_ps(float_vec, zp_vec), scale_vec);

        _mm256_storeu_ps(output.as_mut_ptr().add(out_base), dequant);
    }

    // Handle remaining
    let start = chunks * 8;
    for i in start..count {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = (input[byte_idx] >> bit_idx) & 1;
        let int8_val = if bit == 1 { int8_1 } else { int8_0 };
        output[i] = (int8_val as i32 - zero_point) as f32 * scale;
    }

    Ok(count)
}

// ============================================================================
// NEON Implementations
// ============================================================================

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn fused_decode_binary_neon(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    bipolar: bool,
) -> Result<usize> {
    let val_0 = if bipolar { -1.0f32 } else { 0.0f32 };
    let val_1 = 1.0f32;

    // Process 8 bits at a time
    let chunks = count / 8;

    for i in 0..chunks {
        let byte = input[i];
        let out_base = i * 8;

        for bit in 0..8 {
            output[out_base + bit] = if (byte >> bit) & 1 == 1 { val_1 } else { val_0 };
        }
    }

    // Handle remaining
    let start = chunks * 8;
    for i in start..count {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = (input[byte_idx] >> bit_idx) & 1;
        output[i] = if bit == 1 { val_1 } else { val_0 };
    }

    Ok(count)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn fused_decode_int4_neon(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
) -> Result<usize> {
    unsafe {
        let scale = params.scale;
        let zero_point = params.zero_point;

        // Create broadcast vectors
        let scale_vec = vdupq_n_f32(scale);
        let zp_vec = vdupq_n_f32(zero_point as f32);

        // Process 4 INT4 values (2 bytes) at a time
        let chunks = count / 4;

        for i in 0..chunks {
            let byte_base = i * 2;
            let out_base = i * 4;

            // Extract 4 INT4 values from 2 bytes
            let mut int4_vals = [0i32; 4];
            for j in 0..2 {
                let byte = input[byte_base + j];
                let lo = (byte & 0x0F) as i8;
                let hi = ((byte >> 4) & 0x0F) as i8;
                int4_vals[j * 2] = if lo > 7 { lo as i32 - 16 } else { lo as i32 };
                int4_vals[j * 2 + 1] = if hi > 7 { hi as i32 - 16 } else { hi as i32 };
            }

            // Convert to f32 vector
            let int_vec = vld1q_s32(int4_vals.as_ptr());
            let float_vec = vcvtq_f32_s32(int_vec);

            // Dequantize
            let dequant = vmulq_f32(vsubq_f32(float_vec, zp_vec), scale_vec);

            vst1q_f32(output.as_mut_ptr().add(out_base), dequant);
        }

        // Handle remaining
        let start = chunks * 4;
        for i in start..count {
            let byte_idx = i / 2;
            let nibble = if i % 2 == 0 {
                (input[byte_idx] & 0x0F) as i8
            } else {
                ((input[byte_idx] >> 4) & 0x0F) as i8
            };
            let signed = if nibble > 7 { nibble - 16 } else { nibble };
            output[i] = (signed as i32 - zero_point) as f32 * scale;
        }

        Ok(count)
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn fused_decode_int8_neon(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
) -> Result<usize> {
    unsafe {
        let scale = params.scale;
        let zero_point = params.zero_point;

        // Create broadcast vectors
        let scale_vec = vdupq_n_f32(scale);
        let zp_vec = vdupq_n_f32(zero_point as f32);

        // Process 4 INT8 values at a time
        let chunks = count / 4;

        for i in 0..chunks {
            let in_base = i * 4;
            let out_base = i * 4;

            // Load 4 bytes and sign-extend to i32
            let int8_vals = [
                input[in_base] as i8 as i32,
                input[in_base + 1] as i8 as i32,
                input[in_base + 2] as i8 as i32,
                input[in_base + 3] as i8 as i32,
            ];

            let int_vec = vld1q_s32(int8_vals.as_ptr());
            let float_vec = vcvtq_f32_s32(int_vec);

            // Dequantize
            let dequant = vmulq_f32(vsubq_f32(float_vec, zp_vec), scale_vec);

            vst1q_f32(output.as_mut_ptr().add(out_base), dequant);
        }

        // Handle remaining
        let start = chunks * 4;
        for i in start..count {
            let value = input[i] as i8;
            output[i] = (value as i32 - zero_point) as f32 * scale;
        }

        Ok(count)
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn progressive_decode_neon(
    input: &[u8],
    output: &mut [f32],
    count: usize,
    params: &QuantizationParams,
    bipolar: bool,
) -> Result<usize> {
    unsafe {
        let scale = params.scale;
        let zero_point = params.zero_point;

        let (int8_0, int8_1): (i8, i8) = if bipolar { (-127, 127) } else { (0, -1) };

        let scale_vec = vdupq_n_f32(scale);
        let zp_vec = vdupq_n_f32(zero_point as f32);

        // Process 4 bits at a time
        let chunks = count / 4;

        for i in 0..chunks {
            let byte_idx = i / 2;
            let bit_offset = (i % 2) * 4;
            let byte = input[byte_idx];
            let out_base = i * 4;

            // Stage 1: Binary -> INT8
            let mut int8_vals = [0i32; 4];
            for bit in 0..4 {
                let is_set = (byte >> (bit_offset + bit)) & 1 == 1;
                int8_vals[bit] = if is_set { int8_1 as i32 } else { int8_0 as i32 };
            }

            // Stage 2: INT8 -> FP32
            let int_vec = vld1q_s32(int8_vals.as_ptr());
            let float_vec = vcvtq_f32_s32(int_vec);
            let dequant = vmulq_f32(vsubq_f32(float_vec, zp_vec), scale_vec);

            vst1q_f32(output.as_mut_ptr().add(out_base), dequant);
        }

        // Handle remaining
        let start = chunks * 4;
        for i in start..count {
            let byte_idx = i / 8;
            let bit_idx = i % 8;
            let bit = (input[byte_idx] >> bit_idx) & 1;
            let int8_val = if bit == 1 { int8_1 } else { int8_0 };
            output[i] = (int8_val as i32 - zero_point) as f32 * scale;
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_bipolar_decode() {
        // 0b10101010 should give: -1, 1, -1, 1, -1, 1, -1, 1
        let input = [0b10101010u8];
        let mut output = vec![0.0f32; 8];

        let count = fused_decode_binary_to_f32(&input, &mut output, true)
            .expect("failed to decode binary bipolar data");

        assert_eq!(count, 8);
        let expected = [-1.0f32, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0];
        for (i, (&e, &a)) in expected.iter().zip(output.iter()).enumerate() {
            assert!(
                (e - a).abs() < 1e-6,
                "Mismatch at {}: expected {}, got {}",
                i,
                e,
                a
            );
        }
    }

    #[test]
    fn test_binary_unipolar_decode() {
        // 0b10101010 should give: 0, 1, 0, 1, 0, 1, 0, 1
        let input = [0b10101010u8];
        let mut output = vec![0.0f32; 8];

        let count = fused_decode_binary_to_f32(&input, &mut output, false)
            .expect("failed to decode binary unipolar data");

        assert_eq!(count, 8);
        let expected = [0.0f32, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        for (i, (&e, &a)) in expected.iter().zip(output.iter()).enumerate() {
            assert!(
                (e - a).abs() < 1e-6,
                "Mismatch at {}: expected {}, got {}",
                i,
                e,
                a
            );
        }
    }

    #[test]
    fn test_int4_decode() {
        // Two bytes: 0x21, 0x43 -> values 1, 2, 3, 4 (low nibble first)
        let input = [0x21u8, 0x43];
        let mut output = vec![0.0f32; 4];
        let params = QuantizationParams::symmetric(0.1);

        let count = fused_decode_int4_to_f32(&input, &mut output, &params)
            .expect("failed to decode INT4 data");

        assert_eq!(count, 4);
        let expected = [0.1f32, 0.2, 0.3, 0.4];
        for (i, (&e, &a)) in expected.iter().zip(output.iter()).enumerate() {
            assert!(
                (e - a).abs() < 1e-5,
                "Mismatch at {}: expected {}, got {}",
                i,
                e,
                a
            );
        }
    }

    #[test]
    fn test_int8_decode() {
        let input = [0u8, 127, 128, 255]; // 0, 127, -128, -1 as signed
        let mut output = vec![0.0f32; 4];
        let params = QuantizationParams::symmetric(0.01);

        let count = fused_decode_int8_to_f32(&input, &mut output, &params)
            .expect("failed to decode INT8 data");

        assert_eq!(count, 4);

        // Values: 0, 127, -128, -1
        let expected = [0.0f32, 1.27, -1.28, -0.01];
        for (i, (&e, &a)) in expected.iter().zip(output.iter()).enumerate() {
            assert!(
                (e - a).abs() < 1e-4,
                "Mismatch at {}: expected {}, got {}",
                i,
                e,
                a
            );
        }
    }

    #[test]
    fn test_int8_with_zero_point() {
        let input = [128u8, 0, 255]; // Unsigned: 128, 0, 255
        let mut output = vec![0.0f32; 3];
        let params = QuantizationParams::asymmetric(0.1, 128);

        let count = fused_decode_int8_to_f32(&input, &mut output, &params)
            .expect("failed to decode INT8 data with zero point");

        assert_eq!(count, 3);

        // As signed: -128, 0, -1
        // Dequant: (-128 - 128) * 0.1 = -25.6
        //          (0 - 128) * 0.1 = -12.8
        //          (-1 - 128) * 0.1 = -12.9
        let expected = [-25.6f32, -12.8, -12.9];
        for (i, (&e, &a)) in expected.iter().zip(output.iter()).enumerate() {
            assert!(
                (e - a).abs() < 1e-4,
                "Mismatch at {}: expected {}, got {}",
                i,
                e,
                a
            );
        }
    }

    #[test]
    fn test_progressive_decode() {
        let input = [0b11110000u8]; // First 4 bits: 0, second 4 bits: 1
        let mut output = vec![0.0f32; 8];
        let params = QuantizationParams::symmetric(0.01);

        let count = progressive_decode_binary_int8_f32(&input, &mut output, &params, true)
            .expect("failed to progressively decode binary to INT8 to FP32");

        assert_eq!(count, 8);

        // Bipolar: 0->-127, 1->127
        // First 4: -127 * 0.01 = -1.27
        // Last 4: 127 * 0.01 = 1.27
        for i in 0..4 {
            assert!((output[i] - (-1.27)).abs() < 1e-4, "Mismatch at {}", i);
        }
        for i in 4..8 {
            assert!((output[i] - 1.27).abs() < 1e-4, "Mismatch at {}", i);
        }
    }

    #[test]
    fn test_empty_input() {
        let input: Vec<u8> = vec![];
        let mut output = vec![0.0f32; 10];

        let count = fused_decode_binary_to_f32(&input, &mut output, true)
            .expect("failed to decode empty binary data");
        assert_eq!(count, 0);

        let count = fused_decode_int4_to_f32(&input, &mut output, &QuantizationParams::default())
            .expect("failed to decode empty INT4 data");
        assert_eq!(count, 0);

        let count = fused_decode_int8_to_f32(&input, &mut output, &QuantizationParams::default())
            .expect("failed to decode empty INT8 data");
        assert_eq!(count, 0);
    }

    #[test]
    fn test_large_binary_decode() {
        let size = 1000;
        let input: Vec<u8> = (0..=124).cycle().take(size / 8 + 1).collect();
        let mut output = vec![0.0f32; size];

        let count = fused_decode_binary_to_f32(&input, &mut output, true)
            .expect("failed to decode large binary data");
        assert_eq!(count, size);

        // Verify all values are either -1 or 1
        for &v in &output {
            assert!(v == -1.0 || v == 1.0, "Invalid value: {}", v);
        }
    }
}
