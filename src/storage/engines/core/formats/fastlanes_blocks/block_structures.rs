//! # FastLanes Block Structures - High-Performance Columnar Storage Format
//!
//! This module implements the FastLanes block format, a SIMD-optimized columnar storage
//! format shared between SST and SWIFT storage engines. FastLanes provides efficient
//! encoding, compression, and access patterns for vector data.
//!
//! ## FastLanes Encoding Philosophy
//!
//! FastLanes is designed around SIMD-friendly data layouts that enable:
//! - **Vectorized Operations**: Process multiple values in single CPU instructions
//! - **Cache-Friendly Access**: Sequential memory access patterns
//! - **Compression-Aware**: Encoding schemes that compress well
//! - **Zero-Copy Deserialization**: Direct memory mapping when possible
//!
//! ## Block Structure Overview
//!
//! ```text
//! ┌───────────────────────────────────────────────────────┐
//! │                    FastLanes Data Block                │
//! ├───────────────────────────────────────────────────────┤
//! │ [1 byte]  Encoding Marker (BitPacked/Delta/etc)       │
//! │ [4 bytes] Block ID (u32)                              │
//! │ [Variable] Encoding Metadata                          │
//! ├───────────────────────────────────────────────────────┤
//! │           Vector Data (Columnar Layout)               │
//! │ ┌──────────────────────────────────────────────────┐    │
//! │ │ Dimension 0: [v0_d0, v1_d0, v2_d0, ...]       │    │
//! │ │ Dimension 1: [v0_d1, v1_d1, v2_d1, ...]       │    │
//! │ │ ...                                            │    │
//! │ │ Dimension N: [v0_dN, v1_dN, v2_dN, ...]       │    │
//! │ └──────────────────────────────────────────────────┘    │
//! ├───────────────────────────────────────────────────────┤
//! │           Quantized Vectors (Optional)                │
//! │ ┌──────────────────────────────────────────────────┐    │
//! │ │ Binary: 1-bit per dimension                    │    │
//! │ │ INT8: 8-bit quantized values                   │    │
//! │ │ PQ: Product quantization codes                 │    │
//! │ └──────────────────────────────────────────────────┘    │
//! ├───────────────────────────────────────────────────────┤
//! │ Metadata (IDs, timestamps, custom fields)             │
//! │ Bloom Filter (for existence checks)                   │
//! │ Statistics (min/max, cardinality, etc)                │
//! └───────────────────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::core::bloom::SstableBloomFilter;
use crate::core::{VectorRecord, compression::CompressionAlgorithm};
use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme;
// Quantization now handled by unified compute module

/// FastLanes encoding metadata for efficient vector block encoding
///
/// This metadata structure contains all information needed to decode
/// a FastLanes-encoded block. Different encoding schemes require different
/// metadata fields, hence the use of Option types.
///
/// ## Encoding Schemes Supported:
/// - **BitPacked**: Dense packing of integers using minimum bits
/// - **Delta**: Store deltas from previous value
/// - **FrameOfReference**: Delta from a base value
/// - **PatchedBase**: Base encoding with exceptions
/// - **Dictionary**: Replace values with dictionary indices
/// - **RunLength**: Compress runs of identical values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastLanesMetadata {
    /// Encoding scheme used for this block
    pub scheme: FastLanesScheme,
    /// Original dimension of vectors
    pub dimension: usize,
    /// Number of vectors in this block
    pub vector_count: usize,
    /// Bits per value (for BitPacked encoding)
    /// Example: If values fit in 12 bits, we pack them densely
    pub bits_per_value: Option<u8>,
    /// Base value (for Delta/FrameOfReference)
    /// All values are stored as offsets from this base
    pub base_value: Option<i64>,
    /// Dictionary size (for Dictionary encoding)
    /// Number of unique values in the dictionary
    pub dict_size: Option<usize>,
    /// Patch count (for PatchedBase)
    /// Number of exceptions that don't fit base encoding
    pub patch_count: Option<usize>,
    /// Statistics for adaptive decoding
    pub min_value: f32,       // Minimum value in block
    pub max_value: f32,       // Maximum value in block
    pub range_bits: u8,       // Bits needed for value range
    /// Compression ratio achieved (original_size / compressed_size)
    pub compression_ratio: f32,
}

/// Quantized section for hierarchical storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedSection {
    pub binary_vectors: Option<Vec<Vec<u8>>>,
    pub int8_vectors: Option<Vec<Vec<i8>>>,
    pub pq_vectors: Option<Vec<Vec<u8>>>,
    pub codebooks: Option<Vec<Vec<f32>>>,
}

/// Block metadata statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMetadataStats {
    pub unique_keys: u32,
    pub null_values: u32,
    pub avg_value_size: f32,
    pub compression_ratio: f32,
}

