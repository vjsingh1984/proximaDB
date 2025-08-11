//! SST Storage Engine
//!
//! Sorted String Table (SST) storage engine implementation providing an alternative
//! to VIPER for performance comparison and standard SSTable storage.

pub mod bloom_filter;
pub mod compaction;
pub mod decompression_cache;
pub mod mmap;
pub mod readers;
pub mod sstable_writer;
pub mod unified_search_engine;
pub mod index_based_reader;
pub mod optimized_row_filter;
pub mod three_stage_filter;

// Test modules
#[cfg(test)]
pub mod bloom_filter_tests;
#[cfg(test)]
pub mod compaction_coverage_tests;

// Re-export main types
pub use bloom_filter::{
    BloomFilterStrategy, BloomFilterConfig, BloomFilterFactory,
    SstableBloomFilter, BloomStrategy, CompositeBloomFilter,
};
pub use compaction::{CompactionManager, CompactionPriority, CompactionStats, CompactionTask};
pub use readers::UnifiedSstableReader;

// Additional exports for unified reader (SstableHeader is already defined below)
pub use sstable_writer::SstableWriter;

// Main SST Storage implementation (contents from original lsm/mod.rs)
use crate::core::{SstConfig, VectorRecord};
use crate::core::search::{SearchResult, json_value_serde};
use crate::core::serialization::{VectorSerializationConfig, VectorAnalysis, CompressionAlgorithm};
use crate::storage::optimization::{SortingStats};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::proto::proximadb::Collection;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    UnifiedStorageEngine,
};
use crate::storage::transaction_coordinator::TransactionCoordinator;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::{UnifiedQuantizationEngine, CodebookStore, InMemoryCodebookStore};
use crate::core::search::UnifiedSearchEngine;
use crate::proto::proximadb::MetadataItem;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use std::sync::Arc;

/// Common SST filename generation utilities
pub struct SstFilenameGenerator;

impl SstFilenameGenerator {
    /// Generate consistent SST filename with pattern: level{level}_{timestamp}_{random}.sst
    /// Collection ID is NOT part of filename - it's in the directory path
    pub fn generate_filename(level: u8) -> String {
        let timestamp = Utc::now().timestamp_millis();
        let random_suffix = rand::thread_rng().gen::<u32>();
        format!("level{}_{}_{}.sst", level, timestamp, random_suffix)
    }
    
    /// Generate SST filename for compaction output
    pub fn generate_compaction_filename(level: u8) -> String {
        Self::generate_filename(level)
    }
    
    /// Generate SST filename for flush operations
    pub fn generate_flush_filename() -> String {
        Self::generate_filename(0) // Flush always creates level 0 files
    }
    
    /// Parse level from SST filename (format: level{N}_{timestamp}_{random}.sst)
    pub fn parse_level_from_filename(filename: &str) -> Option<u8> {
        if filename.starts_with("level") {
            let level_str = &filename[5..];
            if let Some(level_end) = level_str.find('_') {
                return level_str[..level_end].parse::<u8>().ok();
            }
        }
        None
    }
    
    /// Check if filename matches SST pattern
    pub fn is_sst_file(filename: &str) -> bool {
        // Must end with .sst
        if !filename.ends_with(".sst") {
            return false;
        }
        
        // Check if filename contains "level" followed by a digit
        // This supports both "levelN_" and "collection_levelN_" patterns
        if let Some(level_pos) = filename.find("level") {
            let after_level = &filename[level_pos + 5..];
            // Must have at least one digit after level
            if let Some(first_char) = after_level.chars().next() {
                return first_char.is_ascii_digit();
            }
        }
        
        false
    }
    
    // 🔴 UNUSED METHOD - CANDIDATE FOR REMOVAL
    // This method assumes collection_id is part of filename, but our new design
    /// Check if filename belongs to a specific collection
    /// Note: In current design, collection_id is in directory path, not filename
    /// This is kept for backward compatibility with tests
    pub fn belongs_to_collection(filename: &str, collection_id: &str) -> bool {
        filename.contains(collection_id) && Self::is_sst_file(filename)
    }
}

#[cfg(test)]
mod sst_filename_tests {
    use super::*;

    #[test]
    fn test_generate_filename() {
        let collection_id = "test_collection";
        let level = 2;
        
        let filename = SstFilenameGenerator::generate_filename(level);
        
        // Check basic pattern
        assert!(filename.starts_with("level2_"));
        assert!(filename.ends_with(".sst"));
        assert!(filename.contains("level2_"));
        
        // Check that it's recognized as an SST file
        assert!(SstFilenameGenerator::is_sst_file(&filename));
    }

    #[test]
    fn test_generate_flush_filename() {
        let filename = SstFilenameGenerator::generate_flush_filename();
        
        // Flush files should always be level 0
        assert!(filename.starts_with("level0_"));
        assert!(filename.ends_with(".sst"));
        assert_eq!(SstFilenameGenerator::parse_level_from_filename(&filename), Some(0));
    }

    #[test]
    fn test_generate_compaction_filename() {
        let level = 5;
        
        let filename = SstFilenameGenerator::generate_compaction_filename(level);
        
        assert!(filename.starts_with("level5_"));
        assert!(filename.ends_with(".sst"));
        assert_eq!(SstFilenameGenerator::parse_level_from_filename(&filename), Some(5));
    }

    #[test]
    fn test_parse_level_from_filename() {
        let test_cases = vec![
            ("level0_123456_789.sst", Some(0)),
            ("level3_987654_321.sst", Some(3)),
            ("level15_111222_333.sst", Some(15)),
            ("invalid_file.sst", None),
            ("no_level_file.txt", None),
            ("levelABC_123_456.sst", None), // Invalid level number
        ];

        for (filename, expected) in test_cases {
            assert_eq!(
                SstFilenameGenerator::parse_level_from_filename(filename),
                expected,
                "Failed for filename: {}",
                filename
            );
        }
    }

    #[test]
    fn test_is_sst_file() {
        let test_cases = vec![
            ("collection_level0_123_456.sst", true),
            ("test_level5_789_012.sst", true),
            ("invalid.txt", false),
            ("no_level.sst", false),
            ("collection_level3_123_456.parquet", false),
            ("level0_file.sst", true), // Should work even without collection prefix if has level
        ];

        for (filename, expected) in test_cases {
            let result = SstFilenameGenerator::is_sst_file(filename);
            debug!("Testing '{}': expected={}, got={}", filename, expected, result);
            assert_eq!(
                result,
                expected,
                "Failed for filename: {}",
                filename
            );
        }
    }

    // test_belongs_to_collection removed as current implementation doesn't include collection IDs in filenames

    #[test]
    fn test_filename_uniqueness() {
        let collection_id = "test";
        let level = 1;
        
        // Generate multiple filenames and ensure they're unique
        let mut filenames = std::collections::HashSet::new();
        for _ in 0..100 {
            let filename = SstFilenameGenerator::generate_filename(level);
            assert!(filenames.insert(filename), "Generated duplicate filename");
        }
    }

    #[test]
    fn test_filename_consistency() {
        let level = 3;
        
        // Test that the generated filename can be properly parsed back
        let filename = SstFilenameGenerator::generate_filename(level);
        
        assert!(SstFilenameGenerator::is_sst_file(&filename));
        assert_eq!(SstFilenameGenerator::parse_level_from_filename(&filename), Some(level));
        // Collection ID validation removed - it's determined from base URL at search time
    }
}

// Remove dummy filesystem factory - SST will use fallback methods

/// SST-specific record format for efficient SSTable storage
/// This stores VectorRecord fields directly without wrapper overhead
// No longer need json_value_serde module - using MetadataItem directly with bincode

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstRecord {
    // Core VectorRecord fields stored directly
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: Vec<MetadataItem>,
    pub timestamp: u32,  // Record timestamp - seconds since epoch (compact, unsigned)
    pub updated_at: Option<u32>,  // Only set if different from timestamp (saves bytes when not updated)
    pub expires_at: Option<u32>,  // TTL support (seconds since epoch, unsigned)
    pub version: Option<u32>, // Use Option<u32> to match proto VectorRecord
    
    // SST-specific fields
    pub is_tombstone: bool,        // True if this is a deletion marker
    pub sequence_number: u64,      // SST sequence for ordering
    pub level: u8,                 // SSTable level this record belongs to
}

/// Metadata and non-vector fields for separate serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SstRecordMetadata {
    pub id: String,
    pub metadata: Vec<MetadataItem>,
    pub timestamp: u32,
    pub updated_at: Option<u32>,
    pub expires_at: Option<u32>,
    pub version: Option<u32>,
    pub is_tombstone: bool,
    pub sequence_number: u64,
    pub level: u8,
}

impl SstRecord {
    /// Create SstRecord from VectorRecord (collection_id no longer needed - SST files are already in collection directories)
    pub fn from_vector_record(record: VectorRecord) -> Self {
        // Use MetadataItem directly - no JSON conversion needed!
        Self {
            id: record.id.as_deref().unwrap_or("").to_string(),
            vector: record.vector,
            metadata: record.metadata,
            timestamp: record.timestamp,
            updated_at: if record.updated_at == Some(record.timestamp) { None } else { record.updated_at },
            expires_at: record.expires_at,
            version: record.version,
            is_tombstone: false,
            sequence_number: 0, // Will be set during flush
            level: 0,           // Will be set during flush
        }
    }

    /// Serialize using optimized vector serialization with bytemuck and ZSTD
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        self.serialize_with_config(&VectorSerializationConfig::default())
    }
    
    /// Serialize with specific configuration for optimal performance
    pub fn serialize_with_config(&self, config: &VectorSerializationConfig) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        
        // Analyze vector for adaptive optimization
        let analysis = config.analyze_vector(&self.vector);
        let mut optimized_config = config.clone();
        optimized_config.optimize_for_analysis(&analysis);
        
        // Pre-allocate buffer with estimated size
        let estimated_size = self.estimate_serialized_size(&analysis);
        let mut buffer = Vec::with_capacity(estimated_size);
        
        // Write format version for backward compatibility
        buffer.write_all(&[0x02u8])?; // Version 2: optimized format
        
        // Serialize vector using bytemuck + ZSTD
        let vector_data = optimized_config.serialize_vector(&self.vector)
            .context("Failed to serialize vector with optimized format")?;
        buffer.write_all(&(vector_data.len() as u32).to_le_bytes())?;
        buffer.write_all(&vector_data)?;
        
        // Serialize metadata and other fields with bincode (smaller structured data)
        let metadata_and_fields = SstRecordMetadata {
            id: self.id.clone(),
            metadata: self.metadata.clone(),
            timestamp: self.timestamp,
            updated_at: self.updated_at,
            expires_at: self.expires_at,
            version: self.version,
            is_tombstone: self.is_tombstone,
            sequence_number: self.sequence_number,
            level: self.level,
        };
        
        let metadata_data = bincode::serialize(&metadata_and_fields)
            .context("Failed to serialize metadata and fields")?;
        buffer.write_all(&(metadata_data.len() as u32).to_le_bytes())?;
        buffer.write_all(&metadata_data)?;
        
        Ok(buffer)
    }
    
    /// Estimate serialized size for buffer pre-allocation
    fn estimate_serialized_size(&self, analysis: &VectorAnalysis) -> usize {
        let vector_size = if analysis.sparsity > 0.5 {
            // Sparse vectors compress well
            (self.vector.len() * 4) / 3  // ~33% compression estimate
        } else {
            self.vector.len() * 4  // Raw size for dense vectors
        };
        
        let metadata_size = self.metadata.len() * 64; // Conservative estimate
        let overhead = 64; // Headers, lengths, etc.
        
        vector_size + metadata_size + overhead
    }
    
    /// Deserialize with automatic format detection
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data for SstRecord deserialization"));
        }
        
        match data[0] {
            0x02 => Self::deserialize_optimized(&data[1..]),
            _ => {
                // Bincode format for backward compatibility  
                bincode::deserialize(data)
                    .map_err(|e| anyhow::anyhow!("Failed to deserialize SstRecord: {}", e))
            }
        }
    }
    
    /// Deserialize optimized format with bytemuck vectors
    fn deserialize_optimized(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read vector data length
        let mut len_bytes = [0u8; 4];
        cursor.read_exact(&mut len_bytes)?;
        let vector_len = u32::from_le_bytes(len_bytes) as usize;
        
        // Read and deserialize vector
        let mut vector_data = vec![0u8; vector_len];
        cursor.read_exact(&mut vector_data)?;
        
        let config = VectorSerializationConfig::default();
        let vector = config.deserialize_vector(&vector_data)
            .context("Failed to deserialize optimized vector")?;
        
        // Read metadata length
        let mut len_bytes = [0u8; 4];
        cursor.read_exact(&mut len_bytes)?;
        let metadata_len = u32::from_le_bytes(len_bytes) as usize;
        
        // Read and deserialize metadata and fields
        let mut metadata_data = vec![0u8; metadata_len];
        cursor.read_exact(&mut metadata_data)?;
        
        let metadata_fields: SstRecordMetadata = bincode::deserialize(&metadata_data)
            .context("Failed to deserialize metadata and fields")?;
        
        Ok(SstRecord {
            id: metadata_fields.id,
            vector,
            metadata: metadata_fields.metadata,
            timestamp: metadata_fields.timestamp,
            updated_at: metadata_fields.updated_at,
            expires_at: metadata_fields.expires_at,
            version: metadata_fields.version,
            is_tombstone: metadata_fields.is_tombstone,
            sequence_number: metadata_fields.sequence_number,
            level: metadata_fields.level,
        })
    }
    
}

