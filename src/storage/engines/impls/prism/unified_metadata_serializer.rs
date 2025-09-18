//! PRISM Metadata Serializer for UnifiedCachingFilesystem
//!
//! Adapts PRISM's existing metadata serialization to work with
//! the new EngineMetadataSerializer trait for engine-owned serialization.
//! PRISM uses multi-resolution storage with aggressive memory optimization.

use anyhow::Result;
use bytes::Bytes;
use std::any::Any;
use std::fmt::Debug;
use std::collections::HashMap;

use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;
use serde::{Deserialize, Serialize};

/// PRISM cached metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrismCachedMetadata {
    /// File size
    pub file_size: u64,
    /// Total number of vectors
    pub vector_count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Memory optimization statistics
    pub memory_stats: MemoryOptimizationStats,
    /// Multi-resolution metadata
    pub resolution_levels: Vec<ResolutionLevelMetadata>,
    /// Tree structure information
    pub tree_metadata: TreeMetadata,
    /// Cache configuration
    pub cache_config: CacheConfig,
    /// Compression statistics
    pub compression_ratio: f32,
    /// Creation timestamp
    pub creation_timestamp: i64,
    /// PRISM magic bytes for validation
    pub magic: [u8; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryOptimizationStats {
    /// Current memory usage in bytes
    pub memory_usage_bytes: u64,
    /// Memory saved through optimization
    pub memory_saved_bytes: u64,
    /// Number of items in L0 (hot) cache
    pub l0_cache_items: usize,
    /// Number of items in L1 (warm) cache
    pub l1_cache_items: usize,
    /// Number of items in L2 (cold) cache
    pub l2_cache_items: usize,
    /// Cache hit rate
    pub cache_hit_rate: f32,
    /// Memory pressure level (0.0 to 1.0)
    pub memory_pressure: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionLevelMetadata {
    /// Resolution level (Binary, INT8, PQ4, PQ8, FP16, FP32)
    pub level: String,
    /// Number of vectors at this resolution
    pub vector_count: usize,
    /// Storage size for this level
    pub storage_bytes: u64,
    /// Quality score for this level
    pub quality_score: f32,
    /// Access frequency for this level
    pub access_frequency: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeMetadata {
    /// Tree fanout factor
    pub fanout: usize,
    /// Maximum tree depth
    pub max_depth: usize,
    /// Current tree depth
    pub current_depth: usize,
    /// Number of leaf nodes
    pub leaf_count: usize,
    /// Number of internal nodes
    pub internal_count: usize,
    /// Tree overlap factor
    pub overlap_factor: f32,
    /// Tree balance factor
    pub balance_factor: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Memory cache size in MB
    pub memory_cache_mb: usize,
    /// SSD cache size in GB
    pub ssd_cache_gb: usize,
    /// Cache TTL in seconds
    pub cache_ttl_sec: u64,
    /// Whether local cache is enabled
    pub local_cache_enabled: bool,
    /// Cache eviction policy
    pub eviction_policy: String,
}

/// PRISM metadata serializer for UnifiedCachingFilesystem
#[derive(Debug)]
pub struct PrismUnifiedMetadataSerializer;

impl PrismUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl EngineMetadataSerializer for PrismUnifiedMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        // Try to downcast to PrismCachedMetadata
        if let Some(prism_meta) = metadata.downcast_ref::<PrismCachedMetadata>() {
            let bytes = bincode::serialize(prism_meta)?;
            Ok(Bytes::from(bytes))
        } else {
            anyhow::bail!("Expected PrismCachedMetadata type for PRISM serializer")
        }
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        let prism_meta: PrismCachedMetadata = bincode::deserialize(bytes)?;
        Ok(Box::new(prism_meta))
    }

    fn engine_type(&self) -> &str {
        "prism"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For PRISM files, extract the metadata header which contains:
        // - Memory optimization statistics
        // - Multi-resolution level information
        // - Tree structure metadata
        // - Cache configuration

        if !file_path.ends_with(".prism") && !file_path.contains("/prism/") {
            return None;
        }

        // PRISM files have a specific structure:
        // [MAGIC][Header][Resolution Levels][Tree Index][Data]
        // The header and resolution levels are most valuable to cache

        if data.len() < 32 { // Minimum header size
            return None;
        }

        // Check for PRISM magic bytes
        if &data[0..4] != b"PRSM" {
            return None;
        }

        // Read header size (bytes 4-8)
        let header_size = u32::from_le_bytes([
            data[4], data[5], data[6], data[7],
        ]) as usize;

        // Read resolution metadata offset (bytes 8-16)
        let resolution_offset = u64::from_le_bytes([
            data[8], data[9], data[10], data[11],
            data[12], data[13], data[14], data[15],
        ]) as usize;

        // Read resolution metadata size (bytes 16-24)
        let resolution_size = u64::from_le_bytes([
            data[16], data[17], data[18], data[19],
            data[20], data[21], data[22], data[23],
        ]) as usize;

        // Validate sizes
        if header_size == 0 || header_size > data.len() ||
           resolution_offset + resolution_size > data.len() {
            return None;
        }

        // Extract header and resolution metadata
        let mut cacheable = Vec::with_capacity(header_size + resolution_size);
        cacheable.extend_from_slice(&data[0..header_size]);

        if resolution_size > 0 && resolution_offset < data.len() {
            let end = (resolution_offset + resolution_size).min(data.len());
            cacheable.extend_from_slice(&data[resolution_offset..end]);
        }

        Some(Bytes::from(cacheable))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for PRISM files
        file_path.ends_with(".prism") ||
        file_path.contains("/prism/") ||
        file_path.contains("_prism_") ||
        file_path.contains("/resolution/") ||
        file_path.contains("/tree_index/") ||
        file_path.contains("/memory_cache/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prism_metadata_serialization() {
        let metadata = PrismCachedMetadata {
            file_size: 26214400, // 25MB
            vector_count: 25000,
            dimension: 512,
            memory_stats: MemoryOptimizationStats {
                memory_usage_bytes: 5242880,
                memory_saved_bytes: 20971520,
                l0_cache_items: 100,
                l1_cache_items: 500,
                l2_cache_items: 2000,
                cache_hit_rate: 0.92,
                memory_pressure: 0.3,
            },
            resolution_levels: vec![
                ResolutionLevelMetadata {
                    level: "Binary".to_string(),
                    vector_count: 25000,
                    storage_bytes: 1600000,
                    quality_score: 0.65,
                    access_frequency: 100,
                },
                ResolutionLevelMetadata {
                    level: "PQ8".to_string(),
                    vector_count: 5000,
                    storage_bytes: 2560000,
                    quality_score: 0.85,
                    access_frequency: 500,
                },
                ResolutionLevelMetadata {
                    level: "FP32".to_string(),
                    vector_count: 500,
                    storage_bytes: 1024000,
                    quality_score: 1.0,
                    access_frequency: 1000,
                },
            ],
            tree_metadata: TreeMetadata {
                fanout: 32,
                max_depth: 6,
                current_depth: 4,
                leaf_count: 1024,
                internal_count: 100,
                overlap_factor: 0.2,
                balance_factor: 0.95,
            },
            cache_config: CacheConfig {
                memory_cache_mb: 3072,
                ssd_cache_gb: 100,
                cache_ttl_sec: 3600,
                local_cache_enabled: true,
                eviction_policy: "LRU".to_string(),
            },
            compression_ratio: 0.2,
            creation_timestamp: 1234567890,
            magic: *b"PRSM",
        };

        let serializer = PrismUnifiedMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<PrismCachedMetadata>().unwrap();

        assert_eq!(restored.file_size, metadata.file_size);
        assert_eq!(restored.vector_count, metadata.vector_count);
        assert_eq!(restored.dimension, metadata.dimension);
        assert_eq!(restored.compression_ratio, metadata.compression_ratio);
        assert_eq!(restored.magic, metadata.magic);
    }

    #[test]
    fn test_prism_header_extraction() {
        let serializer = PrismUnifiedMetadataSerializer::new();

        // Create mock PRISM file data
        let mut data = Vec::new();

        // Magic bytes
        data.extend_from_slice(b"PRSM");

        // Header size (32 bytes)
        data.extend_from_slice(&32u32.to_le_bytes());

        // Resolution metadata offset (at byte 256)
        data.extend_from_slice(&256u64.to_le_bytes());

        // Resolution metadata size (128 bytes)
        data.extend_from_slice(&128u64.to_le_bytes());

        // Fill rest of header
        data.extend_from_slice(&vec![0u8; 8]);

        // Fill to resolution metadata start
        data.extend_from_slice(&vec![0xFFu8; 256 - 32]);

        // Resolution metadata
        data.extend_from_slice(&vec![0xABu8; 128]);

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.prism");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        // Should include header (32) + resolution metadata (128)
        assert_eq!(extracted_bytes.len(), 160);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = PrismUnifiedMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/prism/vectors.prism"));
        assert!(serializer.should_cache_metadata("/collections/test_prism_data.bin"));
        assert!(serializer.should_cache_metadata("/resolution/level_0.dat"));
        assert!(serializer.should_cache_metadata("/tree_index/nodes.idx"));
        assert!(serializer.should_cache_metadata("/memory_cache/hot.cache"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}