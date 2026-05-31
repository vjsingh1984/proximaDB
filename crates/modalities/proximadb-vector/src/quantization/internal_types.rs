//! Internal quantization types (Release 1 - no legacy compatibility)
//!
//! These types are used internally for quantization operations.
//! The proto QuantizationConfig is simplified for user-facing API.

use serde::{Deserialize, Serialize};

/// Unified quantization level configuration
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnifiedQuantizationLevel {
    pub level_type: Option<QuantizationLevel>,
}

/// Quantization level types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum QuantizationLevel {
    None(NoQuantization),
    Uniform(UniformQuantization),
    Pq(ProductQuantization),
    Scalar(ScalarQuantization),
    Binary(BinaryQuantization),
    Custom(CustomQuantization),
    /// TurboQuant — data-oblivious scalar quantizer (ADR-021).
    /// See `TURBOQUANT_LLD_2026_05_30.adoc` §"Locked Type Signatures".
    #[cfg(feature = "experimental-turboquant")]
    TurboQuant(TurboQuantization),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NoQuantization {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UniformQuantization {
    pub bits: i32,
    pub scale: Option<f32>,
    pub offset: Option<f32>,
}

impl Eq for UniformQuantization {}

impl std::hash::Hash for UniformQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.bits.hash(state);
        // Hash the bits representation of the floats for consistency
        self.scale.map(|s| s.to_bits()).hash(state);
        self.offset.map(|o| o.to_bits()).hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ProductQuantization {
    pub bits_per_code: i32,
    pub num_subvectors: i32,
    pub codebook_id: Option<String>,
    pub adaptive_subvectors: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScalarQuantization {
    pub bits: i32,
    pub scale: f32,
    pub offset: f32,
    pub clamp_values: bool,
}

impl Eq for ScalarQuantization {}

impl std::hash::Hash for ScalarQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.bits.hash(state);
        self.scale.to_bits().hash(state);
        self.offset.to_bits().hash(state);
        self.clamp_values.hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinaryQuantization {
    pub threshold: Option<f32>,
    pub sign_based: bool,
}

impl Eq for BinaryQuantization {}

impl std::hash::Hash for BinaryQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.threshold.map(|t| t.to_bits()).hash(state);
        self.sign_based.hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomQuantization {
    pub type_id: String,
    pub bits_per_element: i32,
    pub config: std::collections::HashMap<String, String>,
}

impl std::hash::Hash for CustomQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.type_id.hash(state);
        self.bits_per_element.hash(state);
        // Hash the config map in a deterministic way
        let mut sorted_config: Vec<_> = self.config.iter().collect();
        sorted_config.sort_by_key(|(k, _)| k.as_str());
        for (k, v) in sorted_config {
            k.hash(state);
            v.hash(state);
        }
    }
}

/// TurboQuant configuration (ADR-021, TURBOQUANT_LLD_2026_05_30 §"Locked Type Signatures").
///
/// The three fields are everything a search path needs to dispatch:
/// - `bit_width` ∈ {2, 4} for P1; {2, 3, 4} after P10.
/// - `calibration_mode`: identity vs TQ+ per-coord. Frozen after first
///   qualifying batch when TqPlus is selected. See [`CalibrationMode`].
/// - `rotation_seed`: per-collection, derived via
///   `proximadb_quantization_types::derive_rotation_seed(collection_id)`
///   on first enablement, then persisted in xCatalog (P8).
///
/// The struct itself owns no buffers — codes, scales, and TQ+ calibration
/// vectors are persisted in the `.tq` file (LLD §3) and loaded into
/// `TurboQuantVectorData` (root crate) at search time.
#[cfg(feature = "experimental-turboquant")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TurboQuantization {
    /// Bit-width per coordinate. 2 or 4 in P1.
    pub bit_width: u8,
    /// Per-coord calibration mode. See foundation's `CalibrationMode`.
    pub calibration_mode: proximadb_quantization_types::CalibrationMode,
    /// Per-collection rotation seed. Derived from collection id at first
    /// enablement, then immutable for the collection's lifetime (until an
    /// epoch bump triggers a full re-encode).
    pub rotation_seed: u64,
}