/// Shared data block structure using FastLanes columnar encoding
/// 
/// This is the core data structure for storing vectors in both SST and SWIFT engines.
/// It provides SIMD-optimized columnar storage with multiple encoding schemes,
/// quantization levels, and compression algorithms.
///
/// ## Design Principles:
/// 1. **Columnar Layout**: Store each dimension separately for SIMD processing
/// 2. **Adaptive Encoding**: Choose best encoding based on data characteristics
/// 3. **Progressive Refinement**: Support multiple quantization levels
/// 4. **Zero-Copy**: Enable direct memory mapping when possible
/// 5. **Extensibility**: Encoding marker allows future format evolution
#[derive(Debug, Clone)]
pub struct FastLanesDataBlock {
    /// FASTLANES ENCODING MARKER (1 byte) - First byte of serialized block
    /// 
    /// This marker identifies the encoding scheme used for the block.
    /// Format: [7:4] Major encoding type | [3:0] Sub-variant
    /// 
    /// Encoding Types:
    /// - 0x00: Raw/Uncompressed (backward compatible)
    /// - 0x10-0x1F: FastLanes BitPacked variants (pack integers using minimum bits)
    /// - 0x20-0x2F: FastLanes Delta encoding (store differences)
    /// - 0x30-0x3F: FastLanes FrameOfReference (delta from base value)
    /// - 0x40-0x4F: FastLanes PatchedBase (base + exceptions)
    /// - 0x50-0x5F: FastLanes Dictionary (replace with indices)
    /// - 0x60-0x6F: FastLanes RunLength (compress repeated values)
    /// - 0x70-0x7F: Reserved for future encoding schemes
    pub encoding_marker: u8,

    /// FastLanes encoding metadata (when marker != 0x00)
    pub encoding_metadata: Option<FastLanesMetadata>,

    /// Block identification - u32 supports 4.3 billion blocks
    /// 
    /// ## Capacity Planning:
    /// - u32 = 4,294,967,296 blocks maximum
    /// - With 1000 vectors/block = 4.3 trillion vectors
    /// - With 384D vectors @ 2KB each = 8.6 petabytes
    /// 
    /// ## Why u32 instead of u16?
    /// - u16 limit: 65,536 blocks = 65M vectors (too small)
    /// - Real-world: 100GB file = 100,000 blocks (exceeds u16)
    /// - u32 provides headroom for future growth
    /// - 4 bytes overhead is negligible vs block size
    pub block_id: u32,

    /// Data organization
    pub records: Vec<VectorRecord>,
    /// Quantized vectors using unified engine
    pub quantized_vectors: Option<Vec<Vec<u8>>>,
    /// Quantization level used
    pub quantization_level: Option<crate::compute::quantization::unified::UnifiedQuantizationLevel>,

    /// Quantized section for hierarchical storage (SST/Swift specific)
    pub quantized_section: Option<QuantizedSection>,

    /// Block metadata
    pub metadata: FastLanesBlockMetadata,

    /// Compression information
    pub compression_config: BlockCompressionConfig,

    /// Direct compression algorithm field (for SST compatibility)
    pub compression_algorithm: CompressionAlgorithm,

    /// Uncompressed size (for SST compatibility)
    pub uncompressed_size: u64,

    /// Index structures
    pub bloom_filter: Option<SstableBloomFilter>,

    /// Additional bloom filter field for SST (block-level bloom)
    pub block_bloom_filter: Option<SstableBloomFilter>,

    /// ID and timestamp ranges
    pub id_range: (String, String),
    pub timestamp_range: (i64, i64),

    /// Performance tracking
    pub statistics: BlockStatistics,

    /// Metadata statistics (for SST compatibility)
    pub metadata_stats: Option<BlockMetadataStats>,

    /// Track if block has deletes (for SST compatibility)
    pub has_deletes: bool,
}

/// Block metadata for FastLanes encoded blocks
/// Shared between SST and SWIFT engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastLanesBlockMetadata {
    /// Basic information
    pub record_count: u32,
    pub size_bytes: u64,
    pub compressed_size: u64,
    pub timestamp: i64,
    pub compaction_level: u8,

    /// Data characteristics
    pub has_deletes: bool,
    pub has_updates: bool,
    pub version_range: (i64, i64),

    /// Column statistics for metadata filtering
    pub column_stats: HashMap<String, ColumnStatistics>,

    /// Quantization information
    pub quantization_stats: QuantizationStatistics,

    /// Checksums for integrity
    pub data_checksum: u64,
    pub metadata_checksum: u32,
}

/// Column statistics for optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStatistics {
    pub name: String,
    pub null_count: u32,
    pub distinct_count: u32,
    pub min_value: Option<serde_json::Value>,
    pub max_value: Option<serde_json::Value>,
    pub avg_size_bytes: u64,
    pub bloom_filter_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnData {
    String,
    Integer,
    Float,
    Boolean,
    Timestamp,
    Json,
}

