//! SWIFT Metadata Serializer for UnifiedCachingFilesystem
//!
//! Adapts SWIFT's existing metadata serialization to work with
//! the new EngineMetadataSerializer trait for engine-owned serialization.
//! SWIFT uses hierarchical blocks with Proxima encoding for instant traversal.

use anyhow::Result;
use bytes::Bytes;
use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;

use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;
use serde::{Deserialize, Serialize};

/// SWIFT cached metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwiftCachedMetadata {
    /// File size
    pub file_size: u64,
    /// Total number of vectors
    pub vector_count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Hierarchical structure information
    pub superblock_count: u32,
    pub datablock_count: u32,
    pub tree_depth: u16,
    /// SuperBlock metadata for navigation
    pub superblock_metadata: Vec<SuperBlockMetadata>,
    /// Tree navigation hints for instant traversal
    pub navigation_hints: NavigationHints,
    /// Proxima encoding configuration
    pub proxima_config: ProximaConfig,
    /// Bloom filter configuration
    pub bloom_config: BloomConfig,
    /// Progressive quantization levels
    pub quantization_levels: Vec<String>,
    /// Creation timestamp
    pub creation_timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperBlockMetadata {
    pub superblock_id: u32,
    pub start_offset: u64,
    pub end_offset: u64,
    pub datablock_count: u32,
    pub record_count: u32,
    pub centroid: Vec<f32>,
    pub quantized_signature: Vec<u8>,
    pub tree_node_count: u32,
    pub leaf_node_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationHints {
    /// Frequently accessed paths for optimization
    pub hot_paths: Vec<TreePath>,
    /// Prefetch recommendations
    pub prefetch_superblocks: Vec<u32>,
    /// Cache priority mapping
    pub cache_priorities: HashMap<u32, u8>,
    /// Access frequency statistics
    pub access_frequencies: HashMap<u32, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreePath {
    pub path_id: String,
    pub superblock_sequence: Vec<u32>,
    pub avg_latency_us: u64,
    pub hit_rate: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProximaConfig {
    pub encoding_scheme: String, // "BitPacked", "DeltaEncoded", etc.
    pub bits_per_value: u8,
    pub block_size: usize,
    pub compression_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomConfig {
    pub filter_size_bytes: u32,
    pub hash_functions: u8,
    pub false_positive_rate: f32,
    pub items_count: u64,
}

/// SWIFT metadata serializer for UnifiedCachingFilesystem
#[derive(Debug)]
pub struct SwiftUnifiedMetadataSerializer;

impl SwiftUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SwiftUnifiedMetadataSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineMetadataSerializer for SwiftUnifiedMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        // Try to downcast to SwiftCachedMetadata
        if let Some(swift_meta) = metadata.downcast_ref::<SwiftCachedMetadata>() {
            let bytes = bincode::serialize(swift_meta)?;
            Ok(Bytes::from(bytes))
        } else {
            anyhow::bail!("Expected SwiftCachedMetadata type for SWIFT serializer")
        }
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        let swift_meta: SwiftCachedMetadata = bincode::deserialize(bytes)?;
        Ok(Box::new(swift_meta))
    }

    fn engine_type(&self) -> &str {
        "swift"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For SWIFT files, extract the hierarchical index which contains:
        // - SuperBlock metadata for navigation
        // - Tree structure information
        // - Proxima encoding metadata
        // - Bloom filter data

        if !file_path.ends_with(".swift") && !file_path.contains("/swift/") {
            return None;
        }

        // SWIFT files have a specific structure:
        // [Header][SuperBlock Index][DataBlocks][Footer]
        // The SuperBlock Index is the most valuable part to cache

        if data.len() < 64 {
            // Minimum header size
            return None;
        }

        // Check for SWIFT magic bytes at start
        if &data[0..8] != b"SWIFT001" && &data[0..8] != b"SWIFT002" {
            return None;
        }

        // Read index offset from header (bytes 8-16)
        let index_offset = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]) as usize;

        // Read index size from header (bytes 16-24)
        let index_size = u64::from_le_bytes([
            data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
        ]) as usize;

        // Validate index location
        if index_offset == 0 || index_size == 0 || index_offset + index_size > data.len() {
            return None;
        }

        // Extract the SuperBlock index
        let index_data = &data[index_offset..index_offset + index_size];

        // For SWIFT, we also want to include the header for navigation hints
        let mut cacheable = Vec::with_capacity(64 + index_size);
        cacheable.extend_from_slice(&data[0..64]); // Header
        cacheable.extend_from_slice(index_data); // Index

        Some(Bytes::from(cacheable))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for SWIFT files
        file_path.ends_with(".swift")
            || file_path.contains("/swift/")
            || file_path.contains("_swift_")
            || file_path.contains("/superblocks/")
            || file_path.contains("/hierarchical/")
            || file_path.contains("/proxima/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swift_metadata_serialization() {
        let metadata = SwiftCachedMetadata {
            file_size: 52428800, // 50MB
            vector_count: 50000,
            dimension: 768,
            superblock_count: 10,
            datablock_count: 100,
            tree_depth: 3,
            superblock_metadata: vec![SuperBlockMetadata {
                superblock_id: 0,
                start_offset: 0,
                end_offset: 5242880,
                datablock_count: 10,
                record_count: 5000,
                centroid: vec![0.0; 768],
                quantized_signature: vec![0xAB; 96], // 768/8 bytes for binary quantization
                tree_node_count: 15,
                leaf_node_count: 8,
            }],
            navigation_hints: NavigationHints {
                hot_paths: vec![TreePath {
                    path_id: "path_001".to_string(),
                    superblock_sequence: vec![0, 3, 7],
                    avg_latency_us: 50,
                    hit_rate: 0.95,
                }],
                prefetch_superblocks: vec![0, 1, 2],
                cache_priorities: HashMap::from([(0, 10), (1, 8), (2, 6)]),
                access_frequencies: HashMap::from([(0, 1000), (1, 800), (2, 600)]),
            },
            proxima_config: ProximaConfig {
                encoding_scheme: "BitPacked".to_string(),
                bits_per_value: 16,
                block_size: 1024,
                compression_ratio: 0.4,
            },
            bloom_config: BloomConfig {
                filter_size_bytes: 65536,
                hash_functions: 3,
                false_positive_rate: 0.01,
                items_count: 50000,
            },
            quantization_levels: vec!["binary".to_string(), "int8".to_string(), "pq8".to_string()],
            creation_timestamp: 1234567890,
        };

        let serializer = SwiftUnifiedMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<SwiftCachedMetadata>().unwrap();

        assert_eq!(restored.file_size, metadata.file_size);
        assert_eq!(restored.vector_count, metadata.vector_count);
        assert_eq!(restored.dimension, metadata.dimension);
        assert_eq!(restored.superblock_count, metadata.superblock_count);
        assert_eq!(restored.tree_depth, metadata.tree_depth);
    }

    #[test]
    fn test_swift_index_extraction() {
        let serializer = SwiftUnifiedMetadataSerializer::new();

        // Create mock SWIFT file data
        let mut data = Vec::new();

        // Header with magic bytes
        data.extend_from_slice(b"SWIFT001");

        // Index offset at position 1024
        data.extend_from_slice(&1024u64.to_le_bytes());

        // Index size of 256 bytes
        data.extend_from_slice(&256u64.to_le_bytes());

        // Fill header to 64 bytes
        data.extend_from_slice(&vec![0u8; 40]);

        // Some data content
        data.extend_from_slice(&vec![0xFFu8; 960]); // Up to index start

        // SuperBlock index
        let index = vec![0xABu8; 256];
        data.extend_from_slice(&index);

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.swift");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        // Should include header (64) + index (256)
        assert_eq!(extracted_bytes.len(), 320);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = SwiftUnifiedMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/swift/vectors.swift"));
        assert!(serializer.should_cache_metadata("/collections/test_swift_data.bin"));
        assert!(serializer.should_cache_metadata("/superblocks/sb_001.dat"));
        assert!(serializer.should_cache_metadata("/hierarchical/tree.swift"));
        assert!(serializer.should_cache_metadata("/proxima/encoded.bin"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}
