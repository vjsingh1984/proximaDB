// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SIMD Decoder Traits and Factory
//!
//! This module defines the core traits for SIMD-accelerated decoding operations
//! and a factory for selecting the best available decoder at runtime.
//!
//! # Design Principles (SOLID)
//!
//! - **S**: Single Responsibility - Each decoder handles one platform
//! - **O**: Open/Closed - Add new decoders by implementing `SimdDecoder` trait
//! - **L**: Liskov Substitution - All decoders are interchangeable via trait
//! - **I**: Interface Segregation - Minimal trait with essential operations
//! - **D**: Dependency Inversion - Use `DecoderFactory` to get `Arc<dyn SimdDecoder>`
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::storage::engines::core::ops::simd_decode::{DecoderFactory, SimdDecoder};
//!
//! // Get the best available decoder
//! let decoder = DecoderFactory::best_available();
//!
//! // Decode bitpacked integers
//! let mut output = vec![0i64; 1000];
//! let count = decoder.decode_bitpacked_i64(&packed_data, 8, &mut output)?;
//! ```

use anyhow::Result;
use std::fmt::Debug;
use std::sync::Arc;

/// CPU feature flags detected at runtime
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuFeatures {
    /// AVX2 support (256-bit SIMD on x86_64)
    pub avx2: bool,
    /// AVX-512 support (512-bit SIMD on x86_64)
    pub avx512: bool,
    /// SSE4.1 support (128-bit SIMD on x86_64)
    pub sse41: bool,
    /// NEON support (128-bit SIMD on ARM64)
    pub neon: bool,
}

impl CpuFeatures {
    /// Detect CPU features at runtime
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self {
                avx2: is_x86_feature_detected!("avx2"),
                avx512: is_x86_feature_detected!("avx512f"),
                sse41: is_x86_feature_detected!("sse4.1"),
                neon: false,
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            Self {
                avx2: false,
                avx512: false,
                sse41: false,
                neon: true, // NEON is mandatory on AArch64
            }
        }

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self::default()
        }
    }

    /// Check if any SIMD acceleration is available
    pub fn has_simd(&self) -> bool {
        self.avx512 || self.avx2 || self.sse41 || self.neon
    }

    /// Get the SIMD vector width in bytes
    pub fn vector_width(&self) -> usize {
        if self.avx512 {
            64 // 512 bits
        } else if self.avx2 {
            32 // 256 bits
        } else if self.neon || self.sse41 {
            16 // 128 bits
        } else {
            0 // No SIMD
        }
    }
}

/// SIMD Decoder Strategy Trait
///
/// Implement this trait to add a new SIMD decoder backend.
/// The trait follows the Strategy pattern for swappable implementations.
///
/// # Safety
///
/// Implementations may use unsafe SIMD intrinsics internally, but the trait
/// interface is safe. Implementors must ensure:
/// - Output buffer is large enough for the decoded data
/// - Input data is properly formatted for the encoding scheme
/// - SIMD feature detection is performed before using intrinsics
pub trait SimdDecoder: Send + Sync + Debug {
    /// Decode BitPacked integers to i64
    ///
    /// Unpacks values that were stored with a fixed number of bits per value.
    /// This is the inverse of bitpacking which stores values with variable bit widths.
    ///
    /// # Arguments
    /// * `input` - Packed byte array
    /// * `bits_per_value` - Number of bits used per value (1-64)
    /// * `output` - Pre-allocated output buffer (must have capacity for all values)
    ///
    /// # Returns
    /// Number of values decoded
    ///
    /// # Errors
    /// - If bits_per_value is 0 or > 64
    /// - If input is truncated or malformed
    fn decode_bitpacked_i64(
        &self,
        input: &[u8],
        bits_per_value: u8,
        output: &mut [i64],
    ) -> Result<usize>;

    /// Decode BitPacked integers to i32
    ///
    /// Similar to `decode_bitpacked_i64` but for 32-bit output.
    ///
    /// # Arguments
    /// * `input` - Packed byte array
    /// * `bits_per_value` - Number of bits used per value (1-32)
    /// * `output` - Pre-allocated output buffer
    ///
    /// # Returns
    /// Number of values decoded
    fn decode_bitpacked_i32(
        &self,
        input: &[u8],
        bits_per_value: u8,
        output: &mut [i32],
    ) -> Result<usize>;

    /// Decode Delta-encoded floats
    ///
    /// Reconstructs f32 values from delta-encoded representation.
    /// Delta encoding stores the difference between consecutive values.
    ///
    /// # Algorithm
    /// 1. First value is stored as-is (base value)
    /// 2. Subsequent values: value[i] = value[i-1] + delta[i]
    ///
    /// # Arguments
    /// * `input` - Delta-encoded byte array
    /// * `base` - Base value (first value in the sequence)
    /// * `output` - Pre-allocated output buffer
    ///
    /// # Returns
    /// Number of values decoded
    fn decode_delta_f32(&self, input: &[u8], base: f32, output: &mut [f32]) -> Result<usize>;