/// Quantization statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStatistics {
    pub has_binary: bool,
    pub has_int8: bool,
    pub has_pq: bool,
    pub compression_ratio: f32,
    pub memory_savings_percent: f32,
    pub reconstruction_error: f32,
    pub quantization_time_ms: u64,
}

/// Block compression configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockCompressionConfig {
    pub algorithm: CompressionAlgorithm,
    pub compression_level: u8,
    pub enable_vector_compression: bool,
    pub enable_metadata_compression: bool,
    pub compression_threshold_bytes: usize,
    pub dictionary_compression: bool,
}

/// Block statistics for performance monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockStatistics {
    pub read_count: u64,
    pub write_count: u64,
    pub search_count: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub avg_read_time_ms: f64,
    pub avg_search_time_ms: f64,
    pub last_accessed_at: i64,
}

/// SuperBlock structure for hierarchical organization
#[derive(Debug)]
pub struct SuperBlock {
    /// SUPERBLOCK ENCODING MARKER (for SWIFT hierarchical encoding)
    /// 0x80-0x8F: SWIFT SuperBlock encodings
    /// 0xFF: Inherit from child blocks (mixed encoding)
    pub superblock_encoding_marker: u8,

    /// SuperBlock-level FastLanes metadata (when using unified encoding)
    pub superblock_encoding_metadata: Option<FastLanesMetadata>,

    /// SuperBlock identification
    pub id: u32,
    pub file_path: String,
    pub timestamp: i64,

    /// Organization
    pub blocks: Vec<FastLanesDataBlock>,
    pub total_size_bytes: u64,
    pub compressed_size_bytes: u64,

    /// SuperBlock-level metadata
    pub record_count: u64,
    pub id_range: (String, String),
    pub timestamp_range: (i64, i64),

    /// SuperBlock-level indexes
    pub centroid: Option<Vec<f32>>,
    pub quantized_signature: Vec<u8>,
    pub bloom_filter: Option<SstableBloomFilter>,

    /// Performance optimization
    pub layout: BlockLayout,
    pub access_pattern: AccessPattern,
}

/// Block layout configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLayout {
    /// Layout strategy
    pub layout_type: LayoutType,

    /// Block organization
    pub blocks_per_superblock: u32,
    pub records_per_block: u32,
    pub target_block_size_bytes: u64,

    /// Alignment and padding
    pub block_alignment_bytes: usize,
    pub enable_padding: bool,
    pub padding_strategy: PaddingStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LayoutType {
    /// Sequential layout for streaming access
    Sequential,
    /// Interleaved layout for random access
    Interleaved,
    /// Hierarchical layout for multi-level access
    Hierarchical,
    /// Adaptive layout based on access patterns
    Adaptive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaddingStrategy {
    /// No padding
    None,
    /// Align to block boundaries
    BlockAlign,
    /// Align to page boundaries (4KB)
    PageAlign,
    /// Align to memory pages (64KB)
    MemoryAlign,
}

/// Access pattern tracking for optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessPattern {
    pub pattern_type: AccessPatternType,
    pub frequency: HashMap<String, u64>,
    pub temporal_locality: f64,
    pub spatial_locality: f64,
    pub read_write_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPatternType {
    Sequential,
    Random,
    Hotspot,
    Scan,
    Mixed,
}

/// Block location for ID indexing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLocation {
    pub superblock_id: u32,
    pub block_id: u32,
    pub block_offset: u64,
    pub record_offset: u32,
    pub estimated_load_time_ms: f32,
}

impl FastLanesDataBlock {
    /// Create a new data block
    pub fn new(records: Vec<VectorRecord>, compression_config: BlockCompressionConfig) -> Self {
        let record_count = records.len() as u32;
        // Use a simple counter or provided ID - will be set properly by the writer
        let block_id = 0u32;

        // Calculate ID range
        let mut ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
        ids.sort();
        let id_range = if ids.is_empty() {
            ("".to_string(), "".to_string())
        } else {
            (ids[0].clone(), ids[ids.len() - 1].clone())
        };

        // Calculate timestamp range
        let timestamps: Vec<i64> = records.iter().map(|r| r.timestamp as i64).collect();
        let timestamp_range = if timestamps.is_empty() {
            (0, 0)
        } else {
            (
                *timestamps.iter().min().unwrap(),
                *timestamps.iter().max().unwrap(),
            )
        };

        // Check for deletes (tombstone records)
        let has_deletes = records.iter().any(|r| {
            r.metadata.iter().any(|kv| {
            kv.key == "_deleted" && matches!(
                kv.value.as_ref(), 
                Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(s)) if s == "true"
            )
        })
        });

