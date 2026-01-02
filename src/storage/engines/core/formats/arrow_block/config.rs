//! Arrow Block Configuration
//!
//! Configuration types for Arrow block storage format.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Configuration for Arrow block storage
#[derive(Debug, Clone)]
pub struct ArrowBlockConfig {
    /// Vector dimension
    pub dimension: u32,

    /// Target records per block
    pub records_per_block: u32,

    /// Enable compression (uses Arrow's built-in LZ4)
    pub compression_enabled: bool,

    /// Compression level (1-12 for LZ4, 1-22 for ZSTD)
    pub compression_level: i32,

    /// Compression codec
    pub compression_codec: CompressionCodec,

    /// Enable bloom filter for ID lookups
    pub bloom_filter_enabled: bool,

    /// Bloom filter false positive rate
    pub bloom_filter_fpr: f64,

    /// Enable B+ tree index for O(log n) lookups
    pub bplus_index_enabled: bool,

    /// Custom metadata to embed in Arrow schema
    pub custom_metadata: HashMap<String, String>,
}

/// Compression codec for Arrow IPC
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionCodec {
    /// No compression
    None,
    /// LZ4 Frame compression (fast)
    Lz4Frame,
    /// ZSTD compression (better ratio)
    Zstd,
}

impl Default for ArrowBlockConfig {
    fn default() -> Self {
        Self {
            dimension: 768,
            records_per_block: 2000,
            compression_enabled: true,
            compression_level: 3,
            compression_codec: CompressionCodec::Lz4Frame,
            bloom_filter_enabled: true,
            bloom_filter_fpr: 0.01,
            bplus_index_enabled: true,
            custom_metadata: HashMap::new(),
        }
    }
}

impl ArrowBlockConfig {
    /// Create configuration for a specific dimension
    pub fn new(dimension: u32) -> Self {
        Self {
            dimension,
            ..Default::default()
        }
    }

    /// Configure for high-performance writes
    pub fn high_throughput(mut self) -> Self {
        self.compression_codec = CompressionCodec::Lz4Frame;
        self.compression_level = 1;
        self.records_per_block = 4000;
        self
    }

    /// Configure for maximum compression
    pub fn max_compression(mut self) -> Self {
        self.compression_codec = CompressionCodec::Zstd;
        self.compression_level = 12;
        self
    }

    /// Configure for read-optimized access
    pub fn read_optimized(mut self) -> Self {
        self.bloom_filter_enabled = true;
        self.bplus_index_enabled = true;
        self.records_per_block = 1000; // Smaller blocks for faster access
        self
    }

    /// Disable compression (for memory-mapped access)
    pub fn uncompressed(mut self) -> Self {
        self.compression_enabled = false;
        self.compression_codec = CompressionCodec::None;
        self
    }

    /// Add custom metadata
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.custom_metadata.insert(key.to_string(), value.to_string());
        self
    }
}

/// Metadata stored in Arrow block footer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrowBlockMetadata {
    /// Format version
    pub version: u32,

    /// Number of blocks in file
    pub num_blocks: u32,

    /// Total number of records
    pub total_records: u64,

    /// Vector dimension
    pub dimension: u32,

    /// File creation timestamp (milliseconds)
    pub created_at_ms: i64,

    /// Compression codec used
    pub compression_codec: CompressionCodec,

    /// ID range across all blocks
    pub id_range: Option<(String, String)>,

    /// Timestamp range across all blocks
    pub timestamp_range: Option<(i64, i64)>,

    /// Custom metadata
    pub custom_metadata: HashMap<String, String>,
}

impl Default for ArrowBlockMetadata {
    fn default() -> Self {
        Self {
            version: super::ARROW_BLOCK_VERSION,
            num_blocks: 0,
            total_records: 0,
            dimension: 0,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            compression_codec: CompressionCodec::None,
            id_range: None,
            timestamp_range: None,
            custom_metadata: HashMap::new(),
        }
    }
}

impl ArrowBlockMetadata {
    /// Create metadata from config
    pub fn from_config(config: &ArrowBlockConfig) -> Self {
        Self {
            version: super::ARROW_BLOCK_VERSION,
            dimension: config.dimension,
            compression_codec: config.compression_codec,
            custom_metadata: config.custom_metadata.clone(),
            ..Default::default()
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap_or_default()
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::deserialize(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = ArrowBlockConfig::default();
        assert_eq!(config.dimension, 768);
        assert!(config.compression_enabled);
        assert!(config.bloom_filter_enabled);
    }

    #[test]
    fn test_config_builders() {
        let config = ArrowBlockConfig::new(1536)
            .high_throughput()
            .with_metadata("collection", "test");

        assert_eq!(config.dimension, 1536);
        assert_eq!(config.records_per_block, 4000);
        assert_eq!(config.custom_metadata.get("collection"), Some(&"test".to_string()));
    }

    #[test]
    fn test_metadata_serialization() {
        let meta = ArrowBlockMetadata {
            version: 1,
            num_blocks: 10,
            total_records: 20000,
            dimension: 768,
            ..Default::default()
        };

        let bytes = meta.to_bytes();
        let recovered = ArrowBlockMetadata::from_bytes(&bytes).unwrap();

        assert_eq!(recovered.version, 1);
        assert_eq!(recovered.num_blocks, 10);
        assert_eq!(recovered.total_records, 20000);
    }
}
