//! VIPER Engine Metadata Serializer
//!
//! Provides metadata serialization for VIPER's Parquet-based storage format.
//! This serializer handles:
//! - Parquet file metadata and footer caching
//! - Row group information
//! - Cluster metadata for vector search
//! - Column statistics for query optimization

use anyhow::Result;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;

use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;

/// VIPER-specific metadata for caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViperCachedMetadata {
    /// File path
    pub file_path: String,

    /// Total number of rows
    pub total_rows: usize,

    /// Number of row groups
    pub row_group_count: usize,

    /// Row group metadata
    pub row_groups: Vec<RowGroupMetadata>,

    /// Column statistics
    pub column_stats: HashMap<String, ColumnStats>,

    /// Cluster metadata for vector search optimization
    pub cluster_metadata: Option<Vec<ClusterInfo>>,

    /// Cached Parquet footer (avoids repeated reads)
    pub parquet_footer: Option<Vec<u8>>,

    /// File size in bytes
    pub file_size: u64,

    /// Last modified timestamp
    pub last_modified: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroupMetadata {
    pub id: u32,
    pub row_count: usize,
    pub file_offset: u64,
    pub total_byte_size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStats {
    pub null_count: usize,
    pub distinct_count: Option<usize>,
    pub min_value: Option<serde_json::Value>,
    pub max_value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub cluster_id: u32,
    pub centroid: Vec<f32>,
    pub vector_count: usize,
    pub radius: f32,
}

/// VIPER metadata serializer implementation
#[derive(Debug)]
pub struct ViperMetadataSerializer;

impl ViperMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl EngineMetadataSerializer for ViperMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        // Try to downcast to ViperCachedMetadata
        if let Some(viper_meta) = metadata.downcast_ref::<ViperCachedMetadata>() {
            let bytes = bincode::serialize(viper_meta)?;
            Ok(Bytes::from(bytes))
        } else {
            anyhow::bail!("Expected ViperCachedMetadata type for VIPER serializer")
        }
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        let viper_meta: ViperCachedMetadata = bincode::deserialize(bytes)?;
        Ok(Box::new(viper_meta))
    }

    fn engine_type(&self) -> &str {
        "viper"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For VIPER, extract Parquet footer from .parquet files
        if !file_path.ends_with(".parquet") || data.len() < 12 {
            return None;
        }

        // Parquet files have "PAR1" magic bytes at start and end
        if &data[0..4] != b"PAR1" || &data[data.len()-4..] != b"PAR1" {
            return None;
        }

        // Read footer size from the last 4 bytes before the trailing PAR1
        let footer_size_bytes = &data[data.len()-8..data.len()-4];
        let footer_size = u32::from_le_bytes([
            footer_size_bytes[0],
            footer_size_bytes[1],
            footer_size_bytes[2],
            footer_size_bytes[3],
        ]) as usize;

        // Validate footer size
        if footer_size > data.len() - 12 || footer_size == 0 {
            return None;
        }

        // Extract footer data
        let footer_start = data.len() - 8 - footer_size;
        let footer_data = &data[footer_start..data.len()-8];

        Some(Bytes::copy_from_slice(footer_data))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for Parquet files and VIPER-specific files
        file_path.ends_with(".parquet") ||
        file_path.contains("/viper/") ||
        file_path.contains("cluster_metadata")
    }
}

/// Helper to create metadata from Parquet file
impl ViperCachedMetadata {
    pub fn from_parquet_footer(
        file_path: String,
        footer_bytes: &[u8],
        file_size: u64,
    ) -> Result<Self> {
        // In a real implementation, we would parse the Parquet footer
        // using arrow-rs or similar library to extract actual metadata
        // For now, create a placeholder
        Ok(Self {
            file_path,
            total_rows: 0, // Would be extracted from footer
            row_group_count: 0, // Would be extracted from footer
            row_groups: Vec::new(),
            column_stats: HashMap::new(),
            cluster_metadata: None,
            parquet_footer: Some(footer_bytes.to_vec()),
            file_size,
            last_modified: chrono::Utc::now().timestamp(),
        })
    }

    /// Check if metadata is still valid
    pub fn is_valid(&self, file_size: u64, last_modified: i64) -> bool {
        self.file_size == file_size && self.last_modified == last_modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viper_metadata_serialization() {
        let metadata = ViperCachedMetadata {
            file_path: "/data/viper/collection1.parquet".to_string(),
            total_rows: 1000000,
            row_group_count: 10,
            row_groups: vec![
                RowGroupMetadata {
                    id: 0,
                    row_count: 100000,
                    file_offset: 0,
                    total_byte_size: 1024000,
                    compressed_size: 512000,
                }
            ],
            column_stats: HashMap::new(),
            cluster_metadata: Some(vec![
                ClusterInfo {
                    cluster_id: 0,
                    centroid: vec![0.1, 0.2, 0.3],
                    vector_count: 1000,
                    radius: 0.5,
                }
            ]),
            parquet_footer: Some(vec![1, 2, 3, 4]),
            file_size: 10485760,
            last_modified: 1234567890,
        };

        let serializer = ViperMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<ViperCachedMetadata>().unwrap();

        assert_eq!(restored.file_path, metadata.file_path);
        assert_eq!(restored.total_rows, metadata.total_rows);
        assert_eq!(restored.row_group_count, metadata.row_group_count);
    }

    #[test]
    fn test_parquet_footer_extraction() {
        let serializer = ViperMetadataSerializer::new();

        // Create mock Parquet file data
        let mut data = Vec::new();
        data.extend_from_slice(b"PAR1"); // Magic bytes at start
        data.extend_from_slice(&vec![0u8; 100]); // Some data
        let footer = b"footer_content";
        data.extend_from_slice(footer);
        data.extend_from_slice(&(footer.len() as u32).to_le_bytes()); // Footer size
        data.extend_from_slice(b"PAR1"); // Magic bytes at end

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.parquet");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        assert_eq!(&extracted_bytes[..], footer);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = ViperMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/viper/file.parquet"));
        assert!(serializer.should_cache_metadata("/collections/viper/data.bin"));
        assert!(serializer.should_cache_metadata("cluster_metadata.json"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}