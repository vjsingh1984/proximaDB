//! # ProximaDB Compression Types
//!
//! Foundation compression types for ProximaDB.

#![allow(deprecated)]
//!
//! ## Purpose
//!
//! This crate provides the single source of truth for compression types
//! across the entire ProximaDB codebase. It eliminates the proliferation of
//! duplicate compression definitions (20+ found in audit).
//!
//! ## Types
//!
//! - [`CompressionAlgorithm`] - Standardized compression algorithm enum
//! - [`CompressionConfig`] - Configuration for compression
//!
//! ## Migration
//!
//! If you're using legacy compression types, migrate to this crate's types
//! using the provided conversion traits.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Standardized compression algorithm enum.
///
/// This is the single source of truth for compression algorithms across ProximaDB.
/// All other compression type definitions should migrate to use this enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgorithm {
    /// No compression
    #[default]
    None,
    /// Fast compression/decompression, moderate compression ratio
    Snappy,
    /// Very fast compression/decompression, moderate compression ratio
    Lz4,
    /// Good compression ratio, fast compression/decompression
    Zstd,
    /// Good compression ratio, moderate speed
    Gzip,
    /// Excellent compression ratio, slower compression
    Brotli,
    /// Legacy Bzip2 support
    Bzip2,
    /// Raw Deflate (ZIP compatible)
    Deflate,
    /// XZ / LZMA2 high-ratio compression
    Xz,
    /// Zlib compression
    Zlib,
    /// LZO placeholder (falls back to LZ4 at runtime; no Rust impl)
    Lzo,
    /// LZ4 high-compression variant
    Lz4hc,
    /// LZMA (implemented via XZ at max level)
    Lzma,
    /// Mixed per-column strategy: selects algorithm based on column data type
    Mixed,
}

impl fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Snappy => write!(f, "snappy"),
            Self::Lz4 => write!(f, "lz4"),
            Self::Zstd => write!(f, "zstd"),
            Self::Gzip => write!(f, "gzip"),
            Self::Brotli => write!(f, "brotli"),
            Self::Bzip2 => write!(f, "bzip2"),
            Self::Deflate => write!(f, "deflate"),
            Self::Xz => write!(f, "xz"),
            Self::Zlib => write!(f, "zlib"),
            Self::Lzo => write!(f, "lzo"),
            Self::Lz4hc => write!(f, "lz4hc"),
            Self::Lzma => write!(f, "lzma"),
            Self::Mixed => write!(f, "mixed"),
        }
    }
}

impl CompressionAlgorithm {
    /// Create from string representation
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// Check if this compression algorithm is lossless
    pub fn is_lossless(&self) -> bool {
        *self != Self::None
    }

    /// Get the compression level range for this algorithm.
    pub fn level_range(&self) -> Option<(i32, i32)> {
        match self {
            Self::None | Self::Snappy | Self::Lzo | Self::Mixed => None,
            Self::Lz4 | Self::Lz4hc => Some((0, 16)),
            Self::Zstd => Some((1, 22)),
            Self::Gzip | Self::Deflate | Self::Zlib => Some((0, 9)),
            Self::Brotli => Some((0, 11)),
            Self::Bzip2 => Some((1, 9)),
            Self::Xz | Self::Lzma => Some((0, 9)),
        }
    }
}

impl FromStr for CompressionAlgorithm {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "no" => Some(Self::None),
            "snappy" => Some(Self::Snappy),
            "lz4" => Some(Self::Lz4),
            "zstd" | "zstandard" => Some(Self::Zstd),
            "gzip" => Some(Self::Gzip),
            "brotli" => Some(Self::Brotli),
            "bzip2" => Some(Self::Bzip2),
            "deflate" => Some(Self::Deflate),
            "xz" => Some(Self::Xz),
            "zlib" => Some(Self::Zlib),
            "lzo" => Some(Self::Lzo),
            "lz4hc" => Some(Self::Lz4hc),
            "lzma" => Some(Self::Lzma),
            "mixed" => Some(Self::Mixed),
            _ => None,
        }
        .ok_or(())
    }
}

impl CompressionAlgorithm {

