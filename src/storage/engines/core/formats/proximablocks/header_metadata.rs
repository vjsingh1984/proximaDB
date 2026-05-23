// Shared Header and Metadata Structures for SST and SWIFT engines

use anyhow::Result;
use proximadb_kernel::uuid::Uuid;
use std::collections::HashMap;

use crate::compute::distance_computation::DistanceMetric;
use proximadb_compression::CompressionAlgorithm;

/// Row-based file header structure
#[derive(Debug, Clone)]
pub struct RowBasedHeader {
    /// File format identification
    pub magic: [u8; 8],
    pub version: u32,
    pub format_version: String,

    /// File identification
    pub file_id: Uuid,
    pub timestamp: i64,
    pub created_by: String,

    /// Engine metadata
    pub engine_metadata: EngineMetadata,

    /// Collection information
    pub collection_metadata: CollectionMetadata,

    /// File layout information
    pub layout_metadata: LayoutMetadata,

    /// Index offsets and sizes
    pub index_metadata: IndexMetadata,

    /// Compression and quantization
    pub compression_metadata: CompressionMetadata,

    /// Version and compatibility
    pub version_info: VersionInfo,

    /// Integrity verification
    pub checksum_config: ChecksumConfig,

    /// Extension points for future features
    pub extensions: HashMap<String, serde_json::Value>,
}

/// Engine-specific metadata
#[derive(Debug, Clone)]
pub struct EngineMetadata {
    pub engine_name: String,
    pub engine_version: String,
    pub engine_build: String,
    pub supported_features: Vec<String>,
    pub compatibility_level: u32,
    pub optimization_hints: OptimizationHints,
}

/// Optimization hints for engine behavior
#[derive(Debug, Clone)]
pub struct OptimizationHints {
    pub prefer_sequential_access: bool,
    pub prefer_random_access: bool,
    pub optimize_for_reads: bool,
    pub optimize_for_writes: bool,
    pub memory_usage_hint: MemoryUsageHint,
    pub io_pattern_hint: IOPatternHint,
}

#[derive(Debug, Clone)]
pub enum MemoryUsageHint {
    Low,      // < 1GB
    Medium,   // 1-8GB
    High,     // 8-32GB
    VeryHigh, // > 32GB
}

#[derive(Debug, Clone)]
pub enum IOPatternHint {
    Sequential,
    Random,
    Mixed,
    Streaming,
    BatchOriented,
}

/// Collection-specific metadata
#[derive(Debug, Clone)]
pub struct CollectionMetadata {
    pub collection_id: String,
    pub collection_name: Option<String>,
    pub dimension: usize,
    pub distance_metric: DistanceMetric,
    pub schema_version: u32,
    pub filterable_columns: Vec<HeaderFilterableColumn>,
    pub collection_statistics: CollectionStatistics,
}

/// Backwards-compat alias for [`HeaderFilterableColumn`].
pub type FilterableColumn = HeaderFilterableColumn;

/// Filterable column definition
#[derive(Debug, Clone)]
pub struct HeaderFilterableColumn {
    pub name: String,
    pub indexed: bool,
    pub bloom_filter_enabled: bool,
    pub statistics: HeaderColumnStatistics,
}

#[derive(Debug, Clone)]
pub enum ColumnData {
    String,
    Integer,
    Float,
    Boolean,
    Timestamp,
    Json,
}

/// Column statistics
#[derive(Debug, Clone)]
pub struct HeaderColumnStatistics {
    pub null_count: u64,
    pub distinct_count: u64,
    pub min_value: Option<serde_json::Value>,
    pub max_value: Option<serde_json::Value>,
    pub average_size: f64,
    pub size_distribution: SizeDistribution,
}

#[derive(Debug, Clone)]
pub struct SizeDistribution {
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
}

/// Collection-level statistics
#[derive(Debug, Clone)]
pub struct CollectionStatistics {
    pub total_records: u64,
    pub total_size_bytes: u64,
    pub average_vector_size: f64,
    pub id_distribution: IdDistribution,
    pub timestamp_range: (i64, i64),
    pub version_range: (i64, i64),
}

#[derive(Debug, Clone)]
pub struct IdDistribution {
    pub id_type: IdType,
    pub min_id: String,
    pub max_id: String,
    pub id_length_distribution: SizeDistribution,
}

#[derive(Debug, Clone)]
pub enum IdType {
    Numeric,
    Uuid,
    String,
    Mixed,
}

