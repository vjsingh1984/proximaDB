// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Baseline encoder - Pure Rust implementation (always available)
//!
//! TODO: Phase 2.1 - Extract encoding functions from old ProximaEncoder

use crate::storage::engines::core::ops::proximacodec::traits::RawEncoder;
use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;
use anyhow::Result;

use super::functions;

/// Baseline encoder - Pure Rust, no SIMD, always available
pub struct BaselineEncoder;

impl RawEncoder for BaselineEncoder {
    fn name(&self) -> &'static str {
        "baseline"
    }

    fn supports(&self, scheme: &ProximaScheme) -> bool {
        // Baseline supports ALL schemes (will implement progressively)
        // Currently implemented: Delta, BitPacked, FrameOfReference, SparseBitmap, SparseCOO, RunLength, PForDelta, Zigzag, DoubleDelta
        matches!(
            scheme,
            ProximaScheme::Delta { .. }
            | ProximaScheme::BitPacked { .. }
            | ProximaScheme::FrameOfReference { .. }
            | ProximaScheme::SparseBitmap
            | ProximaScheme::SparseCOO
            | ProximaScheme::RunLength
            | ProximaScheme::PForDelta { .. }
            | ProximaScheme::Zigzag { .. }
            | ProximaScheme::DoubleDelta { .. }
            | ProximaScheme::Gorilla
            | ProximaScheme::VByte
            | ProximaScheme::Dictionary
            | ProximaScheme::Simple8b
            | ProximaScheme::Adaptive
        )
    }

    fn encode_f32(&self, values: &[f32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                functions::delta::encode_f32(values, *base)
            }
            ProximaScheme::BitPacked { bits } => {
                functions::bitpack::encode_f32(values, *bits)
            }
            ProximaScheme::FrameOfReference { reference, .. } => {
                functions::frame_of_ref::encode_f32(values, *reference)
            }
            ProximaScheme::SparseBitmap => {
                functions::sparse_bitmap::encode_f32(values)
            }
            ProximaScheme::SparseCOO => {
                functions::sparse_coo::encode_f32(values)
            }
            ProximaScheme::RunLength => {
                functions::run_length::encode_f32(values)
            }
            ProximaScheme::PForDelta { base, .. } => {
                functions::pfor_delta::encode_f32(values, *base)
            }
            ProximaScheme::Zigzag { bits } => {
                functions::zigzag::encode_f32(values, *bits)
            }
            ProximaScheme::DoubleDelta { .. } => {
                functions::double_delta::encode_f32(values)
            }
            ProximaScheme::Gorilla => {
                functions::gorilla::encode_f32(values)
            }
            ProximaScheme::VByte => {
                functions::vbyte::encode_f32(values)
            }
            ProximaScheme::Dictionary => {
                functions::dictionary::encode_f32(values)
            }
            ProximaScheme::Simple8b => {
                functions::simple8b::encode_f32(values)
            }
            ProximaScheme::Adaptive => {
                functions::adaptive::encode_f32(values)
            }
            _ => {
                Err(anyhow::anyhow!(
                    "Scheme {} not yet implemented for f32",
                    scheme.name()
                ))
            }
        }
    }

    fn encode_i64(&self, values: &[i64], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                functions::delta::encode_i64(values, *base)
            }
            ProximaScheme::BitPacked { bits } => {
                functions::bitpack::encode_i64(values, *bits)
            }
            ProximaScheme::FrameOfReference { reference, .. } => {
                functions::frame_of_ref::encode_i64(values, *reference)
            }
            ProximaScheme::SparseBitmap => {
                functions::sparse_bitmap::encode_i64(values)
            }
            ProximaScheme::SparseCOO => {
                functions::sparse_coo::encode_i64(values)
            }
            ProximaScheme::RunLength => {
                functions::run_length::encode_i64(values)
            }
            ProximaScheme::PForDelta { base, .. } => {
                functions::pfor_delta::encode_i64(values, *base)
            }
            ProximaScheme::Zigzag { bits } => {
                functions::zigzag::encode_i64(values, *bits)
            }
            ProximaScheme::DoubleDelta { .. } => {
                functions::double_delta::encode_i64(values)
            }
            ProximaScheme::Gorilla => {
                functions::gorilla::encode_i64(values)
            }
            ProximaScheme::VByte => {
                functions::vbyte::encode_i64(values)
            }
            ProximaScheme::Dictionary => {
                functions::dictionary::encode_i64(values)
            }
            ProximaScheme::Simple8b => {
                functions::simple8b::encode_i64(values)
            }
            ProximaScheme::Adaptive => {
                functions::adaptive::encode_i64(values)
            }

            // TODO: Implement remaining schemes
            _ => {
                Err(anyhow::anyhow!(
                    "Scheme {} not yet implemented for i64",
                    scheme.name()
                ))
            }
        }
    }

    fn encode_i32(&self, values: &[i32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        match scheme {
            ProximaScheme::Delta { base } => {
                functions::delta::encode_i32(values, *base)
            }
            ProximaScheme::BitPacked { bits } => {
                functions::bitpack::encode_i32(values, *bits)
            }
            ProximaScheme::FrameOfReference { reference, .. } => {
                functions::frame_of_ref::encode_i32(values, *reference)
            }
            ProximaScheme::SparseBitmap => {
                functions::sparse_bitmap::encode_i32(values)
            }
            ProximaScheme::SparseCOO => {
                functions::sparse_coo::encode_i32(values)
            }
            ProximaScheme::RunLength => {
                functions::run_length::encode_i32(values)
            }
            ProximaScheme::PForDelta { base, .. } => {
                functions::pfor_delta::encode_i32(values, *base)
            }
            ProximaScheme::Zigzag { bits } => {
                functions::zigzag::encode_i32(values, *bits)
            }
            ProximaScheme::DoubleDelta { .. } => {
                functions::double_delta::encode_i32(values)
            }
            ProximaScheme::Gorilla => {
                functions::gorilla::encode_i32(values)
            }
            ProximaScheme::VByte => {
                functions::vbyte::encode_i32(values)
            }
            ProximaScheme::Dictionary => {
                functions::dictionary::encode_i32(values)
            }
            ProximaScheme::Simple8b => {
                functions::simple8b::encode_i32(values)
            }
            ProximaScheme::Adaptive => {
                functions::adaptive::encode_i32(values)
            }

            // TODO: Implement remaining schemes
            _ => {
                Err(anyhow::anyhow!(
                    "Scheme {} not yet implemented for i32",
                    scheme.name()
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_supports() {
        let encoder = BaselineEncoder;

        // Should support all implemented schemes
        assert!(encoder.supports(&ProximaScheme::Delta { base: 0 }));
        assert!(encoder.supports(&ProximaScheme::BitPacked { bits: 16 }));
        assert!(encoder.supports(&ProximaScheme::FrameOfReference { reference: 0, bits: 16 }));
        assert!(encoder.supports(&ProximaScheme::SparseBitmap));
        assert!(encoder.supports(&ProximaScheme::SparseCOO));
        assert!(encoder.supports(&ProximaScheme::RunLength));
        assert!(encoder.supports(&ProximaScheme::PForDelta { majority_bits: 16, base: 0 }));
        assert!(encoder.supports(&ProximaScheme::Zigzag { bits: 32 }));
        assert!(encoder.supports(&ProximaScheme::DoubleDelta { first_value: 0, first_delta: 0 }));
        assert!(encoder.supports(&ProximaScheme::Gorilla));
        assert!(encoder.supports(&ProximaScheme::VByte));
        assert!(encoder.supports(&ProximaScheme::Dictionary));
        assert!(encoder.supports(&ProximaScheme::Simple8b));
        assert!(encoder.supports(&ProximaScheme::Adaptive));
    }

    #[test]
    fn test_encode_f32_delta() {
        let encoder = BaselineEncoder;
        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let scheme = ProximaScheme::Delta { base: 0 };

        let result = encoder.encode_f32(&values, &scheme);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_encode_i64_delta() {
        let encoder = BaselineEncoder;
        let values = vec![100i64, 200, 300, 400];
        let scheme = ProximaScheme::Delta { base: 0 };

        let result = encoder.encode_i64(&values, &scheme);
        assert!(result.is_ok());
    }

    #[test]
    fn test_encode_f32_bitpacked() {
        let encoder = BaselineEncoder;
        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let scheme = ProximaScheme::BitPacked { bits: 32 }; // Full precision

        let result = encoder.encode_f32(&values, &scheme);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_encode_i64_bitpacked() {
        let encoder = BaselineEncoder;
        let values = vec![100i64, 200, 300, 400];
        let scheme = ProximaScheme::BitPacked { bits: 16 }; // 16-bit packing

        let result = encoder.encode_i64(&values, &scheme);
        assert!(result.is_ok());
    }

    #[test]
    fn test_encode_i32_bitpacked() {
        let encoder = BaselineEncoder;
        let values = vec![1i32, 2, 3, 4, 5];
        let scheme = ProximaScheme::BitPacked { bits: 8 }; // 8-bit packing

        let result = encoder.encode_i32(&values, &scheme);
        assert!(result.is_ok());
    }

    #[test]
    fn test_encode_f32_for() {
        let encoder = BaselineEncoder;
        let values = vec![100.0f32, 101.0, 102.0, 103.0];
        let scheme = ProximaScheme::FrameOfReference {
            reference: 100.0f32.to_bits() as i64,
            bits: 32,
        };

        let result = encoder.encode_f32(&values, &scheme);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_encode_i64_for() {
        let encoder = BaselineEncoder;
        let values = vec![1000i64, 1001, 1002, 1003];
        let scheme = ProximaScheme::FrameOfReference { reference: 1000, bits: 16 };

        let result = encoder.encode_i64(&values, &scheme);
        assert!(result.is_ok());
    }

    #[test]
    fn test_encode_i32_for() {
        let encoder = BaselineEncoder;
        let values = vec![500i32, 501, 502, 503];
        let scheme = ProximaScheme::FrameOfReference { reference: 500, bits: 16 };

        let result = encoder.encode_i32(&values, &scheme);
        assert!(result.is_ok());
    }
}