impl Into<VectorRecord> for SstRecord {
    fn into(self) -> VectorRecord {
        VectorRecord {
            id: Some(self.id),  // Core VectorRecord expects Option<String>
            vector: self.vector,
            metadata: self.metadata,  // Already Vec<MetadataItem>
            timestamp: self.timestamp,
            updated_at: self.updated_at,
            expires_at: self.expires_at,
            version: self.version,
            rank: None,
            score: None,
            distance: None,
        
        }
    }
}

/// SSTable header for row-based storage format with engine optimizations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableHeader {
    pub version: u32,
    pub level: u8,
    pub entry_count: u64,
    pub min_key: String,
    pub max_key: String,
    pub created_at: i64,
    // Compression configuration
    #[serde(default)]
    pub compression_algorithm: CompressionAlgorithmSst,  // New: specific algorithm
    #[serde(default)]
    pub compression_level: u8,  // New: compression level (e.g., ZSTD 1-22)
    #[serde(default)]
    pub has_bloom_filter: bool,
    #[serde(default = "default_block_size")]
    pub block_size: u32,
    #[serde(default)]
    pub batch_size: u32,
    // Additional fields for SSTable reader
    #[serde(default)]
    pub header_size: u32,
    #[serde(default)]
    pub index_size: u32,
    #[serde(default)]
    pub data_size: u32,
    #[serde(default)]
    pub block_count: u32,
}

/// Compression algorithm for SST storage (separate from proto to avoid dependency)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompressionAlgorithmSst {
    None = 0,
    Zstd = 1,    // ✅ USED - Primary compression algorithm
    Lz4 = 2,     // ✅ USED - Fast compression
    Snappy = 3,  // ✅ USED - Google's compression
    
    // 🔴 UNUSED VARIANTS - CANDIDATES FOR REMOVAL
    // These compression algorithms are implemented but have zero actual usage.
    // Only None, Zstd, Lz4, and Snappy are used based on proto CompressionAlgorithm mapping.
    Gzip = 4,      // UNUSED - No callers found
    Brotli = 5,    // UNUSED - No callers found  
    Bzip2 = 6,     // UNUSED - No callers found
    Deflate = 7,   // UNUSED - No callers found
    Xz = 8,        // UNUSED - No callers found
    Zlib = 9,      // UNUSED - No callers found
    Lzo = 10,      // UNUSED - No callers found (not available in Rust)
    Lz4Hc = 11,    // UNUSED - No callers found
    Lzma = 12,     // UNUSED - No callers found
}

impl Default for CompressionAlgorithmSst {
    fn default() -> Self {
        CompressionAlgorithmSst::None
    }
}

/// Index entry for fast key lookups in SSTable with block organization and metadata statistics
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub key: String,
    pub offset: u64,
    pub size: u32,
    pub block_id: u32,
    pub block_offset: u32,
    pub compressed: bool,
    /// Minimum values for each metadata column in this block
    pub metadata_min_values: HashMap<String, serde_json::Value>,
    /// Maximum values for each metadata column in this block
    pub metadata_max_values: HashMap<String, serde_json::Value>,
    /// Count of null values for each metadata column in this block
    pub metadata_null_counts: HashMap<String, u32>,
}

impl IndexEntry {
    /// Custom serialization to avoid serde_json::Value bincode issues
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();
        
        // Write magic header
        buffer.write_all(b"IDX1")?;
        
        // Write key
        let key_bytes = self.key.as_bytes();
        buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(key_bytes)?;
        
        // Write primitive fields
        buffer.write_all(&self.offset.to_le_bytes())?;
        buffer.write_all(&self.size.to_le_bytes())?;
        buffer.write_all(&self.block_id.to_le_bytes())?;
        buffer.write_all(&self.block_offset.to_le_bytes())?;
        buffer.write_all(&[if self.compressed { 1u8 } else { 0u8 }])?;
        
        // Write metadata_min_values
        buffer.write_all(&(self.metadata_min_values.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_min_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            json_value_serde::serialize_json_value(value, &mut buffer)?;
        }
        
        // Write metadata_max_values
        buffer.write_all(&(self.metadata_max_values.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_max_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            json_value_serde::serialize_json_value(value, &mut buffer)?;
        }
        
        // Write metadata_null_counts
        buffer.write_all(&(self.metadata_null_counts.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_null_counts {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            buffer.write_all(&value.to_le_bytes())?;
        }
        
        Ok(buffer)
    }

    /// Custom deserialization to avoid serde_json::Value bincode issues
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read and validate magic header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != b"IDX1" {
            return Err(anyhow::anyhow!("Invalid IndexEntry format"));
        }
        
        // Read key
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let key_len = u32::from_le_bytes(len_buf) as usize;
        let mut key_bytes = vec![0u8; key_len];
        cursor.read_exact(&mut key_bytes)?;
        let key = String::from_utf8(key_bytes)?;
        
        // Read primitive fields
        let mut u64_buf = [0u8; 8];
        cursor.read_exact(&mut u64_buf)?;
        let offset = u64::from_le_bytes(u64_buf);
        
        let mut u32_buf = [0u8; 4];
        cursor.read_exact(&mut u32_buf)?;
        let size = u32::from_le_bytes(u32_buf);
        
        cursor.read_exact(&mut u32_buf)?;
        let block_id = u32::from_le_bytes(u32_buf);
        
        cursor.read_exact(&mut u32_buf)?;
        let block_offset = u32::from_le_bytes(u32_buf);
        
        let mut bool_buf = [0u8; 1];
        cursor.read_exact(&mut bool_buf)?;
        let compressed = bool_buf[0] != 0;
        
        // Read metadata_min_values
        cursor.read_exact(&mut len_buf)?;
        let min_values_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_min_values = HashMap::new();
        for _ in 0..min_values_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value = json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata_min_values.insert(key, value);
        }
        
        // Read metadata_max_values
        cursor.read_exact(&mut len_buf)?;
        let max_values_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_max_values = HashMap::new();
        for _ in 0..max_values_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value = json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata_max_values.insert(key, value);
        }
        
        // Read metadata_null_counts
        cursor.read_exact(&mut len_buf)?;
        let null_counts_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_null_counts = HashMap::new();
        for _ in 0..null_counts_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            cursor.read_exact(&mut u32_buf)?;
            let value = u32::from_le_bytes(u32_buf);
            metadata_null_counts.insert(key, value);
        }
        
        Ok(Self {
            key,
            offset,
            size,
            block_id,
            block_offset,
            compressed,
            metadata_min_values,
            metadata_max_values,
            metadata_null_counts,
        })
    }
}

// Default function for serde when reading existing SSTable headers
// This preserves backward compatibility with existing SSTable files
fn default_block_size() -> u32 {
    4 * 1024 * 1024 // 4MB default for optimal ZSTD compression effectiveness
}

/// Data block for cache-optimized storage with ZSTD compression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBlock {
    pub block_id: u32,
    pub records: Vec<SstRecord>,
    pub uncompressed_size: u32,
    #[serde(default)]
    pub compression_algorithm: CompressionAlgorithmSst,
    #[serde(default)]
    pub compression_ratio: f32,
}

/// Configuration for DataBlock compression
#[derive(Debug, Clone)]
pub struct DataBlockCompressionConfig {
    pub enable_compression: bool,
    pub compression_threshold: usize, // Minimum block size to compress
    pub compression_level: i32,       // ZSTD compression level (1-22)
    pub vector_config: VectorSerializationConfig,
    pub collection_compression: Option<crate::proto::proximadb::CompressionConfig>, // Collection-level compression config
}

impl Default for DataBlockCompressionConfig {
    fn default() -> Self {
        Self {
            enable_compression: true,
            compression_threshold: 8192, // 8KB threshold
            compression_level: 3,        // Balanced speed/compression
            vector_config: VectorSerializationConfig::default(),
            collection_compression: None,
        }
    }
}

impl DataBlockCompressionConfig {
    /// Create from SstConfig settings
    pub fn from_sst_config(config: &SstConfig) -> Self {
        Self {
            enable_compression: false, // Will be determined by algorithm (None = disabled)
            compression_threshold: 8192, // 8KB threshold
            compression_level: config.compression_level,
            vector_config: VectorSerializationConfig {
                use_bytemuck: true,
                compression_threshold: 256,
                compression_algorithm: match config.compression.as_str() {
                    "zstd" => CompressionAlgorithm::Zstd,
                    "lz4" => CompressionAlgorithm::Lz4,
                    _ => CompressionAlgorithm::None,
                },
                compression_level: config.compression_level,
                adaptive_compression: true,
            },
            collection_compression: None,
        }
    }
    
    /// Create from proto CompressionConfig
    pub fn from_proto_config(config: Option<&crate::proto::proximadb::CompressionConfig>, vector_dim: usize) -> Self {
        if let Some(config) = config {
            let block_size = if config.dynamic_block_sizing {
                Self::optimal_block_size(vector_dim)
            } else {
                config.block_size_mb.unwrap_or(8) as usize * 1024 * 1024
            };
            
            Self {
                enable_compression: config.algorithm != crate::proto::proximadb::CompressionAlgorithm::CompressionNone as i32,
                compression_threshold: block_size / 1000, // Use 0.1% of block size as threshold
                compression_level: config.level.unwrap_or(3),
                vector_config: VectorSerializationConfig {
                    use_bytemuck: true,
                    compression_threshold: 256,
                    compression_algorithm: match config.algorithm {
                        x if x == crate::proto::proximadb::CompressionAlgorithm::CompressionZstd as i32 => CompressionAlgorithm::Zstd,
                        x if x == crate::proto::proximadb::CompressionAlgorithm::CompressionLz4 as i32 => CompressionAlgorithm::Lz4,
                        _ => CompressionAlgorithm::None,
                    },
                    compression_level: config.level.unwrap_or(3),
                    adaptive_compression: config.adaptive,
                },
                collection_compression: Some(config.clone()),
            }
        } else {
            Self::default()
        }
    }
    
    /// Calculate optimal block size based on vector dimensions
    /// Target: 2000-2500 vectors per block
    pub fn optimal_block_size(vector_dim: usize) -> usize {
        let vector_size = vector_dim * 4 + 100; // FP32 + metadata overhead
        let target_vectors = 2250;
        let block_size = target_vectors * vector_size;
        
        // Clamp between 4MB and 16MB
        block_size.max(4 * 1024 * 1024).min(16 * 1024 * 1024)
    }
}

// Self-contained block compression markers
const MARKER_UNCOMPRESSED: u8 = 0x02;
const MARKER_ZSTD: u8 = 0x03;
const MARKER_LZ4: u8 = 0x04;
const MARKER_SNAPPY: u8 = 0x05;
const MARKER_GZIP: u8 = 0x06;
const MARKER_BROTLI: u8 = 0x07;
const MARKER_BZIP2: u8 = 0x08;
const MARKER_DEFLATE: u8 = 0x09;
const MARKER_XZ: u8 = 0x0A;
const MARKER_ZLIB: u8 = 0x0B;
const MARKER_LZ4HC: u8 = 0x0C;
const MARKER_LZMA: u8 = 0x0D;

