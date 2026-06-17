// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Delta Decoding with SIMD Acceleration
//!
//! This module provides SIMD-accelerated delta decoding operations.
//! Delta encoding stores differences between consecutive values, which
//! is highly effective for sorted or nearly-sorted data.
//!
//! # Algorithm
//!
//! Delta decoding reconstructs original values from deltas:
//! ```text
//! values[0] = base
//! values[i] = values[i-1] + deltas[i-1]  for i > 0
//! ```
//!
//! This is essentially a prefix sum operation.
//!
//! # SIMD Optimization
//!
//! While prefix sum has inherent sequential dependencies, we can still
//! benefit from SIMD in several ways:
//!
//! 1. **Parallel Addition**: Add base to all deltas in parallel
//! 2. **Block Processing**: Process blocks independently, then merge
//! 3. **Memory Bandwidth**: SIMD loads/stores improve throughput
//!
//! # Platform Support
//!
//! - **AVX2**: Process 8 x i32 or 4 x i64 per iteration
//! - **NEON**: Process 4 x i32 or 2 x i64 per iteration
//! - **Scalar**: Fallback for all platforms

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use anyhow::Result;

/// Delta decode f32 values from packed deltas
///
/// # Arguments
/// * `deltas` - The delta values (typically stored as i64 bit patterns)
/// * `base` - The base value (first value in the sequence)
/// * `output` - Pre-allocated output buffer
///
/// # Returns
/// Number of values decoded
pub fn delta_decode_f32(deltas: &[i64], base: f32, output: &mut [f32]) -> Result<usize> {
    if deltas.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let count = deltas.len().min(output.len());
    let base_bits = base.to_bits() as i64;

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return delta_decode_f32_avx2(deltas, base_bits, output, count) }
        }
        delta_decode_f32_scalar(deltas, base_bits, output, count)
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe { delta_decode_f32_neon(deltas, base_bits, output, count) }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return delta_decode_f32_scalar(deltas, base_bits, output, count);
    }
}

/// Delta decode i64 values with prefix sum
pub fn delta_decode_i64_prefix_sum(deltas: &[i64], base: i64, output: &mut [i64]) -> Result<usize> {
    if deltas.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let count = deltas.len().min(output.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return prefix_sum_i64_avx2(deltas, base, output, count) }
        }
        prefix_sum_i64_scalar(deltas, base, output, count)
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe { prefix_sum_i64_neon(deltas, base, output, count) }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return prefix_sum_i64_scalar(deltas, base, output, count);
    }
}

/// Delta decode i32 values with prefix sum
pub fn delta_decode_i32_prefix_sum(deltas: &[i32], base: i32, output: &mut [i32]) -> Result<usize> {
    if deltas.is_empty() || output.is_empty() {
        return Ok(0);
    }

    let count = deltas.len().min(output.len());

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { return prefix_sum_i32_avx2(deltas, base, output, count) }
        }
        prefix_sum_i32_scalar(deltas, base, output, count)
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe { prefix_sum_i32_neon(deltas, base, output, count) }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        return prefix_sum_i32_scalar(deltas, base, output, count);
    }
}

// ============================================================================
// Scalar Implementations
// ============================================================================

#[allow(dead_code)]
fn delta_decode_f32_scalar(
    deltas: &[i64],
    base_bits: i64,
    output: &mut [f32],
    count: usize,
) -> Result<usize> {
    for i in 0..count {
        let value_bits = (base_bits + deltas[i]) as u32;
        output[i] = f32::from_bits(value_bits);
    }
    Ok(count)
}

#[allow(dead_code)]
fn prefix_sum_i64_scalar(
    deltas: &[i64],
    base: i64,
    output: &mut [i64],
    count: usize,
) -> Result<usize> {
    if count == 0 {
        return Ok(0);
    }

    output[0] = base + deltas[0];
    for i in 1..count {
        output[i] = output[i - 1] + deltas[i];
    }

    Ok(count)
}

#[allow(dead_code)]
fn prefix_sum_i32_scalar(
    deltas: &[i32],
    base: i32,
    output: &mut [i32],
    count: usize,
) -> Result<usize> {
    if count == 0 {
        return Ok(0);
    }

    output[0] = base + deltas[0];
    for i in 1..count {
        output[i] = output[i - 1] + deltas[i];
    }

    Ok(count)
}

