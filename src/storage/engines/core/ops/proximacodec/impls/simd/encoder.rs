// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SIMD Encoder - Hardware-accelerated encoding using NEON/AVX2/AVX512
//!
//! This encoder uses SIMD intrinsics to accelerate encoding operations.
//! It automatically detects the best available SIMD backend (NEON on ARM,
//! AVX2/AVX512 on x86_64) and falls back to scalar if SIMD is unavailable.

use anyhow::Result;
use tracing::{debug, trace};

use crate::storage::engines::core::ops::proximacodec::traits::RawEncoder;
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;
use crate::storage::engines::core::ops::proximacodec::simd::{
    get_simd_backend, simd_delta_encode_f32, simd_bitpack_encode_f32,
};

/// SIMD-accelerated encoder
///
/// Supports:
/// - Delta encoding (NEON/AVX2)
/// - BitPacked encoding (AVX2)
///
/// Falls back to baseline implementation for unsupported schemes.
pub struct SimdEncoder;

impl RawEncoder for SimdEncoder {
    fn name(&self) -> &'static str {
        "SIMD"
    }

    fn supports(&self, scheme: &ProximaScheme) -> bool {
        // Check if SIMD is available
        let backend = get_simd_backend();
        if !backend.has_acceleration() {
            return false;
        }

        // SIMD supports: Delta, BitPacked, FrameOfReference, Zigzag, PForDelta
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
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [SIMD] Encoding {} values with Delta (base={})", values.len(), base);

                // Encode using SIMD Delta
                let deltas = simd_delta_encode_f32(values, *base as f32)?;

                // Serialize deltas to bytes
                let mut result = Vec::with_capacity(deltas.len() * 8);
                for &delta in &deltas {
                    result.extend_from_slice(&delta.to_le_bytes());
                }

                debug!("✅ [SIMD] Delta encoded {} values → {} bytes", values.len(), result.len());
                Ok(result)
            }

            ProximaScheme::BitPacked { bits } => {
                trace!("🚀 [SIMD] Encoding {} values with BitPacked ({}b/val)", values.len(), bits);

                // Encode using SIMD BitPacked
                let packed = simd_bitpack_encode_f32(values, *bits)?;

                debug!("✅ [SIMD] BitPacked encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            ProximaScheme::FrameOfReference { reference, bits } => {
                trace!("🚀 [SIMD] Encoding {} values with FrameOfReference (ref={}, {}b/val)", values.len(), reference, bits);

                // Encode using SIMD FrameOfReference
                let packed = crate::storage::engines::core::ops::proximacodec::simd::simd_frame_of_reference_encode_f32(values, *reference, *bits)?;

                debug!("✅ [SIMD] FrameOfReference encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            ProximaScheme::Zigzag { bits } => {
                trace!("🚀 [SIMD] Encoding {} values with Zigzag ({}b/val)", values.len(), bits);

                // Encode using SIMD Zigzag
                let packed = crate::storage::engines::core::ops::proximacodec::simd::simd_zigzag_encode_f32(values, *bits)?;

                debug!("✅ [SIMD] Zigzag encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            ProximaScheme::PForDelta { majority_bits, base } => {
                trace!("🚀 [SIMD] Encoding {} values with PForDelta ({}b majority, base={})", values.len(), majority_bits, base);

                // Encode using SIMD PForDelta
                let packed = crate::storage::engines::core::ops::proximacodec::simd::simd_pfor_delta_encode_f32(values, *majority_bits, *base)?;

                debug!("✅ [SIMD] PForDelta encoded {} values → {} bytes", values.len(), packed.len());
                Ok(packed)
            }

            _ => {
                anyhow::bail!("SIMD encoder does not support scheme: {}", scheme.name())
            }
        }
    }

    fn encode_i64(&self, values: &[i64], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [SIMD] Encoding {} i64 values with Delta (base={})", values.len(), base);

                // Compute deltas using scalar (SIMD i64 support can be added later)
                let base_val = *base;
                let deltas: Vec<i64> = values.iter().map(|&v| v - base_val).collect();

                // Serialize deltas to bytes
                let mut result = Vec::with_capacity(deltas.len() * 8);
                for &delta in &deltas {
                    result.extend_from_slice(&delta.to_le_bytes());
                }

                debug!("✅ [SIMD] Delta encoded {} i64 values → {} bytes", values.len(), result.len());
                Ok(result)
            }

            _ => {
                anyhow::bail!("SIMD encoder does not support scheme: {} for i64", scheme.name())
            }
        }
    }

    fn encode_i32(&self, values: &[i32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                trace!("🚀 [SIMD] Encoding {} i32 values with Delta (base={})", values.len(), base);

                // Compute deltas using scalar (SIMD i32 support can be added later)
                let base_val = *base as i32;
                let deltas: Vec<i32> = values.iter().map(|&v| v - base_val).collect();

                // Serialize deltas to bytes
                let mut result = Vec::with_capacity(deltas.len() * 4);
                for &delta in &deltas {
                    result.extend_from_slice(&delta.to_le_bytes());
                }

                debug!("✅ [SIMD] Delta encoded {} i32 values → {} bytes", values.len(), result.len());
                Ok(result)
            }

            _ => {
                anyhow::bail!("SIMD encoder does not support scheme: {} for i32", scheme.name())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simd_encoder_supports() {
        let encoder = SimdEncoder;

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
    fn test_simd_delta_encode() {
        let encoder = SimdEncoder;
        let values = vec![1.0f32, 2.0, 3.0, 4.0];

        let result = encoder.encode_f32(&values, &ProximaScheme::Delta { base: 0 });
        assert!(result.is_ok());

        let encoded = result.unwrap();
        // 4 values × 8 bytes per i64 = 32 bytes
        assert_eq!(encoded.len(), 32);
    }

    #[test]
    fn test_simd_bitpack_encode() {
        let encoder = SimdEncoder;
        let values = vec![0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

        let result = encoder.encode_f32(&values, &ProximaScheme::BitPacked { bits: 8 });
        assert!(result.is_ok());

        let encoded = result.unwrap();
        // 8 values × 8 bits = 64 bits = 8 bytes
        assert_eq!(encoded.len(), 8);
    }
}
