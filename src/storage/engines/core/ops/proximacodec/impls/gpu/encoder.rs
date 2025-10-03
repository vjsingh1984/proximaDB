// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU Encoder - Hardware-accelerated encoding using GPU compute
//!
//! This encoder uses GPU parallel processing to accelerate encoding operations.
//! It automatically detects the best available GPU backend (CUDA on Linux,
//! ROCm on AMD, MPS on Apple Silicon) and falls back to SIMD if GPU is unavailable.

use anyhow::Result;
use tracing::{debug, trace};

use crate::storage::engines::core::ops::proximacodec::traits::RawEncoder;
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;
use crate::storage::engines::core::ops::proximacodec::simd::{
    get_simd_backend,
    simd_delta_encode_f32,
    simd_bitpack_encode_f32,
    simd_frame_of_reference_encode_f32,
    simd_zigzag_encode_f32,
    simd_pfor_delta_encode_f32,
};
use crate::core::hardware_capabilities::HardwareBackend;

// Import GPU kernels
#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
use super::kernels::cuda;

#[cfg(all(feature = "gpu", target_os = "linux"))]
use super::kernels::rocm;

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
use super::kernels::metal;

#[cfg(feature = "gpu")]
use super::kernels::opencl;

/// GPU-accelerated encoder
///
/// Supports:
/// - Delta encoding (GPU/CUDA/ROCm/MPS)
/// - BitPacked encoding (GPU/CUDA/ROCm/MPS)
/// - FrameOfReference encoding (GPU/CUDA/ROCm/MPS)
/// - Zigzag encoding (GPU/CUDA/ROCm/MPS)
/// - PForDelta encoding (GPU/CUDA/ROCm/MPS)
///
/// Falls back to SIMD implementation if GPU is unavailable.
pub struct GpuEncoder;

impl GpuEncoder {
    /// Dispatch Delta encoding to appropriate GPU backend
    fn gpu_delta_encode(&self, values: &[f32], base: f32, backend: &HardwareBackend) -> Result<Vec<i64>> {
        match backend {
            #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
            HardwareBackend::CUDA => cuda::cuda_delta_encode_f32(values, base),

            #[cfg(all(feature = "gpu", target_os = "linux"))]
            HardwareBackend::ROCm => rocm::rocm_delta_encode_f32(values, base),

            #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
            HardwareBackend::MPS => metal::metal_delta_encode_f32(values, base),

            #[cfg(feature = "gpu")]
            HardwareBackend::OpenCL => opencl::opencl_delta_encode_f32(values, base),

            _ => {
                // Fall back to SIMD if GPU kernel not available
                trace!("⚠️  [GPU] Backend {:?} not available, falling back to SIMD", backend);
                simd_delta_encode_f32(values, base)
            }
        }
    }

    /// Dispatch BitPacked encoding to appropriate GPU backend
    fn gpu_bitpack_encode(&self, values: &[f32], bits: u8, backend: &HardwareBackend) -> Result<Vec<u8>> {
        match backend {
            #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
            HardwareBackend::CUDA => cuda::cuda_bitpack_encode_f32(values, bits),

            #[cfg(all(feature = "gpu", target_os = "linux"))]
            HardwareBackend::ROCm => rocm::rocm_bitpack_encode_f32(values, bits),

            #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
            HardwareBackend::MPS => metal::metal_bitpack_encode_f32(values, bits),

            #[cfg(feature = "gpu")]
            HardwareBackend::OpenCL => opencl::opencl_bitpack_encode_f32(values, bits),

            _ => {
                trace!("⚠️  [GPU] Backend {:?} not available, falling back to SIMD", backend);
                simd_bitpack_encode_f32(values, bits)
            }
        }
    }

    /// Dispatch FrameOfReference encoding to appropriate GPU backend
    fn gpu_frame_of_reference_encode(&self, values: &[f32], reference: i64, bits: u8, backend: &HardwareBackend) -> Result<Vec<u8>> {
        match backend {
            #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
            HardwareBackend::CUDA => cuda::cuda_frame_of_reference_encode_f32(values, reference, bits),

            #[cfg(all(feature = "gpu", target_os = "linux"))]
            HardwareBackend::ROCm => rocm::rocm_frame_of_reference_encode_f32(values, reference, bits),

            #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
            HardwareBackend::MPS => metal::metal_frame_of_reference_encode_f32(values, reference, bits),

            #[cfg(feature = "gpu")]
            HardwareBackend::OpenCL => opencl::opencl_frame_of_reference_encode_f32(values, reference, bits),

            _ => {
                trace!("⚠️  [GPU] Backend {:?} not available, falling back to SIMD", backend);
                simd_frame_of_reference_encode_f32(values, reference, bits)
            }
        }
    }