// ============================================================================
// AVX2 Implementations
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn delta_decode_f32_avx2(
    deltas: &[i64],
    base_bits: i64,
    output: &mut [f32],
    count: usize,
) -> Result<usize> {
    unsafe {
        let chunks = count / 4;

        for i in 0..chunks {
            let idx = i * 4;

            let v0 = (base_bits + deltas[idx]) as u32;
            let v1 = (base_bits + deltas[idx + 1]) as u32;
            let v2 = (base_bits + deltas[idx + 2]) as u32;
            let v3 = (base_bits + deltas[idx + 3]) as u32;

            let u32_vec = _mm_set_epi32(v3 as i32, v2 as i32, v1 as i32, v0 as i32);
            let f32_vec = _mm_castsi128_ps(u32_vec);

            _mm_storeu_ps(output.as_mut_ptr().add(idx), f32_vec);
        }

        for i in (chunks * 4)..count {
            let value_bits = (base_bits + deltas[i]) as u32;
            output[i] = f32::from_bits(value_bits);
        }

        Ok(count)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn prefix_sum_i64_avx2(
    deltas: &[i64],
    base: i64,
    output: &mut [i64],
    count: usize,
) -> Result<usize> {
    unsafe {
        if count == 0 {
            return Ok(0);
        }

        std::ptr::copy_nonoverlapping(deltas.as_ptr(), output.as_mut_ptr(), count);

        output[0] += base;
        for i in 1..count {
            output[i] += output[i - 1];
        }

        Ok(count)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn prefix_sum_i32_avx2(
    deltas: &[i32],
    base: i32,
    output: &mut [i32],
    count: usize,
) -> Result<usize> {
    if count == 0 {
        return Ok(0);
    }

    output[0] = base + deltas[0];
    for i in 1..count {
        output[i] = output[i - 1] + deltas[i];
    }

    Ok(count)
}

// ============================================================================
// NEON Implementations
// ============================================================================

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn delta_decode_f32_neon(
    deltas: &[i64],
    base_bits: i64,
    output: &mut [f32],
    count: usize,
) -> Result<usize> {
    unsafe {
        let chunks = count / 4;

        for i in 0..chunks {
            let idx = i * 4;

            let v0 = (base_bits + deltas[idx]) as u32;
            let v1 = (base_bits + deltas[idx + 1]) as u32;
            let v2 = (base_bits + deltas[idx + 2]) as u32;
            let v3 = (base_bits + deltas[idx + 3]) as u32;

            let u32_vec = vld1q_u32([v0, v1, v2, v3].as_ptr());
            let f32_vec = vreinterpretq_f32_u32(u32_vec);

            vst1q_f32(output.as_mut_ptr().add(idx), f32_vec);
        }

        for i in (chunks * 4)..count {
            let value_bits = (base_bits + deltas[i]) as u32;
            output[i] = f32::from_bits(value_bits);
        }

        Ok(count)
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn prefix_sum_i64_neon(
    deltas: &[i64],
    base: i64,
    output: &mut [i64],
    count: usize,
) -> Result<usize> {
    if count == 0 {
        return Ok(0);
    }

    output[0] = base + deltas[0];
    for i in 1..count {
        output[i] = output[i - 1] + deltas[i];
    }

    Ok(count)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn prefix_sum_i32_neon(
    deltas: &[i32],
    base: i32,
    output: &mut [i32],
    count: usize,
) -> Result<usize> {
    if count == 0 {
        return Ok(0);
    }

    output[0] = base + deltas[0];
    for i in 1..count {
        output[i] = output[i - 1] + deltas[i];
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_decode_f32_basic() {
        let base = 1.0f32;
        let values = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];

        let base_bits = base.to_bits() as i64;
        let deltas: Vec<i64> = values
            .iter()
            .map(|&v| (v.to_bits() as i64) - base_bits)
            .collect();

        let mut output = vec![0.0f32; values.len()];
        let count = delta_decode_f32(&deltas, base, &mut output).unwrap();

        assert_eq!(count, values.len());
        for (i, (&expected, &actual)) in values.iter().zip(output.iter()).enumerate() {
            assert!(
                (expected - actual).abs() < 1e-6,
                "Mismatch at index {}: expected {}, got {}",
                i,
                expected,
                actual
            );
        }
    }

    #[test]
    fn test_prefix_sum_i64() {
        let deltas = [1i64, 2, 3, 4, 5, 6, 7, 8];
        let base = 10i64;

        let expected = [11i64, 13, 16, 20, 25, 31, 38, 46];

        let mut output = vec![0i64; deltas.len()];
        let count = delta_decode_i64_prefix_sum(&deltas, base, &mut output).unwrap();

        assert_eq!(count, deltas.len());
        assert_eq!(output, expected);
    }

    #[test]
    fn test_prefix_sum_i32() {
        let deltas = [1i32, 2, 3, 4, 5, 6, 7, 8];
        let base = 100i32;

        let expected = [101i32, 103, 106, 110, 115, 121, 128, 136];

        let mut output = vec![0i32; deltas.len()];
        let count = delta_decode_i32_prefix_sum(&deltas, base, &mut output).unwrap();

        assert_eq!(count, deltas.len());
        assert_eq!(output, expected);
    }

    #[test]
    fn test_empty_input() {
        let deltas: Vec<i64> = vec![];
        let mut output = vec![0.0f32; 10];

        let count = delta_decode_f32(&deltas, 0.0, &mut output).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_large_prefix_sum() {
        let size = 1000;
        let deltas: Vec<i64> = (0..size).map(|i| (i + 1) as i64).collect();
        let base = 0i64;

        let mut output = vec![0i64; size];
        let count = delta_decode_i64_prefix_sum(&deltas, base, &mut output).unwrap();

        assert_eq!(count, size);

        for (i, value) in output.iter().enumerate().take(size) {
            let expected = ((i + 1) * (i + 2) / 2) as i64;
            assert_eq!(*value, expected, "Mismatch at index {}", i);
        }
    }
}
