// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Implementation registry for hardware-aware routing
//!
//! The registry maintains a prioritized list of encoders/decoders:
//! 1. GPU implementations (fastest, if available)
//! 2. SIMD implementations (fast, if CPU supports)
//! 3. Baseline implementation (always available)
//!
//! When encoding/decoding, the registry tries implementations in order
//! until it finds one that supports the requested scheme.

use super::traits::{RawDecoder, RawEncoder};
use super::types::ProximaScheme;
use crate::core::hardware_capabilities::HardwareCapabilities;
use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Registry of available encoders/decoders with hardware-aware routing
pub struct ImplementationRegistry {
    encoders: Vec<Box<dyn RawEncoder>>,
    decoders: Vec<Box<dyn RawDecoder>>,
    hw_caps: Arc<HardwareCapabilities>,
}

impl ImplementationRegistry {
    /// Create a new registry with hardware capabilities
    pub fn new(hw_caps: HardwareCapabilities) -> Self {
        let gpu_info = match hw_caps.gpu.backend {
            crate::core::hardware_capabilities::GpuBackend::CUDA => "CUDA",
            crate::core::hardware_capabilities::GpuBackend::ROCm => "ROCm",
            crate::core::hardware_capabilities::GpuBackend::MPS => "MPS",
            crate::core::hardware_capabilities::GpuBackend::OpenCL => "OpenCL",
            crate::core::hardware_capabilities::GpuBackend::None => "None",
        };

        debug!(
            "🔧 [REGISTRY] Initializing with HW capabilities: CPU={}, cores={}, GPU={}",
            hw_caps.cpu.model_name,
            hw_caps.cpu.physical_cores,
            gpu_info
        );

        Self {
            encoders: Vec::new(),
            decoders: Vec::new(),
            hw_caps: Arc::new(hw_caps),
        }
    }

    /// Register an encoder
    ///
    /// Encoders are tried in registration order (GPU → SIMD → Baseline).
    /// Register high-priority implementations first.
    pub fn register_encoder(&mut self, encoder: Box<dyn RawEncoder>) {
        debug!(
            "✅ [REGISTRY] Registered encoder: {}",
            encoder.name()
        );
        self.encoders.push(encoder);
    }

    /// Register a decoder
    ///
    /// Decoders are tried in registration order (GPU → SIMD → Baseline).
    /// Register high-priority implementations first.
    pub fn register_decoder(&mut self, decoder: Box<dyn RawDecoder>) {
        debug!(
            "✅ [REGISTRY] Registered decoder: {}",
            decoder.name()
        );
        self.decoders.push(decoder);
    }

    /// Get best encoder for scheme
    ///
    /// Tries encoders in registration order until finding one that supports the scheme.
    /// Returns error if no encoder supports the scheme.
    ///
    /// # Arguments
    /// - `scheme`: Encoding scheme
    ///
    /// # Returns
    /// Reference to first encoder that supports the scheme
    ///
    /// # Errors
    /// - If no encoder supports the scheme
    pub fn get_encoder(&self, scheme: &ProximaScheme) -> Result<&dyn RawEncoder> {
        trace!(
            "🔍 [REGISTRY] Finding encoder for scheme: {}",
            scheme.name()
        );

        for encoder in &self.encoders {
            if encoder.supports(scheme) {
                debug!(
                    "✅ [REGISTRY] Selected encoder: {} for scheme: {}",
                    encoder.name(),
                    scheme.name()
                );
                return Ok(encoder.as_ref());
            } else {
                trace!(
                    "⏭️  [REGISTRY] Encoder {} does not support {}",
                    encoder.name(),
                    scheme.name()
                );
            }
        }

        warn!(
            "❌ [REGISTRY] No encoder found for scheme: {}",
            scheme.name()
        );
        Err(anyhow::anyhow!(
            "No encoder available for scheme: {}",
            scheme.name()
        ))
    }

    /// Get best decoder for scheme
    ///
    /// Tries decoders in registration order until finding one that supports the scheme.
    /// Returns error if no decoder supports the scheme.
    ///
    /// # Arguments
    /// - `scheme`: Encoding scheme
    ///
    /// # Returns
    /// Reference to first decoder that supports the scheme
    ///
    /// # Errors
    /// - If no decoder supports the scheme
    pub fn get_decoder(&self, scheme: &ProximaScheme) -> Result<&dyn RawDecoder> {
        trace!(
            "🔍 [REGISTRY] Finding decoder for scheme: {}",
            scheme.name()
        );

        for decoder in &self.decoders {
            if decoder.supports(scheme) {
                debug!(
                    "✅ [REGISTRY] Selected decoder: {} for scheme: {}",
                    decoder.name(),
                    scheme.name()
                );
                return Ok(decoder.as_ref());
            } else {
                trace!(
                    "⏭️  [REGISTRY] Decoder {} does not support {}",
                    decoder.name(),
                    scheme.name()
                );
            }
        }

        warn!(
            "❌ [REGISTRY] No decoder found for scheme: {}",
            scheme.name()
        );
        Err(anyhow::anyhow!(
            "No decoder available for scheme: {}",
            scheme.name()
        ))
    }

