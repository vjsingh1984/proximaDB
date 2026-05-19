//! INT8 SIMD Distance Computation
//!
//! Native INT8 distance computation using hardware-specific SIMD instructions.
//! Avoids expensive conversion to FP32 by working directly with quantized data.

use tracing::trace;

/// AVX2-optimized INT8 dot product using VPMADDWD instruction
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn int8_dot_product_avx2(vec_a: &[i8], vec_b: &[i8]) -> i32 {
    unsafe {
        use std::arch::x86_64::*;

        debug_assert_eq!(vec_a.len(), vec_b.len());

        let len = vec_a.len();
        let chunks = len / 32; // 32 INT8 values per AVX2 register

        let mut sum_vec = _mm256_setzero_si256();

        for i in 0..chunks {
            let offset = i * 32;

            let a_vec = _mm256_loadu_si256(vec_a.as_ptr().add(offset) as *const __m256i);
            let b_vec = _mm256_loadu_si256(vec_b.as_ptr().add(offset) as *const __m256i);

            let a_lo =
                _mm256_unpacklo_epi8(a_vec, _mm256_cmpgt_epi8(_mm256_setzero_si256(), a_vec));
            let a_hi =
                _mm256_unpackhi_epi8(a_vec, _mm256_cmpgt_epi8(_mm256_setzero_si256(), a_vec));
            let b_lo =
                _mm256_unpacklo_epi8(b_vec, _mm256_cmpgt_epi8(_mm256_setzero_si256(), b_vec));
            let b_hi =
                _mm256_unpackhi_epi8(b_vec, _mm256_cmpgt_epi8(_mm256_setzero_si256(), b_vec));

            let prod_lo = _mm256_madd_epi16(a_lo, b_lo);
            let prod_hi = _mm256_madd_epi16(a_hi, b_hi);

            sum_vec = _mm256_add_epi32(sum_vec, prod_lo);
            sum_vec = _mm256_add_epi32(sum_vec, prod_hi);
        }

        let sum_128_lo = _mm256_extracti128_si256(sum_vec, 0);
        let sum_128_hi = _mm256_extracti128_si256(sum_vec, 1);
        let sum_128 = _mm_add_epi32(sum_128_lo, sum_128_hi);
        let sum_64 = _mm_add_epi32(sum_128, _mm_srli_si128(sum_128, 8));
        let sum_32 = _mm_add_epi32(sum_64, _mm_srli_si128(sum_64, 4));
        let mut result = _mm_cvtsi128_si32(sum_32);

        let start = chunks * 32;
        for i in start..len {
            result += vec_a[i] as i32 * vec_b[i] as i32;
        }

        trace!("AVX2 INT8 dot product computed for {} elements", len);
        result
    }
}

/// AVX2-optimized INT8 squared difference computation
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn int8_squared_diff_avx2(vec_a: &[i8], vec_b: &[i8]) -> i32 {
    unsafe {
        use std::arch::x86_64::*;

        debug_assert_eq!(vec_a.len(), vec_b.len());

        let len = vec_a.len();
        let chunks = len / 32;

        let mut sum_vec = _mm256_setzero_si256();

        for i in 0..chunks {
            let offset = i * 32;

            let a_vec = _mm256_loadu_si256(vec_a.as_ptr().add(offset) as *const __m256i);
            let b_vec = _mm256_loadu_si256(vec_b.as_ptr().add(offset) as *const __m256i);

            let diff_vec = _mm256_sub_epi8(a_vec, b_vec);

            let diff_lo = _mm256_unpacklo_epi8(
                diff_vec,
                _mm256_cmpgt_epi8(_mm256_setzero_si256(), diff_vec),
            );
            let diff_hi = _mm256_unpackhi_epi8(
                diff_vec,
                _mm256_cmpgt_epi8(_mm256_setzero_si256(), diff_vec),
            );

            let squared_lo = _mm256_madd_epi16(diff_lo, diff_lo);
            let squared_hi = _mm256_madd_epi16(diff_hi, diff_hi);

            sum_vec = _mm256_add_epi32(sum_vec, squared_lo);
            sum_vec = _mm256_add_epi32(sum_vec, squared_hi);
        }

        let sum_128_lo = _mm256_extracti128_si256(sum_vec, 0);
        let sum_128_hi = _mm256_extracti128_si256(sum_vec, 1);
        let sum_128 = _mm_add_epi32(sum_128_lo, sum_128_hi);
        let sum_64 = _mm_add_epi32(sum_128, _mm_srli_si128(sum_128, 8));
        let sum_32 = _mm_add_epi32(sum_64, _mm_srli_si128(sum_64, 4));
        let mut result = _mm_cvtsi128_si32(sum_32);

        let start = chunks * 32;
        for i in start..len {
            let diff = vec_a[i] as i32 - vec_b[i] as i32;
            result += diff * diff;
        }

        trace!("AVX2 INT8 squared difference computed for {} elements", len);
        result
    }
}

