//! HELIX Metadata Serializer for UnifiedCachingFilesystem
//
//! Adapts HELIX's existing metadata serialization to work with
//! the new EngineMetadataSerializer trait for engine-owned serialization.
//! HELIX uses Hilbert curve ordering with PCA for time-series and spatial data.
//
//! **TD-DRY-METADATA**: The shared helpers in
//! `crate::storage::engines::core::metadata_serializer` can be used
//! for serialize/deserialize. This file is kept for now because it
//! defines engine-specific metadata types (`HelixCachedMetadata`),
//! but the `EngineMetadataSerializer` impl delegates to shared helpers.

use anyhow::Result;
use bytes::Bytes;
use std::any::Any;
use std::fmt::Debug;

use crate::storage::persistence::filesystem::metadata_traits::{
    EngineMetadataSerializer, deserialize_typed_metadata, serialize_typed_metadata,
};
use serde::{Deserialize, Serialize};

/// HELIX cached metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelixCachedMetadata {
    /// File size
    pub file_size: u64,
    /// Total number of vectors
    pub vector_count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Hilbert curve configuration
    pub hilbert_config: HilbertConfig,
    /// PCA model information
    pub pca_model: PcaModelMetadata,
    /// Liquid clustering state
    pub liquid_clustering: LiquidClusteringMetadata,
    /// Zone map statistics for pruning
    pub zone_maps: Vec<ZoneMapEntry>,
    /// SSTable metadata for LSM structure
    pub sstable_metadata: SstableMetadata,
    /// Query optimization statistics
    pub query_stats: QueryOptimizationStats,
    /// Creation timestamp
    pub creation_timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilbertConfig {
    /// Bits per dimension for Hilbert curve resolution
    pub bits_per_dimension: usize,
    /// Number of dimensions after PCA reduction
    pub reduced_dimensions: usize,
    /// Hilbert key range (min, max)
    pub key_range: (u64, u64),
    /// Spatial locality preservation score
    pub locality_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcaModelMetadata {
    /// Whether PCA is enabled
    pub enabled: bool,
    /// Original dimensions
    pub original_dimensions: usize,
    /// Reduced dimensions
    pub reduced_dimensions: usize,
    /// Explained variance ratio
    pub explained_variance: f32,
    /// Last retrain timestamp
    pub last_retrain: i64,
    /// Number of vectors used for training
    pub training_vectors: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidClusteringMetadata {
    /// Whether liquid clustering is enabled
    pub enabled: bool,
    /// Current number of clusters
    pub cluster_count: usize,
    /// Cluster adaptation rate
    pub adaptation_rate: f32,
    /// Query pattern categories detected
    pub query_patterns: Vec<String>,
    /// Hot clusters (frequently accessed)
    pub hot_clusters: Vec<u32>,
    /// Cold clusters (rarely accessed)
    pub cold_clusters: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneMapEntry {
    /// Zone ID
    pub zone_id: u32,
    /// Hilbert key range for this zone
    pub hilbert_range: (u64, u64),
    /// Number of vectors in this zone
    pub vector_count: usize,
    /// Minimum values per dimension
    pub min_values: Vec<f32>,
    /// Maximum values per dimension
    pub max_values: Vec<f32>,
    /// Temporal range (for time-series data)
    pub time_range: Option<(i64, i64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableMetadata {
    /// LSM level (0 for flush files)
    pub lsm_level: usize,
    /// Number of SSTables at each level
    pub level_counts: Vec<usize>,
    /// Total size per level in bytes
    pub level_sizes: Vec<u64>,
    /// Proxima block size
    pub block_size: usize,
    /// Bloom filter configuration
    pub bloom_filter_bits: u32,
    /// Compression ratio achieved
    pub compression_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryOptimizationStats {
    /// Cache hit rate
    pub cache_hit_rate: f32,
    /// Average query pruning percentage
    pub avg_pruning_percent: f32,
    /// Query patterns detected
    pub pattern_count: usize,
    /// Prefetch accuracy
    pub prefetch_accuracy: f32,
    /// Average query latency in microseconds
    pub avg_query_latency_us: u64,
}

/// HELIX metadata serializer for UnifiedCachingFilesystem
#[derive(Debug)]
pub struct HelixUnifiedMetadataSerializer;

impl HelixUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HelixUnifiedMetadataSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineMetadataSerializer for HelixUnifiedMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        serialize_typed_metadata::<HelixCachedMetadata>(metadata, "HelixCachedMetadata")
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        deserialize_typed_metadata::<HelixCachedMetadata>(bytes)
    }

    fn engine_type(&self) -> &str {
        "helix"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For HELIX files, extract the metadata header which contains:
        // - Hilbert curve configuration
        // - PCA model parameters
        // - Zone maps for pruning
        // - LSM structure information

        if !file_path.ends_with(".hlx") && !file_path.contains("/helix/") {
            return None;
        }

        // HELIX files have a specific structure:
        // [MAGIC][Header][PCA Model][Zone Maps][Hilbert Index][Data]
        // The header, PCA model, and zone maps are most valuable to cache

        if data.len() < 16 {
            // Minimum header size
            return None;
        }

        // Check for HELIX magic bytes
        if &data[0..4] != b"HLIX" {
            return None;
        }

        // Read header size (bytes 4-8)
        let header_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        // Read metadata section size (bytes 8-16)
        let metadata_size = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]) as usize;

        // Validate sizes
        if header_size == 0
            || header_size > data.len()
            || metadata_size == 0
            || header_size + metadata_size > data.len()
        {
            return None;
        }

        // Extract header and metadata sections
        let cacheable_size = header_size + metadata_size;
        let cacheable_data = &data[0..cacheable_size.min(data.len())];

        Some(Bytes::copy_from_slice(cacheable_data))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for HELIX files
        file_path.ends_with(".hlx")
            || file_path.contains("/helix/")
            || file_path.contains("_helix_")
            || file_path.contains("/hilbert/")
            || file_path.contains("/pca_model/")
            || file_path.contains("/zone_maps/")
            || file_path.contains("/liquid_clusters/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helix_metadata_serialization() {
        let metadata = HelixCachedMetadata {
            file_size: 41943040, // 40MB
            vector_count: 40000,
            dimension: 384,
            hilbert_config: HilbertConfig {
                bits_per_dimension: 16,
                reduced_dimensions: 32,
                key_range: (0, u64::MAX),
                locality_score: 0.88,
            },
            pca_model: PcaModelMetadata {
                enabled: true,
                original_dimensions: 384,
                reduced_dimensions: 32,
                explained_variance: 0.95,
                last_retrain: 1234567890,
                training_vectors: 10000,
            },
            liquid_clustering: LiquidClusteringMetadata {
                enabled: true,
                cluster_count: 64,
                adaptation_rate: 0.1,
                query_patterns: vec!["temporal".to_string(), "spatial".to_string()],
                hot_clusters: vec![0, 1, 2, 3, 4],
                cold_clusters: vec![60, 61, 62, 63],
            },
            zone_maps: vec![ZoneMapEntry {
                zone_id: 0,
                hilbert_range: (0, 1000000),
                vector_count: 1000,
                min_values: vec![-1.0; 32],
                max_values: vec![1.0; 32],
                time_range: Some((1234567000, 1234567890)),
            }],
            sstable_metadata: SstableMetadata {
                lsm_level: 3,
                level_counts: vec![4, 10, 25, 50],
                level_sizes: vec![4194304, 41943040, 419430400, 4194304000],
                block_size: 1024,
                bloom_filter_bits: 10,
                compression_ratio: 0.3,
            },
            query_stats: QueryOptimizationStats {
                cache_hit_rate: 0.85,
                avg_pruning_percent: 0.92,
                pattern_count: 5,
                prefetch_accuracy: 0.78,
                avg_query_latency_us: 1200,
            },
            creation_timestamp: 1234567890,
        };

        let serializer = HelixUnifiedMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<HelixCachedMetadata>().unwrap();

        assert_eq!(restored.file_size, metadata.file_size);
        assert_eq!(restored.vector_count, metadata.vector_count);
        assert_eq!(restored.dimension, metadata.dimension);
        assert_eq!(
            restored.hilbert_config.bits_per_dimension,
            metadata.hilbert_config.bits_per_dimension
        );
        assert_eq!(
            restored.pca_model.explained_variance,
            metadata.pca_model.explained_variance
        );
    }

    #[test]
    fn test_helix_metadata_extraction() {
        let serializer = HelixUnifiedMetadataSerializer::new();

        // Create mock HELIX file data
        let mut data = Vec::new();

        // Magic bytes
        data.extend_from_slice(b"HLIX");

        // Header size (256 bytes)
        data.extend_from_slice(&256u32.to_le_bytes());

        // Metadata section size (512 bytes)
        data.extend_from_slice(&512u64.to_le_bytes());

        // Header content
        data.extend_from_slice(&vec![0xAAu8; 256 - 16]);

        // Metadata content (PCA model, zone maps, etc.)
        data.extend_from_slice(&vec![0xBBu8; 512]);

        // Some data content
        data.extend_from_slice(&vec![0xCCu8; 1024]);

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.hlx");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        // Should include header (256) + metadata (512)
        assert_eq!(extracted_bytes.len(), 768);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = HelixUnifiedMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/helix/vectors.hlx"));
        assert!(serializer.should_cache_metadata("/collections/test_helix_data.bin"));
        assert!(serializer.should_cache_metadata("/hilbert/index.dat"));
        assert!(serializer.should_cache_metadata("/pca_model/model.bin"));
        assert!(serializer.should_cache_metadata("/zone_maps/zones.idx"));
        assert!(serializer.should_cache_metadata("/liquid_clusters/clusters.dat"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}