    /// Get the default compression level for this algorithm
    pub fn default_level(&self) -> Option<i32> {
        match self {
            Self::None | Self::Snappy | Self::Lzo | Self::Mixed => None,
            Self::Lz4 | Self::Lz4hc => Some(0),
            Self::Zstd => Some(3),
            Self::Gzip | Self::Deflate | Self::Zlib => Some(6),
            Self::Brotli => Some(4),
            Self::Bzip2 => Some(6),
            Self::Xz | Self::Lzma => Some(6),
        }
    }
}

/// Configuration for compression
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Compression algorithm
    pub algorithm: CompressionAlgorithm,

    /// Compression level (if applicable)
    pub level: Option<i32>,

    /// Whether to use streaming compression
    pub streaming: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CompressionConfig {
    /// Create a new compression config with no compression
    pub fn new() -> Self {
        Self {
            algorithm: CompressionAlgorithm::None,
            level: None,
            streaming: false,
        }
    }

    /// Create a compression config for a specific algorithm
    pub fn with_algorithm(algorithm: CompressionAlgorithm) -> Self {
        let level = algorithm.default_level();
        Self {
            algorithm,
            level,
            streaming: false,
        }
    }

    /// Set the compression level
    pub fn with_level(mut self, level: i32) -> Self {
        self.level = Some(level);
        self
    }

    /// Enable streaming compression
    pub fn with_streaming(mut self) -> Self {
        self.streaming = true;
        self
    }

    /// Create a Snappy compression config
    pub fn snappy() -> Self {
        Self::with_algorithm(CompressionAlgorithm::Snappy)
    }

    /// Create an LZ4 compression config
    pub fn lz4() -> Self {
        Self::with_algorithm(CompressionAlgorithm::Lz4)
    }

    /// Create a Zstd compression config
    pub fn zstd() -> Self {
        Self::with_algorithm(CompressionAlgorithm::Zstd)
    }

    /// Create a Gzip compression config
    pub fn gzip() -> Self {
        Self::with_algorithm(CompressionAlgorithm::Gzip)
    }

    /// Create a Brotli compression config
    pub fn brotli() -> Self {
        Self::with_algorithm(CompressionAlgorithm::Brotli)
    }

    /// Get the compression algorithm
    pub fn algorithm(&self) -> CompressionAlgorithm {
        self.algorithm
    }

    /// Get the compression level
    pub fn level(&self) -> Option<i32> {
        self.level
    }
}

// ============================================================================
// Legacy Type Conversions (for migration)
// ============================================================================

/// Legacy: CompressionCodec from src/storage/formats/traits.rs
#[deprecated(note = "Use CompressionAlgorithm instead")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionCodec {
    None,
    Snappy,
    Lz4,
    Zstd,
    Gzip,
}

impl From<CompressionCodec> for CompressionAlgorithm {
    fn from(legacy: CompressionCodec) -> Self {
        match legacy {
            CompressionCodec::None => Self::None,
            CompressionCodec::Snappy => Self::Snappy,
            CompressionCodec::Lz4 => Self::Lz4,
            CompressionCodec::Zstd => Self::Zstd,
            CompressionCodec::Gzip => Self::Gzip,
        }
    }
}

impl From<CompressionAlgorithm> for CompressionCodec {
    fn from(algorithm: CompressionAlgorithm) -> Self {
        match algorithm {
            CompressionAlgorithm::None => Self::None,
            CompressionAlgorithm::Snappy => Self::Snappy,
            CompressionAlgorithm::Lz4 | CompressionAlgorithm::Lz4hc => Self::Lz4,
            CompressionAlgorithm::Zstd => Self::Zstd,
            CompressionAlgorithm::Gzip => Self::Gzip,
            // Fallback: unsupported algorithms default to None in this legacy codec
            _ => Self::None,
        }
    }
}

/// Legacy: FlightCompression from src/network/arrow_ipc/file_export.rs
#[deprecated(note = "Use CompressionAlgorithm instead")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightCompression {
    None,
    Lz4,
    Zstd,
}

impl From<FlightCompression> for CompressionAlgorithm {
    fn from(legacy: FlightCompression) -> Self {
        match legacy {
            FlightCompression::None => Self::None,
            FlightCompression::Lz4 => Self::Lz4,
            FlightCompression::Zstd => Self::Zstd,
        }
    }
}

