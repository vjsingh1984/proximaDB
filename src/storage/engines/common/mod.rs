// Common Storage Engine Infrastructure
// Shared capabilities and abstractions for all storage engines (SST, SWIFT, VIPER, NOVA)

// Core universal modules
pub mod search_common;
// NOTE: quantization_common and quantization_adapter have been removed
// All engines now use the unified quantization engine from compute module directly
pub mod compression_common;
pub mod search_modes;
pub mod progressive_search;
/// Universal performance optimization module for all storage engines
pub mod performance_optimization;

// Import common types used across the module
use serde::{Deserialize, Serialize};
use crate::core::search::FilterExpression;

/// Filterable metadata column configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterableColumn {
    /// Column name
    pub name: String,
    /// Column data type
    pub data_type: ColumnDataType,
    /// Whether this column is indexed
    pub is_indexed: bool,
    /// Estimated cardinality for optimization
    pub estimated_cardinality: Option<usize>,
}

/// Column data types for type-safe filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnDataType {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Json,
}

/// UNIFIED ZERO-COPY I/O SYSTEM - Complete solution for intelligent cloud storage access
pub mod zero_copy_io_system;

/// Zero-copy reader integration examples and utilities
pub mod zero_copy_reader_integration;

/// FastLanes SIMD-optimized encoding for columnar data within blocks
/// Used by SST, SWIFT, RAPTOR, and PRISM for efficient vector storage
pub mod fastlanes_encoding;

/// Tensor-specific encoding operations (sparse tensors, quantization, transpose)
/// Re-exported through fastlanes_encoding for consolidated access
pub mod fastlanes_tensor_encoding;

// Legacy cache module removed - use zero_copy_io_system instead

// UNIFIED ZERO-COPY I/O SYSTEM - Complete intelligent cloud storage solution
pub use zero_copy_io_system::{
    // Main system components
    ZeroCopyIOSystem, ZeroCopyIOSystemBuilder, WorkloadType,
    
    // Core functionality
    OptimizedIOResult, IOStrategy, IOSavings,
    
    // Configuration
    ZeroCopyIOConfig, MetadataCacheConfig, DownloadOptimizerConfig,
    SizeBasedThresholds, NetworkAdjustments, AccessPredictionConfig,
    
    // Metrics and monitoring
    SystemPerformanceMetrics, MetadataCacheMetrics, DownloadOptimizerMetrics,
    
    // Common traits and types
    MetadataSerializer, EngineMetadata, QueryContext, DataRange,
    FileAccessRequest, RequestPriority, QueryType,
    
    // Access tracking
    AccessPatternTracker, AccessEvent,
    
    // Preset configurations
    presets,
    
    // Constants
    VERSION as ZERO_COPY_IO_VERSION, MAGIC_BYTES,
};
// TODO: Create these modules when needed:
// pub mod validation_common;
// pub mod statistics_common;
// pub mod batch_common;
// pub mod utilities_common;

// Synergy adapters - Bridge universal abstractions with existing implementations
pub mod compression_adapter;

// DEPRECATED: These types now live in unified_query_optimizer
// Removed deprecated metadata_filters module - import from crate::query::unified_query_optimizer instead
// NOTE: Quantization exports removed - use compute::quantization module directly
pub use compression_common::{
    UniversalCompressionConfig, CompressionCapabilities, CompressionStrategy,
    CompressionStats, AdaptiveCompressionSettings,
};
pub use search_modes::{
    UniversalSearchMode, SearchCapabilities, SearchOptimizations,
    CandidateRecord, SearchCandidate, ProgressiveSearchResult,
};

// Universal performance optimization exports
pub use performance_optimization::{
    UniversalPerformanceOptimizer, UniversalOptimizationStrategy, UniversalIOConfig,
    UniversallyOptimized, AccessStats,
};

// Duplicate zero-copy I/O system export removed - see consolidated version above

// Zero-copy reader integration utilities
pub use zero_copy_reader_integration::{
    ZeroCopyReaderIntegration, EnhancedReader, ReaderMetrics,
    EnhancedSstReader, EnhancedParquetReader, EnhancedSwiftReader,
    ReaderMigrationHelper,
};

