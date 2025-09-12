//! Hardware-Accelerated Quantization
//!
//! This module provides SIMD and GPU-accelerated implementations of quantization
//! operations, automatically selecting the best available hardware at runtime.

use crate::core::hardware_capabilities::{HardwareBackend, get_hardware_capabilities};
use anyhow::Result;

/// Hardware-accelerated quantization dispatcher
pub struct AcceleratedQuantization {
    backend: HardwareBackend,
}

impl AcceleratedQuantization {
    /// Create new accelerated quantization using global hardware capabilities
    pub fn new() -> Self {
        let caps = get_hardware_capabilities();
        let backend = {
            // Check for GPU first (highest performance)
            #[cfg(feature = "gpu")]
            {
                if caps.gpu.backend != crate::core::hardware_capabilities::GpuBackend::None {
                    match caps.gpu.backend {
                        crate::core::hardware_capabilities::GpuBackend::CUDA => {
                            HardwareBackend::CUDA
                        }
                        crate::core::hardware_capabilities::GpuBackend::ROCm => {
                            HardwareBackend::ROCm
                        }
                        crate::core::hardware_capabilities::GpuBackend::MPS => HardwareBackend::MPS,
                        crate::core::hardware_capabilities::GpuBackend::OpenCL => {
                            HardwareBackend::OpenCL
                        }
                        _ => Self::select_cpu_backend(&caps),
                    }
                } else {
                    Self::select_cpu_backend(&caps)
                }
            }
            #[cfg(not(feature = "gpu"))]
            Self::select_cpu_backend(&caps)
        };

        tracing::info!(
            "AcceleratedQuantization initialized with backend: {:?}",
            backend
        );
        Self { backend }
    }

    fn select_cpu_backend(
        caps: &crate::core::hardware_capabilities::HardwareCapabilities,
    ) -> HardwareBackend {
        if caps.cpu.features.avx512_support {
            HardwareBackend::AVX512
        } else if caps.cpu.features.avx2_support {
            HardwareBackend::AVX2
        } else if caps.cpu.features.sse42_support {
            HardwareBackend::SSE
        } else if caps.cpu.features.neon_support {
            HardwareBackend::NEON
        } else {
            HardwareBackend::Scalar
        }
    }

    /// Quantize to 4-bit with hardware acceleration
    pub fn quantize_u4_accelerated(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32, usize)> {
        // For now, use scalar implementation as 4-bit packing is complex for SIMD
        self.quantize_u4_scalar(values)
    }

    /// Quantize to 6-bit with hardware acceleration
    pub fn quantize_u6_accelerated(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32, usize)> {
        // For now, use scalar implementation as 6-bit packing is complex for SIMD
        self.quantize_u6_scalar(values)
    }

    /// Quantize to 8-bit with hardware acceleration
    pub fn quantize_u8_accelerated(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        match self.backend {
            HardwareBackend::AVX512 => self.quantize_u8_avx2(values), // Use AVX2 for now, AVX512 requires unstable
            HardwareBackend::AVX2 => self.quantize_u8_avx2(values),
            HardwareBackend::SSE => self.quantize_u8_sse(values),
            HardwareBackend::NEON => self.quantize_u8_neon(values),
            #[cfg(feature = "gpu")]
            HardwareBackend::CUDA => self.quantize_u8_cuda(values),
            #[cfg(feature = "gpu")]
            HardwareBackend::ROCm => self.quantize_u8_rocm(values),
            #[cfg(feature = "gpu")]
            HardwareBackend::MPS => self.quantize_u8_mps(values),
            #[cfg(feature = "gpu")]
            HardwareBackend::OpenCL => self.quantize_u8_opencl(values),
            _ => self.quantize_u8_scalar(values),
        }
    }

    /// Quantize to 16-bit with hardware acceleration
    pub fn quantize_u16_accelerated(&self, values: &[f32]) -> Result<(Vec<u16>, f32, f32)> {
        // For now, use scalar implementation
        // TODO: Add SIMD implementation for u16
        self.quantize_u16_scalar(values)
    }