/// File layout metadata
#[derive(Debug, Clone)]
pub struct LayoutMetadata {
    /// Hierarchical structure
    pub superblock_count: u32,
    pub blocks_per_superblock: u32,
    pub records_per_block: u32,

    /// Size information
    pub header_size: u64,
    pub data_section_offset: u64,
    pub data_section_size: u64,
    pub index_section_offset: u64,
    pub index_section_size: u64,
    pub footer_offset: u64,
    pub total_file_size: u64,

    /// Block layout
    pub block_layout: BlockLayoutInfo,

    /// Alignment and padding
    pub alignment_bytes: usize,
    pub padding_strategy: PaddingStrategy,
}

#[derive(Debug, Clone)]
pub struct BlockLayoutInfo {
    pub layout_strategy: LayoutStrategy,
    pub target_block_size: u64,
    pub actual_block_sizes: Vec<u64>,
    pub compression_ratios: Vec<f32>,
}

#[derive(Debug, Clone)]
pub enum LayoutStrategy {
    FixedSize,
    VariableSize,
    Adaptive,
    Compressed,
}

#[derive(Debug, Clone)]
pub enum PaddingStrategy {
    None,
    BlockAlign,
    PageAlign,
    CacheLineAlign,
}

/// Index metadata
#[derive(Debug, Clone)]
pub struct IndexMetadata {
    /// ID index information
    pub id_index_offset: u64,
    pub id_index_size: u64,
    pub id_index_type: Index,
    pub id_index_compression: Option<CompressionAlgorithm>,

    /// Bloom filter information
    pub bloom_filter_offset: u64,
    pub bloom_filter_size: u64,
    pub bloom_filter_config: BloomFilterMetadata,

    /// Quantization index information
    pub quantization_index_offset: u64,
    pub quantization_index_size: u64,
    pub quantization_metadata: BlockQuantizationMetadata,

    /// Hierarchical index information
    pub hierarchical_levels: u8,
    pub level_offsets: Vec<u64>,
    pub level_sizes: Vec<u64>,
}

#[derive(Debug, Clone)]
pub enum Index {
    BTree,
    HashMap,
    Dense,
    Hierarchical,
    Hybrid,
}

/// Bloom filter metadata
#[derive(Debug, Clone)]
pub struct BloomFilterMetadata {
    pub filter_type: BloomFilter,
    pub false_positive_rate: f64,
    #[allow(dead_code)]
    pub expected_items: u64,
    pub actual_items: u64,
    pub hash_functions: u32,
    pub bit_array_size: u64,
}

#[derive(Debug, Clone)]
pub enum BloomFilter {
    Standard,
    Counting,
    Cuckoo,
    XorFilter,
}

/// Backwards-compat alias for [`BlockQuantizationMetadata`].
pub type QuantizationMetadata = BlockQuantizationMetadata;

/// Quantization metadata
#[derive(Debug, Clone)]
pub struct BlockQuantizationMetadata {
    pub quantization_enabled: bool,
    pub binary_quantization: Option<BinaryQuantizationMeta>,
    pub int8_quantization: Option<Int8QuantizationMeta>,
    pub pq_quantization: Option<PQQuantizationMeta>,
    pub memory_savings_percent: f32,
    pub reconstruction_error: f32,
}

#[derive(Debug, Clone)]
pub struct BinaryQuantizationMeta {
    pub threshold: f32,
    pub bit_count: u64,
    pub compression_ratio: f32,
}

#[derive(Debug, Clone)]
pub struct Int8QuantizationMeta {
    pub scale: f32,
    pub zero_point: i8,
    pub symmetric: bool,
    pub per_channel: bool,
}

#[derive(Debug, Clone)]
pub struct PQQuantizationMeta {
    pub segments: u8,
    pub bits_per_segment: u8,
    pub codebook_size: u32,
    pub training_vectors: u64,
    pub centroid_norms: Vec<f32>,
}

/// Compression metadata
#[derive(Debug, Clone)]
pub struct CompressionMetadata {
    /// Global compression settings
    pub compression_enabled: bool,
    pub compression_algorithm: CompressionAlgorithm,
    pub compression_level: u8,

    /// Per-section compression
    pub vector_compression: SectionCompressionInfo,
    pub metadata_compression: SectionCompressionInfo,
    pub index_compression: SectionCompressionInfo,

    /// Compression statistics
    pub overall_compression_ratio: f32,
    pub compression_time_ms: u64,
    pub decompression_time_ms: u64,
}

