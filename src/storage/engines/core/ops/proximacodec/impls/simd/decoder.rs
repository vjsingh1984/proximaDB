// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SIMD Decoder - Hardware-accelerated decoding using NEON/AVX2/AVX512
//!
//! This decoder uses SIMD intrinsics to accelerate decoding operations.
//! It automatically detects the best available SIMD backend (NEON on ARM,
//! AVX2/AVX512 on x86_64) and falls back to scalar if SIMD is unavailable.

use anyhow::Result;
use tracing::{debug, trace};

use crate::storage::engines::core::ops::proximacodec::traits::RawDecoder;
use crate::storage::engines::core::ops::proximacodec::types::{ProximaScheme, TypeId};
use crate::storage::engines::core::ops::proximacodec::simd::{
    get_simd_backend, simd_delta_decode_f32, simd_bitpack_decode_f32,
};

/// SIMD-accelerated decoder
///
/// Supports:
/// - Delta decoding (NEON/AVX2)
/// - BitPacked decoding (AVX2)
///
/// Falls back to baseline implementation for unsupported schemes.
pub struct SimdDecoder;

impl RawDecoder for SimdDecoder {
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

    fn decode_f32(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<f32>> {
        match scheme {
            ProximaScheme::Delta { .. } => {
                trace!("🚀 [SIMD] Decoding {} bytes with Delta (count={})", data.len(), count);

                // Wire format for f32: [base:4 bytes][deltas:4 bytes each (i32)]
                if data.len() < 4 {
                    anyhow::bail!("Data too short: {} bytes (need at least 4 for base)", data.len());
                }

                // Read base from first 4 bytes (f32 stored as i32 bits)
                let base_i32 = i32::from_le_bytes([
                    data[0], data[1], data[2], data[3],
                ]);
                let base_f32 = base_i32 as f32;

                // Deserialize i32 deltas from remaining bytes
                let deltas_data = &data[4..];
                if deltas_data.len() % 4 != 0 {
                    anyhow::bail!("Invalid deltas length: {} (must be multiple of 4)", deltas_data.len());
                }

                let num_deltas = deltas_data.len() / 4;
                let mut deltas = Vec::with_capacity(num_deltas);
                for chunk in deltas_data.chunks_exact(4) {
                    let delta_i32 = i32::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ]);
                    deltas.push(delta_i32 as i64); // Convert i32→i64 for simd_delta_decode_f32
                }

                // Decode using SIMD Delta with base from data (not wire format)
                let values = simd_delta_decode_f32(&deltas, base_f32)?;

                debug!("✅ [SIMD] Delta decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::BitPacked { bits } => {
                trace!("🚀 [SIMD] Decoding {} bytes with BitPacked ({}b/val, count={})", data.len(), bits, count);

                // Decode using SIMD BitPacked
                let values = simd_bitpack_decode_f32(data, *bits, count)?;

                debug!("✅ [SIMD] BitPacked decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::FrameOfReference { reference, bits } => {
                trace!("🚀 [SIMD] Decoding {} bytes with FrameOfReference (ref={}, {}b/val, count={})", data.len(), reference, bits, count);

                // Decode using SIMD FrameOfReference
                let values = crate::storage::engines::core::ops::proximacodec::simd::simd_frame_of_reference_decode_f32(data, *reference, *bits, count)?;

                debug!("✅ [SIMD] FrameOfReference decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::Zigzag { bits } => {
                trace!("🚀 [SIMD] Decoding {} bytes with Zigzag ({}b/val, count={})", data.len(), bits, count);

                // Decode using SIMD Zigzag
                let values = crate::storage::engines::core::ops::proximacodec::simd::simd_zigzag_decode_f32(data, *bits, count)?;

                debug!("✅ [SIMD] Zigzag decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            ProximaScheme::PForDelta { majority_bits, base } => {
                trace!("🚀 [SIMD] Decoding {} bytes with PForDelta ({}b majority, base={}, count={})", data.len(), majority_bits, base, count);

                // Decode using SIMD PForDelta
                let values = crate::storage::engines::core::ops::proximacodec::simd::simd_pfor_delta_decode_f32(data, *majority_bits, *base, count)?;

                debug!("✅ [SIMD] PForDelta decoded {} bytes → {} values", data.len(), values.len());
                Ok(values)
            }

            _ => {
                anyhow::bail!("SIMD decoder does not support scheme: {}", scheme.name())
            }
        }
    }

    fn decode_i64(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<i64>> {
        match scheme {
            ProximaScheme::Delta { .. } => {
                trace!("🚀 [SIMD] Decoding {} bytes as i64 with Delta (count={})", data.len(), count);

                // Wire format: [base:8 bytes][deltas:8 bytes each]
                if data.len() < 8 {
                    anyhow::bail!("Data too short: {} bytes (need at least 8 for base)", data.len());
                }

                // Read base from first 8 bytes
                let base_val = i64::from_le_bytes([
                    data[0], data[1], data[2], data[3],
                    data[4], data[5], data[6], data[7],
                ]);

                // Deserialize deltas from remaining bytes
                let deltas_data = &data[8..];
                if deltas_data.len() % 8 != 0 {
                    anyhow::bail!("Invalid deltas length: {} (must be multiple of 8)", deltas_data.len());
                }

                let num_deltas = deltas_data.len() / 8;
                let mut deltas = Vec::with_capacity(num_deltas);
                for chunk in deltas_data.chunks_exact(8) {
                    let delta = i64::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                        chunk[4], chunk[5], chunk[6], chunk[7],
                    ]);
                    deltas.push(delta);
                }

                // Reconstruct original values using scalar (SIMD i64 support can be added later)
                let values: Vec<i64> = deltas.iter().map(|&delta| delta + base_val).collect();

                debug!("✅ [SIMD] Delta decoded {} i64 values → {} values", data.len(), values.len());
                Ok(values)
            }

            _ => {
                anyhow::bail!("SIMD decoder does not support scheme: {} for i64", scheme.name())
            }
        }
    }

    fn decode_i32(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<i32>> {
        match scheme {
            ProximaScheme::Delta { .. } => {
                trace!("🚀 [SIMD] Decoding {} bytes as i32 with Delta (count={})", data.len(), count);

                // Wire format: [base:4 bytes][deltas:4 bytes each]
                if data.len() < 4 {
                    anyhow::bail!("Data too short: {} bytes (need at least 4 for base)", data.len());
                }

                // Read base from first 4 bytes
                let base_val = i32::from_le_bytes([
                    data[0], data[1], data[2], data[3],
                ]);

                // Deserialize deltas from remaining bytes
                let deltas_data = &data[4..];
                if deltas_data.len() % 4 != 0 {
                    anyhow::bail!("Invalid deltas length: {} (must be multiple of 4)", deltas_data.len());
                }

                let num_deltas = deltas_data.len() / 4;
                let mut deltas = Vec::with_capacity(num_deltas);
                for chunk in deltas_data.chunks_exact(4) {
                    let delta = i32::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ]);
                    deltas.push(delta);
                }

                // Reconstruct original values using scalar (SIMD i32 support can be added later)
                let values: Vec<i32> = deltas.iter().map(|&delta| delta + base_val).collect();

                debug!("✅ [SIMD] Delta decoded {} i32 values → {} values", data.len(), values.len());
                Ok(values)
            }

            _ => {
                anyhow::bail!("SIMD decoder does not support scheme: {} for i32", scheme.name())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simd_decoder_supports() {
        let decoder = SimdDecoder;

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
    fn test_simd_delta_decode() {
        use crate::storage::engines::core::ops::proximacodec::impls::simd::encoder::SimdEncoder;
        use crate::storage::engines::core::ops::proximacodec::traits::RawEncoder;

        let encoder = SimdEncoder;
        let decoder = SimdDecoder;
        let values = vec![1.0f32, 2.0, 3.0, 4.0];

        // Encode
        let encoded = encoder.encode_f32(&values, &ProximaScheme::Delta { base: 0 }).unwrap();

        // Decode (count = number of values)
        let decoded = decoder.decode_f32(&encoded, &ProximaScheme::Delta { base: 0 }, values.len()).unwrap();

        // Verify round-trip
        assert_eq!(values, decoded);
    }

    #[test]
    fn test_simd_bitpack_decode() {
        use crate::storage::engines::core::ops::proximacodec::impls::simd::encoder::SimdEncoder;
        use crate::storage::engines::core::ops::proximacodec::traits::RawEncoder;

        let encoder = SimdEncoder;
        let decoder = SimdDecoder;
        let values = vec![0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

        // Encode
        let encoded = encoder.encode_f32(&values, &ProximaScheme::BitPacked { bits: 8 }).unwrap();

        // Decode (count = number of values)
        let decoded = decoder.decode_f32(&encoded, &ProximaScheme::BitPacked { bits: 8 }, values.len()).unwrap();

        // Verify round-trip
        assert_eq!(values, decoded);
    }
}
