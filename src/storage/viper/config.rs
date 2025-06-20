//! VIPER Configuration and Builder Patterns
//!
//! This module contains configuration structures and builder patterns
//! for creating flexible and extensible VIPER configurations.

use super::schema::ViperSchemaStrategy;
use crate::schema_types::CollectionConfig;
use chrono::Duration;

/// VIPER configuration for intelligent partitioning
#[derive(Debug, Clone)]
pub struct ViperConfig {
    /// Enable VIPER clustering (groups similar vectors together)
    pub enable_clustering: bool,

    /// Number of clusters for partitioning (0 = auto-detect)
    pub cluster_count: usize,

    /// Minimum vectors per partition
    pub min_vectors_per_partition: usize,

    /// Maximum vectors per partition  
    pub max_vectors_per_partition: usize,

    /// Enable dictionary encoding for vector IDs
    pub enable_dictionary_encoding: bool,

    /// Target compression ratio (0.0 = no compression, 1.0 = maximum)
    pub target_compression_ratio: f64,

    /// Parquet block size in bytes
    pub parquet_block_size: usize,

    /// Row group size for Parquet files
    pub row_group_size: usize,

    /// Enable column statistics
    pub enable_column_stats: bool,

    /// Enable bloom filters for string columns
    pub enable_bloom_filters: bool,

    /// TTL (Time-To-Live) configuration
    pub ttl_config: TTLConfig,

    /// Compaction trigger configuration
    pub compaction_config: CompactionConfig,
}

/// Compaction trigger configuration for background file optimization
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Enable automatic compaction
    pub enabled: bool,

    /// Minimum number of files to trigger compaction (MVP default: 8)
    pub min_files_for_compaction: usize,

    /// Maximum average file size for compaction trigger (in KB for granularity)
    pub max_avg_file_size_kb: usize,

    // No inspection interval needed - compaction checks happen immediately after flush
    /// Maximum files to compact in a single operation
    pub max_files_per_compaction: usize,

    /// Target file size after compaction (in MB)
    pub target_file_size_mb: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_files_for_compaction: 2, // Testing: >2 files trigger compaction
            max_avg_file_size_kb: 16 * 1024, // Testing: <16MB (16384 KB) average triggers compaction
            max_files_per_compaction: 5,     // Testing: smaller batches
            target_file_size_mb: 64,         // Testing: smaller target size
        }
    }
}

/// TTL (Time-To-Live) configuration for automatic vector expiration
#[derive(Debug, Clone)]
pub struct TTLConfig {
    /// Enable TTL functionality
    pub enabled: bool,

    /// Default TTL duration for vectors (None = no default TTL)
    pub default_ttl: Option<Duration>,

    /// Background cleanup interval
    pub cleanup_interval: Duration,

    /// Maximum number of expired vectors to clean per batch
    pub max_cleanup_batch_size: usize,

    /// Enable TTL-based Parquet file filtering during reads
    pub enable_file_level_filtering: bool,

    /// Minimum expiration age before a Parquet file is considered for deletion
    pub min_file_expiration_age: Duration,
}

impl Default for TTLConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl: None, // No default TTL - vectors live forever unless explicitly set
            cleanup_interval: Duration::hours(1), // Run cleanup every hour
            max_cleanup_batch_size: 10_000,
            enable_file_level_filtering: true,
            min_file_expiration_age: Duration::days(1), // Wait 1 day before deleting files
        }
    }
}

impl Default for ViperConfig {
    fn default() -> Self {
        Self {
            enable_clustering: true,
            cluster_count: 0, // Auto-detect
            min_vectors_per_partition: 1000,
            max_vectors_per_partition: 100_000,
            enable_dictionary_encoding: true,
            target_compression_ratio: 0.8,
            parquet_block_size: 256 * 1024 * 1024, // 256MB
            row_group_size: 50_000,
            enable_column_stats: true,
            enable_bloom_filters: true,
            ttl_config: TTLConfig::default(),
            compaction_config: CompactionConfig::default(),
        }
    }
}

/// Builder Pattern: For complex schema configuration
#[derive(Debug)]
pub struct ViperSchemaBuilder {
    filterable_fields: Vec<String>,
    enable_ttl: bool,
    enable_extra_meta: bool,
    vector_dimension: Option<usize>,
    schema_version: u32,
    compression_level: Option<i32>,
}

impl ViperSchemaBuilder {
    pub fn new() -> Self {
        Self {
            filterable_fields: Vec::new(),
            enable_ttl: false,
            enable_extra_meta: true,
            vector_dimension: None,
            schema_version: 1,
            compression_level: None,
        }
    }

    pub fn with_filterable_fields(mut self, fields: Vec<String>) -> Self {
        self.filterable_fields = fields;
        self
    }

    pub fn with_ttl(mut self, enable: bool) -> Self {
        self.enable_ttl = enable;
        self
    }

    pub fn with_extra_meta(mut self, enable: bool) -> Self {
        self.enable_extra_meta = enable;
        self
    }

    pub fn with_vector_dimension(mut self, dim: usize) -> Self {
        self.vector_dimension = Some(dim);
        self
    }

    pub fn with_schema_version(mut self, version: u32) -> Self {
        self.schema_version = version;
        self
    }

    pub fn with_compression_level(mut self, level: i32) -> Self {
        self.compression_level = Some(level);
        self
    }

    pub fn build(self, collection_config: &CollectionConfig) -> ViperSchemaStrategy {
        ViperSchemaStrategy::from_builder(self, collection_config)
    }

    // Internal getters for ViperSchemaStrategy
    pub(super) fn get_schema_version(&self) -> u32 {
        self.schema_version
    }

    pub(super) fn get_enable_ttl(&self) -> bool {
        self.enable_ttl
    }

    pub(super) fn get_enable_extra_meta(&self) -> bool {
        self.enable_extra_meta
    }
}

impl Default for ViperSchemaBuilder {
    fn default() -> Self {
        Self::new()
    }
}