#[derive(Debug, Clone)]
pub struct SectionCompressionInfo {
    pub algorithm: CompressionAlgorithm,
    pub level: u8,
    pub original_size: u64,
    pub compressed_size: u64,
    pub compression_ratio: f32,
}

/// Backwards-compat alias for [`BlockFileMetadata`].
pub type FileMetadata = BlockFileMetadata;

/// File metadata (high-level file information)
#[derive(Debug, Clone)]
pub struct BlockFileMetadata {
    /// Basic file information
    pub file_path: String,
    pub file_size: u64,
    pub timestamp: i64,
    pub modified_at: i64,
    pub accessed_at: i64,

    /// Content information
    pub content_hash: String,
    pub content_type: String,
    pub encoding: String,

    /// Ownership and permissions
    pub owner: String,
    pub permissions: u32,
    pub access_control: AccessControl,

    /// Storage information
    pub storage_location: StorageLocation,
    pub backup_info: Option<BackupInfo>,
    pub replication_info: Option<ReplicationInfo>,
}

#[derive(Debug, Clone)]
pub struct AccessControl {
    pub read_permissions: Vec<String>,
    pub write_permissions: Vec<String>,
    pub admin_permissions: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum StorageLocation {
    Local(String),
    S3 {
        bucket: String,
        key: String,
        region: String,
    },
    GCS {
        bucket: String,
        object: String,
    },
    Azure {
        container: String,
        blob: String,
        account: String,
    },
}

#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub backup_location: StorageLocation,
    pub backup_frequency: BackupFrequency,
    pub last_backup: i64,
    pub backup_retention_days: u32,
}

#[derive(Debug, Clone)]
pub enum BackupFrequency {
    None,
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone)]
pub struct ReplicationInfo {
    pub replication_factor: u8,
    pub replica_locations: Vec<StorageLocation>,
    pub consistency_level: ConsistencyLevel,
}

#[derive(Debug, Clone)]
pub enum ConsistencyLevel {
    Eventual,
    Strong,
    BoundedStaleness(u64), // Max staleness in milliseconds
}

/// Version information
#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub format_version: String,
    pub schema_version: u32,
    pub compatibility_version: u32,
    pub migration_path: Option<MigrationPath>,
    pub deprecation_warnings: Vec<DeprecationWarning>,
}

#[derive(Debug, Clone)]
pub struct MigrationPath {
    pub from_version: String,
    pub to_version: String,
    pub migration_steps: Vec<MigrationStep>,
    pub estimated_migration_time: u64,
}

#[derive(Debug, Clone)]
pub struct MigrationStep {
    pub step_name: String,
    pub step_type: MigrationStepType,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone)]
pub enum MigrationStepType {
    SchemaUpdate,
    DataTransformation,
    IndexRebuild,
    CompressionUpdate,
    MetadataUpdate,
}

#[derive(Debug, Clone)]
pub struct DeprecationWarning {
    pub feature: String,
    pub deprecated_in_version: String,
    pub removal_in_version: String,
    pub replacement: Option<String>,
    pub migration_guide: Option<String>,
}

/// Checksum configuration
#[derive(Debug, Clone)]
pub struct ChecksumConfig {
    pub header_checksum: u32,
    pub data_checksum: u64,
    pub index_checksum: u64,
    pub overall_checksum: u64,
    pub checksum_algorithm: ChecksumAlgorithm,
    pub verify_on_read: bool,
}

#[derive(Debug, Clone)]
pub enum ChecksumAlgorithm {
    CRC32,
    CRC64,
    XXHash,
    Blake3,
    SHA256,
}

impl RowBasedHeader {
    /// Create a new header for SST engine
    pub fn new_sst(collection_id: String, dimension: usize) -> Self {
        Self {
            magic: *b"PROXSST\0",
            version: 1,
            format_version: "1.0.0".to_string(),
            file_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now().timestamp(),
            created_by: "SST Engine".to_string(),
            engine_metadata: EngineMetadata::new_sst(),
            collection_metadata: CollectionMetadata::new(collection_id, dimension),
            layout_metadata: LayoutMetadata::default(),
            index_metadata: IndexMetadata::default(),
            compression_metadata: CompressionMetadata::default(),
            version_info: VersionInfo::default(),
            checksum_config: ChecksumConfig::default(),
            extensions: HashMap::new(),
        }
    }