// Legacy exports removed - use zero_copy_io_system instead
// pub use validation_common::{
//     UniversalValidationConfig, ValidationCapabilities, ValidationReport,
//     RecordValidationError, IntegrityCheck,
// };
// pub use statistics_common::{
//     UniversalStatistics, OperationStatistics, PerformanceMetrics,
//     MemoryUsageReport, AccessPatternAnalysis,
// };
// pub use batch_common::{
//     UniversalBatchConfig, BatchCapabilities, BatchResult,
//     ConcurrencyStrategy, BatchProcessingMode,
// };
// pub use utilities_common::{
//     UniversalUtilities, MemoryEstimator, PerformanceProfiler,
//     FilenameGenerator, PathResolver,
// };

// Synergy adapter re-exports
pub use compression_adapter::{
    UniversalCompressionAdapter, CompressedData, CompressionMetadata,
    CompressionPerformanceStats,
};
// Quantization now handled by unified compute module

// Search pipeline re-exports
pub use search_common::{
    UniversalSearchPipeline, SearchConfig, ProgressiveConfig,
    SearchableFile, SearchableBlock, FileSearcher,
    FilterProcessor, ResultManager,
};

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::{VectorRecord, hardware_capabilities::HardwareCapabilities};
use crate::compute::distance_computation::DistanceMetric;
use crate::core::compression::CompressionAlgorithm;

// Temporary placeholder types until modules are created
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UniversalPerformanceConfig {
    pub max_concurrent_operations: usize,
    pub enable_prefetching: bool,
    pub cache_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UniversalValidationConfig {
    pub validate_on_insert: bool,
    pub strict_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UniversalBatchConfig {
    pub batch_size: usize,
    pub max_batch_memory_mb: usize,
}

/// Universal engine configuration that can be adapted for any storage engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalEngineConfig {
    /// Engine identification
    pub engine_name: String,
    pub engine_type: EngineType,
    pub engine_version: String,
    
    /// Collection configuration
    pub collection_id: String,
    pub dimension: usize,
    pub distance_metric: DistanceMetric,
    
    /// Storage organization
    pub storage_config: UniversalStorageConfig,
    
    /// Performance optimization
    pub performance: UniversalPerformanceConfig,
    
    /// Quantization settings
    pub quantization: crate::compute::quantization::storage_engine::StorageQuantizationConfig,
    
    /// Compression settings
    pub compression: UniversalCompressionConfig,
    
    /// Validation settings
    pub validation: UniversalValidationConfig,
    
    /// Batch operation settings
    pub batch_operations: UniversalBatchConfig,
    
    /// Engine-specific extensions
    pub extensions: HashMap<String, serde_json::Value>,
}

/// Engine type classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EngineType {
    /// Row-based storage (SST, SWIFT)
    RowBased,
    /// Columnar storage (VIPER, NOVA)
    Columnar,
    /// Hybrid storage
    Hybrid,
    /// Memory-only storage
    InMemory,
    /// External storage adapter
    External,
}

/// Universal storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalStorageConfig {
    /// Data organization
    pub organization: StorageOrganization,
    
    /// Block/chunk configuration
    pub block_config: UniversalBlockConfig,
    
    /// Index configuration
    pub index_config: UniversalIndexConfig,
    
    /// Schema configuration
    pub schema_config: UniversalSchemaConfig,
}

/// Storage organization patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageOrganization {
    /// Hierarchical blocks (SuperBlock → Block → Record)
    Hierarchical {
        superblock_size_target: u64,
        blocks_per_superblock: u32,
        records_per_block: u32,
    },
    /// Flat structure (Block → Record)
    Flat {
        target_block_size: u64,
        records_per_block: u32,
    },
    /// Columnar organization (Row Groups → Columns)
    Columnar {
        row_group_size_target: u64,
        rows_per_group: u32,
        column_chunk_size: u32,
    },
    /// Adaptive organization based on workload
    Adaptive {
        workload_hints: Vec<WorkloadHint>,
        adaptation_frequency: u64,
    },
}

/// Workload hints for adaptive storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkloadHint {
    ReadHeavy,
    WriteHeavy,
    ScanHeavy,
    PointQueryHeavy,
    AnalyticsHeavy,
    RealTimeHeavy,
}

/// Universal block configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalBlockConfig {
    /// Block size settings
    pub target_block_size: u64,
    pub min_block_size: u64,
    pub max_block_size: u64,
    
    /// Block alignment
    pub alignment_bytes: usize,
    pub enable_padding: bool,
    
    /// Block-level features
    pub compression: bool,
    pub enable_checksums: bool,
    pub enable_bloom_filters: bool,
    pub enable_statistics: bool,
}