        // Analyze vectors to choose optimal encoding
        let encoding_marker = Self::choose_optimal_encoding_marker(&records);
        let encoding_metadata = if encoding_marker != 0x00 {
            Some(Self::create_encoding_metadata(&records, encoding_marker))
        } else {
            None
        };

        Self {
            encoding_marker,
            encoding_metadata,
            block_id,
            records: records.clone(),
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: FastLanesBlockMetadata {
                record_count,
                size_bytes: 0, // Will be calculated
                compressed_size: 0,
                timestamp: chrono::Utc::now().timestamp(),
                compaction_level: 0,
                has_deletes,
                has_updates: false,
                version_range: (0, 0),
                column_stats: HashMap::new(),
                quantization_stats: QuantizationStatistics::default(),
                data_checksum: 0,
                metadata_checksum: 0,
            },
            compression_config: compression_config.clone(),
            compression_algorithm: compression_config.algorithm,
            uncompressed_size: 0, // Will be calculated during compression
            bloom_filter: None,
            block_bloom_filter: None,
            id_range,
            timestamp_range,
            statistics: BlockStatistics::default(),
            metadata_stats: None,
            has_deletes,
        }
    }

    /// Get record by index
    pub fn get_record(&self, index: usize) -> Option<&VectorRecord> {
        self.records.get(index)
    }

    /// Find record by ID
    pub fn find_record_by_id(&self, id: &str) -> Option<&VectorRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Check if block contains ID (using bloom filter if available)
    pub fn contains_id(&self, id: &str) -> bool {
        if let Some(ref bloom) = self.bloom_filter {
            bloom.might_contain_key(id).unwrap_or(true)
        } else {
            self.find_record_by_id(id).is_some()
        }
    }

    /// Get memory usage estimate
    pub fn memory_usage_bytes(&self) -> usize {
        let records_size = self.records.len() * std::mem::size_of::<VectorRecord>();
        let quantized_size = self
            .quantized_vectors
            .as_ref()
            .map(|qv| qv.iter().map(|v| v.len()).sum())
            .unwrap_or(0);
        let metadata_size = std::mem::size_of::<FastLanesBlockMetadata>();

        records_size + quantized_size + metadata_size
    }

    /// Update access statistics
    pub fn update_access_stats(&mut self, operation: &str) {
        match operation {
            "read" => self.statistics.read_count += 1,
            "write" => self.statistics.write_count += 1,
            "search" => self.statistics.search_count += 1,
            _ => {}
        }
        self.statistics.last_accessed_at = chrono::Utc::now().timestamp();
    }

    /// Choose optimal encoding based on vector statistics
    fn choose_optimal_encoding_marker(records: &[VectorRecord]) -> u8 {
        if records.is_empty() || records[0].vector.is_empty() {
            return 0x00; // Raw for empty blocks
        }

        // Analyze vector statistics
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;
        let mut total_delta = 0.0f32;
        let mut prev_values: Option<Vec<f32>> = None;

        for record in records {
            for (i, &val) in record.vector.iter().enumerate() {
                min_val = min_val.min(val);
                max_val = max_val.max(val);

                // Calculate delta for adjacent vectors
                if let Some(ref prev) = prev_values {
                    if i < prev.len() {
                        total_delta += (val - prev[i]).abs();
                    }
                }
            }
            prev_values = Some(record.vector.clone());
        }

        let range = max_val - min_val;
        let avg_delta = if records.len() > 1 {
            total_delta / (records.len() as f32 * records[0].vector.len() as f32)
        } else {
            range
        };

        // Decision tree for encoding selection
        if range < 1e-6 {
            // Near-constant values
            0x60 // RunLength encoding
        } else if avg_delta < range / 4.0 {
            // Strong temporal correlation
            0x20 // Delta encoding
        } else if range < 100.0 && min_val.abs() < 1000.0 {
            // Small range, use FrameOfReference
            0x30 // FrameOfReference
        } else {
            // Default to BitPacking for general case
            0x10 // BitPacked - most versatile for SIMD
        }
    }

    /// Create encoding metadata for the chosen scheme
    fn create_encoding_metadata(records: &[VectorRecord], marker: u8) -> FastLanesMetadata {
        let dimension = if !records.is_empty() {
            records[0].vector.len()
        } else {
            0
        };

        // Calculate statistics
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;
        for record in records {
            for &val in &record.vector {
                min_val = min_val.min(val);
                max_val = max_val.max(val);
            }
        }

        let range = max_val - min_val;
        let range_bits = if range > 0.0 {
            (range.log2().ceil() as u8).max(1)
        } else {
            1
        };

        // Determine scheme from marker
        let scheme = match marker & 0xF0 {
            0x10 => FastLanesScheme::BitPacked { bits: range_bits },
            0x20 => FastLanesScheme::Delta {
                base: min_val as i64,
            },
            0x30 => FastLanesScheme::FrameOfReference {
                reference: min_val as i64,
                bits: range_bits,
            },
            0x40 => FastLanesScheme::PatchedBase {
                base: ((min_val + max_val) / 2.0) as i64,
                patch_bits: 8,
            },
            0x50 => FastLanesScheme::Dictionary,
            0x60 => FastLanesScheme::RunLength,
            _ => FastLanesScheme::BitPacked { bits: 8 }, // Default to 8-bit packing
        };

        FastLanesMetadata {
            scheme,
            dimension,
            vector_count: records.len(),
            bits_per_value: Some(range_bits),
            base_value: Some(min_val as i64),
            dict_size: None,
            patch_count: None,
            min_value: min_val,
            max_value: max_val,
            range_bits,
            compression_ratio: 1.0, // Will be calculated during actual encoding
        }
    }

    /// Serialize the block with optional compression
    /// Delegates encoding to the fastlanes module
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        self.serialize_with_config(&self.compression_config)
    }

    /// Serialize with specific compression configuration
    /// Uses optimized columnar compression with dimension grouping and sparse metadata
    pub fn serialize_with_config(
        &self,
        _config: &BlockCompressionConfig,
    ) -> anyhow::Result<Vec<u8>> {
        use crate::core::compression::CompressionAlgorithm;
        use crate::core::compression::{CompressionContext, compress};
        use crate::storage::engines::core::ops::fastlanes_encoding::{FastLanesEncoder, markers};
        use std::collections::{HashMap, HashSet};
        use std::io::Write;

        let mut result = Vec::new();

        // Write format version for backward compatibility
        const COLUMNAR_FORMAT_VERSION: u8 = 1; // Version 1 = initial release
        result.push(COLUMNAR_FORMAT_VERSION);
        result.push(self.encoding_marker);

        if self.records.is_empty() {
            result.write_all(&0u32.to_le_bytes())?; // Zero records
            return Ok(result);
        }

        // Write record count and dimension
        result.write_all(&(self.records.len() as u32).to_le_bytes())?;
        let dimension = self.records[0].vector.len();
        result.write_all(&(dimension as u32).to_le_bytes())?;

        // ============ STEP 1: Encode vectors using FastLanes dual-mode encoding ============
        // Initialize encoder - delegate to fastlanes_encoding module
        let encoder = if self.encoding_marker != 0x00 {
            FastLanesEncoder::new(markers::to_scheme(self.encoding_marker).unwrap_or(
                crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme::Delta {
                    base: 0,
                },
            ))
        } else {
            // Default to delta encoding for better compression
            FastLanesEncoder::new(
                crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme::Delta {
                    base: 0,
                },
            )
        };

        // Collect vectors from records
        let vectors: Vec<Vec<f32>> = self.records.iter().map(|r| r.vector.clone()).collect();

        // Choose encoding layout based on dimension and configuration
        // Use columnar for better compression on low-medium dimensions
        // Use row-wise for high dimensions to speed up reconstruction
        let encoded_vectors = if dimension <= 512 {
            // Columnar layout for better compression
            let columnar = encoder.encode_vectors_columnar(&vectors, 64)?;
            // Serialize columnar format
            let mut bytes = Vec::new();
            bytes.extend(&(dimension as u32).to_le_bytes());
            bytes.extend(&(vectors.len() as u32).to_le_bytes());
            for group in &columnar.dimension_groups {
                // Serialize each dimension in the group
                for dim in &group.dimensions {
                    bytes.extend(&(dim.encoded_data.len()).to_le_bytes());
                    bytes.extend(&dim.encoded_data);
                }
            }
            bytes
        } else {
            // Row-wise for medium-high dimensions
            let rowwise = encoder.encode_vectors_rowwise(&vectors, dimension <= 2048)?;
            // Concatenate all encoded vectors
            let mut all_bytes = Vec::new();
            for vec_data in &rowwise.encoded_vectors {
                all_bytes.extend(vec_data);
            }
            all_bytes
        };

        // Write encoded vectors
        result.write_all(&(encoded_vectors.len() as u32).to_le_bytes())?;
        result.write_all(&encoded_vectors)?;

        // ============ STEP 2: Encode IDs using FastLanes dictionary encoding ============
        let mut unique_ids = HashSet::new();
        for record in &self.records {
            unique_ids.insert(record.id.clone());
        }

        let id_dictionary: Vec<String> = unique_ids.into_iter().collect();
        let id_lookup: HashMap<String, i64> = id_dictionary
            .iter()
            .enumerate()
            .map(|(idx, id)| (id.clone(), idx as i64))
            .collect();

        // Write dictionary
        result.write_all(&(id_dictionary.len() as u32).to_le_bytes())?;
        for id in &id_dictionary {
            let bytes = id.as_bytes();
            result.write_all(&(bytes.len() as u32).to_le_bytes())?;
            result.write_all(bytes)?;
        }

        // Collect indices and encode using FastLanes delta encoding
        let id_indices: Vec<i64> = self
            .records
            .iter()
            .map(|record| *id_lookup.get(&record.id).unwrap())
            .collect();

        let encoded_ids = encoder.encode_i64(&id_indices)?;
        result.write_all(&(encoded_ids.len() as u32).to_le_bytes())?;
        result.write_all(&encoded_ids)?;

        // ============ STEP 3: Build sparse metadata columns ============
        let mut metadata_keys = HashSet::new();
        for record in &self.records {
            for item in &record.metadata {
                metadata_keys.insert(item.key.clone());
            }
        }

        let metadata_key_list: Vec<String> = metadata_keys.into_iter().collect();
        result.write_all(&(metadata_key_list.len() as u32).to_le_bytes())?;

        for key in &metadata_key_list {
            // Write key name
            let key_bytes = key.as_bytes();
            result.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            result.write_all(key_bytes)?;

            // Build sparse column for this key
            let mut sparse_values = Vec::new();
            let mut presence_bitmap = vec![0u8; (self.records.len() + 7) / 8];

            for (idx, record) in self.records.iter().enumerate() {
                if let Some(item) = record.metadata.iter().find(|m| m.key == *key) {
                    // Set bit in presence bitmap
                    presence_bitmap[idx / 8] |= 1 << (idx % 8);

                    // Serialize value
                    if let Some(value) = &item.value {
                        
                        // Encode the metadata value based on its type
                        let value_bytes = match value {
                            crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => s.as_bytes().to_vec(),
                            crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n) => n.to_le_bytes().to_vec(),
                            crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b) => vec![if *b { 1 } else { 0 }],
                        };
                        sparse_values.write_all(&(value_bytes.len() as u32).to_le_bytes())?;
                        sparse_values.write_all(&value_bytes)?;
                    } else {
                        sparse_values.write_all(&0u32.to_le_bytes())?;
                    }
                }
            }

            // Write presence bitmap
            result.write_all(&(presence_bitmap.len() as u32).to_le_bytes())?;
            result.write_all(&presence_bitmap)?;

            // Compress and write sparse values
            if !sparse_values.is_empty() {
                let compressed_values = compress(
                    &sparse_values,
                    CompressionAlgorithm::Zstd,
                    3,
                    CompressionContext::Block,
                )?;
                result.write_all(&(compressed_values.len()).to_le_bytes())?;
                result.write_all(&compressed_values)?;
            } else {
                result.write_all(&0u32.to_le_bytes())?;
            }
        }

        // ============ STEP 4: Encode timestamps using FastLanes ============
        let timestamps: Vec<i64> = self
            .records
            .iter()
            .map(|record| record.updated_at.unwrap_or(0) as i64)
            .collect();

        let encoded_timestamps = encoder.encode_i64(&timestamps)?;
        result.write_all(&(encoded_timestamps.len() as u32).to_le_bytes())?;
        result.write_all(&encoded_timestamps)?;

        // ============ STEP 5: Write block metadata ============
        let metadata_bytes = bincode::serialize(&self.metadata)?;
        result.write_all(&(metadata_bytes.len() as u32).to_le_bytes())?;
        result.write_all(&metadata_bytes)?;

        Ok(result)
    }

    /// Deserialize a block
    /// Delegates decoding to the fastlanes module
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use crate::core::compression::CompressionContext;
        use crate::storage::engines::core::ops::fastlanes_encoding::{FastLanesDecoder, markers};
        use std::io::Read;

        if data.is_empty() {
            return Err(anyhow::anyhow!(
                "Empty data for FastLanesDataBlock deserialization"
            ));
        }

        let marker = data[0];
        let data = &data[1..];

        // Check for compression
        let decompressed_data = if marker >= 0x80 && marker < 0xA0 {
            // Compressed data
            let mut cursor = std::io::Cursor::new(data);
            let mut size_bytes = [0u8; 4];
            cursor.read_exact(&mut size_bytes)?;
            let original_size = u32::from_le_bytes(size_bytes) as usize;

            let compressed_data = &data[4..];
            // Map marker to compression algorithm
            let algorithm = match marker {
                0x10 => CompressionAlgorithm::Lz4,
                0x11 => CompressionAlgorithm::Zstd,
                0x12 => CompressionAlgorithm::Snappy,
                0x13 => CompressionAlgorithm::Gzip,
                _ => CompressionAlgorithm::None,
            };

            crate::core::compression::decompress(
                compressed_data,
                algorithm,
                CompressionContext::Block,
            )?
        } else {
            data.to_vec()
        };

        let mut cursor = std::io::Cursor::new(&decompressed_data);

        // Read metadata length and deserialize
        let mut len_bytes = [0u8; 4];
        cursor.read_exact(&mut len_bytes)?;
        let metadata_len = u32::from_le_bytes(len_bytes) as usize;

        let mut metadata_bytes = vec![0u8; metadata_len];
        cursor.read_exact(&mut metadata_bytes)?;
        let metadata: FastLanesBlockMetadata = bincode::deserialize(&metadata_bytes)?;

        // Decode vectors based on marker
        let records = if marker != 0x00 && marker < 0x80 {
            // FastLanes encoded
            let decoder = FastLanesDecoder::new(
                markers::to_scheme(marker).unwrap_or(
                    crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme::BitPacked { bits: 16 }
                )
            );

            // Read dimensions and count
            let mut dim_bytes = [0u8; 4];
            cursor.read_exact(&mut dim_bytes)?;
            let dimension = u32::from_le_bytes(dim_bytes) as usize;

            let mut count_bytes = [0u8; 4];
            cursor.read_exact(&mut count_bytes)?;
            let vector_count = u32::from_le_bytes(count_bytes) as usize;

            // Decode columns
            let mut columns: Vec<Vec<f32>> = Vec::with_capacity(dimension);
            for _ in 0..dimension {
                let mut col_len_bytes = [0u8; 4];
                cursor.read_exact(&mut col_len_bytes)?;
                let col_len = u32::from_le_bytes(col_len_bytes) as usize;

                let mut col_data = vec![0u8; col_len];
                cursor.read_exact(&mut col_data)?;

                // Each column contains `vector_count` floats
                let column = decoder.decode_f32(&col_data, vector_count)?;
                columns.push(column);
            }

            // Transpose back to row-major
            let mut records = Vec::with_capacity(vector_count);
            for i in 0..vector_count {
                let mut vector = Vec::with_capacity(dimension);
                for col in &columns {
                    vector.push(col[i]);
                }

                records.push(VectorRecord {
                    id: format!("record_{}", i), // Will be updated from metadata
                    vector,
                    metadata: Vec::new(),
                    timestamp: 0,
                    updated_at: None,
                    quantized_vector: None,
                    expires_at: None,
                    version: None,
                    source: None,
                });
            }

            records
        } else {
            // Raw encoding
            let mut count_bytes = [0u8; 4];
            cursor.read_exact(&mut count_bytes)?;
            let record_count = u32::from_le_bytes(count_bytes) as usize;

            let mut records = Vec::with_capacity(record_count);
            for _ in 0..record_count {
                let mut len_bytes = [0u8; 4];
                cursor.read_exact(&mut len_bytes)?;
                let record_len = u32::from_le_bytes(len_bytes) as usize;

                let mut record_data = vec![0u8; record_len];
                cursor.read_exact(&mut record_data)?;

                use prost::Message;
                let record = VectorRecord::decode(&record_data[..])?;
                records.push(record);
            }

            records
        };

        // Reconstruct the block
        let block_id = metadata.record_count;
        let has_deletes = metadata.has_deletes;
        Ok(Self {
            encoding_marker: marker,
            encoding_metadata: None, // Will be reconstructed if needed
            block_id,
            records,
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata,
            compression_config: BlockCompressionConfig::default(),
            compression_algorithm: CompressionAlgorithm::None,
            uncompressed_size: 0,
            bloom_filter: None,
            block_bloom_filter: None,
            id_range: ("".to_string(), "".to_string()),
            timestamp_range: (0, 0),
            statistics: BlockStatistics::default(),
            metadata_stats: None,
            has_deletes,
        })
    }
}

