// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! ProximaCodec - Main encoding/decoding API
//!
//! This is the ONLY public interface for encoding/decoding in ProximaDB.
//! It provides:
//! - Hardware-aware routing (Baseline for now, SIMD/GPU in future phases)
//! - Unified wire format with versioning
//! - Type safety (cannot decode wrong type)
//! - Metrics integration (future)

use anyhow::Result;
use std::sync::OnceLock;

use super::impls::baseline::{BaselineDecoder, BaselineEncoder};
use super::registry::ImplementationRegistry;
use super::types::{Decodable, Encodable, ProximaScheme, TypeId};
use super::wire_format::WireFormatManager;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
use super::impls::simd::{SimdDecoder, SimdEncoder};

/// The ONLY public encoding/decoding interface
///
/// Usage:
/// ```text
/// let codec = ProximaCodec::global();
/// let encoded = codec.encode(&values, ProximaScheme::Delta { base: 0 })?;
/// let decoded = codec.decode::<f32>(&encoded)?;
/// ```text
pub struct ProximaCodec {
    wire_format: WireFormatManager,
    registry: ImplementationRegistry,
}

impl ProximaCodec {
    /// Get the global codec instance (singleton)
    pub fn global() -> &'static Self {
        static CODEC: OnceLock<ProximaCodec> = OnceLock::new();
        CODEC.get_or_init(|| Self::new())
    }

    /// Create a new codec with automatic hardware detection
    fn new() -> Self {
        use crate::core::hardware_capabilities::HardwareCapabilities;

        // Use default hardware capabilities (detects at first access)
        let hw_caps = HardwareCapabilities::default();
        let mut registry = ImplementationRegistry::new(hw_caps);

        // Registration order matters! First registered = highest priority
        // Priority: GPU > SIMD > Baseline

        // Phase 1: GPU implementations (highest priority, CUDA/ROCm/MPS/OpenCL)
        #[cfg(feature = "gpu")]
        {
            use crate::storage::engines::core::ops::proximacodec::impls::gpu::{
                GpuDecoder, GpuEncoder,
            };

            // GPU encoders/decoders only activate if GPU backend is detected
            registry.register_encoder(Box::new(GpuEncoder));
            registry.register_decoder(Box::new(GpuDecoder));
        }

        // Phase 2: SIMD implementations (medium priority, ARM64 NEON/x86_64 AVX2/AVX512)
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        {
            registry.register_encoder(Box::new(SimdEncoder));
            registry.register_decoder(Box::new(SimdDecoder));
        }

        // Phase 3: Baseline implementation (lowest priority, always available as fallback)
        registry.register_encoder(Box::new(BaselineEncoder));
        registry.register_decoder(Box::new(BaselineDecoder));

        Self {
            wire_format: WireFormatManager::new(),
            registry,
        }
    }

    /// Encode f32 values with specified scheme
    pub fn encode(&self, values: &[f32], scheme: ProximaScheme) -> Result<Vec<u8>> {
        // Check if scheme is lossy for f32, if so, use Delta instead (lossless)
        let safe_scheme = if scheme.is_lossy(TypeId::F32) {
            tracing::warn!(
                "Scheme {:?} is lossy for F32, falling back to Delta encoding",
                scheme
            );
            ProximaScheme::Delta { base: 0 }
        } else {
            scheme
        };

        // Find encoder (currently only baseline)
        let encoder = self.registry.get_encoder(&safe_scheme)?;

        // Encode raw data
        let raw_data = encoder.encode_f32(values, &safe_scheme)?;

        // Add wire format header (use safe_scheme, not original)
        let header = self
            .wire_format
            .write_header(&safe_scheme, values.len(), TypeId::F32);

        let mut result = header;
        result.extend_from_slice(&raw_data);

        Ok(result)
    }

    /// Encode i64 values with specified scheme
    pub fn encode_i64(&self, values: &[i64], scheme: ProximaScheme) -> Result<Vec<u8>> {
        // Check if scheme is lossy for i64, if so, use Delta instead (lossless)
        let safe_scheme = if scheme.is_lossy(TypeId::I64) {
            tracing::warn!(
                "Scheme {:?} is lossy for I64, falling back to Delta encoding",
                scheme
            );
            ProximaScheme::Delta { base: 0 }
        } else {
            scheme
        };

        let encoder = self.registry.get_encoder(&safe_scheme)?;
        let raw_data = encoder.encode_i64(values, &safe_scheme)?;
        let header = self
            .wire_format
            .write_header(&safe_scheme, values.len(), TypeId::I64);

        let mut result = header;
        result.extend_from_slice(&raw_data);
        Ok(result)
    }

    /// Encode i32 values with specified scheme
    pub fn encode_i32(&self, values: &[i32], scheme: ProximaScheme) -> Result<Vec<u8>> {
        // Check if scheme is lossy for i32, if so, use Delta instead (lossless)
        let safe_scheme = if scheme.is_lossy(TypeId::I32) {
            tracing::warn!(
                "Scheme {:?} is lossy for I32, falling back to Delta encoding",
                scheme
            );
            ProximaScheme::Delta { base: 0 }
        } else {
            scheme
        };

        let encoder = self.registry.get_encoder(&safe_scheme)?;
        let raw_data = encoder.encode_i32(values, &safe_scheme)?;
        let header = self
            .wire_format
            .write_header(&safe_scheme, values.len(), TypeId::I32);

        let mut result = header;
        result.extend_from_slice(&raw_data);
        Ok(result)
    }

    /// Decode f32 values from encoded data
    pub fn decode(&self, data: &[u8]) -> Result<Vec<f32>> {
        // Parse wire format header
        let header = self.wire_format.read_header(data)?;

        // Validate type matches
        if header.type_id != TypeId::F32 {
            return Err(anyhow::anyhow!(
                "Type mismatch: encoded as {:?}, expected F32",
                header.type_id
            ));
        }

        // Find decoder
        let decoder = self.registry.get_decoder(&header.scheme)?;

        // Decode raw data
        let raw_data = &data[header.data_offset..];
        decoder.decode_f32(raw_data, &header.scheme, header.count)
    }

    /// Decode i64 values from encoded data
    pub fn decode_i64(&self, data: &[u8]) -> Result<Vec<i64>> {
        let header = self.wire_format.read_header(data)?;

        if header.type_id != TypeId::I64 {
            return Err(anyhow::anyhow!(
                "Type mismatch: encoded as {:?}, expected I64",
                header.type_id
            ));
        }

        let decoder = self.registry.get_decoder(&header.scheme)?;
        let raw_data = &data[header.data_offset..];
        decoder.decode_i64(raw_data, &header.scheme, header.count)
    }

    /// Decode i32 values from encoded data
    pub fn decode_i32(&self, data: &[u8]) -> Result<Vec<i32>> {
        let header = self.wire_format.read_header(data)?;

        if header.type_id != TypeId::I32 {
            return Err(anyhow::anyhow!(
                "Type mismatch: encoded as {:?}, expected I32",
                header.type_id
            ));
        }

        let decoder = self.registry.get_decoder(&header.scheme)?;
        let raw_data = &data[header.data_offset..];
        decoder.decode_i32(raw_data, &header.scheme, header.count)
    }

    /// Encode u32 values with specified scheme
    ///
    /// NOTE: Currently delegates to i64 encoding internally.
    /// Future optimization: Add native u32 support to avoid conversion overhead.
    pub fn encode_u32(&self, values: &[u32], scheme: ProximaScheme) -> Result<Vec<u8>> {
        // Convert u32 to i64 for encoding
        let values_i64: Vec<i64> = values.iter().map(|&v| v as i64).collect();

        // Delegate to i64 encoding
        // The wire format will store this as I64, so decode_u32 must know to convert back
        self.encode_i64(&values_i64, scheme)
    }

    /// Decode u32 values from encoded data
    ///
    /// NOTE: Currently delegates to i64 decoding internally.
    /// Future optimization: Add native u32 support to avoid conversion overhead.
    pub fn decode_u32(&self, data: &[u8]) -> Result<Vec<u32>> {
        // Decode as i64 (that's how encode_u32 stores it)
        let values_i64 = self.decode_i64(data)?;

        // Convert back to u32
        let values_u32: Vec<u32> = values_i64.iter().map(|&v| v as u32).collect();
        Ok(values_u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u32_roundtrip() {
        let codec = ProximaCodec::global();

        // Test u32 encoding/decoding (internally uses i64 delegation)
        let values = vec![1u32, 2, 3, 4, 5, 100, 1000, 10000];
        let encoded = codec
            .encode_u32(&values, ProximaScheme::Delta { base: 0 })
            .unwrap();
        let decoded = codec.decode_u32(&encoded).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_basic_roundtrip() {
        let codec = ProximaCodec::global();

        let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
        let encoded = codec
            .encode(&values, ProximaScheme::Delta { base: 0 })
            .unwrap();
        let decoded = codec.decode(&encoded).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_type_safety() {
        let codec = ProximaCodec::global();

        // Encode as f32
        let values_f32 = vec![1.0f32, 2.0, 3.0];
        let encoded = codec
            .encode(&values_f32, ProximaScheme::Delta { base: 0 })
            .unwrap();

        // Try to decode as i64 (should fail)
        let result_i64 = codec.decode_i64(&encoded);
        assert!(result_i64.is_err(), "Should fail to decode f32 as i64");

        // Decode as f32 (should succeed)
        let decoded = codec.decode(&encoded).unwrap();
        assert_eq!(values_f32, decoded);
    }
}