/// Universal index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalIndexConfig {
    /// Index types to enable
    pub index_types: Vec<IndexType>,
    
    /// ID index configuration
    pub id_index: IdIndexConfig,
    
    /// Secondary index configuration
    pub secondary_indexes: Vec<SecondaryIndexConfig>,
    
    /// Bloom filter configuration
    pub bloom_filters: BloomFilterConfig,
    
    /// Index maintenance
    pub maintenance_config: IndexMaintenanceConfig,
}

/// Supported index types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexType {
    /// Primary ID index
    PrimaryId,
    /// Hash index for O(1) lookups
    Hash,
    /// B+ tree for range queries
    BTree,
    /// Bitmap index for categorical data
    Bitmap,
    /// Full-text search index
    FullText,
    /// Vector similarity index
    VectorSimilarity,
    /// Spatial index
    Spatial,
}

/// ID index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdIndexConfig {
    pub compression: bool,
    pub enable_caching: bool,
    pub cache_size_mb: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IdIndexStrategy {
    HashMap,
    BTree,
    Dense,
    Hierarchical,
    Hybrid,
}

/// Secondary index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecondaryIndexConfig {
    pub column_name: String,
    pub index_type: IndexType,
    pub unique: bool,
    pub sparse: bool,
    pub configuration: HashMap<String, serde_json::Value>,
}

/// Bloom filter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterConfig {
    pub enabled: bool,
    pub false_positive_rate: f64,
    pub per_block: bool,
    pub hierarchical: bool,
    pub filter_type: BloomFilterType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BloomFilterType {
    Standard,
    Counting,
    Cuckoo,
    XorFilter,
}

/// Index maintenance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMaintenanceConfig {
    pub auto_rebuild: bool,
    pub rebuild_threshold: f64,
    pub maintenance_interval_ms: u64,
    pub background_maintenance: bool,
}

/// Universal schema configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalSchemaConfig {
    /// Core vector schema
    pub vector_schema: VectorSchemaConfig,
    
    /// Metadata schema
    pub metadata_schema: MetadataSchemaConfig,
    
    /// Schema evolution settings
    pub evolution: SchemaEvolutionConfig,
}

/// Vector schema configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSchemaConfig {
    pub dimension: usize,
    pub normalization: Option<VectorNormalization>,
    pub validation: VectorValidationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorDataType {
    Float32,
    Float16,
    BFloat16,
    Int8,
    Binary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorNormalization {
    L2,
    L1,
    Max,
    None,
}

/// Vector validation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorValidationConfig {
    pub check_dimension: bool,
    pub check_nan: bool,
    pub check_infinity: bool,
    pub check_range: Option<(f32, f32)>,
    pub normalize_on_insert: bool,
}

/// Metadata schema configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSchemaConfig {
    pub filterable_columns: Vec<FilterableColumn>,
    pub searchable_columns: Vec<String>,
    pub required_columns: Vec<String>,
    pub schema_validation: bool,
}

/// Schema evolution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaEvolutionConfig {
    pub allow_schema_changes: bool,
    pub backward_compatibility: bool,
    pub migration_strategy: SchemaMigrationStrategy,
    pub version_tracking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaMigrationStrategy {
    Strict,        // No changes allowed
    Compatible,    // Only compatible changes
    Automatic,     // Automatic migration
    Manual,        // Manual migration required
}

/// Universal engine capabilities trait
pub trait UniversalEngineCapabilities {
    /// Get engine configuration
    fn get_config(&self) -> &UniversalEngineConfig;
    
    /// Get supported engine features
    fn get_capabilities(&self) -> EngineCapabilities;
    
    /// Check if feature is supported
    fn supports_feature(&self, feature: &str) -> bool;
    
    /// Get performance characteristics
    fn get_performance_profile(&self) -> PerformanceProfile;
    
    // TODO: Restore when ResourceRequirements is available
    // fn get_resource_requirements(&self) -> ResourceRequirements;
}

/// Engine capabilities description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineCapabilities {
    /// Core capabilities
    pub supports_id_lookup: bool,
    pub supports_similarity_search: bool,
    pub supports_range_queries: bool,
    pub supports_full_text_search: bool,
    
    /// Advanced capabilities
    pub supports_transactions: bool,
    pub supports_multi_tenancy: bool,
    pub supports_replication: bool,
    pub supports_sharding: bool,
    
    /// Performance capabilities
    pub supports_parallel_operations: bool,
    pub supports_streaming: bool,
    pub supports_batch_operations: bool,
    pub supports_caching: bool,
    
    /// Storage capabilities
    pub supports_compression: bool,
    pub supports_quantization: bool,
    pub supports_encryption: bool,
    pub supports_backup_restore: bool,
    
    /// Integration capabilities
    pub supports_cloud_storage: bool,
    pub supports_external_indexes: bool,
    pub supports_custom_extensions: bool,
    pub supports_metrics_export: bool,
}

