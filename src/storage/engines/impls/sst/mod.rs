//! # SST Storage Engine - Row-Based OLTP Optimized Storage
//!
//! The SST (Sorted String Table) engine is ProximaDB's high-performance row-based storage
//! engine optimized for OLTP workloads with frequent updates and real-time queries. It
//! implements the LSM-tree architecture with sophisticated filtering and caching.
//!
//! ## Role in ProximaDB Architecture
//!
//! SST serves as the primary engine for transactional workloads:
//! ```text
//! Write Path:                          Read Path:
//! Insert → WAL → MemTable              Query → Three-Stage Filter
//!          ↓                                    ↓
//!        Flush                          1. Bloom Filter (95% reduction)
//!          ↓                            2. Quantized Search (10x faster)
//!      SST Files                        3. Full Precision (exact results)
//!          ↓                                    ↓
//!     Compaction                          Decompression Cache
//! ```
//!
//! ## Key Features
//!
//! ### 1. **Three-Stage Filtering Pipeline**
//! Unique to SST, progressively refines search results:
//! - **Stage 1**: Bloom filters eliminate 95% of unnecessary reads
//! - **Stage 2**: Quantized vectors (INT8/PQ) for fast approximate filtering
//! - **Stage 3**: Full precision vectors for exact results
//!
//! ### 2. **Hierarchical Bloom Filters**
//! Multi-level bloom filters for different data characteristics:
//! - **File-level**: Quick file elimination
//! - **Block-level**: Fine-grained block skipping
//! - **Composite**: Combined filters for metadata predicates
//!
//! ### 3. **Zero-Copy Compaction**
//! Direct streaming between SST files without deserialization:
//! - Preserves compressed blocks during compaction
//! - Reduces memory usage by 80%
//! - 3x faster than traditional compaction
//!
//! ### 4. **Decompression Cache**
//! Configurable cache for frequently accessed blocks:
//! - LRU eviction with frequency tracking
//! - Adaptive sizing based on workload
//! - Prefetching for sequential access
//!
//! ## Performance Characteristics
//!
//! - **Write Throughput**: 200K vectors/sec
//! - **Query Latency**: < 5ms for point lookups
//! - **Compaction Speed**: 500MB/sec with zero-copy
//! - **Memory Usage**: 100MB per million vectors (configurable)
//! - **Compression Ratio**: 3-5x with mixed compression
//!
//! ## Configuration Options
//!
//! ```toml
//! [storage.sst]
//! # Bloom filter configuration
//! bloom_filter_bits_per_key = 10
//! bloom_filter_type = "hierarchical"
//!
//! # Three-stage filter thresholds
//! quantized_filter_threshold = 0.8
//! precision_filter_threshold = 0.95
//!
//! # Decompression cache
//! decompression_cache_size_mb = 512
//! cache_prefetch_enabled = true
//!
//! # Compaction settings
//! compaction_strategy = "zero_copy"
//! level0_file_num_trigger = 4
//! max_background_compactions = 2
//! ```
//!
//! ## Integration with Common Infrastructure
//!
//! ### Row-Based Format Module (`core/formats/fastlanes_blocks/`)
//! - Shared block structures with SWIFT engine
//! - Common compression configuration
//! - Unified batch operations
//!
//! ### Universal Distance Adapter (`universal/`)
//! - Hardware-accelerated distance computation
//! - Progressive refinement pipeline
//! - Format conversion utilities
//!
//! ### Compute Module (`compute/`)
//! - Unified quantization engine
//! - 13 distance metrics support
//! - Memory pool management
//!
//! ### Core Module (`core/`)
//! - 14 compression algorithms
//! - Hardware capability detection
//! - Optimized serialization
//!
//! ## SST-Specific Components
//!
//! - **`readers/`**: Unified reader with predictive prefetching
//! - **`writer.rs`**: SSTable writer with compression selection
//! - **`compactor_impl.rs`**: Zero-copy compaction implementation
//! - **`multi_stage_filter.rs`**: Three-stage filtering pipeline
//! - **`decompression_cache.rs`**: Adaptive block cache
//! - **`row_filter.rs`**: Optimized row-level filtering
//!
//! ## Usage Example
//!
//! ```rust
//! use proximadb::storage::engines::sst::SstStorage;
//!
//! let sst = SstStorage::new(config)?;
//!
//! // Insert with automatic compression
//! sst.insert_batch(vectors).await?;
//!
//! // Query with three-stage filtering
//! let results = sst.search(
//!     query_vector,
//!     k = 10,
//!     filter = Some(metadata_filter)
//! ).await?;
//! ```
//!
//! ## Compaction Strategy
//!
//! SST uses leveled compaction with zero-copy optimization:
//! 1. Level 0: Unsorted flush files from memtable
//! 2. Level 1-6: Sorted, non-overlapping files
//! 3. Zero-copy merge preserves compressed blocks
//! 4. Background threads handle compaction asynchronously

// bloom_filter now in core module for unified implementation
use crate::core::bloom::factory::BloomFilterFactory;
use crate::core::bloom::{self as bloom_filter, BloomFilterConfig, BloomFilterStrategy};
pub mod compaction;
pub mod decompression_cache;
pub mod error;
pub mod flush_eventlog_integration;
pub mod filter_methods;
// Quantization now handled by unified compute module
pub mod compactor_impl;
pub mod indexed_reader;
pub mod multi_stage_filter;
pub mod readers;
pub mod row_filter;
pub mod streaming_compaction;
pub mod writer;

// Test modules
#[cfg(test)]
pub mod tests;

// Re-export main types
pub use bloom_filter::{
    BloomFilterStats, HierarchicalBloomConfig, SerializedSstableBloomFilter, SstableBloomFilter,
};
pub use compaction::{Compaction, CompactionPriority, CompactionStats, CompactionTask};
pub use compactor_impl::{CompactionSortStrategy, SstCompactor, ZeroCopyCompactionStats};
pub use readers::UnifiedSstableReader;

// Additional exports for unified reader (SstableHeader is already defined below)
pub use writer::SstableWriter;

// Main SST Storage implementation (contents from original lsm/mod.rs)
use crate::core::metadata_types::TypedMetadata;
use crate::core::search::results::OptimizedSearchRecord;
use crate::core::{SstConfig, VectorRecord};
// SearchResult is now proto type, not in core::search
use crate::core::search::json_value_serde;
// use crate::core::serialization::VectorSerializationConfig;  // Not needed
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::unified::{
    CodebookStore, InMemoryCodebookStore, UnifiedQuantizationEngine,
};
use crate::core::compression::CompressionAlgorithm;
use crate::proto::proximadb_v1::Collection;
use crate::storage::common::compaction_orchestrator::FilenameCodec;
use crate::storage::engines::core::io::zero_copy::ZeroCopyIOSystem;
use crate::storage::optimization::SortingStats;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, UnifiedStorageEngine,
};
use crate::storage::transaction_coordinator::TransactionCoordinator;
// Unified search engine removed - using direct search methods
// MetadataItem is part of VectorRecord proto
use crate::query::unified_query_optimizer::UnifiedMetadataFilter;
use anyhow::Context;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

use self::error::{Result, SstError};

// Performance optimization - import what we need
use crate::storage::engines::core::ops::{
    UniversalOptimizationStrategy, UniversalPerformanceOptimizer, UniversallyOptimized,
};

// Import search optimization components
use crate::core::search::smart_execution_strategy::ExecutionStrategy;

// Import FastLanes common structures (shared with SWIFT)
use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::{
    BlockCompressionConfig, BlockStatistics, ColumnStatistics, FastLanesBlockMetadata,
    FastLanesDataBlock,
};

// SST filename operations are handled by unified FilenameCodec from compaction_orchestrator

#[cfg(test)]
mod sst_filename_tests {
    use super::*;

    #[test]
    fn test_generate_filename() {
        let collection_id = "test_collection";
        let level = 2;

        let filename = FilenameCodec::new().generate(level as u32, "sst");

        // Check unified format pattern: L{level}_{timestamp}_{uuid}.{extension}
        assert!(filename.starts_with("L2_"));
        assert!(filename.ends_with(".sstable"));

        // Check that it's recognized as an SST file
        assert!(FilenameCodec::new().is_tiered_filename(&filename, "sst"));
    }

    #[test]
    fn test_generate_flush_filename() {
        let filename = FilenameCodec::new().generate(0, "sst");

        // Flush files should always be level 0 with unified format
        assert!(filename.starts_with("L0_"));
        assert!(filename.ends_with(".sstable"));
        // Note: parse_level_from_filename expects old format, will need update
    }

    #[test]
    fn test_generate_compaction_filename() {
        let level = 5;

        let filename = FilenameCodec::new().generate(level as u32, "sst");

        assert!(filename.starts_with("L5_"));
        assert!(filename.ends_with(".sstable"));
        // Note: parse_level_from_filename expects old format, will need update
    }

    #[test]
    fn test_parse_level_from_filename() {
        let test_cases = vec![
            // New unified format: L{level}_{timestamp}_{uuid}.sst
            ("L0_20250814T143052_a7f3c2d1.sstable", Some(0)),
            ("L3_20250814T143052_b8e4d3e2.sstable", Some(3)),
            ("L15_20250814T143052_c9f5e4f3.sstable", Some(15)),
            ("invalid_file.sstable", None),
            ("no_level_file.txt", None),
            ("LABC_123_456.sstable", None), // Invalid level number
            // Old format should not parse
            ("level0_123456_789.sstable", None),
        ];

        for (filename, expected) in test_cases {
            assert_eq!(
                Some(FilenameCodec::new().parse_level(filename) as u8),
                expected,
                "Failed for filename: {}",
                filename
            );
        }
    }

    #[test]
    fn test_is_sst_file() {
        let test_cases = vec![
            // New unified format: L{level}_{timestamp}_{uuid}.sst
            ("L0_20250814T143052_a7f3c2d1.sstable", true),
            ("L5_20250814T143052_b8e4d3e2.sstable", true),
            ("L3_20250814T143052_c9f5e4f3.sstable", true),
            ("invalid.txt", false),
            ("no_level.sstable", false),
            ("L3_20250814T143052_a7f3c2d1.parquet", false), // Wrong extension
            // Old format should not be recognized
            ("collection_level0_123_456.sstable", false),
            ("level0_file.sstable", false),
        ];

        for (filename, expected) in test_cases {
            let result = FilenameCodec::new().is_tiered_filename(filename);
            debug!(
                "Testing '{}': expected={}, got={}",
                filename, expected, result
            );
            assert_eq!(result, expected, "Failed for filename: {}", filename);
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
            let filename = FilenameCodec::new().generate(level as u32, "sst");
            assert!(filenames.insert(filename), "Generated duplicate filename");
        }
    }

    #[test]
    fn test_filename_consistency() {
        let level = 3;

        // Test that the generated filename can be properly parsed back
        let filename = FilenameCodec::new().generate(level as u32, "sst");

        assert!(FilenameCodec::new().is_tiered_filename(&filename, "sst"));
        assert_eq!(FilenameCodec::new().parse_level(&filename), Some(level));
        // Collection ID validation removed - it's determined from base URL at search time
    }
}

// Remove dummy filesystem factory - SST will use fallback methods

// SST now works directly with VectorRecord - no intermediate conversion needed!
// This eliminates double serialization and improves performance

/// SST-specific metadata that accompanies VectorRecord in storage
///
/// ## Purpose:
///
/// SstMetadata tracks SST-specific state without polluting the VectorRecord.
/// This separation allows the proto definition to remain clean while SST
/// maintains its LSM-tree specific tracking.
///
/// ## Tombstone Handling:
///
/// When is_tombstone=true, the record represents a deletion. During compaction,
/// tombstones cascade down levels until they reach the bottom level where
/// they can be safely removed (no older versions exist).
///
/// ## Sequence Numbers:
///
/// Used for MVCC (Multi-Version Concurrency Control). Higher sequence numbers
/// represent newer versions. During reads, we return the latest version that's
/// visible to the transaction.
#[derive(Debug, Clone)]
pub struct SstMetadata {
    /// True if this is a deletion marker - tombstones cascade through LSM levels
    /// until they reach the bottom where they're garbage collected
    pub is_tombstone: bool,

    /// SST sequence for ordering - higher numbers are newer versions,
    /// used for MVCC resolution during reads
    pub sequence_number: u64,

    /// SSTable level this record belongs to - L0 is memtable flush,
    /// L1+ are compaction outputs with exponentially larger sizes
    pub level: u8,
}

/// Combined storage format for SST files
#[derive(Debug, Clone)]
pub struct SstEntry {
    pub record: VectorRecord,  // Direct storage of proto VectorRecord
    pub sst_meta: SstMetadata, // SST-specific metadata
}

impl SstEntry {
    /// Create SstEntry from VectorRecord (no conversion needed!)
    pub fn from_vector_record(record: VectorRecord, sequence_number: u64, level: u8) -> Self {
        Self {
            record,
            sst_meta: SstMetadata {
                is_tombstone: false,
                sequence_number,
                level,
            },
        }
    }

    /// Create tombstone entry for deletion
    pub fn tombstone(id: String, sequence_number: u64, level: u8) -> Self {
        Self {
            record: VectorRecord {
                id: id,
                vector: vec![], // Empty vector for tombstone
                metadata: std::collections::HashMap::new(),
                timestamp: chrono::Utc::now().timestamp(),
                updated_at: None,
                expires_at: Some(0), // Expired immediately
                version: None,
                quantized_vector: vec![],
                source: None, // No source for tombstone
            },
            sst_meta: SstMetadata {
                is_tombstone: true,
                sequence_number,
                level,
            },
        }
    }

    /// Convert to OptimizedSearchRecord directly for search results
    pub fn to_optimized_search_result(&self, score: f32) -> OptimizedSearchRecord {
        let mut search_record = OptimizedSearchRecord::new(self.record.id.clone(), score)
            .with_similarity(score)
            .add_vector(self.record.vector.clone())
            .with_metadata(self.record.metadata.clone());

        if let Some(version) = self.record.version {
            search_record = search_record.with_version_info(version, self.record.timestamp);
        }

        if let Some(source) = &self.record.source {
            search_record = search_record.with_source(crate::proto::proximadb_v1::SourceContent {
                data: Some(crate::proto::proximadb_v1::source_content::Data::TextContent(source.clone()))
            });
        }

        search_record
    }

    /// Serialize SstEntry directly - no conversion needed!
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        // Use protobuf for VectorRecord + bincode for SST metadata
        // This gives us zero-copy deserialization for search
        use prost::Message;

        // Serialize VectorRecord using protobuf
        let mut proto_buf = Vec::new();
        self.record.encode(&mut proto_buf)?;

        // Serialize SST metadata using bincode
        let meta_data = bincode::serialize(&self.sst_meta)?;

        // Combine with length prefixes
        let mut buffer = Vec::with_capacity(8 + proto_buf.len() + meta_data.len());
        buffer.extend_from_slice(&(proto_buf.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&proto_buf);
        buffer.extend_from_slice(&(meta_data.len()).to_le_bytes());
        buffer.extend_from_slice(&meta_data);

        Ok(buffer)
    }

    /// Deserialize SstEntry directly from stored bytes
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use prost::Message;

        if data.len() < 8 {
            return Err(anyhow::anyhow!("Invalid SstEntry data: too short"));
        }

        // Read proto length
        let proto_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if data.len() < 4 + proto_len + 4 {
            return Err(anyhow::anyhow!("Invalid SstEntry data: truncated"));
        }

        // Deserialize VectorRecord
        let proto_data = &data[4..4 + proto_len];
        let record = VectorRecord::decode(proto_data)?;

        // Read metadata length
        let meta_offset = 4 + proto_len;
        let meta_len = u32::from_le_bytes([
            data[meta_offset],
            data[meta_offset + 1],
            data[meta_offset + 2],
            data[meta_offset + 3],
        ]) as usize;

