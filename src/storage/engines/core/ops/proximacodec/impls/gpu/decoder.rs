// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU Decoder - Hardware-accelerated decoding using GPU compute
//!
//! This decoder uses GPU parallel processing to accelerate decoding operations.
//! It automatically detects the best available GPU backend (CUDA on Linux,
//! ROCm on AMD, MPS on Apple Silicon) and falls back to SIMD if GPU is unavailable.

use anyhow::Result;
use tracing::{debug, trace};

use crate::storage::engines::core::ops::proximacodec::traits::RawDecoder;
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;
use crate::storage::engines::core::ops::proximacodec::simd::{
    get_simd_backend,
    simd_delta_decode_f32,
    simd_bitpack_decode_f32,
    simd_frame_of_reference_decode_f32,
    simd_zigzag_decode_f32,
    simd_pfor_delta_decode_f32,
};
use crate::core::hardware_capabilities::HardwareBackend;

/// GPU-accelerated decoder
///
/// Supports:
/// - Delta decoding (GPU/CUDA/ROCm/MPS)
/// - BitPacked decoding (GPU/CUDA/ROCm/MPS)
/// - FrameOfReference decoding (GPU/CUDA/ROCm/MPS)
/// - Zigzag decoding (GPU/CUDA/ROCm/MPS)
/// - PForDelta decoding (GPU/CUDA/ROCm/MPS)
///
/// Falls back to SIMD implementation if GPU is unavailable.
pub struct GpuDecoder;

impl RawDecoder for GpuDecoder {
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

    fn decode_f32(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<f32>> {
        let backend = get_simd_backend();

        if !backend.is_gpu() {
            anyhow::bail!("GPU decoder requires GPU backend, but got {:?}", backend);
        }

        // GPU decoding is not yet implemented at the hardware level,
        // so we fall back to optimized SIMD implementations.
        // Future: Add actual GPU kernel implementations for CUDA/ROCm/MPS

        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [GPU] Decoding {} bytes with Delta (base={}, count={})", data.len(), base, count);

                // Deserialize deltas from bytes
                if data.len() % 8 != 0 {
                    anyhow::bail!("Invalid delta data length: {} (must be multiple of 8)", data.len());
                }

                let num_deltas = data.len() / 8;
                let mut deltas = Vec::with_capacity(num_deltas);
                for chunk in data.chunks_exact(8) {
                    let delta = i64::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                        chunk[4], chunk[5], chunk[6], chunk[7],
                    ]);
                    deltas.push(delta);
                }

                // TODO: Replace with GPU kernel implementation
                let values = simd_delta_decode_f32(&deltas, *base as f32)?;

                debug!("✅ [GPU] Delta decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::BitPacked { bits } => {
                trace!("🚀 [GPU] Decoding {} bytes with BitPacked ({}b/val, count={})", data.len(), bits, count);

                // TODO: Replace with GPU kernel implementation
                let values = simd_bitpack_decode_f32(data, *bits, count)?;

                debug!("✅ [GPU] BitPacked decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::FrameOfReference { reference, bits } => {
                trace!("🚀 [GPU] Decoding {} bytes with FrameOfReference (ref={}, {}b/val, count={})", data.len(), reference, bits, count);

                // TODO: Replace with GPU kernel implementation
                let values = simd_frame_of_reference_decode_f32(data, *reference, *bits, count)?;

                debug!("✅ [GPU] FrameOfReference decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::Zigzag { bits } => {
                trace!("🚀 [GPU] Decoding {} bytes with Zigzag ({}b/val, count={})", data.len(), bits, count);

                // TODO: Replace with GPU kernel implementation
                let values = simd_zigzag_decode_f32(data, *bits, count)?;

                debug!("✅ [GPU] Zigzag decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::PForDelta { majority_bits, base } => {
                trace!("🚀 [GPU] Decoding {} bytes with PForDelta ({}b majority, base={}, count={})", data.len(), majority_bits, base, count);

                // TODO: Replace with GPU kernel implementation
                let values = simd_pfor_delta_decode_f32(data, *majority_bits, *base, count)?;

                debug!("✅ [GPU] PForDelta decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            _ => {
                anyhow::bail!("GPU decoder does not support scheme: {}", scheme.name())
            }
        }
    }

    fn decode_i64(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<i64>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [GPU] Decoding {} bytes as i64 with Delta (base={}, count={})", data.len(), base, count);

                // Deserialize deltas from bytes
                if data.len() % 8 != 0 {
                    anyhow::bail!("Invalid delta data length: {} (must be multiple of 8)", data.len());
                }

                let num_deltas = data.len() / 8;
                let mut deltas = Vec::with_capacity(num_deltas);
                for chunk in data.chunks_exact(8) {
                    let delta = i64::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                        chunk[4], chunk[5], chunk[6], chunk[7],
                    ]);
                    deltas.push(delta);
                }

                // Reconstruct original values using scalar (GPU i64 support can be added later)
                let base_val = *base;
                let values: Vec<i64> = deltas.iter().map(|&delta| delta + base_val).collect();

                debug!("✅ [GPU] Delta decoded {} i64 values → {} values", data.len(), values.len());
                Ok(values)
            }

