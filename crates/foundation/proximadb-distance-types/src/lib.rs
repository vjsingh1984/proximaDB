//! # ProximaDB Distance Types
//!
//! Foundation distance metric types for ProximaDB.

#![allow(deprecated)]
//!
//! ## Purpose
//!
//! This crate provides the single source of truth for distance metric types
//! across the entire ProximaDB codebase. It eliminates the proliferation of
//! duplicate distance metric definitions (40+ found in audit).
//!
//! ## Types
//!
//! - [`DistanceMetric`] - Standardized distance metric enum
//! - [`DistanceConfig`] - Configuration for distance computation
//! - [`DistanceMode`] - Computation mode (quantized, exact, etc.)
//!
//! ## Migration
//!
//! If you're using legacy distance types (`CompactDistanceMetric`, `DuckDBDistanceMetric`, etc.),
//! migrate to this crate's types using the provided conversion traits.
//!
//! ```rust
//! use proximadb_distance_types::{DistanceMetric, DistanceConfig};
//!
//! let config = DistanceConfig::new(DistanceMetric::L2);
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Standardized distance metric enum.
///
/// This is the single source of truth for distance metrics across ProximaDB.
/// All other distance metric types should migrate to use this enum.
///
/// ## Variants
///
/// - `L2` - Euclidean distance (L2 norm)
/// - `Cosine` - Cosine similarity/distance
/// - `InnerProduct` - Inner product (dot product)
/// - `L1` - Manhattan distance (L1 norm)
///
/// ## Migration Notes
///
/// Legacy type mappings:
/// - `CompactDistanceMetric::Euclidean` → `DistanceMetric::L2`
/// - `DistanceMetric::Euclidean` → `DistanceMetric::L2`
/// - `DistanceMetric::DotProduct` → `DistanceMetric::InnerProduct`
/// - `DistanceMetric::Manhattan` → `DistanceMetric::L1`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistanceMetric {
    /// Euclidean distance (L2 norm)
    ///
    /// Also known as: Euclidean, L2, SquaredEuclidean
    ///
    /// Formula: `sqrt(sum((a_i - b_i)^2))`
    #[default]
    L2,

    /// Cosine similarity/distance
    ///
    /// Formula: `1 - (a · b) / (||a|| * ||b||)`
    Cosine,

    /// Inner product (dot product)
    ///
    /// Also known as: DotProduct, InnerProduct
    ///
    /// Formula: `sum(a_i * b_i)`
    InnerProduct,

    /// Manhattan distance (L1 norm)
    ///
    /// Also known as: Manhattan, L1, Taxicab
    ///
    /// Formula: `sum(|a_i - b_i|)`
    L1,
}

impl fmt::Display for DistanceMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::L2 => write!(f, "l2"),
            Self::Cosine => write!(f, "cosine"),
            Self::InnerProduct => write!(f, "inner_product"),
            Self::L1 => write!(f, "l1"),
        }
    }
}

impl DistanceMetric {
    /// Create from string representation
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use proximadb_distance_types::DistanceMetric;
    ///
    /// assert_eq!(DistanceMetric::from_str("l2"), Some(DistanceMetric::L2));
    /// assert_eq!(DistanceMetric::from_str("cosine"), Some(DistanceMetric::Cosine));
    /// assert_eq!(DistanceMetric::from_str("unknown"), None);
    /// ```
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl FromStr for DistanceMetric {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "l2" | "euclidean" | "euclideandistance" => Some(Self::L2),
            "cosine" | "cosinesimilarity" => Some(Self::Cosine),
            "innerproduct" | "dotproduct" | "dot" => Some(Self::InnerProduct),
            "l1" | "manhattan" | "manhattandistance" => Some(Self::L1),
            _ => None,
        }
        .ok_or(())
    }
}

impl DistanceMetric {

    /// Check if this is a similarity metric (higher is better)
    ///
    /// Similarity metrics: InnerProduct
    /// Distance metrics: L2, Cosine, L1 (lower is better)
    pub fn is_similarity(&self) -> bool {
        matches!(self, Self::InnerProduct)
    }