    /// Dispatch Zigzag encoding to appropriate GPU backend
    fn gpu_zigzag_encode(&self, values: &[f32], bits: u8, backend: &HardwareBackend) -> Result<Vec<u8>> {
        match backend {
            #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
            HardwareBackend::CUDA => cuda::cuda_zigzag_encode_f32(values, bits),

            #[cfg(all(feature = "gpu", target_os = "linux"))]
            HardwareBackend::ROCm => rocm::rocm_zigzag_encode_f32(values, bits),

            #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
            HardwareBackend::MPS => metal::metal_zigzag_encode_f32(values, bits),

            #[cfg(feature = "gpu")]
            HardwareBackend::OpenCL => opencl::opencl_zigzag_encode_f32(values, bits),

            _ => {
                trace!("⚠️  [GPU] Backend {:?} not available, falling back to SIMD", backend);
                simd_zigzag_encode_f32(values, bits)
            }
        }
    }

    /// Dispatch PForDelta encoding to appropriate GPU backend
    fn gpu_pfor_delta_encode(&self, values: &[f32], majority_bits: u8, base: i64, backend: &HardwareBackend) -> Result<Vec<u8>> {
        match backend {
            #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
            HardwareBackend::CUDA => cuda::cuda_pfor_delta_encode_f32(values, majority_bits, base),

            #[cfg(all(feature = "gpu", target_os = "linux"))]
            HardwareBackend::ROCm => rocm::rocm_pfor_delta_encode_f32(values, majority_bits, base),

            #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
            HardwareBackend::MPS => metal::metal_pfor_delta_encode_f32(values, majority_bits, base),

            #[cfg(feature = "gpu")]
            HardwareBackend::OpenCL => opencl::opencl_pfor_delta_encode_f32(values, majority_bits, base),

            _ => {
                trace!("⚠️  [GPU] Backend {:?} not available, falling back to SIMD", backend);
                simd_pfor_delta_encode_f32(values, majority_bits, base)
            }
        }
    }
}