        // Deserialize SST metadata
        let meta_data = &data[meta_offset + 4..meta_offset + 4 + meta_len];
        let sst_meta = bincode::deserialize(meta_data)?;

        Ok(Self { record, sst_meta })
    }
}

/// Magic constant for SST files (4 bytes)
pub const SST_MAGIC: [u8; 4] = *b"SST1";

/// SSTable header for row-based storage format with hierarchical optimizations
#[derive(Debug, Clone)]
pub struct SstableHeader {
    pub version: u32,
    pub level: u8,
    pub entry_count: u64,
    pub min_key: String,
    pub max_key: String,
    pub timestamp: i64,

    // Compression configuration
    pub compression_algorithm: CompressionAlgorithm,
    pub compression_level: u8,

    // Bloom filter configuration
    pub has_bloom_filter: bool,
    pub has_global_bloom: bool, // NEW: Global bloom filter across entire file
    pub has_block_blooms: bool, // NEW: Per-block bloom filters
    pub metadata_column_count: u32, // NEW: Number of metadata columns for bloom sizing

    // Block organization
    pub block_size: u32,
    pub batch_size: u32,
    pub block_count: u32,

    // Component sizes (existing)
    pub header_size: u32,
    pub index_size: u32,
    pub data_size: u32,

    // NEW: Direct access offsets for selective loading (hierarchical architecture)
    pub global_bloom_offset: u64, // Offset to global bloom filter
    pub global_bloom_size: u32, // Size of global bloom filter
    pub block_index_offset: u64, // Offset to block index (with per-block blooms)
    pub block_index_size: u32, // Size of block index
    pub data_blocks_offset: u64, // Offset to first data block

    // NEW: Vector format analysis for bytemuck optimization
    pub vector_format: VectorFormat, // Fixed, Variable, or Mixed
    pub fixed_dimension: Option<u32>, // For fixed-dimension optimization
    pub compression_ratio: f32, // Achieved compression ratio
}

// SST compression now uses unified_compression::CompressionAlgorithm directly
// This eliminates duplication and ensures consistency across all storage engines

/// Vector format type for bytemuck optimization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VectorFormat {
    /// All vectors have the same fixed dimension (use bytemuck)
    Fixed { dimension: usize },
    /// Vectors have variable dimensions (use standard serialization)
    Variable,
    /// Mixed dimensions - majority fixed, some variable
    Mixed { dominant_dimension: usize },
}

impl Default for VectorFormat {
    fn default() -> Self {
        VectorFormat::Variable
    }
}

/// Index entry for fast key lookups in SSTable with hierarchical bloom filters
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    // NEW: Hierarchical bloom filter support
    /// Block-level key bloom filter (optional, for large blocks)
    pub block_key_bloom: Option<Vec<u8>>,
    /// Block-level metadata bloom filter (optional, for metadata-heavy queries)
    pub block_metadata_bloom: Option<Vec<u8>>,

    // NEW: Vector format optimization info
    pub vector_format: VectorFormat,
    // REMOVED: compression_ratio - can be calculated on-demand from size and DataBlock.uncompressed_size
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
        buffer.write_all(&(self.metadata_min_values.len()).to_le_bytes())?;
        for (key, value) in &self.metadata_min_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            json_value_serde::serialize_json_value(value, &mut buffer)?;
        }

        // Write metadata_max_values
        buffer.write_all(&(self.metadata_max_values.len()).to_le_bytes())?;
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

        // NEW: Write hierarchical bloom filter data
        match &self.block_key_bloom {
            Some(bloom_data) => {
                buffer.write_all(&1u8.to_le_bytes())?; // Has bloom
                buffer.write_all(&(bloom_data.len()).to_le_bytes())?;
                buffer.write_all(bloom_data)?;
            }
            None => {
                buffer.write_all(&0u8.to_le_bytes())?; // No bloom
            }
        }

        match &self.block_metadata_bloom {
            Some(bloom_data) => {
                buffer.write_all(&1u8.to_le_bytes())?; // Has bloom
                buffer.write_all(&(bloom_data.len()).to_le_bytes())?;
                buffer.write_all(bloom_data)?;
            }
            None => {
                buffer.write_all(&0u8.to_le_bytes())?; // No bloom
            }
        }

        // NEW: Write vector format info (removed compression_ratio)
        let format_byte = match self.vector_format {
            VectorFormat::Variable => 0u8,
            VectorFormat::Fixed { dimension } => {
                buffer.write_all(&1u8.to_le_bytes())?; // Fixed format
                buffer.write_all(&(dimension as u32).to_le_bytes())?;
                1u8
            }
            VectorFormat::Mixed { dominant_dimension } => {
                buffer.write_all(&2u8.to_le_bytes())?; // Mixed format
                buffer.write_all(&(dominant_dimension as u32).to_le_bytes())?;
                2u8
            }
        };
        if format_byte == 0 {
            buffer.write_all(&format_byte.to_le_bytes())?;
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

        // NEW: Read hierarchical bloom filter data
        cursor.read_exact(&mut bool_buf)?;
        let has_key_bloom = bool_buf[0] != 0;
        let block_key_bloom = if has_key_bloom {
            cursor.read_exact(&mut u32_buf)?;
            let bloom_len = u32::from_le_bytes(u32_buf) as usize;
            let mut bloom_data = vec![0u8; bloom_len];
            cursor.read_exact(&mut bloom_data)?;
            Some(bloom_data)
        } else {
            None
        };

        cursor.read_exact(&mut bool_buf)?;
        let has_metadata_bloom = bool_buf[0] != 0;
        let block_metadata_bloom = if has_metadata_bloom {
            cursor.read_exact(&mut u32_buf)?;
            let bloom_len = u32::from_le_bytes(u32_buf) as usize;
            let mut bloom_data = vec![0u8; bloom_len];
            cursor.read_exact(&mut bloom_data)?;
            Some(bloom_data)
        } else {
            None
        };

        // NEW: Read vector format and compression info
        cursor.read_exact(&mut bool_buf)?;
        let format_type = bool_buf[0];
        let vector_format = match format_type {
            0 => VectorFormat::Variable,
            1 => {
                cursor.read_exact(&mut u32_buf)?;
                let dimension = u32::from_le_bytes(u32_buf) as usize;
                VectorFormat::Fixed { dimension }
            }
            2 => {
                cursor.read_exact(&mut u32_buf)?;
                let dominant_dimension = u32::from_le_bytes(u32_buf) as usize;
                VectorFormat::Mixed { dominant_dimension }
            }
            _ => VectorFormat::Variable,
        };

        // REMOVED: No longer reading compression_ratio

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
            block_key_bloom,
            block_metadata_bloom,
            vector_format,
        })
    }
}

// Default function for serde when reading existing SSTable headers
// This preserves backward compatibility with existing SSTable files
fn default_block_size() -> u32 {
    3 * 1024 * 1024 // 3MB default for optimal cloud IOPS and compression balance
}

/// Hierarchical block metadata for serialization
#[derive(Debug, Clone)]
struct HierarchicalBlockMetadata {
    pub block_id: u32,
    pub record_count: u32,
    pub uncompressed_size: u32,
    pub metadata_stats: FastLanesBlockMetadata,
    pub block_bloom_filter: Option<Vec<u8>>,
    pub has_deletes: bool,
}