    /// Check if this is a distance metric (lower is better)
    pub fn is_distance(&self) -> bool {
        !self.is_similarity()
    }
}

/// Configuration for distance computation
///
/// ## Examples
///
/// ```rust
/// use proximadb_distance_types::{DistanceMetric, DistanceConfig};
///
/// let config = DistanceConfig::new(DistanceMetric::L2);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistanceConfig {
    /// Distance metric to use
    pub metric: DistanceMetric,

    /// Whether to use quantized computation (if available)
    pub quantized: bool,

    /// Whether to cache computed distances
    pub cache: bool,
}

impl Default for DistanceConfig {
    fn default() -> Self {
        Self::new(DistanceMetric::L2)
    }
}

impl DistanceConfig {
    /// Create a new distance config with the specified metric
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use proximadb_distance_types::{DistanceMetric, DistanceConfig};
    ///
    /// let config = DistanceConfig::new(DistanceMetric::Cosine);
    /// ```
    pub fn new(metric: DistanceMetric) -> Self {
        Self {
            metric,
            quantized: false,
            cache: false,
        }
    }

    /// Enable quantized computation
    pub fn with_quantized(mut self) -> Self {
        self.quantized = true;
        self
    }

    /// Enable caching
    pub fn with_cache(mut self) -> Self {
        self.cache = true;
        self
    }

    /// Create a cosine distance config
    pub fn cosine() -> Self {
        Self::new(DistanceMetric::Cosine)
    }

    /// Create an L2 distance config
    pub fn l2() -> Self {
        Self::new(DistanceMetric::L2)
    }

    /// Create an inner product config
    pub fn inner_product() -> Self {
        Self::new(DistanceMetric::InnerProduct)
    }

    /// Create an L1 distance config
    pub fn l1() -> Self {
        Self::new(DistanceMetric::L1)
    }
}

/// Distance computation mode
///
/// ## Examples
///
/// ```rust
/// use proximadb_distance_types::DistanceMode;
///
/// let mode = DistanceMode::Exact;
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DistanceMode {
    /// Exact distance computation
    #[default]
    Exact,

    /// Quantized distance computation (approximate)
    Quantized,

    /// SIMD-accelerated distance computation
    Simd,

    /// Streaming distance computation (for large vectors)
    Streaming,
}

// ============================================================================
// Legacy Type Conversions (for migration)
// ============================================================================
//
// NOTE: These conversions are provided for ProximaDB-internal types only.
// External connector conversions (e.g., for DuckDB, PostgreSQL) should be
// implemented in the connector modules themselves, not in foundation types.
// This keeps foundation types independent of external dependencies.
//
// Example for external connectors (in src/connectors/duckdb.rs):
// ```rust
// use proximadb_distance_types::DistanceMetric;
//
// enum DuckDBDistanceMetric { Euclidean, Cosine }
//
// impl From<DuckDBDistanceMetric> for DistanceMetric {
//     fn from(legacy: DuckDBDistanceMetric) -> Self {
//         match legacy {
//             DuckDBDistanceMetric::Euclidean => Self::L2,
//             DuckDBDistanceMetric::Cosine => Self::Cosine,
//         }
//     }
// }
// ```
// ============================================================================

/// Legacy: CompactDistanceMetric from src/core/compact_enums.rs
#[deprecated(note = "Use DistanceMetric instead")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactDistanceMetric {
    Euclidean,
    Cosine,
    DotProduct,
    Manhattan,
}

impl From<CompactDistanceMetric> for DistanceMetric {
    fn from(legacy: CompactDistanceMetric) -> Self {
        match legacy {
            CompactDistanceMetric::Euclidean => Self::L2,
            CompactDistanceMetric::Cosine => Self::Cosine,
            CompactDistanceMetric::DotProduct => Self::InnerProduct,
            CompactDistanceMetric::Manhattan => Self::L1,
        }
    }
}

