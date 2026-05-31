//! # ProximaDB Quantization Types
//!
//! Foundation quantization types for ProximaDB.

#![allow(deprecated)]
//!
//! ## Purpose
//!
//! This crate provides the single source of truth for quantization types
//! across the entire ProximaDB codebase. It eliminates the proliferation of
//! duplicate quantization definitions (30+ found in audit).
//!
//! ## Types
//!
//! - [`QuantizationType`] - Standardized quantization type enum
//! - [`QuantizationLevel`] - Standardized quantization level enum
//! - [`QuantizationConfig`] - Configuration for quantization
//!
//! ## Migration
//!
//! If you're using legacy quantization types, migrate to this crate's types
//! using the provided conversion traits.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Standardized quantization type enum.
///
/// This is the single source of truth for quantization types across ProximaDB.
/// All other quantization type definitions should migrate to use this enum.
///
/// ## Variants
///
/// - `None` - No quantization
/// - `Scalar` - Scalar quantization (per-vector scalar quantization)
/// - `Product` - Product quantization (PQ)
/// - `Binary` - Binary quantization
/// - `TurboQuant` (feature `experimental-turboquant`) - Data-oblivious scalar
///   quantizer per ADR-021. Online ingest, no codebook training.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuantizationType {
    /// No quantization
    #[default]
    None,

    /// Scalar quantization
    ///
    /// Also known as: Scalar, PQScalar, Int8Quantization
    Scalar,

    /// Product quantization
    ///
    /// Also known as: PQ, ProductQuantization, OPQ
    Product,

    /// Binary quantization
    ///
    /// Also known as: Binary, BitQuantization
    Binary,

    /// TurboQuant — data-oblivious scalar quantizer (ADR-021, arXiv:2504.19874).
    /// Online ingest, no codebook training. See TURBOQUANT_HLD_2026_05_30.
    #[cfg(feature = "experimental-turboquant")]
    TurboQuant,
}

impl fmt::Display for QuantizationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Scalar => write!(f, "scalar"),
            Self::Product => write!(f, "product"),
            Self::Binary => write!(f, "binary"),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant => write!(f, "turboquant"),
        }
    }
}

impl QuantizationType {
    /// Create from string representation
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl FromStr for QuantizationType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "no" => Some(Self::None),
            "scalar" | "int8" | "uint8" | "scalarquantization" => Some(Self::Scalar),
            "product" | "pq" | "productquantization" => Some(Self::Product),
            "binary" | "bit" | "binaryquantization" => Some(Self::Binary),
            #[cfg(feature = "experimental-turboquant")]
            "turboquant" | "tq" | "tq+" | "turbo" => Some(Self::TurboQuant),
            _ => None,
        }
        .ok_or(())
    }
}

impl QuantizationType {
    /// Check if this quantization type uses fixed-point arithmetic
    pub fn is_fixed_point(&self) -> bool {
        #[cfg(feature = "experimental-turboquant")]
        {
            // TurboQuant codes are 2/3/4-bit integers — fixed-point.
            return matches!(self, Self::Scalar | Self::Binary | Self::TurboQuant);
        }
        #[cfg(not(feature = "experimental-turboquant"))]
        {
            matches!(self, Self::Scalar | Self::Binary)
        }
    }

    /// Check if this quantization type uses floating-point arithmetic
    pub fn is_floating_point(&self) -> bool {
        !self.is_fixed_point() && *self != Self::None
    }
}

/// Standardized quantization level enum.
///
/// This represents the bit-width or precision level for quantization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuantizationLevel {
    /// No quantization
    #[default]
    None,

    /// 4-bit quantization
    Int4,

    /// 8-bit quantization (signed)
    Int8,

    /// 8-bit quantization (unsigned)
    UInt8,

    /// 16-bit floating point
    FP16,

    /// 32-bit floating point (no quantization)
    FP32,
}

impl fmt::Display for QuantizationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Int4 => write!(f, "int4"),
            Self::Int8 => write!(f, "int8"),
            Self::UInt8 => write!(f, "uint8"),
            Self::FP16 => write!(f, "fp16"),
            Self::FP32 => write!(f, "fp32"),
        }
    }
}