/// NEON-optimized INT8 dot product for ARM64
///
/// # Safety
///
/// This function enables and uses ARM NEON intrinsics. Callers must ensure the
/// current CPU supports NEON, and both input slices must have the same length.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub unsafe fn int8_dot_product_neon(vec_a: &[i8], vec_b: &[i8]) -> i32 {
    unsafe {
        use std::arch::aarch64::*;

        debug_assert_eq!(vec_a.len(), vec_b.len());

        let len = vec_a.len();
        let chunks = len / 16; // 16 INT8 values per NEON register

        let mut sum_vec = vdupq_n_s32(0);

        for i in 0..chunks {
            let offset = i * 16;

            let a_vec = vld1q_s8(vec_a.as_ptr().add(offset));
            let b_vec = vld1q_s8(vec_b.as_ptr().add(offset));

            let a_lo = vmovl_s8(vget_low_s8(a_vec));
            let a_hi = vmovl_s8(vget_high_s8(a_vec));
            let b_lo = vmovl_s8(vget_low_s8(b_vec));
            let b_hi = vmovl_s8(vget_high_s8(b_vec));

            let prod_lo = vmull_s16(vget_low_s16(a_lo), vget_low_s16(b_lo));
            let prod_hi = vmull_s16(vget_high_s16(a_lo), vget_high_s16(b_lo));
            sum_vec = vaddq_s32(sum_vec, prod_lo);
            sum_vec = vaddq_s32(sum_vec, prod_hi);

            let prod_lo = vmull_s16(vget_low_s16(a_hi), vget_low_s16(b_hi));
            let prod_hi = vmull_s16(vget_high_s16(a_hi), vget_high_s16(b_hi));
            sum_vec = vaddq_s32(sum_vec, prod_lo);
            sum_vec = vaddq_s32(sum_vec, prod_hi);
        }

        let mut result = vaddvq_s32(sum_vec);

        let start = chunks * 16;
        for i in start..len {
            result += vec_a[i] as i32 * vec_b[i] as i32;
        }

        trace!("NEON INT8 dot product computed for {} elements", len);
        result
    }
}

