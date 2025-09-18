//! SST Metadata Serializer for UnifiedCachingFilesystem
//!
//! Adapts SST's existing metadata serialization to work with
//! the new EngineMetadataSerializer trait for engine-owned serialization.

use anyhow::Result;
use bytes::Bytes;
use std::any::Any;
use std::fmt::Debug;
use std::collections::HashMap;

use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;
use serde::{Deserialize, Serialize};

/// SST cached metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstCachedMetadata {
    /// SST file size
    pub file_size: u64,
    /// Total number of entries in the file
    pub entry_count: usize,
    /// Number of blocks in the file
    pub block_count: usize,
    /// Column metadata
    pub column_info: HashMap<String, ColumnMetadata>,
    /// Block index for efficient seeking
    pub block_index: Vec<BlockIndexEntry>,
    /// Bloom filter data for quick lookups
    pub bloom_filter_data: Vec<u8>,
    /// Compression type used
    pub compression_type: String,
    /// Creation timestamp
    pub creation_timestamp: i64,
    /// SST level (0 for L0, 1+ for compacted levels)
    pub sst_level: u8,
    /// Min and max sequence numbers in this file
    pub sequence_range: (u64, u64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMetadata {
    pub name: String,
    pub data_type: String,
    pub null_count: usize,
    pub distinct_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockIndexEntry {
    /// Offset of the block in the file
    pub offset: u64,
    /// Size of the block in bytes
    pub size: u32,
    /// First key in the block
    pub first_key: String,
    /// Last key in the block
    pub last_key: String,
    /// Number of entries in the block
    pub entry_count: u32,
}

/// SST metadata serializer for UnifiedCachingFilesystem
#[derive(Debug)]
pub struct SstUnifiedMetadataSerializer;

impl SstUnifiedMetadataSerializer {
    pub fn new() -> Self {
        Self
    }
}

impl EngineMetadataSerializer for SstUnifiedMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        // Try to downcast to SstCachedMetadata
        if let Some(sst_meta) = metadata.downcast_ref::<SstCachedMetadata>() {
            let bytes = bincode::serialize(sst_meta)?;
            Ok(Bytes::from(bytes))
        } else {
            anyhow::bail!("Expected SstCachedMetadata type for SST serializer")
        }
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        let sst_meta: SstCachedMetadata = bincode::deserialize(bytes)?;
        Ok(Box::new(sst_meta))
    }

    fn engine_type(&self) -> &str {
        "sst"
    }

    fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
        // For SST files, extract the footer/index which contains:
        // - Block index for efficient seeking
        // - Bloom filter data
        // - Compression metadata
        // - File-level statistics

        if !file_path.ends_with(".sst") && !file_path.contains("/sst/") {
            return None;
        }

        // SST files typically have the footer/index at the end
        // Format: [data blocks][index block][footer]
        // Footer contains pointer to index block

        if data.len() < 48 { // Minimum footer size
            return None;
        }

        // Read footer (last 48 bytes)
        let footer_start = data.len() - 48;
        let footer = &data[footer_start..];

        // Parse magic number to verify it's an SST file
        let magic = &footer[40..48];
        if magic != b"sstv0001" && magic != b"sstv0002" { // SST magic numbers
            // Try alternative magic format
            if &data[data.len()-8..] != b"SSTFILE\0" {
                return None;
            }
        }

        // Extract index block size from footer
        let index_size_bytes = &footer[32..40];
        let index_size = u64::from_le_bytes([
            index_size_bytes[0],
            index_size_bytes[1],
            index_size_bytes[2],
            index_size_bytes[3],
            index_size_bytes[4],
            index_size_bytes[5],
            index_size_bytes[6],
            index_size_bytes[7],
        ]) as usize;

        // Validate index size
        if index_size == 0 || index_size > data.len() - 48 {
            return None;
        }

        // Extract index block
        let index_start = data.len() - 48 - index_size;
        let index_data = &data[index_start..data.len()];

        Some(Bytes::copy_from_slice(index_data))
    }

    fn should_cache_metadata(&self, file_path: &str) -> bool {
        // Cache metadata for SST files
        file_path.ends_with(".sst") ||
        file_path.contains("/sst/") ||
        file_path.contains("_sst_") ||
        file_path.contains("/L0/") ||
        file_path.contains("/L1/") ||
        file_path.contains("/L2/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sst_metadata_serialization() {
        let metadata = SstCachedMetadata {
            file_size: 10240000,
            entry_count: 50000,
            block_count: 100,
            column_info: HashMap::new(),
            block_index: vec![
                BlockIndexEntry {
                    offset: 0,
                    size: 4096,
                    first_key: "key_000000".to_string(),
                    last_key: "key_000499".to_string(),
                    entry_count: 500,
                },
            ],
            bloom_filter_data: vec![0xFF; 1024],
            compression_type: "snappy".to_string(),
            creation_timestamp: 1234567890,
            sst_level: 0,
            sequence_range: (1000, 2000),
        };

        let serializer = SstUnifiedMetadataSerializer::new();

        // Test serialization
        let bytes = serializer.serialize(&metadata).unwrap();
        assert!(!bytes.is_empty());

        // Test deserialization
        let deserialized = serializer.deserialize(&bytes).unwrap();
        let restored = deserialized.downcast_ref::<SstCachedMetadata>().unwrap();

        assert_eq!(restored.file_size, metadata.file_size);
        assert_eq!(restored.entry_count, metadata.entry_count);
        assert_eq!(restored.block_count, metadata.block_count);
        assert_eq!(restored.sst_level, metadata.sst_level);
    }

    #[test]
    fn test_footer_extraction() {
        let serializer = SstUnifiedMetadataSerializer::new();

        // Create mock SST file data with footer
        let mut data = Vec::new();
        data.extend_from_slice(&vec![0u8; 1000]); // Some file content

        // Add index block (100 bytes)
        let index_block = b"index_block_content";
        data.extend_from_slice(index_block);

        // Add footer (48 bytes)
        data.extend_from_slice(&[0u8; 32]); // Padding
        data.extend_from_slice(&(index_block.len() as u64 + 48).to_le_bytes()); // Index size
        data.extend_from_slice(b"sstv0001"); // Magic number

        // Test extraction
        let extracted = serializer.extract_cacheable_component(&data, "test.sst");
        assert!(extracted.is_some());

        let extracted_bytes = extracted.unwrap();
        assert!(extracted_bytes.len() > 0);
    }

    #[test]
    fn test_should_cache_metadata() {
        let serializer = SstUnifiedMetadataSerializer::new();

        assert!(serializer.should_cache_metadata("/data/sst/file.sst"));
        assert!(serializer.should_cache_metadata("/collections/test_sst_data.bin"));
        assert!(serializer.should_cache_metadata("/L0/000123.sst"));
        assert!(serializer.should_cache_metadata("/L1/000456.sst"));
        assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
    }
}