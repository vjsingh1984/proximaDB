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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuantizationType {
    /// No quantization
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
}

impl Default for QuantizationType {
    fn default() -> Self {
        Self::None
    }
}

impl fmt::Display for QuantizationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Scalar => write!(f, "scalar"),
            Self::Product => write!(f, "product"),
            Self::Binary => write!(f, "binary"),
        }
    }
}

impl QuantizationType {
    /// Create from string representation
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" | "no" => Some(Self::None),
            "scalar" | "int8" | "uint8" | "scalarquantization" => Some(Self::Scalar),
            "product" | "pq" | "productquantization" => Some(Self::Product),
            "binary" | "bit" | "binaryquantization" => Some(Self::Binary),
            _ => None,
        }
    }

    /// Check if this quantization type uses fixed-point arithmetic
    pub fn is_fixed_point(&self) -> bool {
        matches!(self, Self::Scalar | Self::Binary)
    }

    /// Check if this quantization type uses floating-point arithmetic
    pub fn is_floating_point(&self) -> bool {
        !self.is_fixed_point() && *self != Self::None
    }
}

/// Standardized quantization level enum.
///
/// This represents the bit-width or precision level for quantization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuantizationLevel {
    /// No quantization
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

impl Default for QuantizationLevel {
    fn default() -> Self {
        Self::None
    }
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
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" | "no" => Some(Self::None),
            "int4" | "4bit" => Some(Self::Int4),
            "int8" | "8bit" | "signed8" => Some(Self::Int8),
            "uint8" | "unsigned8" => Some(Self::UInt8),
            "fp16" | "float16" | "half" => Some(Self::FP16),
            "fp32" | "float32" => Some(Self::FP32),
            _ => None,
        }
    }

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
}