impl DataBlock {
    /// Create a new DataBlock with compression settings
    pub fn new(block_id: u32, records: Vec<SstRecord>) -> Self {
        let uncompressed_size = records.iter()
            .map(|r| r.vector.len() * 4 + r.id.len() + r.metadata.len() * 32) // Rough estimate
            .sum::<usize>() as u32;
            
        Self {
            block_id,
            records,
            uncompressed_size,
            compression_algorithm: CompressionAlgorithmSst::None,
            compression_ratio: 1.0,
        }
    }
    
    /// Serialize with ZSTD compression support
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        self.serialize_with_config(&DataBlockCompressionConfig::default())
    }
    
    /// Serialize with specific compression configuration
    pub fn serialize_with_config(&self, config: &DataBlockCompressionConfig) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        
        // Serialize each record with optimized vector serialization
        let mut records_data = Vec::new();
        for record in &self.records {
            let record_data = record.serialize_with_config(&config.vector_config)
                .context("Failed to serialize record with optimized config")?;
            records_data.write_all(&(record_data.len() as u32).to_le_bytes())?;
            records_data.write_all(&record_data)?;
        }
        
        // Create block metadata
        let block_metadata = DataBlockMetadata {
            block_id: self.block_id,
            record_count: self.records.len() as u32,
            uncompressed_size: self.uncompressed_size,
        };
        
        let metadata_data = bincode::serialize(&block_metadata)
            .context("Failed to serialize block metadata")?;
        
        // Combine metadata and records
        let mut raw_data = Vec::with_capacity(metadata_data.len() + records_data.len() + 8);
        raw_data.write_all(&(metadata_data.len() as u32).to_le_bytes())?;
        raw_data.write_all(&metadata_data)?;
        raw_data.write_all(&records_data)?;
        
        // Apply compression if beneficial (algorithm determined by config)
        if config.enable_compression && raw_data.len() >= config.compression_threshold {
            // Try compression with the configured algorithm
            // Use SST compression algorithm from config
            let compression_algo = if let Some(ref compression) = config.collection_compression {
                // Convert from proto to SST enum
                use crate::proto::proximadb::CompressionAlgorithm;
                match CompressionAlgorithm::try_from(compression.algorithm) {
                    Ok(CompressionAlgorithm::CompressionZstd) => CompressionAlgorithmSst::Zstd,
                    Ok(CompressionAlgorithm::CompressionLz4) => CompressionAlgorithmSst::Lz4,
                    Ok(CompressionAlgorithm::CompressionSnappy) => CompressionAlgorithmSst::Snappy,
                    Ok(CompressionAlgorithm::CompressionGzip) => CompressionAlgorithmSst::Gzip,
                    Ok(CompressionAlgorithm::CompressionBrotli) => CompressionAlgorithmSst::Brotli,
                    Ok(CompressionAlgorithm::CompressionBzip2) => CompressionAlgorithmSst::Bzip2,
                    Ok(CompressionAlgorithm::CompressionDeflate) => CompressionAlgorithmSst::Deflate,
                    Ok(CompressionAlgorithm::CompressionXz) => CompressionAlgorithmSst::Xz,
                    Ok(CompressionAlgorithm::CompressionZlib) => CompressionAlgorithmSst::Zlib,
                    Ok(CompressionAlgorithm::CompressionLz4hc) => CompressionAlgorithmSst::Lz4Hc,
                    Ok(CompressionAlgorithm::CompressionLzma) => CompressionAlgorithmSst::Lzma,
                    _ => CompressionAlgorithmSst::None,
                }
            } else {
                CompressionAlgorithmSst::None
            };
            
            let compression_result = match compression_algo {
                CompressionAlgorithmSst::Zstd => {
                    zstd::encode_all(raw_data.as_slice(), config.compression_level)
                        .map(|compressed| (compressed, MARKER_ZSTD))
                        .ok()
                }
                CompressionAlgorithmSst::Lz4 => {
                    let compressed = lz4_flex::compress(&raw_data);
                    Some((compressed, MARKER_LZ4))
                }
                CompressionAlgorithmSst::Snappy => {
                    let mut encoder = snap::raw::Encoder::new();
                    encoder.compress_vec(&raw_data)
                        .map(|compressed| (compressed, MARKER_SNAPPY))
                        .ok()
                }
                // 🔴 UNUSED - Gzip compression never used
                // CompressionAlgorithmSst::Gzip => {
                //     use flate2::write::GzEncoder;
                //     use flate2::Compression;
                //     use std::io::Write;
                //     let mut encoder = GzEncoder::new(Vec::new(), Compression::new(config.compression_level as u32));
                //     if encoder.write_all(&raw_data).is_err() {
                //         None
                //     } else {
                //         encoder.finish()
                //             .map(|compressed| (compressed, MARKER_GZIP))
                //             .ok()
                //     }
                // }
                // 🔴 UNUSED - Brotli compression never used
                // CompressionAlgorithmSst::Brotli => {
                //     use std::io::Write;
                //     let mut encoder = brotli::CompressorWriter::new(
                //         Vec::new(),
                //         4096,
                //         config.compression_level as u32,
                //         22
                //     );
                //     if encoder.write_all(&raw_data).is_err() {
                //         None
                //     } else {
                //         let compressed = encoder.into_inner();
                //         Some((compressed, MARKER_BROTLI))
                //     }
                // }
                // 🔴 UNUSED - Bzip2 compression never used
                // CompressionAlgorithmSst::Bzip2 => {
                //     use bzip2::write::BzEncoder;
                //     use bzip2::Compression;
                //     use std::io::Write;
                //     let mut encoder = BzEncoder::new(Vec::new(), Compression::new(config.compression_level as u32));
                //     if encoder.write_all(&raw_data).is_err() {
                //         None
                //     } else {
                //         encoder.finish()
                //             .map(|compressed| (compressed, MARKER_BZIP2))
                //             .ok()
                //     }
                // }
                CompressionAlgorithmSst::Deflate => {
                    use flate2::write::DeflateEncoder;
                    use flate2::Compression;
                    use std::io::Write;
                    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(config.compression_level as u32));
                    if encoder.write_all(&raw_data).is_err() {
                        None
                    } else {
                        encoder.finish()
                            .map(|compressed| (compressed, MARKER_DEFLATE))
                            .ok()
                    }
                }
                CompressionAlgorithmSst::Xz => {
                    use xz2::write::XzEncoder;
                    use std::io::Write;
                    let mut encoder = XzEncoder::new(Vec::new(), config.compression_level as u32);
                    if encoder.write_all(&raw_data).is_err() {
                        None
                    } else {
                        encoder.finish()
                            .map(|compressed| (compressed, MARKER_XZ))
                            .ok()
                    }
                }
                CompressionAlgorithmSst::Zlib => {
                    use flate2::write::ZlibEncoder;
                    use flate2::Compression;
                    use std::io::Write;
                    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(config.compression_level as u32));
                    if encoder.write_all(&raw_data).is_err() {
                        None
                    } else {
                        encoder.finish()
                            .map(|compressed| (compressed, MARKER_ZLIB))
                            .ok()
                    }
                }
                CompressionAlgorithmSst::Lz4Hc => {
                    // LZ4 high compression variant - use lz4_flex with same API
                    let compressed = lz4_flex::compress(&raw_data);
                    Some((compressed, MARKER_LZ4HC))
                }
                CompressionAlgorithmSst::Lzma => {
                    // Use XZ2 library's LZMA support
                    use xz2::write::XzEncoder;
                    use std::io::Write;
                    let mut encoder = XzEncoder::new(Vec::new(), config.compression_level as u32);
                    if encoder.write_all(&raw_data).is_err() {
                        None
                    } else {
                        encoder.finish()
                            .map(|compressed| (compressed, MARKER_LZMA))
                            .ok()
                    }
                }
                CompressionAlgorithmSst::None => None,
                _ => None  // For unsupported algorithms like LZO
            };
            
            if let Some((compressed, marker)) = compression_result {
                let compression_ratio = compressed.len() as f32 / raw_data.len() as f32;
                
                // Only use compression if it's beneficial (< 95% of original size)
                if compression_ratio < 0.95 {
                    let mut result = Vec::with_capacity(compressed.len() + 9);
                    result.write_all(&[marker])?; // Algorithm-specific marker
                    result.write_all(&(raw_data.len() as u32).to_le_bytes())?; // Original size
                    result.write_all(&compressed)?;
                    
                    tracing::debug!(
                        "✅ DataBlock {} compressed: {} → {} bytes ({:.1}% ratio)",
                        self.block_id, 
                        raw_data.len(), compressed.len(), compression_ratio * 100.0
                    );
                    
                    return Ok(result);
                }
            }
        }
        
        // Fallback to uncompressed format
        let mut result = Vec::with_capacity(raw_data.len() + 1);
        result.write_all(&[MARKER_UNCOMPRESSED])?; // Uncompressed format marker
        result.write_all(&raw_data)?;
        
        Ok(result)
    }
    
    /// Deserialize with automatic compression detection
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data for DataBlock deserialization"));
        }
        
        // Constants for self-contained block compression markers
        const MARKER_UNCOMPRESSED: u8 = 0x02;
        const MARKER_ZSTD: u8 = 0x03;
        const MARKER_LZ4: u8 = 0x04;
        const MARKER_SNAPPY: u8 = 0x05;
        const MARKER_GZIP: u8 = 0x06;
        const MARKER_BROTLI: u8 = 0x07;
        const MARKER_BZIP2: u8 = 0x08;
        const MARKER_DEFLATE: u8 = 0x09;
        const MARKER_XZ: u8 = 0x0A;
        const MARKER_ZLIB: u8 = 0x0B;
        const MARKER_LZ4HC: u8 = 0x0C;
        const MARKER_LZMA: u8 = 0x0D;
        
        match data[0] {
            MARKER_ZSTD => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Zstd),
            MARKER_LZ4 => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Lz4),
            MARKER_SNAPPY => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Snappy),
            MARKER_GZIP => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Gzip),
            MARKER_BROTLI => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Brotli),
            MARKER_BZIP2 => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Bzip2),
            MARKER_DEFLATE => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Deflate),
            MARKER_XZ => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Xz),
            MARKER_ZLIB => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Zlib),
            MARKER_LZ4HC => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Lz4Hc),
            MARKER_LZMA => Self::deserialize_compressed(&data[1..], CompressionAlgorithmSst::Lzma),
            MARKER_UNCOMPRESSED => Self::deserialize_uncompressed(&data[1..]),
            _ => {
                // Bincode format for backward compatibility
                let mut block: DataBlock = bincode::deserialize(data)
                    .map_err(|e| anyhow::anyhow!("Failed to deserialize DataBlock: {}", e))?;
                block.compression_algorithm = CompressionAlgorithmSst::None;
                block.compression_ratio = 1.0;
                Ok(block)
            }
        }
    }
    
    /// Deserialize compressed format (supports all algorithms)
    fn deserialize_compressed(data: &[u8], algorithm: CompressionAlgorithmSst) -> anyhow::Result<Self> {
        use std::io::Read;
        
        if data.len() < 4 {
            return Err(anyhow::anyhow!("Invalid compressed DataBlock: missing size header"));
        }
        
        // Read original size
        let original_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let compressed_data = &data[4..];
        
        // Decompress based on algorithm
        let decompressed = match algorithm {
            CompressionAlgorithmSst::Zstd => {
                zstd::decode_all(compressed_data)
                    .context("Failed to decompress DataBlock with ZSTD")?
            }
            CompressionAlgorithmSst::Lz4 => {
                lz4_flex::decompress(compressed_data, original_size)
                    .context("Failed to decompress DataBlock with LZ4")?
            }
            CompressionAlgorithmSst::Snappy => {
                let mut decoder = snap::read::FrameDecoder::new(std::io::Cursor::new(compressed_data));
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with Snappy")?;
                decompressed
            }
            CompressionAlgorithmSst::Gzip => {
                use flate2::read::GzDecoder;
                let mut decoder = GzDecoder::new(compressed_data);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with Gzip")?;
                decompressed
            }
            CompressionAlgorithmSst::Brotli => {
                use brotli::Decompressor;
                let mut decoder = Decompressor::new(compressed_data, 4096);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with Brotli")?;
                decompressed
            }
            CompressionAlgorithmSst::Bzip2 => {
                use bzip2::read::BzDecoder;
                let mut decoder = BzDecoder::new(compressed_data);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with Bzip2")?;
                decompressed
            }
            CompressionAlgorithmSst::Deflate => {
                use flate2::read::DeflateDecoder;
                let mut decoder = DeflateDecoder::new(compressed_data);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with Deflate")?;
                decompressed
            }
            CompressionAlgorithmSst::Xz => {
                use xz2::read::XzDecoder;
                let mut decoder = XzDecoder::new(compressed_data);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with XZ")?;
                decompressed
            }
            CompressionAlgorithmSst::Zlib => {
                use flate2::read::ZlibDecoder;
                let mut decoder = ZlibDecoder::new(compressed_data);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with Zlib")?;
                decompressed
            }
            CompressionAlgorithmSst::Lz4Hc => {
                // LZ4HC uses the same decompression as regular LZ4
                lz4_flex::decompress(compressed_data, original_size)
                    .context("Failed to decompress DataBlock with LZ4HC")?
            }
            CompressionAlgorithmSst::Lzma => {
                // Use XZ2 library's LZMA support
                use xz2::read::XzDecoder;
                let mut decoder = XzDecoder::new(compressed_data);
                let mut decompressed = Vec::with_capacity(original_size);
                decoder.read_to_end(&mut decompressed)
                    .context("Failed to decompress DataBlock with LZMA")?;
                decompressed
            }
            _ => return Err(anyhow::anyhow!("Unsupported compression algorithm: {:?}", algorithm)),
        };
            
        if decompressed.len() != original_size {
            return Err(anyhow::anyhow!(
                "DataBlock decompression size mismatch: expected {}, got {}",
                original_size, decompressed.len()
            ));
        }
        
        let mut block = Self::deserialize_uncompressed(&decompressed)?;
        block.compression_algorithm = CompressionAlgorithmSst::Zstd;
        block.compression_ratio = compressed_data.len() as f32 / original_size as f32;
        
        Ok(block)
    }
    
    /// Deserialize uncompressed format
    fn deserialize_uncompressed(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read metadata length
        let mut len_bytes = [0u8; 4];
        cursor.read_exact(&mut len_bytes)?;
        let metadata_len = u32::from_le_bytes(len_bytes) as usize;
        
        // Read and deserialize metadata
        let mut metadata_data = vec![0u8; metadata_len];
        cursor.read_exact(&mut metadata_data)?;
        
        let metadata: DataBlockMetadata = bincode::deserialize(&metadata_data)
            .context("Failed to deserialize DataBlock metadata")?;
        
        // Read records
        let mut records = Vec::with_capacity(metadata.record_count as usize);
        
        for _ in 0..metadata.record_count {
            // Read record length
            let mut len_bytes = [0u8; 4];
            cursor.read_exact(&mut len_bytes)?;
            let record_len = u32::from_le_bytes(len_bytes) as usize;
            
            // Read and deserialize record
            let mut record_data = vec![0u8; record_len];
            cursor.read_exact(&mut record_data)?;
            
            let record = SstRecord::deserialize(&record_data)
                .context("Failed to deserialize SstRecord in DataBlock")?;
            records.push(record);
        }
        
        Ok(DataBlock {
            block_id: metadata.block_id,
            records,
            uncompressed_size: metadata.uncompressed_size,
            compression_algorithm: CompressionAlgorithmSst::None,
            compression_ratio: 1.0,
        })
    }
    
    /// Get compression statistics
    pub fn compression_stats(&self) -> (bool, f32, usize) {
        (
            self.compression_algorithm != CompressionAlgorithmSst::None,
            self.compression_ratio,
            self.uncompressed_size as usize,
        )
    }
}