/// NEON-optimized INT8 squared difference for ARM64
///
/// # Safety
///
/// This function enables and uses ARM NEON intrinsics. Callers must ensure the
/// current CPU supports NEON, and both input slices must have the same length.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub unsafe fn int8_squared_diff_neon(vec_a: &[i8], vec_b: &[i8]) -> i32 {
    unsafe {
        use std::arch::aarch64::*;

        debug_assert_eq!(vec_a.len(), vec_b.len());

        let len = vec_a.len();
        let chunks = len / 16;

        let mut sum_vec = vdupq_n_s32(0);

        for i in 0..chunks {
            let offset = i * 16;

            let a_vec = vld1q_s8(vec_a.as_ptr().add(offset));
            let b_vec = vld1q_s8(vec_b.as_ptr().add(offset));

            let diff_vec = vsubq_s8(a_vec, b_vec);

            let diff_lo = vmovl_s8(vget_low_s8(diff_vec));
            let diff_hi = vmovl_s8(vget_high_s8(diff_vec));

            let squared_lo = vmull_s16(vget_low_s16(diff_lo), vget_low_s16(diff_lo));
            let squared_hi = vmull_s16(vget_high_s16(diff_lo), vget_high_s16(diff_lo));
            sum_vec = vaddq_s32(sum_vec, squared_lo);
            sum_vec = vaddq_s32(sum_vec, squared_hi);

            let squared_lo = vmull_s16(vget_low_s16(diff_hi), vget_low_s16(diff_hi));
            let squared_hi = vmull_s16(vget_high_s16(diff_hi), vget_high_s16(diff_hi));
            sum_vec = vaddq_s32(sum_vec, squared_lo);
            sum_vec = vaddq_s32(sum_vec, squared_hi);
        }

        let mut result = vaddvq_s32(sum_vec);

        let start = chunks * 16;
        for i in start..len {
            let diff = vec_a[i] as i32 - vec_b[i] as i32;
            result += diff * diff;
        }

        trace!("NEON INT8 squared difference computed for {} elements", len);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_int8_dot_product_scalar_vs_simd() {
        let vec_a: Vec<i8> = vec![1, -2, 3, -4, 5, -6, 7, -8];
        let vec_b: Vec<i8> = vec![2, 3, -1, 2, -3, 1, -2, 3];

        let scalar_result: i32 = vec_a
            .iter()
            .zip(vec_b.iter())
            .map(|(&a, &b)| a as i32 * b as i32)
            .sum();

        #[cfg(target_arch = "x86_64")]
        {
            use proximadb_hardware::{SimdLevel, best_simd_level};
            if best_simd_level() >= SimdLevel::AVX2 {
                let simd_result = unsafe { int8_dot_product_avx2(&vec_a, &vec_b) };
                assert_eq!(scalar_result, simd_result);
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            let simd_result = unsafe { int8_dot_product_neon(&vec_a, &vec_b) };
            assert_eq!(scalar_result, simd_result);
        }
    }

    #[test]
    fn test_int8_squared_diff_scalar_vs_simd() {
        let vec_a: Vec<i8> = vec![10, -20, 30, -40, 50, -60, 70, -80];
        let vec_b: Vec<i8> = vec![5, -15, 25, -35, 45, -55, 65, -75];

        let scalar_result: i32 = vec_a
            .iter()
            .zip(vec_b.iter())
            .map(|(&a, &b)| {
                let diff = a as i32 - b as i32;
                diff * diff
            })
            .sum();

        #[cfg(target_arch = "x86_64")]
        {
            use proximadb_hardware::{SimdLevel, best_simd_level};
            if best_simd_level() >= SimdLevel::AVX2 {
                let simd_result = unsafe { int8_squared_diff_avx2(&vec_a, &vec_b) };
                assert_eq!(scalar_result, simd_result);
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            let simd_result = unsafe { int8_squared_diff_neon(&vec_a, &vec_b) };
            assert_eq!(scalar_result, simd_result);
        }
    }

    #[test]
    fn test_large_vectors() {
        let size = 1000;
        let vec_a: Vec<i8> = (0..size).map(|i| (i % 127) as i8).collect();
        let vec_b: Vec<i8> = (0..size).map(|i| ((i * 3) % 127) as i8).collect();

        let scalar_result: i32 = vec_a
            .iter()
            .zip(vec_b.iter())
            .map(|(&a, &b)| a as i32 * b as i32)
            .sum();

        #[cfg(target_arch = "x86_64")]
        {
            use proximadb_hardware::{SimdLevel, best_simd_level};
            if best_simd_level() >= SimdLevel::AVX2 {
                let simd_result = unsafe { int8_dot_product_avx2(&vec_a, &vec_b) };
                assert_eq!(scalar_result, simd_result);
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            let simd_result = unsafe { int8_dot_product_neon(&vec_a, &vec_b) };
            assert_eq!(scalar_result, simd_result);
        }
    }
}