impl QuantizationLevel {
    /// Create from string representation
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl FromStr for QuantizationLevel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "no" => Some(Self::None),
            "int4" | "4bit" => Some(Self::Int4),
            "int8" | "8bit" | "signed8" => Some(Self::Int8),
            "uint8" | "unsigned8" => Some(Self::UInt8),
            "fp16" | "float16" | "half" => Some(Self::FP16),
            "fp32" | "float32" => Some(Self::FP32),
            _ => None,
        }
        .ok_or(())
    }
}

impl QuantizationLevel {
    /// Get the bit width for this quantization level
    pub fn bit_width(&self) -> usize {
        match self {
            Self::None => 32,
            Self::Int4 => 4,
            Self::Int8 | Self::UInt8 => 8,
            Self::FP16 => 16,
            Self::FP32 => 32,
        }
    }

    /// Check if this is a signed integer type
    pub fn is_signed_integer(&self) -> bool {
        matches!(self, Self::Int4 | Self::Int8)
    }

    /// Check if this is an unsigned integer type
    pub fn is_unsigned_integer(&self) -> bool {
        matches!(self, Self::UInt8)
    }

    /// Check if this is a floating-point type
    pub fn is_floating_point(&self) -> bool {
        matches!(self, Self::FP16 | Self::FP32)
    }
}

/// Configuration for quantization
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuantizationConfig {
    /// Quantization type
    pub quantization_type: QuantizationType,

    /// Quantization level
    pub quantization_level: QuantizationLevel,

    /// Whether to use asymmetric quantization
    pub asymmetric: bool,

    /// Whether to cache quantization results
    pub cache: bool,
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl QuantizationConfig {
    /// Create a new quantization config with no quantization
    pub fn new() -> Self {
        Self {
            quantization_type: QuantizationType::None,
            quantization_level: QuantizationLevel::None,
            asymmetric: false,
            cache: false,
        }
    }

    /// Create a scalar quantization config
    pub fn scalar(level: QuantizationLevel) -> Self {
        Self {
            quantization_type: QuantizationType::Scalar,
            quantization_level: level,
            asymmetric: false,
            cache: false,
        }
    }

    /// Create a product quantization config
    pub fn product(level: QuantizationLevel) -> Self {
        Self {
            quantization_type: QuantizationType::Product,
            quantization_level: level,
            asymmetric: false,
            cache: false,
        }
    }

    /// Create a binary quantization config
    pub fn binary() -> Self {
        Self {
            quantization_type: QuantizationType::Binary,
            quantization_level: QuantizationLevel::Int4, // Binary uses 1 bit per value
            asymmetric: false,
            cache: false,
        }
    }

    /// Enable asymmetric quantization
    pub fn with_asymmetric(mut self) -> Self {
        self.asymmetric = true;
        self
    }

    /// Enable caching
    pub fn with_cache(mut self) -> Self {
        self.cache = true;
        self
    }

    /// Get the quantization type
    pub fn quantization_type(&self) -> QuantizationType {
        self.quantization_type
    }

    /// Get the quantization level
    pub fn quantization_level(&self) -> QuantizationLevel {
        self.quantization_level
    }
}

// ============================================================================
// TurboQuant types (ADR-021, TURBOQUANT_LLD_2026_05_30)
// ============================================================================
//
// Gated by `experimental-turboquant`. These types are the foundation surface
// used by the modality crate's `QuantizationLevel::TurboQuant` variant and
// the root crate's `TurboQuantVectorData` struct. They are intentionally
// minimal: bit-width, calibration mode discriminator, and per-collection
// rotation seed. All algorithm logic (Lloyd-Max, rotation, encode pipeline,
// SIMD kernels) lives in `crates/modalities/proximadb-vector/src/quantization/
// turboquant/` and lands in P2+.

/// Per-coordinate calibration mode for TurboQuant. See LLD §6 (Q6, Q7).
#[cfg(feature = "experimental-turboquant")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationMode {
    /// Identity calibration — no TQ+ per-coord adjustment. Used when the first
    /// batch is below `TQPLUS_MIN_SAMPLES` (1000), or when the operator
    /// explicitly opts out. EXPLAIN surfaces `calibration_mode="identity"`.
    #[default]
    Identity,

    /// TQ+ per-coord `(shift, scale)` fit from empirical 5/95% quantiles of
    /// the first qualifying batch. Frozen after the first fit so all future
    /// adds quantize against the same target distribution. See LLD §6.
    TqPlus,
}

