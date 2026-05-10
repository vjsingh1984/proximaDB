//! NOVA Metadata Serializer for UnifiedCachingFilesystem
//
//! Adapts NOVA's existing metadata serialization to work with
//! the new EngineMetadataSerializer trait for engine-owned serialization.
//! NOVA uses Parquet format with hierarchical statistics for advanced analytics.
//
//! **TD-DRY-METADATA**: Shared helpers in `core::metadata_serializer` available.

use anyhow::Result;
use bytes::Bytes;
use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;

use crate::storage::persistence::filesystem::metadata_traits::{
    deserialize_typed_metadata, serialize_typed_metadata, EngineMetadataSerializer,
};
use crate::storage::engines::core::metadata_serializer::{extract_footer, path_matches_engine};
use serde::{Deserialize, Serialize};

/// NOVA cached metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovaCachedMetadata {
    /// File size
    pub file_size: u64,
    /// Total number of vectors
    pub vector_count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Number of super blocks
    pub super_block_count: usize,
    /// Number of row groups
    pub row_group_count: usize,
    /// Hierarchical statistics for query optimization
    pub hierarchical_stats: HierarchicalStatsCache,
    /// Zone maps for efficient pruning
    pub zone_maps: Vec<ZoneMapEntry>,
    /// Column metadata for filterable columns
    pub column_metadata: HashMap<String, ColumnMetadata>,
    /// Compression ratio achieved
    pub compression_ratio: f32,
    /// Quantization configuration
    pub quantization_config: Option<QuantizationMetadata>,
    /// Creation timestamp
    pub creation_timestamp: i64,
    /// Parquet schema hash for validation
    pub schema_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalStatsCache {
    /// Super block level statistics
    pub super_block_stats: Vec<SuperBlockStat>,
    /// Global statistics across all data
    pub global_min_values: Vec<f32>,
    pub global_max_values: Vec<f32>,
    pub global_centroid: Vec<f32>,
    /// Estimated pruning efficiency (0.0 to 1.0)
    pub pruning_efficiency: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperBlockStat {
    pub super_block_id: usize,
    pub start_row_group: usize,
    pub end_row_group: usize,
    pub vector_count: usize,
    pub min_similarity: f32,
    pub max_similarity: f32,
    pub centroid: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneMapEntry {
    pub row_group_id: usize,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
    pub null_count: usize,
    pub distinct_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMetadata {
    pub name: String,
    pub data_type: String,
    pub encoding: String,
    pub compression: String,
    pub total_compressed_size: u64,
    pub total_uncompressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationMetadata {
    pub algorithm: String, // "binary", "int8", "pq4", "pq8", etc.
    pub codebook_size: usize,
    pub subvector_count: Option<usize>,
    pub bits_per_subvector: Option<u8>,
}

/// NOVA metadata serializer for UnifiedCachingFilesystem
#[derive(Debug)]
pub struct NovaUnifiedMetadataSerializer;

impl NovaUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NovaUnifiedMetadataSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineMetadataSerializer for NovaUnifiedMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        serialize_typed_metadata::<NovaCachedMetadata>(metadata, "NovaCachedMetadata")
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        deserialize_typed_metadata::<NovaCachedMetadata>(bytes)
    }

    fn engine_type(&self) -> &str {
        "nova"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For NOVA files (Parquet format), extract the footer which contains:
        // - Hierarchical statistics (super blocks, row groups)
        // - Zone maps for pruning
        // - Column metadata
        // - Parquet schema

        if !file_path.ends_with(".parquet") && !file_path.contains("/nova/") {
            return None;
        }

        // Parquet files have a specific structure:
        // [PAR1][data][metadata][footer_length(4 bytes)][PAR1]

        if data.len() < 12 {
            return None;
        }

        // Check for PAR1 magic bytes at start and end
        if &data[0..4] != b"PAR1" || &data[data.len() - 4..] != b"PAR1" {
            return None;
        }

        // Read footer length (4 bytes before the trailing PAR1)
        let footer_len_bytes = &data[data.len() - 8..data.len() - 4];
        let footer_len = u32::from_le_bytes([
            footer_len_bytes[0],
            footer_len_bytes[1],
            footer_len_bytes[2],
            footer_len_bytes[3],
        ]) as usize;

        // Validate footer length
        if footer_len == 0 || footer_len > data.len() - 12 {
            return None;
        }

        // Extract footer including metadata
        let footer_start = data.len() - 8 - footer_len;
        let footer_data = &data[footer_start..data.len()];

        // For NOVA, we want to cache not just the Parquet footer but also
        // any NOVA-specific metadata that might be stored in custom metadata fields
        Some(Bytes::copy_from_slice(footer_data))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for NOVA Parquet files
        file_path.ends_with(".parquet")
            || file_path.contains("/nova/")
            || file_path.contains("_nova_")
            || file_path.contains("/superblocks/")
            || file_path.contains("/progressive/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nova_metadata_serialization() {
        let metadata = NovaCachedMetadata {
            file_size: 104857600, // 100MB
            vector_count: 100000,
            dimension: 1536,
            super_block_count: 10,
            row_group_count: 100,
            hierarchical_stats: HierarchicalStatsCache {
                super_block_stats: vec![SuperBlockStat {
                    super_block_id: 0,
                    start_row_group: 0,
                    end_row_group: 10,
                    vector_count: 10000,
                    min_similarity: 0.1,
                    max_similarity: 0.99,
                    centroid: vec![0.5; 1536],
                }],
                global_min_values: vec![-1.0; 1536],
                global_max_values: vec![1.0; 1536],
                global_centroid: vec![0.0; 1536],
                pruning_efficiency: 0.85,
            },
            zone_maps: vec![ZoneMapEntry {
                row_group_id: 0,
                min_values: vec![-0.5; 16], // Abbreviated for test
                max_values: vec![0.5; 16],
                null_count: 0,
                distinct_count: 1000,
            }],
            column_metadata: HashMap::new(),
            compression_ratio: 0.25,
            quantization_config: Some(QuantizationMetadata {
                algorithm: "pq8".to_string(),
                codebook_size: 256,
                subvector_count: Some(192),
                bits_per_subvector: Some(8),
            }),
            creation_timestamp: 1234567890,
            schema_hash: 0xDEADBEEF,
        };

        let serializer = NovaUnifiedMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<NovaCachedMetadata>().unwrap();

        assert_eq!(restored.file_size, metadata.file_size);
        assert_eq!(restored.vector_count, metadata.vector_count);
        assert_eq!(restored.dimension, metadata.dimension);
        assert_eq!(restored.super_block_count, metadata.super_block_count);
        assert_eq!(restored.compression_ratio, metadata.compression_ratio);
    }

    #[test]
    fn test_parquet_footer_extraction() {
        let serializer = NovaUnifiedMetadataSerializer::new();

        // Create mock Parquet file data
        let mut data = Vec::new();

        // PAR1 magic at start
        data.extend_from_slice(b"PAR1");

        // Some data content
        data.extend_from_slice(&vec![0u8; 1000]);

        // Footer content
        let footer = b"parquet_footer_metadata_content";
        let _footer_start = data.len();
        data.extend_from_slice(footer);

        // Footer length (4 bytes)
        data.extend_from_slice(&(footer.len() as u32).to_le_bytes());

        // PAR1 magic at end
        data.extend_from_slice(b"PAR1");

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.parquet");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        // Should include footer + length + trailing PAR1
        assert_eq!(extracted_bytes.len(), footer.len() + 8);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = NovaUnifiedMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/nova/vectors.parquet"));
        assert!(serializer.should_cache_metadata("/collections/test_nova_data.parquet"));
        assert!(serializer.should_cache_metadata("/superblocks/sb_001.parquet"));
        assert!(serializer.should_cache_metadata("/progressive/level_0.parquet"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}