    /// Decode Delta-encoded integers
    ///
    /// Reconstructs i64 values from delta-encoded representation.
    ///
    /// # Arguments
    /// * `input` - Delta-encoded byte array
    /// * `base` - Base value (first value in the sequence)
    /// * `output` - Pre-allocated output buffer
    ///
    /// # Returns
    /// Number of values decoded
    fn decode_delta_i64(&self, input: &[u8], base: i64, output: &mut [i64]) -> Result<usize>;

    /// Get the CPU features supported by this decoder
    fn supported_features(&self) -> CpuFeatures;

    /// Get the name of this decoder backend
    fn name(&self) -> &'static str;

    /// Check if this decoder has hardware acceleration
    fn has_acceleration(&self) -> bool {
        self.supported_features().has_simd()
    }
}

/// Factory for selecting the best available decoder
///
/// The factory detects CPU features at runtime and returns the optimal decoder.
/// Priority: AVX-512 > AVX2 > NEON > SSE4.1 > Scalar
///
/// # Thread Safety
///
/// The factory returns `Arc<dyn SimdDecoder>` which is thread-safe.
/// The same decoder instance can be shared across threads.
pub struct DecoderFactory;

impl DecoderFactory {
    /// Get the best available decoder for the current platform
    ///
    /// This performs runtime CPU feature detection and returns the optimal decoder.
    /// The detection is cached after the first call.
    ///
    /// # Returns
    /// An `Arc<dyn SimdDecoder>` pointing to the best available decoder
    ///
    /// # Example
    /// ```rust,ignore
    /// let decoder = DecoderFactory::best_available();
    /// println!("Using {} decoder", decoder.name());
    /// ```
    pub fn best_available() -> Arc<dyn SimdDecoder> {
        use super::*;

        let features = CpuFeatures::detect();

        #[cfg(target_arch = "x86_64")]
        {
            if features.avx2 {
                return Arc::new(bitpacked_avx2::Avx2Decoder::new());
            }
            // Note: SSE decoder could be added as intermediate fallback
        }

        #[cfg(target_arch = "aarch64")]
        {
            if features.neon {
                return Arc::new(bitpacked_neon::NeonDecoder::new());
            }
        }

        Arc::new(bitpacked_scalar::ScalarDecoder::new())
    }

    /// Create a decoder for specific features (for testing)
    ///
    /// # Arguments
    /// * `features` - The CPU features to target
    ///
    /// # Returns
    /// A decoder that uses the specified features (falls back to scalar if unavailable)
    #[allow(dead_code)]
    pub fn for_features(features: CpuFeatures) -> Arc<dyn SimdDecoder> {
        use super::*;

        #[cfg(target_arch = "x86_64")]
        {
            if features.avx2 && is_x86_feature_detected!("avx2") {
                return Arc::new(bitpacked_avx2::Avx2Decoder::new());
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            if features.neon {
                return Arc::new(bitpacked_neon::NeonDecoder::new());
            }
        }

        Arc::new(bitpacked_scalar::ScalarDecoder::new())
    }

    /// Get a scalar-only decoder (for testing/comparison)
    pub fn scalar() -> Arc<dyn SimdDecoder> {
        use super::bitpacked_scalar::ScalarDecoder;
        Arc::new(ScalarDecoder::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_features_detect() {
        let features = CpuFeatures::detect();

        #[cfg(target_arch = "x86_64")]
        {
            // On x86_64, at least SSE2 should be available (it's baseline)
            println!("x86_64 features: {:?}", features);
            // Note: AVX2 availability depends on the CPU
        }

        #[cfg(target_arch = "aarch64")]
        {
            // On ARM64, NEON is mandatory
            assert!(features.neon, "NEON should be available on ARM64");
            println!("ARM64 features: {:?}", features);
        }
    }

    #[test]
    fn test_cpu_features_vector_width() {
        let features = CpuFeatures {
            avx512: true,
            avx2: true,
            sse41: true,
            neon: false,
        };
        assert_eq!(features.vector_width(), 64); // AVX-512 wins

        let features = CpuFeatures {
            avx512: false,
            avx2: true,
            sse41: true,
            neon: false,
        };
        assert_eq!(features.vector_width(), 32); // AVX2 wins

        let features = CpuFeatures {
            avx512: false,
            avx2: false,
            sse41: false,
            neon: true,
        };
        assert_eq!(features.vector_width(), 16); // NEON

        let features = CpuFeatures::default();
        assert_eq!(features.vector_width(), 0); // No SIMD
    }

    #[test]
    fn test_decoder_factory_best_available() {
        let decoder = DecoderFactory::best_available();
        println!("Best decoder: {} ({:?})", decoder.name(), decoder.supported_features());

        // Should always return a valid decoder
        assert!(!decoder.name().is_empty());
    }

    #[test]
    fn test_decoder_factory_scalar() {
        let decoder = DecoderFactory::scalar();
        assert_eq!(decoder.name(), "Scalar");
        assert!(!decoder.has_acceleration());
    }
}
