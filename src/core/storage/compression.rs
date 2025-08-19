//! Compression algorithms and configuration

use serde::{Deserialize, Serialize};
use crate::core::foundation::BaseConfig;

/// Unified compression algorithm enum - replaces 10+ duplicates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    /// No compression
    None,
    /// LZ4 fast compression
    Lz4,
    /// LZ4 high compression
    Lz4Hc,
    /// Zstandard compression with configurable level
    Zstd { level: i32 },
    /// Snappy compression  
    Snappy,
    /// GZIP compression
    Gzip,
    /// Deflate compression
    Deflate,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        // Smart default: Snappy provides good balance of compression ratio and speed
        Self::Snappy
    }
}

/// Unified compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Algorithm to use
    pub algorithm: CompressionAlgorithm,
    /// Compression level (1-9, algorithm dependent)
    pub level: u8,
    /// Enable compression for vectors
    pub compress_vectors: bool,
    /// Enable compression for metadata
    pub compress_metadata: bool,
    /// Minimum file size to compress (bytes)
    pub min_compress_size: usize,
    /// Target compression ratio (0.0-1.0, 0.5 = 50% compression)
    pub target_ratio: f32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::default(),
            level: 3, // Balanced compression
            compress_vectors: true,
            compress_metadata: true,
            min_compress_size: 1024, // 1KB minimum
            target_ratio: 0.5, // 50% target compression
        }
    }
}

impl BaseConfig for CompressionConfig {
    fn validate(&self) -> Result<(), String> {
        if self.level > 9 {
            return Err("Compression level must be between 1-9".to_string());
        }
        if !(0.0..=1.0).contains(&self.target_ratio) {
            return Err("Target ratio must be between 0.0 and 1.0".to_string());
        }
        Ok(())
    }
}