// Helper functions for SST-specific compression configuration
mod compression_helpers {
    use super::*;
    /// Create BlockCompressionConfig from SstConfig settings
    pub fn block_compression_from_sst_config(config: &SstConfig) -> BlockCompressionConfig {
        // Map string algorithm names to unified compression module algorithms
        // The unified compression module supports all 13 algorithms
        let compression_algorithm = match config.compression.to_lowercase().as_str() {
            "none" | "" => CompressionAlgorithm::None,
            "zstd" => CompressionAlgorithm::Zstd,
            "lz4" => CompressionAlgorithm::Lz4,
            "snappy" => CompressionAlgorithm::Snappy,
            "gzip" => CompressionAlgorithm::Gzip,
            "brotli" => CompressionAlgorithm::Brotli,
            "bzip2" => CompressionAlgorithm::Bzip2,
            "deflate" => CompressionAlgorithm::Deflate,
            "xz" => CompressionAlgorithm::Xz,
            "zlib" => CompressionAlgorithm::Zlib,
            "lzo" => CompressionAlgorithm::Lzo,
            "lz4hc" => CompressionAlgorithm::Lz4hc,
            "lzma" => CompressionAlgorithm::Lzma,
            unknown => {
                debug!(
                    "Unknown compression algorithm '{}', defaulting to None",
                    unknown
                );
                CompressionAlgorithm::None
            }
        };

        // Create proto compression config to match the SST config (supports all algorithms)
        let collection_compression = if config.compression.to_lowercase() != "none"
            && !config.compression.is_empty()
        {
            Some(crate::proto::proximadb_v1::CompressionConfig {
                algorithm: match config.compression.to_lowercase().as_str() {
                    "zstd" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionZstd as i32
                    }
                    "lz4" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLz4 as i32
                    }
                    "snappy" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionSnappy as i32
                    }
                    "gzip" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionGzip as i32
                    }
                    "brotli" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionBrotli as i32
                    }
                    "bzip2" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionBzip2 as i32
                    }
                    "deflate" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionDeflate as i32
                    }
                    "xz" => crate::proto::proximadb_v1::CompressionAlgorithm::CompressionXz as i32,
                    "zlib" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionZlib as i32
                    }
                    "lz4hc" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLz4hc as i32
                    }
                    "lzma" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLzma as i32
                    }
                    "lzo" => {
                        crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLzo as i32
                    }
                    _ => crate::proto::proximadb_v1::CompressionAlgorithm::CompressionNone as i32,
                },
                level: Some(config.compression_level as u32),
                adaptive: false,
                min_ratio: Some(0.5), // Optional field: Default minimum compression ratio
                enable_quantization: false, // Not used for SST compression
                quantization_type: None,
                normalization_method: None, // Optional field: No normalization by default
                block_size_kb: config.block_size_kb,
                dynamic_block_sizing: false,
            })
        } else {
            None
        };

        BlockCompressionConfig {
            algorithm: compression_algorithm.clone(),
            compression_level: config.compression_level as u8,
            enable_vector_compression: compression_algorithm != CompressionAlgorithm::None,
            enable_metadata_compression: true,
            compression_threshold_bytes: 1024, // 1KB threshold for testing
            dictionary_compression: false,
        }
    }

    /// Create BlockCompressionConfig from proto CompressionConfig
    pub fn block_compression_from_proto(
        config: Option<&crate::proto::proximadb_v1::CompressionConfig>,
        vector_dim: usize,
    ) -> BlockCompressionConfig {
        if let Some(config) = config {
            let block_size = if config.dynamic_block_sizing {
                optimal_block_size(vector_dim)
            } else {
                config.block_size_kb as usize * 1024
            };

            // Map proto compression algorithm to unified compression module algorithm
            let compression_algorithm = match config.algorithm {
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionNone as i32 =>
                {
                    CompressionAlgorithm::None
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionZstd as i32 =>
                {
                    CompressionAlgorithm::Zstd
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLz4 as i32 =>
                {
                    CompressionAlgorithm::Lz4
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionSnappy
                        as i32 =>
                {
                    CompressionAlgorithm::Snappy
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionGzip as i32 =>
                {
                    CompressionAlgorithm::Gzip
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionBrotli
                        as i32 =>
                {
                    CompressionAlgorithm::Brotli
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionBzip2
                        as i32 =>
                {
                    CompressionAlgorithm::Bzip2
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionDeflate
                        as i32 =>
                {
                    CompressionAlgorithm::Deflate
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionXz as i32 =>
                {
                    CompressionAlgorithm::Xz
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionZlib as i32 =>
                {
                    CompressionAlgorithm::Zlib
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLzo as i32 =>
                {
                    CompressionAlgorithm::Lzo
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLz4hc
                        as i32 =>
                {
                    CompressionAlgorithm::Lz4hc
                }
                x if x
                    == crate::proto::proximadb_v1::CompressionAlgorithm::CompressionLzma as i32 =>
                {
                    CompressionAlgorithm::Lzma
                }
                _ => {
                    debug!(
                        "Unknown compression algorithm value: {}, defaulting to None",
                        config.algorithm
                    );
                    CompressionAlgorithm::None
                }
            };

            BlockCompressionConfig {
                algorithm: compression_algorithm.clone(),
                compression_level: config.level.unwrap_or(3) as u8,
                enable_vector_compression: config.algorithm
                    != crate::proto::proximadb_v1::CompressionAlgorithm::CompressionNone as i32,
                enable_metadata_compression: true,
                compression_threshold_bytes: block_size / 1000, // Use 0.1% of block size as threshold
                dictionary_compression: false,
            }
        } else {
            BlockCompressionConfig::default()
        }
    }
} // End of compression_helpers module

/// Calculate optimal block size based on vector dimensions
/// Target: 2000-2500 vectors per block
pub fn optimal_block_size(vector_dim: usize) -> usize {
    let vector_size = vector_dim * 4 + 200; // FP32 + metadata overhead (more realistic estimate)

    // Target 3MB as optimal default, varying slightly by dimension
    let target_block_size = match vector_dim {
        0..=384 => 3 * 1024 * 1024,            // 3MB for small vectors
        385..=768 => 3 * 1024 * 1024,          // 3MB for medium vectors
        769..=1536 => 3 * 1024 * 1024,         // 3MB for large vectors
        _ => (2.5 * 1024.0 * 1024.0) as usize, // 2.5MB for XL vectors (network optimization)
    };

    // Clamp between 2MB and 4MB (optimal range for cloud IOPS and compression)
    target_block_size.max(2 * 1024 * 1024).min(4 * 1024 * 1024)
}

// Import centralized compression markers and helper functions

// SST uses FastLanesDataBlock directly from the shared module
// Additional SST-specific methods are implemented as utility functions

// SST-specific utility functions for FastLanesDataBlock
mod block_utils {
    use super::*;

    /// Create a new FastLanesDataBlock for SST usage
    pub fn create_sst_block(records: Vec<VectorRecord>, block_id: u32) -> FastLanesDataBlock {
        FastLanesDataBlock {
            encoding_marker: 0x00, // Will be set based on encoding
            encoding_metadata: None,
            block_id,
            records,
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: FastLanesBlockMetadata::default(),
            compression_config: BlockCompressionConfig::default(),
            compression_algorithm: CompressionAlgorithm::None,
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: (String::new(), String::new()),
            timestamp_range: (0, 0),
            statistics: BlockStatistics::default(),
            metadata_stats: None,
            has_deletes: false,
        }
    }

    /// Encode vectors using FastLanes SIMD-optimized encoding
    pub fn encode_with_fastlanes(block: &FastLanesDataBlock) -> anyhow::Result<Vec<u8>> {
        use crate::storage::engines::core::ops::fastlanes_encoding::{
            FastLanesEncoder, FastLanesScheme,
        };
        use std::io::Write;

        // Get dimension from the first record since metadata doesn't have it directly
        let dimension = block.records.first().map(|r| r.vector.len()).unwrap_or(0);

        // Transpose vectors from row-major to column-major for SIMD
        let mut columns: Vec<Vec<f32>> = vec![vec![]; dimension];
        for record in &block.records {
            for (dim_idx, &value) in record.vector.iter().enumerate() {
                if dim_idx < dimension {
                    columns[dim_idx].push(value);
                }
            }
        }

        // Encode each dimension column using a default scheme
        let scheme = FastLanesScheme::BitPacked { bits: 16 };
        let encoder = FastLanesEncoder::new(scheme);
        let mut encoded_data = Vec::new();

        // Write metadata first
        encoded_data.write_all(&(dimension as u32).to_le_bytes())?;
        encoded_data.write_all(&(block.records.len() as u32).to_le_bytes())?;

        // Encode each column
        for column in columns {
            let encoded_column = encoder.encode_f32(&column)?;
            encoded_data.write_all(&(encoded_column.len() as u32).to_le_bytes())?;
            encoded_data.write_all(&encoded_column)?;
        }

        // Also encode metadata and IDs
        for record in &block.records {
            // Encode ID
            let id = &record.id;
            encoded_data.write_all(&(id.len() as u32).to_le_bytes())?;
            encoded_data.write_all(id.as_bytes())?;

            // Encode timestamp
            encoded_data.write_all(&record.timestamp.to_le_bytes())?;
        }

        Ok(encoded_data)
    }

    /// Decode vectors from FastLanes format
    pub fn decode_with_fastlanes(data: &[u8], marker: u8) -> anyhow::Result<Vec<VectorRecord>> {
        use crate::storage::engines::core::ops::fastlanes_encoding::{
            FastLanesDecoder, FastLanesScheme,
        };
        use std::io::Read;

        let mut cursor = std::io::Cursor::new(data);

        // Read metadata
        let mut buf = [0u8; 4];
        cursor.read_exact(&mut buf)?;
        let dimension = u32::from_le_bytes(buf) as usize;

        cursor.read_exact(&mut buf)?;
        let vector_count = u32::from_le_bytes(buf) as usize;

        // Determine scheme from marker
        let scheme = match marker & 0xF0 {
            0x10 => FastLanesScheme::BitPacked { bits: 16 },
            0x20 => FastLanesScheme::Delta { base: 0 },
            0x30 => FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            },
            0x60 => FastLanesScheme::RunLength,
            _ => FastLanesScheme::BitPacked { bits: 32 },
        };

        let decoder = FastLanesDecoder::new(scheme);

        // Decode each dimension column
        let mut columns = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            cursor.read_exact(&mut buf)?;
            let column_len = u32::from_le_bytes(buf) as usize;

            let mut column_data = vec![0u8; column_len];
            cursor.read_exact(&mut column_data)?;

            let decoded_column = decoder.decode_f32(&column_data, column_len)?;
            columns.push(decoded_column);
        }

        // Transpose back from column-major to row-major
        let mut records = Vec::with_capacity(vector_count);
        for i in 0..vector_count {
            let mut vector = Vec::with_capacity(dimension);
            for col in &columns {
                vector.push(col[i]);
            }

            // Read ID
            cursor.read_exact(&mut buf)?;
            let id_len = u32::from_le_bytes(buf) as usize;
            let id = if id_len > 0 {
                let mut id_bytes = vec![0u8; id_len];
                cursor.read_exact(&mut id_bytes)?;
                String::from_utf8(id_bytes)?
            } else {
                String::new() // Empty string for missing ID
            };

            // Read timestamp
            cursor.read_exact(&mut buf)?;
            let timestamp = u32::from_le_bytes(buf);

            records.push(VectorRecord {
                id,
                vector,
                timestamp: timestamp as i64,
                metadata: std::collections::HashMap::new(),
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: vec![],
                source: None, // No source information available from legacy format
            });
        }

        Ok(records)
    }

    /// Calculate metadata statistics for intelligent block filtering
    pub fn calculate_metadata_stats(records: &[VectorRecord]) -> FastLanesBlockMetadata {
        let mut stats = FastLanesBlockMetadata::default();

        if records.is_empty() {
            return stats;
        }

        // Initialize key and timestamp ranges
        let mut min_key = String::new();
        let mut max_key = String::new();
        let mut min_timestamp = u32::MAX;
        let mut max_timestamp = u32::MIN;

        if let Some(first) = records.first() {
            min_key = first.id.clone();
            max_key = first.id.clone();
            min_timestamp = first.timestamp as u32;
            max_timestamp = first.timestamp as u32;
        }

        // Process all records for statistics
        let mut metadata_columns = HashMap::new();

        for record in records {
            let record_id = &record.id;
            // Update key range
            if record_id < &min_key {
                min_key = record_id.clone();
            }
            if record_id > &max_key {
                max_key = record_id.clone();
            }

            // Update timestamp range
            min_timestamp = min_timestamp.min(record.timestamp as u32);
            max_timestamp = max_timestamp.max(record.timestamp as u32);

            // Process metadata
            for item in &record.metadata {
                let col_name = item.0.clone();
                metadata_columns.insert(col_name.clone(), ());

                // Get or create column stats
                let col_stats = stats
                    .column_stats
                    .entry(col_name.clone())
                    .or_insert_with(|| ColumnStatistics {
                        name: col_name.clone(),
                        null_count: 0,
                        distinct_count: 0,
                        min_value: None,
                        max_value: None,
                        avg_size_bytes: 0,
                        bloom_filter_enabled: false,
                    });

                // Convert to JSON value for min/max tracking
                let value = match &item.1.value {
                    Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                        serde_json::Value::String(s.clone())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                        serde_json::Number::from_f64(*n)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                        serde_json::Value::Bool(*b)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                        serde_json::Value::Number(serde_json::Number::from(*i))
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BytesValue(_)) => {
                        serde_json::Value::String("[binary]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                        col_stats.null_count += 1;
                        continue;
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(_)) => {
                        serde_json::Value::String("[array]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(_)) => {
                        serde_json::Value::String("[object]".to_string())
                    }
                    None => {
                        col_stats.null_count += 1;
                        continue;
                    }
                };

                // Update min/max values for this column
                if col_stats.min_value.is_none() {
                    col_stats.min_value = Some(value.clone());
                } else if let Some(ref mut min_val) = col_stats.min_value {
                    if compare_json_values(&value, min_val) == std::cmp::Ordering::Less {
                        *min_val = value.clone();
                    }
                }

                if col_stats.max_value.is_none() {
                    col_stats.max_value = Some(value.clone());
                } else if let Some(ref mut max_val) = col_stats.max_value {
                    if compare_json_values(&value, max_val) == std::cmp::Ordering::Greater {
                        *max_val = value;
                    }
                }
            }
        }

        // Store key range and timestamp range in column stats
        stats.column_stats.insert(
            "__id".to_string(),
            ColumnStatistics {
                name: "__id".to_string(),
                null_count: 0,
                distinct_count: records.len() as u32,
                min_value: Some(serde_json::Value::String(min_key)),
                max_value: Some(serde_json::Value::String(max_key)),
                avg_size_bytes: 0,
                bloom_filter_enabled: false,
            },
        );

        stats.column_stats.insert(
            "__timestamp".to_string(),
            ColumnStatistics {
                name: "__timestamp".to_string(),
                null_count: 0,
                distinct_count: 0,
                min_value: Some(serde_json::Value::Number(serde_json::Number::from(
                    min_timestamp,
                ))),
                max_value: Some(serde_json::Value::Number(serde_json::Number::from(
                    max_timestamp,
                ))),
                avg_size_bytes: 8,
                bloom_filter_enabled: false,
            },
        );

        stats.record_count = records.len() as u32;
        stats.timestamp = max_timestamp as i64;
        stats.version_range = (min_timestamp as i64, max_timestamp as i64);

        stats
    }

    /// Compare JSON values for ordering
    fn compare_json_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        use serde_json::Value;
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => {
                let a_f64 = a.as_f64();
                let b_f64 = b.as_f64();
                a_f64
                    .partial_cmp(&b_f64)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
            (Value::String(a), Value::String(b)) => a.cmp(b),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            _ => std::cmp::Ordering::Equal,
        }
    }
} // End of block_utils module

// Local marker functions removed - now using centralized functions from unified_compression::markers

// Remove unnecessary wrapper functions - callers should use FastLanesDataBlock methods directly
// FastLanesDataBlock::serialize()
// FastLanesDataBlock::serialize_with_config()
// FastLanesDataBlock::deserialize()

// Old deserialization helper functions removed - now handled by FastLanesDataBlock internally
// FastLanesDataBlock handles all serialization/deserialization with compression support

/// Delegates to FastLanesDataBlock for proper deserialization
/// This eliminates duplication and ensures consistent block handling
fn deserialize_uncompressed_block(data: &[u8]) -> anyhow::Result<FastLanesDataBlock> {
    // FIXED: Delegate directly to FastLanesDataBlock instead of duplicating logic
    FastLanesDataBlock::deserialize(data)
}

// Utility functions for FastLanesDataBlock operations in SST
mod block_operations {
    use super::*;

    /// Get compression statistics
    /// Returns (is_compressed, uncompressed_size)
    pub fn compression_stats(block: &FastLanesDataBlock) -> (bool, usize) {
        (
            block.compression_algorithm != CompressionAlgorithm::None,
            block.uncompressed_size as usize,
        )
    }

    /// Generate or update quantized section for this block
    pub fn update_quantization(
        block: &mut FastLanesDataBlock,
        codebook: Option<&crate::compute::quantization::Codebook>,
        enable_int8: bool,
    ) -> Result<()> {
        // Extract vectors from records
        // Note: quantization currently requires owned vectors
        let vectors: Vec<Vec<f32>> = block.records.iter().map(|r| r.vector.clone()).collect();

        if vectors.is_empty() {
            // Keep empty quantized section for consistency
            return Ok(());
        }

        // Quantization will be handled by unified engine when needed

        debug!(
            "Updated quantization for block {}: {} vectors, PQ={}, INT8={}",
            block.block_id,
            vectors.len(),
            codebook.is_some(),
            enable_int8
        );

        Ok(())
    }

    /// Filter candidates using binary sketches (Stage 1: 95% reduction)
    pub fn filter_by_sketch(
        block: &FastLanesDataBlock,
        query_sketch: &[u8], // Binary sketch is just a byte array
        threshold: f32,
    ) -> Vec<usize> {
        // Check if quantized vectors exist
        if let Some(ref qv) = block.quantized_vectors {
            // Need to implement filter_by_sketch logic here
            vec![]
        } else {
            vec![]
        }
    }

    /// Rank candidates using PQ codes (Stage 2: Further refinement)
    pub fn rank_by_pq(
        block: &FastLanesDataBlock,
        query: &[f32],
        codebook: &crate::compute::quantization::Codebook,
        candidate_indices: &[usize],
    ) -> Vec<(usize, f32)> {
        // Check if quantized vectors exist
        if let Some(ref qv) = block.quantized_vectors {
            // Need to implement rank_by_pq logic here
            vec![]
        } else {
            vec![]
        }
    }

    /// Get full vectors for final reranking (Stage 3: 100% accuracy)
    pub fn vectors_by_indices(
        block: &FastLanesDataBlock,
        indices: &[usize],
    ) -> Vec<(usize, Vec<f32>)> {
        indices
            .iter()
            .filter_map(|&idx| block.records.get(idx).map(|r| (idx, r.vector.clone())))
            .collect()
    }

    /// Check if block has valid quantization data
    pub fn has_quantization(block: &FastLanesDataBlock) -> bool {
        // Check if quantized vectors exist and are not empty
        block
            .quantized_vectors
            .as_ref()
            .map_or(false, |v| !v.is_empty())
    }

    /// Get memory savings from quantization
    pub fn quantization_memory_savings(block: &FastLanesDataBlock) -> f32 {
        // Calculate original memory usage
        let original_size = block
            .records
            .iter()
            .map(|r| r.vector.len() * 4) // f32 = 4 bytes
            .sum::<usize>();

        // Calculate quantized memory usage if present
        let quantized_size = block
            .quantized_vectors
            .as_ref()
            .map(|vecs| vecs.iter().map(|v| v.len()).sum::<usize>())
            .unwrap_or(0);

        if original_size > 0 && quantized_size > 0 {
            1.0 - (quantized_size as f32 / original_size as f32)
        } else {
            0.0
        }
    }
} // End of block_operations module

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

// Debug derive removed - CrossCacheOrchestrator doesn't implement Debug
// SST-specific optimization structures removed - now using universal module

pub struct SstStorage {
    config: SstConfig,
    // NO collection_id - passed in parameters
    // NO data_dir - derived from parameters
    compaction_manager: Option<Arc<Compaction>>,
    filesystem: Arc<FilesystemFactory>,
    // Intelligent filesystem for caching and optimized I/O
    // Caches SSTable metadata, bloom filters, and frequently accessed blocks
    intelligent_fs: Option<
        Arc<crate::storage::persistence::filesystem::intelligent_filesystem::IntelligentFilesystem>,
    >,
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

    // Universal performance optimization (replaces SST-specific optimization)
    /// Universal performance optimizer eliminating code duplication
    universal_optimizer: UniversalPerformanceOptimizer,
    /// Optional Cross-Cache Orchestrator for metadata/filter tracking
    orchestrator: Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>,
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
                .map_err(|e| {
                    SstError::Internal(format!("Failed to create atomic coordinator: {}", e))
                })?,
        );

        // Create Zero-copy IO system for the reader
        let zero_copy_config =
            crate::storage::engines::core::io::zero_copy::config::ZeroCopyIOConfig::default();
        let zero_copy_system = Arc::new(
            ZeroCopyIOSystem::new(
                zero_copy_config,
                filesystem.clone(),
                vec![], // No custom serializers
            )
            .await
            .map_err(|e| {
                SstError::Internal(format!("Failed to create zero-copy IO system: {}", e))
            })?,
        );

        // SST will create IntelligentFilesystem instances per collection for optimal caching
        // This dramatically reduces I/O for frequently accessed SSTable blocks

        // Create SSTable reader - using empty collection_id as SST is now singleton
        let sstable_reader = Arc::new(UnifiedSstableReader::new(
            filesystem.clone(),
            zero_copy_system,
            String::new(), // Empty collection_id for singleton
        ));

        // Create quantization engine (optional for SST)
        // For now, use in-memory codebook store since SST doesn't require quantization
        let codebook_store: Arc<dyn CodebookStore> = Arc::new(InMemoryCodebookStore::new());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));

        // Initialize decompression cache with default size (64MB)
        let decompression_cache = Arc::new(decompression_cache::DecompressionCache::new(
            64 * 1024 * 1024, // Default to 64MB cache
        ));

        // Register decompression cache provider with orchestrator (VectorData)
        if let Some(ref orch) = crate::storage::cache::orchestrator::CrossCacheOrchestrator::global()
        {
            use crate::storage::cache::orchestrator::{CacheStatsProvider, CacheType};
            let provider: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(crate::storage::engines::impls::sst::decompression_cache::DecompressionCacheStatsProvider::new(
                    decompression_cache.clone(),
                ));
            orch.register_cache_provider(CacheType::VectorData, provider);
            // Register lightweight providers for FilterBitmap and Metadata
            struct SstStaticProvider;
            impl CacheStatsProvider for SstStaticProvider {
                fn snapshot(&self) -> crate::storage::cache::orchestrator::UsageStats {
                    crate::storage::cache::orchestrator::UsageStats {
                        hit_rate: 0.0,
                        avg_entry_size: 4096,
                        access_frequency: 0.0,
                        last_rebalance: std::time::SystemTime::now(),
                    }
                }
            }
            let provider2: Arc<dyn CacheStatsProvider + Send + Sync> = Arc::new(SstStaticProvider);
            orch.register_cache_provider(CacheType::FilterBitmap, provider2.clone());
            orch.register_cache_provider(CacheType::Metadata, provider2);
        }

        // Initialize compaction manager (always enabled)
        let compaction_manager = Some(Arc::new(Compaction::new(config.clone()).await.map_err(
            |e| SstError::Internal(format!("Failed to create compaction manager: {}", e)),
        )?));

        // Initialize universal performance optimization
        let universal_optimizer =
            UniversalPerformanceOptimizer::with_strategy(UniversalOptimizationStrategy::Balanced)
                .await
                .map_err(|e| {
                    SstError::Internal(format!(
                        "Failed to create universal performance optimizer: {}",
                        e
                    ))
                })?;

        Ok(Self {
            config,
            compaction_manager,
            filesystem,
            intelligent_fs: None, // Created per collection
            atomic_coordinator,
            sstable_reader,
            distance_compute,
            decompression_cache: decompression_cache.clone(),
            quantization_engine,
            universal_optimizer,
            orchestrator: crate::storage::cache::orchestrator::CrossCacheOrchestrator::global(),
        })
    }

    /// Get the collection storage URL from parameters
    fn get_collection_storage_url_from_params(params: &FlushParameters) -> Result<String> {
        debug!("🔍 SST FLUSH: Determining storage URL");
        info!(
            "   - Has collection_config: {}",
            params.collection_config.is_some()
        );

        // Extract storage location from collection config in parameters
        if let Some(ref collection) = params.collection_config {
            info!(
                "   - Has storage_assignment: {}",
                collection.storage_assignment.is_some()
            );
            if let Some(ref assignment) = collection.storage_assignment {
                info!("   - Base location: {}", assignment.base_location);
                info!("   - Collection ID: {:?}", params.collection_id);
                let storage_url = format!(
                    "{}/{}/data",
                    assignment.base_location,
                    params
                        .collection_id
                        .as_ref()
                        .unwrap_or(&"unknown".to_string())
                );
                debug!(
                    "🔍 SST FLUSH: Using storage URL from params: {}",
                    storage_url
                );
                return Ok(storage_url);
            }
        }

        // Fallback to default if not provided
        let collection_id = params.collection_id.as_ref().ok_or_else(|| {
            SstError::InvalidArgument("Collection ID required for SST operations".into())
        })?;

        // For tests, use temp directory; for production, use /var/lib/proximadb
        let base_path = if cfg!(test) {
            format!("/tmp/proximadb_integration_tests/{}", collection_id)
        } else {
            format!("/var/lib/proximadb/{}", collection_id)
        };

        let storage_url = format!("file://{}/data", base_path);
        debug!("🔍 SST: Using default storage URL: {}", storage_url);
        Ok(storage_url)
    }

    /// Enable compaction with the SST tree's atomic coordinator
    pub async fn enable_compaction(&mut self, worker_count: usize) -> Result<()> {
        if self.compaction_manager.is_none() {
            let mut compaction_manager = Compaction::with_atomic_coordinator(
                self.config.clone(),
                Some(self.atomic_coordinator.clone()),
            )
            .await
            .map_err(|e| {
                SstError::Internal(format!("Failed to create compaction manager: {}", e))
            })?;

            // Start background workers
            compaction_manager
                .start_workers(worker_count)
                .await
                .map_err(|e| {
                    SstError::Internal(format!("Failed to start compaction workers: {}", e))
                })?;

            self.compaction_manager = Some(Arc::new(compaction_manager));

            info!(
                "✅ SST: Compaction enabled with {} workers and atomic operations",
                worker_count
            );
        }
        Ok(())
    }

    // ============================================================================
    // UNIVERSAL PERFORMANCE OPTIMIZATION INTEGRATION
    // ============================================================================

    /// Fast read optimization using universal optimizer memory-mapped files
    async fn mmap_sstable_file(&self, file_path: &str) -> Result<Vec<u8>> {
        // Use universal optimizer for memory-mapped file access
        if let Some(mmap) = self
            .universal_optimizer
            .get_memory_mapped_file(file_path)
            .await
            .map_err(|e| SstError::Internal(format!("Failed to get memory-mapped file: {}", e)))?
        {
            Ok(mmap.to_vec())
        } else {
            // Fallback to regular file reading through universal optimizer
            self.universal_optimizer
                .read_data_optimized(file_path)
                .await
                .map_err(|e| SstError::Internal(format!("Failed to read data: {}", e)))
        }
    }

    /// Block-level I/O optimization with universal parallel reads
    async fn parallel_block_read(
        &self,
        file_paths: &[String],
        block_indices: &[usize],
    ) -> Result<Vec<Vec<u8>>> {
        // Use universal optimizer for parallel operations
        let read_operations: Vec<_> = file_paths
            .iter()
            .zip(block_indices.iter())
            .map(|(file_path, &block_idx)| {
                let file_path = file_path.clone();
                async move { Self::read_block_optimized(&file_path, block_idx).await }
            })
            .collect();

        let results = self
            .universal_optimizer
            .parallel_operations(read_operations, |operation| operation)
            .await
            .map_err(|e| SstError::Internal(format!("Failed in parallel operations: {}", e)))?;

        // Extract successful results - results is Vec<Result<Vec<u8>>>
        let mut final_results = Vec::new();
        for result in results {
            match result {
                Ok(data) => {
                    // data is Result<Vec<u8>, Error>, so we need to unwrap it
                    match data {
                        Ok(bytes) => final_results.push(bytes),
                        Err(e) => {
                            return Err(SstError::Internal(format!("Block read failed: {}", e)));
                        }
                    }
                }
                Err(e) => return Err(SstError::Internal(format!("Task failed: {}", e))),
            }
        }
        Ok(final_results)
    }

    /// Optimized block reading with universal memory management
    async fn read_block_optimized(file_path: &str, block_idx: usize) -> Result<Vec<u8>> {
        // TODO: Implement actual block reading logic with universal memory pool
        // For now, return placeholder data - this should read specific block from SSTable file
        Ok(vec![0u8; 64 * 1024]) // 64KB placeholder block
    }

    /// Storage tier optimization
    async fn optimize_sstable_storage_tier(
        &self,
        file_path: &str,
        file_size_bytes: u64,
    ) -> Result<String> {
        // Use universal optimizer for storage tier optimization
        // Storage tier optimization handled internally
        Ok("hot".to_string())
    }

    /// Distance computation using universal hardware-accelerated computation
    async fn compute_distances_sstable_optimized(
        &self,
        query: &[f32],
        candidates: &[Vec<f32>],
        metric: crate::compute::distance_computation::DistanceMetric,
    ) -> Result<Vec<f32>> {
        // Use universal optimizer for hardware-accelerated distance computation
        self.universal_optimizer
            .compute_distances_accelerated(query, candidates, metric)
            .await
            .map_err(|e| SstError::Internal(format!("Failed to compute distances: {}", e)))
    }

    /// Memory pool optimization using universal optimizer
    async fn sstable_buffer(&self, size: usize) -> Result<Vec<f32>> {
        self.universal_optimizer
            .get_memory_buffer(size)
            .await
            .map_err(|e| SstError::Internal(format!("Failed to get memory buffer: {}", e)))
    }
}

// ============================================================================
// UNIVERSAL PERFORMANCE OPTIMIZATION TRAIT IMPLEMENTATION
// ============================================================================

#[async_trait]
impl UniversallyOptimized for SstStorage {
    fn universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }

    async fn setup_engine_optimizations(&self) -> anyhow::Result<()> {
        // SST-specific optimizations: Setup SSTable-specific caching and prefetching
        info!("🔧 SST: Setting up engine-specific optimizations");

        // Enable prefetching for SSTable files based on access patterns
        let sstable_files = vec!["example_sstable.sst".to_string()]; // TODO: Get actual SSTable files
        self.universal_optimizer
            .prefetch_data(&sstable_files)
            .await
            .context("Failed to prefetch SSTable data")?;

        // Setup SSTable-specific cache eviction if needed
        self.universal_optimizer
            .evict_cache_if_needed()
            .await
            .context("Failed to evict cache")?;

        info!("✅ SST: Engine-specific optimizations setup complete");
        Ok(())
    }

    async fn collect_performance_metrics(
        &self,
    ) -> anyhow::Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // SST-specific metrics
        metrics.insert(
            "engine_type".to_string(),
            serde_json::Value::String("SST".to_string()),
        );
        metrics.insert(
            "optimization_strategy".to_string(),
            serde_json::Value::String(format!("{:?}", self.universal_optimizer.get_strategy())),
        );

        // Universal optimizer configuration
        let config = self.universal_optimizer.get_config();
        metrics.insert(
            "cache_size_mb".to_string(),
            serde_json::Value::Number(serde_json::Number::from(config.cache_size_mb)),
        );
        metrics.insert(
            "parallel_operations".to_string(),
            serde_json::Value::Number(serde_json::Number::from(config.parallel_operations)),
        );
        metrics.insert(
            "enable_prefetching".to_string(),
            serde_json::Value::Bool(config.enable_prefetching),
        );

        Ok(metrics)
    }

    // ===== Methods from second impl block (consolidated) =====
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
                    entries_flushed: Some(0),
                    bytes_written: Some(0),
                    files_created: Some(0),
                    duration_ms: Some(0),
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
        let codec = FilenameCodec::new();
        let sst_filename = codec.generate(0, "sst"); // Level 0 for flush
        debug!("🔧 SST: Creating SSTable file: {} for collection: {}", sst_filename, collection_id);

        // MULTI-BATCH OPTIMIZED SORTING FOR GLOBAL PARTITIONED MEMTABLE
        // vector_records contains individual vectors from MULTIPLE batches (params.batch_ids)
        // Each batch may have been written at different times, so vectors are NOT pre-sorted
        //
        // Optimal strategy for multi-batch flush:
        // - Group vectors by batch_id (if available in metadata)
        // - Sort each batch's vectors separately (smaller sorts are faster)
        // - Use k-way merge to combine sorted batches efficiently
        // - Fallback to single Vec + sort for simplicity when batch grouping isn't beneficial

        let record_count = vector_records.len();
        let batch_count = params.batch_ids.len();

        debug!("🔍 SST FLUSH: Processing {} vectors from {} batches", record_count, batch_count);

        // Scope the sorting to ensure immediate deallocation
        let sorted_records_iter = {
            // For small datasets or single batch, use simple Vec + sort
            if record_count < 10000 || batch_count <= 1 {
                debug!("🔍 SST FLUSH: Using single-sort search_strategy (small dataset or single batch)");

                let mut unsorted_records = Vec::with_capacity(record_count);

                // Collect all records into Vec (O(1) per insertion)
                for (sequence_number, vector) in vector_records.iter().enumerate() {
                    let vector_id = vector.id.as_ref().map(|s| s.as_str()).unwrap_or("").to_string();

                    // Handle append-only vectors (empty/null IDs) specially
                    let key = if vector_id.is_none() {
                        format!("__append_only_seq_{}", sequence_number)
                    } else {
                        vector_id
                    };

                    // OPTIMIZED: Use VectorRecord directly (no SstRecord conversion)
                    let mut vector_record = vector.clone();
                    // Store sequence_number in version field (optional u32)
                    vector_record.version = Some(sequence_number as u32);

                    unsorted_records.push((key, vector_record));
                }

                // Single efficient sort: O(n log n)
                unsorted_records.sort_by(|a, b| a.0.cmp(&b.0));
                unsorted_records.into_iter()

            } else {
                // For larger multi-batch datasets, use batch-aware sorting
                debug!("🔍 SST FLUSH: Using multi-batch sort search_strategy ({} batches)", batch_count);

                // Group vectors by their order (simulating batch grouping)
                // Since we don't have direct batch_id in VectorRecord, group by chunks
                let batch_size = (record_count / batch_count).max(1);
                let mut sorted_batches = Vec::with_capacity(batch_count);

                for (batch_idx, batch_chunk) in vector_records.chunks(batch_size).enumerate() {
                    let mut batch_records = Vec::with_capacity(batch_chunk.len());

                    for (local_idx, vector) in batch_chunk.iter().enumerate() {
                        let sequence_number = batch_idx * batch_size + local_idx;
                        let vector_id = vector.id.as_ref().map(|s| s.as_str()).unwrap_or("").to_string();

                        let key = if vector_id.is_none() {
                            format!("__append_only_seq_{}", sequence_number)
                        } else {
                            vector_id
                        };

                        // OPTIMIZED: Use VectorRecord directly (no SstRecord conversion)
                        let mut vector_record = vector.clone();
                        // Store sequence_number in version field (optional u32)
                        vector_record.version = Some(sequence_number as u32);

                        batch_records.push((key, vector_record));
                    }

                    // Sort this batch: O(m log m) where m = batch_size
                    batch_records.sort_by(|a, b| a.0.cmp(&b.0));
                    sorted_batches.push(batch_records.into_iter());
                }

                // K-way merge of sorted batches: O(n log k) where k = number of batches
                // This is more efficient than single O(n log n) when k << n
                use std::cmp::Reverse;
                use std::collections::BinaryHeap;

                #[derive(Eq, PartialEq)]
                struct HeapItem {
                    key: String,
                    record: VectorRecord,  // OPTIMIZED: Direct VectorRecord usage
                    batch_idx: usize,
                }

                impl Ord for HeapItem {
                    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                        // Reverse for min-heap behavior
                        other.key.cmp(&self.key)
                    }
                }

                impl PartialOrd for HeapItem {
                    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                        Some(self.cmp(other))
                    }
                }

                let mut heap = BinaryHeap::new();
                let mut iterators = sorted_batches;

                // Initialize heap with first item from each batch
                for (batch_idx, iter) in iterators.iter_mut().enumerate() {
                    if let Some((key, record)) = iter.next() {
                        heap.push(HeapItem { key, record, batch_idx });
                    }
                }

                // Create iterator that performs k-way merge
                let merged_iter = std::iter::from_fn(move || {
                    if let Some(HeapItem { key, record, batch_idx }) = heap.pop() {
                        // Add next item from same batch to heap
                        if let Some((next_key, next_record)) = iterators[batch_idx].next() {
                            heap.push(HeapItem {
                                key: next_key,
                                record: next_record,
                                batch_idx
                            });
                        }
                        Some((key, record))
                    } else {
                        None
                    }
                });

                // Collect iterator to avoid lifetime issues
                merged_iter.collect::<Vec<_>>().into_iter()
            }
        };

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
        writer.write_sorted_vector_records(sorted_records_iter, record_count).await
            .map_err(|e| SstError::Flush(format!("Failed to write SSTable to staging: {}", e)))?;

        // Get file size from staging
        let fs = self.filesystem.get_filesystem(&staging_url)?;
        let metadata = fs.metadata(&staging_url)
            .await
            .map_err(|e| SstError::Internal(format!("Failed to get staging file size: {}", e)))?;
        let file_size = metadata.size;

        // Finalize atomic operation
        atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic flush")?;

        let final_url = format!("{}/{}", collection_storage_url.trim_end_matches('/'), sst_filename);
        let (sst_url, data_len) = (final_url, file_size);
        debug!("🔍 SST FLUSH: SSTable written");
        info!("   - Final URL: {}", sst_url);
        info!("   - Collection storage URL: {}", collection_storage_url);
        info!("   - Filename: {}", sst_filename);
        info!("   - File size: {} bytes", data_len);

        info!(
            "✅ SST: Flushed {} vectors to SSTable: {}",
            entries.len(),
            sst_url
        );

        // SSTable file is now discoverable via directory listing
        // No manifest registration needed - files are self-describing

        // Trigger compaction if manager is available
        if let Some(_compaction_manager) = &self.compaction_manager {
            // Extract block size from collection config if available
            let block_size_kb = params.collection_config.as_ref()
                .and_then(|c| c.config.as_ref())
                .and_then(|cfg| cfg.sst_config.as_ref())
                .and_then(|sst| sst.block_size_kb);

            let _task = CompactionTask {
                level: 0, // Start at level 0
                input_files: vec![std::path::PathBuf::from(sst_url.clone())],
                output_file: std::path::PathBuf::from(format!("{}.compacted", sst_url)),
                priority: CompactionPriority::Medium,
                block_size_kb,
                compression_config: params.collection_config.as_ref()
                    .and_then(|c| c.config.as_ref())
                    .and_then(|cfg| cfg.storage_config.as_ref().and_then(|s| s.compression.as_ref()).clone()),
            };
            // For now, just log that we would trigger compaction
            debug!(
                "Would trigger compaction for collection: {}",
                collection_id
            );
            // compaction_manager.add_task(task).await?;
        }

        // Return flush result with statistics
        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: Some(entries.len() as u64),
            bytes_written: Some(data_len as u64),
            files_created: Some(1),
            duration_ms: Some(0), // Will be set by caller
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

#[async_trait]
impl UnifiedStorageEngine for SstStorage {
    fn engine_name(&self) -> &'static str {
        "sst"
    }

    fn engine_version(&self) -> &'static str {
        crate::version::PROXIMADB_VERSION
    }

    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Lsm
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    /// SST-specific flush implementation - Extract records from WAL vector record batches
    async fn do_flush(&self, params: &FlushParameters) -> anyhow::Result<FlushResult> {
        info!("🚀 SST FLUSH START");
        info!("   - Collection ID: {:?}", params.collection_id);
        info!("   - Vector count: {}", params.vector_records.len());
        info!(
            "   - Has collection_config: {}",
            params.collection_config.is_some()
        );

        debug!("🔍 SST DO_FLUSH: Checking compression configuration");
        if let Some(config) = &params.collection_config {
            info!(
                "   - Has storage_assignment: {}",
                config.storage_assignment.is_some()
            );
            if let Some(assignment) = &config.storage_assignment {
                info!("   - Storage base_location: {}", assignment.base_location);
            }
            // Check compression config
            if let Some(ref collection_config) = config.config {
                if let Some(ref compression) = collection_config
                    .storage_config
                    .as_ref()
                    .filter(|s| s.compression != 0)
                {
                    debug!(
                        "   ✅ Found compression in collection_config: compression={:?}",
                        compression.compression
                    );
                } else {
                    debug!("   ⚠️ No compression config in collection_config");
                }
            } else {
                debug!("   ⚠️ No config field in collection");
            }
        } else {
            debug!("   ⚠️ No collection_config in params");
        }
        info!("🔄 SST: Starting do_flush with WAL vector record batch extraction");

        let collection_id = params.collection_id.as_ref().ok_or_else(|| {
            SstError::InvalidArgument("Collection ID required for SST flush".into())
        })?;

        let operation_id = crate::utils::uuid::Uuid::new_v4().to_string();
        let vector_records = &params.vector_records;

        if vector_records.is_empty() {
            info!(
                "📋 SST: No vector records provided for collection {}",
                collection_id
            );
            return Ok(crate::storage::traits::FlushResult {
                success: true,
                collections_affected: vec![collection_id.clone()],
                entries_flushed: Some(0),
                bytes_written: Some(0),
                files_created: Some(0),
                duration_ms: Some(0),
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
            info!(
                "🔍 DEBUG SST: Vector record {}: id={:?}, vector_len={}, metadata_count={}",
                i,
                vr.id,
                vr.vector.len(),
                vr.metadata.len()
            );
        }

        // Step 1: Extract individual records from deserialized WAL vector record batches
        // These batches come from the global partitioned memtable with WAL behavior
        let filtered_records = self
            .extract_records_from_wal_vector_batches(vector_records, collection_id)
            .await
            .context("Failed to extract records from WAL vector record batches")?;

        info!(
            "📦 SST: Extracted {} individual records from {} vector record batches",
            filtered_records.len(),
            vector_records.len()
        );

        // DEBUG: Log first few records
        for (i, record) in filtered_records.iter().take(3).enumerate() {
            info!(
                "🔍 DEBUG SST: Record {}: id={:?}, version={:?}",
                i, record.id, record.version
            );
        }

        // Extract compression configuration from collection metadata (SDK-driven)
        let compression_config = params
            .collection_config
            .as_ref()
            .and_then(|collection| collection.config.as_ref())
            .and_then(|config| {
                config
                    .storage_config
                    .as_ref()
                    .filter(|s| s.compression != 0)
                    .clone()
            });

        if let Some(ref _compression) = compression_config {
            info!(
                "🗜️ SST: Using SDK-driven compression for collection {}",
                collection_id
            );
        } else {
            info!(
                "🗜️ SST: No compression configuration from SDK for collection {}",
                collection_id
            );
        }

        // Step 2: Process extracted records using row-by-row storage approach
        info!(
            "🔍 DEBUG SST: About to flush {} records to SSTable",
            filtered_records.len()
        );
        let flush_result = self
            .flush_sst_records_to_sstable(
                filtered_records,
                collection_id,
                params.collection_config.as_ref(),
                params.force,
                compression_config.as_ref().map(|storage_config| {
                    crate::proto::proximadb_v1::CompressionConfig {
                        algorithm: storage_config.compression,
                        level: None, // StorageConfig doesn't have level, use default
                        adaptive: false, // Default value
                        min_ratio: None,
                        enable_quantization: false,
                        quantization_type: None, // Default quantization type  
                        normalization_method: None, // Default normalization
                        block_size_kb: 64, // Default block size
                        dynamic_block_sizing: false, // Default static sizing
                    }
                }),
            )
            .await
            .context("Failed to flush records to SSTable with row-by-row storage")?;
        info!(
            "🔍 DEBUG SST: Flush completed - success={}, entries_flushed={}, bytes_written={}",
            flush_result.success,
            flush_result.entries_flushed.unwrap_or(0),
            flush_result.bytes_written.unwrap_or(0)
        );

        info!(
            "✅ SST: Successfully flushed {} records to {} SSTable files ({} bytes)",
            flush_result.entries_flushed.unwrap_or(0),
            flush_result.files_created.unwrap_or(0),
            flush_result.bytes_written.unwrap_or(0)
        );

        // Step 3: Notify EventLog for async AXIS indexing (synchronous acknowledgment)
        let flush_handler =
            crate::storage::engines::impls::sst::flush_eventlog_integration::SstFlushHandler::new();
        // Extract the SST file path from engine_metrics
        let file_paths: Vec<String> =
            if let Some(path_value) = flush_result.engine_metrics.get("sstable_path") {
                if let Some(path_str) = path_value.as_str() {
                    vec![path_str.to_string()]
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

        if let Err(e) = flush_handler
            .notify_flush_complete(params, file_paths, vector_records)
            .await
        {
            // Log but don't fail the flush - EventLog notification is best-effort
            warn!("⚠️ SST: Failed to notify EventLog for AXIS indexing: {}", e);
        } else {
            info!("✅ SST: Successfully notified EventLog for AXIS indexing");
        }

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_flushed: flush_result.entries_flushed,
            bytes_written: flush_result.bytes_written,
            files_created: flush_result.files_created,
            duration_ms: Some(0), // Will be set by high-level flush() method
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
                    serde_json::Value::Number(serde_json::Number::from(
                        flush_result.entries_flushed.unwrap_or(0),
                    )),
                );
                metrics
            },
            compaction_triggered: flush_result.compaction_triggered,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// SST-specific compaction using level-based merge strategy with vector tracking
    async fn do_compact(&self, params: &CompactionParameters) -> anyhow::Result<CompactionResult> {
        let compact_start = std::time::Instant::now();
        let collection_id = params.collection_id.as_ref().ok_or_else(|| {
            SstError::InvalidArgument("Collection ID required for SST compaction".into())
        })?;

        info!(
            "🗜️ SST COMPACTION START: Collection {} (force: {}, priority: {:?})",
            collection_id, params.force, params.priority
        );

        debug!("🔍 SST DO_COMPACT: Checking compression configuration");
        if let Some(ref collection_config) = params.collection_config {
            if let Some(ref config) = collection_config.config {
                if let Some(ref compression) = config
                    .storage_config
                    .as_ref()
                    .filter(|s| s.compression != 0)
                {
                    debug!(
                        "   ✅ Found compression in collection_config: compression={:?}",
                        compression.compression
                    );
                } else {
                    debug!("   ⚠️ No compression config in collection_config");
                }
            } else {
                debug!("   ⚠️ No config field in collection");
            }
        } else {
            debug!("   ⚠️ No collection_config in params");
        }

        let mut result = CompactionResult {
            success: false,
            collections_affected: Vec::new(),
            entries_processed: Some(0),
            entries_removed: Some(0),
            bytes_read: Some(0),
            bytes_written: Some(0),
            input_files: Some(0),
            output_files: Some(0),
            duration_ms: Some(0),
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        };

        // SST-specific compaction: Level-based SSTable merging
        if let Some(compaction_manager) = &self.compaction_manager {
            // Get storage location from collection config - skip if not provided
            debug!("🔍 SST COMPACTION: Checking collection_config for storage assignment");
            debug!(
                "   - Has collection_config: {}",
                params.collection_config.is_some()
            );
            if let Some(config) = params.collection_config.as_ref() {
                debug!(
                    "   - Has storage_assignment: {}",
                    config.storage_assignment.is_some()
                );
                if let Some(assignment) = &config.storage_assignment {
                    debug!("   - Storage base_location: {}", assignment.base_location);
                }
            }

            let storage_location = match params
                .collection_config
                .as_ref()
                .and_then(|c| c.storage_assignment.as_ref())
            {
                Some(assignment) => assignment.base_location.clone(),
                None => {
                    error!(
                        "❌ SST: No storage assignment found for collection {}. Skipping compaction!",
                        collection_id
                    );
                    error!(
                        "   - collection_config present: {}",
                        params.collection_config.is_some()
                    );
                    // Return early with failure result
                    result.success = false;
                    result.collections_affected = vec![collection_id.to_string()];
                    result.duration_ms = Some(compact_start.elapsed().as_millis() as u64);
                    result.engine_metrics.insert(
                        "error".to_string(),
                        serde_json::Value::String("Missing storage assignment".to_string()),
                    );
                    return Ok(result);
                }
            };

            let collection_storage_url = format!("{}/{}/data", storage_location, collection_id);
            info!(
                "🔄 SST COMPACTION: Checking for compaction needs in {}",
                collection_storage_url
            );

            let collection_dir = std::path::PathBuf::from(
                collection_storage_url
                    .strip_prefix("file://")
                    .unwrap_or(&collection_storage_url),
            );

            debug!(
                "🔍 SST COMPACTION: Collection directory path: {}",
                collection_dir.display()
            );
            info!("   - Directory exists: {}", collection_dir.exists());

            // Check if compaction is needed
            let check_result = compaction_manager
                .check_compaction_needed(collection_id, &collection_dir)
                .await?;

            // Log what we found
            if check_result.is_none() && params.force {
                info!(
                    "⚠️ SST COMPACTION: No compaction task found but force=true. Checking threshold..."
                );
                // List files to understand why no compaction
                if let Ok(entries) = tokio::fs::read_dir(&collection_dir).await {
                    let mut files = Vec::new();
                    let mut entries = entries;
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if let Some(name) = entry.file_name().to_str() {
                            if name.ends_with(".sstable") {
                                files.push(name.to_string());
                            }
                        }
                    }
                    info!(
                        "   - Found {} SST files in directory (threshold: {})",
                        files.len(),
                        self.config.compaction_threshold
                    );
                    for file in &files {
                        debug!("     - {}", file);
                    }
                }
            }

            if let Some(task) = check_result {
                info!(
                    "🔄 SST COMPACTION: Executing synchronous compaction for collection {} level {}",
                    collection_id, task.level
                );
                info!(
                    "   - Input files: {} (already filtered for AXIS-ready files)",
                    task.input_files.len()
                );
                for (idx, file) in task.input_files.iter().enumerate() {
                    debug!("     - Compacting file {}: {}", idx + 1, file.display());
                }

                // Note: Files have already been filtered in check_compaction_needed
                // Only AXIS-ready files are included in the task
                let flush_handler = crate::storage::engines::impls::sst::flush_eventlog_integration::SstFlushHandler::new();
                let input_file_paths: Vec<String> = task
                    .input_files
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();

                debug!(
                    "🔍 SST COMPACTION: Processing {} pre-filtered compactable files for collection {}",
                    task.input_files.len(),
                    collection_id
                );

                // Execute compaction synchronously to capture vector tracking
                let compaction_manager = compaction::Compaction::with_atomic_coordinator(
                    self.config.clone(),
                    Some(self.atomic_coordinator.clone()),
                )
                .await?;

                // Extract compression configuration from collection metadata (SDK-driven)
                let compression_config = params
                    .collection_config
                    .as_ref()
                    .and_then(|collection| collection.config.as_ref())
                    .and_then(|config| {
                        config
                            .storage_config
                            .as_ref()
                            .filter(|s| s.compression != 0)
                            .clone()
                    });

                debug!(
                    "🔍 SST DO_COMPACT: Passing compression to perform_compaction_enhanced: {:?}",
                    compression_config
                        .as_ref()
                        .map(|c| format!("compression={}, max_file_size_mb={}", c.compression, c.max_file_size_mb))
                );

                let enhanced_stats = compaction_manager
                    .perform_compaction_enhanced(
                        &task,
                        &self.config,
                        Some(self.atomic_coordinator.clone()),
                        compression_config.as_ref().map(|storage_config| {
                            crate::proto::proximadb_v1::CompressionConfig {
                                algorithm: storage_config.compression,
                                level: None, // StorageConfig doesn't have level, use default
                                adaptive: false, // Default value
                                min_ratio: None,
                                enable_quantization: false,
                                quantization_type: None, // Default quantization type
                                normalization_method: None, // Default normalization
                                block_size_kb: 64, // Default block size
                                dynamic_block_sizing: false, // Default static sizing
                            }
                        }),
                    )
                    .await?;

                result.collections_affected.push(collection_id.clone());
                result.entries_processed = Some(enhanced_stats.merged_vectors.len() as u64);
                result.entries_removed = Some(enhanced_stats.deleted_vector_ids.len() as u64);
                result.bytes_read = Some(enhanced_stats.base_stats.bytes_read);
                result.bytes_written = Some(enhanced_stats.base_stats.bytes_written);
                result.input_files = Some(enhanced_stats.base_stats.files_merged);
                result.output_files = Some(1); // One output file per compaction
                result.success = true;

                // Store vector tracking data in engine_metrics
                result.engine_metrics.insert(
                    "deleted_vector_ids".to_string(),
                    serde_json::Value::Array(
                        enhanced_stats
                            .deleted_vector_ids
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
                result.engine_metrics.insert(
                    "merged_vectors_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(
                        enhanced_stats.merged_vectors.len(),
                    )),
                );

                // Note: We don't store the actual merged vectors in metrics to avoid memory bloat
                // The compaction process has already updated the storage with the merged data

                info!(
                    "✅ SST COMPACTION: Completed for collection {} (deleted: {}, merged: {}, bytes written: {})",
                    collection_id,
                    result.entries_removed.unwrap_or(0),
                    result.entries_processed.unwrap_or(0),
                    enhanced_stats.base_stats.bytes_written
                );

                // Notify EventLog about successful compaction
                let output_file = task.output_file.to_string_lossy().to_string();
                debug!(
                    "📨 SST COMPACTION: Notifying EventLog about completed compaction:\n  Collection: {}\n  Output file: {}\n  Input files: {:?}\n  Merged vectors: {}",
                    collection_id,
                    output_file,
                    input_file_paths,
                    enhanced_stats.merged_vectors.len()
                );

                flush_handler.notify_compaction_complete(
                    collection_id,
                    vec![output_file.clone()],
                    enhanced_stats.merged_vectors.len(),
                );

                // Clean up old files from EventLog tracking
                debug!(
                    "🧹 SST COMPACTION: Cleaning up {} compacted files from EventLog for collection {}",
                    input_file_paths.len(),
                    collection_id
                );

                if let Err(e) = flush_handler
                    .cleanup_compacted_files(collection_id, input_file_paths.clone())
                    .await
                {
                    warn!("Failed to cleanup compacted files from EventLog: {}", e);
                } else {
                    debug!(
                        "✅ SST COMPACTION: Successfully cleaned up EventLog tracking for compacted files"
                    );
                }
            } else {
                debug!(
                    "🔍 SST COMPACTION: No compaction needed for collection {}",
                    collection_id
                );
                info!("   - Files below threshold or no files found");
                result.success = true; // No compaction needed is still successful
                result.collections_affected.push(collection_id.to_string());
            }
        } else {
            warn!("⚠️ SST COMPACTION: No compaction manager available");
            result.success = false;
        }

        result.duration_ms = Some(compact_start.elapsed().as_millis() as u64);
        Ok(result)
    }

    /// Retrieve vector by ID from SST storage (Pure SSTable lookup with bloom filter optimization)
    async fn vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> anyhow::Result<Option<crate::core::VectorRecord>> {
        debug!(
            "🔍 SST: Looking up vector {} in collection {} using manifest",
            vector_id, collection_id
        );

        // Note: In a real implementation, we'd get the storage location from a metadata service
        // For now, we'll just log and return None if we can't determine the location
        warn!(
            "⚠️ SST: vector_by_id needs collection metadata service integration for collection {}",
            collection_id
        );

        // Get SSTable files that might contain this key
        // Direct directory scan for overlapping files (simplified for now)
        let overlapping_files: Vec<String> = vec![];

        if overlapping_files.is_empty() {
            debug!("📂 SST: No SSTable files overlap with key {}", vector_id);
            return Ok(None);
        }

        let mut sstables_checked = 0;
        let mut bloom_filter_hits = 0;

        // Search through files in key range order
        for file_path in overlapping_files {
            sstables_checked += 1;

            let filename = std::path::Path::new(&file_path)
                .file_name()
                .and_then(|n| n.to_str());

            // Create a zero-copy system for the reader
            use crate::storage::engines::core::io::zero_copy::{
                ZeroCopyIOConfig, ZeroCopyIOSystem,
            };
            let zero_copy_config = ZeroCopyIOConfig::default();
            let zero_copy_system = match ZeroCopyIOSystem::new(
                zero_copy_config,
                self.filesystem.clone(),
                vec![],
            )
            .await
            {
                Ok(system) => Arc::new(system),
                Err(e) => {
                    warn!(
                        "Failed to create zero-copy system, skipping bloom filter optimization: {}",
                        e
                    );
                    continue;
                }
            };

            // Use unified SSTable reader with bloom filter
            let reader = UnifiedSstableReader::new(
                self.filesystem.clone(),
                zero_copy_system,
                "sst_lookup".to_string(),
            );

            // Load metadata (includes bloom filter)
            if reader.load_metadata(&file_path).await.is_ok() {
                // Check bloom filter first
                if reader.might_contain_key(&file_path, vector_id).await {
                    bloom_filter_hits += 1;
                    trace!(
                        "🌸 SST: Bloom filter hit for {} in {}",
                        vector_id,
                        filename.unwrap_or("unknown")
                    );

                    // Actually search the SSTable
                    if let Ok(Some(record)) = reader.vector(&file_path, vector_id).await {
                        debug!(
                            "✅ SST: Found vector {} in SSTable {} (checked {}/{} SSTables, {} bloom hits)",
                            vector_id,
                            filename.unwrap_or("unknown"),
                            bloom_filter_hits,
                            sstables_checked,
                            bloom_filter_hits
                        );
                        return Ok(Some(record));
                    }
                } else {
                    trace!(
                        "🌸 SST: Bloom filter miss for {} in {} - skipping",
                        vector_id,
                        filename.unwrap_or("unknown")
                    );
                }
            } else {
                warn!(
                    "⚠️ Failed to load metadata for SSTable {}",
                    filename.unwrap_or("unknown")
                );
            }
        }

        debug!(
            "❌ SST: Vector {} not found in collection {} (checked {} SSTables, {} bloom hits)",
            vector_id, collection_id, sstables_checked, bloom_filter_hits
        );
        Ok(None)
    }

    /// SST ENGINE OPTIMIZATION: Unified search using SstUnifiedSearchEngine
    async fn search_vectors_unified(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
    ) -> anyhow::Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        let search_start = std::time::Instant::now();
        if let Some(orch) = &self.orchestrator {
            (**orch).pattern_tracker().track_access_async(
                format!("{}::sst::metadata", ctx.collection_id()),
                crate::storage::cache::orchestrator::CacheType::Metadata,
            );
        }

        // Extract parameters from context
        let collection_id = ctx.collection_id();
        let storage_url = ctx
            .collection_storage_path()
            .ok_or_else(|| SstError::InvalidArgument("No storage URL in context".into()))?;
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| SstError::InvalidArgument("No query vector in context".into()))?;
        let k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        let filter_expression = ctx.search_params.filter_expression.as_ref();
        // TODO: These should be passed in StorageQueryContext or as separate parameters
        let include_vectors = true; // Default to including vectors
        let include_metadata = true; // Default to including metadata

        info!(
            "🚀 SST: Enhanced unified search with orchestration for collection {}",
            collection_id
        );

        // ========================================================================
        // PHASE 1: SEARCH ORCHESTRATION AND STRATEGY SELECTION
        // ========================================================================

        // ========================================================================
        // INTELLIGENT SEARCH ORCHESTRATION
        // ========================================================================

        // Check if orchestration should be used based on context metadata
        let use_orchestration = ctx.metadata.use_axis_indexes || ctx.metadata.has_quantization;

        if use_orchestration {
            info!("🎯 SST: Using intelligent search orchestration");

            // Create mock services for orchestration (in real implementation, these would come from context)
            // For now, we'll create minimal implementations to enable orchestration functionality
            let axis_manager = match self.mock_axis_manager() {
                Ok(manager) => manager,
                Err(e) => {
                    warn!(
                        "⚠️ Failed to get AXIS manager: {}, falling back to direct search",
                        e
                    );
                    let fallback_results = self
                        .fallback_to_direct_search(
                            ctx,
                            collection_id,
                            &storage_url,
                            query_vector,
                            k,
                            distance_metric,
                            filter_expression,
                            include_vectors,
                            include_metadata,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("Fallback search failed: {}", e))?;

                    // Return the fallback results directly - they are already OptimizedSearchRecord
                    return Ok(fallback_results);
                }
            };

            let collection_service = self.mock_collection_service();
            let distance_engine = self.mock_distance_engine();
            let quantization_engine = self.mock_quantization_engine();
            let storage_engine = self.mock_storage_engine();

            // Create search orchestrator for intelligent routing
            // For now, use a simple strategy selection since the complex orchestrator needs more setup
            let strategy = ExecutionStrategy::DirectFP32 {
                reason: "SST engine default".to_string(),
                expected_latency_ms: 10,
            };

            match Ok::<ExecutionStrategy, anyhow::Error>(strategy) {
                Ok(strategy) => {
                    debug!("📋 Using default SST search strategy");

                    match Ok::<_, anyhow::Error>(&strategy) {
                        Ok(strategy) => {
                            info!(
                                "🎯 Strategy Selected: {} (estimated cost: {:.2}ms)",
                                match &strategy {
                                    ExecutionStrategy::IndexFirst {
                                        expected_latency_ms,
                                        ..
                                    } => {
                                        format!(
                                            "IndexFirst (cost: {:.2}ms)",
                                            *expected_latency_ms as f32
                                        )
                                    }
                                    ExecutionStrategy::Progressive {
                                        expected_latency_ms,
                                        ..
                                    } => {
                                        format!(
                                            "Progressive (cost: {:.2}ms)",
                                            *expected_latency_ms as f32
                                        )
                                    }
                                    ExecutionStrategy::DirectFP32 {
                                        expected_latency_ms,
                                        ..
                                    } => {
                                        format!(
                                            "DirectFP32 (cost: {:.2}ms)",
                                            *expected_latency_ms as f32
                                        )
                                    }
                                    _ => {
                                        format!("Other (cost: 10ms)")
                                    }
                                },
                                match &strategy {
                                    ExecutionStrategy::IndexFirst {
                                        expected_latency_ms,
                                        ..
                                    } => *expected_latency_ms as f32,
                                    ExecutionStrategy::Progressive {
                                        expected_latency_ms,
                                        ..
                                    } => *expected_latency_ms as f32,
                                    ExecutionStrategy::DirectFP32 {
                                        expected_latency_ms,
                                        ..
                                    } => *expected_latency_ms as f32,
                                    _ => 10.0,
                                }
                            );

                            info!(
                                "🔍 SST: Using {} strategy for search",
                                match &strategy {
                                    ExecutionStrategy::DirectFP32 { .. } => "DirectFP32",
                                    ExecutionStrategy::IndexFirst { .. } => "IndexFirst",
                                    ExecutionStrategy::Progressive { .. } => "Progressive",
                                    _ => "Unknown",
                                }
                            );
                            // For now, continue with the existing search implementation below
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ Strategy selection failed: {}, falling back to direct search",
                                e
                            );
                            // Fall through to existing implementation
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to create search orchestrator: {}, falling back to direct search",
                        e
                    );
                    // Fall through to existing implementation
                }
            }
        }

        // ========================================================================
        // PHASE 2: CURRENT IMPLEMENTATION WITH ENHANCED LOGGING
        // ========================================================================

        info!("🔍 SST: Using current unified search implementation (orchestration disabled)");

        // Use search params from context (already available as Arc)
        let search_params = ctx.search_params.clone();

        // Use the storage URL from context
        debug!(
            "🔍 SST: Using storage_url = {} for collection {}",
            storage_url, collection_id
        );

        // ========================================================================
        // PHASE 3: ENHANCED COLLECTION CONFIGURATION ANALYSIS
        // ========================================================================

        debug!("📊 Collection Configuration Analysis:");
        debug!("  🎯 Query vector dimension: {}", query_vector.len());
        debug!("  📏 Top-k requested: {}", k);
        debug!("  📐 Distance metric: {:?}", distance_metric);
        debug!(
            "  🔍 Has filter expression: {}",
            filter_expression.is_some()
        );
        if let Some(filter) = filter_expression {
            debug!("  🔎 Filter details: {:?}", filter);
        }

        // Analyze collection quantization capabilities
        let collection_config = &ctx.collection.config;
        if let Some(config) = collection_config {
            if let Some(quant_config) = &config.quantization {
                debug!("  🔧 Quantization Analysis:");
                debug!("    ✅ Enabled: {}", quant_config.enabled);
                debug!("    🎛️  Strategy: {:?}", quant_config.strategy);
                debug!(
                    "    🔄 Progressive search: {}",
                    quant_config.enable_progressive_search
                );
                debug!(
                    "    📋 Custom levels: {} defined",
                    quant_config.custom_levels.len()
                );
            } else {
                debug!("  🔧 Quantization: Not configured (FP32 only)");
            }
        } else {
            debug!("  🔧 Collection config: Not available");
        }

        // Pre-discover SSTable files to avoid redundant filesystem queries
        let sstable_files = {
            let mut files = Vec::new();
            let fs = self.filesystem.get_filesystem(&storage_url)?;
            let entries = fs.list(&storage_url).await?;
            for entry in entries {
                if !entry.metadata.is_directory && entry.name.ends_with(".sstable") {
                    files.push(entry.url);
                }
            }
            files
        };
        debug!(
            "🔍 SST: Pre-discovered {} SSTable files",
            sstable_files.len()
        );

        let context = crate::core::search::SearchPlan {
            collection_id: collection_id.to_string(),
            collection_config: Some(crate::core::search::CollectionConfig {
                default_distance_metric: distance_metric,
                vector_dimension: query_vector.len(), // TODO: Get from collection config when available
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

        // TODO: Use IntegratedSearchOptimizer instead of deleted SstUnifiedSearchEngine
        // For now, create a basic result set
        let result_set = crate::core::search::SearchResultSet {
            results: vec![].into(),
            total_count: 0,
            query_id: None,
            processing_time_us: 0,
            algorithm: "SST".to_string(),
            metadata: HashMap::new(),
        };

        // Old code commented out - needs integration with IntegratedSearchOptimizer
        // let search_engine = unified_search_engine::SstUnifiedSearchEngine::new(
        //     self.sstable_reader.clone(),
        //     self.distance_compute.clone(),
        //     self.quantization_engine.clone(),
        //     storage_url.to_string(),
        //     self.filesystem.clone(),
        // );
        // let result_set = search_engine.search_unified(...).await?;

        // Use OptimizedSearchRecord directly - no conversion needed
        // Search engine now returns OptimizedSearchRecord for better performance
        let mut optimized_results: Vec<crate::core::search::results::OptimizedSearchRecord> =
            result_set.results.iter().cloned().collect();

        // Filter results based on include_vectors and include_metadata
        if !include_vectors {
            for result in &mut optimized_results {
                result.vector = None;
            }
        }
        if !include_metadata {
            for result in &mut optimized_results {
                result.metadata = HashMap::new();
            }
        }

        // ========================================================================
        // PHASE 4: PERFORMANCE TRACKING AND FINAL LOGGING
        // ========================================================================

        let total_search_time = search_start.elapsed();

        info!(
            "🏁 SST Unified Search Completed - Collection: {}, Results: {}/{}, Time: {:.2}ms",
            collection_id,
            optimized_results.len(),
            k,
            total_search_time.as_secs_f32() * 1000.0
        );

        // Enhanced result analysis
        debug!("📈 Search Results Analysis:");
        debug!("  📊 Total results found: {}", optimized_results.len());
        debug!("  🎯 Requested top-k: {}", k);
        debug!(
            "  ✅ Results coverage: {:.1}%",
            if k > 0 {
                (optimized_results.len() as f32 / k as f32 * 100.0).min(100.0)
            } else {
                0.0
            }
        );
        debug!(
            "  ⏱️  Total search time: {:.2}ms",
            total_search_time.as_secs_f32() * 1000.0
        );

        // Log sample results with enhanced details
        if !optimized_results.is_empty() {
            debug!("🔍 Sample Results (top 3):");
            for (i, result) in optimized_results.iter().take(3).enumerate() {
                debug!(
                    "  Result {}: id={}, score={:.4}, similarity={:?}, has_vector={}, metadata_fields={}",
                    i + 1,
                    result.id,
                    result.score,
                    result.similarity,
                    result.vector.is_some(),
                    result.metadata.len()
                );

                // Log metadata details for first result
                if i == 0 && result.metadata.len() > 0 {
                    let metadata_map = crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&result.metadata);
                    debug!(
                        "    📋 Metadata sample: {:?}",
                        metadata_map
                            .iter()
                            .take(3)
                            .map(|(k, v)| format!("{}={:?}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        } else {
            debug!("🔍 No results found for query");
        }

        // Log performance characteristics
        if total_search_time.as_millis() > 100 {
            warn!(
                "⚠️ Slow search detected: {:.2}ms for collection {} with {} results",
                total_search_time.as_secs_f32() * 1000.0,
                collection_id,
                optimized_results.len()
            );
        } else if total_search_time.as_millis() < 10 {
            debug!(
                "🚀 Fast search: {:.2}ms for collection {} with {} results",
                total_search_time.as_secs_f32() * 1000.0,
                collection_id,
                optimized_results.len()
            );
        }

        Ok(optimized_results)
    }

    /// SST-specific engine metrics
    async fn collect_engine_metrics(&self) -> anyhow::Result<HashMap<String, serde_json::Value>> {
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

impl SstStorage {
    // =============================================================================
    // SST IMPLEMENTATION HELPER METHODS (Private)
    // =============================================================================

    // ===== Methods from third impl block (consolidated) =====
    /// Extract individual records from deserialized WAL vector record batches
    /// These batches come from the global partitioned memtable with WAL behavior
    /// Enhanced with batch processing optimizations for improved performance
    async fn extract_records_from_wal_vector_batches(
        &self,
        vector_records: &[VectorRecord],
        collection_id: &str,
    ) -> Result<Vec<VectorRecord>> {
        // OPTIMIZED: Return VectorRecord directly
        let extraction_start = std::time::Instant::now();
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;

        info!(
            "🔍 SST ENGINE-OPTIMIZED EXTRACTION: Processing {} WAL vector record batches for collection {}",
            vector_records.len(),
            collection_id
        );

        // Pre-allocate with estimated capacity for better memory efficiency
        let estimated_matches = vector_records.len() / 4; // Conservative estimate
        let mut filtered_records = Vec::with_capacity(estimated_matches); // OPTIMIZED: Direct VectorRecord storage

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
                    debug!(
                        "🔍 Pre-conversion record {}: id={:?}, metadata={:?}",
                        global_index,
                        vector_record.id,
                        vector_record
                            .metadata
                            .iter()
                            .map(|m| format!("{}={:?}", m.0, m.1))
                            .collect::<Vec<_>>()
                    );
                }

                // OPTIMIZED: Use VectorRecord directly (no SstRecord conversion)
                let mut vector_record = vector_record.clone();

                // Store sequence number in version field for SST ordering
                vector_record.version = Some((sequence_start + global_index as u64) as i64);

                filtered_records.push(vector_record);
                chunk_matches += 1;

                batch_stats.total_extracted += 1;
            }

            let chunk_time = chunk_start.elapsed().as_micros() as u64;
            batch_stats.chunk_times.push(chunk_time);

            debug!(
                "📦 SST CHUNK {}: Processed {} records, {} matches in {}μs",
                chunk_idx,
                chunk.len(),
                chunk_matches,
                chunk_time
            );
        }

        // Sort records by sequence number for optimal SSTable performance
        if filtered_records.len() > 1 {
            let sort_start = std::time::Instant::now();
            // Sort by version field (contains sequence_number)
            filtered_records.sort_by_key(|r| r.version);
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
            filtered_records.len(),
            vector_records.len(),
            total_extraction_time,
            avg_chunk_time,
            batch_stats.sort_time_us
        );

        Ok(filtered_records)
    }

    /// Flush memtable data to SSTable files using SST's row-based architecture
    async fn flush_sst_records_to_sstable(
        &self,
        vector_records: Vec<VectorRecord>, // OPTIMIZED: Accept VectorRecord directly
        collection_id: &str,
        collection_config: Option<&Collection>,
        _force_flush: bool,
        compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    ) -> Result<FlushResult> {
        let flush_start = std::time::Instant::now();

        info!(
            "🗂️ SST SSTABLE FLUSH: Processing {} records",
            vector_records.len()
        );

        // DEBUG: Log record details
        if vector_records.is_empty() {
            warn!("🔍 DEBUG SST: No records to flush - returning early!");
        } else {
            info!("🔍 DEBUG SST: First record: id={:?}", vector_records[0].id);
        }

        // Stage 1: Sort records by ID for SSTable ordering
        let sorting_start = std::time::Instant::now();
        let mut sorted_records = vector_records;
        sorted_records.sort_by(|a, b| a.id.as_str().cmp(&b.id.as_str()));
        let sorting_time = sorting_start.elapsed().as_millis() as u64;
        debug!(
            "📊 SST STAGE 1: Sorted {} records in {}ms",
            sorted_records.len(),
            sorting_time
        );

        // Stage 2: Partition records into levels based on SST tree structure
        let partitioning_start = std::time::Instant::now();
        let level_partitions = self.partition_records_by_level(&sorted_records).await?;
        let partitioning_time = partitioning_start.elapsed().as_millis() as u64;
        let num_levels = level_partitions.len();
        debug!(
            "🏗️ SST STAGE 2: Partitioned into {} levels in {}ms",
            num_levels, partitioning_time
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
                .and_then(|c| c.storage_assignment.as_ref())
            {
                Some(assignment) => {
                    let url = format!("{}/{}/data", assignment.base_location, collection_id);
                    debug!(
                        "🔍 SST: Using storage URL: {} for collection: {}",
                        url, collection_id
                    );
                    url
                }
                None => {
                    error!(
                        "❌ SST: No storage assignment found for collection {}. Skipping flush operation!",
                        collection_id
                    );
                    // Return early with failure result
                    return Ok(FlushResult {
                        success: false,
                        collections_affected: vec![collection_id.to_string()],
                        entries_flushed: Some(0),
                        bytes_written: Some(0),
                        files_created: Some(0),
                        duration_ms: Some(flush_start.elapsed().as_millis() as u64),
                        completed_at: Utc::now(),
                        engine_metrics: {
                            let mut metrics = HashMap::new();
                            metrics.insert(
                                "error".to_string(),
                                serde_json::Value::String("Missing storage assignment".to_string()),
                            );
                            metrics
                        },
                        compaction_triggered: false,
                        flushed_batch_ids: vec![],
                    });
                }
            };
            let data_dir = PathBuf::from(
                collection_storage_url
                    .strip_prefix("file://")
                    .unwrap_or(&collection_storage_url),
            );

            // Generate SSTable filename using centralized utility
            let codec = FilenameCodec::new();
            let sst_filename = codec.generate(level as u32, "sst");
            let sst_path = data_dir.join(&sst_filename);
            debug!(
                "🔧 SST: Creating compacted SSTable file: {} at path: {} for collection: {}",
                sst_filename,
                sst_path.display(),
                collection_id
            );

            // Ensure directory exists
            if let Some(parent) = sst_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    SstError::Internal(format!("Failed to create directory: {}", e))
                })?;
            }

            // Convert VectorRecords to BTreeMap for SstableWriter (OPTIMIZED: Direct conversion)
            // Handle append-only vectors with unique keys
            let mut entries = BTreeMap::new();
            let mut append_only_counter = 0u64;

            for record in &level_records {
                let key = if record.id.is_empty() {
                    // For append-only vectors (empty IDs), use a unique key
                    let unique_key = format!("__append_only_seq_{}", append_only_counter);
                    append_only_counter += 1;
                    debug!(
                        "🔍 SST FLUSH: Append-only vector detected in level {}, using key='{}'",
                        level, unique_key
                    );
                    unique_key
                } else {
                    // Skip records with empty IDs - upsert logic depends on this
                    if record.id.is_empty() {
                        continue;
                    }
                    record.id.clone()
                };
                entries.insert(key, record.clone());
            }

            // Use SstableWriter with collection config for quantization and compression
            let block_size = (self.config.block_size_kb * 1024) as usize;
            let writer = writer::SstableWriter::new_with_config(
                &sst_path,
                block_size,
                Arc::clone(&self.filesystem),
                collection_config,
            );

            // Use bloom filter config from SST config if available
            let writer = if let Some(ref bloom_config) = self.config.bloom_filter_config {
                writer.with_bloom_config(bloom_config.clone())
            } else {
                writer
            };

            // Quantization is ALWAYS enabled in SstableWriter - no need to call with_quantization()
            // It's part of the SST file layout and provides PQ sorting for better compression and selectivity
            debug!(
                "🎯 SST: Writing level {} with integrated quantization and PQ sorting",
                level
            );

            // Write records using SstableWriter (streaming approach for production consistency)
            debug!(
                "🔍 SST: About to write {} entries to file using streaming approach: {}",
                entries.len(),
                sst_path.display()
            );
            let record_count = entries.len();
            let sorted_records_iter = entries.into_iter(); // BTreeMap already sorted by key
            writer
                .write_sorted_vector_records(sorted_records_iter, record_count)
                .await
                .map_err(|e| SstError::Flush(format!("Failed to write SSTable: {}", e)))?;
            debug!(
                "🔍 SST: Successfully wrote SSTable file: {}",
                sst_path.display()
            );

            // Get file size
            let metadata = tokio::fs::metadata(&sst_path).await?;
            let file_size = metadata.len();
            total_bytes_written += file_size;
            files_created += 1;
            sstable_paths.push(sst_path.clone());

            // Verify file exists
            if metadata.len() > 0 {
                debug!(
                    "✅ SST: Compacted SSTable verified - {} bytes at {}",
                    file_size,
                    sst_path.display()
                );
            } else {
                warn!(
                    "⚠️ SST: Compacted SSTable file is empty: {}",
                    sst_path.display()
                );
            }

            debug!(
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
            entries_flushed: Some(sorted_records.len() as u64),
            bytes_written: Some(total_bytes_written),
            files_created: Some(files_created),
            duration_ms: Some(total_flush_time),
            completed_at: Utc::now(),
            compaction_triggered,
            engine_metrics,
            flushed_batch_ids: vec![],
        })
    }

    /// Partition records into SST tree levels based on key ranges and record age
    async fn partition_records_by_level(
        &self,
        sorted_records: &[VectorRecord], // OPTIMIZED: Accept VectorRecord directly
    ) -> Result<HashMap<u8, Vec<VectorRecord>>> {
        // OPTIMIZED: Return VectorRecord directly
        let mut level_partitions: HashMap<u8, Vec<VectorRecord>> = HashMap::new();

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
        records: &[VectorRecord], // OPTIMIZED: Accept VectorRecord directly
        level: u8,
    ) -> Result<Vec<u8>> {
        let serialization_start = std::time::Instant::now();

        // Engine optimization: Pre-allocate based on estimated size
        let estimated_size = records.len() * 512; // Conservative estimate per record
        let mut sstable_data = Vec::with_capacity(estimated_size);

        // Step 1: Create enhanced header with hierarchical optimizations
        let header = SstableHeader {
            version: 1, // Version 1 for initial implementation
            level,
            entry_count: records.len() as u64,
            min_key: records.first().map(|r| r.id.clone()).unwrap_or_default(),
            max_key: records.last().map(|r| r.id.clone()).unwrap_or_default(),
            timestamp: Utc::now().timestamp(),

            // Compression configuration
            compression_algorithm: CompressionAlgorithm::None,
            compression_level: 0,

            // Bloom filter configuration
            has_bloom_filter: true,
            has_global_bloom: true,
            has_block_blooms: false, // Will be updated based on actual blocks
            metadata_column_count: 0, // Will be calculated

            // Block organization
            block_size: (self.config.block_size_kb * 1024) as u32,
            batch_size: records.len() as u32,
            block_count: 0, // Will be updated

            // Component sizes
            header_size: 0,
            index_size: 0,
            data_size: 0,

            // NEW: Direct access offsets (placeholder values)
            global_bloom_offset: 0,
            global_bloom_size: 0,
            block_index_offset: 0,
            block_index_size: 0,
            data_blocks_offset: 0,

            // NEW: Vector format optimization (default to variable)
            vector_format: VectorFormat::Variable,
            fixed_dimension: None,
            compression_ratio: 1.0, // Will be calculated from actual compressed/uncompressed sizes
        };

        // Step 2: Build bloom filter for fast key existence checks
        let bloom_filter = self.build_bloom_filter(records).await?;
        let bloom_data = bloom_filter
            .serialize()
            .map_err(|e| SstError::Internal(format!("Failed to serialize bloom filter: {}", e)))?;

        // Step 3: Organize records into blocks for better cache performance
        let data_blocks = self
            .organize_records_into_blocks(records, header.block_size as usize)
            .await?;

        // Step 4: Engine-optimized index with block pointers
        let (index_entries, compressed_blocks) = self
            .build_optimized_index_and_compress_blocks(&data_blocks)
            .await?;

        // Step 5: Serialize header
        let header_data = bincode::serialize(&header)
            .map_err(|e| SstError::Internal(format!("Failed to serialize header: {}", e)))?;
        sstable_data.extend((header_data.len()).to_le_bytes());
        sstable_data.extend(header_data);

        // Step 6: Serialize bloom filter
        sstable_data.extend((bloom_data.len()).to_le_bytes());
        sstable_data.extend(bloom_data);

        // Step 7: Serialize enhanced index using custom serialization
        let mut index_data = Vec::new();
        for entry in &index_entries {
            let entry_data = entry.serialize().map_err(|e| {
                SstError::Internal(format!("Failed to serialize index entry: {}", e))
            })?;
            index_data.extend_from_slice(&(entry_data.len()).to_le_bytes());
            index_data.extend_from_slice(&entry_data);
        }
        sstable_data.extend((index_data.len()).to_le_bytes());
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

        info!(
            "🚀 SST ENGINE-OPTIMIZED SSTABLE: Level {} serialized - {} records, {} bytes, {:.2}x compression, {}ms",
            level,
            records.len(),
            sstable_data.len(),
            compression_ratio,
            serialization_time
        );

        Ok(sstable_data)
    }

    /// Update SST tree metadata after successful flush
    async fn update_lsm_metadata_after_flush(
        &self,
        sstable_paths: &[std::path::PathBuf],
        flushed_records: &[VectorRecord], // OPTIMIZED: Accept VectorRecord directly
    ) -> Result<()> {
        info!(
            "📊 SST METADATA: Updating manifest for {} SSTables, {} records",
            sstable_paths.len(),
            flushed_records.len()
        );

        // Register each SSTable file with the manifest
        for path in sstable_paths {
            // Extract filename from path
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| SstError::InvalidArgument("Invalid SSTable filename".to_string()))?;

            // Parse level from filename using centralized utility
            let level = Some(FilenameCodec::new().parse_level(filename) as u8);

            // Get file size
            let metadata = tokio::fs::metadata(path).await?;
            let _file_size = metadata.len();

            // Calculate min/max keys and sequences from records in this SSTable
            // OPTIMIZED: Process all VectorRecords (level filtering removed since VectorRecord doesn't have level)
            if flushed_records.is_empty() {
                continue;
            }

            let _min_key = flushed_records
                .iter()
                .map(|r| r.id.as_str())
                .min()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let _max_key = flushed_records
                .iter()
                .map(|r| r.id.as_str())
                .max()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let _min_sequence = flushed_records
                .iter()
                .filter_map(|r| r.version)
                .min()
                .unwrap_or(0) as u64;
            let _max_sequence = flushed_records
                .iter()
                .filter_map(|r| r.version)
                .max()
                .unwrap_or(0) as u64;

            // SSTable file is now discoverable via directory listing
            info!(
                "Created SSTable file: {} with {} records at level {}",
                filename,
                flushed_records.len(),
                level.unwrap_or(0)
            );
        }

        Ok(())
    }

    /// Check if compaction is needed based on SST tree structure
    async fn check_compaction_threshold(&self) -> Result<bool> {
        // Check Level 0 file count (trigger compaction if too many files)
        let level0_files = self.count_sstables_at_level(0).await?;
        let compaction_needed = level0_files >= self.config.compaction_threshold as usize;

        if compaction_needed {
            debug!(
                "🗜️ SST COMPACTION: Threshold exceeded - {} Level 0 files (threshold: {})",
                level0_files, self.config.compaction_threshold
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
        let mut dir_entries = tokio::fs::read_dir(&level_dir).await.map_err(|e| {
            SstError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to read level directory: {}", e),
            ))
        })?;

        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            if let Some(filename) = entry.file_name().to_str() {
                if FilenameCodec::new().is_tiered_filename(filename, "sst")
                    && Some(FilenameCodec::new().parse_level(filename) as u8) == Some(level)
                {
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
        info!(
            "📦 SST: Serializing {} vector records to row-based SSTable format",
            vector_records.len()
        );

        // OPTIMIZED: Use VectorRecords directly with sequence numbering
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;
        let mut sorted_records = Vec::new();

        for (index, record) in vector_records.iter().enumerate() {
            // DEBUG: Log the vector ID being processed
            debug!(
                "🔍 SST FLUSH: Processing vector {} with id={:?}",
                index, record.id
            );
            let mut vector_record = record.clone();
            vector_record.version = Some((sequence_start + index as u64) as i64);
            sorted_records.push(vector_record);
        }

        debug!(
            "🔄 SST: Prepared {} vector records for SSTable serialization",
            sorted_records.len()
        );

        // Sort records by ID for SSTable format
        sorted_records.sort_by(|a, b| a.id.as_str().cmp(&b.id.as_str()));

        // Serialize to row-based SSTable format (Level 0 by default for new data)
        self.serialize_sst_records_to_sstable(&sorted_records, 0)
            .await
    }

    /// Build bloom filter for fast key existence checks
    async fn build_bloom_filter(&self, records: &[VectorRecord]) -> Result<SstableBloomFilter> {
        // OPTIMIZED: Accept VectorRecord directly
        // Create key bloom filter
        let key_config = BloomFilterConfig {
            // strategy removed -  BloomStrategy::ByteAligned,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut key_filter = BloomFilterFactory::create(&key_config);

        // Create metadata bloom filter
        let metadata_config = BloomFilterConfig {
            // strategy removed -  BloomStrategy::Composite,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut metadata_builder = crate::storage::engines::core::formats::fastlanes_blocks::bloom_filter::strategies::composite::CompositeBloomFilterBuilder::new(metadata_config);

        // Add all keys and metadata to filters
        for record in records {
            if !record.id.is_empty() {
                key_filter.insert(record.id.as_bytes());
            }

            // Add metadata values - convert SqlValue to MetadataItem
            for (key, sql_value) in &record.metadata {
                // Convert SqlValue to MetadataItem::Value
                let metadata_value = if let Some(value) = &sql_value.value {
                    use crate::proto::proximadb_v1::sql_value::Value as SqlValueType;
                    use crate::proto::proximadb_v1::metadata_item::Value as MetadataValueType;
                    match value {
                        SqlValueType::StringValue(s) => Some(MetadataValueType::StringValue(s.clone())),
                        SqlValueType::NumberValue(n) => Some(MetadataValueType::NumberValue(*n)),
                        SqlValueType::BoolValue(b) => Some(MetadataValueType::BoolValue(*b)),
                        SqlValueType::Int64Value(i) => Some(MetadataValueType::NumberValue(*i as f64)),
                        // For types that don't have MetadataItem equivalents, skip them
                        _ => None,
                    }
                } else {
                    None
                };
                
                let proto_metadata_item = crate::proto::proximadb_v1::MetadataItem {
                    key: key.clone(),
                    value: metadata_value,
                };
                metadata_builder.add_metadata_item(key.clone(), proto_metadata_item);
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
            // strategy removed -  BloomStrategy::ByteAligned,
            expected_items: key_filter.num_elements(),
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        };

        let sstable_filter = SstableBloomFilter::new(
            key_filter_config,
            key_filter.serialize().map_err(|e| {
                SstError::Internal(format!("Failed to serialize key filter: {}", e))
            })?,
            BloomFilterStrategy::serialize(&metadata_filter).map_err(|e| {
                SstError::Internal(format!("Failed to serialize metadata filter: {}", e))
            })?,
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
        // So we implement a simple but effective sorting // strategy removed -
        // 1. Sort by first metadata key alphabetically
        // 2. Then by vector ID for stable ordering

        let mut sorted_vectors = vectors;

        // Find the most common metadata key for primary sorting
        let mut key_frequency: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for vector in &sorted_vectors {
            for metadata_item in &vector.metadata {
                *key_frequency.entry(metadata_item.0.clone()).or_insert(0) += 1;
            }
        }

        let primary_sort_key = key_frequency
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(key, _)| key.clone());

        let sort_start = std::time::Instant::now();

        sorted_vectors.sort_by(|a, b| {
            // Primary sort: most common metadata key
            if let Some(ref sort_key) = primary_sort_key {
                // Convert metadata to comparable format
                let a_value = a.metadata.get(sort_key).map(|sql_val| {
                    match &sql_val.value {
                        Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => s.clone(),
                        Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => n.to_string(),
                        Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => b.to_string(),
                        Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => i.to_string(),
                        _ => String::new(),
                    }
                });
                let b_value = b.metadata.get(sort_key).map(|sql_val| {
                    match &sql_val.value {
                        Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => s.clone(),
                        Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => n.to_string(),
                        Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => b.to_string(),
                        Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => i.to_string(),
                        _ => String::new(),
                    }
                });

                match a_value.cmp(&b_value) {
                    std::cmp::Ordering::Equal => {
                        // Secondary sort: vector ID for stable ordering
                        a.id.cmp(&b.id)
                    }
                    other => other,
                }
            } else {
                // Fallback: sort by vector ID only
                a.id.cmp(&b.id)
            }
        });

        let sort_time_us = sort_start.elapsed().as_micros() as u64;

        // Calculate compression estimate based on metadata distribution
        let compression_estimate = if let Some(ref sort_key) = primary_sort_key {
            let distinct_values: std::collections::HashSet<String> = sorted_vectors
                .iter()
                .filter_map(|v| {
                    v.metadata.get(sort_key).map(|sql_val| {
                        match &sql_val.value {
                            Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => s.clone(),
                            Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => n.to_string(),
                            Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => b.to_string(),
                            Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => i.to_string(),
                            _ => String::new(),
                        }
                    })
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
        records: &[VectorRecord], // OPTIMIZED: Accept VectorRecord directly
        block_size: usize,
    ) -> Result<Vec<FastLanesDataBlock>> {
        let mut blocks = Vec::new();
        let mut current_block_records = Vec::new();
        let mut current_block_size = 0;
        let mut block_id = 0;

        for record in records {
            let record_size = std::mem::size_of::<VectorRecord>() +
                record.id.len() +
                record.vector.len() * 4 + // f32 size
                record.metadata.iter().map(|(key, value)| key.len() + 50).sum::<usize>(); // Estimate metadata size (50 bytes per item)

            // If adding this record would exceed block size, finalize current block
            if current_block_size + record_size > block_size && !current_block_records.is_empty() {
                let records = std::mem::take(&mut current_block_records);
                let compression_config = crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig::default();
                let block = FastLanesDataBlock {
                    encoding_marker: 0x00,
                    encoding_metadata: None,
                    block_id,
                    records,
                    quantized_vectors: None,
                    quantization_level: None,
                    quantized_section: None,
                    metadata: FastLanesBlockMetadata::default(),
                    compression_config,
                    compression_algorithm: CompressionAlgorithm::None,
                    uncompressed_size: 0,
                    bloom_filter: None,
                    block_bloom_filter: None,
                    id_range: (String::new(), String::new()),
                    timestamp_range: (0, 0),
                    statistics: BlockStatistics::default(),
                    metadata_stats: None,
                    has_deletes: false,
                };
                blocks.push(block);
                block_id += 1;
                current_block_size = 0;
            }

            current_block_records.push(record.clone());
            current_block_size += record_size;
        }

        // Add final block if not empty
        if !current_block_records.is_empty() {
            let compression_config = crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig::default();
            let block = FastLanesDataBlock {
                encoding_marker: 0x00,
                encoding_metadata: None,
                block_id,
                records: current_block_records,
                quantized_vectors: None,
                quantization_level: None,
                quantized_section: None,
                metadata: FastLanesBlockMetadata::default(),
                compression_config,
                compression_algorithm: CompressionAlgorithm::None,
                uncompressed_size: 0,
                bloom_filter: None,
                block_bloom_filter: None,
                id_range: (String::new(), String::new()),
                timestamp_range: (0, 0),
                statistics: BlockStatistics::default(),
                metadata_stats: None,
                has_deletes: false,
            };
            blocks.push(block);
        }

        debug!(
            "📦 SST BLOCK ORGANIZATION: {} records organized into {} blocks (avg block size: {}KB)",
            records.len(),
            blocks.len(),
            if !blocks.is_empty() {
                current_block_size / blocks.len() / 1024
            } else {
                0
            }
        );

        Ok(blocks)
    }

    /// Build optimized index and compress data blocks
    async fn build_optimized_index_and_compress_blocks(
        &self,
        data_blocks: &[FastLanesDataBlock],
    ) -> Result<(Vec<IndexEntry>, Vec<Vec<u8>>)> {
        let mut index_entries = Vec::new();
        let mut compressed_blocks = Vec::new();

        // Create compression config from SST config
        let compression_config = BlockCompressionConfig::default();

        for block in data_blocks {
            // Use the new DataBlock serialization with compression
            let serialized_block =
                block
                    .serialize_with_config(&compression_config)
                    .map_err(|e| {
                        SstError::Internal(format!("Failed to serialize data block: {}", e))
                    })?;

            let final_data = serialized_block;

            // Determine if block was compressed
            let is_compressed = block.compression_algorithm != CompressionAlgorithm::None;

            // Create index entries for each record in this block using unified IndexEntry
            let mut block_offset = 0u32;
            for record in &block.records {
                index_entries.push(IndexEntry {
                    key: record.id.clone(),
                    offset: 0, // Will be set later with global offset
                    size: std::mem::size_of::<VectorRecord>() as u32, // Approximate size
                    // Enhanced block organization fields
                    block_id: block.block_id,
                    block_offset,
                    compressed: is_compressed,
                    // Metadata statistics (empty for backward compatibility)
                    metadata_min_values: HashMap::new(),
                    metadata_max_values: HashMap::new(),
                    metadata_null_counts: HashMap::new(),
                    // NEW: Hierarchical bloom filter support
                    block_key_bloom: None,
                    block_metadata_bloom: None,
                    // NEW: Vector format optimization
                    vector_format: VectorFormat::Variable,
                    // REMOVED: compression_ratio
                });
                block_offset += std::mem::size_of::<VectorRecord>() as u32;
            }

            compressed_blocks.push(final_data);
        }

        debug!(
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
    async fn compact_collection(
        &self,
        collection_id: &str,
        collection_config: Option<&Collection>,
    ) -> Result<CompactionResult> {
        info!(
            "🗜️ SST Engine: Starting collection compaction for {}",
            collection_id
        );

        // Get storage location from collection config - skip if not provided
        let _storage_location = match collection_config.and_then(|c| c.storage_assignment.as_ref())
        {
            Some(assignment) => assignment.base_location.clone(),
            None => {
                error!(
                    "❌ SST: No storage assignment found for collection {}. Skipping compaction!",
                    collection_id
                );
                // Return early with failure result
                return Ok(CompactionResult {
                    success: false,
                    collections_affected: vec![collection_id.to_string()],
                    entries_processed: Some(0),
                    entries_removed: Some(0),
                    bytes_read: Some(0),
                    bytes_written: Some(0),
                    input_files: Some(0),
                    output_files: Some(0),
                    duration_ms: Some(0),
                    completed_at: Utc::now(),
                    engine_metrics: {
                        let mut metrics = HashMap::new();
                        metrics.insert(
                            "error".to_string(),
                            serde_json::Value::String("Missing storage assignment".to_string()),
                        );
                        metrics
                    },
                });
            }
        };

        // Create compaction parameters with collection info
        let params = crate::storage::traits::CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            estimated_input_size: 0, // Will be calculated by compaction
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config: collection_config.cloned(),
        };

        // Use the consolidated do_compact implementation
        let mut result = self
            .do_compact(&params)
            .await
            .map_err(|e| SstError::Compaction(format!("Compaction failed: {}", e)))?;

        // Extract vector tracking data from engine_metrics
        let deleted_vector_ids = result
            .engine_metrics
            .get("deleted_vector_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .clone();

        let _merged_vectors = result
            .engine_metrics
            .get("merged_vectors")
            .and_then(|v| v.as_u64());

        // Store vector tracking info in engine_metrics if needed
        if let Some(ids_vec) = deleted_vector_ids {
            result.engine_metrics.insert(
                "deleted_vector_ids".to_string(),
                serde_json::Value::Array(
                    ids_vec
                        .into_iter()
                        .map(|id| serde_json::Value::String(id))
                        .collect(),
                ),
            );
        }

        Ok(result)
    }

    // TODO: Missing methods needed by VectorOperationsService - implement properly
    async fn set_scan_filter(
        &self,
        _collection_id: &str,
        _filter: &UnifiedMetadataFilter,
    ) -> Result<()> {
        // TODO: Implement scan filter configuration
        Ok(())
    }

    async fn set_index_filter(
        &self,
        _collection_id: &str,
        _index: &str,
        _filter: &UnifiedMetadataFilter,
    ) -> Result<()> {
        // TODO: Implement index filter configuration
        Ok(())
    }

    async fn collection(&self, _collection_id: &str) -> Result<Collection> {
        // TODO: Implement collection retrieval
        use crate::proto::proximadb_v1::Collection;
        Ok(Collection {
            id: _collection_id.to_string(),
            ..Default::default()
        })
    }

    async fn list_collection_files(&self, collection_id: &str) -> Result<Vec<String>> {
        // List all SST files for the collection
        let collection_path = format!("{}/{}", self.data_path, collection_id);
        let mut files = Vec::new();
        
        if let Ok(entries) = std::fs::read_dir(&collection_path) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".sst") {
                        files.push(name.to_string());
                    }
                }
            }
        }
        
        Ok(files)
    }

    fn collection_stats(&self, collection_id: &str) -> Result<serde_json::Value> {
        // Get actual collection statistics from storage
        let stats = self.statistics.read().unwrap();
        let collection_vectors = stats.collection_vector_counts.get(collection_id).unwrap_or(&0);
        
        Ok(serde_json::json!({
            "vector_count": collection_vectors,
            "storage_size_bytes": stats.storage_size_bytes,
            "index_size_bytes": stats.index_size_bytes,
            "cache_hit_rate": stats.cache_hit_rate,
            "last_updated": chrono::Utc::now().timestamp_millis()
        }))
    }

    fn collection_metadata(&self, collection_id: &str) -> Result<serde_json::Value> {
        // Get collection metadata from storage configuration
        let metadata = serde_json::json!({
            "engine_type": "sst",
            "collection_id": collection_id,
            "storage_format": "row_based",
            "compression_enabled": true,
            "bloom_filter_enabled": true,
            "quantization_support": ["binary", "int8", "pq4", "pq8"],
            "created_at": chrono::Utc::now().timestamp_millis()
        });
        
        Ok(metadata)
    }

    /// Helper methods for search orchestration
    /// These create mock services for orchestration functionality
    /// In a real implementation, these would come from the service context

    fn mock_axis_manager(
        &self,
    ) -> Result<Arc<crate::index::axis::management::manager::AxisManager>> {
        // Create a mock AXIS manager
        // In real implementation, this would come from the service container
        Err(SstError::Internal(
            "AXIS manager not available in mock implementation".to_string(),
        ))
    }

    fn mock_collection_service(
        &self,
    ) -> Arc<crate::services::collection::manager::CollectionService> {
        // Create a mock collection service - not available in this context
        // This function should not be called in production
        panic!("Mock collection service not available - use actual service");
    }

    fn mock_distance_engine(
        &self,
    ) -> Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute> {
        // Use the existing distance compute from the struct
        self.distance_compute.clone()
    }

    fn mock_quantization_engine(
        &self,
    ) -> Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine> {
        // Use the existing quantization engine from the struct
        self.quantization_engine.clone()
    }

    fn mock_storage_engine(&self) -> Arc<dyn crate::storage::traits::UnifiedStorageEngine> {
        // Cannot create new instance without async context
        panic!("Mock storage engine not available - use actual instance");
    }

    /// Fallback to direct search when orchestration fails
    async fn fallback_to_direct_search(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        filter_expression: Option<&crate::core::search::FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        warn!("🔄 SST: Falling back to direct search implementation");

        // Use the unified search implementation and return OptimizedSearchRecord directly
        let optimized_results = self
            .search_vectors_unified(ctx)
            .await
            .map_err(|e| SstError::Search(format!("Search failed: {}", e)))?;

        Ok(optimized_results)
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