/// Performance profile for an engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceProfile {
    /// Throughput characteristics
    pub read_throughput_ops_per_sec: f64,
    pub write_throughput_ops_per_sec: f64,
    pub search_throughput_ops_per_sec: f64,
    
    /// Latency characteristics
    pub avg_read_latency_ms: f64,
    pub avg_write_latency_ms: f64,
    pub avg_search_latency_ms: f64,
    
    /// Scalability characteristics
    pub max_collections: u64,
    pub max_vectors_per_collection: u64,
    pub max_concurrent_operations: usize,
    
    /// Resource characteristics
    pub memory_overhead_percent: f32,
    pub storage_overhead_percent: f32,
    pub cpu_efficiency_score: f32,
}

/// Universal engine operations trait
pub trait UniversalEngineOperations {
    /// Core CRUD operations
    async fn insert_vectors(&self, vectors: Vec<VectorRecord>) -> Result<Vec<String>>;
    async fn get_vectors(&self, ids: &[String]) -> Result<Vec<Option<VectorRecord>>>;
    async fn update_vectors(&self, updates: Vec<(String, VectorRecord)>) -> Result<usize>;
    async fn delete_vectors(&self, ids: &[String]) -> Result<usize>;
    
    /// Search operations
    async fn search_vectors(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<FilterExpression>,
    ) -> Result<Vec<VectorRecord>>;
    
    /// Batch operations
    // TODO: Restore when BatchResult is available  
    // async fn batch_insert(&self, vectors: Vec<VectorRecord>) -> Result<BatchResult>;
    async fn batch_search(
        &self,
        queries: &[Vec<f32>],
        top_k: usize,
        filter: Option<FilterExpression>,
    ) -> Result<Vec<Vec<VectorRecord>>>;
    
    /// Administrative operations
    async fn optimize(&self) -> Result<()>;
    async fn compact(&self) -> Result<()>;
    // TODO: Restore when UniversalStatistics is available
    // async fn get_statistics(&self) -> Result<UniversalStatistics>;
    async fn health_check(&self) -> Result<HealthStatus>;
}

/// Engine health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub is_healthy: bool,
    pub status: HealthLevel,
    pub issues: Vec<HealthIssue>,
    pub performance_score: f32,
    pub resource_utilization: ResourceUtilization,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthLevel {
    Excellent,
    Good,
    Warning,
    Critical,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthIssue {
    pub severity: IssueSeverity,
    pub category: IssueCategory,
    pub description: String,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IssueCategory {
    Performance,
    Resource,
    Data,
    Configuration,
    Infrastructure,
}

/// Resource utilization tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUtilization {
    pub memory_usage_percent: f32,
    pub storage_usage_percent: f32,
    pub cpu_usage_percent: f32,
    pub io_usage_percent: f32,
    pub network_usage_percent: f32,
}

impl Default for UniversalEngineConfig {
    fn default() -> Self {
        Self {
            engine_name: "universal".to_string(),
            engine_type: EngineType::RowBased,
            engine_version: "1.0.0".to_string(),
            collection_id: "default".to_string(),
            dimension: 768,
            distance_metric: DistanceMetric::Cosine,
            storage_config: UniversalStorageConfig::default(),
            performance: UniversalPerformanceConfig::default(),
            quantization: crate::compute::quantization::storage_engine::StorageQuantizationConfig::default(),
            compression: UniversalCompressionConfig::default(),
            validation: UniversalValidationConfig::default(),
            batch_operations: UniversalBatchConfig::default(),
            extensions: HashMap::new(),
        }
    }
}

impl Default for UniversalStorageConfig {
    fn default() -> Self {
        Self {
            organization: StorageOrganization::Hierarchical {
                superblock_size_target: 1024 * 1024 * 1024, // 1GB
                blocks_per_superblock: 64,
                records_per_block: 2000,
            },
            block_config: UniversalBlockConfig::default(),
            index_config: UniversalIndexConfig::default(),
            schema_config: UniversalSchemaConfig::default(),
        }
    }
}

impl Default for UniversalBlockConfig {
    fn default() -> Self {
        Self {
            target_block_size: 16 * 1024 * 1024, // 16MB
            min_block_size: 1024 * 1024,         // 1MB
            max_block_size: 64 * 1024 * 1024,    // 64MB
            alignment_bytes: 4096,
            enable_padding: true,
            compression: true,
            enable_checksums: true,
            enable_bloom_filters: true,
            enable_statistics: true,
        }
    }
}

impl Default for UniversalIndexConfig {
    fn default() -> Self {
        Self {
            index_types: vec![IndexType::PrimaryId, IndexType::Hash],
            id_index: IdIndexConfig::default(),
            secondary_indexes: Vec::new(),
            bloom_filters: BloomFilterConfig::default(),
            maintenance_config: IndexMaintenanceConfig::default(),
        }
    }
}

impl Default for IdIndexConfig {
    fn default() -> Self {
        Self {
            // strategy removed -  IdIndexStrategy::Hybrid,
            compression: true,
            enable_caching: true,
            cache_size_mb: 256,
        }
    }
}

impl Default for BloomFilterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            false_positive_rate: 0.01,
            per_block: true,
            hierarchical: true,
            filter_type: BloomFilterType::Standard,
        }
    }
}