    /// Scalar fallback implementation for 4-bit
    fn quantize_u4_scalar(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32, usize)> {
        let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let range = if max > min { max - min } else { 1.0 };

        let mut packed = Vec::with_capacity((values.len() + 1) / 2);
        let mut packed_byte = 0u8;
        let mut is_high_nibble = true;

        for &v in values {
            let normalized = ((v - min) / range).clamp(0.0, 1.0);
            let quantized = (normalized * 15.0).round() as u8;

            if is_high_nibble {
                packed_byte = quantized << 4;
                is_high_nibble = false;
            } else {
                packed_byte |= quantized & 0x0F;
                packed.push(packed_byte);
                packed_byte = 0;
                is_high_nibble = true;
            }
        }

        if !is_high_nibble {
            packed.push(packed_byte);
        }

        Ok((packed, min, max, values.len()))
    }

    /// Scalar fallback implementation for 6-bit
    fn quantize_u6_scalar(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32, usize)> {
        let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let range = if max > min { max - min } else { 1.0 };

        let mut packed = Vec::new();
        let max_val = 63.0;

        for chunk in values.chunks(4) {
            let mut vals = [0u8; 4];
            for (i, &v) in chunk.iter().enumerate() {
                let normalized = ((v - min) / range).clamp(0.0, 1.0);
                vals[i] = (normalized * max_val).round() as u8;
            }

            match chunk.len() {
                1 => packed.push(vals[0] << 2),
                2 => {
                    packed.push((vals[0] << 2) | (vals[1] >> 4));
                    packed.push((vals[1] & 0x0F) << 4);
                }
                3 => {
                    packed.push((vals[0] << 2) | (vals[1] >> 4));
                    packed.push((vals[1] & 0x0F) << 4 | (vals[2] >> 2));
                    packed.push((vals[2] & 0x03) << 6);
                }
                4 => {
                    packed.push((vals[0] << 2) | (vals[1] >> 4));
                    packed.push((vals[1] & 0x0F) << 4 | (vals[2] >> 2));
                    packed.push((vals[2] & 0x03) << 6 | vals[3]);
                }
                _ => {}
            }
        }

        Ok((packed, min, max, values.len()))
    }

    /// Scalar fallback implementation for 8-bit
    fn quantize_u8_scalar(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let range = if max > min { max - min } else { 1.0 };

        let quantized: Vec<u8> = values
            .iter()
            .map(|&v| {
                let normalized = ((v - min) / range).clamp(0.0, 1.0);
                (normalized * 255.0).round() as u8
            })
            .collect();

        Ok((quantized, min, max))
    }

    /// Scalar fallback implementation for 16-bit
    fn quantize_u16_scalar(&self, values: &[f32]) -> Result<(Vec<u16>, f32, f32)> {
        let min = values.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let range = if max > min { max - min } else { 1.0 };

        let quantized: Vec<u16> = values
            .iter()
            .map(|&v| {
                let normalized = ((v - min) / range).clamp(0.0, 1.0);
                (normalized * 65535.0).round() as u16
            })
            .collect();

        Ok((quantized, min, max))
    }

    /// AVX-512 implementation
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn quantize_u8_avx512(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        // AVX512 requires unstable features, use AVX2 implementation instead
        self.quantize_u8_avx2(values)
        /*
        #[cfg(target_feature = "avx512f")]
        unsafe {
            use std::arch::x86_64::*;

            // Find min/max using AVX-512
            let mut min_vec = _mm512_set1_ps(f32::INFINITY);
            let mut max_vec = _mm512_set1_ps(f32::NEG_INFINITY);

            let chunks = values.chunks_exact(16);
            let remainder = chunks.remainder();

            for chunk in chunks {
                let v = _mm512_loadu_ps(chunk.as_ptr());
                min_vec = _mm512_min_ps(min_vec, v);
                max_vec = _mm512_max_ps(max_vec, v);
            }

            // Reduce vectors to scalars
            let min = self.reduce_min_avx512(min_vec);
            let max = self.reduce_max_avx512(max_vec);

            // Handle remainder
            let min = remainder.iter().cloned().fold(min, f32::min);
            let max = remainder.iter().cloned().fold(max, f32::max);

            let range = if max > min { max - min } else { 1.0 };
            let scale = _mm512_set1_ps(255.0 / range);
            let min_vec = _mm512_set1_ps(min);

            let mut quantized = Vec::with_capacity(values.len());

            // Quantize using AVX-512
            for chunk in values.chunks_exact(16) {
                let v = _mm512_loadu_ps(chunk.as_ptr());
                let normalized = _mm512_mul_ps(_mm512_sub_ps(v, min_vec), scale);

                // Convert to u8 with saturation
                let i32_vals = _mm512_cvtps_epi32(normalized);
                let packed = _mm512_packus_epi32(i32_vals, i32_vals);

                // Store results
                let mut temp = [0u8; 16];
                _mm_storeu_si128(temp.as_mut_ptr() as *mut __m128i,
                                 _mm512_castsi512_si128(packed));
                quantized.extend_from_slice(&temp[..chunk.len()]);
            }

            // Handle remainder
            for &v in remainder {
                let normalized = ((v - min) / range).clamp(0.0, 1.0);
                quantized.push((normalized * 255.0).round() as u8);
            }

            Ok((quantized, min, max))
        }

        #[cfg(not(target_feature = "avx512f"))]
        self.quantize_u8_scalar(values)
        */
    }