/// Metadata for DataBlock separate from record data
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DataBlockMetadata {
    pub block_id: u32,
    pub record_count: u32,
    pub uncompressed_size: u32,
}


/// Batch extraction statistics for performance monitoring
#[derive(Debug, Default)]
struct BatchExtractionStats {
    pub total_extracted: usize,
    pub total_skipped: usize,
    pub chunk_times: Vec<u64>, // In microseconds
    pub sort_time_us: u64,
}

impl BatchExtractionStats {
    fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub struct SstStorage {
    config: SstConfig,
    // NO collection_id - passed in parameters
    // NO data_dir - derived from parameters
    compaction_manager: Option<Arc<CompactionManager>>,
    filesystem: Arc<FilesystemFactory>,
    // Atomic coordinator for safe flush and compaction operations
    atomic_coordinator: Arc<TransactionCoordinator>,
    // Shared reader across all collections
    sstable_reader: Arc<UnifiedSstableReader>,
    // Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    // Shared decompression cache across all collections
    decompression_cache: Arc<decompression_cache::DecompressionCache>,
    // Shared quantization engine
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl SstStorage {
    pub async fn new(
        config: SstConfig,
        filesystem: Arc<FilesystemFactory>,
        distance_compute: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
    ) -> Result<Self> {
        info!("🌲 Creating SST storage engine (collection-agnostic singleton)");
        
        // SST is now a singleton - no collection-specific initialization
        // Collection-specific paths will be determined at operation time
        
        // Always create atomic coordinator for safe operations
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?
        );

        // Create SSTable reader
        let sstable_reader = Arc::new(UnifiedSstableReader::new(filesystem.clone()));
        
        // Create quantization engine (optional for SST)
        // For now, use in-memory codebook store since SST doesn't require quantization
        let codebook_store: Arc<dyn CodebookStore> = 
            Arc::new(InMemoryCodebookStore::new());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        // Initialize decompression cache with default size (64MB)
        let decompression_cache = Arc::new(decompression_cache::DecompressionCache::new(
            64 * 1024 * 1024,  // Default to 64MB cache
        ));
        
        // Initialize compaction manager (always enabled)
        let compaction_manager = Some(Arc::new(CompactionManager::new(
            config.clone(),
        )));

        Ok(Self {
            config,
            compaction_manager,
            filesystem,
            atomic_coordinator,
            sstable_reader,
            distance_compute,
            decompression_cache,
            quantization_engine,
        })
    }
    
    /// Get the collection storage URL from parameters
    fn get_collection_storage_url_from_params(params: &FlushParameters) -> Result<String> {
        // Extract storage location from collection config in parameters
        if let Some(ref collection) = params.collection_config {
            if let Some(ref assignment) = collection.storage_assignment {
                let storage_url = format!("{}/{}/data", 
                    assignment.base_location,
                    params.collection_id.as_ref().unwrap_or(&"default".to_string()));
                debug!("🔍 SST: Using storage URL from params: {}", storage_url);
                return Ok(storage_url);
            }
        }
        
        // Fallback to default if not provided
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for SST operations"))?;
        let storage_url = format!("file:///data/{}/data", collection_id);
        debug!("🔍 SST: Using default storage URL: {}", storage_url);
        Ok(storage_url)
    }
    
    
    /// Enable compaction with the SST tree's atomic coordinator
    pub async fn enable_compaction(&mut self, worker_count: usize) -> Result<()> {
        if self.compaction_manager.is_none() {
            let mut compaction_manager = CompactionManager::with_atomic_coordinator(
                self.config.clone(),
                Some(self.atomic_coordinator.clone()),
            );
            
            // Start background workers
            compaction_manager.start_workers(worker_count).await?;
            
            self.compaction_manager = Some(Arc::new(compaction_manager));
            
            info!("✅ SST: Compaction enabled with {} workers and atomic operations", worker_count);
        }
        Ok(())
    }
    



    // All writes go through WAL → Flush → SSTable directly
    // No intermediate memtable needed

    // 🔴 LEGACY METHOD - CANDIDATE FOR REMOVAL
    // This method is deprecated and has no active callers found in the codebase.
    // All usage has been migrated to do_flush through UnifiedStorageEngine trait.
    /*
    /// Legacy flush method - DEPRECATED - use do_flush through UnifiedStorageEngine trait
    #[deprecated(note = "Use do_flush through UnifiedStorageEngine trait instead")]
    async fn flush_vectors_direct_legacy(
        &self,
        vectors: Vec<VectorRecord>,
        collection_id: &str,
        collection_config: Option<&Collection>,
    ) -> Result<FlushResult> {
        if vectors.is_empty() {
            return Ok(FlushResult::default());
        }

        // Sort vectors by metadata for better SSTable organization and compression
        info!(
            "🔄 SST: Sorting {} vectors by metadata for optimal SSTable encoding",
            vectors.len()
        );
        let (sorted_vectors, sort_stats) = self.sort_vectors_for_sstable_encoding(vectors).await?;
        info!(
            "✅ SST: Sorted {} vectors (estimated compression improvement: {:.1}%)",
            sort_stats.records_sorted,
            sort_stats.compression_estimate * 100.0
        );

        // Get the collection storage URL from assignment service
        // Get storage URL from collection config - skip if not provided
        let collection_storage_url = match collection_config
            .and_then(|c| c.storage_assignment.as_ref()) {
            Some(assignment) => {
                let url = format!("{}/{}/data", assignment.base_location, collection_id);
                debug!("🔍 SST: Using storage URL: {} for collection: {}", url, collection_id);
                url
            }
            None => {
                error!("❌ SST: No storage assignment found for collection {}. Skipping flush operation!", collection_id);
                // Return early with failure result
                return Ok(FlushResult {
                    success: false,
                    collections_affected: vec![collection_id.to_string()],
                    entries_flushed: 0,
                    bytes_written: 0,
                    files_created: 0,
                    duration_ms: 0,
                    completed_at: Utc::now(),
                    engine_metrics: {
                        let mut metrics = HashMap::new();
                        metrics.insert("error".to_string(), 
                            serde_json::Value::String("Missing storage assignment".to_string()));
                        metrics
                    },
                    compaction_triggered: false,
                    flushed_batch_ids: vec![],
                });
            }
        };
        
        // Generate SSTable filename using centralized utility
        let sst_filename = SstFilenameGenerator::generate_flush_filename();
        debug!("🔧 SST: Creating SSTable file: {} for collection: {}", sst_filename, collection_id);
        
        // Convert sorted vectors to SstRecord format with sequence numbers
        // Handle both ID-based and append-only vectors
        let mut entries: BTreeMap<String, SstRecord> = BTreeMap::new();
        let mut sequence_number = 0u64;
        
        for vector in sorted_vectors {
            let vector_id = vector.id.as_deref().unwrap_or("").to_string();
            
            // Handle append-only vectors (empty/null IDs) specially
            if vector_id.is_empty() {
                // For append-only vectors, use sequence number as unique key
                let append_only_key = format!("__append_only_seq_{}", sequence_number);
                info!("🔍 DEBUG SST FLUSH: Append-only vector detected, using key='{}'", append_only_key);
                let mut sst_record = SstRecord::from_vector_record(vector);
                sst_record.sequence_number = sequence_number;
                sst_record.level = 0; // New SSTables start at level 0
                entries.insert(append_only_key, sst_record);
            } else {
                // Normal ID-based vector
                info!("🔍 DEBUG SST FLUSH: Inserting vector with id='{}' into BTreeMap", vector_id);
                let mut sst_record = SstRecord::from_vector_record(vector);
                sst_record.sequence_number = sequence_number;
                sst_record.level = 0; // New SSTables start at level 0
                entries.insert(vector_id, sst_record);
            }
            sequence_number += 1;
        }

        // Write SSTable using atomic operations (always available now)
        let atomic_coordinator = &self.atomic_coordinator;
        
        // Use atomic flush pattern
        info!("🔄 SST: Using atomic flush for {}", sst_filename);
        
        // Begin atomic operation
        let staging_config = StagingConfig {
            base_url: collection_storage_url.clone(),
            collection_id: None, // Already included in base_url
            operation_type: TransactionStageType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            skip_uuid_subdir: true,  // Avoid creating subdirectories that get left behind
            ..Default::default()
        };
        
        let atomic_op = atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic flush operation")?;
        
        // Write to staging using SSTable writer (legacy path - no compression config available)
        let staging_url = format!("{}/{}", atomic_op.staging_url, sst_filename);
        let block_size = (self.config.block_size_kb * 1024) as usize;
        let writer = SstableWriter::with_compression(&staging_url, block_size, Arc::clone(&self.filesystem), None); // No compression in legacy flush_vectors_direct path
        // Use bloom filter config from SST config if available
        let writer = if let Some(ref bloom_config) = self.config.bloom_filter_config {
            writer.with_bloom_config(bloom_config.clone())
        } else {
            writer
        };
        writer.write_records(entries.clone()).await
            .map_err(|e| anyhow::anyhow!("Failed to write SSTable to staging: {}", e))?;
        
        // Get file size from staging
        let fs = self.filesystem.get_filesystem(&staging_url)?;
        let metadata = fs.metadata(&staging_url)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get staging file size: {}", e))?;
        let file_size = metadata.size;
        
        // Finalize atomic operation
        atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic flush")?;
        
        let final_url = format!("{}/{}", collection_storage_url.trim_end_matches('/'), sst_filename);
        let (sst_url, data_len) = (final_url, file_size);
        debug!("💾 SST: SSTable written to URL: {} (collection_storage_url: {}, filename: {})", 
               sst_url, collection_storage_url, sst_filename);

        info!(
            "✅ SST: Flushed {} vectors to SSTable: {}",
            entries.len(),
            sst_url
        );
        
        // SSTable file is now discoverable via directory listing
        // No manifest registration needed - files are self-describing

        // Trigger compaction if manager is available
        if let Some(_compaction_manager) = &self.compaction_manager {
            let _task = CompactionTask {
                level: 0, // Start at level 0
                input_files: vec![std::path::PathBuf::from(sst_url.clone())],
                output_file: std::path::PathBuf::from(format!("{}.compacted", sst_url)),
                priority: CompactionPriority::Medium,
            };
            // For now, just log that we would trigger compaction
            tracing::debug!(
                "Would trigger compaction for collection: {}",
                collection_id
            );
            // compaction_manager.add_task(task).await?;
        }

        // Return flush result with statistics
        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: entries.len() as u64,
            bytes_written: data_len as u64,
            files_created: 1,
            duration_ms: 0, // Will be set by caller
            completed_at: Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert("sstable_path".to_string(), serde_json::Value::String(sst_url.clone()));
                metrics.insert("level".to_string(), serde_json::Value::Number(serde_json::Number::from(0)));
                metrics
            },
            compaction_triggered: self.compaction_manager.is_some(),
            flushed_batch_ids: vec![], // Would be provided by caller if needed
        })
    }
    */

    // SST is now pure SSTable storage - no memtable to query
    
}

// =============================================================================
// UNIFIED STORAGE ENGINE TRAIT IMPLEMENTATION FOR SST
// =============================================================================

#[async_trait]
impl UnifiedStorageEngine for SstStorage {
    // =============================================================================
    // ABSTRACT METHODS - SST-specific implementations
    // =============================================================================