impl Default for IndexMaintenanceConfig {
    fn default() -> Self {
        Self {
            auto_rebuild: true,
            rebuild_threshold: 0.7,
            maintenance_interval_ms: 300000, // 5 minutes
            background_maintenance: true,
        }
    }
}

impl Default for UniversalSchemaConfig {
    fn default() -> Self {
        Self {
            vector_schema: VectorSchemaConfig::default(),
            metadata_schema: MetadataSchemaConfig::default(),
            evolution: SchemaEvolutionConfig::default(),
        }
    }
}

impl Default for VectorSchemaConfig {
    fn default() -> Self {
        Self {
            dimension: 768,
            // data_type removed -  VectorDataType::Float32,
            normalization: Some(VectorNormalization::L2),
            validation: VectorValidationConfig::default(),
        }
    }
}

impl Default for VectorValidationConfig {
    fn default() -> Self {
        Self {
            check_dimension: true,
            check_nan: true,
            check_infinity: true,
            check_range: None,
            normalize_on_insert: false,
        }
    }
}

impl Default for MetadataSchemaConfig {
    fn default() -> Self {
        Self {
            filterable_columns: Vec::new(),
            searchable_columns: Vec::new(),
            required_columns: Vec::new(),
            schema_validation: true,
        }
    }
}

impl Default for SchemaEvolutionConfig {
    fn default() -> Self {
        Self {
            allow_schema_changes: true,
            backward_compatibility: true,
            migration_strategy: SchemaMigrationStrategy::Compatible,
            version_tracking: true,
        }
    }
}

/// Utility functions for universal engine configuration
pub mod utils {
    use super::*;
    
    /// Create configuration optimized for a specific workload
    pub fn create_config_for_workload(
        workload: WorkloadType,
        hardware: &HardwareCapabilities,
    ) -> UniversalEngineConfig {
        let mut config = UniversalEngineConfig::default();
        
        match workload {
            WorkloadType::HighThroughput => {
                config.storage.organization = StorageOrganization::Hierarchical {
                    superblock_size_target: 2 * 1024 * 1024 * 1024, // 2GB
                    blocks_per_superblock: 128,
                    records_per_block: 4000,
                };
                config.performance.max_concurrent_operations = 32;
                config.storage.as_ref().and_then(|s| s.compression.as_ref()).compression_level = 1; // Fast compression
            }
            WorkloadType::LowLatency => {
                config.storage.organization = StorageOrganization::Flat {
                    target_block_size: 4 * 1024 * 1024, // 4MB
                    records_per_block: 500,
                };
                config.performance.enable_prefetching = true;
                config.storage.index_config.id_index.enable_caching = true;
            }
            WorkloadType::Analytics => {
                config.engine_type = EngineType::Columnar;
                config.storage.organization = StorageOrganization::Columnar {
                    row_group_size_target: 256 * 1024 * 1024, // 256MB
                    rows_per_group: 1000000,
                    column_chunk_size: 65536,
                };
                config.storage.as_ref().and_then(|s| s.compression.as_ref()).compression_level = 6; // Better compression
            }
            WorkloadType::RealTime => {
                config.storage.organization = StorageOrganization::Adaptive {
                    workload_hints: vec![WorkloadHint::RealTimeHeavy, WorkloadHint::PointQueryHeavy],
                    adaptation_frequency: 60000, // 1 minute
                };
                config.performance.max_concurrent_operations = 16;
            }
        }
        
        // Hardware-specific optimizations
        if hardware.memory.total_memory / (1024 * 1024 * 1024) > 64 {
            config.performance.cache_size_bytes = 8 * 1024 * 1024 * 1024; // 8GB
        }
        
        if hardware.cpu.physical_cores > 16 {
            config.performance.max_concurrent_operations = hardware.cpu.physical_cores;
        }
        
        config
    }
    