impl SuperBlock {
    /// Create a new SuperBlock
    pub fn new(id: u32, file_path: String) -> Self {
        Self {
            superblock_encoding_marker: 0xFF, // Default to inherit from child blocks
            superblock_encoding_metadata: None,
            id,
            file_path,
            timestamp: chrono::Utc::now().timestamp(),
            blocks: Vec::new(),
            total_size_bytes: 0,
            compressed_size_bytes: 0,
            record_count: 0,
            id_range: ("".to_string(), "".to_string()),
            timestamp_range: (0, 0),
            centroid: None,
            quantized_signature: Vec::new(),
            bloom_filter: None,
            layout: BlockLayout::default(),
            access_pattern: AccessPattern::default(),
        }
    }

    /// Add a block to the SuperBlock
    pub fn add_block(&mut self, block: FastLanesDataBlock) {
        self.record_count += block.metadata.record_count as u64;
        self.total_size_bytes += block.metadata.size_bytes;
        self.compressed_size_bytes += block.metadata.compressed_size;

        // Update ID range
        if self.blocks.is_empty() {
            self.id_range = block.id_range.clone();
        } else {
            if block.id_range.0 < self.id_range.0 {
                self.id_range.0 = block.id_range.0.clone();
            }
            if block.id_range.1 > self.id_range.1 {
                self.id_range.1 = block.id_range.1.clone();
            }
        }

        // Update timestamp range
        if self.blocks.is_empty() {
            self.timestamp_range = block.timestamp_range;
        } else {
            self.timestamp_range.0 = self.timestamp_range.0.min(block.timestamp_range.0);
            self.timestamp_range.1 = self.timestamp_range.1.max(block.timestamp_range.1);
        }

        self.blocks.push(block);
    }