    fn engine_name(&self) -> &'static str {
        "sst"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Lsm
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
            None
    }

    /// SST-specific flush implementation - Extract records from WAL vector record batches
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("🔄 SST: Starting do_flush with WAL vector record batch extraction");

        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for SST flush"))?;

        let operation_id = uuid::Uuid::new_v4().to_string();
        let vector_records = &params.vector_records;

        if vector_records.is_empty() {
            info!(
                "📋 SST: No vector records provided for collection {}",
                collection_id
            );
            return Ok(crate::storage::traits::FlushResult {
                success: true,
                collections_affected: vec![collection_id.clone()],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: {
                    let mut metrics = std::collections::HashMap::new();
                    metrics.insert(
                        "operation_id".to_string(),
                        serde_json::Value::String(operation_id.clone()),
                    );
                    metrics.insert("empty_flush".to_string(), serde_json::Value::Bool(true));
                    metrics
                },
                compaction_triggered: false,
                flushed_batch_ids: vec![],
            });
        }

        info!(
            "💾 SST: Processing {} vector records from WAL vector record batches",
            vector_records.len()
        );
        
        // DEBUG: Log first few vector records
        for (i, vr) in vector_records.iter().take(3).enumerate() {
            info!("🔍 DEBUG SST: Vector record {}: id={:?}, vector_len={}, metadata_count={}", 
                i, vr.id, vr.vector.len(), vr.metadata.len());
        }

        // Step 1: Extract individual records from deserialized WAL vector record batches
        // These batches come from the global partitioned memtable with WAL behavior
        let sst_records = self
            .extract_records_from_wal_vector_batches(vector_records, collection_id)
            .await
            .context("Failed to extract records from WAL vector record batches")?;

        info!(
            "📦 SST: Extracted {} individual records from {} vector record batches",
            sst_records.len(),
            vector_records.len()
        );
        
        // DEBUG: Log first few LSM records
        for (i, lr) in sst_records.iter().take(3).enumerate() {
            info!("🔍 DEBUG SST: LSM record {}: id={}, level={}, seq={}", 
                i, lr.id, lr.level, lr.sequence_number);
        }

        // Extract compression configuration from collection metadata (SDK-driven)
        let compression_config = params.collection_config.as_ref()
            .and_then(|collection| collection.config.as_ref())
            .and_then(|config| config.compression.clone());
            
        if let Some(ref _compression) = compression_config {
            info!("🗜️ SST: Using SDK-driven compression for collection {}", collection_id);
        } else {
            info!("🗜️ SST: No compression configuration from SDK for collection {}", collection_id);
        }

        // Step 2: Process extracted records using row-by-row storage approach
        info!("🔍 DEBUG SST: About to flush {} LSM records to SSTable", sst_records.len());
        let flush_result = self
            .flush_sst_records_to_sstable(sst_records, collection_id, params.collection_config.as_ref(), params.force, compression_config)
            .await
            .context("Failed to flush SST records to SSTable with row-by-row storage")?;
        info!("🔍 DEBUG SST: Flush completed - success={}, entries_flushed={}, bytes_written={}", 
            flush_result.success, flush_result.entries_flushed, flush_result.bytes_written);

        info!(
            "✅ SST: Successfully flushed {} records to {} SSTable files ({} bytes)",
            flush_result.entries_flushed,
            flush_result.files_created,
            flush_result.bytes_written
        );

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_flushed: flush_result.entries_flushed,
            bytes_written: flush_result.bytes_written,
            files_created: flush_result.files_created,
            duration_ms: 0, // Will be set by high-level flush() method
            completed_at: chrono::Utc::now(),
            engine_metrics: {
                let mut metrics = flush_result.engine_metrics;
                metrics.insert(
                    "operation_id".to_string(),
                    serde_json::Value::String(operation_id),
                );
                metrics.insert(
                    "extraction_source".to_string(),
                    serde_json::Value::String("wal_vector_record_batches".to_string()),
                );
                metrics.insert(
                    "storage_approach".to_string(),
                    serde_json::Value::String("row_by_row".to_string()),
                );
                metrics.insert(
                    "batch_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(vector_records.len())),
                );
                metrics.insert(
                    "extracted_records_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(flush_result.entries_flushed)),
                );
                metrics
            },
            compaction_triggered: flush_result.compaction_triggered,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// SST-specific compaction using level-based merge strategy with vector tracking
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let compact_start = std::time::Instant::now();
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for SST compaction"))?;

        tracing::info!(
            "🗜️ SST COMPACTION START: Collection {} (force: {}, priority: {:?})",
            collection_id,
            params.force,
            params.priority
        );

        let mut result = CompactionResult {
            success: false,
            collections_affected: Vec::new(),
            entries_processed: 0,
            entries_removed: 0,
            bytes_read: 0,
            bytes_written: 0,
            input_files: 0,
            output_files: 0,
            duration_ms: 0,
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        };

        // SST-specific compaction: Level-based SSTable merging
        if let Some(compaction_manager) = &self.compaction_manager {
            // Get storage location from collection config - skip if not provided
            let storage_location = match params.collection_config.as_ref()
                .and_then(|c| c.storage_assignment.as_ref()) {
                Some(assignment) => assignment.base_location.clone(),
                None => {
                    error!("❌ SST: No storage assignment found for collection {}. Skipping compaction!", collection_id);
                    // Return early with failure result
                    result.success = false;
                    result.collections_affected = vec![collection_id.to_string()];
                    result.duration_ms = compact_start.elapsed().as_millis() as u64;
                    result.engine_metrics.insert("error".to_string(), 
                        serde_json::Value::String("Missing storage assignment".to_string()));
                    return Ok(result);
                }
            };
                
            let collection_storage_url = format!("{}/{}/data", storage_location, collection_id);
            tracing::debug!(
                "🔄 SST COMPACTION: Checking for compaction needs in {}",
                collection_storage_url
            );

            let collection_dir = std::path::PathBuf::from(
                collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
            );

            // Check if compaction is needed
            if let Some(task) = compaction_manager
                .check_compaction_needed(collection_id, &collection_dir)
                .await?
            {
                tracing::info!(
                    "🔄 SST COMPACTION: Executing synchronous compaction for collection {} level {}",
                    collection_id, task.level
                );

                // Execute compaction synchronously to capture vector tracking
                let compaction_manager = compaction::CompactionManager::with_atomic_coordinator(
                    self.config.clone(),
                    Some(self.atomic_coordinator.clone()),
                );
                let enhanced_stats = compaction_manager.perform_compaction_enhanced(
                    &task,
                    &self.config,
                    Some(self.atomic_coordinator.clone()),
                ).await?;
                
                result.collections_affected.push(collection_id.clone());
                result.entries_processed = enhanced_stats.merged_vectors.len() as u64;
                result.entries_removed = enhanced_stats.deleted_vector_ids.len() as u64;
                result.bytes_read = enhanced_stats.base_stats.bytes_read;
                result.bytes_written = enhanced_stats.base_stats.bytes_written;
                result.input_files = enhanced_stats.base_stats.files_merged;
                result.output_files = 1; // One output file per compaction
                result.success = true;
                
                // Store vector tracking data in engine_metrics
                result.engine_metrics.insert(
                    "deleted_vector_ids".to_string(),
                    serde_json::Value::Array(
                        enhanced_stats.deleted_vector_ids.into_iter()
                            .map(serde_json::Value::String)
                            .collect()
                    )
                );
                result.engine_metrics.insert(
                    "merged_vectors_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(enhanced_stats.merged_vectors.len()))
                );
                
                // Note: We don't store the actual merged vectors in metrics to avoid memory bloat
                // The compaction process has already updated the storage with the merged data

                tracing::info!(
                    "✅ SST COMPACTION: Completed for collection {} (deleted: {}, merged: {}, bytes written: {})",
                    collection_id, 
                    result.entries_removed, 
                    result.entries_processed, 
                    enhanced_stats.base_stats.bytes_written
                );
            } else {
                tracing::debug!("📊 SST COMPACTION: No compaction needed for collection {}", collection_id);
                result.success = true; // No compaction needed is still successful
            }
        } else {
            tracing::warn!("⚠️ SST COMPACTION: No compaction manager available");
            result.success = false;
        }