impl From<CompressionAlgorithm> for FlightCompression {
    fn from(algorithm: CompressionAlgorithm) -> Self {
        match algorithm {
            CompressionAlgorithm::None => Self::None,
            CompressionAlgorithm::Lz4 | CompressionAlgorithm::Lz4hc => Self::Lz4,
            CompressionAlgorithm::Zstd => Self::Zstd,
            // Arrow Flight only supports Lz4 and Zstd; fallback to None
            _ => Self::None,
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
    fn test_compression_algorithm_default() {
        assert_eq!(CompressionAlgorithm::default(), CompressionAlgorithm::None);
    }

    #[test]
    fn test_compression_algorithm_display() {
        assert_eq!(CompressionAlgorithm::None.to_string(), "none");
        assert_eq!(CompressionAlgorithm::Snappy.to_string(), "snappy");
        assert_eq!(CompressionAlgorithm::Lz4.to_string(), "lz4");
        assert_eq!(CompressionAlgorithm::Zstd.to_string(), "zstd");
        assert_eq!(CompressionAlgorithm::Gzip.to_string(), "gzip");
        assert_eq!(CompressionAlgorithm::Brotli.to_string(), "brotli");
    }

    #[test]
    fn test_compression_algorithm_from_str() {
        assert_eq!(
            CompressionAlgorithm::from_str("none"),
            Some(CompressionAlgorithm::None)
        );
        assert_eq!(
            CompressionAlgorithm::from_str("snappy"),
            Some(CompressionAlgorithm::Snappy)
        );
        assert_eq!(
            CompressionAlgorithm::from_str("lz4"),
            Some(CompressionAlgorithm::Lz4)
        );
        assert_eq!(
            CompressionAlgorithm::from_str("zstd"),
            Some(CompressionAlgorithm::Zstd)
        );
        assert_eq!(
            CompressionAlgorithm::from_str("gzip"),
            Some(CompressionAlgorithm::Gzip)
        );
        assert_eq!(
            CompressionAlgorithm::from_str("brotli"),
            Some(CompressionAlgorithm::Brotli)
        );
        assert_eq!(CompressionAlgorithm::from_str("unknown"), None);
    }

    #[test]
    fn test_compression_algorithm_is_lossless() {
        assert!(!CompressionAlgorithm::None.is_lossless());
        assert!(CompressionAlgorithm::Snappy.is_lossless());
        assert!(CompressionAlgorithm::Lz4.is_lossless());
        assert!(CompressionAlgorithm::Zstd.is_lossless());
        assert!(CompressionAlgorithm::Gzip.is_lossless());
        assert!(CompressionAlgorithm::Brotli.is_lossless());
    }

    #[test]
    fn test_compression_algorithm_level_range() {
        assert_eq!(CompressionAlgorithm::None.level_range(), None);
        assert_eq!(CompressionAlgorithm::Snappy.level_range(), None);
        assert_eq!(CompressionAlgorithm::Lz4.level_range(), Some((0, 16)));
        assert_eq!(CompressionAlgorithm::Zstd.level_range(), Some((1, 22)));
        assert_eq!(CompressionAlgorithm::Gzip.level_range(), Some((0, 9)));
        assert_eq!(CompressionAlgorithm::Brotli.level_range(), Some((0, 11)));
    }

    #[test]
    fn test_compression_algorithm_default_level() {
        assert_eq!(CompressionAlgorithm::None.default_level(), None);
        assert_eq!(CompressionAlgorithm::Snappy.default_level(), None);
        assert_eq!(CompressionAlgorithm::Lz4.default_level(), Some(0));
        assert_eq!(CompressionAlgorithm::Zstd.default_level(), Some(3));
        assert_eq!(CompressionAlgorithm::Gzip.default_level(), Some(6));
        assert_eq!(CompressionAlgorithm::Brotli.default_level(), Some(4));
    }

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert_eq!(config.algorithm(), CompressionAlgorithm::None);
        assert_eq!(config.level(), None);
        assert!(!config.streaming);
    }

    #[test]
    fn test_compression_config_builder() {
        let config = CompressionConfig::zstd().with_level(10).with_streaming();

        assert_eq!(config.algorithm(), CompressionAlgorithm::Zstd);
        assert_eq!(config.level(), Some(10));
        assert!(config.streaming);
    }

    #[test]
    fn test_compression_config_constructors() {
        assert_eq!(
            CompressionConfig::snappy().algorithm(),
            CompressionAlgorithm::Snappy
        );
        assert_eq!(
            CompressionConfig::lz4().algorithm(),
            CompressionAlgorithm::Lz4
        );
        assert_eq!(
            CompressionConfig::zstd().algorithm(),
            CompressionAlgorithm::Zstd
        );
        assert_eq!(
            CompressionConfig::gzip().algorithm(),
            CompressionAlgorithm::Gzip
        );
        assert_eq!(
            CompressionConfig::brotli().algorithm(),
            CompressionAlgorithm::Brotli
        );
    }

    #[test]
    fn test_legacy_compression_codec_conversion() {
        // Legacy -> New
        assert_eq!(
            CompressionAlgorithm::from(CompressionCodec::None),
            CompressionAlgorithm::None
        );
        assert_eq!(
            CompressionAlgorithm::from(CompressionCodec::Snappy),
            CompressionAlgorithm::Snappy
        );
        assert_eq!(
            CompressionAlgorithm::from(CompressionCodec::Lz4),
            CompressionAlgorithm::Lz4
        );

        // New -> Legacy
        assert_eq!(
            CompressionCodec::from(CompressionAlgorithm::None),
            CompressionCodec::None
        );
        assert_eq!(
            CompressionCodec::from(CompressionAlgorithm::Snappy),
            CompressionCodec::Snappy
        );
    }

    #[test]
    fn test_legacy_compression_codec_unsupported_falls_back_to_none() {
        // Algorithms not representable in the 5-variant legacy codec default to None
        assert_eq!(
            CompressionCodec::from(CompressionAlgorithm::Brotli),
            CompressionCodec::None
        );
        assert_eq!(
            CompressionCodec::from(CompressionAlgorithm::Bzip2),
            CompressionCodec::None
        );
        assert_eq!(
            CompressionCodec::from(CompressionAlgorithm::Mixed),
            CompressionCodec::None
        );
    }

    #[test]
    fn test_legacy_flight_compression_conversion() {
        // Legacy -> New
        assert_eq!(
            CompressionAlgorithm::from(FlightCompression::None),
            CompressionAlgorithm::None
        );
        assert_eq!(
            CompressionAlgorithm::from(FlightCompression::Lz4),
            CompressionAlgorithm::Lz4
        );
        assert_eq!(
            CompressionAlgorithm::from(FlightCompression::Zstd),
            CompressionAlgorithm::Zstd
        );

        // New -> Legacy
        assert_eq!(
            FlightCompression::from(CompressionAlgorithm::None),
            FlightCompression::None
        );
        assert_eq!(
            FlightCompression::from(CompressionAlgorithm::Lz4),
            FlightCompression::Lz4
        );
    }

    #[test]
    fn test_legacy_flight_compression_unsupported_falls_back_to_none() {
        // Arrow Flight only supports Lz4/Zstd; everything else falls back to None
        assert_eq!(
            FlightCompression::from(CompressionAlgorithm::Gzip),
            FlightCompression::None
        );
        assert_eq!(
            FlightCompression::from(CompressionAlgorithm::Brotli),
            FlightCompression::None
        );
        assert_eq!(
            FlightCompression::from(CompressionAlgorithm::Bzip2),
            FlightCompression::None
        );
    }

    #[test]
    fn test_compression_algorithm_serialization() {
        let algorithm = CompressionAlgorithm::Zstd;
        let json = serde_json::to_string(&algorithm).unwrap();
        assert_eq!(json, "\"zstd\"");

        let deserialized: CompressionAlgorithm = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_compression_config_serialization() {
        let config = CompressionConfig::gzip().with_streaming();
        let json = serde_json::to_string(&config).unwrap();

        let deserialized: CompressionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.algorithm(), CompressionAlgorithm::Gzip);
        assert!(deserialized.streaming);
    }
}
