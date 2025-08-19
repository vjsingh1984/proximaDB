// Shared Block Structures for SST and SWIFT engines
// Common data block, metadata, and layout structures

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::core::{VectorRecord, compression::CompressionAlgorithm};
use crate::storage::engines::sst::bloom_filter::SstableBloomFilter;
// Quantization now handled by unified compute module

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

/// Shared data block structure for row-based engines
#[derive(Debug, Clone)]
pub struct RowBasedDataBlock {
    /// Block identification - u32 supports 4.3 billion blocks (4.3 trillion vectors)
    /// This is sufficient for 34+ petabytes of storage
    /// 
    /// Rationale for 4-byte u32 (future-proofing for larger files):
    /// - With 384D vectors: ~2KB per vector (1.6KB data + 0.4KB metadata)
    /// - 1MB blocks hold ~500 vectors
    /// - 100GB file = 100,000 blocks (well within u32's limit)
    /// - u16 (65,536) would be insufficient for large files
    /// - u32 provides conservative headroom even though files rarely exceed 10GB in practice
    pub block_id: u32,
    
    /// Sequence number for ordering (used by SST and Swift)
    pub sequence_number: u64,
    
    /// Data organization
    pub records: Vec<VectorRecord>,
    /// Quantized vectors using unified engine
    pub quantized_vectors: Option<Vec<Vec<u8>>>,
    /// Quantization level used
    pub quantization_level: Option<crate::compute::quantization::unified::UnifiedQuantizationLevel>,
    
    /// Quantized section for hierarchical storage (SST/Swift specific)
    pub quantized_section: Option<QuantizedSection>,
    
    /// Block metadata
    pub metadata: RowBasedBlockMetadata,
    
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

/// Block metadata shared between engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowBasedBlockMetadata {
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
pub enum ColumnDataType {
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
    /// SuperBlock identification
    pub id: u32,
    pub file_path: String,
    pub timestamp: i64,
    
    /// Organization
    pub blocks: Vec<RowBasedDataBlock>,
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

impl RowBasedDataBlock {
    /// Create a new data block
    pub fn new(
        records: Vec<VectorRecord>,
        compression_config: BlockCompressionConfig,
    ) -> Self {
        let record_count = records.len() as u32;
        // Use a simple counter or provided ID - will be set properly by the writer
        let block_id = 0u32;
        
        // Calculate ID range
        let mut ids: Vec<String> = records
            .iter()
            .filter_map(|r| r.id.as_ref())
            .cloned()
            .collect();
        ids.sort();
        let id_range = if ids.is_empty() {
            ("".to_string(), "".to_string())
        } else {
            (ids[0].clone(), ids[ids.len() - 1].clone())
        };
        
        // Calculate timestamp range
        let timestamps: Vec<i64> = records.iter().map(|r| r.timestamp).collect();
        let timestamp_range = if timestamps.is_empty() {
            (0, 0)
        } else {
            (*timestamps.iter().min().unwrap(), *timestamps.iter().max().unwrap())
        };
        
        // Check for deletes (tombstone records)
        let has_deletes = records.iter().any(|r| r.metadata.iter().any(|kv| 
            kv.key == "_deleted" && kv.value.as_str() == Some("true")
        ));
        
        Self {
            block_id,
            sequence_number: 0, // Will be set by writer
            records: records.clone(),
            quantized_vectors: None,
            quantization_level: None,
            quantized_section: None,
            metadata: RowBasedBlockMetadata {
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
        self.records.iter().find(|r| {
            r.id.as_ref().map(|record_id| record_id == id).unwrap_or(false)
        })
    }
    
    /// Check if block contains ID (using bloom filter if available)
    pub fn contains_id(&self, id: &str) -> bool {
        if let Some(ref bloom) = self.bloom_filter {
            bloom.contains(id.as_bytes())
        } else {
            self.find_record_by_id(id).is_some()
        }
    }
    
    /// Get memory usage estimate
    pub fn memory_usage_bytes(&self) -> usize {
        let records_size = self.records.len() * std::mem::size_of::<VectorRecord>();
        let quantized_size = self.quantized_vectors.as_ref()
            .map(|qv| qv.iter().map(|v| v.len()).sum())
            .unwrap_or(0);
        let metadata_size = std::mem::size_of::<RowBasedBlockMetadata>();
        
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
}

impl SuperBlock {
    /// Create a new SuperBlock
    pub fn new(id: u32, file_path: String) -> Self {
        Self {
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
    pub fn add_block(&mut self, block: RowBasedDataBlock) {
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
            block_alignment_bytes: 4096, // 4KB alignment
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

impl Default for RowBasedBlockMetadata {
    fn default() -> Self {
        Self {
            record_count: 0,
            size_bytes: 0,
            compressed_size: 0,
            timestamp: chrono::Utc::now().timestamp(),
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
        let records = vec![
            VectorRecord {
                id: Some("test_id".to_string()),
                vector: vec![1.0, 2.0],
                ..Default::default()
            },
        ];
        
        let block = RowBasedDataBlock::new(
            records,
            BlockCompressionConfig::default(),
        );
        
        assert!(block.find_record_by_id("test_id").is_some());
        assert!(block.find_record_by_id("non_existent").is_none());
    }
}