    /// Create a new header for SWIFT engine
    pub fn new_swift(collection_id: String, dimension: usize) -> Self {
        Self {
            magic: *b"PROXSWF\0",
            version: 1,
            format_version: "1.0.0".to_string(),
            file_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now().timestamp(),
            created_by: "SWIFT Engine".to_string(),
            engine_metadata: EngineMetadata::new_swift(),
            collection_metadata: CollectionMetadata::new(collection_id, dimension),
            layout_metadata: LayoutMetadata::default(),
            index_metadata: IndexMetadata::default(),
            compression_metadata: CompressionMetadata::default(),
            version_info: VersionInfo::default(),
            checksum_config: ChecksumConfig::default(),
            extensions: HashMap::new(),
        }
    }

    /// Validate header integrity
    pub fn validate(&self) -> Result<()> {
        // Check magic bytes
        if &self.magic != b"PROXSST\0" && &self.magic != b"PROXSWF\0" {
            return Err(anyhow::anyhow!("Invalid magic bytes"));
        }

        // Check version compatibility
        if self.version == 0 {
            return Err(anyhow::anyhow!("Invalid version"));
        }

        // Check dimension
        if self.collection_metadata.dimension == 0 {
            return Err(anyhow::anyhow!("Invalid dimension"));
        }

        Ok(())
    }

    /// Calculate header size in bytes
    pub fn serialized_size(&self) -> usize {
        // Estimate based on typical header sizes
        2048 + self.extensions.len() * 128 // Base + extensions
    }
}

impl EngineMetadata {
    pub fn new_sst() -> Self {
        Self {
            engine_name: "SST".to_string(),
            engine_version: "1.0.0".to_string(),
            engine_build: env!("CARGO_PKG_VERSION").to_string(),
            supported_features: vec![
                "id_lookup".to_string(),
                "similarity_search".to_string(),
                "bloom_filters".to_string(),
                "compression".to_string(),
                "quantization".to_string(),
            ],
            compatibility_level: 1,
            optimization_hints: OptimizationHints {
                prefer_sequential_access: true,
                prefer_random_access: false,
                optimize_for_reads: false,
                optimize_for_writes: true,
                memory_usage_hint: MemoryUsageHint::Medium,
                io_pattern_hint: IOPatternHint::Sequential,
            },
        }
    }

    pub fn new_swift() -> Self {
        Self {
            engine_name: "SWIFT".to_string(),
            engine_version: "1.0.0".to_string(),
            engine_build: env!("CARGO_PKG_VERSION").to_string(),
            supported_features: vec![
                "dual_mode".to_string(),
                "progressive_search".to_string(),
                "hierarchical_blocks".to_string(),
                "zero_overhead".to_string(),
                "quantization".to_string(),
                "compression".to_string(),
            ],
            compatibility_level: 1,
            optimization_hints: OptimizationHints {
                prefer_sequential_access: false,
                prefer_random_access: true,
                optimize_for_reads: true,
                optimize_for_writes: false,
                memory_usage_hint: MemoryUsageHint::High,
                io_pattern_hint: IOPatternHint::Mixed,
            },
        }
    }
}

impl CollectionMetadata {
    pub fn new(collection_id: String, dimension: usize) -> Self {
        Self {
            collection_id,
            collection_name: None,
            dimension,
            distance_metric: DistanceMetric::Cosine,
            schema_version: 1,
            filterable_columns: Vec::new(),
            collection_statistics: CollectionStatistics::default(),
        }
    }
}

impl Default for LayoutMetadata {
    fn default() -> Self {
        Self {
            superblock_count: 0,
            blocks_per_superblock: 64,
            records_per_block: 2000,
            header_size: 2048,
            data_section_offset: 2048,
            data_section_size: 0,
            index_section_offset: 0,
            index_section_size: 0,
            footer_offset: 0,
            total_file_size: 0,
            block_layout: BlockLayoutInfo {
                layout_strategy: LayoutStrategy::FixedSize,
                target_block_size: 16 * 1024 * 1024, // 16MB
                actual_block_sizes: Vec::new(),
                compression_ratios: Vec::new(),
            },
            alignment_bytes: 4096,
            padding_strategy: PaddingStrategy::BlockAlign,
        }
    }
}

impl Default for IndexMetadata {
    fn default() -> Self {
        Self {
            id_index_offset: 0,
            id_index_size: 0,
            id_index_type: Index::Hybrid,
            id_index_compression: Some(CompressionAlgorithm::Lz4),
            bloom_filter_offset: 0,
            bloom_filter_size: 0,
            bloom_filter_config: BloomFilterMetadata::default(),
            quantization_index_offset: 0,
            quantization_index_size: 0,
            quantization_metadata: BlockQuantizationMetadata::default(),
            hierarchical_levels: 0,
            level_offsets: Vec::new(),
            level_sizes: Vec::new(),
        }
    }
}