        result.duration_ms = compact_start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Retrieve vector by ID from SST storage (Pure SSTable lookup with bloom filter optimization)
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<crate::core::VectorRecord>> {
        tracing::debug!("🔍 SST: Looking up vector {} in collection {} using manifest", vector_id, collection_id);

        // Note: In a real implementation, we'd get the storage location from a metadata service
        // For now, we'll just log and return None if we can't determine the location
        warn!("⚠️ SST: get_vector_by_id needs collection metadata service integration for collection {}", collection_id);
        
        // Get SSTable files that might contain this key
        // Direct directory scan for overlapping files (simplified for now)
        let overlapping_files: Vec<String> = vec![];
        
        if overlapping_files.is_empty() {
            tracing::debug!("📂 SST: No SSTable files overlap with key {}", vector_id);
            return Ok(None);
        }
        
        let mut sstables_checked = 0;
        let mut bloom_filter_hits = 0;
        
        // Search through files in key range order
        for file_path in overlapping_files {
            sstables_checked += 1;
            
            let filename = std::path::Path::new(&file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            
            // Use unified SSTable reader with bloom filter
            let reader = UnifiedSstableReader::new(self.filesystem.clone());
            
            // Load metadata (includes bloom filter)
            if reader.load_metadata(&file_path).await.is_ok() {
                // Check bloom filter first
                if reader.might_contain_key(&file_path, vector_id).await {
                    bloom_filter_hits += 1;
                    tracing::trace!("🌸 SST: Bloom filter hit for {} in {}", vector_id, filename);
                    
                    // Actually search the SSTable
                    if let Ok(Some(record)) = reader.get_vector(&file_path, vector_id).await {
                        tracing::debug!(
                            "✅ SST: Found vector {} in SSTable {} (checked {}/{} SSTables, {} bloom hits)",
                            vector_id, filename, bloom_filter_hits, sstables_checked, bloom_filter_hits
                        );
                        return Ok(Some(record));
                    }
                } else {
                    tracing::trace!("🌸 SST: Bloom filter miss for {} in {} - skipping", vector_id, filename);
                }
            } else {
                tracing::warn!("⚠️ Failed to load metadata for SSTable {}", filename);
            }
        }

        tracing::debug!(
            "❌ SST: Vector {} not found in collection {} (checked {} SSTables, {} bloom hits)",
            vector_id, collection_id, sstables_checked, bloom_filter_hits
        );
        Ok(None)
    }

    /// SST ENGINE OPTIMIZATION: Unified search using SstUnifiedSearchEngine
    async fn search_vectors_unified(
        &self,
        collection_id: &str,
        storage_url: &str,  // Storage URL provided by VectorOperationsService
        query_vector: &[f32],
        k: usize,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
        filter_expression: Option<&crate::core::search::FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        // SST engine is instantiated per collection and stores data in collection-specific directories
        // No need to check collection_id - the engine inherently only has data for its collection
        
        info!("🔍 SST: Using unified search engine for collection {}", collection_id);
        
        // Build search parameters
        let search_params = crate::core::search::SearchParams {
            query_vectors: Some(vec![query_vector.to_vec()]),
            top_k: Some(k),
            distance_metric: Some(*distance_metric),
            filter_expression: filter_expression.cloned(),
            ..Default::default()
        };
        
        // Use the provided storage URL directly
        debug!("🔍 SST: Using provided storage_url = {} for collection {}", storage_url, collection_id);
        
        // Pre-discover SSTable files to avoid redundant filesystem queries
        let sstable_files = {
            let mut files = Vec::new();
            let fs = self.filesystem.get_filesystem(storage_url)?;
            let entries = fs.list(storage_url).await?;
            for entry in entries {
                if !entry.metadata.is_directory && entry.name.ends_with(".sst") {
                    files.push(entry.url);
                }
            }
            files
        };
        debug!("🔍 SST: Pre-discovered {} SSTable files", sstable_files.len());
        
        let context = crate::core::search::UnifiedSearchContext {
            collection_id: collection_id.to_string(),
            collection_config: Some(crate::core::search::CollectionConfig {
                default_distance_metric: *distance_metric,
                vector_dimension: query_vector.len(),
                enable_quantization: false,
                enable_metadata_filtering: self.config.bloom_filter_config.is_some(),
                estimated_document_count: 10000, // Will be discovered by unified search engine
            }),
            filterable_columns: Vec::new(), // TODO: Extract from schema
            available_quantization: Vec::new(),
            storage_info: crate::core::search::StorageInfo {
                is_cloud_storage: !storage_url.starts_with("file://"),
                storage_type: "SST".to_string(),
                estimated_size_mb: 100.0, // Will be discovered by unified search engine
                file_count: sstable_files.len(),
                supports_range_requests: true,
                file_paths: Some(sstable_files), // Pass pre-discovered files
            },
        };
        
        // Create a search engine instance for this search
        let search_engine = unified_search_engine::SstUnifiedSearchEngine::new(
            self.sstable_reader.clone(),
            self.distance_compute.clone(),
            self.quantization_engine.clone(),
            storage_url.to_string(),
            self.filesystem.clone(),
        );
        
        // Use the search engine
        let result_set = search_engine.search_unified(
            &context,
            &search_params,
            &self.distance_compute,
            Some(&*self.quantization_engine),
        ).await?;
        
        // Filter results based on include_vectors and include_metadata
        let mut results: Vec<SearchResult> = result_set.results.iter().cloned().collect();
        if !include_vectors {
            for result in &mut results {
                result.vector = None;
            }
        }
        if !include_metadata {
            for result in &mut results {
                result.metadata.clear();
            }
        }
        
        debug!("✅ SST: Found {} results (top {} requested)", results.len(), k);
        
        // Debug: print sample results before returning
        for (i, result) in results.iter().take(3).enumerate() {
            debug!("  SST Result {}: id={}, score={}", 
                  i, result.id, result.score);
        }
        
        Ok(results)
    }

    /// SST-specific engine metrics
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        metrics.insert(
            "engine_type".to_string(),
            serde_json::Value::String("SST".to_string()),
        );
        // Collection ID is no longer stored in the engine
        // It would be passed as context when needed
        metrics.insert(
            "storage_type".to_string(),
            serde_json::Value::String("Pure SSTable".to_string()),
        );
        metrics.insert(
            "compaction_threshold".to_string(),
            serde_json::Value::Number((self.config.compaction_threshold as u64).into()),
        );
        metrics.insert(
            "level_count".to_string(),
            serde_json::Value::Number((self.config.level_count as u64).into()),
        );
        metrics.insert(
            "storage_format".to_string(),
            serde_json::Value::String("SSTable".to_string()),
        );
        metrics.insert(
            "has_compaction_manager".to_string(),
            serde_json::Value::Bool(self.compaction_manager.is_some()),
        );

        // Count SSTable files instead of memtable utilization
        let sstable_count = self.count_sstables_at_level(0).await.unwrap_or(0);
        metrics.insert(
            "sstable_count".to_string(),
            serde_json::Value::Number((sstable_count as u64).into()),
        );

        Ok(metrics)
    }
    
}

// =============================================================================
// SST IMPLEMENTATION HELPER METHODS (Private)
// =============================================================================

impl SstStorage {
    /// Extract individual records from deserialized WAL vector record batches
    /// These batches come from the global partitioned memtable with WAL behavior
    /// Enhanced with batch processing optimizations for improved performance
    async fn extract_records_from_wal_vector_batches(
        &self,
        vector_records: &[VectorRecord],
        collection_id: &str,
    ) -> Result<Vec<SstRecord>> {
        let extraction_start = std::time::Instant::now();
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;

        info!(
            "🔍 SST ENGINE-OPTIMIZED EXTRACTION: Processing {} WAL vector record batches for collection {}",
            vector_records.len(),
            collection_id
        );

        // Pre-allocate with estimated capacity for better memory efficiency
        let estimated_matches = vector_records.len() / 4; // Conservative estimate
        let mut sst_records = Vec::with_capacity(estimated_matches);

        // Batch optimization: Use vectorized processing for better performance
        let mut batch_stats = BatchExtractionStats::new();

        // Process records in chunks for better cache locality
        const CHUNK_SIZE: usize = 1000;
        for (chunk_idx, chunk) in vector_records.chunks(CHUNK_SIZE).enumerate() {
            let chunk_start = std::time::Instant::now();
            let mut chunk_matches = 0;

            for (index, vector_record) in chunk.iter().enumerate() {
                // All records should already be filtered for this collection
                let global_index = chunk_idx * CHUNK_SIZE + index;
                
                // Debug: log metadata before conversion
                if global_index < 5 {
                    debug!("🔍 Pre-conversion record {}: id={:?}, metadata={:?}", 
                             global_index,
                             vector_record.id,
                             vector_record.metadata.iter().map(|m| format!("{}={:?}", m.key, m.value)).collect::<Vec<_>>());
                }
                
                // Convert VectorRecord to SstRecord for row-by-row storage
                let mut sst_record = SstRecord::from_vector_record(vector_record.clone());
                
                // Set SST-specific fields for proper ordering and level management
                sst_record.sequence_number = sequence_start + global_index as u64;
                sst_record.level = 0; // New records from WAL start at level 0
                sst_record.is_tombstone = false; // WAL records are active (not tombstones)
                
                sst_records.push(sst_record);
                chunk_matches += 1;
                
                batch_stats.total_extracted += 1;
            }

            let chunk_time = chunk_start.elapsed().as_micros() as u64;
            batch_stats.chunk_times.push(chunk_time);
            
            tracing::debug!(
                "📦 SST CHUNK {}: Processed {} records, {} matches in {}μs",
                chunk_idx,
                chunk.len(),
                chunk_matches,
                chunk_time
            );
        }

        // Sort records by sequence number for optimal SSTable performance
        if sst_records.len() > 1 {
            let sort_start = std::time::Instant::now();
            sst_records.sort_by_key(|r| r.sequence_number);
            batch_stats.sort_time_us = sort_start.elapsed().as_micros() as u64;
        }

        let total_extraction_time = extraction_start.elapsed().as_millis() as u64;
        let avg_chunk_time = if !batch_stats.chunk_times.is_empty() {
            batch_stats.chunk_times.iter().sum::<u64>() / batch_stats.chunk_times.len() as u64
        } else {
            0
        };

        info!(
            "🚀 SST ENGINE-OPTIMIZED EXTRACTION COMPLETE: {} records extracted from {} WAL records in {}ms (avg chunk: {}μs, sort: {}μs)",
            sst_records.len(),
            vector_records.len(),
            total_extraction_time,
            avg_chunk_time,
            batch_stats.sort_time_us
        );

        Ok(sst_records)
    }


    /// Flush memtable data to SSTable files using SST's row-based architecture
    async fn flush_sst_records_to_sstable(
        &self,
        sst_records: Vec<SstRecord>,
        collection_id: &str,
        collection_config: Option<&Collection>,
        _force_flush: bool,
        compression_config: Option<crate::proto::proximadb::CompressionConfig>,
    ) -> Result<FlushResult> {
        let flush_start = std::time::Instant::now();

        tracing::info!(
            "🗂️ SST SSTABLE FLUSH: Processing {} records",
            sst_records.len()
        );
        
        // DEBUG: Log record details
        if sst_records.is_empty() {
            tracing::warn!("🔍 DEBUG SST: No records to flush - returning early!");
        } else {
            tracing::info!("🔍 DEBUG SST: First record: id={}", 
                sst_records[0].id);
        }

        // Stage 1: Sort records by ID for SSTable ordering
        let sorting_start = std::time::Instant::now();
        let mut sorted_records = sst_records;
        sorted_records.sort_by(|a, b| a.id.cmp(&b.id));
        let sorting_time = sorting_start.elapsed().as_millis() as u64;
        tracing::debug!(
            "📊 SST STAGE 1: Sorted {} records in {}ms",
            sorted_records.len(),
            sorting_time
        );

        // Stage 2: Partition records into levels based on SST tree structure
        let partitioning_start = std::time::Instant::now();
        let level_partitions = self.partition_records_by_level(&sorted_records).await?;
        let partitioning_time = partitioning_start.elapsed().as_millis() as u64;
        let num_levels = level_partitions.len();
        tracing::debug!(
            "🏗️ SST STAGE 2: Partitioned into {} levels in {}ms",
            num_levels,
            partitioning_time
        );

        // Stage 3: Create SSTable files for each level
        let sstable_start = std::time::Instant::now();
        let mut total_bytes_written = 0u64;
        let mut files_created = 0u64;
        let mut sstable_paths = Vec::new();

        for (level, level_records) in level_partitions {
            if level_records.is_empty() {
                continue;
            }

            // Get the collection storage URL from assignment service
            // Get storage URL from collection config - skip if not provided
            let collection_storage_url = match collection_config
                .and_then(|c| c.storage_assignment.as_ref()) {
                Some(assignment) => {
                    let url = format!("{}/{}/data", assignment.base_location, collection_id);
                    debug!("🔍 SST: Using storage URL: {} for collection: {}", url, collection_id);
                    url
                }
                None => {
                    error!("❌ SST: No storage assignment found for collection {}. Skipping flush operation!", collection_id);
                    // Return early with failure result
                    return Ok(FlushResult {
                        success: false,
                        collections_affected: vec![collection_id.to_string()],
                        entries_flushed: 0,
                        bytes_written: 0,
                        files_created: 0,
                        duration_ms: flush_start.elapsed().as_millis() as u64,
                        completed_at: Utc::now(),
                        engine_metrics: {
                            let mut metrics = HashMap::new();
                            metrics.insert("error".to_string(), 
                                serde_json::Value::String("Missing storage assignment".to_string()));
                            metrics
                        },
                        compaction_triggered: false,
                        flushed_batch_ids: vec![],
                    });
                }
            };
            let data_dir = PathBuf::from(
                collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
            );

            // Generate SSTable filename using centralized utility
            let sst_filename = SstFilenameGenerator::generate_compaction_filename(level);
            let sst_path = data_dir.join(&sst_filename);
            debug!("🔧 SST: Creating compacted SSTable file: {} at path: {} for collection: {}", 
                   sst_filename, sst_path.display(), collection_id);

            // Ensure directory exists
            if let Some(parent) = sst_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create directory: {}", e))?;
            }

            // Convert SstRecords to BTreeMap for SstableWriter
            // Handle append-only vectors with unique keys
            let mut entries = BTreeMap::new();
            let mut append_only_counter = 0u64;
            
            for record in &level_records {
                let key = if record.id.is_empty() {
                    // For append-only vectors (empty IDs), use a unique key
                    let unique_key = format!("__append_only_seq_{}", append_only_counter);
                    append_only_counter += 1;
                    info!("🔍 SST FLUSH: Append-only vector detected in level {}, using key='{}'", level, unique_key);
                    unique_key
                } else {
                    record.id.clone()
                };
                entries.insert(key, record.clone());
            }

            // Use SstableWriter for consistent format with SDK-driven compression
            let block_size = (self.config.block_size_kb * 1024) as usize;
            let writer = sstable_writer::SstableWriter::with_compression(&sst_path, block_size, Arc::clone(&self.filesystem), compression_config.clone());
            
            // Use bloom filter config from SST config if available
            let writer = if let Some(ref bloom_config) = self.config.bloom_filter_config {
                writer.with_bloom_config(bloom_config.clone())
            } else {
                writer
            };
            
            // Write records using SstableWriter
            debug!("🔍 SST: About to write {} entries to file: {}", entries.len(), sst_path.display());
            writer.write_records(entries).await
                .map_err(|e| anyhow::anyhow!("Failed to write SSTable: {}", e))?;
            debug!("🔍 SST: Successfully wrote SSTable file: {}", sst_path.display());

            // Get file size
            let metadata = tokio::fs::metadata(&sst_path).await?;
            let file_size = metadata.len();
            total_bytes_written += file_size;
            files_created += 1;
            sstable_paths.push(sst_path.clone());
            
            // Verify file exists
            if metadata.len() > 0 {
                debug!("✅ SST: Compacted SSTable verified - {} bytes at {}", file_size, sst_path.display());
            } else {
                warn!("⚠️ SST: Compacted SSTable file is empty: {}", sst_path.display());
            }

            tracing::debug!(
                "💾 SST STAGE 3: Level {} SSTable {} written - {} records, {} bytes",
                level,
                sst_filename,
                level_records.len(),
                file_size
            );
        }