#[cfg(feature = "experimental-turboquant")]
impl fmt::Display for CalibrationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity => write!(f, "identity"),
            Self::TqPlus => write!(f, "tq_plus"),
        }
    }
}

#[cfg(feature = "experimental-turboquant")]
impl FromStr for CalibrationMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "identity" | "none" | "off" => Ok(Self::Identity),
            "tq_plus" | "tqplus" | "tq+" | "plus" => Ok(Self::TqPlus),
            _ => Err(()),
        }
    }
}

/// Derive the per-collection TurboQuant rotation seed from a collection id.
///
/// Per LLD Q3: `seed = u64::from_le_bytes(blake3("turboquant.v1.rotation" ||
/// collection_id).as_bytes()[0..8])`. Implemented here with a simple FNV-1a
/// hash to avoid pulling blake3 into the foundation crate; P8 may swap in
/// blake3 when the xCatalog wiring lands and the dependency is paid elsewhere.
///
/// The exact hash function is part of the wire contract — changing it
/// requires re-encoding every collection's codes. Mark this as load-bearing.
#[cfg(feature = "experimental-turboquant")]
pub fn derive_rotation_seed(collection_id: &str) -> u64 {
    // FNV-1a 64-bit. Deterministic, stdlib-only.
    let prefix = b"turboquant.v1.rotation:";
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in prefix.iter().chain(collection_id.as_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ============================================================================
// Legacy Type Conversions (for migration)
// ============================================================================

/// Legacy: ParsedQuantizationConfig from src/storage/traits/query.rs
#[deprecated(note = "Use QuantizationConfig instead")]
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedQuantizationConfig {
    pub quantization_type: String,
    pub quantization_level: String,
}

impl From<ParsedQuantizationConfig> for QuantizationConfig {
    fn from(legacy: ParsedQuantizationConfig) -> Self {
        let quantization_type =
            QuantizationType::from_str(&legacy.quantization_type).unwrap_or(QuantizationType::None);
        let quantization_level = QuantizationLevel::from_str(&legacy.quantization_level)
            .unwrap_or(QuantizationLevel::None);

        Self {
            quantization_type,
            quantization_level,
            asymmetric: false,
            cache: false,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantization_type_default() {
        assert_eq!(QuantizationType::default(), QuantizationType::None);
    }

    #[test]
    fn test_quantization_type_display() {
        assert_eq!(QuantizationType::None.to_string(), "none");
        assert_eq!(QuantizationType::Scalar.to_string(), "scalar");
        assert_eq!(QuantizationType::Product.to_string(), "product");
        assert_eq!(QuantizationType::Binary.to_string(), "binary");
    }

    #[test]
    fn test_quantization_type_from_str() {
        assert_eq!(
            QuantizationType::from_str("none"),
            Some(QuantizationType::None)
        );
        assert_eq!(
            QuantizationType::from_str("scalar"),
            Some(QuantizationType::Scalar)
        );
        assert_eq!(
            QuantizationType::from_str("int8"),
            Some(QuantizationType::Scalar)
        );
        assert_eq!(
            QuantizationType::from_str("product"),
            Some(QuantizationType::Product)
        );
        assert_eq!(
            QuantizationType::from_str("pq"),
            Some(QuantizationType::Product)
        );
        assert_eq!(
            QuantizationType::from_str("binary"),
            Some(QuantizationType::Binary)
        );
        assert_eq!(QuantizationType::from_str("unknown"), None);
    }

    #[test]
    fn test_quantization_type_is_fixed_point() {
        assert!(!QuantizationType::None.is_fixed_point());
        assert!(QuantizationType::Scalar.is_fixed_point());
        assert!(!QuantizationType::Product.is_fixed_point());
        assert!(QuantizationType::Binary.is_fixed_point());
    }

    #[test]
    fn test_quantization_level_default() {
        assert_eq!(QuantizationLevel::default(), QuantizationLevel::None);
    }

    #[test]
    fn test_quantization_level_display() {
        assert_eq!(QuantizationLevel::None.to_string(), "none");
        assert_eq!(QuantizationLevel::Int4.to_string(), "int4");
        assert_eq!(QuantizationLevel::Int8.to_string(), "int8");
        assert_eq!(QuantizationLevel::UInt8.to_string(), "uint8");
        assert_eq!(QuantizationLevel::FP16.to_string(), "fp16");
        assert_eq!(QuantizationLevel::FP32.to_string(), "fp32");
    }

    #[test]
    fn test_quantization_level_from_str() {
        assert_eq!(
            QuantizationLevel::from_str("none"),
            Some(QuantizationLevel::None)
        );
        assert_eq!(
            QuantizationLevel::from_str("int4"),
            Some(QuantizationLevel::Int4)
        );
        assert_eq!(
            QuantizationLevel::from_str("int8"),
            Some(QuantizationLevel::Int8)
        );
        assert_eq!(
            QuantizationLevel::from_str("uint8"),
            Some(QuantizationLevel::UInt8)
        );
        assert_eq!(
            QuantizationLevel::from_str("fp16"),
            Some(QuantizationLevel::FP16)
        );
        assert_eq!(
            QuantizationLevel::from_str("fp32"),
            Some(QuantizationLevel::FP32)
        );
        assert_eq!(QuantizationLevel::from_str("unknown"), None);
    }

    #[test]
    fn test_quantization_level_bit_width() {
        assert_eq!(QuantizationLevel::None.bit_width(), 32);
        assert_eq!(QuantizationLevel::Int4.bit_width(), 4);
        assert_eq!(QuantizationLevel::Int8.bit_width(), 8);
        assert_eq!(QuantizationLevel::UInt8.bit_width(), 8);
        assert_eq!(QuantizationLevel::FP16.bit_width(), 16);
        assert_eq!(QuantizationLevel::FP32.bit_width(), 32);
    }

    #[test]
    fn test_quantization_level_type_checks() {
        assert!(QuantizationLevel::Int4.is_signed_integer());
        assert!(QuantizationLevel::Int8.is_signed_integer());
        assert!(QuantizationLevel::UInt8.is_unsigned_integer());
        assert!(!QuantizationLevel::UInt8.is_signed_integer());
        assert!(QuantizationLevel::FP16.is_floating_point());
        assert!(QuantizationLevel::FP32.is_floating_point());
        assert!(!QuantizationLevel::Int8.is_floating_point());
    }

    #[test]
    fn test_quantization_config_default() {
        let config = QuantizationConfig::default();
        assert_eq!(config.quantization_type(), QuantizationType::None);
        assert_eq!(config.quantization_level(), QuantizationLevel::None);
        assert!(!config.asymmetric);
        assert!(!config.cache);
    }

    #[test]
    fn test_quantization_config_builder() {
        let config = QuantizationConfig::scalar(QuantizationLevel::Int8)
            .with_asymmetric()
            .with_cache();

        assert_eq!(config.quantization_type(), QuantizationType::Scalar);
        assert_eq!(config.quantization_level(), QuantizationLevel::Int8);
        assert!(config.asymmetric);
        assert!(config.cache);
    }

    #[test]
    fn test_quantization_config_constructors() {
        let scalar = QuantizationConfig::scalar(QuantizationLevel::Int8);
        assert_eq!(scalar.quantization_type(), QuantizationType::Scalar);
        assert_eq!(scalar.quantization_level(), QuantizationLevel::Int8);

        let product = QuantizationConfig::product(QuantizationLevel::Int4);
        assert_eq!(product.quantization_type(), QuantizationType::Product);
        assert_eq!(product.quantization_level(), QuantizationLevel::Int4);

        let binary = QuantizationConfig::binary();
        assert_eq!(binary.quantization_type(), QuantizationType::Binary);
        assert_eq!(binary.quantization_level(), QuantizationLevel::Int4);
    }

    #[test]
    fn test_quantization_serialization() {
        let qtype = QuantizationType::Scalar;
        let json = serde_json::to_string(&qtype).unwrap();
        assert_eq!(json, "\"scalar\"");

        let deserialized: QuantizationType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, QuantizationType::Scalar);

        let qlevel = QuantizationLevel::Int8;
        let json = serde_json::to_string(&qlevel).unwrap();
        assert_eq!(json, "\"int8\"");

        let deserialized: QuantizationLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, QuantizationLevel::Int8);
    }

    #[test]
    fn test_quantization_config_serialization() {
        let config = QuantizationConfig::scalar(QuantizationLevel::Int8).with_cache();
        let json = serde_json::to_string(&config).unwrap();

        let deserialized: QuantizationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.quantization_type(), QuantizationType::Scalar);
        assert!(deserialized.cache);
    }

    #[test]
    fn test_legacy_parsed_quantization_config_conversion() {
        let legacy = ParsedQuantizationConfig {
            quantization_type: "scalar".to_string(),
            quantization_level: "int8".to_string(),
        };

        let config: QuantizationConfig = legacy.into();
        assert_eq!(config.quantization_type(), QuantizationType::Scalar);
        assert_eq!(config.quantization_level(), QuantizationLevel::Int8);
    }

    // ------------------------------------------------------------------
    // TurboQuant types (P1 — ADR-021 / TURBOQUANT_LLD_2026_05_30)
    // ------------------------------------------------------------------

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_type_display_and_parse() {
        assert_eq!(QuantizationType::TurboQuant.to_string(), "turboquant");
        assert_eq!(
            QuantizationType::from_str("turboquant"),
            Some(QuantizationType::TurboQuant)
        );
        assert_eq!(
            QuantizationType::from_str("tq"),
            Some(QuantizationType::TurboQuant)
        );
        assert_eq!(
            QuantizationType::from_str("TURBOQUANT"),
            Some(QuantizationType::TurboQuant)
        );
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_is_fixed_point() {
        assert!(QuantizationType::TurboQuant.is_fixed_point());
        // is_floating_point is the negation, so this must be false.
        assert!(!QuantizationType::TurboQuant.is_floating_point());
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_calibration_mode_default_is_identity() {
        assert_eq!(CalibrationMode::default(), CalibrationMode::Identity);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_calibration_mode_display_and_parse() {
        assert_eq!(CalibrationMode::Identity.to_string(), "identity");
        assert_eq!(CalibrationMode::TqPlus.to_string(), "tq_plus");
        assert_eq!(
            CalibrationMode::from_str("identity"),
            Ok(CalibrationMode::Identity)
        );
        assert_eq!(
            CalibrationMode::from_str("tq+"),
            Ok(CalibrationMode::TqPlus)
        );
        assert_eq!(CalibrationMode::from_str("nonsense"), Err(()));
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_calibration_mode_serde_snake_case() {
        let s = serde_json::to_string(&CalibrationMode::TqPlus).unwrap();
        assert_eq!(s, "\"tq_plus\"");
        let v: CalibrationMode = serde_json::from_str("\"identity\"").unwrap();
        assert_eq!(v, CalibrationMode::Identity);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_derive_rotation_seed_is_deterministic_and_collision_resistant() {
        // Determinism: same id → same seed across calls.
        let a1 = derive_rotation_seed("col-abc");
        let a2 = derive_rotation_seed("col-abc");
        assert_eq!(a1, a2);
        // Multi-tenant: different ids → different seeds. With 64-bit output,
        // birthday-paradox risk at any realistic collection count is zero;
        // for FNV-1a these short distinct inputs are practically guaranteed
        // to land on different seeds.
        let b = derive_rotation_seed("col-def");
        assert_ne!(a1, b);
        // Spot-check that an empty collection id still produces a stable
        // (non-zero) seed.
        assert_ne!(derive_rotation_seed(""), 0);
    }
}