impl RawEncoder for GpuEncoder {
    fn name(&self) -> &'static str {
        "GPU"
    }

    fn supports(&self, scheme: &ProximaScheme) -> bool {
        // Check if GPU is available
        let backend = get_simd_backend();
        if !backend.is_gpu() {
            return false;
        }

        // GPU supports: Delta, BitPacked, FrameOfReference, Zigzag, PForDelta
        matches!(
            scheme,
            ProximaScheme::Delta { .. }
                | ProximaScheme::BitPacked { .. }
                | ProximaScheme::FrameOfReference { .. }
                | ProximaScheme::Zigzag { .. }
                | ProximaScheme::PForDelta { .. }
        )
    }

    fn encode_f32(&self, values: &[f32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        let backend = get_simd_backend();

        if !backend.is_gpu() {
            anyhow::bail!("GPU encoder requires GPU backend, but got {:?}", backend);
        }

        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [GPU] Encoding {} values with Delta (base={})", values.len(), base);

                let deltas_i64 = self.gpu_delta_encode(values, *base as f32, &backend)?;

                // Wire format for f32: [base:4 bytes][deltas:4 bytes each (i32)]
                // Cast i64 deltas to i32 to save 50% storage (f32 deltas always fit in i32)
                let mut result = Vec::with_capacity(4 + deltas_i64.len() * 4);
                result.extend_from_slice(&(*base as i32).to_le_bytes());

                for &delta_i64 in &deltas_i64 {
                    result.extend_from_slice(&(delta_i64 as i32).to_le_bytes());
                }

                debug!("✅ [GPU] Delta encoded {} values → {} bytes (base:4 + {}×4 deltas)", values.len(), result.len(), deltas_i64.len());
                Ok(result)
            }

            ProximaScheme::BitPacked { bits } => {
                trace!("🚀 [GPU] Encoding {} values with BitPacked ({}b/val)", values.len(), bits);

                let packed = self.gpu_bitpack_encode(values, *bits, &backend)?;

                debug!("✅ [GPU] BitPacked encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            ProximaScheme::FrameOfReference { reference, bits } => {
                trace!("🚀 [GPU] Encoding {} values with FrameOfReference (ref={}, {}b/val)", values.len(), reference, bits);

                let packed = self.gpu_frame_of_reference_encode(values, *reference, *bits, &backend)?;

                debug!("✅ [GPU] FrameOfReference encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            ProximaScheme::Zigzag { bits } => {
                trace!("🚀 [GPU] Encoding {} values with Zigzag ({}b/val)", values.len(), bits);

                let packed = self.gpu_zigzag_encode(values, *bits, &backend)?;

                debug!("✅ [GPU] Zigzag encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            ProximaScheme::PForDelta { majority_bits, base } => {
                trace!("🚀 [GPU] Encoding {} values with PForDelta ({}b majority, base={})", values.len(), majority_bits, base);

                let packed = self.gpu_pfor_delta_encode(values, *majority_bits, *base, &backend)?;

                debug!("✅ [GPU] PForDelta encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            _ => {
                anyhow::bail!("GPU encoder does not support scheme: {}", scheme.name())
            }
        }
    }

    fn encode_i64(&self, values: &[i64], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [GPU] Encoding {} i64 values with Delta (base={})", values.len(), base);

                // Compute deltas using scalar (GPU i64 support can be added later)
                let base_val = *base;
                let deltas: Vec<i64> = values.iter().map(|&v| v - base_val).collect();

                // Wire format: [base:8 bytes][deltas:8 bytes each]
                let mut result = Vec::with_capacity(8 + deltas.len() * 8);
                result.extend_from_slice(&base.to_le_bytes());

                for &delta in &deltas {
                    result.extend_from_slice(&delta.to_le_bytes());
                }

                debug!("✅ [GPU] Delta encoded {} i64 values → {} bytes (base + {} deltas)", values.len(), result.len(), deltas.len());
                Ok(result)
            }

            _ => {
                anyhow::bail!("GPU encoder does not support scheme: {} for i64", scheme.name())
            }
        }
    }

    fn encode_i32(&self, values: &[i32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [GPU] Encoding {} i32 values with Delta (base={})", values.len(), base);

                // Compute deltas using scalar (GPU i32 support can be added later)
                let base_val = *base as i32;
                let deltas: Vec<i32> = values.iter().map(|&v| v - base_val).collect();

                // Wire format: [base:4 bytes][deltas:4 bytes each]
                let mut result = Vec::with_capacity(4 + deltas.len() * 4);
                result.extend_from_slice(&base_val.to_le_bytes());

                for &delta in &deltas {
                    result.extend_from_slice(&delta.to_le_bytes());
                }

                debug!("✅ [GPU] Delta encoded {} i32 values → {} bytes (base + {} deltas)", values.len(), result.len(), deltas.len());
                Ok(result)
            }

            _ => {
                anyhow::bail!("GPU encoder does not support scheme: {} for i32", scheme.name())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_encoder_supports() {
        let encoder = GpuEncoder;
        let backend = get_simd_backend();

        // Only test if GPU is available
        if !backend.is_gpu() {
            println!("⚠️  GPU not available, skipping GPU encoder tests");
            return;
        }

        // Should support: Delta, BitPacked, FrameOfReference, Zigzag, PForDelta
        assert!(encoder.supports(&ProximaScheme::Delta { base: 0 }));
        assert!(encoder.supports(&ProximaScheme::BitPacked { bits: 8 }));
        assert!(encoder.supports(&ProximaScheme::FrameOfReference { reference: 0, bits: 8 }));
        assert!(encoder.supports(&ProximaScheme::Zigzag { bits: 8 }));
        assert!(encoder.supports(&ProximaScheme::PForDelta { majority_bits: 8, base: 0 }));

        // Should not support other schemes
        assert!(!encoder.supports(&ProximaScheme::RunLength));
        assert!(!encoder.supports(&ProximaScheme::Dictionary));
        assert!(!encoder.supports(&ProximaScheme::Simple8b));
        assert!(!encoder.supports(&ProximaScheme::VByte));
    }

    #[test]
    fn test_gpu_delta_encode() {
        let encoder = GpuEncoder;
        let backend = get_simd_backend();

        // Only test if GPU is available
        if !backend.is_gpu() {
            println!("⚠️  GPU not available, skipping GPU delta encode test");
            return;
        }

        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let result = encoder.encode_f32(&values, &ProximaScheme::Delta { base: 0 });
        assert!(result.is_ok());

        let encoded = result.unwrap();
        // base:4 bytes + 4 values × 4 bytes per i32 = 4 + 16 = 20 bytes (50% savings vs i64!)
        assert_eq!(encoded.len(), 20);
    }
}