        let sstable_time = sstable_start.elapsed().as_millis() as u64;

        // Stage 4: Update SST tree metadata and indexes
        let metadata_start = std::time::Instant::now();
        self.update_lsm_metadata_after_flush(&sstable_paths, &sorted_records)
            .await?;
        let metadata_time = metadata_start.elapsed().as_millis() as u64;

        // Stage 5: Invalidate decompression cache for this collection
        // This ensures cached blocks are refreshed after new data is flushed
        // Note: Decompression cache invalidation removed as SST is now collection-agnostic
        // Cache invalidation should be handled at a higher level if needed
        
        // Stage 6: Trigger compaction if threshold exceeded
        let compaction_check_start = std::time::Instant::now();
        let compaction_triggered = self.check_compaction_threshold().await?;
        let compaction_check_time = compaction_check_start.elapsed().as_millis() as u64;

        let total_flush_time = flush_start.elapsed().as_millis() as u64;

        // Build detailed engine metrics
        let mut engine_metrics = HashMap::new();
        engine_metrics.insert(
            "sorting_time_ms".to_string(),
            serde_json::Value::Number(sorting_time.into()),
        );
        engine_metrics.insert(
            "partitioning_time_ms".to_string(),
            serde_json::Value::Number(partitioning_time.into()),
        );
        engine_metrics.insert(
            "sstable_creation_time_ms".to_string(),
            serde_json::Value::Number(sstable_time.into()),
        );
        engine_metrics.insert(
            "metadata_update_time_ms".to_string(),
            serde_json::Value::Number(metadata_time.into()),
        );
        engine_metrics.insert(
            "compaction_check_time_ms".to_string(),
            serde_json::Value::Number(compaction_check_time.into()),
        );
        engine_metrics.insert(
            "total_flush_time_ms".to_string(),
            serde_json::Value::Number(total_flush_time.into()),
        );
        engine_metrics.insert(
            "levels_created".to_string(),
            serde_json::Value::Number(num_levels.into()),
        );
        engine_metrics.insert(
            "sstables_created".to_string(),
            serde_json::Value::Number(files_created.into()),
        );
        engine_metrics.insert(
            "compaction_triggered".to_string(),
            serde_json::Value::Bool(compaction_triggered),
        );
        engine_metrics.insert(
            "storage_format".to_string(),
            serde_json::Value::String("SSTable".to_string()),
        );
        engine_metrics.insert(
            "serialization_format".to_string(),
            serde_json::Value::String("Bincode".to_string()),
        );

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: sorted_records.len() as u64,
            bytes_written: total_bytes_written,
            files_created,
            duration_ms: total_flush_time,
            completed_at: Utc::now(),
            compaction_triggered,
            engine_metrics,
            flushed_batch_ids: vec![],
        })
    }

    /// Partition records into SST tree levels based on key ranges and record age
    async fn partition_records_by_level(
        &self,
        sorted_records: &[SstRecord],
    ) -> Result<HashMap<u8, Vec<SstRecord>>> {
        let mut level_partitions: HashMap<u8, Vec<SstRecord>> = HashMap::new();

        // SST Level 0: Recent entries (direct from memtable)
        // Level 1+: Compacted entries (would come from compaction process)

        let records_per_level = 10000; // Fixed number of records per level for pure SSTable storage

        for (i, record) in sorted_records.iter().enumerate() {
            let level = if i < records_per_level {
                0 // Most recent records go to Level 0
            } else {
                // Distribute older records across higher levels
                ((i / records_per_level) as u8).min(self.config.level_count - 1)
            };

            level_partitions
                .entry(level)
                .or_insert_with(Vec::new)
                .push(record.clone());
        }

        Ok(level_partitions)
    }

    /// Engine-optimized batch serialization to row-based SSTable format
    /// Includes compression, bloom filters, and block-based organization
    async fn serialize_sst_records_to_sstable(
        &self,
        records: &[SstRecord],
        level: u8,
    ) -> Result<Vec<u8>> {
        let serialization_start = std::time::Instant::now();
        
        // Engine optimization: Pre-allocate based on estimated size
        let estimated_size = records.len() * 512; // Conservative estimate per record
        let mut sstable_data = Vec::with_capacity(estimated_size);

        // Step 1: Create enhanced header with engine optimizations
        let header = SstableHeader {
            version: 1, // Version 1 for initial implementation
            level,
            entry_count: records.len() as u64,
            min_key: records.first().map(|r| r.id.clone()).unwrap_or_default(),
            max_key: records.last().map(|r| r.id.clone()).unwrap_or_default(),
            created_at: Utc::now().timestamp(),
            // Engine optimizations - compression determined by algorithm (None = disabled)
            compression_algorithm: CompressionAlgorithmSst::None,  // No compression by default
            compression_level: 0,  // No compression level by default
            has_bloom_filter: true,
            block_size: (self.config.block_size_kb * 1024) as u32, // Use configured block size
            batch_size: records.len() as u32,
            // Additional fields (will be updated later)
            header_size: 0,
            index_size: 0,
            data_size: 0,
            block_count: 0,
        };

        // Step 2: Build bloom filter for fast key existence checks
        let bloom_filter = self.build_bloom_filter(records).await?;
        let bloom_data = bloom_filter.serialize()
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))?;

        // Step 3: Organize records into blocks for better cache performance
        let data_blocks = self.organize_records_into_blocks(records, header.block_size as usize).await?;
        
        // Step 4: Engine-optimized index with block pointers
        let (index_entries, compressed_blocks) = self.build_optimized_index_and_compress_blocks(&data_blocks).await?;

        // Step 5: Serialize header
        let header_data = bincode::serialize(&header)
            .map_err(|e| anyhow::anyhow!("Failed to serialize header: {}", e))?;
        sstable_data.extend((header_data.len() as u32).to_le_bytes());
        sstable_data.extend(header_data);

        // Step 6: Serialize bloom filter
        sstable_data.extend((bloom_data.len() as u32).to_le_bytes());
        sstable_data.extend(bloom_data);

        // Step 7: Serialize enhanced index using custom serialization
        let mut index_data = Vec::new();
        for entry in &index_entries {
            let entry_data = entry.serialize()
                .map_err(|e| anyhow::anyhow!("Failed to serialize index entry: {}", e))?;
            index_data.extend_from_slice(&(entry_data.len() as u32).to_le_bytes());
            index_data.extend_from_slice(&entry_data);
        }
        sstable_data.extend((index_data.len() as u32).to_le_bytes());
        sstable_data.extend(index_data);

        // Step 8: Append compressed data blocks
        let total_data_size = compressed_blocks.iter().map(|b| b.len()).sum::<usize>();
        sstable_data.extend(compressed_blocks.into_iter().flatten());

        let serialization_time = serialization_start.elapsed().as_millis() as u64;
        let compression_ratio = if total_data_size > 0 {
            estimated_size as f64 / sstable_data.len() as f64
        } else {
            1.0
        };

        tracing::info!(
            "🚀 SST ENGINE-OPTIMIZED SSTABLE: Level {} serialized - {} records, {} bytes, {:.2}x compression, {}ms",
            level, records.len(), sstable_data.len(), compression_ratio, serialization_time
        );

        Ok(sstable_data)
    }

    /// Update SST tree metadata after successful flush
    async fn update_lsm_metadata_after_flush(
        &self,
        sstable_paths: &[std::path::PathBuf],
        flushed_records: &[SstRecord],
    ) -> Result<()> {
        tracing::info!(
            "📊 SST METADATA: Updating manifest for {} SSTables, {} records",
            sstable_paths.len(),
            flushed_records.len()
        );

        // Register each SSTable file with the manifest
        for path in sstable_paths {
            // Extract filename from path
            let filename = path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid SSTable filename"))?;
            
            // Parse level from filename using centralized utility
            let level = SstFilenameGenerator::parse_level_from_filename(filename).unwrap_or(0);
            
            // Get file size
            let metadata = tokio::fs::metadata(path).await?;
            let _file_size = metadata.len();
            
            // Calculate min/max keys and sequences from records in this SSTable
            let sstable_records: Vec<&SstRecord> = flushed_records.iter()
                .filter(|r| r.level == level)
                .collect();
            
            if sstable_records.is_empty() {
                continue;
            }
            
            let _min_key = sstable_records.iter().map(|r| &r.id).min().cloned().unwrap_or_default();
            let _max_key = sstable_records.iter().map(|r| &r.id).max().cloned().unwrap_or_default();
            let _min_sequence = sstable_records.iter().map(|r| r.sequence_number).min().unwrap_or(0);
            let _max_sequence = sstable_records.iter().map(|r| r.sequence_number).max().unwrap_or(0);
            
            
            // SSTable file is now discoverable via directory listing
            info!("Created SSTable file: {} with {} records at level {}", filename, sstable_records.len(), level);
        }

        Ok(())
    }

    /// Check if compaction is needed based on SST tree structure
    async fn check_compaction_threshold(&self) -> Result<bool> {
        // Check Level 0 file count (trigger compaction if too many files)
        let level0_files = self.count_sstables_at_level(0).await?;
        let compaction_needed = level0_files >= self.config.compaction_threshold as usize;

        if compaction_needed {
            tracing::debug!(
                "🗜️ SST COMPACTION: Threshold exceeded - {} Level 0 files (threshold: {})",
                level0_files,
                self.config.compaction_threshold
            );
        }

        Ok(compaction_needed)
    }

    /// Count SSTable files at a specific level
    async fn count_sstables_at_level(&self, level: u8) -> Result<usize> {
        // SST is collection-agnostic, use a generic path
        let level_dir = std::path::PathBuf::from("/tmp/sst_staging");
        if !level_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut dir_entries = tokio::fs::read_dir(&level_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read level directory: {}", e))?;

        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            if let Some(filename) = entry.file_name().to_str() {
                if SstFilenameGenerator::is_sst_file(filename) && 
                   SstFilenameGenerator::parse_level_from_filename(filename) == Some(level) {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Convert vector records directly to row-based SSTable format for staging pattern
    async fn serialize_records_to_sstable_row_format(
        &self,
        vector_records: &[VectorRecord],
    ) -> Result<Vec<u8>> {
        tracing::info!(
            "📦 SST: Serializing {} vector records to row-based SSTable format",
            vector_records.len()
        );

        // Convert VectorRecords to SstRecords with proper sequencing
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;
        let mut sst_records = Vec::new();

        for (index, record) in vector_records.iter().enumerate() {
            // DEBUG: Log the vector ID being converted
            info!("🔍 DEBUG SST FLUSH: Converting vector {} with id={:?}", index, record.id);
            let mut sst_record = SstRecord::from_vector_record(record.clone());
            info!("🔍 DEBUG SST FLUSH: SstRecord has id='{}'", sst_record.id);
            sst_record.sequence_number = sequence_start + index as u64;
            sst_record.level = 0; // New records start at level 0
            sst_records.push(sst_record);
        }

        tracing::debug!(
            "🔄 SST: Converted {} vector records to row-based SST records",
            sst_records.len()
        );

        // Sort records by ID for SSTable format
        let mut sorted_records = sst_records;
        sorted_records.sort_by(|a, b| a.id.cmp(&b.id));

        // Serialize to row-based SSTable format (Level 0 by default for new data)
        self.serialize_sst_records_to_sstable(&sorted_records, 0).await
    }


    /// Build bloom filter for fast key existence checks
    async fn build_bloom_filter(&self, records: &[SstRecord]) -> Result<SstableBloomFilter> {
        // Create key bloom filter
        let key_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut key_filter = BloomFilterFactory::create(&key_config);
        
        // Create metadata bloom filter
        let metadata_config = BloomFilterConfig {
            strategy: BloomStrategy::Composite,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut metadata_builder = crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder::new(metadata_config);
        
        // Add all keys and metadata to filters
        for record in records {
            key_filter.insert(record.id.as_bytes());
            
            // Add metadata values - already have MetadataItem
            for metadata_item in &record.metadata {
                metadata_builder.add_metadata_item(metadata_item.key.clone(), metadata_item.clone());
            }
        }
        
        let metadata_filter = metadata_builder.build();
        
        // Create the SstableBloomFilter manually
        let stats = bloom_filter::BloomFilterStats {
            key_count: key_filter.num_elements() as u64,
            metadata_columns: metadata_filter.num_columns() as u64,
            total_keys: 0,
            key_lookups_saved: 0,
            metadata_queries_saved: 0,
        };
        
        let key_filter_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: key_filter.num_elements(),
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        };
        
        let sstable_filter = SstableBloomFilter::new(
            key_filter_config,
            key_filter.serialize()?,
            BloomFilterStrategy::serialize(&metadata_filter)?,
            stats,
        );
        
        debug!(
            "📊 SST: Built SSTable bloom filter for {} keys (FPR: {:.2}%)",
            records.len(),
            key_filter.false_positive_rate() * 100.0
        );
        
        Ok(sstable_filter)
    }

    /// Sort vector records by metadata for optimal SSTable encoding
    async fn sort_vectors_for_sstable_encoding(
        &self,
        vectors: Vec<VectorRecord>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        // For SST, we don't have direct access to collection config here
        // So we implement a simple but effective sorting strategy:
        // 1. Sort by first metadata key alphabetically
        // 2. Then by vector ID for stable ordering
        
        let mut sorted_vectors = vectors;
        
        // Find the most common metadata key for primary sorting
        let mut key_frequency: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for vector in &sorted_vectors {
            for metadata_item in &vector.metadata {
                *key_frequency.entry(metadata_item.key.clone()).or_insert(0) += 1;
            }
        }
        
        let primary_sort_key = key_frequency
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(key, _)| key.clone());
        
        let sort_start = std::time::Instant::now();
        
        sorted_vectors.sort_by(|a, b| {
            // Primary sort: most common metadata key
            if let Some(ref sort_key) = primary_sort_key {
                // Convert metadata to comparable format
                let a_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&a.metadata);
                let b_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&b.metadata);
                
                let a_value = a_map.get(sort_key).and_then(|v| v.as_str()).unwrap_or("");
                let b_value = b_map.get(sort_key).and_then(|v| v.as_str()).unwrap_or("");
                
                match a_value.cmp(&b_value) {
                    std::cmp::Ordering::Equal => {
                        // Secondary sort: vector ID for stable ordering
                        let empty_id = String::new();
                        let a_id = a.id.as_deref().unwrap_or(&empty_id);
                        let b_id = b.id.as_deref().unwrap_or(&empty_id);
                        a_id.cmp(b_id)
                    }
                    other => other,
                }
            } else {
                // Fallback: sort by vector ID only
                let empty_id = String::new();
                let a_id = a.id.as_deref().unwrap_or(&empty_id);
                let b_id = b.id.as_deref().unwrap_or(&empty_id);
                a_id.cmp(b_id)
            }
        });
        
        let sort_time_us = sort_start.elapsed().as_micros() as u64;
        
        // Calculate compression estimate based on metadata distribution
        let compression_estimate = if let Some(ref sort_key) = primary_sort_key {
            let distinct_values: std::collections::HashSet<String> = sorted_vectors
                .iter()
                .filter_map(|v| {
                    let metadata_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&v.metadata);
                    metadata_map.get(sort_key).and_then(|val| val.as_str()).map(|s| s.to_string())
                })
                .collect();
            
            // Lower cardinality = better compression
            1.0 - (distinct_values.len() as f64 / sorted_vectors.len() as f64)
        } else {
            0.05 // Small improvement from ID sorting
        };
        
        let stats = SortingStats {
            records_sorted: sorted_vectors.len(),
            sort_keys_used: if let Some(key) = primary_sort_key {
                vec![key, "vector_id".to_string()]
            } else {
                vec!["vector_id".to_string()]
            },
            compression_estimate,
            sort_time_us,
            ..Default::default()
        };
        
        debug!(
            "🎯 SST: Sorted {} vectors by metadata key for SSTable optimization",
            stats.records_sorted
        );
        
        Ok((sorted_vectors, stats))
    }

    /// Hash function for bloom filter
    fn hash_key(&self, key: &str, hash_num: u32) -> u32 {
        // Simple hash function - in production would use a proper hash function
        let mut hash = 5381u32;
        for byte in key.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
        }
        hash.wrapping_add(hash_num)
    }

    /// Organize records into blocks for better cache locality
    async fn organize_records_into_blocks(
        &self,
        records: &[SstRecord],
        block_size: usize,
    ) -> Result<Vec<DataBlock>> {
        let mut blocks = Vec::new();
        let mut current_block_records = Vec::new();
        let mut current_block_size = 0;
        let mut block_id = 0;

        for record in records {
            let record_size = std::mem::size_of::<SstRecord>() + 
                record.id.len() + 
                record.vector.len() * 4 + // f32 size
                record.metadata.iter().map(|item| item.key.len() + 50).sum::<usize>(); // Estimate metadata size (50 bytes per item)

            // If adding this record would exceed block size, finalize current block
            if current_block_size + record_size > block_size && !current_block_records.is_empty() {
                let records = std::mem::take(&mut current_block_records);
                blocks.push(DataBlock::new(block_id, records));
                block_id += 1;
                current_block_size = 0;
            }

            current_block_records.push(record.clone());
            current_block_size += record_size;
        }

        // Add final block if not empty
        if !current_block_records.is_empty() {
            blocks.push(DataBlock::new(block_id, current_block_records));
        }

        tracing::debug!(
            "📦 SST BLOCK ORGANIZATION: {} records organized into {} blocks (avg block size: {}KB)",
            records.len(),
            blocks.len(),
            if !blocks.is_empty() { current_block_size / blocks.len() / 1024 } else { 0 }
        );

        Ok(blocks)
    }

    /// Build optimized index and compress data blocks
    async fn build_optimized_index_and_compress_blocks(
        &self,
        data_blocks: &[DataBlock],
    ) -> Result<(Vec<IndexEntry>, Vec<Vec<u8>>)> {
        let mut index_entries = Vec::new();
        let mut compressed_blocks = Vec::new();
        
        // Create compression config from SST config
        let compression_config = DataBlockCompressionConfig::from_sst_config(&self.config);

        for block in data_blocks {
            // Use the new DataBlock serialization with compression
            let serialized_block = block.serialize_with_config(&compression_config)
                .map_err(|e| anyhow::anyhow!("Failed to serialize data block: {}", e))?;
            
            let final_data = serialized_block;
            
            // Determine if block was compressed
            let is_compressed = block.compression_algorithm != CompressionAlgorithmSst::None;

            // Create index entries for each record in this block using unified IndexEntry
            let mut block_offset = 0u32;
            for record in &block.records {
                index_entries.push(IndexEntry {
                    key: record.id.clone(),
                    offset: 0, // Will be set later with global offset
                    size: std::mem::size_of::<SstRecord>() as u32, // Approximate size
                    // Enhanced block organization fields
                    block_id: block.block_id,
                    block_offset,
                    compressed: is_compressed,
                    // Metadata statistics (empty for backward compatibility)
                    metadata_min_values: HashMap::new(),
                    metadata_max_values: HashMap::new(),
                    metadata_null_counts: HashMap::new(),
                });
                block_offset += std::mem::size_of::<SstRecord>() as u32;
            }

            compressed_blocks.push(final_data);
        }

        tracing::debug!(
            "🗜️ SST COMPRESSION: {} blocks processed, {} index entries created",
            data_blocks.len(),
            index_entries.len()
        );

        Ok((index_entries, compressed_blocks))
    }

    /// Simple block compression
    async fn compress_block_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Simple run-length encoding for demonstration
        // In production, would use proper compression like zstd or lz4
        let mut compressed = Vec::new();
        
        if data.is_empty() {
            return Ok(compressed);
        }

        let mut i = 0;
        while i < data.len() {
            let current_byte = data[i];
            let mut count = 1u8;
            
            // Count consecutive identical bytes
            while i + 1 < data.len() && data[i + 1] == current_byte && count < 255 {
                count += 1;
                i += 1;
            }
            
            // Store count and byte
            compressed.push(count);
            compressed.push(current_byte);
            i += 1;
        }

        Ok(compressed)
    }

    /// Convenient compact_collection method for CompactionCoordinator integration
    /// Returns enhanced result with vector tracking for AXIS integration
    /// Compact a specific collection - returns standard CompactionResult
    pub async fn compact_collection(&self, collection_id: &str, collection_config: Option<&Collection>) -> Result<CompactionResult> {
        info!("🗜️ SST Engine: Starting collection compaction for {}", collection_id);
        
        // Get storage location from collection config - skip if not provided
        let _storage_location = match collection_config
            .and_then(|c| c.storage_assignment.as_ref()) {
            Some(assignment) => assignment.base_location.clone(),
            None => {
                error!("❌ SST: No storage assignment found for collection {}. Skipping compaction!", collection_id);
                // Return early with failure result
                return Ok(CompactionResult {
                    success: false,
                    collections_affected: vec![collection_id.to_string()],
                    entries_processed: 0,
                    entries_removed: 0,
                    bytes_read: 0,
                    bytes_written: 0,
                    input_files: 0,
                    output_files: 0,
                    duration_ms: 0,
                    completed_at: Utc::now(),
                    engine_metrics: {
                        let mut metrics = HashMap::new();
                        metrics.insert("error".to_string(), 
                            serde_json::Value::String("Missing storage assignment".to_string()));
                        metrics
                    },
                });
            }
        };
        
        // Create compaction parameters
        let params = crate::storage::traits::CompactionParameters {
            collection_id: None,  // SST is collection-agnostic
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config: None,
        };
        
        // Use the consolidated do_compact implementation
        let mut result = self.do_compact(&params).await?;
        
        // Extract vector tracking data from engine_metrics
        let deleted_vector_ids = result.engine_metrics.get("deleted_vector_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
            )
            .unwrap_or_default();
            
        let _merged_vectors = result.engine_metrics.get("merged_vectors_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
            
        // Store vector tracking info in engine_metrics if needed
        if !deleted_vector_ids.is_empty() {
            result.engine_metrics.insert(
                "deleted_vector_ids".to_string(),
                serde_json::Value::Array(
                    deleted_vector_ids.into_iter()
                        .map(serde_json::Value::String)
                        .collect()
                ),
            );
        }
        
        Ok(result)
    }

}

// 🔴 OBSOLETE - Consolidated into storage::traits::CompactionResult
// This was never actually used - CompactionCoordinator now uses the unified CompactionResult
/*
/// Simplified compaction result for CompactionCoordinator
#[derive(Debug, Clone)]
pub struct EngineCompactionResult {
    pub files_processed: u64,
    pub bytes_processed: u64,
}
*/