    /// Validate configuration compatibility
    pub fn validate_config_compatibility(config: &UniversalEngineConfig) -> Result<()> {
        // Validate engine type matches storage organization
        match (&config.engine_type, &config.storage.organization) {
            (EngineType::Columnar, StorageOrganization::Columnar { .. }) => {}
            (EngineType::RowBased, StorageOrganization::Hierarchical { .. }) => {}
            (EngineType::RowBased, StorageOrganization::Flat { .. }) => {}
            (EngineType::Hybrid, _) => {} // Hybrid supports any organization
            _ => {
                return Err(anyhow::anyhow!(
                    "Engine type {:?} incompatible with storage organization",
                    config.engine_type
                ));
            }
        }
        
        // Validate dimension consistency
        if config.dimension != config.storage.schema_config.vector_schema.dimension {
            return Err(anyhow::anyhow!("Dimension mismatch in configuration"));
        }
        
        // Validate performance settings
        if config.performance.max_concurrent_operations == 0 {
            return Err(anyhow::anyhow!("Max concurrent operations must be > 0"));
        }
        
        Ok(())
    }
    
    #[derive(Debug, Clone)]
    pub enum WorkloadType {
        HighThroughput,
        LowLatency,
        Analytics,
        RealTime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::utils::*;
    
    #[test]
    fn test_universal_config_creation() {
        let config = UniversalEngineConfig::default();
        
        assert_eq!(config.engine_name, "universal");
        assert_eq!(config.dimension, 768);
        assert!(matches!(config.engine_type, EngineType::RowBased));
        assert!(config.storage.block_config.enable_compression);
    }
    
    #[test]
    fn test_workload_specific_config() {
        let hardware = HardwareCapabilities::detect().unwrap();
        
        let high_throughput_config = create_config_for_workload(
            WorkloadType::HighThroughput,
            &hardware,
        );
        
        if let StorageOrganization::Hierarchical { records_per_block, .. } = 
            high_throughput_config.storage.organization {
            assert_eq!(records_per_block, 4000);
        } else {
            panic!("Expected hierarchical organization for high throughput");
        }
        
        let analytics_config = create_config_for_workload(
            WorkloadType::Analytics,
            &hardware,
        );
        
        assert!(matches!(analytics_config.engine_type, EngineType::Columnar));
    }
    
    #[test]
    fn test_config_validation() {
        let mut config = UniversalEngineConfig::default();
        
        // Valid config should pass
        assert!(validate_config_compatibility(&config).is_ok());
        
        // Mismatched dimensions should fail
        config.storage.schema_config.vector_schema.dimension = 512;
        assert!(validate_config_compatibility(&config).is_err());
        
        // Fix dimension and test invalid concurrent operations
        config.storage.schema_config.vector_schema.dimension = 768;
        config.performance.max_concurrent_operations = 0;
        assert!(validate_config_compatibility(&config).is_err());
    }
    
    #[test]
    fn test_engine_capabilities() {
        let capabilities = EngineCapabilities {
            supports_id_lookup: true,
            supports_similarity_search: true,
            supports_range_queries: false,
            supports_full_text_search: false,
            supports_transactions: true,
            supports_multi_tenancy: false,
            supports_replication: true,
            supports_sharding: false,
            supports_parallel_operations: true,
            supports_streaming: true,
            supports_batch_operations: true,
            supports_caching: true,
            supports_compression: true,
            supports_quantization: true,
            supports_encryption: false,
            supports_backup_restore: true,
            supports_cloud_storage: true,
            supports_external_indexes: false,
            supports_custom_extensions: true,
            supports_metrics_export: true,
        };
        
        assert!(capabilities.supports_id_lookup);
        assert!(capabilities.supports_similarity_search);
        assert!(capabilities.supports_compression);
        assert!(!capabilities.supports_encryption);
    }
}