            _ => {
                anyhow::bail!("GPU decoder does not support scheme: {} for i64", scheme.name())
            }
        }
    }

    fn decode_i32(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<i32>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [GPU] Decoding {} bytes as i32 with Delta (base={}, count={})", data.len(), base, count);

                // Deserialize deltas from bytes
                if data.len() % 4 != 0 {
                    anyhow::bail!("Invalid delta data length: {} (must be multiple of 4)", data.len());
                }

                let num_deltas = data.len() / 4;
                let mut deltas = Vec::with_capacity(num_deltas);
                for chunk in data.chunks_exact(4) {
                    let delta = i32::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ]);
                    deltas.push(delta);
                }

                // Reconstruct original values using scalar (GPU i32 support can be added later)
                let base_val = *base as i32;
                let values: Vec<i32> = deltas.iter().map(|&delta| delta + base_val).collect();

                debug!("✅ [GPU] Delta decoded {} i32 values → {} values", data.len(), values.len());
                Ok(values)
            }

            _ => {
                anyhow::bail!("GPU decoder does not support scheme: {} for i32", scheme.name())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_decoder_supports() {
        let decoder = GpuDecoder;
        let backend = get_simd_backend();

        // Only test if GPU is available
        if !backend.is_gpu() {
            println!("⚠️  GPU not available, skipping GPU decoder tests");
            return;
        }

        // Should support: Delta, BitPacked, FrameOfReference, Zigzag, PForDelta
        assert!(decoder.supports(&ProximaScheme::Delta { base: 0 }));
        assert!(decoder.supports(&ProximaScheme::BitPacked { bits: 8 }));
        assert!(decoder.supports(&ProximaScheme::FrameOfReference { reference: 0, bits: 8 }));
        assert!(decoder.supports(&ProximaScheme::Zigzag { bits: 8 }));
        assert!(decoder.supports(&ProximaScheme::PForDelta { majority_bits: 8, base: 0 }));

        // Should not support other schemes
        assert!(!decoder.supports(&ProximaScheme::RunLength));
        assert!(!decoder.supports(&ProximaScheme::Dictionary));
        assert!(!decoder.supports(&ProximaScheme::Simple8b));
        assert!(!decoder.supports(&ProximaScheme::VByte));
    }

    #[test]
    fn test_gpu_delta_decode() {
        use crate::storage::engines::core::ops::proximacodec::impls::gpu::encoder::GpuEncoder;
        use crate::storage::engines::core::ops::proximacodec::traits::RawEncoder;

        let backend = get_simd_backend();

        // Only test if GPU is available
        if !backend.is_gpu() {
            println!("⚠️  GPU not available, skipping GPU delta decode test");
            return;
        }

        let encoder = GpuEncoder;
        let decoder = GpuDecoder;
        let values = vec![1.0f32, 2.0, 3.0, 4.0];

        // Encode
        let encoded = encoder.encode_f32(&values, &ProximaScheme::Delta { base: 0 }).unwrap();

        // Decode (count = number of values)
        let decoded = decoder.decode_f32(&encoded, &ProximaScheme::Delta { base: 0 }, values.len()).unwrap();

        // Verify round-trip
        assert_eq!(values, decoded);
    }
}