impl From<DistanceMetric> for CompactDistanceMetric {
    fn from(metric: DistanceMetric) -> Self {
        match metric {
            DistanceMetric::L2 => Self::Euclidean,
            DistanceMetric::Cosine => Self::Cosine,
            DistanceMetric::InnerProduct => Self::DotProduct,
            DistanceMetric::L1 => Self::Manhattan,
        }
    }
}

/// Legacy: NetworkDistanceMetric from src/network/unified_handler.rs
#[deprecated(note = "Use DistanceMetric instead")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkDistanceMetric {
    Euclidean,
    Cosine,
    DotProduct,
    Manhattan,
}

impl From<NetworkDistanceMetric> for DistanceMetric {
    fn from(legacy: NetworkDistanceMetric) -> Self {
        match legacy {
            NetworkDistanceMetric::Euclidean => Self::L2,
            NetworkDistanceMetric::Cosine => Self::Cosine,
            NetworkDistanceMetric::DotProduct => Self::InnerProduct,
            NetworkDistanceMetric::Manhattan => Self::L1,
        }
    }
}

impl From<DistanceMetric> for NetworkDistanceMetric {
    fn from(metric: DistanceMetric) -> Self {
        match metric {
            DistanceMetric::L2 => Self::Euclidean,
            DistanceMetric::Cosine => Self::Cosine,
            DistanceMetric::InnerProduct => Self::DotProduct,
            DistanceMetric::L1 => Self::Manhattan,
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
    fn test_distance_metric_default() {
        assert_eq!(DistanceMetric::default(), DistanceMetric::L2);
    }

    #[test]
    fn test_distance_metric_display() {
        assert_eq!(DistanceMetric::L2.to_string(), "l2");
        assert_eq!(DistanceMetric::Cosine.to_string(), "cosine");
        assert_eq!(DistanceMetric::InnerProduct.to_string(), "inner_product");
        assert_eq!(DistanceMetric::L1.to_string(), "l1");
    }

    #[test]
    fn test_distance_metric_from_str() {
        assert_eq!(DistanceMetric::from_str("l2"), Some(DistanceMetric::L2));
        assert_eq!(DistanceMetric::from_str("L2"), Some(DistanceMetric::L2));
        assert_eq!(
            DistanceMetric::from_str("euclidean"),
            Some(DistanceMetric::L2)
        );
        assert_eq!(
            DistanceMetric::from_str("cosine"),
            Some(DistanceMetric::Cosine)
        );
        assert_eq!(
            DistanceMetric::from_str("innerproduct"),
            Some(DistanceMetric::InnerProduct)
        );
        assert_eq!(
            DistanceMetric::from_str("dotproduct"),
            Some(DistanceMetric::InnerProduct)
        );
        assert_eq!(DistanceMetric::from_str("l1"), Some(DistanceMetric::L1));
        assert_eq!(
            DistanceMetric::from_str("manhattan"),
            Some(DistanceMetric::L1)
        );
        assert_eq!(DistanceMetric::from_str("unknown"), None);
    }

    #[test]
    fn test_distance_metric_is_similarity() {
        assert!(!DistanceMetric::L2.is_similarity());
        assert!(!DistanceMetric::Cosine.is_similarity());
        assert!(DistanceMetric::InnerProduct.is_similarity());
        assert!(!DistanceMetric::L1.is_similarity());
    }

    #[test]
    fn test_distance_metric_is_distance() {
        assert!(DistanceMetric::L2.is_distance());
        assert!(DistanceMetric::Cosine.is_distance());
        assert!(!DistanceMetric::InnerProduct.is_distance());
        assert!(DistanceMetric::L1.is_distance());
    }

    #[test]
    fn test_distance_config_default() {
        let config = DistanceConfig::default();
        assert_eq!(config.metric, DistanceMetric::L2);
        assert!(!config.quantized);
        assert!(!config.cache);
    }

    #[test]
    fn test_distance_config_builder() {
        let config = DistanceConfig::new(DistanceMetric::Cosine)
            .with_quantized()
            .with_cache();

        assert_eq!(config.metric, DistanceMetric::Cosine);
        assert!(config.quantized);
        assert!(config.cache);
    }

    #[test]
    fn test_distance_config_constructors() {
        assert_eq!(DistanceConfig::cosine().metric, DistanceMetric::Cosine);
        assert_eq!(DistanceConfig::l2().metric, DistanceMetric::L2);
        assert_eq!(
            DistanceConfig::inner_product().metric,
            DistanceMetric::InnerProduct
        );
        assert_eq!(DistanceConfig::l1().metric, DistanceMetric::L1);
    }

    #[test]
    fn test_distance_mode_default() {
        assert_eq!(DistanceMode::default(), DistanceMode::Exact);
    }

    #[test]
    fn test_legacy_compact_distance_metric_conversion() {
        // Legacy -> New
        assert_eq!(
            DistanceMetric::from(CompactDistanceMetric::Euclidean),
            DistanceMetric::L2
        );
        assert_eq!(
            DistanceMetric::from(CompactDistanceMetric::Cosine),
            DistanceMetric::Cosine
        );
        assert_eq!(
            DistanceMetric::from(CompactDistanceMetric::DotProduct),
            DistanceMetric::InnerProduct
        );
        assert_eq!(
            DistanceMetric::from(CompactDistanceMetric::Manhattan),
            DistanceMetric::L1
        );

        // New -> Legacy
        assert_eq!(
            CompactDistanceMetric::from(DistanceMetric::L2),
            CompactDistanceMetric::Euclidean
        );
        assert_eq!(
            CompactDistanceMetric::from(DistanceMetric::Cosine),
            CompactDistanceMetric::Cosine
        );
        assert_eq!(
            CompactDistanceMetric::from(DistanceMetric::InnerProduct),
            CompactDistanceMetric::DotProduct
        );
        assert_eq!(
            CompactDistanceMetric::from(DistanceMetric::L1),
            CompactDistanceMetric::Manhattan
        );
    }

    #[test]
    fn test_legacy_network_distance_metric_conversion() {
        // Legacy -> New
        assert_eq!(
            DistanceMetric::from(NetworkDistanceMetric::Euclidean),
            DistanceMetric::L2
        );
        assert_eq!(
            DistanceMetric::from(NetworkDistanceMetric::Cosine),
            DistanceMetric::Cosine
        );
        assert_eq!(
            DistanceMetric::from(NetworkDistanceMetric::DotProduct),
            DistanceMetric::InnerProduct
        );
        assert_eq!(
            DistanceMetric::from(NetworkDistanceMetric::Manhattan),
            DistanceMetric::L1
        );

        // New -> Legacy
        assert_eq!(
            NetworkDistanceMetric::from(DistanceMetric::L2),
            NetworkDistanceMetric::Euclidean
        );
        assert_eq!(
            NetworkDistanceMetric::from(DistanceMetric::Cosine),
            NetworkDistanceMetric::Cosine
        );
        assert_eq!(
            NetworkDistanceMetric::from(DistanceMetric::InnerProduct),
            NetworkDistanceMetric::DotProduct
        );
        assert_eq!(
            NetworkDistanceMetric::from(DistanceMetric::L1),
            NetworkDistanceMetric::Manhattan
        );
    }

    #[test]
    fn test_distance_metric_serialization() {
        let metric = DistanceMetric::L2;
        let json = serde_json::to_string(&metric).unwrap();
        assert_eq!(json, "\"l2\"");

        let deserialized: DistanceMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, DistanceMetric::L2);
    }

    #[test]
    fn test_distance_config_serialization() {
        let config = DistanceConfig::new(DistanceMetric::Cosine).with_cache();
        let json = serde_json::to_string(&config).unwrap();

        let deserialized: DistanceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.metric, DistanceMetric::Cosine);
        assert!(deserialized.cache);
    }
}