    /// Get compression ratio for the SuperBlock
    pub fn compression_ratio(&self) -> f32 {
        if self.total_size_bytes == 0 {
            1.0
        } else {
            self.compressed_size_bytes as f32 / self.total_size_bytes as f32
        }
    }
}

impl Default for BlockLayout {
    fn default() -> Self {
        Self {
            layout_type: LayoutType::Hierarchical,
            blocks_per_superblock: 64,
            records_per_block: 2000,
            target_block_size_bytes: 16 * 1024 * 1024, // 16MB
            block_alignment_bytes: 4096,               // 4KB alignment
            enable_padding: true,
            padding_strategy: PaddingStrategy::BlockAlign,
        }
    }
}

impl Default for BlockCompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 8192, // 8KB
            dictionary_compression: false,
        }
    }
}

impl Default for QuantizationStatistics {
    fn default() -> Self {
        Self {
            has_binary: false,
            has_int8: false,
            has_pq: false,
            compression_ratio: 1.0,
            memory_savings_percent: 0.0,
            reconstruction_error: 0.0,
            quantization_time_ms: 0,
        }
    }
}

impl Default for BlockStatistics {
    fn default() -> Self {
        Self {
            read_count: 0,
            write_count: 0,
            search_count: 0,
            cache_hits: 0,
            cache_misses: 0,
            avg_read_time_ms: 0.0,
            avg_search_time_ms: 0.0,
            last_accessed_at: chrono::Utc::now().timestamp(),
        }
    }
}

