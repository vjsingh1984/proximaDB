// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Raw encoder/decoder traits
//!
//! These traits define the interface for compression implementations.
//! Each implementation (Baseline, SIMD, GPU) provides raw compression
//! WITHOUT adding wire format headers.
//!
//! The ProximaCodec layer handles header management.

use super::types::{Decodable, Encodable, ProximaScheme};
use anyhow::Result;

/// Raw encoder trait - compresses data WITHOUT adding headers
///
/// Implementations:
/// - `BaselineEncoder`: Pure Rust, always available
/// - `SimdEncoder`: SIMD-accelerated (AVX2/AVX-512/NEON)
/// - `CudaEncoder`: GPU-accelerated (CUDA)
/// - `RocmEncoder`: GPU-accelerated (ROCm)
/// - `MetalEncoder`: GPU-accelerated (Metal on macOS)
///
/// All implementations must return **raw compressed data** only.
/// Wire format headers are added by ProximaCodec.
///
/// NOTE: This trait is object-safe (no generics) to allow dynamic dispatch.
/// Separate methods for each supported type (f32, i64, etc.).
pub trait RawEncoder: Send + Sync {
    /// Implementation name (for metrics/debugging)
    ///
    /// Examples: "baseline", "simd-avx2", "simd-avx512", "simd-neon", "cuda", "rocm", "metal"
    fn name(&self) -> &'static str;

    /// Check if this encoder supports the scheme
    ///
    /// Returns true if this encoder can encode data with the given scheme.
    /// The ProximaCodec registry tries encoders in priority order (GPU → SIMD → Baseline).
    fn supports(&self, scheme: &ProximaScheme) -> bool;

    /// Encode f32 values (NO HEADER)
    ///
    /// Returns raw compressed bytes for f32 data.
    fn encode_f32(&self, values: &[f32], scheme: &ProximaScheme) -> Result<Vec<u8>>;

    /// Encode i64 values (NO HEADER)
    ///
    /// Returns raw compressed bytes for i64 data.
    fn encode_i64(&self, values: &[i64], scheme: &ProximaScheme) -> Result<Vec<u8>>;

    /// Encode i32 values (NO HEADER)
    ///
    /// Returns raw compressed bytes for i32 data.
    fn encode_i32(&self, values: &[i32], scheme: &ProximaScheme) -> Result<Vec<u8>>;
}

/// Raw decoder trait - decompresses data WITHOUT reading headers
///
/// Implementations:
/// - `BaselineDecoder`: Pure Rust, always available
/// - `SimdDecoder`: SIMD-accelerated (AVX2/AVX-512/NEON)
/// - `CudaDecoder`: GPU-accelerated (CUDA)
/// - `RocmDecoder`: GPU-accelerated (ROCm)
/// - `MetalDecoder`: GPU-accelerated (Metal on macOS)
///
/// All implementations receive **raw compressed data** without headers.
/// Wire format headers are parsed by ProximaCodec before calling decode.
///
/// NOTE: This trait is object-safe (no generics) to allow dynamic dispatch.
/// Separate methods for each supported type (f32, i64, etc.).
pub trait RawDecoder: Send + Sync {
    /// Implementation name (for metrics/debugging)
    ///
    /// Examples: "baseline", "simd-avx2", "simd-avx512", "simd-neon", "cuda", "rocm", "metal"
    fn name(&self) -> &'static str;

    /// Check if this decoder supports the scheme
    ///
    /// Returns true if this decoder can decode data with the given scheme.
    /// The ProximaCodec registry tries decoders in priority order (GPU → SIMD → Baseline).
    fn supports(&self, scheme: &ProximaScheme) -> bool;

    /// Decode f32 values (NO HEADER)
    ///
    /// Decompresses raw bytes into f32 values.
    fn decode_f32(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<f32>>;

    /// Decode i64 values (NO HEADER)
    ///
    /// Decompresses raw bytes into i64 values.
    fn decode_i64(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<i64>>;

    /// Decode i32 values (NO HEADER)
    ///
    /// Decompresses raw bytes into i32 values.
    fn decode_i32(&self, data: &[u8], scheme: &ProximaScheme, count: usize) -> Result<Vec<i32>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock encoder for testing
    struct MockEncoder;

    impl RawEncoder for MockEncoder {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn supports(&self, scheme: &ProximaScheme) -> bool {
            matches!(scheme, ProximaScheme::Delta { .. })
        }

        fn encode_f32(&self, _values: &[f32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
            if !self.supports(scheme) {
                return Err(anyhow::anyhow!("Scheme not supported"));
            }
            Ok(vec![0xF3, 0x2F]) // Mock data
        }

        fn encode_i64(&self, _values: &[i64], scheme: &ProximaScheme) -> Result<Vec<u8>> {
            if !self.supports(scheme) {
                return Err(anyhow::anyhow!("Scheme not supported"));
            }
            Ok(vec![0x16, 0x4D]) // Mock data
        }

        fn encode_i32(&self, _values: &[i32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
            if !self.supports(scheme) {
                return Err(anyhow::anyhow!("Scheme not supported"));
            }
            Ok(vec![0x13, 0x2D]) // Mock data
        }
    }

    #[test]
    fn test_encoder_trait() {
        let encoder = MockEncoder;
        assert_eq!(encoder.name(), "mock");
        assert!(encoder.supports(&ProximaScheme::Delta { base: 0 }));
        assert!(!encoder.supports(&ProximaScheme::BitPacked { bits: 16 }));

        // Test encoding
        let values_f32 = vec![1.0f32, 2.0, 3.0];
        let result = encoder.encode_f32(&values_f32, &ProximaScheme::Delta { base: 0 });
        assert!(result.is_ok());
    }
}