    /// AVX2 implementation
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn quantize_u8_avx2(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        use crate::core::hardware_capabilities::try_get_hardware_capabilities;
        if let Some(caps) = try_get_hardware_capabilities() {
            if caps.cpu.features.avx2_support {
                unsafe {
                    use std::arch::x86_64::*;

                    // Find min/max using AVX2
                    let mut min_vec = _mm256_set1_ps(f32::INFINITY);
                    let mut max_vec = _mm256_set1_ps(f32::NEG_INFINITY);

                    let chunks = values.chunks_exact(8);
                    let remainder = chunks.remainder();

                    for chunk in chunks {
                        let v = _mm256_loadu_ps(chunk.as_ptr());
                        min_vec = _mm256_min_ps(min_vec, v);
                        max_vec = _mm256_max_ps(max_vec, v);
                    }

                    // Reduce vectors to scalars
                    let min = self.reduce_min_avx2(min_vec);
                    let max = self.reduce_max_avx2(max_vec);

                    // Handle remainder
                    let min = remainder.iter().cloned().fold(min, f32::min);
                    let max = remainder.iter().cloned().fold(max, f32::max);

                    let range = if max > min { max - min } else { 1.0 };
                    let scale = _mm256_set1_ps(255.0 / range);
                    let min_vec = _mm256_set1_ps(min);

                    let mut quantized = Vec::with_capacity(values.len());

                    // Quantize using AVX2
                    for chunk in values.chunks_exact(8) {
                        let v = _mm256_loadu_ps(chunk.as_ptr());
                        let normalized = _mm256_mul_ps(_mm256_sub_ps(v, min_vec), scale);

                        // Convert to i32 then pack to u8
                        let i32_vals = _mm256_cvtps_epi32(normalized);
                        let packed = _mm256_packus_epi32(i32_vals, i32_vals);
                        let packed = _mm256_packus_epi16(packed, packed);

                        // Extract lower 8 bytes
                        let lower = _mm256_extracti128_si256(packed, 0);
                        let mut temp = [0u8; 16];
                        _mm_storeu_si128(temp.as_mut_ptr() as *mut __m128i, lower);
                        quantized.extend_from_slice(&temp[..8]);
                    }

                    // Handle remainder
                    for &v in remainder {
                        let normalized = ((v - min) / range).clamp(0.0, 1.0);
                        quantized.push((normalized * 255.0).round() as u8);
                    }

                    Ok((quantized, min, max))
                }
            } else {
                self.quantize_u8_scalar(values)
            }
        } else {
            self.quantize_u8_scalar(values)
        }
    }