impl Default for BloomFilterMetadata {
    fn default() -> Self {
        Self {
            filter_type: BloomFilter::Standard,
            false_positive_rate: 0.01,
            expected_items: 1000000,
            actual_items: 0,
            hash_functions: 10,
            bit_array_size: 0,
        }
    }
}

impl Default for BlockQuantizationMetadata {
    fn default() -> Self {
        Self {
            quantization_enabled: true,
            binary_quantization: None,
            int8_quantization: None,
            pq_quantization: None,
            memory_savings_percent: 0.0,
            reconstruction_error: 0.0,
        }
    }
}

impl Default for CompressionMetadata {
    fn default() -> Self {
        Self {
            compression_enabled: true,
            compression_algorithm: CompressionAlgorithm::Zstd,
            compression_level: 3,
            vector_compression: SectionCompressionInfo::default(),
            metadata_compression: SectionCompressionInfo::default(),
            index_compression: SectionCompressionInfo::default(),
            overall_compression_ratio: 1.0,
            compression_time_ms: 0,
            decompression_time_ms: 0,
        }
    }
}

impl Default for SectionCompressionInfo {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            level: 3,
            original_size: 0,
            compressed_size: 0,
            compression_ratio: 1.0,
        }
    }
}

impl Default for VersionInfo {
    fn default() -> Self {
        Self {
            format_version: "1.0.0".to_string(),
            schema_version: 1,
            compatibility_version: 1,
            migration_path: None,
            deprecation_warnings: Vec::new(),
        }
    }
}

impl Default for ChecksumConfig {
    fn default() -> Self {
        Self {
            header_checksum: 0,
            data_checksum: 0,
            index_checksum: 0,
            overall_checksum: 0,
            checksum_algorithm: ChecksumAlgorithm::XXHash,
            verify_on_read: true,
        }
    }
}

impl Default for CollectionStatistics {
    fn default() -> Self {
        Self {
            total_records: 0,
            total_size_bytes: 0,
            average_vector_size: 0.0,
            id_distribution: IdDistribution {
                id_type: IdType::String,
                min_id: String::new(),
                max_id: String::new(),
                id_length_distribution: SizeDistribution {
                    p50: 0.0,
                    p90: 0.0,
                    p95: 0.0,
                    p99: 0.0,
                },
            },
            timestamp_range: (0, 0),
            version_range: (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sst_header_creation() {
        let header = RowBasedHeader::new_sst("test_collection".to_string(), 768);

        assert_eq!(&header.magic, b"PROXSST\0");
        assert_eq!(header.engine_metadata.engine_name, "SST");
        assert_eq!(header.collection_metadata.collection_id, "test_collection");
        assert_eq!(header.collection_metadata.dimension, 768);
    }

    #[test]
    fn test_swift_header_creation() {
        let header = RowBasedHeader::new_swift("test_collection".to_string(), 384);

        assert_eq!(&header.magic, b"PROXSWF\0");
        assert_eq!(header.engine_metadata.engine_name, "SWIFT");
        assert_eq!(header.collection_metadata.collection_id, "test_collection");
        assert_eq!(header.collection_metadata.dimension, 384);
    }

    #[test]
    fn test_header_validation() {
        let header = RowBasedHeader::new_sst("test".to_string(), 768);
        assert!(header.validate().is_ok());

        let mut invalid_header = header.clone();
        invalid_header.magic = *b"INVALID\0";
        assert!(invalid_header.validate().is_err());

        let mut zero_dim_header = header.clone();
        zero_dim_header.collection_metadata.dimension = 0;
        assert!(zero_dim_header.validate().is_err());
    }

    #[test]
    fn test_engine_metadata_features() {
        let sst_meta = EngineMetadata::new_sst();
        assert!(
            sst_meta
                .supported_features
                .contains(&"bloom_filters".to_string())
        );
        assert!(sst_meta.optimization_hints.prefer_sequential_access);

        let swift_meta = EngineMetadata::new_swift();
        assert!(
            swift_meta
                .supported_features
                .contains(&"dual_mode".to_string())
        );
        assert!(swift_meta.optimization_hints.prefer_random_access);
    }
}
