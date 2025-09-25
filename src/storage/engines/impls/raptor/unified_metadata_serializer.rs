//! RAPTOR Metadata Serializer for UnifiedCachingFilesystem
//!
//! Adapts RAPTOR's existing metadata serialization to work with
//! the new EngineMetadataSerializer trait for engine-owned serialization.

use anyhow::Result;
use bytes::Bytes;
use std::any::Any;
use std::fmt::Debug;

use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;

use super::common::{CentroidStats, VectorCentroidCompressionMetadata};

/// Cached RAPTOR metadata structure
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RaptorCachedMetadata {
    /// File size in bytes
    pub file_size: u64,
    /// Number of vectors in the file
    pub vector_count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Centroid statistics for boundary detection
    pub centroid_stats: Vec<CentroidStats>,
    /// Row group offsets for selective reading
    pub rowgroup_offsets: Vec<u64>,
    /// Bloom filter data for ID lookups
    pub bloom_filter_data: Vec<u8>,
    /// Compression metadata for quantization
    pub compression_metadata: VectorCentroidCompressionMetadata,
    /// File creation timestamp
    pub creation_timestamp: u64,
    /// P×K matrix coverage percentage (for spillover detection)
    pub pxk_coverage: f32,
    /// Whether file has HNSW index
    pub has_hnsw: bool,
    /// HNSW graph offset if present
    pub hnsw_offset: Option<u64>,
}

/// RAPTOR metadata serializer for UnifiedCachingFilesystem
#[derive(Debug)]
pub struct RaptorUnifiedMetadataSerializer;

impl RaptorUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl EngineMetadataSerializer for RaptorUnifiedMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        // Try to downcast to RaptorCachedMetadata
        if let Some(raptor_meta) = metadata.downcast_ref::<RaptorCachedMetadata>() {
            let bytes = bincode::serialize(raptor_meta)?;
            Ok(Bytes::from(bytes))
        } else {
            anyhow::bail!("Expected RaptorCachedMetadata type for RAPTOR serializer")
        }
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        let raptor_meta: RaptorCachedMetadata = bincode::deserialize(bytes)?;
        Ok(Box::new(raptor_meta))
    }

    fn engine_type(&self) -> &str {
        "raptor"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For RAPTOR files, extract the footer which contains:
        // - Centroid statistics for P×K matrix
        // - Row group metadata
        // - Bloom filter data
        // - Compression metadata

        if !file_path.ends_with(".raptor") && !file_path.contains("/raptor/") {
            return None;
        }

        // RAPTOR files have footer at the end
        // Last 8 bytes contain footer size
        if data.len() < 8 {
            return None;
        }

        let footer_size_bytes = &data[data.len()-8..];
        let footer_size = u64::from_le_bytes([
            footer_size_bytes[0],
            footer_size_bytes[1],
            footer_size_bytes[2],
            footer_size_bytes[3],
            footer_size_bytes[4],
            footer_size_bytes[5],
            footer_size_bytes[6],
            footer_size_bytes[7],
        ]) as usize;

        // Validate footer size
        if footer_size > data.len() - 8 || footer_size == 0 {
            return None;
        }

        // Extract footer data
        let footer_start = data.len() - 8 - footer_size;
        let footer_data = &data[footer_start..data.len()-8];

        Some(Bytes::copy_from_slice(footer_data))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for RAPTOR files
        file_path.ends_with(".raptor") ||
        file_path.contains("/raptor/") ||
        file_path.contains("_raptor_") ||
        file_path.contains("/rowgroups/") ||
        file_path.contains("/centroids/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raptor_metadata_serialization() {
        let metadata = RaptorCachedMetadata {
            file_size: 1024000,
            vector_count: 10000,
            dimension: 768,
            centroid_stats: Vec::new(),
            rowgroup_offsets: vec![0, 51200, 102400],
            bloom_filter_data: vec![0xFF; 1024],
            compression_metadata: Default::default(),
            creation_timestamp: 1234567890,
            pxk_coverage: 0.85,
            has_hnsw: true,
            hnsw_offset: Some(204800),
        };

        let serializer = RaptorUnifiedMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<RaptorCachedMetadata>().unwrap();

        assert_eq!(restored.file_size, metadata.file_size);
        assert_eq!(restored.vector_count, metadata.vector_count);
        assert_eq!(restored.dimension, metadata.dimension);
        assert_eq!(restored.pxk_coverage, metadata.pxk_coverage);
    }

    #[test]
    fn test_footer_extraction() {
        let serializer = RaptorUnifiedMetadataSerializer::new();

        // Create mock RAPTOR file data with footer
        let footer = b"raptor_footer_content";
        let footer_size = footer.len() as u64;

        let mut data = Vec::new();
        data.extend_from_slice(&vec![0u8; 100]); // Some file content
        data.extend_from_slice(footer);
        data.extend_from_slice(&footer_size.to_le_bytes());

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.raptor");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        assert_eq!(&extracted_bytes[..], footer);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = RaptorUnifiedMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/raptor/file.raptor"));
        assert!(serializer.should_cache_metadata("/collections/test_raptor_data.bin"));
        assert!(serializer.should_cache_metadata("/rowgroups/rg_001.dat"));
        assert!(serializer.should_cache_metadata("/centroids/centroid_stats.bin"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}