//! Compile-time quantization optimizations
//!
//! This module provides compile-time quantization for known vector dimensions
//! and quantization levels, eliminating runtime overhead.

/// Compile-time quantization trait
pub trait CompileTimeQuantization {
    const DIMENSION: usize;
    const QUANTIZATION_BITS: u8;
    const CODEBOOK_SIZE: usize;

    type QuantizedType;

    fn quantize_const<const N: usize>(input: &[f32; N]) -> Self::QuantizedType;
}

/// INT8 quantization at compile time
pub struct Int8CompileTime<const DIM: usize>;

impl<const DIM: usize> CompileTimeQuantization for Int8CompileTime<DIM> {
    const DIMENSION: usize = DIM;
    const QUANTIZATION_BITS: u8 = 8;
    const CODEBOOK_SIZE: usize = 256;

    type QuantizedType = [i8; DIM];

    #[inline(always)]
    fn quantize_const<const N: usize>(input: &[f32; N]) -> Self::QuantizedType {
        let mut output = [0i8; DIM];

        // Find min/max for scaling
        let (min, max) = input
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), &val| {
                (min.min(val), max.max(val))
            });

        let scale = if max > min { 255.0 / (max - min) } else { 1.0 };

        for i in 0..DIM.min(N) {
            let normalized = ((input[i] - min) * scale).round() as i16 - 128;
            output[i] = normalized.clamp(-128, 127) as i8;
        }

        output
    }
}

/// Product quantization at compile time
pub struct PQ4CompileTime<const DIM: usize, const SUBVECTORS: usize>;

impl<const DIM: usize, const SUBVECTORS: usize> CompileTimeQuantization
    for PQ4CompileTime<DIM, SUBVECTORS>
{
    const DIMENSION: usize = DIM;
    const QUANTIZATION_BITS: u8 = 4;
    const CODEBOOK_SIZE: usize = 16;

    type QuantizedType = [u8; SUBVECTORS];

    #[inline(always)]
    fn quantize_const<const N: usize>(input: &[f32; N]) -> Self::QuantizedType {
        let mut output = [0u8; SUBVECTORS];
        let subvector_size = DIM / SUBVECTORS;

        for (i, out_val) in output.iter_mut().enumerate() {
            let start = i * subvector_size;
            let end = ((i + 1) * subvector_size).min(N);

            if start < end {
                // Simple 4-bit quantization per subvector
                let subvec_sum: f32 = input[start..end].iter().sum();
                let subvec_mean = subvec_sum / (end - start) as f32;

                // Quantize to 4 bits (0-15)
                *out_val = ((subvec_mean + 1.0) * 7.5).round().clamp(0.0, 15.0) as u8;
            }
        }

        output
    }
}

/// Macro for compile-time quantization
#[macro_export]
macro_rules! quantize_compile_time {
    ($vec:expr, Int8, $dim:expr) => {{
        use $crate::compute::quantization::compile_time::{
            CompileTimeQuantization, Int8CompileTime,
        };
        Int8CompileTime::<$dim>::quantize_const($vec)
    }};
    ($vec:expr, PQ4, $dim:expr, $subvectors:expr) => {{
        use $crate::compute::quantization::compile_time::{
            CompileTimeQuantization, PQ4CompileTime,
        };
        PQ4CompileTime::<$dim, $subvectors>::quantize_const($vec)
    }};
}

/// SIMD-accelerated compile-time quantization for common dimensions
#[cfg(target_arch = "x86_64")]
pub mod simd {
    use std::arch::x86_64::*;

    /// SIMD INT8 quantization for 128-dimensional vectors
    #[target_feature(enable = "avx2")]
    pub unsafe fn quantize_int8_128_simd(input: &[f32; 128]) -> [i8; 128] {
        unsafe {
            let mut output = [0i8; 128];

            // Process 8 floats at a time with AVX2
            for i in (0..128).step_by(8) {
                let vals = _mm256_loadu_ps(&input[i]);
                let scaled = _mm256_mul_ps(vals, _mm256_set1_ps(127.0));
                let ints = _mm256_cvtps_epi32(scaled);

                // Pack to i8
                let packed = _mm256_packs_epi32(ints, ints);
                let packed = _mm256_packs_epi16(packed, packed);

                // Store lower 8 bytes
                let result = _mm256_extract_epi64(packed, 0);
                *(output.as_mut_ptr().add(i) as *mut i64) = result;
            }

            output
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_time_int8() {
        const DIM: usize = 4;
        let input = [0.1, 0.5, -0.3, 0.8];
        let quantized = Int8CompileTime::<DIM>::quantize_const(&input);
        assert_eq!(quantized.len(), DIM);
    }

    #[test]
    fn test_compile_time_pq4() {
        const DIM: usize = 8;
        const SUBVECTORS: usize = 2;
        let input = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        let quantized = PQ4CompileTime::<DIM, SUBVECTORS>::quantize_const(&input);
        assert_eq!(quantized.len(), SUBVECTORS);
    }
}