impl Default for AccessPattern {
    fn default() -> Self {
        Self {
            pattern_type: AccessPatternType::Mixed,
            frequency: HashMap::new(),
            temporal_locality: 0.5,
            spatial_locality: 0.5,
            read_write_ratio: 1.0,
        }
    }
}

impl Default for QuantizedSection {
    fn default() -> Self {
        Self {
            binary_vectors: None,
            int8_vectors: None,
            pq_vectors: None,
            codebooks: None,
        }
    }
}

impl Default for BlockMetadataStats {
    fn default() -> Self {
        Self {
            unique_keys: 0,
            null_values: 0,
            avg_value_size: 0.0,
            compression_ratio: 1.0,
        }
    }
}

impl Default for FastLanesBlockMetadata {
    fn default() -> Self {
        Self {
            record_count: 0,
            size_bytes: 0,
            compressed_size: 0,
            timestamp: 0,
            compaction_level: 0,
            has_deletes: false,
            has_updates: false,
            version_range: (0, 0),
            column_stats: HashMap::new(),
            quantization_stats: QuantizationStatistics::default(),
            data_checksum: 0,
            metadata_checksum: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_block_creation() {
        let records = vec![
            VectorRecord {
                id: Some("vec_1".to_string()),
                vector: vec![1.0, 2.0, 3.0],
                timestamp: 1000,
                ..Default::default()
            },
            VectorRecord {
                id: Some("vec_2".to_string()),
                vector: vec![4.0, 5.0, 6.0],
                timestamp: 2000,
                ..Default::default()
            },
        ];

        let compression_config = BlockCompressionConfig::default();

        let block = RowBasedDataBlock::new(records, compression_config);

        assert_eq!(block.metadata.record_count, 2);
        assert_eq!(block.id_range.0, "vec_1");
        assert_eq!(block.id_range.1, "vec_2");
        assert_eq!(block.timestamp_range, (1000, 2000));
    }

    #[test]
    fn test_superblock_management() {
        let mut superblock = SuperBlock::new(1, "/path/to/file".to_string());

        let block = RowBasedDataBlock::new(
            vec![VectorRecord::default()],
            BlockCompressionConfig::default(),
        );

        superblock.add_block(block);

        assert_eq!(superblock.blocks.len(), 1);
        assert_eq!(superblock.record_count, 1);
    }

    #[test]
    fn test_block_id_lookup() {
        let records = vec![VectorRecord {
            id: Some("test_id".to_string()),
            vector: vec![1.0, 2.0],
            ..Default::default()
        }];

        let block = RowBasedDataBlock::new(records, BlockCompressionConfig::default());

        assert!(block.find_record_by_id("test_id").is_some());
        assert!(block.find_record_by_id("non_existent").is_none());
    }
}