    /// SSE implementation
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn quantize_u8_sse(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        use crate::core::hardware_capabilities::try_get_hardware_capabilities;
        if let Some(caps) = try_get_hardware_capabilities() {
            if caps.cpu.features.sse42_support {
                unsafe {
                    use std::arch::x86_64::*;

                    // Find min/max using SSE
                    let mut min_vec = _mm_set1_ps(f32::INFINITY);
                    let mut max_vec = _mm_set1_ps(f32::NEG_INFINITY);

                    let chunks = values.chunks_exact(4);
                    let remainder = chunks.remainder();

                    for chunk in chunks {
                        let v = _mm_loadu_ps(chunk.as_ptr());
                        min_vec = _mm_min_ps(min_vec, v);
                        max_vec = _mm_max_ps(max_vec, v);
                    }

                    // Reduce vectors to scalars
                    let min = self.reduce_min_sse(min_vec);
                    let max = self.reduce_max_sse(max_vec);

                    // Handle remainder
                    let min = remainder.iter().cloned().fold(min, f32::min);
                    let max = remainder.iter().cloned().fold(max, f32::max);

                    let range = if max > min { max - min } else { 1.0 };
                    let scale = _mm_set1_ps(255.0 / range);
                    let min_vec = _mm_set1_ps(min);

                    let mut quantized = Vec::with_capacity(values.len());

                    // Quantize using SSE
                    for chunk in values.chunks_exact(4) {
                        let v = _mm_loadu_ps(chunk.as_ptr());
                        let normalized = _mm_mul_ps(_mm_sub_ps(v, min_vec), scale);

                        // Convert to integers
                        let i32_vals = _mm_cvtps_epi32(normalized);
                        let packed = _mm_packus_epi32(i32_vals, i32_vals);
                        let packed = _mm_packus_epi16(packed, packed);

                        // Store results
                        let mut temp = [0u8; 16];
                        _mm_storeu_si128(temp.as_mut_ptr() as *mut __m128i, packed);
                        quantized.extend_from_slice(&temp[..4]);
                    }

                    // Handle remainder
                    for &v in remainder {
                        let normalized = ((v - min) / range).clamp(0.0, 1.0);
                        quantized.push((normalized * 255.0).round() as u8);
                    }

                    Ok((quantized, min, max))
                }
            } else {
                self.quantize_u8_scalar(values)
            }
        } else {
            self.quantize_u8_scalar(values)
        }
    }