    /// Get hardware capabilities
    pub fn hardware_capabilities(&self) -> Arc<HardwareCapabilities> {
        Arc::clone(&self.hw_caps)
    }

    /// Get number of registered encoders
    pub fn encoder_count(&self) -> usize {
        self.encoders.len()
    }

    /// Get number of registered decoders
    pub fn decoder_count(&self) -> usize {
        self.decoders.len()
    }

    /// List all registered encoder names
    pub fn encoder_names(&self) -> Vec<&'static str> {
        self.encoders.iter().map(|e| e.name()).collect()
    }

    /// List all registered decoder names
    pub fn decoder_names(&self) -> Vec<&'static str> {
        self.decoders.iter().map(|d| d.name()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock implementations for testing
    struct MockBaselineEncoder;
    impl RawEncoder for MockBaselineEncoder {
        fn name(&self) -> &'static str {
            "mock-baseline"
        }
        fn supports(&self, _scheme: &ProximaScheme) -> bool {
            true // Supports everything
        }
        fn encode_f32(&self, _values: &[f32], _scheme: &ProximaScheme) -> Result<Vec<u8>> {
            Ok(vec![0xBB]) // Mock data
        }
        fn encode_i64(&self, _values: &[i64], _scheme: &ProximaScheme) -> Result<Vec<u8>> {
            Ok(vec![0xBB]) // Mock data
        }
        fn encode_i32(&self, _values: &[i32], _scheme: &ProximaScheme) -> Result<Vec<u8>> {
            Ok(vec![0xBB]) // Mock data
        }
    }

    struct MockSimdEncoder;
    impl RawEncoder for MockSimdEncoder {
        fn name(&self) -> &'static str {
            "mock-simd"
        }
        fn supports(&self, scheme: &ProximaScheme) -> bool {
            // Only supports Delta and BitPacked
            matches!(
                scheme,
                ProximaScheme::Delta { .. } | ProximaScheme::BitPacked { .. }
            )
        }
        fn encode_f32(&self, _values: &[f32], _scheme: &ProximaScheme) -> Result<Vec<u8>> {
            Ok(vec![0xDD]) // Mock data
        }
        fn encode_i64(&self, _values: &[i64], _scheme: &ProximaScheme) -> Result<Vec<u8>> {
            Ok(vec![0xDD]) // Mock data
        }
        fn encode_i32(&self, _values: &[i32], _scheme: &ProximaScheme) -> Result<Vec<u8>> {
            Ok(vec![0xDD]) // Mock data
        }
    }

    struct MockBaselineDecoder;
    impl RawDecoder for MockBaselineDecoder {
        fn name(&self) -> &'static str {
            "mock-baseline"
        }
        fn supports(&self, _scheme: &ProximaScheme) -> bool {
            true
        }
        fn decode_f32(&self, _data: &[u8], _scheme: &ProximaScheme, _count: usize) -> Result<Vec<f32>> {
            Ok(Vec::new())
        }
        fn decode_i64(&self, _data: &[u8], _scheme: &ProximaScheme, _count: usize) -> Result<Vec<i64>> {
            Ok(Vec::new())
        }
        fn decode_i32(&self, _data: &[u8], _scheme: &ProximaScheme, _count: usize) -> Result<Vec<i32>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn test_registry_priority() {
        use crate::core::config::HardwareConfig;
        let hw = HardwareCapabilities::detect_with_config(HardwareConfig::default()).unwrap();
        let mut registry = ImplementationRegistry::new(hw);

        // Register in priority order: SIMD → Baseline
        registry.register_encoder(Box::new(MockSimdEncoder));
        registry.register_encoder(Box::new(MockBaselineEncoder));

        // Delta should use SIMD
        let delta_encoder = registry.get_encoder(&ProximaScheme::Delta { base: 0 }).unwrap();
        assert_eq!(delta_encoder.name(), "mock-simd");

        // SparseBitmap should fall back to Baseline (SIMD doesn't support)
        let sparse_encoder = registry.get_encoder(&ProximaScheme::SparseBitmap).unwrap();
        assert_eq!(sparse_encoder.name(), "mock-baseline");
    }

    #[test]
    fn test_registry_no_encoder() {
        use crate::core::config::HardwareConfig;
        let hw = HardwareCapabilities::detect_with_config(HardwareConfig::default()).unwrap();
        let registry = ImplementationRegistry::new(hw);

        // No encoders registered
        let result = registry.get_encoder(&ProximaScheme::Delta { base: 0 });
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_info() {
        use crate::core::config::HardwareConfig;
        let hw = HardwareCapabilities::detect_with_config(HardwareConfig::default()).unwrap();
        let mut registry = ImplementationRegistry::new(hw);

        registry.register_encoder(Box::new(MockSimdEncoder));
        registry.register_encoder(Box::new(MockBaselineEncoder));
        registry.register_decoder(Box::new(MockBaselineDecoder));

        assert_eq!(registry.encoder_count(), 2);
        assert_eq!(registry.decoder_count(), 1);

        let encoder_names = registry.encoder_names();
        assert!(encoder_names.contains(&"mock-simd"));
        assert!(encoder_names.contains(&"mock-baseline"));
    }
}