#[cfg(feature = "experimental-turboquant")]
impl TurboQuantization {
    /// Construct a TurboQuant config with identity calibration. Use when
    /// the collection is too small for TQ+ or when explicitly disabling it.
    pub fn identity(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            bit_width,
            calibration_mode: proximadb_quantization_types::CalibrationMode::Identity,
            rotation_seed,
        }
    }

    /// Construct a TurboQuant config with TQ+ calibration. The calibration
    /// itself is fit lazily on the first qualifying batch (≥ 1000 vectors).
    pub fn tq_plus(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            bit_width,
            calibration_mode: proximadb_quantization_types::CalibrationMode::TqPlus,
            rotation_seed,
        }
    }
}

#[allow(non_upper_case_globals)]
impl UnifiedQuantizationLevel {
    /// Common quantization level constants for easy access
    /// Using PascalCase for API consistency with enum variant style
    pub const Binary: Self = Self {
        level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
            threshold: None,
            sign_based: false,
        })),
    };

    pub const Int8: Self = Self {
        level_type: Some(QuantizationLevel::Scalar(ScalarQuantization {
            bits: 8,
            scale: 1.0,
            offset: 0.0,
            clamp_values: true,
        })),
    };

    /// Create a PQ4 constant (requires runtime initialization due to parameter)
    pub const Pq4: Self = Self {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 4,
            num_subvectors: 8, // Default value
            codebook_id: None,
            adaptive_subvectors: false,
        })),
    };

    /// Create a PQ8 constant (requires runtime initialization due to parameter)
    pub const Pq8: Self = Self {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 8, // Default value
            codebook_id: None,
            adaptive_subvectors: false,
        })),
    };

    /// Create a PQ8 configuration (common case)
    pub fn pq8(num_subvectors: u8) -> Self {
        Self {
            level_type: Some(QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 8,
                num_subvectors: num_subvectors as i32,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }
    }

    /// Create a PQ4 configuration (higher compression)
    pub fn pq4(num_subvectors: u8) -> Self {
        Self {
            level_type: Some(QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 4,
                num_subvectors: num_subvectors as i32,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }
    }

    /// Create an INT8 scalar quantization
    pub fn int8() -> Self {
        Self {
            level_type: Some(QuantizationLevel::Scalar(ScalarQuantization {
                bits: 8,
                scale: 1.0,
                offset: 0.0,
                clamp_values: true,
            })),
        }
    }

    /// Create a binary quantization
    pub fn binary() -> Self {
        Self {
            level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
                threshold: None,
                sign_based: false,
            })),
        }
    }

    /// Get the number of bits per element
    pub fn bits_per_element(&self) -> u32 {
        match &self.level_type {
            Some(QuantizationLevel::Pq(pq)) => pq.bits_per_code as u32,
            Some(QuantizationLevel::Scalar(sq)) => sq.bits as u32,
            Some(QuantizationLevel::Binary(_)) => 1,
            Some(QuantizationLevel::Uniform(uq)) => uq.bits as u32,
            Some(QuantizationLevel::Custom(cq)) => cq.bits_per_element as u32,
            #[cfg(feature = "experimental-turboquant")]
            Some(QuantizationLevel::TurboQuant(tq)) => tq.bit_width as u32,
            _ => 32, // Full precision
        }
    }

    /// Calculate bytes per vector based on quantization level
    pub fn bytes_per_vector(&self, dimension: usize) -> usize {
        match &self.level_type {
            Some(QuantizationLevel::Pq(pq)) => {
                let codes_per_vector = pq.num_subvectors as usize;
                let bytes_per_code = ((pq.bits_per_code + 7) / 8) as usize;
                codes_per_vector * bytes_per_code
            }
            Some(QuantizationLevel::Scalar(sq)) => dimension * ((sq.bits + 7) / 8) as usize,
            Some(QuantizationLevel::Binary(_)) => {
                dimension.div_ceil(8) // 1 bit per dimension
            }
            Some(QuantizationLevel::Uniform(uq)) => dimension * ((uq.bits + 7) / 8) as usize,
            Some(QuantizationLevel::Custom(cq)) => {
                dimension * ((cq.bits_per_element + 7) / 8) as usize
            }
            #[cfg(feature = "experimental-turboquant")]
            Some(QuantizationLevel::TurboQuant(tq)) => {
                // ceil(dim * bit_width / 8) — matches LLD §3 wire layout.
                // Plus 4 bytes per vector for the RaBitQ-style length-renorm
                // scale (stored separately in the .tq file but accounted for
                // here so callers get a single bytes-per-vector estimate).
                let bits_per_vec = dimension * tq.bit_width as usize;
                bits_per_vec.div_ceil(8) + 4
            }
            _ => dimension * 4, // Full FP32 precision
        }
    }

    /// Get compression ratio compared to FP32
    pub fn compression_ratio(&self, dimension: usize) -> f32 {
        let fp32_bytes = dimension * 4;
        let compressed_bytes = self.bytes_per_vector(dimension);
        fp32_bytes as f32 / compressed_bytes.max(1) as f32
    }

    /// Construct a TurboQuant configuration. Bit width must be 2 or 4 in P1
    /// (3-bit is deferred per LLD Q10).
    #[cfg(feature = "experimental-turboquant")]
    pub fn turboquant(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            level_type: Some(QuantizationLevel::TurboQuant(TurboQuantization::tq_plus(
                bit_width,
                rotation_seed,
            ))),
        }
    }

    /// Construct a TurboQuant configuration with identity calibration
    /// (no TQ+ per-coord adjustment).
    #[cfg(feature = "experimental-turboquant")]
    pub fn turboquant_identity(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            level_type: Some(QuantizationLevel::TurboQuant(TurboQuantization::identity(
                bit_width,
                rotation_seed,
            ))),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "experimental-turboquant")]
    use proximadb_quantization_types::CalibrationMode;

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_variant_2bit_bytes_per_vector() {
        // d=1536, bit_width=2 → ceil(1536*2/8) = 384 bytes + 4 scale = 388
        let q = UnifiedQuantizationLevel::turboquant(2, 0xdeadbeef);
        assert_eq!(q.bits_per_element(), 2);
        assert_eq!(q.bytes_per_vector(1536), 388);
        // Compression ratio matches the headline 16x at 2-bit minus the scale
        // overhead (~6144 / 388 ≈ 15.8).
        let r = q.compression_ratio(1536);
        assert!(r > 15.0 && r < 16.0, "ratio = {}", r);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_variant_4bit_bytes_per_vector() {
        // d=1536, bit_width=4 → ceil(1536*4/8) = 768 bytes + 4 scale = 772
        let q = UnifiedQuantizationLevel::turboquant(4, 0xdeadbeef);
        assert_eq!(q.bits_per_element(), 4);
        assert_eq!(q.bytes_per_vector(1536), 772);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_identity_constructor() {
        let q = UnifiedQuantizationLevel::turboquant_identity(4, 42);
        match &q.level_type {
            Some(QuantizationLevel::TurboQuant(tq)) => {
                assert_eq!(tq.bit_width, 4);
                assert_eq!(tq.rotation_seed, 42);
                assert_eq!(tq.calibration_mode, CalibrationMode::Identity);
            }
            _ => panic!("expected TurboQuant variant"),
        }
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_serde_round_trip() {
        let q = TurboQuantization::tq_plus(2, 0xcafe_babe_dead_beef);
        let s = serde_json::to_string(&q).unwrap();
        let back: TurboQuantization = serde_json::from_str(&s).unwrap();
        assert_eq!(q, back);
    }
}
