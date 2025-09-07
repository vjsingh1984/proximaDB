//! AVX-512 SIMD implementations for distance calculations
//!
//! Provides highly optimized distance computations. Since AVX-512 intrinsics
//! require unstable Rust features, we implement enhanced AVX2 with unrolling
//! to achieve similar performance benefits.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Helper function to reduce a 256-bit vector to a scalar sum
#[cfg(target_arch = "x86_64")]
#[inline]
pub unsafe fn reduce_sum_ps_256(v: __m256) -> f32 { unsafe {
    let low = _mm256_extractf128_ps(v, 0);
    let high = _mm256_extractf128_ps(v, 1);
    let sum128 = _mm_add_ps(low, high);

    // Horizontal add within 128-bit lanes
    let shuf = _mm_shuffle_ps(sum128, sum128, 0x0E);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf = _mm_shuffle_ps(sums, sums, 0x01);
    let sums = _mm_add_ps(sums, shuf);

    _mm_cvtss_f32(sums)
}}

/// Helper function to reduce a 256-bit vector to maximum value
#[cfg(target_arch = "x86_64")]
#[inline]
pub unsafe fn reduce_max_ps_256(v: __m256) -> f32 { unsafe {
    let low = _mm256_extractf128_ps(v, 0);
    let high = _mm256_extractf128_ps(v, 1);
    let max128 = _mm_max_ps(low, high);

    // Horizontal max within 128-bit lanes
    let shuf = _mm_shuffle_ps(max128, max128, 0x0E);
    let maxs = _mm_max_ps(max128, shuf);
    let shuf = _mm_shuffle_ps(maxs, maxs, 0x01);
    let maxs = _mm_max_ps(maxs, shuf);

    _mm_cvtss_f32(maxs)
}}

/// Helper function to reduce a 256-bit vector to minimum value
#[cfg(target_arch = "x86_64")]
#[inline]
pub unsafe fn reduce_min_ps_256(v: __m256) -> f32 { unsafe {
    let low = _mm256_extractf128_ps(v, 0);
    let high = _mm256_extractf128_ps(v, 1);
    let min128 = _mm_min_ps(low, high);

    // Horizontal min within 128-bit lanes
    let shuf = _mm_shuffle_ps(min128, min128, 0x0E);
    let mins = _mm_min_ps(min128, shuf);
    let shuf = _mm_shuffle_ps(mins, mins, 0x01);
    let mins = _mm_min_ps(mins, shuf);

    _mm_cvtss_f32(mins)
}}

/// Prefetch data into cache
#[cfg(target_arch = "x86_64")]
#[inline]
pub unsafe fn prefetch_vector(data: *const f32, offset: usize) { unsafe {
    _mm_prefetch(data.add(offset) as *const i8, _MM_HINT_T0);
}}