    /// ARM NEON implementation
    #[cfg(target_arch = "aarch64")]
    fn quantize_u8_neon(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        use std::arch::aarch64::*;

        unsafe {
            // Find min/max using NEON
            let mut min_vec = vdupq_n_f32(f32::INFINITY);
            let mut max_vec = vdupq_n_f32(f32::NEG_INFINITY);

            let chunks = values.chunks_exact(4);
            let remainder = chunks.remainder();

            for chunk in chunks {
                let v = vld1q_f32(chunk.as_ptr());
                min_vec = vminq_f32(min_vec, v);
                max_vec = vmaxq_f32(max_vec, v);
            }

            // Reduce vectors to scalars
            let min = vminvq_f32(min_vec);
            let max = vmaxvq_f32(max_vec);

            // Handle remainder
            let min = remainder.iter().cloned().fold(min, f32::min);
            let max = remainder.iter().cloned().fold(max, f32::max);

            let range = if max > min { max - min } else { 1.0 };
            let scale = vdupq_n_f32(255.0 / range);
            let min_vec = vdupq_n_f32(min);

            let mut quantized = Vec::with_capacity(values.len());

            // Quantize using NEON
            for chunk in values.chunks_exact(4) {
                let v = vld1q_f32(chunk.as_ptr());
                let normalized = vmulq_f32(vsubq_f32(v, min_vec), scale);

                // Convert to u32 then to u8
                let u32_vals = vcvtq_u32_f32(normalized);
                let u16_vals = vqmovn_u32(u32_vals);
                let u8_vals = vqmovn_u16(vcombine_u16(u16_vals, u16_vals));

                // Store results
                let mut temp = [0u8; 8];
                vst1_u8(temp.as_mut_ptr(), u8_vals);
                quantized.extend_from_slice(&temp[..4]);
            }

            // Handle remainder
            for &v in remainder {
                let normalized = ((v - min) / range).clamp(0.0, 1.0);
                quantized.push((normalized * 255.0).round() as u8);
            }

            Ok((quantized, min, max))
        }
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn quantize_u8_neon(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        self.quantize_u8_scalar(values)
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn quantize_u8_avx512(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        self.quantize_u8_scalar(values)
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn quantize_u8_avx2(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        self.quantize_u8_scalar(values)
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    fn quantize_u8_sse(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        self.quantize_u8_scalar(values)
    }

    // Helper functions for SIMD reductions
    // AVX512 reduction functions commented out - require unstable features
    /*
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe fn reduce_min_avx512(&self, v: std::arch::x86_64::__m512) -> f32 {
        use std::arch::x86_64::*;
        // Manual reduction since _mm512_reduce_min_ps is unstable
        // Extract 256-bit halves
        let low = _mm512_extractf32x8_ps(v, 0);
        let high = _mm512_extractf32x8_ps(v, 1);
        let min256 = _mm256_min_ps(low, high);
        // Continue reduction using AVX2
        self.reduce_min_avx2(min256)
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe fn reduce_max_avx512(&self, v: std::arch::x86_64::__m512) -> f32 {
        use std::arch::x86_64::*;
        // Manual reduction since _mm512_reduce_max_ps is unstable
        // Extract 256-bit halves
        let low = _mm512_extractf32x8_ps(v, 0);
        let high = _mm512_extractf32x8_ps(v, 1);
        let max256 = _mm256_max_ps(low, high);
        // Continue reduction using AVX2
        self.reduce_max_avx2(max256)
    }
    */

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe fn reduce_min_avx2(&self, v: std::arch::x86_64::__m256) -> f32 {
        unsafe {
            use std::arch::x86_64::*;
            // Reduce 256-bit to 128-bit
            let low = _mm256_extractf128_ps(v, 0);
            let high = _mm256_extractf128_ps(v, 1);
            let min128 = _mm_min_ps(low, high);

            // Reduce 128-bit to scalar
            self.reduce_min_sse(min128)
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe fn reduce_max_avx2(&self, v: std::arch::x86_64::__m256) -> f32 {
        unsafe {
            use std::arch::x86_64::*;
            // Reduce 256-bit to 128-bit
            let low = _mm256_extractf128_ps(v, 0);
            let high = _mm256_extractf128_ps(v, 1);
            let max128 = _mm_max_ps(low, high);

            // Reduce 128-bit to scalar
            self.reduce_max_sse(max128)
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe fn reduce_min_sse(&self, v: std::arch::x86_64::__m128) -> f32 {
        unsafe {
            use std::arch::x86_64::*;
            let shuf = _mm_shuffle_ps(v, v, 0b00001110);
            let min1 = _mm_min_ps(v, shuf);
            let shuf = _mm_shuffle_ps(min1, min1, 0b00000001);
            let min2 = _mm_min_ps(min1, shuf);
            _mm_cvtss_f32(min2)
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe fn reduce_max_sse(&self, v: std::arch::x86_64::__m128) -> f32 {
        unsafe {
            use std::arch::x86_64::*;
            let shuf = _mm_shuffle_ps(v, v, 0b00001110);
            let max1 = _mm_max_ps(v, shuf);
            let shuf = _mm_shuffle_ps(max1, max1, 0b00000001);
            let max2 = _mm_max_ps(max1, shuf);
            _mm_cvtss_f32(max2)
        }
    }

    // GPU implementations (stubs for now)
    #[cfg(feature = "gpu")]
    fn quantize_u8_cuda(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        // TODO: Implement CUDA quantization
        tracing::warn!("CUDA quantization not yet implemented, falling back to scalar");
        self.quantize_u8_scalar(values)
    }

    #[cfg(feature = "gpu")]
    fn quantize_u8_rocm(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        // TODO: Implement ROCm quantization
        tracing::warn!("ROCm quantization not yet implemented, falling back to scalar");
        self.quantize_u8_scalar(values)
    }

    #[cfg(feature = "gpu")]
    fn quantize_u8_mps(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        // TODO: Implement Metal Performance Shaders quantization
        tracing::warn!("MPS quantization not yet implemented, falling back to scalar");
        self.quantize_u8_scalar(values)
    }

    #[cfg(feature = "gpu")]
    fn quantize_u8_opencl(&self, values: &[f32]) -> Result<(Vec<u8>, f32, f32)> {
        // TODO: Implement OpenCL quantization
        tracing::warn!("OpenCL quantization not yet implemented, falling back to scalar");
        self.quantize_u8_scalar(values)
    }
}

impl Default for AcceleratedQuantization {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accelerated_quantization() {
        let accel = AcceleratedQuantization::new();
        let values = vec![0.1, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];

        let (quantized, min, max) = accel.quantize_u8_accelerated(&values).unwrap();

        assert_eq!(quantized.len(), values.len());
        assert!(min <= 0.1);
        assert!(max >= 4.0);

        // Verify quantization accuracy
        let range = max - min;
        for (i, &q) in quantized.iter().enumerate() {
            let reconstructed = min + (q as f32 / 255.0) * range;
            let error = (reconstructed - values[i]).abs();
            assert!(error < range / 255.0 * 1.1); // Allow 10% tolerance
        }
    }
}
